//! reprise — semantic-ish code duplicate detection (a common and central
//! use case: duplication and divergence from LLM coding agents).
//! Spec: docs/PLAN.md (Rev 9); deviations: DECISIONS.md.

pub mod api;
pub mod au;
pub mod baseline;
pub mod cache;
pub mod check;
pub mod config;
pub mod fingerprint;
pub mod fold;
pub mod formats;
pub mod frontend;
pub mod group;
pub mod inline;
pub mod ir;
pub mod lang;
pub mod matchtree;
pub mod normalize;
pub mod report;
pub mod seq;
pub mod stream;
pub mod tree;
pub mod unit;
pub mod walk;

pub use config::Config;
pub use report::ScanReport;
pub use unit::{Unit, units_from_source};

use crate::report::{Group, Tier};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

/// A walked-and-extracted corpus: every [`Unit`] under `root` (respecting `Config`'s
/// walk excludes), the parallel pre-normalization `raw_trees` the inliner needs
/// (index-aligned with the leading *plain* units — `units[..raw_trees.len()]` — since
/// this accessor covers only extraction; inline-expanded variants are `scan()`'s own
/// later phase and carry no raw tree), each file's [`unit::InternalRepeat`] findings,
/// every scanned file's full source text (keyed by the same `PathBuf` a `Unit::file`
/// carries, for line-span rendering), and the subset of [`report::Stats`] this phase
/// fills (`files_scanned`, `files_skipped_generated`, `files_unreadable`,
/// `suppressed_units`, `cache_hits`/`cache_misses`, `units_indexed`,
/// `parse_degraded_units`, `test_units`) — every other `Stats` field is left at its
/// `Default` for the caller to fill in as it goes.
pub struct CorpusUnits {
    pub units: Vec<Unit>,
    pub raw_trees: Vec<tree::NormNode>,
    pub repeats: Vec<unit::InternalRepeat>,
    pub sources: HashMap<std::path::PathBuf, String>,
    pub stats: report::Stats,
}

