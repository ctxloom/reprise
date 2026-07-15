//! `reprise.toml` configuration (spec §9). Every key has a default; the tool
//! must run with zero config.

use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub scan: ScanCfg,
    pub thresholds: Thresholds,
    pub normalize: NormalizeCfg,
    pub report: ReportCfg,
    pub tests: TestsCfg,
    pub retrieval: RetrievalCfg,
    pub inline: InlineCfg,
    pub api_profile: ApiProfileCfg,
    pub baseline: BaselineCfg,
    pub cache: CacheCfg,
    pub memory: MemoryCfg,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MemoryCfg {
    /// Fraction of detected system RAM to budget for the scan (Gate 2,
    /// `src/memory.rs`). The default path on machines without a pinned budget.
    pub budget_fraction: f64,
    /// Explicit budget in bytes — outranks `budget_fraction`; pin it for
    /// reproducible gate decisions (CI, cross-machine comparisons).
    pub budget_bytes: Option<u64>,
    /// "auto" (default: gate on the estimate) | "always" | "never". The
    /// `REPRISE_MEMORY_FORCE_GATE` env var outranks this key (the harness knob
    /// for forcing the spill path on small corpora without a config file).
    pub force_gate: String,
    /// Explicit directory for the scan-scoped content-addressed pack's
    /// backing file(s) (`src/pack.rs`) — outranks the default `<scan root>/
    /// .reprise/tmp/`. Pin this to a real-disk path when the scan root's
    /// volume is small or RAM-backed (e.g. a tmpfs `/tmp`): spilling "to disk"
    /// onto tmpfs still occupies RAM (defeating the point of spilling) and
    /// can exhaust a small tmpfs outright (ENOSPC) on large scans.
    pub pack_dir: Option<std::path::PathBuf>,
}

