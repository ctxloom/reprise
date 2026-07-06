//! Phase-3 M3b integration contract: `reprise baseline` (spec §2), the
//! `reprise:ignore` pragma, baseline-aware `reprise check --base` with
//! `inconsistent-update` drift findings (spec §6), and the version-keyed
//! on-disk cache (spec §4, DECISIONS.md D8).

use reprise::config::Config;
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

// ---------- corpus ----------

/// Copy `n` of a ~50-token function family: same normalized structure,
/// different locals and (non-keep-list) literals — a Type-2 clone family that
/// converges on the exact-normalized tier.
fn family_member(n: usize) -> String {
    let names = ["entries", "items", "rows", "records"];
    let acc = ["total", "acc", "sum", "tally"];
    let lit = [90, 85, 70, 60];
    format!(
        r#"
pub fn process_batch_{n}({xs}: &[(String, i64)], cutoff: i64) -> Vec<String> {{
    let mut out = Vec::new();
    let mut {acc} = 0;
    for (label, score) in {xs} {{
        if *score < cutoff {{
            continue;
        }}
        {acc} += score;
        let grade = if *score >= {lit} {{ "high" }} else {{ "low" }};
        out.push(format!("{{label}}: {{score}} ({{grade}})"));
    }}
    if {acc} > {lit2} {{
        out.push(String::from("aggregate: high"));
    }}
    out
}}
"#,
        xs = names[n % 4],
        acc = acc[n % 4],
        lit = lit[n % 4],
        lit2 = lit[n % 4] * 5,
    )
}

const UNRELATED: &str = r#"
pub fn checksum_stream(data: &[u8], salt: u8) -> u8 {
    let mut state = salt;
    for chunk in data.chunks(4) {
        for byte in chunk {
            state = state.wrapping_mul(31).wrapping_add(*byte);
        }
        state ^= 0x5A;
    }
    if state == 0 {
        state = salt.wrapping_add(7);
    }
    state
}
"#;

/// A second unrelated function, distinct from `UNRELATED` and from the family.
/// It sits below the dup group in the consolidation corpus and shifts up into
/// a deleted member's old line range after the group is collapsed.
const SECOND_UNRELATED: &str = r#"
pub fn merge_windows(spans: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let mut sorted = spans.to_vec();
    sorted.sort_unstable();
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for (lo, hi) in sorted {
        match merged.last_mut() {
            Some(prev) if lo <= prev.1 => prev.1 = prev.1.max(hi),
            _ => merged.push((lo, hi)),
        }
    }
    merged
}
"#;

/// The single generic helper the family is consolidated INTO — the ideal
/// dedup. Its structure (and fingerprint) is unrelated to the family it
/// replaces, so it forms no exact group with anything.
const GENERIC_HELPER: &str = r#"
pub fn only_new_keys<K: Eq + std::hash::Hash + Clone, V: Clone>(
    current: &[(K, V)],
    seen: &std::collections::HashSet<K>,
) -> Vec<(K, V)> {
    current
        .iter()
        .filter(|(k, _)| !seen.contains(k))
        .cloned()
        .collect()
}
"#;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn git_repo_with_family() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    for n in 0..3 {
        fs::write(root.join(format!("m{n}.rs")), family_member(n)).unwrap();
    }
    fs::write(root.join("unrelated.rs"), UNRELATED).unwrap();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "t@example.com"]);
    git(root, &["config", "user.name", "t"]);
    dir
}

/// D40: the persistent baseline IS a git ref — no file is written. Each call
/// site follows this with `git add + commit`, and THAT commit is the baseline
/// point `check --base HEAD` scans for base state.
fn write_baseline(_root: &Path, _cfg: &Config) {}

// ---------- reprise:ignore pragma (spec §2) ----------