/// Walk `root` (respecting `config`'s excludes) and extract every unit, reusing the
/// D19 per-file cache (a warm hit is byte-identical to a cold extraction by contract,
/// `src/cache.rs`). **This is THE sanctioned way to obtain corpus units** — `scan()`
/// calls it for its extract half; any other consumer (the frozen-index builder,
/// reprise-mcp's `find_similar`) must route through here too instead of hand-rolling
/// a walk+extract loop, so every caller shares one walk/cache/extract path and its
/// D19 cache-reuse (docs/SERVERS.md §7 M1, DECISIONS.md D46).
pub fn corpus_units(root: &Path, config: &Config) -> anyhow::Result<CorpusUnits> {
    let files = walk::collect_files(root, config)?;

    // `normalizer = "historical"` has no `LanguageProfile` for C (WP-K1a: C is IR-frontend
    // only — CLAUDE.md, the historical per-grammar layer is retired for new languages).
    // Fail loudly, up front, with a clean actionable error — before spending any work — rather
    // than let a per-file historical extraction reach `Lang::C.profile()` (which panics; see
    // `lang::Lang::profile`/`unit::extract_file_units_keep_raw`, the lower-level belt-and-
    // suspenders guard for callers that bypass this entry point).
    if config.normalize.normalizer == config::Normalizer::Historical
        && let Some((path, lang)) = files
            .iter()
            .find(|(_, lang)| !lang.has_historical_profile())
    {
        anyhow::bail!(
            "`normalizer = \"historical\"` does not support {lang:?} ({}) — C is IR-frontend-\
             only; select `normalizer = \"ir\"` (the default) or exclude these files from the \
             scan",
            path.display(),
        );
    }

    enum FileOutcome {
        /// (extraction, source text, cache hit)
        Units(unit::FileUnits, String, bool),
        SkippedGenerated,
        Unreadable,
    }

    let outcomes: Vec<(std::path::PathBuf, FileOutcome)> = files
        .par_iter()
        .map(|(path, lang)| {
            let outcome = match std::fs::read_to_string(path) {
                Err(_) => FileOutcome::Unreadable,
                Ok(src) if walk::is_generated(&src, config) => FileOutcome::SkippedGenerated,
                Ok(src) => {
                    // D8/D19 version-keyed per-file cache: a hit is
                    // byte-identical to a cold extraction by contract.
                    let key = cache::key(&baseline::relative_file(path, root), &src, config);
                    let cache_root = config.cache.shared_root.as_deref().unwrap_or(root);
                    let cached = config
                        .cache
                        .enabled
                        .then(|| cache::load(cache_root, key, path))
                        .flatten();
                    match cached {
                        Some(extracted) => FileOutcome::Units(extracted, src, true),
                        None => {
                            let extracted =
                                unit::extract_file_units_keep_raw(path, &src, *lang, config);
                            if config.cache.enabled {
                                cache::store(cache_root, key, &extracted);
                            }
                            FileOutcome::Units(extracted, src, false)
                        }
                    }
                }
            };
            (path.clone(), outcome)
        })
        .collect();

    let mut units: Vec<Unit> = Vec::new();
    let mut raw_trees: Vec<tree::NormNode> = Vec::new();
    let mut repeats = Vec::new();
    let mut sources: HashMap<std::path::PathBuf, String> = HashMap::new();
    let mut stats = report::Stats::default();
    for (path, outcome) in outcomes {
        match outcome {
            FileOutcome::Units(mut extracted, src, cache_hit) => {
                stats.files_scanned += 1;
                stats.suppressed_units += extracted.suppressed;
                if cache_hit {
                    stats.cache_hits += 1;
                } else {
                    stats.cache_misses += 1;
                }
                units.append(&mut extracted.units);
                raw_trees.append(&mut extracted.raw_trees);
                repeats.append(&mut extracted.repeats);
                sources.insert(path, src);
            }
            FileOutcome::SkippedGenerated => stats.files_skipped_generated += 1,
            FileOutcome::Unreadable => stats.files_unreadable += 1,
        }
    }
    stats.units_indexed = units.len();
    stats.parse_degraded_units = units.iter().filter(|u| u.parse_degraded).count();
    stats.test_units = units.iter().filter(|u| u.is_test).count();

    Ok(CorpusUnits {
        units,
        raw_trees,
        repeats,
        sources,
        stats,
    })
}

