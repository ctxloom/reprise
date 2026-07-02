//! jscpd JSON emitter (spec §6.1, `--format jscpd`): matches jscpd's report
//! shape closely enough to be a pipeline drop-in and to make §7.3's baseline
//! comparison a mechanical diff. Top-level `statistics.total` (`{lines,
//! sources, clones, duplicatedLines, percentage}`, plus token figures) and a
//! `duplicates[]` array.
//!
//! Multi-member groups expand to **pairwise** entries — the group's first
//! member vs. each other member (D28) — because jscpd models a clone as a
//! `firstFile`/`secondFile` pair, not an N-way group.

use crate::check::CheckReport;
use crate::report::{Group, ScanReport, Stats};
use serde_json::{Value, json};
use std::path::Path;

fn file_obj(root: &Path, member: &crate::report::Member) -> Value {
    let (start, end) = member.line_span;
    json!({
        "name": super::rel(root, &member.file),
        "start": start,
        "end": end,
        "startLoc": { "line": start, "column": 1, "position": start },
        "endLoc": { "line": end, "column": 1, "position": end },
    })
}

fn duplicates_of(group: &Group, root: &Path, out: &mut Vec<Value>) {
    let first = &group.members[0];
    let lines = first.line_span.1.saturating_sub(first.line_span.0) + 1;
    let fragment = super::read_slice(root, &first.file, first.line_span);
    for other in &group.members[1..] {
        out.push(json!({
            "format": first.lang,
            "lines": lines,
            "tokens": group.token_count,
            "fragment": fragment,
            "firstFile": file_obj(root, first),
            "secondFile": file_obj(root, other),
        }));
    }
}

fn render(groups: &[&Group], stats: &Stats, root: &Path) -> String {
    let mut duplicates = Vec::new();
    for g in groups {
        duplicates_of(g, root, &mut duplicates);
    }
    let report = json!({
        "statistics": {
            "total": {
                "lines": stats.total_lines,
                "sources": stats.files_scanned,
                "clones": duplicates.len(),
                "duplicatedLines": stats.duplicated_lines,
                "percentage": round2(stats.duplicated_lines_pct),
                "duplicatedTokens": stats.duplicated_tokens,
                "tokensPercentage": round2(stats.duplicated_tokens_pct),
            }
        },
        "duplicates": duplicates,
    });
    serde_json::to_string_pretty(&report).unwrap_or_default()
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

pub fn scan_jscpd(report: &ScanReport, root: &Path, verbose: bool) -> String {
    render(
        &super::cpd_jscpd_scan_groups(report, verbose),
        &report.stats,
        root,
    )
}

pub fn check_jscpd(report: &CheckReport, root: &Path) -> String {
    render(&super::cpd_jscpd_check_groups(report), &report.stats, root)
}
