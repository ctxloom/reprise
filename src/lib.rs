//! reprise — semantic-ish duplicate detection for LLM-generated code.
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
pub mod group;
pub mod inline;
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

/// Full-repo scan (spec §2 `reprise scan`).
pub fn scan(root: &Path, config: &Config) -> anyhow::Result<ScanReport> {
    let started = Instant::now();
    let files = walk::collect_files(root, config)?;

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
    let mut internal_repeats = Vec::new();
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
                internal_repeats.append(&mut extracted.repeats);
                sources.insert(path, src);
            }
            FileOutcome::SkippedGenerated => stats.files_skipped_generated += 1,
            FileOutcome::Unreadable => {}
        }
    }
    stats.units_indexed = units.len();
    stats.parse_degraded_units = units.iter().filter(|u| u.parse_degraded).count();
    stats.test_units = units.iter().filter(|u| u.is_test).count();
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
        let mut variants: Vec<Unit> = Vec::new();
        for (i, exp) in expansions.into_iter().enumerate() {
            stats.ambiguity_skips += exp.ambiguity_skips as usize;
            if exp.calls_inlined == 0 {
                continue;
            }
            let Some(mut variant) = unit::finish_variant(i, &units[i], exp.tree, config) else {
                continue; // inlining changed nothing post-normalization
            };
            let expanded_fps: Vec<u128> = exp
                .expanded_units
                .iter()
                .map(|&u| units[u].fingerprint)
                .collect();
            if expanded_fps.contains(&variant.fingerprint) {
                continue; // D3: a pure wrapper's variant IS its callee
            }
            stats.calls_inlined += exp.calls_inlined as usize;
            let tag = variant.variant.as_mut().expect("finish_variant tags");
            tag.chain = exp.chain;
            tag.expanded_fps = expanded_fps;
            tag.scc = exp.scc;
            variants.push(variant);
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
    for r in &internal_repeats {
        let is_test_repeat = units
            .iter()
            .any(|u| u.file == r.file && u.name == r.unit_name && u.is_test);
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
            value: group::consolidation_value(&member_refs, ng.template_tokens),
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
