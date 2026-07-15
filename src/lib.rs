//! reprise — semantic-ish code duplicate detection (a common and central
//! use case: duplication and divergence from LLM coding agents).
//! Spec: docs/PLAN.md (Rev 9); deviations: DECISIONS.md.

pub mod api;
pub mod au;
pub mod baseline;
pub mod cache;
pub mod check;
pub mod config;
pub mod digest;
pub mod fingerprint;
pub mod fold;
pub mod formats;
pub mod frontend;
pub mod group;
pub mod inline;
pub mod intern;
pub mod ir;
pub mod lang;
pub mod matchtree;
pub mod memory;
pub mod normalize;
pub mod pack;
pub mod rawmemo;
pub mod report;
pub mod seq;
pub mod source;
pub mod store;
pub mod stream;
#[cfg(test)]
pub(crate) mod test_utils;
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
/// walk excludes), the per-unit `call_sites` metadata projection and `raw_tree_files`/
/// `unit_file_idx` coordinates the inliner's raw-tree memo (`rawmemo::RawTreeMemo`)
/// rehydrates from (WP-D: raw trees are no longer corpus-wide resident — see below),
/// each file's [`unit::InternalRepeat`] findings, a per-file content digest (xxh3-128
/// of the bytes read at extraction time, keyed by the same `PathBuf` a `Unit::file`
/// carries) for consumers that need to re-read a file's text later and detect whether
/// it changed on disk in the meantime, and the subset of [`report::Stats`] this phase
/// fills (`files_scanned`, `files_skipped_generated`, `files_unreadable`,
/// `suppressed_units`, `cache_hits`/`cache_misses`, `units_indexed`,
/// `parse_degraded_units`, `test_units`, `total_lines`) — every other `Stats` field is
/// left at its `Default` for the caller to fill in as it goes.
///
/// Full source text is intentionally NOT retained here (it used to be, keyed the same
/// way) — the only two production consumers (`scan`'s sequence-tier line-span
/// rendering and its `total_lines` stat) either need just a line count (computed
/// incrementally below, while the text is already in hand for parsing) or a handful of
/// on-demand re-reads for the rare files that actually surface in a sequence-tier
/// finding; `source_digests` lets that re-read detect a mid-scan edit instead of
/// silently rendering against stale-vs-fresh mismatched content.
///
/// WP-D (raw-trees elimination, session `woozy-uncut-comic`): this struct used to
/// carry a corpus-wide resident `raw_trees: Vec<NormNode>` — the LARGER half of the
/// tree mass, 38% of extraction's peak. It is gone: the inliner's resolvability
/// precheck and adjacency (SCC) pass read `call_sites` (a per-unit metadata
/// projection, no tree bytes) instead of a raw tree, and the raw tree itself is
/// fetched only at splice time, lazily, through a budget-bounded read-through memo
/// (`rawmemo::RawTreeMemo`) built from `raw_tree_files`/`unit_file_idx` — a scan-scoped
/// object `scan_source` constructs right before the inline phase, not carried on this
/// struct (its lifetime needs `source`/`source_digests` borrows this struct doesn't own).
pub struct CorpusUnits {
    pub units: Vec<Unit>,
    /// Gate 2 (memory architecture P2), decided here — a phase boundary, from
    /// exact post-extraction counts. `scan()` reads this instead of re-deciding.
    pub gate: memory::GateDecision,
    /// Per-unit fused digests (`digest::compute`), index-aligned with `units`:
    /// everything the single-tree consumers (exact ranking, near-tier substrate,
    /// sequence stream, api multiset) read instead of walking `Unit::tree` once
    /// trees spill. **Populated ONLY over the memory gate** (`gate.over`) —
    /// under the gate this is EMPTY and every consumer walks the resident trees
    /// exactly as before the drop-trees work (P2: "under budget → today's
    /// residency, zero new work"; also the Tier-2 join contract — digests exist
    /// exactly when a gate is over). NOT part of the D19 cache (a pure recompute
    /// from the tree; P4 freezes the cache format).
    pub digests: Vec<digest::UnitDigest>,
    /// WP-D (raw-trees elimination): per-unit metadata projection of what used to
    /// be a corpus-wide resident raw tree — index-aligned with `units`. The
    /// inliner's resolvability precheck and adjacency (SCC) pass read this
    /// instead of a raw tree; the raw tree itself is fetched only at splice
    /// time, through the memo built from `raw_tree_files`/`unit_file_idx` below.
    pub call_sites: Vec<inline::UnitCallSites>,
    /// One entry per FILE (not per unit) — the raw-tree memo's rehydration
    /// coordinates (WP-D).
    pub raw_tree_files: Vec<rawmemo::RawFileKey>,
    /// Per unit, which `raw_tree_files` entry it belongs to (WP-D).
    pub unit_file_idx: Vec<u32>,
    pub repeats: Vec<unit::InternalRepeat>,
    pub source_digests: HashMap<std::path::PathBuf, u128>,
    pub stats: report::Stats,
    /// The per-scan label interner used to build every `Unit` tree above
    /// (interning WP, session `stark-mixed-front`). `scan()`'s later inline phase
    /// (§5.4) reuses this SAME instance for variant trees, so a variant's identifiers
    /// dedupe against the rest of the scan instead of paying for a second table.
    pub label_interner: std::sync::Arc<crate::intern::LabelInterner>,
}