/// Full-repo scan (spec §2 `reprise scan`).
pub fn scan(root: &Path, config: &Config) -> anyhow::Result<ScanReport> {
    let started = Instant::now();
    let CorpusUnits {
        mut units,
        raw_trees,
        repeats: internal_repeats,
        sources,
        mut stats,
    } = corpus_units(root, config)?;
    let plain_count = units.len();

    let mut phase_started = started;
    let mut phase = |name: &str, stats: &mut report::Stats, now: Instant| {
        stats
            .phase_ms
            .insert(name.to_string(), (now - phase_started).as_millis() as u64);
        phase_started = now;
    };
    phase("extract", &mut stats, Instant::now());

    // ---- P5: best-effort inliner (spec §5.4) — variants appended, tagged ----
    if config.inline.enabled {
        let table = inline::DefTable::build(&units, &raw_trees, config);
        stats.scc_units = table.scc_unit_count();
        let expansions: Vec<inline::Expansion> = (0..plain_count)
            .into_par_iter()
            .map(|i| inline::expand_unit(i, &raw_trees[i], &units, &table, config))
            .collect();

        /// Per-unit result of the tail below: `finish_variant` is a full
        /// re-normalization + merkle, independent per index (reads only
        /// `units[i]` and this unit's own `exp`, writes nothing shared), so
        /// it maps in parallel like `FileOutcome` above. The stats fields are
        /// summed in the sequential fold that follows, over the
        /// order-preserving `collect()` — identical to the serial form.
        struct VariantOutcome {
            ambiguity_skips: u32,
            calls_inlined: u32,
            variant: Option<Unit>,
        }
        let outcomes: Vec<VariantOutcome> = expansions
            .into_par_iter()
            .enumerate()
            .map(|(i, exp)| {
                let ambiguity_skips = exp.ambiguity_skips;
                if exp.calls_inlined == 0 {
                    return VariantOutcome {
                        ambiguity_skips,
                        calls_inlined: 0,
                        variant: None,
                    };
                }
                // calls_inlined > 0 (checked above) guarantees expand_unit produced
                // a spliced tree (see inline::any_resolvable_call / Expansion::tree).
                let tree = exp
                    .tree
                    .expect("calls_inlined > 0 implies expand_unit produced a tree");
                let Some(mut variant) = unit::finish_variant(i, &units[i], tree, config) else {
                    // inlining changed nothing post-normalization
                    return VariantOutcome {
                        ambiguity_skips,
                        calls_inlined: 0,
                        variant: None,
                    };
                };
                let expanded_fps: Vec<u128> = exp
                    .expanded_units
                    .iter()
                    .map(|&u| units[u].fingerprint)
                    .collect();
                if expanded_fps.contains(&variant.fingerprint) {
                    // D3: a pure wrapper's variant IS its callee
                    return VariantOutcome {
                        ambiguity_skips,
                        calls_inlined: 0,
                        variant: None,
                    };
                }
                let tag = variant.variant.as_mut().expect("finish_variant tags");
                tag.chain = exp.chain;
                tag.expanded_fps = expanded_fps;
                tag.scc = exp.scc;
                VariantOutcome {
                    ambiguity_skips,
                    calls_inlined: exp.calls_inlined,
                    variant: Some(variant),
                }
            })
            .collect();
        let mut variants: Vec<Unit> = Vec::new();
        for outcome in outcomes {
            stats.ambiguity_skips += outcome.ambiguity_skips as usize;
            stats.calls_inlined += outcome.calls_inlined as usize;
            if let Some(variant) = outcome.variant {
                variants.push(variant);
            }
        }
        stats.inline_variants = variants.len(); // ≤1 per unit: the §12 cap
        units.extend(variants);
    }
    drop(raw_trees);
    phase("inline", &mut stats, Instant::now());

    // ---- exact tier (P7 bucketing; plain) + inline-assisted exact matches ----
    let (exact, below_floor) = group::build_exact_groups(&units, config);
    stats.units_below_floor = below_floor;
    let inline_exact = group::build_inline_exact_groups(&units, config);

    let pair_key = |set: &mut HashSet<(usize, usize)>, indices: &[usize]| {
        for i in 0..indices.len() {
            for j in i + 1..indices.len() {
                let (a, b) = (indices[i].min(indices[j]), indices[i].max(indices[j]));
                set.insert((a, b));
            }
        }
    };

    // Pairs owned by exact tiers — near-tier inline pairs and api pairs on the
    // same bases would be echoes.
    let mut exact_pairs: HashSet<(usize, usize)> = HashSet::new();
    for (_, indices) in &exact {
        pair_key(&mut exact_pairs, indices);
    }
    for (_, indices) in &inline_exact {
        pair_key(&mut exact_pairs, indices);
    }

    phase("exact", &mut stats, Instant::now());

    // ---- near-miss tier (retrieval → histogram → AU; plain + variants) ----
    let near = matchtree::find_near_groups(&units, config, &mut stats.retrieval, &exact_pairs);
    phase("near", &mut stats, Instant::now());

    // Pairs co-grouped by a tree tier — sequence regions between them are
    // echoes. (Inline-assisted groups do NOT suppress regions: an exact plain
    // token run is harder evidence than an inline-expanded match, D10.)
    let mut co_grouped: HashSet<(usize, usize)> = HashSet::new();
    for (_, indices) in &exact {
        pair_key(&mut co_grouped, indices);
    }
    for g in &near {
        // Weak-similarity findings are verbose-only; they must not subsume a
        // harder exact-region finding between the same units.
        if g.tier == Tier::NearNormalized {
            pair_key(&mut co_grouped, &g.member_units);
        }
    }

    // api-profile exclusion (spec §5.7): every stronger tier's pairs.
    let mut api_excluded: HashSet<(usize, usize)> = exact_pairs.clone();
    for g in &near {
        if matches!(g.tier, Tier::NearNormalized | Tier::InlineAssisted) {
            pair_key(&mut api_excluded, &g.member_units);
        }
    }

    // ---- sequence tier (per language partition; plain units only) ----
    let mut region_groups: Vec<(Group, bool)> = Vec::new();
    {
        let mut langs: Vec<lang::Lang> = units.iter().map(|u| u.lang).collect();
        langs.sort();
        langs.dedup();
        for l in langs {
            let part: Vec<usize> = (0..units.len())
                .filter(|&i| units[i].lang == l && units[i].variant.is_none())
                .collect();
            if part.len() < 2 {
                continue;
            }
            let refs: Vec<&Unit> = part.iter().map(|&i| &units[i]).collect();
            let corpus = stream::build_corpus(&refs);
            for r in seq::maximal_repeats(&corpus, config.thresholds.min_seq_tokens as usize) {
                stats.sequence_regions_found += 1;
                let ua = part[r.unit_a as usize];
                let ub = part[r.unit_b as usize];
                let key = (ua.min(ub), ua.max(ub));
                if co_grouped.contains(&key) {
                    stats.sequence_regions_subsumed += 1;
                    continue;
                }
                // Nested units (an inner fn and its parent) serialize the same
                // source text twice — a "repeat" between them is an artifact.
                if units[ua].file == units[ub].file {
                    let (s1, e1) = units[ua].byte_span;
                    let (s2, e2) = units[ub].byte_span;
                    if s1 < e2 && s2 < e1 {
                        stats.sequence_regions_subsumed += 1;
                        continue;
                    }
                }
                api_excluded.insert(key);
                let mem = |u: usize, range: (usize, usize)| {
                    let unit = &units[u];
                    let src = sources.get(&unit.file).map(String::as_str).unwrap_or("");
                    let (mut lo, mut hi) = (u32::MAX, 0u32);
                    for t in range.0..range.1 {
                        let (s, e) = corpus.spans[t];
                        if (s, e) != (0, 0) {
                            lo = lo.min(s);
                            hi = hi.max(e);
                        }
                    }
                    report::Member {
                        file: unit.file.clone(),
                        lang: unit.lang.name().to_string(),
                        name: unit.name.clone(),
                        line_span: (unit::byte_to_line(src, lo), unit::byte_to_line(src, hi)),
                        parse_degraded: unit.parse_degraded,
                    }
                };
                // Spec §2: a region's baseline key is the hash of the run's
                // token content (interner-independent, both sides identical).
                let mut fp_buf = Vec::with_capacity(8 * r.len);
                for t in r.tok_a.0..r.tok_a.1 {
                    fp_buf.extend_from_slice(&corpus.key_hash[t].to_le_bytes());
                }
                region_groups.push((
                    Group {
                        id: String::new(),
                        tier: Tier::ExactRegion,
                        fingerprint: fingerprint::hex(xxhash_rust::xxh3::xxh3_128(&fp_buf)),
                        token_count: r.len as u32,
                        value: r.len as f64,
                        note: None,
                        divergence: 0.0,
                        template: None,
                        inline_chain: None,
                        members: vec![mem(ua, r.tok_a), mem(ub, r.tok_b)],
                    },
                    units[ua].is_test && units[ub].is_test,
                ));
            }
        }
    }
    phase("sequence", &mut stats, Instant::now());

    // ---- api-profile tier (spec §5.7): own section, never fails CI ----
    let mut api_groups =
        api::find_api_groups(&units, config, &api_excluded, &mut stats.api_signatures);
    phase("api", &mut stats, Instant::now());

    // ---- assemble, apply test policy, rank ----
    let mut main: Vec<Group> = Vec::new();
    let all_test = |members: &[usize]| members.iter().all(|&i| units[i].is_test);

    let mut test_groups: Vec<Group> = Vec::new();
    let push_policied =
        |g: Group, is_test_group: bool, main: &mut Vec<Group>, tg: &mut Vec<Group>| match (
            is_test_group,
            config.tests.mode.as_str(),
        ) {
            (true, "separate") => tg.push(g),
            (true, "exclude") => {}
            _ => main.push(g),
        };

    for (g, indices) in exact.into_iter().chain(inline_exact) {
        let t = all_test(&indices);
        push_policied(g, t, &mut main, &mut test_groups);
    }
    // An internal repeat's (file, unit_name) pins a specific enclosing unit;
    // the previous `units.iter().any(...)` re-scanned every unit per repeat —
    // O(repeats × units), which dominated the assemble phase at kernel scale
    // (98% of assemble time on the drivers/net corpus). Index test units by
    // (file, name) once — same predicate, now an O(1) hash lookup per repeat.
    let test_unit_keys = test_unit_key_index(&units, internal_repeats.is_empty());
    for r in &internal_repeats {
        let is_test_repeat = test_unit_keys.contains(&(r.file.as_path(), r.unit_name.as_str()));
        // Spec §2: internal-repeat key = (unit fingerprint, template hash).
        let mut fp_buf = Vec::with_capacity(32);
        fp_buf.extend_from_slice(&r.unit_fp.to_le_bytes());
        fp_buf.extend_from_slice(&r.template_hash.to_le_bytes());
        let g = Group {
            id: String::new(),
            tier: Tier::InternalRepeat,
            fingerprint: fingerprint::hex(xxhash_rust::xxh3::xxh3_128(&fp_buf)),
            token_count: r.template_tokens,
            value: f64::from(r.count - 1) * f64::from(r.template_tokens),
            note: None,
            divergence: 0.0,
            template: None,
            inline_chain: None,
            members: vec![report::Member {
                file: r.file.clone(),
                lang: r.lang.name().to_string(),
                name: r.unit_name.clone(),
                line_span: r.line_span,
                parse_degraded: false,
            }],
        };
        push_policied(g, is_test_repeat, &mut main, &mut test_groups);
    }
    let mut weak_groups: Vec<Group> = Vec::new();
    for ng in near {
        let member_refs: Vec<&Unit> = ng.member_units.iter().map(|&i| &units[i]).collect();
        let g = Group {
            id: String::new(),
            tier: ng.tier,
            // Spec §2/§6.1: the AU template hash is the group's stable key.
            fingerprint: fingerprint::hex(ng.template_hash),
            token_count: ng.template_tokens,
            value: group::consolidation_value(
                &member_refs,
                group::substantive_tokens(ng.template_tokens, ng.template_boilerplate),
            ),
            note: None,
            divergence: ng.divergence,
            template: Some(ng.template),
            inline_chain: (!ng.inline_chains.is_empty()).then_some(ng.inline_chains),
            members: member_refs.iter().map(|u| group::member_of(u)).collect(),
        };
        match ng.tier {
            Tier::WeakSimilarity => weak_groups.push(g),
            _ => {
                let t = all_test(&ng.member_units);
                push_policied(g, t, &mut main, &mut test_groups);
            }
        }
    }
    // Cross-granularity dedup: plain-unit and inline-variant streams can carry
    // the same duplication, yielding overlapping regions for one file pair —
    // keep the largest covering run only. Candidates are bucketed by file
    // pair (member order is unit-index order, hence consistent), so the check
    // stays linear-ish when a repetitive corpus yields thousands of regions.
    region_groups.sort_by_key(|(g, _)| std::cmp::Reverse(g.token_count));
    let mut kept_regions: Vec<(Group, bool)> = Vec::new();
    let mut regions_by_pair: HashMap<(std::path::PathBuf, std::path::PathBuf), Vec<usize>> =
        HashMap::new();
    for (g, is_test_region) in region_groups {
        let key = (g.members[0].file.clone(), g.members[1].file.clone());
        let covered = regions_by_pair.get(&key).is_some_and(|kept| {
            kept.iter().any(|&k| {
                let (h, _) = &kept_regions[k];
                g.members.iter().all(|gm| {
                    h.members.iter().any(|hm| {
                        hm.file == gm.file
                            && hm.line_span.0 <= gm.line_span.0
                            && gm.line_span.1 <= hm.line_span.1
                    })
                })
            })
        });
        if covered {
            stats.sequence_regions_subsumed += 1;
        } else {
            regions_by_pair
                .entry(key)
                .or_default()
                .push(kept_regions.len());
            kept_regions.push((g, is_test_region));
        }
    }
    for (g, is_test_region) in kept_regions {
        push_policied(g, is_test_region, &mut main, &mut test_groups);
    }

    // Cross-tier membership subsumption (subset groups add nothing).
    let before = main.len();
    main = group::dedupe_subset_groups(main);
    stats.groups_subsumed_subset = before - main.len();
    stats.regions_substantial = main
        .iter()
        .filter(|g| {
            g.tier == Tier::ExactRegion && g.token_count >= config.report.micro_region_tokens
        })
        .count();

    rank(&mut main, "g");
    rank(&mut test_groups, "t");
    rank(&mut api_groups, "a");
    rank(&mut weak_groups, "w");
    for g in &main {
        *stats
            .findings_by_tier
            .entry(g.tier.to_string())
            .or_insert(0) += 1;
    }

    // ---- duplication ratios (spec §6.1 metrics; context, not primary
    // evidence) computed over the scanned corpus and the MAIN section only —
    // the actionable, CI-gating findings. Line union stays ≤ total (∈[0,100]);
    // token mass is the removable-copy count in D1 normalized tokens. D28. ----
    stats.total_lines = sources.values().map(|src| src.lines().count()).sum();
    stats.total_tokens = units
        .iter()
        .filter(|u| u.variant.is_none())
        .map(|u| u64::from(u.token_count))
        .sum();
    let mut covered: HashMap<&std::path::Path, HashSet<u32>> = HashMap::new();
    for g in &main {
        for m in &g.members {
            let set = covered.entry(m.file.as_path()).or_default();
            for line in m.line_span.0..=m.line_span.1 {
                set.insert(line);
            }
        }
        stats.duplicated_tokens +=
            u64::from(g.token_count) * (g.members.len().saturating_sub(1)) as u64;
    }
    stats.duplicated_lines = covered.values().map(HashSet::len).sum();
    stats.duplicated_lines_pct = if stats.total_lines > 0 {
        100.0 * stats.duplicated_lines as f64 / stats.total_lines as f64
    } else {
        0.0
    };
    stats.duplicated_tokens_pct = if stats.total_tokens > 0 {
        100.0 * stats.duplicated_tokens as f64 / stats.total_tokens as f64
    } else {
        0.0
    };
    stats.clones_per_kloc = if stats.total_lines > 0 {
        1000.0 * main.len() as f64 / stats.total_lines as f64
    } else {
        0.0
    };

    phase("assemble", &mut stats, Instant::now());
    stats.duration_ms = started.elapsed().as_millis() as u64;

    // Plain-unit coordinates for check mode's baseline-member mapping.
    let unit_index: Vec<report::UnitSummary> = units
        .iter()
        .filter(|u| u.variant.is_none())
        .map(|u| report::UnitSummary {
            file: u.file.clone(),
            name: u.name.clone(),
            line_span: u.line_span,
            fingerprint: fingerprint::hex(u.fingerprint),
            accept_drift: u.accept_drift,
        })
        .collect();

    Ok(ScanReport {
        groups: main,
        test_groups,
        api_groups,
        weak_groups,
        stats,
        unit_index,
    })
}

