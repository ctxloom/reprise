//! jscpd JSON emitter (spec §6.1, `--format jscpd`): matches jscpd's report
//! shape closely enough to be a pipeline drop-in and to make §7.3's baseline
//! comparison a mechanical diff. Top-level `statistics.total` follows jscpd's
//! `IStatisticRow` field names (`lines, tokens, sources, clones,
//! duplicatedLines, duplicatedTokens, percentage, percentageTokens`) and a
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
    // jscpd's ITokenLocation.position is a stream OFFSET, not a line number.
    // We can't cheaply recover a token index, but the byte offset into the file
    // is the right *kind* of value (and never the line number). None when the
    // file can't be read → the key is simply omitted rather than faked.
    let (start_pos, end_pos) = byte_offsets(root, &member.file, start, end);
    json!({
        "name": super::rel(root, &member.file),
        "start": start,
        "end": end,
        "startLoc": token_loc(start, start_pos),
        "endLoc": token_loc(end, end_pos),
    })
}

/// jscpd `ITokenLocation` = `{ line, column?, position? }`. `position` carries a
/// stream offset (here a byte offset), omitted when it couldn't be computed.
fn token_loc(line: u32, position: Option<u64>) -> Value {
    let mut loc = json!({ "line": line, "column": 1 });
    if let Some(p) = position {
        loc["position"] = json!(p);
    }
    loc
}

/// Byte offset of the start of `start_line` and the end of `end_line` (1-based,
/// inclusive) into `file`. Cheap: one read + a single scan for line starts.
fn byte_offsets(
    root: &Path,
    file: &Path,
    start_line: u32,
    end_line: u32,
) -> (Option<u64>, Option<u64>) {
    let Ok(src) = std::fs::read_to_string(super::resolve(root, file)) else {
        return (None, None);
    };
    // `line_starts[k]` = byte offset of the start of the (k+1)-th line.
    let mut line_starts = vec![0usize];
    for (i, b) in src.bytes().enumerate() {
        if b == b'\n' {
            line_starts.push(i + 1);
        }
    }
    let start_pos = start_line
        .checked_sub(1)
        .and_then(|l| line_starts.get(l as usize).copied())
        .unwrap_or(0);
    // End of the span = start of the line after `end_line` (or EOF for the last).
    let end_pos = line_starts
        .get(end_line as usize)
        .copied()
        .unwrap_or(src.len());
    (Some(start_pos as u64), Some(end_pos as u64))
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
    // jscpd's `IStatisticRow` (packages/core/src/interfaces/statistic.interface.ts).
    // We populate every field reprise can compute from scan data; the two `new*`
    // fields (baseline-delta counts) have no reprise analog and are omitted.
    let report = json!({
        "statistics": {
            "total": {
                "lines": stats.total_lines,
                "tokens": stats.total_tokens,
                "sources": stats.files_scanned,
                "clones": duplicates.len(),
                "duplicatedLines": stats.duplicated_lines,
                "duplicatedTokens": stats.duplicated_tokens,
                "percentage": round2(stats.duplicated_lines_pct),
                "percentageTokens": round2(stats.duplicated_tokens_pct),
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
