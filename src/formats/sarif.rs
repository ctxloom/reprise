//! SARIF 2.1.0 emitter (spec §6.1). Implements the three clone-specific mapping
//! decisions the default recipe gets wrong:
//!
//! 1. **A duplicate group is ONE result at N locations, not N results.** The
//!    primary `locations[0]` is the highest-ranked member (scan) or the
//!    diff-touched member (check); the rest ride in `relatedLocations[]` with
//!    integer ids, and `message.text` references them GitHub-style as
//!    `[member](id)` (the multi-location convention).
//! 2. **`partialFingerprints` is structural, not line-based** — keyed
//!    `reprise/structuralFingerprint/v1` to the group's stable structural/
//!    template hash (`Group.fingerprint`), so an alert survives cosmetic drift
//!    instead of churning on every whitespace/rename edit. Config
//!    `report.sarif_fingerprint = "line"` omits our key so GitHub falls back to
//!    `primaryLocationLineHash`.
//! 3. **`level` is NOT overloaded with the tier taxonomy.** The tier lives in
//!    `properties.tier`; `level` is only the coarse CI-gate map (at/above
//!    `fail_on` → error, api-profile/weak → note, else warning).

use crate::check::CheckReport;
use crate::config::Config;
use crate::report::{Group, ScanReport, Tier};
use serde_json::{Value, json};
use std::path::Path;

/// The structural-fingerprint partialFingerprints key — the §6.1 headline
/// decision. Versioned so a fingerprint-scheme change can rev the key.
const FINGERPRINT_KEY: &str = "reprise/structuralFingerprint/v1";

/// One rule per tier (spec §6.1: `driver.rules[]` with `shortDescription`).
const TIER_RULES: &[(&str, &str, &str)] = &[
    (
        "inconsistent-update",
        "InconsistentUpdate",
        "A change touched some but not all members of a known duplicate group; the untouched members may now be stale (drift/fault window).",
    ),
    (
        "exact-normalized",
        "ExactNormalized",
        "Functions that are identical after normalization (Type-1/Type-2 clones).",
    ),
    (
        "internal-repeat",
        "InternalRepeat",
        "A function contains a run of near-identical statement groups — extract a loop or helper.",
    ),
    (
        "near-normalized",
        "NearNormalized",
        "Near-miss clones sharing an anti-unification template with factorable holes (Type-3).",
    ),
    (
        "exact-region",
        "ExactRegion",
        "An exact duplicated token run spanning part of two functions (sequence tier).",
    ),
    (
        "inline-assisted",
        "InlineAssisted",
        "A clone found only after inline-expanding a shared helper (reimplemented-helper duplication).",
    ),
    (
        "api-profile",
        "ApiProfile",
        "Suspicion-only: two functions share a rare external-call profile (possible reimplemented task).",
    ),
    (
        "weak-similarity",
        "WeakSimilarity",
        "Demoted near-miss over the hole budget or with non-factorable holes (verbose only, never fails CI).",
    ),
];

/// Coarse `level` for the CI gate ONLY (spec §6.1 "do not overload level"):
/// api-profile/weak → note; tier at or above `fail_on` → error; else warning.
fn coarse_level(tier: Tier, fail_threshold: Option<u32>) -> &'static str {
    match tier {
        Tier::ApiProfile | Tier::WeakSimilarity => "note",
        _ => match (tier.fail_rank(), fail_threshold) {
            (Some(r), Some(t)) if r <= t => "error",
            _ => "warning",
        },
    }
}

/// Shared emit context, threaded through result building (keeps the per-result
/// argument count sane).
struct Ctx<'a> {
    cfg: &'a Config,
    fail_threshold: Option<u32>,
    root: &'a Path,
}

fn physical_location(root: &Path, file: &Path, span: (u32, u32)) -> Value {
    let start = span.0.max(1);
    let end = span.1.max(start);
    json!({
        "physicalLocation": {
            "artifactLocation": { "uri": super::rel(root, file) },
            "region": { "startLine": start, "endLine": end }
        }
    })
}