/// Walk `root` (respecting `config`'s excludes) and extract every unit, reusing the
/// D19 per-file cache (a warm hit is byte-identical to a cold extraction by contract,
/// `src/cache.rs`). **This is THE sanctioned way to obtain corpus units** — `scan()`
/// calls it for its extract half; any other consumer (the frozen-index builder,
/// reprise-mcp's `find_similar`) must route through here too instead of hand-rolling
/// a walk+extract loop, so every caller shares one walk/cache/extract path and its
/// D19 cache-reuse (docs/SERVERS.md §7 M1, DECISIONS.md D46).
pub fn corpus_units(root: &Path, config: &Config) -> anyhow::Result<CorpusUnits> {
    corpus_units_from(&source::FsSource::new(root), config)
}

/// [`corpus_units`] over an arbitrary [`ContentSource`](source::ContentSource) —
/// the live checkout (`scan`), or a git tree with no checkout at all (`check`,
/// reading the index or a base ref straight from the object store).
///
/// Unit identity is source-independent: paths are `source.root()`-relative and
/// the D19 cache key is content-addressed, so a file whose staged bytes equal its
/// on-disk bytes is a cache HIT across the two sources rather than a re-extraction.
pub fn corpus_units_from(
    source: &dyn source::ContentSource,
    config: &Config,
) -> anyhow::Result<CorpusUnits> {
    let root = source.root();
    let files = source.files(config)?;

    // Interning WP (session `stark-mixed-front`): ONE fresh per-scan `LabelInterner`
    // for this whole `corpus_units` call — scoped exactly to this call (per the
    // preamble), never global, so a long-lived server (`reprise-mcp`) calling this
    // repeatedly never leaks label ids across scans. `Arc` clones cheaply into every
    // rayon worker closure below.
    let label_interner = crate::intern::LabelInterner::new();

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
        /// (extraction, content digest, line count, cache hit, D19 cache key,
        /// language) — the digest and line count are cheap-to-compute-now facts
        /// derived from the source text while it's in hand for parsing; the text
        /// itself is not retained (see `CorpusUnits` doc comment). The cache key
        /// is the SAME `u128` `cache::key(...)` already computes below — retained
        /// for the raw-tree memo (WP-D) instead of thrown away.
        Units(unit::FileUnits, u128, usize, bool, u128, lang::Lang),
        SkippedGenerated,
        Unreadable,
    }

    let outcomes: Vec<(std::path::PathBuf, FileOutcome)> = files
        .par_iter()
        .map(|(path, lang)| {
            let outcome = match source.read(path) {
                None => FileOutcome::Unreadable,
                Some(src) if walk::is_generated(&src, config) => FileOutcome::SkippedGenerated,
                Some(src) => {
                    // D8/D19 version-keyed per-file cache: a hit is
                    // byte-identical to a cold extraction by contract.
                    let key = cache::key(&baseline::relative_file(path, root), &src, config);
                    let cache_root = config.cache.shared_root.as_deref().unwrap_or(root);
                    let cached = config
                        .cache
                        .enabled
                        .then(|| cache::load(cache_root, key, path, &label_interner))
                        .flatten();
                    let digest = xxhash_rust::xxh3::xxh3_128(src.as_bytes());
                    let line_count = src.lines().count();
                    match cached {
                        Some(extracted) => {
                            FileOutcome::Units(extracted, digest, line_count, true, key, *lang)
                        }
                        None => {
                            let extracted = unit::extract_file_units_keep_raw(
                                path,
                                &src,
                                *lang,
                                config,
                                &label_interner,
                            );
                            if config.cache.enabled {
                                cache::store(cache_root, key, &extracted, &label_interner);
                            }
                            FileOutcome::Units(extracted, digest, line_count, false, key, *lang)
                        }
                    }
                }
            };
            (path.clone(), outcome)
        })
        .collect();

    let mut units: Vec<Unit> = Vec::new();
    let mut repeats = Vec::new();
    let mut source_digests: HashMap<std::path::PathBuf, u128> = HashMap::new();
    let mut stats = report::Stats::default();
    // WP-D (raw-trees elimination): the metadata projection + retained memo
    // coordinates — the ONLY things extraction keeps from each file's raw
    // trees; `extracted.raw_trees` itself drops at the end of each loop
    // iteration below, never accumulated corpus-wide. `kw_arg` interned ONCE
    // per scan (rule 5), exactly as `DefTable::build` used to.
    let kw_arg = label_interner.intern("keyword_argument");
    let mut call_sites: Vec<inline::UnitCallSites> = Vec::new();
    let mut raw_tree_files: Vec<rawmemo::RawFileKey> = Vec::new();
    let mut unit_file_idx: Vec<u32> = Vec::new();
    for (path, outcome) in outcomes {
        match outcome {
            FileOutcome::Units(mut extracted, digest, line_count, cache_hit, cache_key, lang) => {
                stats.files_scanned += 1;
                stats.suppressed_units += extracted.suppressed;
                stats.total_lines += line_count;
                if cache_hit {
                    stats.cache_hits += 1;
                } else {
                    stats.cache_misses += 1;
                }

                let file_idx = raw_tree_files.len() as u32;
                let unit_start = units.len() as u32;
                for (unit, tree) in extracted.units.iter().zip(&extracted.raw_trees) {
                    let shapes = inline::Shapes::for_lang(unit.lang, config);
                    let body = crate::lang::child_field(tree, "body");
                    let mut calls = Vec::new();
                    if let Some(b) = body {
                        inline::collect_call_sites(b, shapes, &mut calls, kw_arg);
                    }
                    call_sites.push(inline::UnitCallSites {
                        body_child_count: body.map(|b| b.children.len() as u32),
                        body_tokens: body.map_or(0, |b| b.token_count()),
                        calls,
                        params: shapes.params(tree),
                    });
                    unit_file_idx.push(file_idx);
                }
                raw_tree_files.push(rawmemo::RawFileKey {
                    file: path.clone(),
                    lang,
                    cache_key,
                    unit_start,
                    unit_count: extracted.units.len() as u32,
                });

                units.append(&mut extracted.units);
                repeats.append(&mut extracted.repeats);
                source_digests.insert(path, digest);
                // `extracted.raw_trees` drops here — its only durable trace is
                // `call_sites` above and, on disk, the D19 cache blob the memo
                // rehydrates from later.
            }
            FileOutcome::SkippedGenerated => stats.files_skipped_generated += 1,
            FileOutcome::Unreadable => stats.files_unreadable += 1,
        }
    }
    stats.units_indexed = units.len();
    stats.parse_degraded_units = units.iter().filter(|u| u.parse_degraded).count();
    stats.test_units = units.iter().filter(|u| u.is_test).count();
    // Lever 4: reclaim the ~1.25-1.5x average doubling-growth slack these three
    // corpus-wide Vecs accumulated via repeated `.append()` above — a fixed-size,
    // zero-risk win independent of the digest/re-read redesign above.
    units.shrink_to_fit();
    repeats.shrink_to_fit();
    call_sites.shrink_to_fit();
    raw_tree_files.shrink_to_fit();
    unit_file_idx.shrink_to_fit();

    // ---- Gate 2 (memory architecture P2): ONE decision, at this phase
    // boundary, from exact post-extraction counts. Under the gate the fused
    // digests are NOT computed — every downstream consumer walks the resident
    // trees exactly as it did before the drop-trees work (zero new work, zero
    // new residency: the common case pays nothing). Over the gate, one
    // parallel pass computes every plain unit's digest while the trees are
    // still resident (variants get theirs at the `finish_variant` tail), and
    // `scan()` spills the trees to the scan-scoped pack. ----
    let gate = memory::decide(&units, config);
    // The extraction boundary: every tree exists, nothing has spilled yet, and the
    // gate is deciding from a MODEL of the very residency now sitting in RAM. Read
    // it. The reading changes no decision — it makes the model auditable on every
    // run instead of only under an external harness.
    stats.memory_gate_rss_bytes = memory::current_rss_bytes().unwrap_or(0);
    let digests: Vec<digest::UnitDigest> = if gate.over {
        units
            .par_iter()
            .map(|u| {
                digest::compute(
                    u.tree.expect_resident(),
                    u.lang,
                    config,
                    u.variant.is_none(),
                    &label_interner,
                )
            })
            .collect()
    } else {
        Vec::new()
    };

    // END of extraction — AFTER the digests. The gate-decision reading above is
    // taken before they exist, and they cost GiB: reading extraction's peak there
    // understated it by 2.6 GiB at drivers and inverted the conclusion about where
    // the peak is set.
    stats.memory_extract_peak_bytes = memory::peak_rss_bytes().unwrap_or(0);

    Ok(CorpusUnits {
        units,
        gate,
        digests,
        call_sites,
        raw_tree_files,
        unit_file_idx,
        repeats,
        source_digests,
        stats,
        label_interner,
    })
}

