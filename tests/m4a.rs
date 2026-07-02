//! Phase-4 M4a contract tests: api-profile retune (D29), internal-repeat
//! dispatch-table suppression (D30), pragma-over-attributes (D21 refined),
//! generated first-line signatures, fail_on config validation, --top 0.

use reprise::config::Config;
use reprise::report::ScanReport;
use std::fs;
use tempfile::TempDir;

fn scan_snippets(ext: &str, sources: &[&str]) -> ScanReport {
    let dir = TempDir::new().unwrap();
    for (i, src) in sources.iter().enumerate() {
        fs::write(dir.path().join(format!("m{i}.{ext}")), src).unwrap();
    }
    reprise::scan(dir.path(), &Config::default()).unwrap()
}

// ---------- api-profile retune (spec §7.4d, D29) ----------

#[test]
fn api_profile_default_thresholds_are_the_d29_calibration() {
    let cfg = Config::default();
    assert_eq!(
        cfg.api_profile.api_profile_sim, 0.5,
        "D29: sweep + 20-finding hand-label set sim=0.5"
    );
    assert_eq!(
        cfg.api_profile.api_min_distinct_rare, 3,
        "D29: the rare>=3 signature floor is the precision knob"
    );
}

// ---------- internal-repeat: dispatch tables are not findings (D30) ----------

/// A match dispatch table (enum/int -> literal payload) is idiomatic, not
/// duplication a reviewer can act on; the M4a stratified sample labeled all
/// three sampled match-arm internal-repeats FP. The fold still happens (it is
/// load-bearing canonicalization); only the report-level finding is dropped.
#[test]
fn internal_repeat_not_reported_for_match_arm_tables() {
    let src = r#"
fn label_of(kind: u32, out: &mut String) -> usize {
    match kind {
        1 => out.push_str("alpha section"),
        2 => out.push_str("beta section"),
        3 => out.push_str("gamma section"),
        4 => out.push_str("delta section"),
        5 => out.push_str("epsilon section"),
        _ => out.push_str("unknown section"),
    }
    out.len()
}
"#;
    let report = scan_snippets("rs", &[src]);
    assert!(
        !report
            .groups
            .iter()
            .any(|g| g.tier.to_string() == "internal-repeat"),
        "match-arm table reported as internal-repeat: {:#?}",
        report.groups
    );
}

/// Statement-run repeats (the S44 shape: detect-feature/push pairs) remain
/// reportable — the suppression is scoped to dispatch arms only.
#[test]
fn internal_repeat_still_reported_for_statement_runs() {
    let src = r#"
fn gather_features(probe: &Probe) -> Vec<String> {
    let mut features = Vec::new();
    let sse2 = probe.detect("sse2");
    features.push(format!("{}SSE2", sign(sse2)));
    let ssse3 = probe.detect("ssse3");
    features.push(format!("{}SSSE3", sign(ssse3)));
    let avx2 = probe.detect("avx2");
    features.push(format!("{}AVX2", sign(avx2)));
    features
}
"#;
    let report = scan_snippets("rs", &[src]);
    assert!(
        report
            .groups
            .iter()
            .any(|g| g.tier.to_string() == "internal-repeat"),
        "statement-run internal-repeat lost: {:#?}",
        report.groups
    );
}

/// Python case_clause tables get the same treatment as Rust match arms.
#[test]
fn internal_repeat_not_reported_for_python_case_tables() {
    let src = "def label_of(kind, out):\n    match kind:\n        case 1:\n            out.append(\"alpha section\")\n        case 2:\n            out.append(\"beta section\")\n        case 3:\n            out.append(\"gamma section\")\n        case 4:\n            out.append(\"delta section\")\n        case 5:\n            out.append(\"epsilon section\")\n    return len(out)\n";
    let report = scan_snippets("py", &[src]);
    assert!(
        !report
            .groups
            .iter()
            .any(|g| g.tier.to_string() == "internal-repeat"),
        "case-clause table reported as internal-repeat: {:#?}",
        report.groups
    );
}

// ---------- reprise:ignore over attributes/decorators (D21 refined) ----------

const RS_DUP: &str = r#"
pub fn summarize_scores(entries: &[(String, i64)], threshold: i64) -> Vec<String> {
    let mut summaries = Vec::new();
    let mut running_total = 0;
    for (label, score) in entries {
        if *score < threshold {
            continue;
        }
        running_total += score;
        let grade = if *score >= 90 { "excellent" } else { "poor" };
        summaries.push(format!("{label}: {score} ({grade})"));
    }
    if running_total > 250 {
        summaries.push(String::from("aggregate: high"));
    }
    summaries
}
"#;

