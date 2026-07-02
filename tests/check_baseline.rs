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

fn write_baseline(root: &Path, cfg: &Config) {
    let report = reprise::scan(root, cfg).unwrap();
    let baseline = reprise::baseline::create(&report, root);
    baseline
        .save(&reprise::baseline::baseline_path(root, cfg))
        .unwrap();
}

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
        msg.contains("re-baseline"),
        "scheme mismatch must demand re-baseline, got: {msg}"
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
    // baseline counts in the header.
    let text = report.render_terminal();
    assert!(text.contains("inconsistent-update"), "{text}");
    assert!(text.contains("baseline"), "{text}");
    assert!(text.contains("process_batch_1"), "{text}");
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
fn check_without_baseline_treats_touched_findings_as_new() {
    let repo = git_repo_with_family();
    let root = repo.path();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "initial"]);
    let edited = family_member(0).replace("total", "running_sum");
    fs::write(root.join("m0.rs"), edited).unwrap();

    let report = reprise::check::run(root, &Config::default(), "HEAD", None).unwrap();
    assert_eq!(report.baseline_total, 0);
    assert!(report.failed(), "un-baselined touched duplicate must fail");
}

// ---------- CLI exit codes (spec §2) ----------

#[test]
fn cli_check_exit_codes() {
    let repo = git_repo_with_family();
    let root = repo.path();
    let bin = env!("CARGO_BIN_EXE_reprise");

    let out = Command::new(bin)
        .args(["baseline"])
        .arg(root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(root.join("reprise-baseline.json").is_file());

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
