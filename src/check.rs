//! `reprise check --base <git-ref>` (spec §2, §6): PR mode. Parses
//! `git diff -U0` hunk ranges (DECISIONS.md D7a: a unit is touched iff its
//! line span intersects a hunk), scans, then classifies findings against the
//! baseline. A finding fails CI iff its tier is at or above `fail_on` in the
//! §6 confidence order, it involves a touched unit, and it is not baselined —
//! or is baselined but worsened. Baselined groups are STILL tracked for
//! inconsistent updates: accepting a duplicate's existence is not accepting
//! its divergence (spec §6).

use crate::baseline::{Baseline, BaselineEntry, BaselineMember, relative_file};
use crate::config::Config;
use crate::report::{Group, Member, Stats, Tier, UnitSummary};
use anyhow::{Context, bail};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Divergence increase below this is measurement noise, not drift (D20).
pub const DIVERGENCE_EPS: f64 = 0.01;

// ---------- git diff → touched line ranges (D7a) ----------

#[derive(Debug, Default)]
pub struct DiffMap {
    /// Root-relative file → new-side hunk line ranges (1-based, inclusive).
    hunks: HashMap<String, Vec<(u32, u32)>>,
    /// Files the diff deletes: their baseline members count as touched.
    deleted: HashSet<String>,
}

fn spans_overlap(a: (u32, u32), b: (u32, u32)) -> bool {
    a.0 <= b.1 && b.0 <= a.1
}

impl DiffMap {
    pub fn touches(&self, file: &str, span: (u32, u32)) -> bool {
        self.deleted.contains(file)
            || self
                .hunks
                .get(file)
                .is_some_and(|ranges| ranges.iter().any(|&r| spans_overlap(r, span)))
    }

    pub fn file_touched(&self, file: &str) -> bool {
        self.deleted.contains(file) || self.hunks.contains_key(file)
    }
}