#[test]
fn ignore_pragma_on_line_above_suppresses_unit() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.rs"), family_member(0)).unwrap();
    fs::write(
        dir.path().join("b.rs"),
        format!("// reprise:ignore\n{}", family_member(1).trim_start()),
    )
    .unwrap();
    let report = reprise::scan(dir.path(), &Config::default()).unwrap();
    assert_eq!(report.stats.suppressed_units, 1);
    assert!(
        report.groups.is_empty(),
        "suppressed unit still grouped: {:#?}",
        report.groups
    );
    // Suppression count is visible in the terminal header (spec §12).
    let header = report.render_terminal(20, false);
    assert!(
        header.contains("1 suppressed"),
        "suppression count missing from header: {header}"
    );
}

#[test]
fn ignore_pragma_on_first_line_suppresses_unit_python() {
    let py_a = "def build_report(rows, cutoff):\n    lines = []\n    acc = 0\n    for tag, points in rows:\n        if points < cutoff:\n            continue\n        acc += points\n        if points >= 85:\n            band = \"stellar\"\n        else:\n            band = \"weak\"\n        lines.append(tag + \": \" + str(points) + \" (\" + band + \")\")\n    if acc > 400:\n        lines.append(\"aggregate: high\")\n    return lines\n";
    let py_b = py_a
        .replace(
            "build_report(rows, cutoff):",
            "make_summary(items, floor):  # reprise:ignore",
        )
        .replace("rows", "items")
        .replace("cutoff", "floor");
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.py"), py_a).unwrap();
    fs::write(dir.path().join("b.py"), py_b).unwrap();
    let report = reprise::scan(dir.path(), &Config::default()).unwrap();
    assert_eq!(report.stats.suppressed_units, 1);
    assert!(report.groups.is_empty(), "{:#?}", report.groups);
}

// ---------- baseline mode (spec §2) ----------

#[test]
fn baseline_carries_scheme_and_stable_fingerprints() {
    let dir = TempDir::new().unwrap();
    for n in 0..3 {
        fs::write(dir.path().join(format!("m{n}.rs")), family_member(n)).unwrap();
    }
    let cfg = Config::default();
    let report = reprise::scan(dir.path(), &cfg).unwrap();
    let baseline = reprise::baseline::create(&report, dir.path());
    assert_eq!(
        baseline.fingerprint_scheme,
        reprise::fingerprint::FINGERPRINT_SCHEME
    );
    assert!(!baseline.findings.is_empty());
    let entry = &baseline.findings[0];
    assert_eq!(entry.tier, "exact-normalized");
    assert_eq!(entry.members.len(), 3);
    // u128 hex key.
    assert_eq!(entry.fingerprint.len(), 32, "{}", entry.fingerprint);
    // Member files are root-relative (the baseline is checked in).
    assert!(entry.members.iter().all(|m| !m.file.starts_with('/')));

    // Round-trips through disk, and the fingerprint is reproducible.
    let path = dir.path().join("reprise-baseline.json");
    baseline.save(&path).unwrap();
    let loaded = reprise::baseline::Baseline::load(&path).unwrap();
    assert_eq!(loaded.findings[0].fingerprint, entry.fingerprint);
    let report2 = reprise::scan(dir.path(), &cfg).unwrap();
    let baseline2 = reprise::baseline::create(&report2, dir.path());
    assert_eq!(baseline2.findings[0].fingerprint, entry.fingerprint);
}

#[test]
fn baseline_scheme_mismatch_is_an_explicit_error() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("reprise-baseline.json");
    fs::write(&path, r#"{"fingerprint_scheme": 999, "findings": []}"#).unwrap();
    let err = reprise::baseline::Baseline::load(&path).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("rescan"),
        "scheme mismatch must demand a rescan (treated as cache miss), got: {msg}"
    );
}

// ---------- on-disk cache (spec §4, D8) ----------