impl Default for MemoryCfg {
    fn default() -> Self {
        MemoryCfg {
            budget_fraction: 0.5,
            budget_bytes: None,
            force_gate: "auto".into(),
            pack_dir: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BaselineCfg {
    /// Pinned git ref (hash/tag/branch) used as `check`'s default base when
    /// `--base` is omitted — the persistent baseline IS a git ref; base state
    /// is (re)scanned from it and cached transiently (D40; supersedes the
    /// spec §2/§9 baseline file, which no longer exists).
    #[serde(rename = "ref", alias = "pinned")]
    pub pinned: Option<String>,
    /// inconsistent-update findings + divergence trend on base-state groups
    /// (spec §6).
    pub track_drift: bool,
}

impl Default for BaselineCfg {
    fn default() -> Self {
        BaselineCfg {
            pinned: None,
            track_drift: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CacheCfg {
    /// Version-keyed per-file cache under `.reprise/cache/` (spec §4, D8/D19).
    pub enabled: bool,
    /// Internal: read/write the cache under this root instead of the scan
    /// root — lets a base-ref worktree scan reuse the main repo's warm cache
    /// (keys are relative-path + content, identical for unchanged files).
    #[serde(skip)]
    pub shared_root: Option<std::path::PathBuf>,
}

impl Default for CacheCfg {
    fn default() -> Self {
        CacheCfg {
            enabled: true,
            shared_root: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct InlineCfg {
    /// Best-effort inliner (spec §5.4, §9 `[inline]`).
    pub enabled: bool,
    /// Inline only callees at or below this many tokens, approximated on the
    /// raw body tree (DECISIONS.md D18: post-fold counts under-report the
    /// spliced mass when the callee folds).
    pub max_callee_tokens: u32,
    pub max_depth: u32,
    /// Maximum nested SCC-partner splices. The SCC round exists to inline a mutual-recursion
    /// partner ONCE, turning mutual recursion into direct self-recursion (§5.4; Rev 5 then
    /// lowers it). Partners bypass `max_depth`/`max_callee_tokens` to let that round complete —
    /// so without this cap a large SCC nests as deep as it has distinct members (measured: 31
    /// on redis, against max_depth = 2) with no size limit, which is what produced a 25.1 M-node
    /// single-unit expansion and OOM-killed an 8 GiB scan.
    pub max_scc_depth: u32,
    /// Aggregate per-unit expansion budget: max spliced nodes admitted into one unit's inline
    /// variant. Over budget, the unit gets NO variant (never a truncated one), so this value
    /// never enters any fingerprint — it selects a variant's presence, never its content.
    ///
    /// A pure backstop. After `max_scc_depth` it binds on NOTHING measured: the largest unit on
    /// any of eight corpora is 12,232 spliced nodes (redis; next is linux/fs at 9,995, and p99
    /// anywhere is ≤ 3,077), and zero units truncate at any value in [25k, 500k]. The default is
    /// sited at **20× the largest unit ever observed** and ~100× below the pathology it exists to
    /// stop (a 25.1 M-node single-unit expansion that OOM-killed a 16 GiB scan). A `NormNode` is
    /// pinned at 64 B (`tree.rs`), so one pathological unit is bounded at ~16 MB. It is not set
    /// higher because `lib.rs` collects every `Expansion` before consuming any, so the aggregate
    /// ceiling scales with this value — no reason to loosen it past the margin actually needed.
    ///
    /// If this ever fires, `Stats::inline_budget_skipped_units` says so (and the terminal summary
    /// shows it): a backstop that drops a unit's inline-tier recall must never do it silently.
    pub max_expansion_nodes: u32,
    /// Skip a call site with more than this many resolution candidates
    /// (after the same-file → same-dir → repo-wide preference filter).
    pub max_candidates: usize,
}

impl Default for InlineCfg {
    fn default() -> Self {
        InlineCfg {
            enabled: true,
            max_callee_tokens: 120,
            max_depth: 2,
            max_scc_depth: 1,
            max_expansion_nodes: 250_000,
            max_candidates: 3,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ApiProfileCfg {
    /// api-profile tier (spec §5.7, §9 `[api_profile]`): suspicion-only.
    pub enabled: bool,
    /// Weighted-Jaccard threshold; expect to tighten per §7.4(d).
    pub api_profile_sim: f64,
    /// Units with fewer distinct rare callees emit no signature.
    pub api_min_distinct_rare: usize,
}

impl Default for ApiProfileCfg {
    fn default() -> Self {
        ApiProfileCfg {
            enabled: true,
            // Spec §9 guessed 0.8 ("expect to tighten"); the §7.4(d) sweep
            // went the other way: the rare>=3 signature floor carries the
            // precision, and 0.8 leaves the tier inert (D29, CALIBRATION.md
            // Phase 4). 0.5/3 measured 55% useful-precision at n=20.
            api_profile_sim: 0.5,
            api_min_distinct_rare: 3,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TestsCfg {
    /// Test-code policy (spec §5.1): separate | exclude | normal.
    pub mode: String,
}

impl Default for TestsCfg {
    fn default() -> Self {
        TestsCfg {
            mode: "separate".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RetrievalCfg {
    /// Which candidate-generation retriever runs. It is the SOLE source of near-tier
    /// candidates — retrieval is one candidate set, never a union of layers.
    /// **Validated at load** against the set the shipping binary actually honors
    /// (`matchtree::KNOWN_RETRIEVERS`, currently just `"landmark"`): an unknown value
    /// is an error, never a silent fallback to the default. The bake-off's entrants
    /// (minhash-lsh, winnowing, sourcerer-rare) implement the same
    /// `matchtree::Retriever` trait BENCH-SIDE (`examples/bakeoff.rs`) and are not
    /// selectable here — if it does not run in the shipped binary, it is not valid
    /// config. String-keyed so further retrievers can register later without a schema
    /// break; widening the set is a one-line change to `KNOWN_RETRIEVERS`.
    /// `"landmark"` is the §5.5.4 rare-peak constellation retriever (the §7.4b rivalry
    /// winner). Matching-time only — retrieval is post-fingerprint, so this NEVER
    /// enters the extraction cache key (hash-neutral).
    pub retriever: String,
    /// §5.5.4 landmark pairs — rivalry winner (§7.4b, see CALIBRATION.md;
    /// hole-context hashes were dropped per the §5.5 rivalry clause).
    ///
    /// Master on/off for the landmark layer. Since landmark is the only retriever, `false`
    /// leaves the near tier with NO retriever at all: it proposes zero candidates and finds
    /// nothing. This is a kill-switch for near-tier retrieval, not a layer selector.
    /// Default `true`.
    pub landmark_pairs: bool,
    /// Minimum shared landmark-pair hashes for a candidate. The M3b value of 4
    /// cost real recall (D27 A/B: serde lost 15/43 pairs); 2 is recall-neutral.
    pub shared_landmarks_min: usize,
    /// Pair-event window for hashes with many owners; 0 = unlimited (df_cap
    /// already bounds the quadratic). The M3b window of 3 cost 30/43 pairs (D27).
    pub owner_pair_window: usize,
    /// Coverage-fraction candidate gate (docs/substantiality-metric.md §0.3):
    /// retain a landmark candidate only when its shared constellation is at least
    /// this fraction of the smaller unit's landmark set. A whole-unit clone covers
    /// most of each unit (accepted near-clones sit at median ~0.61); a coincidental
    /// boilerplate region covers little (rejected flood sits at ~0.06). Gating
    /// pre-AU cuts the landmark flood at zero recall cost: the live sweep found
    /// t=0.05 → recall 1.000, flood cut ~0.44 (fidelity-proven). A pure
    /// candidate-generation filter — it never touches the fingerprint or cache key
    /// (hash-neutral). 0 disables the gate.
    pub landmark_coverage_min: f64,
    /// Ordered near-tier FILTER cascade: a declarative list of named pre-`anti_unify`
    /// filters, each a PURE predicate on a candidate pair (pass/reject). A pair must
    /// pass ALL listed filters to reach `anti_unify`. The cascade is a conjunction, so
    /// order is result-invariant — it sets only short-circuit cost, never recall. Known
    /// filters:
    ///   - `coverage` — the §0.3 coverage-fraction filter, scoped to LANDMARK candidacy
    ///     (threshold = `landmark_coverage_min`). It runs in the landmark retriever's
    ///     phase, not as a per-pair `&Ctx` predicate. It applies UNCONDITIONALLY — there is
    ///     no second layer to re-propose a pair it drops — and is recall-neutral (measured:
    ///     no output change).
    ///   - `offset-histogram` — the Shazam Δoffset diagonal (spec §5.6), a consistency
    ///     predicate over the shared floor-3 subtree evidence.
    ///   - `h-tree` — H-tree-verify, the Δdepth diagonal (docs/substantiality-metric.md
    ///     §0.5), a consistency predicate over the SAME shared evidence (no extra walk).
    ///
    /// `anti_unify` is deliberately NOT a filter: it is the FIXED TERMINAL producer
    /// (yields the match, not pass/fail) and the dominant cost, run once on the pairs
    /// that survive every filter — the cascade exists precisely to minimize how many
    /// reach it (see its call site in `matchtree`). Keeping it out keeps this list
    /// homogeneous (pure predicates) and un-misorderable. The default
    /// `[coverage, offset-histogram, h-tree]` was promoted from `[coverage,
    /// offset-histogram]` after the whole-repo + inline-on byte-identical `verified_pairs`
    /// gate held (187=187, every clone-group set identical; only the diagnostic
    /// filter-rejection count moved, +98 = 98 fewer anti_unify calls — now attributed
    /// to `htree_rejected`). Unknown names error at
    /// load. Matching-time only — hash-neutral, never in the extraction cache key.
    pub filters: Vec<String>,

    /// How many following offset-sorted subtree peaks each peak is combinatorially
    /// paired with when forming constellation hashes (Shazam-style fan-out). The index
    /// carries ~`fan_out` hashes per peak, so this scales index size almost linearly.
    /// 0 disables landmark hashing (no pairs ⇒ no candidates). The dominant lever on
    /// the landmark constellation index — the largest memory row in a big scan — and
    /// on landmark generation, which is 41–46% of near-phase wall. Matching-time only —
    /// hash-neutral, never in the extraction cache key.
    pub landmark_fan_out: usize,
}

impl Default for RetrievalCfg {
    fn default() -> Self {
        RetrievalCfg {
            retriever: "landmark".into(),
            landmark_pairs: true,
            shared_landmarks_min: 2,
            owner_pair_window: 0,
            landmark_coverage_min: 0.05,
            filters: vec![
                "coverage".into(),
                "offset-histogram".into(),
                "h-tree".into(),
            ],
            landmark_fan_out: 3,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ScanCfg {
    /// Globs excluded from scanning, additive to .gitignore.
    pub exclude: Vec<String>,
    /// Path globs treated as generated code and skipped (spec §5.1).
    pub generated_paths: Vec<String>,
    /// Marker strings in the first 5 lines that flag a file as generated.
    pub generated_markers: Vec<String>,
}

impl Default for ScanCfg {
    fn default() -> Self {
        ScanCfg {
            exclude: Vec::new(),
            generated_paths: vec![
                "**/generated/**".into(),
                "*.pb.go".into(),
                "*_pb2.py".into(),
            ],
            generated_markers: vec![
                "@generated".into(),
                "DO NOT EDIT".into(),
                // Go generator convention (`// Code generated by X`) — many
                // generators omit the "DO NOT EDIT" suffix (M4a).
                "Code generated by".into(),
            ],
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Thresholds {
    /// Post-normalization unit size floor — the single most important
    /// precision knob (spec §5.1). Calibrated for the historical normalizer;
    /// the IR path uses `min_unit_tokens_ir` (its trees are more compact).
    pub min_unit_tokens: u32,
    /// Same floor for the `"ir"` normalizer. The IR canonical trees measured
    /// ~18% more compact than the historical grammar trees, so a real clone
    /// scores fewer tokens on the IR path and the shared 40 floor would filter
    /// reportable IR units historical keeps (§8 flip blocker). Set to the
    /// compaction-scaled value (40 × 0.815 ≈ 33): restores wild-net recall
    /// parity at zero measured precision cost. See `Config::min_unit_floor`.
    pub min_unit_tokens_ir: u32,
    pub min_seq_tokens: u32,
    /// Token floor for a subtree to enter `bag_set`. NOT a retrieval threshold: `bag_set`
    /// is not itself a candidate layer — it is a standalone bench-only helper
    /// (`matchtree::bag_set`) consumed by the retrieval bake-off's comparison retrievers
    /// (`examples/bakeoff.rs`), not part of `UnitDigest`.
    pub bag_min_subtree_tokens: u32,
    /// **Not a retrieval threshold, and not read by the core at all.**
    ///
    /// It is live only because `reprise-mcp` reuses the value as a **size-compatibility
    /// ratio** in its own pre-AU prefilter (`size_compatible(a_tokens, b_tokens, ratio)`) —
    /// a semantically unrelated use that happens to want a number near 0.70. That coupling
    /// is accidental: MCP should own its own key, and this one should then go.
    pub candidate_sim: f64,
    pub hole_hash_min_cover: f64,
    pub histogram_min_votes: u32,
    /// Same near-tier offset-histogram vote floor for the `"ir"` normalizer. The IR canonical
    /// trees are ~18% more compact (see `min_unit_tokens_ir`), so the SAME small near-clone
    /// offers proportionally fewer shared subtrees to vote a diagonal — the shared 5-vote bar
    /// over-filters a real IR-path pair historical keeps (§8 flip blocker: wild w6 lands at
    /// exactly 4 shared aligned subtrees). Set to the compaction-scaled value (5 × 0.815 ≈ 4):
    /// restores wild-net recall parity at a negligible precision cost (a near-tier pre-filter;
    /// AU's own divergence/hole gates still apply). See `Config::histogram_min_votes`.
    pub histogram_min_votes_ir: u32,
    pub max_divergence: f64,
    pub max_holes: u32,
    pub fold_min_repeats: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            min_unit_tokens: 40,
            min_unit_tokens_ir: 33,
            min_seq_tokens: 30,
            bag_min_subtree_tokens: 6,
            candidate_sim: 0.70,
            hole_hash_min_cover: 0.5,
            histogram_min_votes: 5,
            histogram_min_votes_ir: 4,
            // Spec §9 guessed 0.15; set empirically per §7.4(c) — see
            // CALIBRATION.md (whole-expression AU holes on realistic
            // subtree-substitution clones land at ~0.17).
            max_divergence: 0.18,
            max_holes: 5,
            fold_min_repeats: 3,
        }
    }
}

/// Which normalizer produces the canonical tree fed to matching (spec §5.2, D-IR-3).
///
/// A **bounded, validated** selector: unlike the plugin-open `retrieval.retriever`
/// String, an unknown value fails at config-load deserialization (serde
/// `unknown variant ...`), so a typo can never silently fall through to the
/// historical path. The two known selectors are `Ir` (`"ir"`) — the `src/frontend` +
/// `src/ir` canonical IR (D-IR-1a/D-IR-3, feature-complete across Rust/Python/Go; **the
/// default** as of the §8 switchover) — and `Historical` (`"historical"`) — the
/// per-language `src/lang` profiles, still fully supported and selectable.
///
/// The IR trees are ~18% more compact, which is why the size floor is per-normalizer:
/// the IR path uses `[thresholds] min_unit_tokens_ir` (33, the compaction-scaled floor —
/// see `Config::min_unit_floor`), recalibrated so IR-path recall reaches parity with
/// historical without a precision cost. A language without an IR frontend (TS, Kotlin)
/// falls back to the historical path even when `Ir` is selected (a per-language
/// capability gate, §9 — see `unit::is_ir`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Normalizer {
    #[default]
    Ir,
    Historical,
}

impl Normalizer {
    /// The exact config string for this normalizer (`"ir"` / `"historical"`).
    /// Load-bearing: `cache::key` feeds these bytes into the extraction cache key,
    /// so they MUST stay byte-identical to the pre-enum String values or warm
    /// caches would be invalidated.
    pub fn as_str(&self) -> &'static str {
        match self {
            Normalizer::Ir => "ir",
            Normalizer::Historical => "historical",
        }
    }
}

impl std::str::FromStr for Normalizer {
    type Err = String;

    /// Parse a plain string (e.g. an env var) into a `Normalizer`, rejecting
    /// unknowns — the same bounded set serde's `Deserialize` accepts.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ir" => Ok(Normalizer::Ir),
            "historical" => Ok(Normalizer::Historical),
            other => Err(format!(
                "unknown normalizer {other:?} (expected one of: ir, historical)"
            )),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct NormalizeCfg {
    /// Literals whose identity is structural (spec §5.2.5). String literals are
    /// compared by their inner content, numeric literals by their text.
    pub literal_keep: Vec<String>,
    /// Which normalizer produces the canonical tree fed to matching. See [`Normalizer`]:
    /// a bounded, load-validated selector — an unknown value (e.g. a typo `"irr"`) fails
    /// at deserialize time rather than silently selecting the historical path.
    pub normalizer: Normalizer,
}

impl Default for NormalizeCfg {
    fn default() -> Self {
        NormalizeCfg {
            literal_keep: vec!["0".into(), "1".into(), "-1".into(), "".into()],
            normalizer: Normalizer::Ir,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ReportCfg {
    pub top: usize,
    /// Regions below this token count are "micro": still reported/ranked, but
    /// the summary line counts substantial regions separately so the headline
    /// number reflects what the ranking actually values.
    pub micro_region_tokens: u32,
    /// PR mode: minimum tier that fails CI; "none" disables (Phase 3).
    pub fail_on: String,
    /// SARIF `partialFingerprints` source (spec §6.1/§9): "structural" sets our
    /// stable structural/template-hash key so an alert survives cosmetic drift
    /// (recommended); "line" omits it so GitHub falls back to
    /// `primaryLocationLineHash` (the churn-on-every-edit default).
    pub sarif_fingerprint: String,
}

impl Default for ReportCfg {
    fn default() -> Self {
        ReportCfg {
            top: 20,
            micro_region_tokens: 60,
            fail_on: "exact-normalized".into(),
            sarif_fingerprint: "structural".into(),
        }
    }
}

impl Config {
    /// Load `reprise.toml` from the scan root if present, else defaults.
    pub fn load(root: &Path) -> anyhow::Result<Config> {
        let path = root.join("reprise.toml");
        if path.is_file() {
            let text = std::fs::read_to_string(&path)?;
            let config: Config = toml::from_str(&text)?;
            config.validate()?;
            Ok(config)
        } else {
            Ok(Config::default())
        }
    }

    /// Effective post-normalization unit-size floor for the active normalizer.
    /// The IR path (`normalizer = "ir"`) uses its own, lower floor because its
    /// canonical trees are ~18% more compact than the historical grammar trees
    /// (measured mean 84.9 vs 104.2 tokens/unit on the self+wild corpus), so the
    /// SAME real clone scores fewer tokens on the IR path — e.g. serde's
    /// `visit_str`/`visit_borrowed_str` near-clone is 61 tokens historical but 33
    /// IR. A shared 40 floor would filter reportable IR units historical keeps
    /// (the §8 switch-over recall blocker). Historical keeps 40, so the default
    /// path is byte-for-byte unchanged. This is a report/filter parameter, NOT a
    /// canonical-form input (it does not enter the fingerprint or cache key).
    /// A non-IR language under `normalizer = "ir"` falls back to the historical
    /// normalizer (§9), so its units are uncompacted and this floor is marginally
    /// looser for them — an accepted edge of the capability-gap fallback.
    pub fn min_unit_floor(&self) -> u32 {
        if self.normalize.normalizer == Normalizer::Ir {
            self.thresholds.min_unit_tokens_ir
        } else {
            self.thresholds.min_unit_tokens
        }
    }

    /// Effective near-tier offset-histogram vote floor for the active normalizer. The IR path
    /// (`normalizer = "ir"`) uses its own, lower floor because its canonical trees are ~18%
    /// more compact than the historical grammar trees (see `min_unit_floor`): the SAME small
    /// near-clone offers proportionally fewer shared subtrees (≥3 tokens) to vote a common
    /// Δoffset diagonal, so the historical 5-vote bar rejects a real IR-path pair before AU —
    /// e.g. flask's `max_content_length`/`max_form_memory_size` near-clone lands at exactly 4
    /// shared aligned subtrees on the IR path (the §8 switch-over recall blocker), yet AU then
    /// accepts it at divergence 0.06. Historical keeps 5, so the default path is unchanged.
    /// This is a report/filter parameter — a near-tier pre-filter — NOT a canonical-form input
    /// (it does not enter the fingerprint or cache key), and AU's divergence/hole gates still
    /// bound precision. A non-IR language under `normalizer = "ir"` falls back to the historical
    /// normalizer (§9), but this floor is matching-time (post-extraction), so its trees still
    /// use the historical size — an accepted edge of the capability-gap fallback.
    pub fn histogram_min_votes(&self) -> u32 {
        if self.normalize.normalizer == Normalizer::Ir {
            self.thresholds.histogram_min_votes_ir
        } else {
            self.thresholds.histogram_min_votes
        }
    }

    /// Reject invalid enumerated values at load time with the full value list
    /// (a typo in `fail_on` must not surface only when `check` runs in CI).
    fn validate(&self) -> anyhow::Result<()> {
        crate::report::Tier::parse_fail_on(&self.report.fail_on)
            .map_err(|e| anyhow::anyhow!("reprise.toml [report] fail_on: {e}"))?;
        crate::matchtree::validate_filters(&self.retrieval.filters)
            .map_err(|e| anyhow::anyhow!("reprise.toml [retrieval] filters: {e}"))?;
        // The retriever must be one the SHIPPING binary actually runs. The bake-off's
        // alternatives (minhash-lsh, winnowing, sourcerer-rare) exist bench-side only,
        // so naming one here selected the landmark retriever anyway — silently ignoring
        // the request. A config key that accepts a value it does not honor is worse than
        // one that errors, so the accepted set is exactly `matchtree::KNOWN_RETRIEVERS`.
        crate::matchtree::validate_retriever(&self.retrieval.retriever)
            .map_err(|e| anyhow::anyhow!("reprise.toml [retrieval] retriever: {e}"))?;
        // Enumerated string keys whose consumer silently treats an unknown value
        // as a fallthrough default. `tests.mode` is the load-bearing one: an
        // unknown value falls through lib.rs's partition catch-all and routes
        // all-test groups into the CI-gating `main` section (they get ranked and
        // counted in the duplication ratio). Allowed set = exactly what that
        // switch understands: `separate` / `exclude`, plus `normal` (the
        // catch-all's intent). `sarif_fingerprint`'s consumer is `!= "line"`, so
        // a typo silently reads as `structural`; its set is the documented pair.
        // (`normalize.normalizer` IS bounded — the `Normalizer` enum — so serde
        // rejects an unknown value at deserialize time, before `validate` runs.)
        validate_enum(
            "[tests] mode",
            &self.tests.mode,
            &["normal", "separate", "exclude"],
        )?;
        validate_enum(
            "[report] sarif_fingerprint",
            &self.report.sarif_fingerprint,
            &["structural", "line"],
        )?;
        // `memory.force_gate`'s consumer (src/memory.rs `force_override`) treats an
        // unknown value as "auto" — a typo like "alway" would silently gate on the
        // estimate instead of forcing the spill path.
        validate_enum(
            "[memory] force_gate",
            &self.memory.force_gate,
            &["auto", "always", "never"],
        )?;
        // `fold_min_repeats` is a run-length floor AND the divisor in fold.rs's period loop
        // (`(n - i) / min_repeats`). Zero divides by zero and panics mid-scan, so reject it at
        // load: a repeat is at least two occurrences, so the meaningful floor is >= 1.
        if self.thresholds.fold_min_repeats == 0 {
            anyhow::bail!(
                "reprise.toml [thresholds] fold_min_repeats: must be >= 1 (got 0); \
                 it is the minimum run length AND the fold-period divisor"
            );
        }
        Ok(())
    }
}

/// Reject an enumerated string value outside its allowed set, naming both the
/// bad value and the full allowed list — so a typo (e.g. `mode = "excluded"`)
/// fails clearly at load, never as silent misbehavior at `check`-time.
fn validate_enum(key: &str, value: &str, allowed: &[&str]) -> anyhow::Result<()> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        anyhow::bail!(
            "reprise.toml {key}: unknown value {value:?} (expected one of: {})",
            allowed.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, Normalizer};

    #[test]
    fn rejects_unknown_normalizer_at_load() {
        // A typo like `"irr"` must fail at DESERIALIZE (load) time — not load clean
        // and silently select the historical path. `Normalizer` is a bounded enum,
        // so serde errors with `unknown variant`, naming the bad value.
        let toml = "[normalize]\nnormalizer = \"irr\"\n";
        let err = toml::from_str::<Config>(toml).unwrap_err().to_string();
        assert!(err.contains("irr"), "error names the bad value: {err}");
    }

    #[test]
    fn inline_scc_and_budget_defaults_are_pinned() {
        // `max_scc_depth = 1` is the documented "inline an SCC partner once" intent
        // (src/inline.rs module doc); the aggregate
        // `max_expansion_nodes = 250_000` backstop sits 20x above the largest unit
        // measured on any of eight corpora (redis, 12,232 spliced nodes) and ~100x
        // below the 25.1 M-node pathology it exists to stop. Zero units truncate at
        // any value in [25k, 500k], so the default is inert by measurement, not hope.
        let cfg = Config::default();
        assert_eq!(cfg.inline.max_scc_depth, 1);
        assert_eq!(cfg.inline.max_expansion_nodes, 250_000);
    }

    #[test]
    fn accepts_both_known_normalizers_at_load() {
        // Both bounded values deserialize to the right variant.
        for (v, want) in [
            ("ir", Normalizer::Ir),
            ("historical", Normalizer::Historical),
        ] {
            let toml = format!("[normalize]\nnormalizer = \"{v}\"\n");
            let cfg: Config = toml::from_str(&toml).unwrap();
            assert_eq!(cfg.normalize.normalizer, want, "normalizer={v}");
            assert!(cfg.validate().is_ok(), "should accept normalizer={v}");
        }
    }

    #[test]
    fn normalizer_default_is_ir_and_round_trips_as_str() {
        assert_eq!(Config::default().normalize.normalizer, Normalizer::Ir);
        assert_eq!(Normalizer::Ir.as_str(), "ir");
        assert_eq!(Normalizer::Historical.as_str(), "historical");
    }

    #[test]
    fn default_filters_are_coverage_offset_h_tree() {
        // The promoted default: coverage (landmark-scoped) + both consistency filters.
        // anti_unify is NOT a filter — it is the fixed terminal producer.
        assert_eq!(
            Config::default().retrieval.filters,
            vec![
                "coverage".to_string(),
                "offset-histogram".to_string(),
                "h-tree".to_string(),
            ]
        );
    }

    #[test]
    fn validate_rejects_a_retriever_the_binary_does_not_honor() {
        // `winnowing` is a BENCH-SIDE entrant (examples/bakeoff.rs), not something
        // the shipping binary can select. Accepting it silently ran the landmark
        // retriever anyway — a config that lies. It must fail at LOAD, naming the
        // bad value and the set that actually works.
        let toml = "[retrieval]\nretriever = \"winnowing\"\n";
        let cfg: Config = toml::from_str(toml).unwrap();
        let err = cfg.validate().unwrap_err().to_string();
        assert!(
            err.contains("winnowing"),
            "error names the bad value: {err}"
        );
        assert!(err.contains("landmark"), "error lists the known set: {err}");
        assert!(err.contains("retriever"), "error names the key: {err}");
    }

    #[test]
    fn validate_accepts_the_shipping_retriever() {
        let toml = "[retrieval]\nretriever = \"landmark\"\n";
        let cfg: Config = toml::from_str(toml).unwrap();
        assert!(cfg.validate().is_ok());
        // And it is the default.
        assert_eq!(Config::default().retrieval.retriever, "landmark");
    }

    #[test]
    fn validate_rejects_an_unknown_filter() {
        let toml = "[retrieval]\nfilters = [\"offset-histogram\", \"bogus\"]\n";
        let cfg: Config = toml::from_str(toml).unwrap();
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("bogus"), "error names the bad filter: {err}");
        assert!(err.contains("coverage"), "error lists the known set: {err}");
    }

    #[test]
    fn validate_rejects_anti_unify_as_a_filter() {
        // anti_unify is the fixed terminal, never a filter — listing it is an error.
        let toml = "[retrieval]\nfilters = [\"offset-histogram\", \"anti-unify\"]\n";
        let cfg: Config = toml::from_str(toml).unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_unknown_tests_mode() {
        // The typo `"excluded"` must fail at LOAD, not fall through lib.rs's
        // catch-all and silently route all-test groups into the CI-gating
        // `main` section.
        let toml = "[tests]\nmode = \"excluded\"\n";
        let cfg: Config = toml::from_str(toml).unwrap();
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("excluded"), "error names the bad value: {err}");
        assert!(err.contains("mode"), "error names the key: {err}");
        assert!(err.contains("separate"), "error lists the known set: {err}");
    }

    #[test]
    fn validate_accepts_all_valid_tests_modes() {
        // The exact set lib.rs's partition switch understands.
        for mode in ["normal", "separate", "exclude"] {
            let toml = format!("[tests]\nmode = \"{mode}\"\n");
            let cfg: Config = toml::from_str(&toml).unwrap();
            assert!(cfg.validate().is_ok(), "should accept mode={mode}");
        }
    }

    #[test]
    fn validate_rejects_unknown_sarif_fingerprint() {
        let toml = "[report]\nsarif_fingerprint = \"structual\"\n";
        let cfg: Config = toml::from_str(toml).unwrap();
        let err = cfg.validate().unwrap_err().to_string();
        assert!(
            err.contains("structual"),
            "error names the bad value: {err}"
        );
        assert!(err.contains("line"), "error lists the known set: {err}");
    }

    #[test]
    fn validate_accepts_valid_sarif_fingerprints() {
        for v in ["structural", "line"] {
            let toml = format!("[report]\nsarif_fingerprint = \"{v}\"\n");
            let cfg: Config = toml::from_str(&toml).unwrap();
            assert!(
                cfg.validate().is_ok(),
                "should accept sarif_fingerprint={v}"
            );
        }
    }

    #[test]
    fn validate_rejects_fold_min_repeats_zero() {
        // `fold_min_repeats = 0` is the divisor in fold.rs's period loop — it must fail at
        // LOAD, not panic 'attempt to divide by zero' mid-scan.
        let toml = "[thresholds]\nfold_min_repeats = 0\n";
        let cfg: Config = toml::from_str(toml).unwrap();
        let err = cfg.validate().unwrap_err().to_string();
        assert!(
            err.contains("fold_min_repeats"),
            "error names the key: {err}"
        );
        assert!(err.contains(">= 1"), "error states the bound: {err}");
    }

    #[test]
    fn validate_accepts_positive_fold_min_repeats() {
        for n in [1u32, 2, 3, 8] {
            let toml = format!("[thresholds]\nfold_min_repeats = {n}\n");
            let cfg: Config = toml::from_str(&toml).unwrap();
            assert!(cfg.validate().is_ok(), "should accept fold_min_repeats={n}");
        }
    }

    #[test]
    fn validate_accepts_reordered_and_empty_filter_lists() {
        // Filters are reorder-safe (a conjunction); an empty list is legal
        // (anti_unify still runs as the fixed terminal).
        for spec in [
            "filters = [\"h-tree\", \"offset-histogram\", \"coverage\"]",
            "filters = []",
        ] {
            let cfg: Config = toml::from_str(&format!("[retrieval]\n{spec}\n")).unwrap();
            assert!(cfg.validate().is_ok(), "should accept: {spec}");
        }
    }
}
