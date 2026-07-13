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

// ---------- base-ref resolution (docs/SERVERS.md §5) ----------

/// How `check` chooses its base ref. Precedence, highest first:
///   1. an explicit base the caller supplies (`--base`, an MCP `base` argument),
///   2. `[baseline] ref` pinned in `reprise.toml` (`config.baseline.pinned`),
///   3. the merge-base of `HEAD` with the repo's default branch — **the PR base**,
///   4. `HEAD`, when git is unavailable or history is too shallow for a base.
///
/// Infallible: it always yields *some* ref, degrading to `HEAD` rather than
/// erroring, so no caller fails a request purely on baseline resolution.
///
/// `check` is a PR check (spec §2): the question it answers is "what does this
/// BRANCH add, relative to the default branch?" — so rung 3 is the default that
/// makes the bare command mean the thing the command is for. It is shared by the
/// CLI and both servers precisely so a local pre-commit run and CI cannot
/// disagree about what is being gated: a base that differs between them turns a
/// green hook into a red PR.
pub fn resolve_base(root: &Path, explicit: Option<&str>, cfg: &crate::Config) -> String {
    explicit_or_pinned(explicit, cfg)
        .unwrap_or_else(|| merge_base_with_default(root).unwrap_or_else(|| "HEAD".to_string()))
}

/// The git-free half of [`resolve_base`]: an explicit base, else the pinned
/// config ref. `None` when neither is set (blank/whitespace counts as unset),
/// leaving the caller to fall back to the git merge-base.
fn explicit_or_pinned(explicit: Option<&str>, cfg: &crate::Config) -> Option<String> {
    if let Some(b) = explicit.map(str::trim).filter(|b| !b.is_empty()) {
        return Some(b.to_string());
    }
    cfg.baseline
        .pinned
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(String::from)
}

/// The merge-base commit between `HEAD` and the repo's default branch. `None`
/// if git is absent, `root` is not a repo, or no base is found — [`resolve_base`]
/// then falls back to `HEAD`.
fn merge_base_with_default(root: &Path) -> Option<String> {
    let default = default_branch(root)?;
    let sha = git_stdout(root, &["merge-base", "HEAD", &default])?;
    let sha = sha.trim();
    (!sha.is_empty()).then(|| sha.to_string())
}

/// The repo's default branch ref: the remote's advertised default
/// (`origin/HEAD` → e.g. `origin/main`), else the first common branch that
/// actually exists.
fn default_branch(root: &Path) -> Option<String> {
    if let Some(sym) = git_stdout(
        root,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) {
        let name = sym.trim();
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    ["origin/main", "origin/master", "main", "master"]
        .into_iter()
        .find(|cand| git_stdout(root, &["rev-parse", "--verify", "--quiet", cand]).is_some())
        .map(String::from)
}

/// Stdout of a git subcommand in `root` on a clean exit, else `None`. Never
/// panics: a missing git binary is just `None`. Spawns through
/// [`crate::check::git_cmd`], which scrubs the hook-injected git env — an
/// inherited `GIT_INDEX_FILE` points a child at the CALLER's index (a
/// pre-commit hook's), not at `root`.
fn git_stdout(root: &Path, args: &[&str]) -> Option<String> {
    let out = crate::check::git_cmd(root).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod base_resolution_tests {
    use super::*;

    #[test]
    fn base_precedence_is_explicit_then_pinned_then_merge_base() {
        let mut cfg = crate::Config::default();
        cfg.baseline.pinned = Some("v1.4.0".into());
        assert_eq!(
            explicit_or_pinned(Some("origin/main"), &cfg).as_deref(),
            Some("origin/main")
        );
        // Blank explicit falls through to the pinned ref.
        assert_eq!(
            explicit_or_pinned(Some("  "), &cfg).as_deref(),
            Some("v1.4.0")
        );
        assert_eq!(explicit_or_pinned(None, &cfg).as_deref(), Some("v1.4.0"));
        // Neither set: the caller falls back to the merge-base with the default
        // branch (the PR base), and finally to HEAD — never an error.
        assert_eq!(explicit_or_pinned(None, &crate::Config::default()), None);
    }
}