#[test]
fn warm_scan_is_identical_to_cold_scan() {
    let dir = TempDir::new().unwrap();
    for n in 0..3 {
        fs::write(dir.path().join(format!("m{n}.rs")), family_member(n)).unwrap();
    }
    fs::write(dir.path().join("unrelated.rs"), UNRELATED).unwrap();
    let cfg = Config::default();
    let mut cold = reprise::scan(dir.path(), &cfg).unwrap();
    assert!(cold.stats.cache_misses > 0);
    assert_eq!(cold.stats.cache_hits, 0);
    let mut warm = reprise::scan(dir.path(), &cfg).unwrap();
    assert!(warm.stats.cache_hits > 0, "{:?}", warm.stats);
    assert_eq!(warm.stats.cache_misses, 0, "{:?}", warm.stats);
    // Identity: everything except the volatile counters (timings, hit/miss
    // tallies) must be byte-equal.
    for r in [&mut cold, &mut warm] {
        r.stats.duration_ms = 0;
        r.stats.phase_ms.clear();
        r.stats.cache_hits = 0;
        r.stats.cache_misses = 0;
    }
    assert_eq!(
        serde_json::to_string(&cold).unwrap(),
        serde_json::to_string(&warm).unwrap(),
        "cache hit changed scan results"
    );
    // The cache directory is self-gitignoring.
    assert!(dir.path().join(".reprise/cache").is_dir());
    assert_eq!(
        fs::read_to_string(dir.path().join(".reprise/.gitignore")).unwrap(),
        "*\n"
    );
}

#[test]
fn cache_respects_ignore_pragma_edits() {
    // Editing only a pragma comment must invalidate that file's cache entry.
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.rs"), family_member(0)).unwrap();
    fs::write(dir.path().join("b.rs"), family_member(1)).unwrap();
    let cfg = Config::default();
    let report = reprise::scan(dir.path(), &cfg).unwrap();
    assert_eq!(report.groups.len(), 1);
    fs::write(
        dir.path().join("b.rs"),
        format!("// reprise:ignore\n{}", family_member(1).trim_start()),
    )
    .unwrap();
    let report = reprise::scan(dir.path(), &cfg).unwrap();
    assert!(report.groups.is_empty(), "{:#?}", report.groups);
    assert_eq!(report.stats.suppressed_units, 1);
}

// ---------- check mode (spec §2 + §6) ----------

#[test]
fn check_outside_git_repo_errors_clearly() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.rs"), family_member(0)).unwrap();
    let err = reprise::check::run(dir.path(), &Config::default(), "HEAD", None).unwrap_err();
    assert!(format!("{err:#}").contains("git"), "{err:#}");
}

#[test]
fn drift_scenario_emits_inconsistent_update_naming_untouched_members() {
    let repo = git_repo_with_family();
    let root = repo.path();
    let cfg = Config::default();
    write_baseline(root, &cfg);
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "baseline"]);

    // Edit exactly one member: rename a local and tweak a literal (the
    // normalized fingerprint survives; the group is still a group).
    let edited = family_member(0)
        .replace("total", "running_sum")
        .replace("90", "77");
    fs::write(root.join("m0.rs"), edited).unwrap();

    let report = reprise::check::run(root, &cfg, "HEAD", None).unwrap();
    assert!(report.failed(), "inconsistent update must fail CI");
    let iu: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.kind == "inconsistent-update")
        .collect();
    assert_eq!(iu.len(), 1, "{:#?}", report.findings);
    let f = iu[0];
    assert!(f.fails);
    assert_eq!(f.group.tier.to_string(), "inconsistent-update");
    assert_eq!(f.touched.len(), 1);
    assert_eq!(f.untouched.len(), 2);
    let untouched_names: Vec<&str> = f.untouched.iter().map(|m| m.name.as_str()).collect();
    assert!(
        untouched_names.contains(&"process_batch_1"),
        "{untouched_names:?}"
    );
    assert!(
        untouched_names.contains(&"process_batch_2"),
        "{untouched_names:?}"
    );
    // Terminal rendering leads with the inconsistent-update finding and shows
    // base-state counts in the header (D40 wording).
    let text = report.render_terminal(false);
    assert!(text.contains("inconsistent-update"), "{text}");
    assert!(text.contains("base state"), "{text}");
    assert!(text.contains("process_batch_1"), "{text}");
}