/// Build one SARIF result for a group, primary location at `primary_idx`.
/// `forced_note` pins `level` to "note" (test/api/weak sections); otherwise the
/// coarse CI map applies. `extra_props` are merged into `properties`.
fn result_for_group(
    group: &Group,
    primary_idx: usize,
    section: &str,
    forced_note: bool,
    ctx: &Ctx,
    extra_props: &[(&str, Value)],
) -> Value {
    let (cfg, fail_threshold, root) = (ctx.cfg, ctx.fail_threshold, ctx.root);
    let primary_idx = primary_idx.min(group.members.len().saturating_sub(1));
    let primary = &group.members[primary_idx];
    let locations = vec![physical_location(root, &primary.file, primary.line_span)];

    // Related locations get integer ids 1..; message links reference them.
    let mut related = Vec::new();
    let mut links = Vec::new();
    let mut next_id = 1i64;
    for (i, m) in group.members.iter().enumerate() {
        if i == primary_idx {
            continue;
        }
        let id = next_id;
        next_id += 1;
        related.push(json!({
            "id": id,
            "physicalLocation": physical_location(root, &m.file, m.line_span)["physicalLocation"],
            "message": { "text": format!("{} @ {}:{}-{}", m.name, super::rel(root, &m.file), m.line_span.0, m.line_span.1) }
        }));
        links.push(format!(
            "[{} @ {}:{}-{}]({})",
            m.name,
            super::rel(root, &m.file),
            m.line_span.0,
            m.line_span.1,
            id
        ));
    }

    let mut text = format!(
        "Duplicate group ({} members, tier `{}`",
        group.members.len(),
        group.tier
    );
    if group.divergence > 0.0 {
        text.push_str(&format!(
            ", similarity {:.0}%",
            (1.0 - group.divergence) * 100.0
        ));
    }
    text.push_str(&format!(
        "). Primary: {} @ {}:{}-{}.",
        primary.name,
        super::rel(root, &primary.file),
        primary.line_span.0,
        primary.line_span.1
    ));
    if !links.is_empty() {
        text.push_str(" Also: ");
        text.push_str(&links.join(", "));
        text.push('.');
    }
    if let Some(template) = &group.template {
        text.push_str("\n\nShared template:\n");
        text.push_str(template);
    }

    let level = if forced_note {
        "note"
    } else {
        coarse_level(group.tier, fail_threshold)
    };

    let mut properties = json!({
        "tier": group.tier.to_string(),
        "section": section,
        "divergence": group.divergence,
        "value": group.value,
        "tokenCount": group.token_count,
    });
    if let Some(chain) = &group.inline_chain {
        properties["inlineChain"] = json!(chain);
    }
    for (k, v) in extra_props {
        properties[*k] = v.clone();
    }

    let mut result = json!({
        "ruleId": group.tier.to_string(),
        "level": level,
        "message": { "text": text },
        "locations": locations,
        "properties": properties,
    });
    if !related.is_empty() {
        result["relatedLocations"] = Value::Array(related);
    }
    // §6.1 headline decision: structural fingerprint unless config asks for the
    // GitHub line-hash default (then omit ours entirely).
    if cfg.report.sarif_fingerprint != "line" {
        result["partialFingerprints"] = json!({ FINGERPRINT_KEY: group.fingerprint });
    }
    result
}

fn driver(results: Vec<Value>) -> Value {
    let rules: Vec<Value> = TIER_RULES
        .iter()
        .map(|(id, name, desc)| {
            json!({
                "id": id,
                "name": name,
                "shortDescription": { "text": desc }
            })
        })
        .collect();
    json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": { "driver": {
                "name": "reprise",
                "version": env!("CARGO_PKG_VERSION"),
                "informationUri": "https://crates.io/crates/reprise",
                "rules": rules
            }},
            "results": results
        }]
    })
}

/// SARIF for a full-repo `scan`. `verbose` includes the weak-similarity section
/// (D28).
pub fn scan_sarif(report: &ScanReport, root: &Path, cfg: &Config, verbose: bool) -> String {
    let ctx = Ctx {
        cfg,
        fail_threshold: Tier::parse_fail_on(&cfg.report.fail_on).unwrap_or(None),
        root,
    };
    let mut results = Vec::new();
    for g in &report.groups {
        results.push(result_for_group(g, 0, "main", false, &ctx, &[]));
    }
    for g in &report.api_groups {
        results.push(result_for_group(g, 0, "api", true, &ctx, &[]));
    }
    for g in &report.test_groups {
        results.push(result_for_group(g, 0, "test", true, &ctx, &[]));
    }
    if verbose {
        for g in &report.weak_groups {
            results.push(result_for_group(g, 0, "weak", true, &ctx, &[]));
        }
    }
    serde_json::to_string_pretty(&driver(results)).unwrap_or_default()
}

/// SARIF for `check`: one result per finding, primary at the diff-touched
/// member; `properties` carry the check `kind`/`gate`/trend.
pub fn check_sarif(report: &CheckReport, root: &Path, cfg: &Config) -> String {
    let ctx = Ctx {
        cfg,
        fail_threshold: Tier::parse_fail_on(&report.fail_on).unwrap_or(None),
        root,
    };
    let mut results = Vec::new();
    for f in &report.findings {
        // Primary = first diff-touched member (spec §6.1); fall back to first.
        let primary_idx = f
            .group
            .members
            .iter()
            .position(|m| {
                f.touched
                    .iter()
                    .any(|t| t.file == m.file && t.name == m.name && t.line_span == m.line_span)
            })
            .unwrap_or(0);
        let mut extra: Vec<(&str, Value)> = vec![("kind", json!(f.kind)), ("gate", json!(f.fails))];
        if let Some((was, now)) = f.trend {
            extra.push(("baselineDivergence", json!(was)));
            extra.push(("currentDivergence", json!(now)));
        }
        results.push(result_for_group(
            &f.group,
            primary_idx,
            "main",
            false,
            &ctx,
            &extra,
        ));
    }
    serde_json::to_string_pretty(&driver(results)).unwrap_or_default()
}