fn git_diff(root: &Path, base: &str) -> anyhow::Result<DiffMap> {
    let probe = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .context("running git (is it installed?)")?;
    if !probe.status.success() {
        bail!(
            "`reprise check` requires a git repository at {} (git rev-parse failed: {})",
            root.display(),
            String::from_utf8_lossy(&probe.stderr).trim()
        );
    }
    // --relative both restricts the diff to the scan root and emits paths
    // relative to it; -U0 hunks carry exact touched ranges (D7a).
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "diff",
            "--no-ext-diff",
            "--no-color",
            "--relative",
            "-U0",
            base,
        ])
        .output()
        .context("running git diff")?;
    if !out.status.success() {
        bail!(
            "git diff -U0 {base} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(parse_diff(&String::from_utf8_lossy(&out.stdout)))
}

/// Parse unified-diff hunk headers. Only `---`/`+++` lines inside a file
/// header block are honored, so content lines can't spoof paths (-U0 emits
/// no context lines, but removed lines still start with `-`).
fn parse_diff(text: &str) -> DiffMap {
    let mut map = DiffMap::default();
    let mut in_header = false;
    let mut old_path: Option<String> = None;
    let mut current: Option<String> = None;
    for line in text.lines() {
        if line.starts_with("diff --git ") {
            in_header = true;
            old_path = None;
            current = None;
            continue;
        }
        if in_header {
            if let Some(rest) = line.strip_prefix("--- ") {
                old_path = rest.strip_prefix("a/").map(str::to_string);
                continue;
            }
            if let Some(rest) = line.strip_prefix("+++ ") {
                if rest == "/dev/null" {
                    if let Some(p) = old_path.take() {
                        map.deleted.insert(p);
                    }
                    current = None;
                } else {
                    current = Some(rest.strip_prefix("b/").unwrap_or(rest).to_string());
                }
                continue;
            }
        }
        if let Some(rest) = line.strip_prefix("@@ ") {
            in_header = false;
            let Some(file) = &current else { continue };
            let Some(plus) = rest.split_whitespace().find(|w| w.starts_with('+')) else {
                continue;
            };
            let nums = &plus[1..];
            let parsed = match nums.split_once(',') {
                Some((s, c)) => s.parse::<u32>().ok().zip(c.parse::<u32>().ok()),
                None => nums.parse::<u32>().ok().map(|s| (s, 1)),
            };
            let Some((start, count)) = parsed else {
                continue;
            };
            let range = if count == 0 {
                // Pure deletion: the removal point sits between new-side
                // lines `start` and `start+1` — both are adjacent to it.
                (start.max(1), start + 1)
            } else {
                (start, start + count - 1)
            };
            map.hunks.entry(file.clone()).or_default().push(range);
        }
    }
    map
}

// ---------- classification ----------

#[derive(Debug, Serialize)]
pub struct CheckFinding {
    /// inconsistent-update | new | worsened | drifting
    pub kind: String,
    /// Fails the CI gate (tier at or above `fail_on`, spec §6 order).
    pub fails: bool,
    pub group: Group,
    /// Members the diff touched (inconsistent-update findings).
    pub touched: Vec<Member>,
    /// Members the diff did NOT touch — "members 2 and 3 were not updated".
    pub untouched: Vec<Member>,
    /// (baseline divergence, current divergence) when drift is measurable.
    pub trend: Option<(f64, f64)>,
}

#[derive(Debug, Serialize)]
pub struct CheckReport {
    pub base: String,
    pub fail_on: String,
    /// Inconsistent-update findings first, then by §6 tier order.
    pub findings: Vec<CheckFinding>,
    pub failing: usize,
    /// Baseline entries in the gating (`main`) section; 0 = no baseline file.
    pub baseline_total: usize,
    /// Current findings matched to a baseline entry.
    pub baseline_matched: usize,
    /// Matched, unworsened findings exempted by the baseline.
    pub baseline_exempt: usize,
    pub touched_units: usize,
    pub stats: Stats,
}

impl CheckReport {
    pub fn failed(&self) -> bool {
        self.failing > 0
    }
}

fn member_matches(bm: &BaselineMember, file_rel: &str, name: &str, span: (u32, u32)) -> bool {
    file_rel == bm.file
        && ((!name.is_empty() && name != "<anon>" && name == bm.name)
            || spans_overlap(span, bm.line_span))
}

pub fn run(
    root: &Path,
    cfg: &Config,
    base: &str,
    fail_on_override: Option<&str>,
) -> anyhow::Result<CheckReport> {
    let diff = git_diff(root, base)?;
    let fail_on = fail_on_override.unwrap_or(&cfg.report.fail_on).to_string();
    let threshold = Tier::parse_fail_on(&fail_on)?;
    let baseline = Baseline::load_if_present(root, cfg)?;
    let report = crate::scan(root, cfg)?;

    let rel = |p: &Path| relative_file(p, root);
    let fails = |tier: Tier| -> bool {
        matches!((tier.fail_rank(), threshold), (Some(r), Some(t)) if r <= t)
    };
    // Findings and members render root-relative: check output is for review.
    let rel_member = |m: &Member| -> Member {
        let mut m = m.clone();
        m.file = PathBuf::from(rel(&m.file));
        m
    };
    let rel_group = |g: &Group| -> Group {
        let mut g = g.clone();
        for m in &mut g.members {
            *m = rel_member(m);
        }
        g
    };

    // Current plain units by relative file, for baseline-member mapping
    // (spec §6.1 tolerance: line numbers shift; fingerprints are stable).
    let mut units_by_file: HashMap<String, Vec<&UnitSummary>> = HashMap::new();
    for u in &report.unit_index {
        units_by_file.entry(rel(&u.file)).or_default().push(u);
    }
    let touched_units = report
        .unit_index
        .iter()
        .filter(|u| diff.touches(&rel(&u.file), u.line_span))
        .count();

    let empty = Vec::new();
    let entries: &Vec<BaselineEntry> = baseline.as_ref().map(|b| &b.findings).unwrap_or(&empty);
    let main_entries: Vec<&BaselineEntry> =
        entries.iter().filter(|e| e.section == "main").collect();
    let mut by_fp: HashMap<&str, Vec<&BaselineEntry>> = HashMap::new();
    for e in &main_entries {
        by_fp.entry(e.fingerprint.as_str()).or_default().push(e);
    }
    let current_by_fp: HashMap<&str, &Group> = report
        .groups
        .iter()
        .map(|g| (g.fingerprint.as_str(), g))
        .collect();

    let mut findings: Vec<CheckFinding> = Vec::new();
    // Baseline groups whose drift an inconsistent-update finding already
    // reports; the per-group pass must not double-report them.
    let mut drift_reported: HashSet<&str> = HashSet::new();

    // ---- inconsistent-update: from BASELINE state (spec §6), so a group
    // whose members drifted apart beyond max_divergence still fires ----
    if cfg.baseline.track_drift {
        for e in main_entries.iter().filter(|e| e.members.len() >= 2) {
            let mut touched: Vec<Member> = Vec::new();
            let mut untouched: Vec<Member> = Vec::new();
            for bm in &e.members {
                // Map the snapshot onto the current tree: same file, matching
                // name first, overlapping span second.
                let unit = units_by_file.get(&bm.file).and_then(|us| {
                    us.iter()
                        .find(|u| u.name == bm.name && bm.name != "<anon>" && !bm.name.is_empty())
                        .or_else(|| us.iter().find(|u| spans_overlap(u.line_span, bm.line_span)))
                });
                let (is_touched, line_span) = match unit {
                    Some(u) => (diff.touches(&bm.file, u.line_span), u.line_span),
                    // Unmapped member: touched iff its file was (deleted or
                    // rewritten past recognition — both are edits).
                    None => (diff.file_touched(&bm.file), bm.line_span),
                };
                let member = Member {
                    file: PathBuf::from(&bm.file),
                    lang: String::new(),
                    name: bm.name.clone(),
                    line_span,
                    parse_degraded: false,
                };
                if is_touched {
                    touched.push(member);
                } else {
                    untouched.push(member);
                }
            }
            if touched.is_empty() || untouched.is_empty() {
                continue; // all-or-nothing updates are consistent
            }
            let cur = current_by_fp.get(e.fingerprint.as_str());
            let trend = cur
                .map(|g| (e.divergence, g.divergence))
                .filter(|(b, c)| (c - b).abs() > DIVERGENCE_EPS);
            drift_reported.insert(e.fingerprint.as_str());
            let mut members = touched.clone();
            members.extend(untouched.iter().cloned());
            findings.push(CheckFinding {
                kind: "inconsistent-update".into(),
                fails: fails(Tier::InconsistentUpdate),
                group: Group {
                    id: String::new(),
                    tier: Tier::InconsistentUpdate,
                    fingerprint: e.fingerprint.clone(),
                    token_count: cur.map(|g| g.token_count).unwrap_or(0),
                    value: cur.map(|g| g.value).unwrap_or(0.0),
                    note: None,
                    divergence: cur.map(|g| g.divergence).unwrap_or(e.divergence),
                    template: cur.and_then(|g| g.template.clone()),
                    inline_chain: None,
                    members,
                },
                touched,
                untouched,
                trend,
            });
        }
    }

    // ---- current findings vs baseline: new / worsened / drifting ----
    let mut baseline_matched = 0usize;
    let mut baseline_exempt = 0usize;
    for g in &report.groups {
        let touched: Vec<Member> = g
            .members
            .iter()
            .filter(|m| diff.touches(&rel(&m.file), m.line_span))
            .map(&rel_member)
            .collect();
        // Baseline identity: fingerprint key + at least one member overlap
        // (guards against key collisions between disjoint groups).
        let entry = by_fp.get(g.fingerprint.as_str()).and_then(|es| {
            es.iter()
                .find(|e| {
                    e.members.iter().any(|bm| {
                        g.members
                            .iter()
                            .any(|m| member_matches(bm, &rel(&m.file), &m.name, m.line_span))
                    })
                })
                .copied()
        });
        let Some(e) = entry else {
            if !touched.is_empty() {
                findings.push(CheckFinding {
                    kind: "new".into(),
                    fails: fails(g.tier),
                    group: rel_group(g),
                    touched,
                    untouched: Vec::new(),
                    trend: None,
                });
            }
            continue;
        };
        baseline_matched += 1;
        let member_added = g.members.iter().any(|m| {
            !e.members
                .iter()
                .any(|bm| member_matches(bm, &rel(&m.file), &m.name, m.line_span))
        });
        let drifted = g.divergence > e.divergence + DIVERGENCE_EPS;
        let trend = drifted.then_some((e.divergence, g.divergence));
        if touched.is_empty() {
            // Untouched findings never fail (§6); still flag measurable drift.
            baseline_exempt += 1;
            if drifted
                && cfg.baseline.track_drift
                && !drift_reported.contains(g.fingerprint.as_str())
            {
                findings.push(CheckFinding {
                    kind: "drifting".into(),
                    fails: false,
                    group: rel_group(g),
                    touched: Vec::new(),
                    untouched: Vec::new(),
                    trend,
                });
            }
            continue;
        }
        if member_added || drifted {
            if drift_reported.contains(g.fingerprint.as_str()) && !member_added {
                continue; // the inconsistent-update finding carries this drift
            }
            findings.push(CheckFinding {
                kind: "worsened".into(),
                fails: fails(g.tier),
                group: rel_group(g),
                touched,
                untouched: Vec::new(),
                trend,
            });
        } else {
            baseline_exempt += 1;
        }
    }

    // Lead with inconsistent-update, then §6 tier order, then value.
    findings.sort_by(|a, b| {
        let key = |f: &CheckFinding| {
            (
                f.group.tier.fail_rank().unwrap_or(u32::MAX),
                std::cmp::Reverse((f.group.value * 1000.0) as u64),
                f.group.fingerprint.clone(),
            )
        };
        key(a).cmp(&key(b))
    });
    let failing = findings.iter().filter(|f| f.fails).count();

    Ok(CheckReport {
        base: base.to_string(),
        fail_on,
        findings,
        failing,
        baseline_total: main_entries.len(),
        baseline_matched,
        baseline_exempt,
        touched_units,
        stats: report.stats,
    })
}

// ---------- terminal rendering (spec §6: lead with inconsistent-update,
// baseline counts in the header) ----------

impl CheckReport {
    pub fn render_terminal(&self) -> String {
        let s = &self.stats;
        let mut out = format!(
            "reprise check vs {}: {} touched units of {} indexed; baseline: {} findings \
             ({} matched, {} exempt); {} suppressed; cache [{} hits, {} misses]; {}ms\n",
            self.base,
            self.touched_units,
            s.units_indexed,
            self.baseline_total,
            self.baseline_matched,
            self.baseline_exempt,
            s.suppressed_units,
            s.cache_hits,
            s.cache_misses,
            s.duration_ms,
        );
        if self.findings.is_empty() {
            out.push_str(&format!(
                "\nclean: no reportable findings (fail_on {})\n",
                self.fail_on
            ));
            return out;
        }
        for (i, f) in self.findings.iter().enumerate() {
            let gate = if f.fails { "FAIL" } else { "info" };
            out.push_str(&format!(
                "\n#{} [{}] {} · {}",
                i + 1,
                f.group.tier,
                gate,
                f.kind,
            ));
            if f.group.divergence > 0.0 {
                out.push_str(&format!(
                    " · similarity {:.0}%",
                    (1.0 - f.group.divergence) * 100.0
                ));
            }
            out.push('\n');
            if f.kind == "inconsistent-update" {
                out.push_str(&format!(
                    "    this change touches {} of {} members of a duplicate group; \
                     {} member(s) were NOT updated:\n",
                    f.touched.len(),
                    f.group.members.len(),
                    f.untouched.len(),
                ));
                for m in &f.touched {
                    out.push_str(&format!("    touched:   {}\n", member_line(m)));
                }
                for m in &f.untouched {
                    out.push_str(&format!("    UNTOUCHED: {}\n", member_line(m)));
                }
            } else {
                for m in &f.group.members {
                    out.push_str(&format!("    {}\n", member_line(m)));
                }
            }
            if let Some((was, now)) = f.trend {
                out.push_str(&format!(
                    "    drifting: divergence {:.3} at baseline → {:.3} now\n",
                    was, now
                ));
            }
            if let Some(template) = &f.group.template {
                crate::report::render_template_block(&mut out, template, 8);
            }
        }
        if self.failing > 0 {
            out.push_str(&format!(
                "\nFAIL: {} finding(s) at or above fail_on {}\n",
                self.failing, self.fail_on
            ));
        } else {
            out.push_str(&format!(
                "\nclean: {} informational finding(s), none at or above fail_on {}\n",
                self.findings.len(),
                self.fail_on
            ));
        }
        out
    }
}

fn member_line(m: &Member) -> String {
    format!(
        "{}:{}-{}  {}",
        m.file.display(),
        m.line_span.0,
        m.line_span.1,
        m.name
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hunk_ranges_and_deletions() {
        let diff = "\
diff --git a/src/a.rs b/src/a.rs
index 111..222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -10,2 +12,3 @@ fn ctx()
+x
+y
+z
@@ -30 +40,0 @@ fn ctx()
-gone
diff --git a/src/dead.rs b/src/dead.rs
deleted file mode 100644
--- a/src/dead.rs
+++ /dev/null
@@ -1,5 +0,0 @@
-a
";
        let map = parse_diff(diff);
        assert!(map.touches("src/a.rs", (12, 12)));
        assert!(map.touches("src/a.rs", (1, 12)));
        assert!(map.touches("src/a.rs", (14, 20)));
        assert!(!map.touches("src/a.rs", (15, 20)));
        // Pure deletion at new-line 40: adjacency touches 40-41.
        assert!(map.touches("src/a.rs", (41, 45)));
        assert!(!map.touches("src/a.rs", (42, 45)));
        assert!(map.touches("src/dead.rs", (3, 7)));
        assert!(!map.file_touched("src/other.rs"));
    }
}