/// Regression (false positive): consolidating a baselined exact-normalized dup
/// group into ONE generic helper — deleting the siblings — is the ideal dedup,
/// not a half-finished edit. The deleted members' names are gone, so they fall
/// to the span-overlap fallback; unless that fallback is identity-aware, each
/// deleted member mis-maps onto whatever unrelated function shifted into its
/// old line range, is scored "untouched", and fires a false inconsistent-update
/// ("touched 1 of 3, 2 not updated"). A complete, correct dedup must not gate
/// CI.
#[test]
fn consolidating_a_dup_group_into_one_helper_does_not_fire_inconsistent_update() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    // One file: the 3-member exact-normalized family, then two unrelated
    // functions that shift up into the family's old line ranges after the
    // group collapses.
    let baseline_src = format!(
        "{}{}{}{}{}",
        family_member(0),
        family_member(1),
        family_member(2),
        UNRELATED,
        SECOND_UNRELATED,
    );
    fs::write(root.join("dup.rs"), &baseline_src).unwrap();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "t@example.com"]);
    git(root, &["config", "user.name", "t"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "baseline"]);

    // The ideal dedup: replace all three siblings with one generic helper and
    // keep the unrelated functions (byte-identical, so they only shift).
    let consolidated = format!("{}{}{}", GENERIC_HELPER, UNRELATED, SECOND_UNRELATED);
    fs::write(root.join("dup.rs"), &consolidated).unwrap();

    let cfg = Config::default();
    let report = reprise::check::run(root, &cfg, "HEAD", None).unwrap();
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.kind == "inconsistent-update"),
        "full consolidation must not fire inconsistent-update: {:#?}",
        report.findings
    );
    assert!(!report.failed(), "{:#?}", report.findings);
}

#[test]
fn baselined_untouched_findings_are_exempt() {
    let repo = git_repo_with_family();
    let root = repo.path();
    let cfg = Config::default();
    write_baseline(root, &cfg);
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "baseline"]);

    // Touch only the unrelated file.
    fs::write(root.join("unrelated.rs"), UNRELATED.replace("0x5A", "0x5B")).unwrap();
    let report = reprise::check::run(root, &cfg, "HEAD", None).unwrap();
    assert!(!report.failed(), "{:#?}", report.findings);
    assert!(report.baseline_total >= 1);
}

#[test]
fn new_duplicate_involving_touched_unit_fails() {
    let repo = git_repo_with_family();
    let root = repo.path();
    let cfg = Config::default();
    write_baseline(root, &cfg);
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "baseline"]);

    // A brand-new duplicate pair (fresh structure, not in the baseline).
    let seed = UNRELATED.replace("checksum_stream", "digest_alpha");
    let copy = UNRELATED
        .replace("checksum_stream", "digest_beta")
        .replace("state", "mix")
        .replace("salt", "seed");
    fs::write(root.join("n0.rs"), seed).unwrap();
    fs::write(root.join("n1.rs"), copy).unwrap();
    git(root, &["add", "."]);

    let report = reprise::check::run(root, &cfg, "HEAD", None).unwrap();
    assert!(report.failed(), "{:#?}", report.findings);
    assert!(
        report.findings.iter().any(|f| f.kind == "new" && f.fails),
        "{:#?}",
        report.findings
    );
}

#[test]
fn member_added_to_baselined_group_is_worsened() {
    let repo = git_repo_with_family();
    let root = repo.path();
    let cfg = Config::default();
    write_baseline(root, &cfg);
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "baseline"]);

    // A fourth copy of the baselined family: baselined fingerprint, but a
    // member was added — accepting a duplicate is not accepting its growth.
    fs::write(root.join("m3.rs"), family_member(3)).unwrap();
    git(root, &["add", "."]);

    let report = reprise::check::run(root, &cfg, "HEAD", None).unwrap();
    assert!(report.failed(), "{:#?}", report.findings);
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.kind == "worsened" && f.fails),
        "{:#?}",
        report.findings
    );
}

