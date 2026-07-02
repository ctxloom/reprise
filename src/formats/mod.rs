//! Output-format conformance to prior-art standards (spec §6.1): SARIF 2.1.0
//! (`sarif`), PMD/CPD XML (`cpd`), and jscpd JSON (`jscpd`). These are pure
//! serializers over a finished `ScanReport`/`CheckReport` — no matching,
//! grouping, or ranking happens here (the M3d working agreement: a serializer,
//! not a matcher). The emitters read member source files to slice code
//! fragments (CPD `<codefragment>`, jscpd `fragment`).
//!
//! Section policy (D28), shared by all three formats:
//! - `main` findings always emit;
//! - `test` and `api` sections emit in SARIF only, as `note`-level results with
//!   a `properties.section` marker; CPD/jscpd take `main` + `test` (both carry
//!   shared code) but never `api` (structurally different by construction — no
//!   shared fragment to slice);
//! - `weak` emits only under `--verbose`;
//! - CPD/jscpd need ≥2 concrete locations, so single-member `internal-repeat`
//!   findings are excluded from those two formats.

pub mod cpd;
pub mod jscpd;
pub mod sarif;

use crate::baseline::relative_file;
use crate::check::CheckReport;
use crate::report::{Group, ScanReport, Tier};
use std::path::{Path, PathBuf};

/// A group carries emittable shared code for CPD/jscpd iff it has ≥2 concrete
/// locations and is not the structurally-different api-profile tier (D28).
fn has_shared_code(g: &Group) -> bool {
    g.members.len() >= 2 && g.tier != Tier::ApiProfile
}

/// Groups CPD/jscpd emit for a scan: `main` + `test`, plus `weak` under
/// verbose (D28). `api` is excluded (no fragment); single-member
/// `internal-repeat` is filtered by the ≥2-location rule.
fn cpd_jscpd_scan_groups(report: &ScanReport, verbose: bool) -> Vec<&Group> {
    let mut out: Vec<&Group> = Vec::new();
    out.extend(report.groups.iter().filter(|g| has_shared_code(g)));
    out.extend(report.test_groups.iter().filter(|g| has_shared_code(g)));
    if verbose {
        out.extend(report.weak_groups.iter().filter(|g| has_shared_code(g)));
    }
    out
}

/// Groups CPD/jscpd emit for a check: every finding's group with shared code.
fn cpd_jscpd_check_groups(report: &CheckReport) -> Vec<&Group> {
    report
        .findings
        .iter()
        .map(|f| &f.group)
        .filter(|g| has_shared_code(g))
        .collect()
}

/// Absolute filesystem path for reading a member's source: member paths are
/// absolute in scan mode and root-relative in check mode.
fn resolve(root: &Path, file: &Path) -> PathBuf {
    if file.is_absolute() {
        file.to_path_buf()
    } else {
        root.join(file)
    }
}

/// Root-relative, slash-separated display path (SARIF `uri`, CPD `path`, jscpd
/// `name`). Works for both absolute (scan) and already-relative (check) members.
fn rel(root: &Path, file: &Path) -> String {
    relative_file(file, root)
}

/// The source lines `[start, end]` (1-based, inclusive) of a member, for the
/// CPD `<codefragment>`/jscpd `fragment`. Returns `String::new()` if the file
/// can't be read (deleted, unreadable) — a fragment is evidence, never load-
/// bearing for the emitter's validity.
fn read_slice(root: &Path, file: &Path, span: (u32, u32)) -> String {
    let Ok(src) = std::fs::read_to_string(resolve(root, file)) else {
        return String::new();
    };
    let (start, end) = span;
    if start == 0 {
        return String::new();
    }
    let lines: Vec<&str> = src.lines().collect();
    let lo = (start as usize - 1).min(lines.len());
    let hi = (end as usize).min(lines.len());
    if lo >= hi {
        return String::new();
    }
    lines[lo..hi].join("\n")
}
