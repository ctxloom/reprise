//! P8: findings and rendering (spec §6). Every finding carries original source
//! coordinates for all members, the producing tier, and ranking value.

use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

/// Tiers in descending confidence/actionability (spec §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Drift tracking (spec §6, check mode): the diff touched a proper subset
    /// of a baselined group's members. Top of the confidence order — the
    /// tool's most actionable finding.
    InconsistentUpdate,
    ExactNormalized,
    InternalRepeat,
    NearNormalized,
    /// Sequence-tier sub-unit exact run (DECISIONS.md D10).
    ExactRegion,
    /// Match involving an inline-expanded variant (spec §5.4); ranked below
    /// near-normalized per §6, carries its inline chain as evidence.
    InlineAssisted,
    /// Suspicion-only birthmark tier (spec §5.7); own section, never fails CI.
    ApiProfile,
    /// Verbose-only; never fails CI (spec §5.6 demotion).
    WeakSimilarity,
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Tier::InconsistentUpdate => "inconsistent-update",
            Tier::ExactNormalized => "exact-normalized",
            Tier::InternalRepeat => "internal-repeat",
            Tier::NearNormalized => "near-normalized",
            Tier::ExactRegion => "exact-region",
            Tier::InlineAssisted => "inline-assisted",
            Tier::ApiProfile => "api-profile",
            Tier::WeakSimilarity => "weak-similarity",
        };
        write!(f, "{s}")
    }
}

impl Tier {
    /// Every tier, in confidence order — the single list `parse_fail_on`
    /// iterates so the name↔rank mapping can't drift from `Display`/`fail_rank`.
    pub const ALL: &'static [Tier] = &[
        Tier::InconsistentUpdate,
        Tier::ExactNormalized,
        Tier::InternalRepeat,
        Tier::NearNormalized,
        Tier::ExactRegion,
        Tier::InlineAssisted,
        Tier::ApiProfile,
        Tier::WeakSimilarity,
    ];

    /// Position in the §6 confidence order for the CI gate; `None` for tiers
    /// that NEVER fail CI (api-profile, weak-similarity). A finding fails iff
    /// `fail_rank(tier) <= fail_rank(fail_on threshold)`.
    pub fn fail_rank(self) -> Option<u32> {
        match self {
            Tier::InconsistentUpdate => Some(0),
            Tier::ExactNormalized => Some(1),
            Tier::InternalRepeat => Some(2),
            Tier::ExactRegion => Some(3),
            Tier::NearNormalized => Some(4),
            Tier::InlineAssisted => Some(5),
            Tier::ApiProfile | Tier::WeakSimilarity => None,
        }
    }

    /// Parse a `fail_on` value (config `[report].fail_on` or `--fail-on`):
    /// a failable tier name, or "none" to disable the gate. Derived from the
    /// single `ALL`/`fail_rank` source of truth — a tier's name and its gate
    /// rank can't disagree because both come from the tier itself.
    pub fn parse_fail_on(s: &str) -> anyhow::Result<Option<u32>> {
        if s == "none" {
            return Ok(None);
        }
        for &tier in Tier::ALL {
            // Only failable tiers (`fail_rank().is_some()`) are valid thresholds;
            // api-profile/weak never gate, so their names aren't accepted here.
            if tier.fail_rank().is_some() && tier.to_string() == s {
                return Ok(tier.fail_rank());
            }
        }
        anyhow::bail!(
            "unknown fail_on tier `{s}` (expected inconsistent-update, \
             exact-normalized, internal-repeat, exact-region, near-normalized, \
             inline-assisted, or none)"
        )
    }
}