#[test]
fn fail_on_none_disables_the_gate() {
    let repo = git_repo_with_family();
    let root = repo.path();
    let cfg = Config::default();
    write_baseline(root, &cfg);
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "baseline"]);
    let edited = family_member(0).replace("total", "running_sum");
    fs::write(root.join("m0.rs"), edited).unwrap();

    let report = reprise::check::run(root, &cfg, "HEAD", Some("none")).unwrap();
    assert!(!report.failed());
    // The finding is still reported, it just doesn't gate.
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.kind == "inconsistent-update" && !f.fails),
        "{:#?}",
        report.findings
    );
}

#[test]
fn check_without_baseline_file_synthesizes_base_state_from_two_scan() {
    let repo = git_repo_with_family();
    let root = repo.path();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "initial"]);
    let edited = family_member(0).replace("total", "running_sum");
    fs::write(root.join("m0.rs"), edited).unwrap();

    let report = reprise::check::run(root, &Config::default(), "HEAD", None).unwrap();
    // Two-scan mode: base state comes from scanning the base ref, so the
    // pre-existing group IS the base state (not "everything is new") and the
    // one-member edit fails as an inconsistent update, not a new finding.
    assert!(report.base_state.starts_with("base-scan"));
    assert!(
        report.baseline_total > 0,
        "base scan must supply base state"
    );
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.kind == "inconsistent-update"),
        "{:#?}",
        report.findings
    );
    assert!(
        report.failed(),
        "touched member of a base-state group must gate"
    );
}

// ---------- CLI exit codes (spec §2) ----------

#[test]
fn cli_check_exit_codes() {
    let repo = git_repo_with_family();
    let root = repo.path();
    let bin = env!("CARGO_BIN_EXE_reprise");

    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "baseline"]);

    // Clean tree: exit 0.
    let out = Command::new(bin)
        .args(["check"])
        .arg(root)
        .args(["--base", "HEAD"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    // Drifted member: nonzero, and the report names the tier.
    let edited = family_member(0).replace("total", "running_sum");
    fs::write(root.join("m0.rs"), edited).unwrap();
    let out = Command::new(bin)
        .args(["check"])
        .arg(root)
        .args(["--base", "HEAD"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("inconsistent-update"), "{stdout}");

    // --fail-on none: exit 0 again.
    let out = Command::new(bin)
        .args(["check"])
        .arg(root)
        .args(["--base", "HEAD", "--fail-on", "none"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

// ---- D37: user-feedback fixes (micro-regions in check, thin wrappers, hint) ----

/// Two functions sharing a ~35-token idiom run (micro: below
/// report.micro_region_tokens) plus one pair sharing a substantial run.
#[test]
fn check_emits_only_substantial_regions() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    // Micro shared run (small env-copy idiom) inside otherwise-unrelated fns.
    let micro =
        "    out = {}\n    for key, value in sorted(env.items()):\n        out[key] = str(value)\n";
    std::fs::write(
        root.join("a.py"),
        format!("def run_cli(env, args):\n{micro}    launch(args, out)\n    monitor(out)\n    return collect_logs(out, args)\n"),
    )
    .unwrap();
    std::fs::write(
        root.join("b.py"),
        format!("def clone_server(env, spec):\n{micro}    validated = check_spec(spec, out)\n    persist(validated)\n    return handle(validated, spec)\n"),
    )
    .unwrap();
    git(root, &["init", "-q"]);
    git(root, &["add", "-A"]);
    git(
        root,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "init",
        ],
    );
    std::fs::write(root.join("c.py"), "def unrelated():\n    return 1\n").unwrap();
    let mut a = std::fs::read_to_string(root.join("a.py")).unwrap();
    a.push_str("\n# touch\n");
    std::fs::write(root.join("a.py"), a).unwrap();
    let report = reprise::check::run(root, &reprise::Config::default(), "HEAD", None).unwrap();
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.group.tier == reprise::report::Tier::ExactRegion
                && f.group.token_count < reprise::Config::default().report.micro_region_tokens),
        "micro-regions must not surface in check: {:#?}",
        report
            .findings
            .iter()
            .map(|f| (f.group.tier, f.group.token_count))
            .collect::<Vec<_>>()
    );
}

/// Thin delegation wrappers (single-statement bodies calling a shared helper)
/// must not re-match through inlining — that is the recommended fix pattern.
#[test]
fn thin_delegation_wrappers_do_not_match_via_inlining() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("m.py"),
        "def render_shared(items, kind, limit):\n    rows = []\n    for item in items:\n        if item.weight > limit:\n            rows.append(format_row(item, kind))\n        else:\n            rows.append(placeholder(kind))\n        audit(item, kind)\n    return assemble(rows, kind)\n\n\ndef placeholder_fragments(items, limit):\n    return render_shared(items, \"fragments\", limit)\n\n\ndef placeholder_prompts(items, limit):\n    return render_shared(items, \"prompts\", limit)\n\n\ndef placeholder_mcp(items, limit):\n    return render_shared(items, \"mcp\", limit)\n",
    )
    .unwrap();
    let report = reprise::scan(root, &reprise::Config::default()).unwrap();
    assert!(
        !report
            .groups
            .iter()
            .any(|g| g.tier == reprise::report::Tier::InlineAssisted),
        "delegation wrappers re-matched via inlining: {:#?}",
        report.groups
    );
}

