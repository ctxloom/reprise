//! Base-state snapshots: the internal serialization of `check`'s transient
//! base-state cache. The persistent baseline is a git REF, never a tracked file
//! (D40) — no `reprise-baseline.json`, no baseline artifact in VCS. `check`
//! scans the base ref itself, records every finding keyed by its stable
//! structural/template fingerprint (§6.1), and caches that snapshot under
//! `.reprise/base-state/`, keyed by commit. A current finding fails only when it
//! is absent from the base state or worsened since it; groups present in the
//! base state are STILL tracked for inconsistent updates (§6).

use crate::report::{Group, ScanReport};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Baseline {
    /// Spec §12: baseline entries carry the fingerprint-scheme version;
    /// a mismatch triggers a re-baseline error, never silent comparison.
    pub fingerprint_scheme: u32,
    pub findings: Vec<BaselineEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaselineEntry {
    /// Stable fingerprint key (hex u128) — see `report::Group::fingerprint`.
    pub fingerprint: String,
    pub tier: String,
    /// Report section at baseline time: main | test | api | weak.
    pub section: String,
    /// Divergence snapshot: the drift-tracking reference point (spec §6).
    pub divergence: f64,
    pub members: Vec<BaselineMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaselineMember {
    /// Root-relative path with `/` separators — stable across checkouts, so a
    /// snapshot stays comparable wherever the base ref is scanned.
    pub file: String,
    pub name: String,
    pub line_span: (u32, u32),
}

/// Root-relative slash-separated rendering of a member path.
pub fn relative_file(file: &Path, root: &Path) -> String {
    let rel = file.strip_prefix(root).unwrap_or(file);
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.join("/")
}

fn entry_of(group: &Group, section: &str, root: &Path) -> BaselineEntry {
    BaselineEntry {
        fingerprint: group.fingerprint.clone(),
        tier: group.tier.to_string(),
        section: section.to_string(),
        divergence: group.divergence,
        members: group
            .members
            .iter()
            .map(|m| BaselineMember {
                file: relative_file(&m.file, root),
                name: m.name.clone(),
                line_span: m.line_span,
            })
            .collect(),
    }
}

/// Every current finding, all sections (spec §2 "writes all current
/// findings"); check mode gates on the `main` section only — the others never
/// fail CI by construction (D20).
pub fn create(report: &ScanReport, root: &Path) -> Baseline {
    let mut findings: Vec<BaselineEntry> = Vec::new();
    for (section, groups) in [
        ("main", &report.groups),
        ("test", &report.test_groups),
        ("api", &report.api_groups),
        ("weak", &report.weak_groups),
    ] {
        findings.extend(groups.iter().map(|g| entry_of(g, section, root)));
    }
    // Deterministic, diff-friendly ordering.
    findings.sort_by(|a, b| {
        (&a.section, &a.tier, &a.fingerprint, &a.members[0].file).cmp(&(
            &b.section,
            &b.tier,
            &b.fingerprint,
            &b.members[0].file,
        ))
    });
    Baseline {
        fingerprint_scheme: crate::fingerprint::FINGERPRINT_SCHEME,
        findings,
    }
}

impl Baseline {
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, format!("{json}\n"))
            .with_context(|| format!("writing baseline {}", path.display()))?;
        Ok(())
    }

    /// Load and validate a transient base-state snapshot. A scheme mismatch is
    /// an error, which callers treat as a cache miss (rescan the base ref) —
    /// stale state is never silently compared (spec §12).
    pub fn load(path: &Path) -> anyhow::Result<Baseline> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading baseline {}", path.display()))?;
        let baseline: Baseline = serde_json::from_str(&text)
            .with_context(|| format!("parsing baseline {}", path.display()))?;
        if baseline.fingerprint_scheme != crate::fingerprint::FINGERPRINT_SCHEME {
            anyhow::bail!(
                "base-state snapshot {} was written with fingerprint scheme {} but this \
                 binary uses scheme {}: rescan required",
                path.display(),
                baseline.fingerprint_scheme,
                crate::fingerprint::FINGERPRINT_SCHEME,
            );
        }
        Ok(baseline)
    }
}