impl Serialize for Tier {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Member {
    pub file: PathBuf,
    pub lang: String,
    pub name: String,
    pub line_span: (u32, u32),
    pub parse_degraded: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Group {
    pub id: String,
    pub tier: Tier,
    /// Stable structural fingerprint key (spec §2/§6.1, hex u128): the shared
    /// normalized hash (exact tiers), the AU template hash (near tiers), the
    /// run's token-content hash (regions), or (unit fp ‖ template hash)
    /// (internal repeats). Baseline entries key on this.
    pub fingerprint: String,
    /// Normalized token count of the shared form (DECISIONS.md D1).
    pub token_count: u32,
    /// Estimated consolidation value (spec §6 ranking).
    pub value: f64,
    /// Cross-group annotations (e.g. "subsumes a 5-member near-normalized group").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// AU divergence ratio; 0 for exact tiers. Similarity = 1 - divergence.
    pub divergence: f64,
    /// Group template as pseudo-source with ⟨hole⟩ markers (near tier), or
    /// the shared rare-callee evidence list (api-profile tier, spec §5.7).
    pub template: Option<String>,
    /// Which callees were expanded to produce the match (inline-assisted tier,
    /// spec §5.4 — "the report must show the inline chain").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline_chain: Option<Vec<String>>,
    pub members: Vec<Member>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Stats {
    /// Groups dropped because their member set was a subset of another group's.
    pub groups_subsumed_subset: usize,
    /// exact-region findings at/above `report.micro_region_tokens`.
    pub regions_substantial: usize,
    pub files_scanned: usize,
    pub files_skipped_generated: usize,
    /// Files dropped because they could not be read as UTF-8 text (I/O error or
    /// non-UTF-8 source): surfaced so the scan can't silently shrink (spec §2).
    pub files_unreadable: usize,
    pub units_indexed: usize,
    pub units_below_floor: u32,
    pub parse_degraded_units: usize,
    pub test_units: usize,
    pub findings_by_tier: BTreeMap<String, usize>,
    pub sequence_regions_found: usize,
    pub sequence_regions_subsumed: usize,
    /// Inline variants that entered the index (≤1 per unit, spec §12 cap).
    pub inline_variants: usize,
    /// Call sites skipped for exceeding `inline.max_candidates` (spec §5.4:
    /// "record ambiguity-skip counts in scan stats so that decision is
    /// data-driven").
    pub ambiguity_skips: usize,
    /// Call sites actually expanded across all variants.
    pub calls_inlined: usize,
    /// Units belonging to a mutual-recursion SCC of size ≥2 (spec §5.4).
    pub scc_units: usize,
    /// Units that got NO inline variant because their expansion exceeded
    /// `inline.max_expansion_nodes` (the aggregate backstop). Distinct from the
    /// benign no-op skips (thin delegation D37 / no resolvable call), which share
    /// the same `Expansion::skipped` shape but are NOT counted here: a backstop
    /// that silently drops a whole unit's inline-tier recall is worse than no
    /// backstop, so it reports itself. Expected 0 on every measured corpus — a
    /// non-zero value means the budget is binding and should be re-sited.
    pub inline_budget_skipped_units: usize,
    /// Units that emitted an api-profile signature (spec §5.7).
    pub api_signatures: usize,
    /// Units suppressed by `reprise:ignore` (spec §2/§12: visible so
    /// suppression can't silently accumulate).
    pub suppressed_units: usize,
    /// D8 per-file cache effectiveness.
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub retrieval: crate::matchtree::RetrievalStats,
    /// Total physical lines across all scanned source files (§6.1 metric base).
    pub total_lines: usize,
    /// Total normalized tokens across plain units (D1 currency; §6.1 base).
    pub total_tokens: u64,
    /// Distinct source lines covered by ≥1 main-section finding member
    /// (union per file; ≤ `total_lines`), and its percentage (§6.1
    /// %duplicated-lines — the GitClear copy/paste-line shape).
    pub duplicated_lines: usize,
    pub duplicated_lines_pct: f64,
    /// Redundant (consolidatable) normalized-token mass over main groups —
    /// `Σ token_count·(members−1)` — and its percentage of `total_tokens`
    /// (§6.1 %duplicated-tokens; more stable across formatting per §6.1).
    pub duplicated_tokens: u64,
    pub duplicated_tokens_pct: f64,
    /// Main-section clone groups per 1,000 scanned lines (§6.1 clones/KLOC).
    pub clones_per_kloc: f64,
    /// Wall time per pipeline phase (extract, inline, near, sequence, api…).
    pub phase_ms: BTreeMap<String, u64>,
    pub duration_ms: u64,
    /// Gate 2 (memory architecture P2) observability — which path ran, and the
    /// spill/reload traffic when the gate tripped. These `memory_*` fields are
    /// the WP's only permitted output addition; the byte-identity harness
    /// strips `stats.phase_ms`, `stats.duration_ms` and `stats.memory_*`.
    pub memory_budget_bytes: u64,
    pub memory_estimated_bytes: u64,
    pub memory_gate_tripped: bool,
    /// Trees spilled to the scan-scoped pack (plain at gate-trip + variants at
    /// `finish_variant`); 0 under the gate.
    pub memory_spilled_trees: usize,
    /// Total serialized bytes across the scan's packs (trees + seq streams).
    pub memory_pack_bytes: u64,
    /// Tree-pack LRU traffic during near-tier verify (hit-rate = hits/(hits+misses)).
    pub memory_lru_hits: u64,
    pub memory_lru_misses: u64,
}

/// Minimal per-unit coordinates check mode needs to map baseline member
/// snapshots onto the current tree state (spec §6.1 tolerance: fingerprints
/// are the stable part, line numbers shift).
#[derive(Debug, Clone, Serialize)]
pub struct UnitSummary {
    pub file: PathBuf,
    pub name: String,
    pub line_span: (u32, u32),
    /// Canonical hex u128 of the unit's exact structural hash (matches
    /// `Group::fingerprint` for the exact-normalized tier). Lets check mode's
    /// span-overlap fallback tell "same duplicate that moved/renamed" from
    /// "deleted; an unrelated function now occupies those lines".
    pub fingerprint: String,
    /// `reprise:accept-drift` (D41): IU on this unit reports as info.
    pub accept_drift: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    pub groups: Vec<Group>,
    /// Test-vs-test findings (spec §5.1 policy `separate`); never fail CI.
    pub test_groups: Vec<Group>,
    /// api-profile findings (spec §5.7): own section, never fails CI.
    pub api_groups: Vec<Group>,
    /// Demoted findings (over hole budget / non-factorable); verbose only.
    pub weak_groups: Vec<Group>,
    pub stats: Stats,
    /// Plain-unit coordinates for check mode (not part of the JSON report).
    #[serde(skip)]
    pub unit_index: Vec<UnitSummary>,
}

impl ScanReport {
    /// Human-format output: top-N findings plus a one-line stats summary
    /// (spec §6). `top == 0` means "all groups".
    pub fn render_terminal(&self, top: usize, verbose: bool) -> String {
        let top = if top == 0 { usize::MAX } else { top };
        let s = &self.stats;
        let tier_summary = if s.findings_by_tier.is_empty() {
            "none".to_string()
        } else {
            s.findings_by_tier
                .iter()
                .map(|(tier, n)| {
                    // The region long tail is real but low-value; the headline
                    // shows what the ranking actually weighs (user feedback).
                    if tier == "exact-region" && *n > s.regions_substantial {
                        format!("{tier}: {} substantial of {n}", s.regions_substantial)
                    } else {
                        format!("{tier}: {n}")
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        // Surface unreadable-file drops only when nonzero — a silent shrink otherwise.
        let unreadable = if s.files_unreadable > 0 {
            format!(" ({} unreadable)", s.files_unreadable)
        } else {
            String::new()
        };
        // The expansion backstop (`inline.max_expansion_nodes`) drops a unit's inline
        // variant WHOLESALE when its expansion blows the budget. That is a real, if
        // bounded, inline-tier recall loss, so it must be visible to the human running
        // the scan — not just in `--format json`. Rendered only when nonzero (it is zero
        // on every measured corpus), same as the unreadable-file drop above: a healthy
        // scan's summary is unchanged.
        let budget_skips = if s.inline_budget_skipped_units > 0 {
            format!(", {} budget-skipped", s.inline_budget_skipped_units)
        } else {
            String::new()
        };
        let mut out = format!(
            "reprise scan: {} files{}, {} units indexed ({} below floor, {} parse-degraded, \
             {} suppressed), inline [{} variants, {} scc units, {} ambiguity skips{}], \
             {} api signatures, cache [{} hits, {} misses], findings [{}] in {}ms\n",
            s.files_scanned,
            unreadable,
            s.units_indexed,
            s.units_below_floor,
            s.parse_degraded_units,
            s.suppressed_units,
            s.inline_variants,
            s.scc_units,
            s.ambiguity_skips,
            budget_skips,
            s.api_signatures,
            s.cache_hits,
            s.cache_misses,
            tier_summary,
            s.duration_ms,
        );
        if self.groups.is_empty() {
            out.push_str("\nno duplicate groups found\n");
        }
        for (i, group) in self.groups.iter().take(top).enumerate() {
            render_group(&mut out, i + 1, group, verbose);
        }
        if self.groups.len() > top {
            out.push_str(&format!(
                "\n… and {} more groups (raise --top or use --format json)\n",
                self.groups.len() - top
            ));
        }
        if !self.test_groups.is_empty() {
            out.push_str(&format!(
                "\n-- test code ({} groups; never fails CI) --\n",
                self.test_groups.len()
            ));
            for (i, group) in self
                .test_groups
                .iter()
                .take(if verbose { top } else { 3 })
                .enumerate()
            {
                render_group(&mut out, i + 1, group, verbose);
            }
        }
        if !self.api_groups.is_empty() {
            out.push_str(&format!(
                "\n-- api-profile ({} suspicion-only findings; never fails CI) --\n",
                self.api_groups.len()
            ));
            for (i, group) in self
                .api_groups
                .iter()
                .take(if verbose { top } else { 3 })
                .enumerate()
            {
                render_group(&mut out, i + 1, group, verbose);
            }
        }
        if verbose && !self.weak_groups.is_empty() {
            out.push_str(&format!(
                "\n-- weak similarity ({} groups; verbose only) --\n",
                self.weak_groups.len()
            ));
            for (i, group) in self.weak_groups.iter().take(top).enumerate() {
                render_group(&mut out, i + 1, group, verbose);
            }
        }
        out
    }
}

/// Verbose mode: dump each member's actual source (capped) under its line —
/// the reviewer sees the real code refs next to the template (D41 feedback).
/// Note: check-mode members render root-relative; resolution falls back to
/// the path as given, so run from the repo root for source dumps in check.
pub(crate) fn render_member_source_block(out: &mut String, member: &Member) {
    render_member_source(out, member)
}

fn render_member_source(out: &mut String, member: &Member) {
    const CAP: usize = 24;
    let Ok(text) = std::fs::read_to_string(&member.file) else {
        return;
    };
    let lines: Vec<&str> = text.lines().collect();
    let start = member.line_span.0.max(1) as usize - 1;
    let end = (member.line_span.1 as usize).min(lines.len());
    if start >= end {
        return;
    }
    for (i, line) in lines[start..end].iter().take(CAP).enumerate() {
        out.push_str(&format!(
            "      {:>4} │ {line}
",
            start + i + 1
        ));
    }
    if end - start > CAP {
        out.push_str(&format!(
            "           │ … {} more lines
",
            end - start - CAP
        ));
    }
}

fn render_group(out: &mut String, rank: usize, group: &Group, verbose: bool) {
    let similarity = if group.divergence > 0.0 {
        format!(" · similarity {:.0}%", (1.0 - group.divergence) * 100.0)
    } else {
        String::new()
    };
    // The api-profile tier overloads `token_count` with its shared-rare-callee
    // count (there is no normalized token mass — no shared structural form), so
    // it needs an accurate label; every other tier prints normalized tokens.
    let count_label = if group.tier == Tier::ApiProfile {
        format!("{} shared rare callees", group.token_count)
    } else {
        format!("{} tokens", group.token_count)
    };
    out.push_str(&format!(
        "\n#{} [{}] value {:.0} · {} members · {}{}\n",
        rank,
        group.tier,
        group.value,
        group.members.len(),
        count_label,
        similarity,
    ));
    for member in &group.members {
        let degraded = if member.parse_degraded {
            "  (parse-degraded)"
        } else {
            ""
        };
        out.push_str(&format!(
            "    {}:{}-{}  {}{}\n",
            member.file.display(),
            member.line_span.0,
            member.line_span.1,
            member.name,
            degraded,
        ));
        if verbose {
            render_member_source(out, member);
        }
    }
    if let Some(chain) = &group.inline_chain {
        for link in chain {
            out.push_str(&format!("    inline: {link}\n"));
        }
    }
    if let Some(note) = &group.note {
        out.push_str(&format!("    note: {note}\n"));
    }
    if let Some(template) = &group.template {
        render_template_block(out, template, 12);
    }
}

/// Shared template-block rendering (consolidated from check-mode rendering —
/// a reprise self-scan finding, like Phase 1's `synth_call`).
pub(crate) fn render_template_block(out: &mut String, template: &str, max_lines: usize) {
    out.push_str("    template:\n");
    for line in template.lines().take(max_lines) {
        out.push_str(&format!("      {line}\n"));
    }
    if template.lines().count() > max_lines {
        out.push_str("      …\n");
    }
}