#[test]
fn check_without_baseline_prints_adoption_hint() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    std::fs::write(root.join("a.py"), "def f(x):\n    return x\n").unwrap();
    git(root, &["init", "-q"]);
    git(root, &["add", "-A"]);
    git(
        root,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "init",
        ],
    );
    let mut a = std::fs::read_to_string(root.join("a.py")).unwrap();
    a.push_str("\n# touch\n");
    std::fs::write(root.join("a.py"), a).unwrap();
    let report = reprise::check::run(root, &reprise::Config::default(), "HEAD", None).unwrap();
    assert!(
        report.base_state.starts_with("base-scan"),
        "{}",
        report.base_state
    );
}

/// Two-scan mode end to end (no baseline file anywhere): a 3-member family is
/// committed, one member is edited — inconsistent-update must fire from the
/// synthesized base state, and pre-existing duplication must be exempt.
#[test]
fn two_scan_mode_fires_inconsistent_update_without_any_baseline_file() {
    let dir = git_repo_with_family();
    let root = dir.path();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "init"]);
    let cfg = Config::default();
    // Pre-existing duplication, untouched: exempt, exit clean.
    let clean = reprise::check::run(root, &cfg, "HEAD", None).unwrap();
    assert!(
        !clean.failed(),
        "untouched pre-existing duplication must be exempt in two-scan mode: {:#?}",
        clean.findings
    );
    assert!(clean.base_state.starts_with("base-scan"));
    // Edit exactly one member (m0 uses lit2 = 90*5 = 450).
    let f0 = root.join("m0.rs");
    let mut src = fs::read_to_string(&f0).unwrap();
    src = src.replace("> 450", "> 999");
    assert!(src.contains("> 999"), "edit did not apply");
    fs::write(&f0, src).unwrap();
    let report = reprise::check::run(root, &cfg, "HEAD", None).unwrap();
    let iu = report
        .findings
        .iter()
        .find(|f| f.kind == "inconsistent-update")
        .unwrap_or_else(|| {
            panic!(
                "no inconsistent-update in two-scan mode: {:#?}",
                report.findings
            )
        });
    assert_eq!(iu.touched.len(), 1);
    assert_eq!(iu.untouched.len(), 2);
    assert!(report.failed());
    // Transient base-state cache exists under .reprise (never in VCS).
    assert!(root.join(".reprise/base-state").is_dir());
    // Second run reuses it.
    let again = reprise::check::run(root, &cfg, "HEAD", None).unwrap();
    assert_eq!(again.base_state, "base-scan (cached)");
}

