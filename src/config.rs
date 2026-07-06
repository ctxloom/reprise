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
    /// Which candidate-generation retriever runs alongside the shared bag layer.
    /// A **string-keyed, plugin-extensible** selector (mirrors `normalize.normalizer`),
    /// NOT a two-way flag: further retrievers may register later. The default `"landmark"`
    /// is the §5.5.4 rare-peak constellation retriever (the §7.4b rivalry winner). The
    /// bake-off alternatives (minhash-lsh, winnowing, sourcerer-rare) implement the same
    /// `matchtree::Retriever` trait bench-side. Matching-time only — retrieval is
    /// post-fingerprint, so this NEVER enters the extraction cache key (hash-neutral).
    pub retriever: String,
    /// §5.5.4 landmark pairs — rivalry winner (§7.4b, see CALIBRATION.md;
    /// hole-context hashes were dropped per the §5.5 rivalry clause). Master on/off for
    /// the landmark layer; when off, the shared bag layer runs alone.
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
    /// H-tree-verify (docs/substantiality-metric.md §0.5): add a depth-delta
    /// (depth_a−depth_b) consistency criterion alongside the offset-delta (Shazam
    /// diagonal) one, over the SAME shared floor-3 subtrees, in the pre-AU verify
    /// cascade. A genuine clone places its shared subtrees at consistent RELATIVE
    /// depths; a coincidental landmark collision scatters. **Default ON** — promoted
    /// after the whole-repo + inline-on byte-identical `verified_pairs` gate held
    /// clean (verified_pairs 187=187, every clone-group set identical; the only stat
    /// that moved was the diagnostic histogram_rejected, +98 = 98 fewer anti_unify
    /// calls at zero recall loss). A pure candidate pre-filter: hash-neutral, never
    /// in the cache key. Set false to disable the depth criterion (offset-delta only).
    pub tree_verify: bool,
}

impl Default for RetrievalCfg {
    fn default() -> Self {
        RetrievalCfg {
            retriever: "landmark".into(),
            landmark_pairs: true,
            shared_landmarks_min: 2,
            owner_pair_window: 0,
            landmark_coverage_min: 0.05,
            tree_verify: true,
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
    pub bag_min_subtree_tokens: u32,
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

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct NormalizeCfg {
    /// Literals whose identity is structural (spec §5.2.5). String literals are
    /// compared by their inner content, numeric literals by their text.
    pub literal_keep: Vec<String>,
    /// Which normalizer produces the canonical tree fed to matching. A **non-boolean,
    /// plugin-extensible** selector (not a two-way flag) — further normalizer plugins
    /// may register later. Known values: `"ir"` (the `src/frontend` + `src/ir` canonical
    /// IR — D-IR-1a/D-IR-3, feature-complete across Rust/Python/Go; **the default** as of
    /// the §8 switchover) and `"historical"` (the per-language `src/lang` profiles, still
    /// fully supported and selectable). The IR trees are ~18% more compact, which is why
    /// the size floor is per-normalizer: the IR path uses `[thresholds] min_unit_tokens_ir`
    /// (33, the compaction-scaled floor — see `Config::min_unit_floor`), recalibrated so
    /// IR-path recall reaches parity with historical without a precision cost. A language
    /// without an IR frontend (TS, Kotlin) falls back to `"historical"` even when `"ir"`
    /// is selected (a per-language capability gate, §9).
    pub normalizer: String,
}

impl Default for NormalizeCfg {
    fn default() -> Self {
        NormalizeCfg {
            literal_keep: vec!["0".into(), "1".into(), "-1".into(), "".into()],
            normalizer: "ir".into(),
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
        if self.normalize.normalizer == "ir" {
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
        if self.normalize.normalizer == "ir" {
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
        Ok(())
    }
}