fn rank(groups: &mut [Group], prefix: &str) {
    groups.sort_by(|a, b| {
        b.value
            .partial_cmp(&a.value)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.token_count.cmp(&a.token_count))
            .then_with(|| a.members[0].file.cmp(&b.members[0].file))
    });
    for (i, group) in groups.iter_mut().enumerate() {
        group.id = format!("{prefix}{}", i + 1);
    }
}

/// Index of (file, name) pairs belonging to some `is_test` unit — the perf
/// fix for the internal-repeat test-policy scan in `scan()`, which used to
/// re-run `units.iter().any(...)` per repeat (O(repeats × units); dominated
/// the assemble phase at kernel scale). Same predicate, memoized once:
/// O(units) to build, O(1) per lookup. `skip` (true when there are no
/// repeats to look up) avoids the O(units) build entirely when it would go
/// unused.
fn test_unit_key_index(units: &[Unit], skip: bool) -> HashSet<(&Path, &str)> {
    if skip {
        return HashSet::new();
    }
    units
        .iter()
        .filter(|u| u.is_test)
        .map(|u| (u.file.as_path(), u.name.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn mk_unit(file: &str, name: &str, is_test: bool) -> Unit {
        Unit {
            file: PathBuf::from(file),
            lang: lang::Lang::Rust,
            name: name.to_string(),
            byte_span: (0, 0),
            line_span: (0, 0),
            token_count: 10,
            parse_degraded: false,
            is_test,
            accept_drift: false,
            fingerprint: 0,
            tree: tree::NormNode::new("Unit", None, (0, 0), Vec::new()),
            variant: None,
        }
    }

    /// The original O(units) per-lookup scan, kept only as the
    /// differential-test oracle for `test_unit_key_index`.
    fn brute_is_test_repeat(units: &[Unit], file: &Path, name: &str) -> bool {
        units
            .iter()
            .any(|u| u.file == file && u.name == name && u.is_test)
    }

    #[test]
    fn test_unit_key_index_matches_brute_force_oracle() {
        // Deterministic LCG (no external rand dependency), mirroring the
        // `contained_flags` oracle test in group.rs: many random
        // file/name/is_test unit shapes, checked against every (file, name)
        // combination that appears, both hit and miss.
        struct Lcg(u64);
        impl Lcg {
            fn next(&mut self) -> u64 {
                self.0 = self
                    .0
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                self.0
            }
            fn range(&mut self, n: u32) -> u32 {
                (self.next() % u64::from(n)) as u32
            }
        }

        for seed in 0..50u64 {
            let mut rng = Lcg(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1));
            let file_count = 1 + rng.range(4);
            let files: Vec<String> = (0..file_count).map(|i| format!("f{i}.rs")).collect();
            let name_count = 1 + rng.range(5);
            let names: Vec<String> = (0..name_count).map(|i| format!("n{i}")).collect();

            let unit_count = rng.range(20);
            let units: Vec<Unit> = (0..unit_count)
                .map(|_| {
                    let file = &files[rng.range(file_count) as usize];
                    let name = &names[rng.range(name_count) as usize];
                    let is_test = rng.range(2) == 0;
                    mk_unit(file, name, is_test)
                })
                .collect();

            let index = test_unit_key_index(&units, false);

            // Every (file, name) combination that appears anywhere, not just
            // among the generated units — exercises misses as well as hits.
            for file in &files {
                for name in &names {
                    let path = Path::new(file.as_str());
                    let expected = brute_is_test_repeat(&units, path, name);
                    let actual = index.contains(&(path, name.as_str()));
                    assert_eq!(actual, expected, "seed {seed} file {file} name {name}");
                }
            }
        }
    }

    #[test]
    fn test_unit_key_index_skip_is_empty() {
        let units = vec![mk_unit("a.rs", "n", true)];
        let index = test_unit_key_index(&units, true);
        assert!(index.is_empty());
    }
}