/// D39: an edit inside a large function must NOT fire inconsistent-update for
/// region groups whose runs the edit never touched (unit-granularity mapping
/// made every edit in a big function "touch" all its regions); an edit inside
/// the shared run itself still fires.
#[test]
fn region_drift_is_span_precise() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let shared = "    buf = prepare(items)\n    buf.sort()\n    emit_header(buf, \"v2\")\n    for entry in buf:\n        validate(entry, STRICT)\n        write_row(entry, buf)\n        bump_metric(\"rows\", entry)\n    flush_all(buf)\n";
    // Big function: preamble, the shared run, then a long tail.
    let tail: String = (0..8)
        .map(|i| format!("    step_{i} = compute_{i}(items, buf)\n    audit(step_{i}, {i})\n"))
        .collect();
    fs::write(
        root.join("big.py"),
        format!("def export_daily(items, path):\n    log_start(\"daily\", path)\n{shared}{tail}    return len(items)\n"),
    )
    .unwrap();
    fs::write(
        root.join("other.py"),
        format!("def export_weekly(items, dest, limit):\n    if limit < 1:\n        return None\n    rotate_old(dest, limit)\n{shared}    notify(dest)\n    return dest\n"),
    )
    .unwrap();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "t@example.com"]);
    git(root, &["config", "user.name", "t"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "init"]);
    let cfg = Config::default();

    // Edit the TAIL of the big function — far from the shared run.
    let mut src = fs::read_to_string(root.join("big.py")).unwrap();
    src = src.replace("audit(step_7, 7)", "audit_v2(step_7, 7, path)");
    fs::write(root.join("big.py"), src).unwrap();
    let report = reprise::check::run(root, &cfg, "HEAD", None).unwrap();
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.kind == "inconsistent-update"),
        "tail edit must not fire region IU: {:#?}",
        report.findings
    );

    // Now edit INSIDE the shared run in one copy only.
    let mut src = fs::read_to_string(root.join("big.py")).unwrap();
    src = src.replace("validate(entry, STRICT)", "validate(entry, LENIENT)");
    fs::write(root.join("big.py"), src).unwrap();
    let report = reprise::check::run(root, &cfg, "HEAD", None).unwrap();
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.kind == "inconsistent-update"),
        "in-run edit must fire region IU: {:#?}",
        report.findings
    );
}

/// D41: `reprise:accept-drift` on the touched member demotes IU to info —
/// the unit stays covered by every other tier, unlike `reprise:ignore`.
#[test]
fn accept_drift_pragma_demotes_iu_to_info() {
    let repo = git_repo_with_family();
    let root = repo.path();
    // Mark m0 as accepted-drift BEFORE the baseline commit.
    // family_member starts with a newline; the pragma must sit directly
    // above the fn line (blank lines end the upward scan by design).
    let marked = family_member(0).replacen('\n', "\n// reprise:accept-drift\n", 1);
    fs::write(root.join("m0.rs"), &marked).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "baseline"]);

    // One-sided edit to the accepted unit.
    fs::write(
        root.join("m0.rs"),
        marked.replace("total", "running_sum").replace("90", "77"),
    )
    .unwrap();
    let report = reprise::check::run(root, &Config::default(), "HEAD", None).unwrap();
    let iu: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.kind == "inconsistent-update")
        .collect();
    assert_eq!(iu.len(), 1, "IU still REPORTS: {:#?}", report.findings);
    assert!(!iu[0].fails, "accept-drift demotes IU to info");
    assert!(!report.failed());

    // Control: the same edit on an unmarked sibling still gates.
    fs::write(root.join("m0.rs"), &marked).unwrap(); // revert
    let m1 = family_member(1).replace("acc", "running_sum");
    fs::write(root.join("m1.rs"), m1).unwrap();
    let report = reprise::check::run(root, &Config::default(), "HEAD", None).unwrap();
    assert!(
        report.failed(),
        "unmarked member must still gate: {:#?}",
        report.findings
    );
}