#[test]
fn pragma_above_rust_attributes_suppresses_unit() {
    let with_attr = format!(
        "// reprise:ignore\n#[inline]\n#[allow(dead_code)]\n{}",
        RS_DUP
            .trim_start()
            .replacen("summarize_scores", "collect_report", 1)
    );
    let report = scan_snippets("rs", &[RS_DUP, &with_attr]);
    assert_eq!(
        report.stats.suppressed_units, 1,
        "pragma above attributes not honored"
    );
    assert!(
        report.groups.is_empty(),
        "suppressed unit still grouped: {:#?}",
        report.groups
    );
}

#[test]
fn pragma_above_python_decorator_suppresses_unit() {
    let py_a = "def compute_totals(rows, cutoff):\n    lines = []\n    acc = 0\n    for tag, points in rows:\n        if points < cutoff:\n            continue\n        acc += points\n        if points >= 85:\n            band = \"stellar\"\n        else:\n            band = \"weak\"\n        lines.append(tag + str(points) + band)\n    if acc > 400:\n        lines.append(\"aggregate: high\")\n    return lines\n";
    let py_b = format!(
        "# reprise:ignore\n@registry.expose(\"totals\")\n{}",
        py_a.replacen("compute_totals", "compute_report", 1)
    );
    let report = scan_snippets("py", &[py_a, &py_b]);
    assert_eq!(
        report.stats.suppressed_units, 1,
        "pragma above decorator not honored"
    );
    assert!(
        report.groups.is_empty(),
        "suppressed unit still grouped: {:#?}",
        report.groups
    );
}

// ---------- generated-code first-line signatures (spec §5.1, M4a) ----------

#[test]
fn minified_first_line_skips_file() {
    let dir = TempDir::new().unwrap();
    // A "minified bundle": one enormous first line, no marker comment.
    let minified = format!("var x=function(){{return {}}};", "1+".repeat(400) + "1");
    assert!(minified.lines().next().unwrap().len() >= 512);
    fs::write(dir.path().join("bundle.min.ts"), &minified).unwrap();
    fs::write(dir.path().join("real.rs"), RS_DUP).unwrap();
    let report = reprise::scan(dir.path(), &Config::default()).unwrap();
    assert_eq!(
        report.stats.files_scanned, 1,
        "minified file not skipped (files_scanned)"
    );
}

#[test]
fn go_style_generated_banner_skips_file() {
    let dir = TempDir::new().unwrap();
    let generated = format!("// Code generated by stringer; edit upstream.\n{RS_DUP}");
    fs::write(dir.path().join("gen.rs"), &generated).unwrap();
    fs::write(dir.path().join("real.rs"), RS_DUP).unwrap();
    let report = reprise::scan(dir.path(), &Config::default()).unwrap();
    assert_eq!(
        report.stats.files_scanned, 1,
        "generated banner not honored"
    );
    assert!(report.groups.is_empty(), "generated file still grouped");
}

// ---------- fail_on validated at config load ----------

#[test]
fn invalid_fail_on_rejected_at_config_load() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("reprise.toml"),
        "[report]\nfail_on = \"sev-high\"\n",
    )
    .unwrap();
    let err = Config::load(dir.path()).unwrap_err().to_string();
    assert!(err.contains("fail_on"), "error names the key: {err}");
    assert!(
        err.contains("exact-normalized") && err.contains("none"),
        "error lists valid values: {err}"
    );
}

#[test]
fn valid_fail_on_accepted_at_config_load() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("reprise.toml"),
        "[report]\nfail_on = \"near-normalized\"\n",
    )
    .unwrap();
    assert!(Config::load(dir.path()).is_ok());
}

// ---------- --top 0 shows all groups ----------

#[test]
fn top_zero_renders_all_groups() {
    // Three structurally DISTINCT functions (different trailing shapes so
    // they do not co-group), each duplicated across the two files.
    let tails = [
        "",
        "    if threshold > 5 {\n        summaries.pop();\n    }\n",
        "    while running_total > 900 {\n        running_total -= 7;\n    }\n",
    ];
    let mut variants = Vec::new();
    for (i, tail) in tails.iter().enumerate() {
        variants.push(
            RS_DUP
                .replacen("summarize_scores", &format!("summarize_scores_v{i}"), 1)
                .replace("    summaries\n}", &format!("{tail}    summaries\n}}")),
        );
    }
    let a = variants.join("\n");
    let b = a
        .replace("summaries", "lines")
        .replace("running_total", "acc")
        .replace("summarize_scores_v", "collect_report_v");
    let report = scan_snippets("rs", &[&a, &b]);
    assert!(
        report.groups.len() >= 2,
        "need multiple groups for the test"
    );
    let rendered = report.render_terminal(0, false);
    assert!(
        !rendered.contains("more groups"),
        "--top 0 must not truncate: {rendered}"
    );
    assert!(
        rendered.contains(&format!("\n#{} [", report.groups.len())),
        "last group rank missing from --top 0 output"
    );
    // And the truncated default still truncates (sanity of the fixture).
    assert!(report.render_terminal(1, false).contains("more groups"));
}