/// The scan-scoped packs (memory architecture P3), created only over the
/// memory gate: one for canonical trees (near-tier verify materializes pairs
/// through its LRU), one for sequence streams (bulk-loaded per language
/// partition). Both die with the scan — anonymous temp files, never the D19
/// cache.
struct ScanPacks {
    trees: pack::Pack<tree::NormNode>,
    seqs: pack::Pack<digest::SeqStream>,
}

/// Best-effort cleanup of the pack's OWNED default directory
/// (`<root>/.reprise/tmp`, `pack::resolve_pack_dir`'s `owned: true` case) —
/// never a `[memory] pack_dir` override, never the `temp_dir()` fallback
/// (neither is ours to remove). `remove_dir` only succeeds if the directory
/// is empty, which it will be (the pack's backing file(s) are anonymous,
/// unlinked at creation — this directory is scan-scoped scratch space, not
/// the durable D19 cache, and must not outlive the scan). A plain struct
/// field (not a closure) so `Drop` runs on every exit from `scan()` —
/// success, an early `?` return, or a panic unwind — matching the pack's own
/// "cannot outlive the scan even on abnormal exit" invariant.
struct PackDirCleanup(Option<std::path::PathBuf>);
impl Drop for PackDirCleanup {
    fn drop(&mut self) {
        if let Some(dir) = self.0.take() {
            let _ = std::fs::remove_dir(&dir);
            // Best-effort: also tidy away the now-possibly-empty `.reprise`
            // parent (a no-op, harmlessly, if it still holds the durable D19
            // cache or anything else) — a scan with caching disabled should
            // leave NO trace under `.reprise/` at all, not an empty `tmp/`'s
            // empty parent.
            if let Some(parent) = dir.parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }
}

/// Full-repo scan (spec §2 `reprise scan`) over the live checkout.
pub fn scan(root: &Path, config: &Config) -> anyhow::Result<ScanReport> {
    scan_source(&source::FsSource::new(root), config)
}

/// [`scan`] over an arbitrary [`ContentSource`](source::ContentSource). `check`
/// scans a git tree through this — the index for the prospective commit, the base
/// ref for base state — so it never materialises a checkout to read.
pub fn scan_source(
    source: &dyn source::ContentSource,
    config: &Config,
) -> anyhow::Result<ScanReport> {
    let root = source.root();
    let started = Instant::now();
    let CorpusUnits {
        mut units,
        gate,
        mut digests,
        call_sites,
        raw_tree_files,
        unit_file_idx,
        repeats: internal_repeats,
        source_digests,
        mut stats,
        label_interner,
    } = corpus_units_from(source, config)?;
    let plain_count = units.len();

    let mut phase_started = started;
    let mut phase = |name: &str, stats: &mut report::Stats, now: Instant| {
        stats
            .phase_ms
            .insert(name.to_string(), (now - phase_started).as_millis() as u64);
        phase_started = now;
    };

    phase("extract", &mut stats, Instant::now());

    // ---- Gate 2 (memory architecture P2): ONE decision, at this phase
    // boundary, from exact post-extraction counts. Under the gate nothing
    // below changes (trees resident, zero new work). Over it, trees and
    // sequence streams spill to the scan-scoped content-addressed pack (P3);
    // near-tier verify materializes pairs through the pack LRU and the
    // sequence tier bulk-loads per language partition. Either way the OUTPUT
    // is byte-identical — the gate changes performance, never output. ----
    let mut pack_dir_owned: Option<std::path::PathBuf> = None;
    let packs = if gate.over {
        // The pack's backing directory (the tmpfs-ENOSPC fix): scan-root-
        // relative by default, `[memory] pack_dir` override outranks it, an
        // unwritable root falls back to the process temp dir with a named
        // warning (never silence — a pack MUST have somewhere to write).
        let resolved = pack::resolve_pack_dir(root, config.memory.pack_dir.as_deref());
        if let Some(warning) = &resolved.warning {
            eprintln!("warning: {warning}");
        }
        if resolved.owned {
            pack_dir_owned = Some(resolved.dir.clone());
        }
        let pack_dir = resolved.dir;
        // Tree LRU: a budget-derived slice, floored so verify pairs fit
        // comfortably; the seq pack needs no real LRU (one bulk load per
        // partition, handles owned by the loop) so it gets a token bound.
        // The tree decoder re-interns labels through THIS scan's interner
        // (post-interning, `NormNode` deserializes only via the wire path).
        let tree_lru_bytes = memory::tree_lru_bytes(gate.budget_bytes);
        let li_encode = std::sync::Arc::clone(&label_interner);
        let li_decode = std::sync::Arc::clone(&label_interner);
        Some(ScanPacks {
            trees: pack::Pack::with_codec(
                tree_lru_bytes,
                16,
                &pack_dir,
                move |node: &tree::NormNode| {
                    let wire = node.to_wire(&li_encode);
                    bincode::serialize(&wire).expect("pack tree serializes")
                },
                move |bytes| {
                    let wire: tree::NormNodeWire =
                        bincode::deserialize(bytes).expect("pack tree decodes");
                    wire.into_real(&li_decode)
                },
            )
            .map_err(|e| {
                anyhow::anyhow!("creating scan tree pack under {}: {e}", pack_dir.display())
            })?,
            seqs: pack::Pack::new(1 << 20, 4, &pack_dir).map_err(|e| {
                anyhow::anyhow!("creating scan seq pack under {}: {e}", pack_dir.display())
            })?,
        })
    } else {
        None
    };
    // Declared once `pack_dir_owned` is settled; drops (and best-effort
    // removes the directory) on every exit from `scan()` below, success or
    // error.
    let _pack_dir_cleanup = PackDirCleanup(pack_dir_owned);
    let mut spilled_trees = 0usize;
    if let Some(p) = &packs {
        // Spill moment 1 (P3): one sequential pass over the already-
        // materialized plain units. (Variants spill at creation, below.) A
        // store failure (e.g. the pack's volume fills) aborts the scan
        // cleanly through `?` — never a panic, never a poisoned mutex (see
        // `pack::PackStoreError`).
        for (u, d) in units.iter_mut().zip(digests.iter_mut()) {
            if let unit::TreeSlot::Resident(t) = &u.tree {
                let key = p
                    .trees
                    .store(t)
                    .map_err(|e| anyhow::anyhow!("spilling a plain unit's tree: {e}"))?;
                u.tree = unit::TreeSlot::Spilled(key);
                spilled_trees += 1;
            }
            if let digest::SeqSlot::Resident(s) = &d.seq_tokens {
                let key = p
                    .seqs
                    .store(s)
                    .map_err(|e| anyhow::anyhow!("spilling a plain unit's token stream: {e}"))?;
                d.seq_tokens = digest::SeqSlot::Spilled(key);
            }
        }
    }

    // ---- P5: best-effort inliner (spec §5.4) — variants appended, tagged ----
    if config.inline.enabled {
        // WP-D (raw-trees elimination): a budget-bounded read-through memo over
        // the D19 cache replaces the corpus-wide resident `raw_trees` Vec (38%
        // of extraction's peak). Scoped to this inline block — same lifetime
        // the old Vec had, dropped at the block's end — and reading through
        // `source` (never `std::fs` directly), so `check`'s `GitSource` never
        // touches a checkout that may not exist. `cache_root` mirrors
        // `corpus_units_from`'s own resolution (`src/lib.rs`'s extraction
        // closure) exactly, so a cache-on memo hits the SAME blobs extraction
        // wrote. Budget `None` (unbounded) for now — wired to the pressure-
        // aware floor in a later step (`memory::raw_tree_memo_bytes(&gate)`).
        let cache_root = config
            .cache
            .shared_root
            .as_deref()
            .unwrap_or(root)
            .to_path_buf();
        let memo = rawmemo::RawTreeMemo::new(
            &raw_tree_files,
            &unit_file_idx,
            source,
            &source_digests,
            cache_root,
            config,
            std::sync::Arc::clone(&label_interner),
            None,
        );
        let table = inline::DefTable::build(&units, &call_sites, config, &label_interner);
        stats.scc_units = table.scc_unit_count();
        let expansions: Vec<inline::Expansion> = (0..plain_count)
            .into_par_iter()
            .map(|i| {
                inline::expand_unit(
                    i,
                    &call_sites,
                    &units,
                    &table,
                    config,
                    &label_interner,
                    &memo,
                )
            })
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
            /// The aggregate expansion budget refused this unit (NOT a benign
            /// thin-delegation/no-resolvable-call skip) — counted separately so the
            /// backstop can never fire silently.
            budget_skipped: bool,
            /// The variant unit plus (over the gate only) its fused digest —
            /// the variant analog of the gate-trip digest moment, computed while
            /// this fresh tree is still resident.
            variant: Option<(Unit, Option<digest::UnitDigest>)>,
        }
        // Each closure below returns `anyhow::Result<VariantOutcome>` — a variant
        // spill failure (spill moment 2, P3) must abort the scan cleanly, not
        // panic, so it's threaded through as a `Result` and short-circuited by
        // `collect::<anyhow::Result<Vec<_>>>()` (rayon's `Result`
        // `FromParallelIterator`), same as the sequential spill above.
        let outcomes: Vec<VariantOutcome> = expansions
            .into_par_iter()
            .enumerate()
            .map(|(i, exp)| -> anyhow::Result<VariantOutcome> {
                let ambiguity_skips = exp.ambiguity_skips;
                let budget_skipped = exp.budget_skipped;
                if exp.calls_inlined == 0 {
                    return Ok(VariantOutcome {
                        ambiguity_skips,
                        calls_inlined: 0,
                        budget_skipped,
                        variant: None,
                    });
                }
                // calls_inlined > 0 (checked above) guarantees expand_unit produced
                // a spliced tree (see inline::any_resolvable_call / Expansion::tree).
                let tree = exp
                    .tree
                    .expect("calls_inlined > 0 implies expand_unit produced a tree");
                let Some(mut variant) =
                    unit::finish_variant(i, &units[i], tree, config, &label_interner)
                else {
                    // inlining changed nothing post-normalization
                    return Ok(VariantOutcome {
                        ambiguity_skips,
                        calls_inlined: 0,
                        budget_skipped: false,
                        variant: None,
                    });
                };
                let expanded_fps: Vec<u128> = exp
                    .expanded_units
                    .iter()
                    .map(|&u| units[u].fingerprint)
                    .collect();
                if expanded_fps.contains(&variant.fingerprint) {
                    // D3: a pure wrapper's variant IS its callee
                    return Ok(VariantOutcome {
                        ambiguity_skips,
                        calls_inlined: 0,
                        budget_skipped: false,
                        variant: None,
                    });
                }
                let tag = variant.variant.as_mut().expect("finish_variant tags");
                tag.chain = exp.chain;
                tag.expanded_fps = expanded_fps;
                tag.scc = exp.scc;
                // Over the gate only: fuse the digest while this fresh tree is
                // resident, then spill it (spill moment 2, P3 — the pack doesn't
                // care where a tree came from: plain and variant, one mechanism).
                // Under the gate: no digest, no spill — zero new work (P2).
                let vd = packs.is_some().then(|| {
                    digest::compute(
                        variant.tree.expect_resident(),
                        variant.lang,
                        config,
                        false,
                        &label_interner,
                    )
                });
                if let Some(p) = &packs
                    && let unit::TreeSlot::Resident(t) = &variant.tree
                {
                    let key = p
                        .trees
                        .store(t)
                        .map_err(|e| anyhow::anyhow!("spilling an inline variant's tree: {e}"))?;
                    variant.tree = unit::TreeSlot::Spilled(key);
                }
                Ok(VariantOutcome {
                    ambiguity_skips,
                    calls_inlined: exp.calls_inlined,
                    budget_skipped: false,
                    variant: Some((variant, vd)),
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let mut variants: Vec<Unit> = Vec::new();
        let mut variant_digests: Vec<digest::UnitDigest> = Vec::new();
        for outcome in outcomes {
            stats.ambiguity_skips += outcome.ambiguity_skips as usize;
            stats.calls_inlined += outcome.calls_inlined as usize;
            stats.inline_budget_skipped_units += usize::from(outcome.budget_skipped);
            if let Some((variant, vd)) = outcome.variant {
                variants.push(variant);
                if let Some(vd) = vd {
                    variant_digests.push(vd);
                }
            }
        }
        stats.inline_variants = variants.len(); // ≤1 per unit: the §12 cap
        if packs.is_some() {
            spilled_trees += variants.len(); // every variant spilled at creation
        }
        units.extend(variants);
        digests.extend(variant_digests);
        // Lever 4: `units` just grew ~1.5x (plain + variants) via `extend`'s own
        // doubling growth; this is its last growth point for the rest of `scan()`,
        // so reclaim the slack here once rather than carry it through every
        // downstream tier.
        units.shrink_to_fit();
        digests.shrink_to_fit();
    }
    debug_assert!(
        if gate.over {
            units.len() == digests.len()
        } else {
            digests.is_empty()
        },
        "digests exist (index-aligned) exactly when the gate is over"
    );
    // WP-D: no `raw_trees` to drop — the memo (scoped to the `if config.inline
    // .enabled` block above) already went out of scope with it.
    phase("inline", &mut stats, Instant::now());

    // ---- exact tier (P7 bucketing; plain) + inline-assisted exact matches ----
    let digests_opt = gate.over.then_some(digests.as_slice());
    let (exact, below_floor) =
        group::build_exact_groups(&units, digests_opt, config, &label_interner);
    stats.units_below_floor = below_floor;
    let inline_exact =
        group::build_inline_exact_groups(&units, digests_opt, config, &label_interner);

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
    let near = matchtree::find_near_groups(
        &units,
        digests_opt,
        config,
        &mut stats.retrieval,
        &exact_pairs,
        packs.as_ref().map(|p| &p.trees),
        &label_interner,
    );
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
        // Full source text is no longer retained corpus-wide (lever B) — only the
        // (comparatively rare) files that actually surface in a sequence-tier
        // finding need their text, so re-read those on demand and memoize per file
        // for the rest of this block. A mismatch against `source_digests` (file
        // changed on disk since extraction, or became unreadable) degrades to the
        // SAME "" fallback `sources.get(..).unwrap_or("")` used to produce for a
        // path absent from the map — never a panic, never a silent render against
        // mismatched content.
        let mut src_cache: HashMap<std::path::PathBuf, String> = HashMap::new();
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
            // Under the gate: serialize this partition's streams from the
            // resident trees, in parallel — exactly the walk the old
            // `build_corpus(&[&Unit])` did internally, same serializer, same
            // transient lifetime (dropped with the partition). Over the gate:
            // streams were fused at digest time and spilled; bulk-load them
            // from the seq pack for exactly this partition (`handles` owns
            // them; both drop with the iteration — P3's per-partition
            // load/drop points).
            let owned: Vec<digest::SeqStream> = if gate.over {
                Vec::new()
            } else {
                part.par_iter()
                    .map(|&i| stream::unit_stream(units[i].tree.expect_resident(), &label_interner))
                    .collect()
            };
            let handles: Vec<Option<std::sync::Arc<digest::SeqStream>>> = if gate.over {
                part.iter()
                    .map(|&i| match &digests[i].seq_tokens {
                        digest::SeqSlot::Spilled(key) => Some(
                            packs
                                .as_ref()
                                .expect("spilled stream but no scan pack — gate wiring bug")
                                .seqs
                                .load(*key),
                        ),
                        digest::SeqSlot::Resident(_) => None,
                        digest::SeqSlot::Absent => {
                            unreachable!("sequence partition holds plain units only")
                        }
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let streams: Vec<&[(u64, (u32, u32))]> = if gate.over {
                part.iter()
                    .zip(&handles)
                    .map(|(&i, handle)| match (&digests[i].seq_tokens, handle) {
                        (digest::SeqSlot::Resident(v), _) => v.as_slice(),
                        (digest::SeqSlot::Spilled(_), Some(h)) => h.as_slice(),
                        _ => unreachable!("plain unit's digest carries a seq stream"),
                    })
                    .collect()
            } else {
                owned.iter().map(Vec::as_slice).collect()
            };
            let corpus = stream::build_corpus(&streams);
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
                let mut mem = |u: usize, range: (usize, usize)| {
                    let unit = &units[u];
                    let src: &str = src_cache.entry(unit.file.clone()).or_insert_with(|| {
                        // Re-read through the SAME source the units came from. Going to
                        // the filesystem here would render a git-sourced scan's line spans
                        // against worktree bytes — the digest guard would catch the
                        // mismatch and blank them, so the bug would show up as silently
                        // missing line numbers rather than as an error.
                        source
                            .read(&unit.file)
                            .filter(|text| {
                                Some(xxhash_rust::xxh3::xxh3_128(text.as_bytes()))
                                    == source_digests.get(&unit.file).copied()
                            })
                            .unwrap_or_default()
                    });
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
    let mut api_groups = api::find_api_groups(
        &units,
        digests_opt,
        config,
        &api_excluded,
        &mut stats.api_signatures,
        &label_interner,
    );
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
    // token mass is the removable-copy count in D1 normalized tokens. D28.
    // `stats.total_lines` is now filled incrementally in `corpus_units` (lever B),
    // while each file's text is already in hand for parsing — no longer computed
    // here from a corpus-wide retained-text map. ----
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

    // The scan is over: read the high-water mark. Emitted beside
    // `memory_estimated_bytes` so every ordinary run shows what the model got
    // wrong — the feedback loop whose absence let six load-bearing numbers drift.
    stats.memory_peak_bytes = memory::peak_rss_bytes().unwrap_or(0);

    // Gate-2 observability (approved amendment: `memory_*` are the WP's only
    // output additions; the identity harness strips them alongside timing).
    stats.memory_budget_bytes = gate.budget_bytes;
    stats.memory_estimated_bytes = gate.estimated_bytes;
    stats.memory_gate_tripped = gate.over;
    stats.memory_spilled_trees = spilled_trees;
    if let Some(p) = &packs {
        stats.memory_pack_bytes = p.trees.stored_bytes() + p.seqs.stored_bytes();
        stats.memory_lru_hits = p.trees.lru_hits();
        stats.memory_lru_misses = p.trees.lru_misses();
    }

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
            tree: unit::TreeSlot::Resident(tree::NormNode::new("Unit", None, (0, 0), Vec::new())),
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
        use crate::test_utils::Lcg;

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

    // ---- WP-D (raw-trees elimination, session `woozy-uncut-comic`):
    // `call_sites` must exactly reproduce direct reads off a unit's raw tree,
    // fetched here through the SAME raw-tree memo `scan_source` builds
    // (`CorpusUnits` carries no corpus-wide resident raw trees itself — see
    // its doc comment) — a failure here is a pure wiring/indexing bug in
    // `corpus_units_from`'s per-file loop, not a change in scan output. ----

    /// Real-world corpus (`benches/wild`): every language, every unit shape
    /// the wild corpus carries.
    #[test]
    fn call_sites_projection_matches_direct_raw_tree_reads_wild_corpus() {
        let mut cfg = Config::default();
        cfg.cache.enabled = false; // don't litter fixture dirs with .reprise/
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/wild");
        let corpus = corpus_units(&root, &cfg).expect("wild corpus scans");
        assert!(!corpus.units.is_empty(), "wild corpus produced no units");
        assert_eq!(corpus.units.len(), corpus.call_sites.len());
        assert_eq!(corpus.units.len(), corpus.unit_file_idx.len());
        let kw_arg = corpus.label_interner.intern("keyword_argument");
        let fs_source = source::FsSource::new(&root);
        let memo = rawmemo::RawTreeMemo::new(
            &corpus.raw_tree_files,
            &corpus.unit_file_idx,
            &fs_source,
            &corpus.source_digests,
            root.clone(),
            &cfg,
            std::sync::Arc::clone(&corpus.label_interner),
            None,
        );
        for (i, unit) in corpus.units.iter().enumerate() {
            let tree = memo.unit(i);
            let tree = &tree;
            let shapes = inline::Shapes::for_lang(unit.lang, &cfg);
            let body = lang::child_field(tree, "body");
            let expected_body_child_count = body.map(|b| b.children.len() as u32);
            let expected_body_tokens = body.map_or(0, |b| b.token_count());
            let expected_params = shapes.params(tree);
            let mut expected_calls = Vec::new();
            if let Some(b) = body {
                inline::collect_call_sites(b, shapes, &mut expected_calls, kw_arg);
            }
            let cs = &corpus.call_sites[i];
            assert_eq!(
                cs.body_child_count, expected_body_child_count,
                "unit {i} ({} {:?})",
                unit.name, unit.lang
            );
            assert_eq!(
                cs.body_tokens, expected_body_tokens,
                "unit {i} ({} {:?})",
                unit.name, unit.lang
            );
            assert_eq!(
                cs.params, expected_params,
                "unit {i} ({} {:?})",
                unit.name, unit.lang
            );
            assert_eq!(
                cs.calls, expected_calls,
                "unit {i} ({} {:?})",
                unit.name, unit.lang
            );
        }
    }

    /// Synthetic fixture exercising the `calls` filter directly: a plain call
    /// (captured), a method call (excluded — not a plain-identifier callee), a
    /// keyword-arg call (excluded), and nested calls (both captured).
    #[test]
    fn call_sites_projection_excludes_method_and_kwarg_calls() {
        let cfg = Config::default();
        let src = r#"
def helper(a, b):
    return a + b

def obj_method_call(x):
    return x.method(1, 2)

def kwarg_call(x):
    return helper(a=x, b=1)

def nested(x):
    return helper(helper(x, 1), 2)
"#;
        let label_interner = crate::intern::LabelInterner::new();
        let fu = unit::extract_file_units_keep_raw(
            Path::new("fixture.py"),
            src,
            lang::Lang::Python,
            &cfg,
            &label_interner,
        );
        assert_eq!(fu.units.len(), 4, "fixture sanity: 4 top-level defs");
        let kw_arg = label_interner.intern("keyword_argument");
        for (i, unit) in fu.units.iter().enumerate() {
            let tree = &fu.raw_trees[i];
            let shapes = inline::Shapes::for_lang(unit.lang, &cfg);
            let body = lang::child_field(tree, "body");
            let mut calls = Vec::new();
            if let Some(b) = body {
                inline::collect_call_sites(b, shapes, &mut calls, kw_arg);
            }
            match unit.name.as_str() {
                "helper" => assert!(calls.is_empty(), "no calls in helper: {calls:?}"),
                "obj_method_call" => assert!(
                    calls.is_empty(),
                    "a method call has no plain-identifier callee: {calls:?}"
                ),
                "kwarg_call" => assert!(
                    calls.is_empty(),
                    "a keyword-arg call is not positionally mappable: {calls:?}"
                ),
                "nested" => assert_eq!(
                    calls.len(),
                    2,
                    "both nested calls to helper must be captured: {calls:?}"
                ),
                other => panic!("unexpected fixture unit {other}"),
            }
        }
    }

    /// A Kotlin unit's root has NO "body" FIELD at all (the grammar locates the
    /// body by node kind, not by field — `src/lang/kotlin.rs`'s own doc
    /// comment) — never `Some(empty)`. `body_child_count` must come out `None`,
    /// not `Some(0)`, or `expand_unit`'s thin-delegation check
    /// (`Some(n) if n <= 1`) would flip from "never fires" (today, since
    /// `child_field(..).is_some_and(..)` is false on `None`) to "always fires"
    /// for every Kotlin unit — silently disabling the inliner for the language.
    #[test]
    fn call_sites_projection_kotlin_body_child_count_is_none_not_zero() {
        let cfg = Config::default();
        let src = "fun add(a: Int, b: Int): Int {\n    return a + b\n}\n";
        let label_interner = crate::intern::LabelInterner::new();
        let fu = unit::extract_file_units_keep_raw(
            Path::new("fixture.kt"),
            src,
            lang::Lang::Kotlin,
            &cfg,
            &label_interner,
        );
        assert_eq!(fu.units.len(), 1, "fixture sanity: one top-level fun");
        let tree = &fu.raw_trees[0];
        assert!(
            lang::child_field(tree, "body").is_none(),
            "Kotlin grammar has no \"body\" field — this must stay None"
        );
        // Mirrors corpus_units_from's own projection-building line.
        let body = lang::child_field(tree, "body");
        let body_child_count = body.map(|b| b.children.len() as u32);
        assert_eq!(
            body_child_count, None,
            "collapsing this to a bare 0 would make expand_unit's \
             `body_child_count <= 1` check ALWAYS true for Kotlin, disabling \
             its inliner outright"
        );
    }
}
