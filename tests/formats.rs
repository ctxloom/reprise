//! M3d output-format conformance (spec §6.1): SARIF 2.1.0, PMD/CPD XML, jscpd
//! JSON. Structural (not golden-file) assertions over a small fixed corpus —
//! golden files would churn on every ranking/token tweak and add no coverage
//! the structural checks miss (recorded in DECISIONS.md D28). Well-formedness
//! is checked by parsing SARIF/jscpd as JSON (serde_json, already a dep) and
//! hand-checking CPD's tag/attribute structure (no quick-xml dev-dep added).

use reprise::config::Config;
use reprise::formats::{cpd, jscpd, sarif};
use serde_json::Value;
use std::fs;
use tempfile::TempDir;

/// Three Type-2 variants of one ~12-line function: renamed identifiers + bucketed
/// literals converge to ONE exact-normalized group with THREE members — enough to
/// exercise the multi-location SARIF mapping (2 relatedLocations) and jscpd's
/// pairwise expansion (2 duplicate entries for the group).
const FN_A: &str = r#"
pub fn summarize_scores(entries: &[(String, i64)], threshold: i64) -> Vec<String> {
    let mut summaries = Vec::new();
    let mut running_total = 0;
    for (label, score) in entries {
        if *score < threshold { continue; }
        running_total += score;
        let grade = if *score >= 90 { "excellent" } else { "poor" };
        summaries.push(format!("{label}: {score} ({grade})"));
    }
    if running_total > 250 { summaries.push(String::from("aggregate: high")); }
    summaries
}
"#;

const FN_B: &str = r#"
pub fn collect_report(items: &[(String, i64)], cutoff: i64) -> Vec<String> {
    let mut lines = Vec::new();
    let mut acc = 0;
    for (tag, points) in items {
        if *points < cutoff { continue; }
        acc += points;
        let band = if *points >= 85 { "stellar" } else { "weak" };
        lines.push(format!("{tag}: {points} ({band})"));
    }
    if acc > 400 { lines.push(String::from("aggregate: high")); }
    lines
}
"#;

const FN_C: &str = r#"
pub fn build_digest(records: &[(String, i64)], floor: i64) -> Vec<String> {
    let mut digest = Vec::new();
    let mut tally = 0;
    for (key, value) in records {
        if *value < floor { continue; }
        tally += value;
        let rank = if *value >= 70 { "top" } else { "low" };
        digest.push(format!("{key}: {value} ({rank})"));
    }
    if tally > 300 { digest.push(String::from("aggregate: high")); }
    digest
}
"#;

fn corpus() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("other")).unwrap();
    fs::write(root.join("src/a.rs"), FN_A).unwrap();
    fs::write(root.join("other/b.rs"), FN_B).unwrap();
    fs::write(root.join("src/c.rs"), FN_C).unwrap();
    dir
}

fn scan(dir: &TempDir) -> reprise::ScanReport {
    reprise::scan(dir.path(), &Config::default()).unwrap()
}

// ---------- SARIF ----------

#[test]
fn sarif_is_valid_json_with_required_skeleton() {
    let dir = corpus();
    let report = scan(&dir);
    let out = sarif::scan_sarif(&report, dir.path(), &Config::default(), false);
    let v: Value = serde_json::from_str(&out).expect("SARIF must parse as JSON");

    assert_eq!(v["version"], "2.1.0");
    assert!(v["$schema"].is_string(), "missing $schema");
    let driver = &v["runs"][0]["tool"]["driver"];
    assert_eq!(driver["name"], "reprise");
    assert_eq!(driver["version"], env!("CARGO_PKG_VERSION"));
    let rules = driver["rules"].as_array().expect("rules[]");
    assert!(!rules.is_empty(), "at least one rule per tier");
    assert!(
        rules
            .iter()
            .all(|r| r["shortDescription"]["text"].is_string())
    );
    // Every result references a defined rule and carries a region.
    let results = v["runs"][0]["results"].as_array().expect("results[]");
    assert!(!results.is_empty(), "the 3-member group yields ≥1 result");
    for r in results {
        assert!(r["ruleId"].is_string());
        let region = &r["locations"][0]["physicalLocation"]["region"];
        assert!(region["startLine"].as_u64().unwrap() >= 1);
        assert!(region["endLine"].as_u64().unwrap() >= region["startLine"].as_u64().unwrap());
        assert!(
            r["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
                .as_str()
                .unwrap()
                .starts_with(|c: char| c != '/'),
            "artifact uri must be relative"
        );
    }
}

#[test]
fn sarif_group_is_one_multilocation_result_with_structural_fingerprint() {
    let dir = corpus();
    let report = scan(&dir);
    let out = sarif::scan_sarif(&report, dir.path(), &Config::default(), false);
    let v: Value = serde_json::from_str(&out).unwrap();
    let results = v["runs"][0]["results"].as_array().unwrap();

    // §6.1 decision 1: N members → ONE result. The 3-member group is a single
    // result with 2 relatedLocations, NOT three results.
    let group = results
        .iter()
        .max_by_key(|r| r["relatedLocations"].as_array().map_or(0, Vec::len))
        .unwrap();
    assert_eq!(
        group["relatedLocations"].as_array().unwrap().len(),
        2,
        "3-member group ⇒ 1 primary + 2 relatedLocations"
    );
    // Related-location ids are integers and the message links reference them.
    let msg = group["message"]["text"].as_str().unwrap();
    for loc in group["relatedLocations"].as_array().unwrap() {
        let id = loc["id"].as_i64().expect("integer related-location id");
        assert!(
            msg.contains(&format!("]({id})")),
            "message links [member](id)"
        );
    }

    // §6.1 decision 2: partialFingerprints is the structural hash, not line-based.
    let fp = &group["partialFingerprints"]["reprise/structuralFingerprint/v1"];
    assert!(fp.is_string(), "structural partialFingerprint present");
    assert_eq!(
        fp.as_str().unwrap(),
        report.groups[0].fingerprint,
        "partialFingerprint == Group.fingerprint (stable across cosmetic drift)"
    );

    // §6.1 decision 3: level is the coarse gate map; the tier lives in properties.
    assert_eq!(
        group["level"], "error",
        "exact-normalized ≥ default fail_on"
    );
    assert_eq!(group["properties"]["tier"], "exact-normalized");
}

#[test]
fn sarif_line_mode_omits_our_fingerprint() {
    let dir = corpus();
    let report = scan(&dir);
    let mut cfg = Config::default();
    cfg.report.sarif_fingerprint = "line".into();
    let out = sarif::scan_sarif(&report, dir.path(), &cfg, false);
    let v: Value = serde_json::from_str(&out).unwrap();
    for r in v["runs"][0]["results"].as_array().unwrap() {
        assert!(
            r.get("partialFingerprints").is_none(),
            "line mode falls back to GitHub primaryLocationLineHash — omit ours"
        );
    }
}

// ---------- CPD XML ----------

#[test]
fn cpd_xml_is_well_formed_and_complete() {
    let dir = corpus();
    let report = scan(&dir);
    let out = cpd::scan_cpd(&report, dir.path(), false);

    assert!(out.starts_with("<?xml version=\"1.0\""));
    assert!(out.contains("<pmd-cpd>") && out.trim_end().ends_with("</pmd-cpd>"));

    let dup_open = out.matches("<duplication ").count();
    let dup_close = out.matches("</duplication>").count();
    assert_eq!(dup_open, dup_close, "balanced <duplication> tags");
    assert!(dup_open >= 1, "the 3-member group emits a duplication");

    // Each duplication carries lines= and tokens= and one <file> per member.
    assert!(out.contains("lines=\"") && out.contains("tokens=\""));
    let file_tags = out.matches("<file ").count();
    assert!(
        file_tags >= 3,
        "3-member group ⇒ ≥3 <file> entries, got {file_tags}"
    );

    // CDATA fragment is present and balanced.
    assert_eq!(
        out.matches("<![CDATA[").count(),
        out.matches("]]></codefragment>").count(),
        "balanced CDATA"
    );
    assert!(out.contains("pub fn"), "codefragment holds real source");
}

// ---------- jscpd JSON ----------

#[test]
fn jscpd_json_has_statistics_and_pairwise_duplicates() {
    let dir = corpus();
    let report = scan(&dir);
    let out = jscpd::scan_jscpd(&report, dir.path(), false);
    let v: Value = serde_json::from_str(&out).expect("jscpd must parse as JSON");

    let total = &v["statistics"]["total"];
    for key in [
        "lines",
        "sources",
        "clones",
        "duplicatedLines",
        "percentage",
    ] {
        assert!(!total[key].is_null(), "statistics.total.{key} present");
    }
    assert_eq!(total["sources"].as_u64().unwrap(), 3, "3 scanned files");

    let dups = v["duplicates"].as_array().expect("duplicates[]");
    // 3-member group ⇒ 2 pairwise entries (first vs each other, D28).
    assert_eq!(dups.len(), 2, "pairwise expansion of the 3-member group");
    assert_eq!(total["clones"].as_u64().unwrap(), dups.len() as u64);
    for d in dups {
        assert_eq!(d["format"], "rust");
        assert!(d["lines"].as_u64().unwrap() >= 1);
        assert!(d["tokens"].as_u64().unwrap() >= 1);
        for side in ["firstFile", "secondFile"] {
            assert!(d[side]["name"].is_string());
            assert!(d[side]["start"].as_u64().unwrap() >= 1);
            assert!(d[side]["end"].as_u64().unwrap() >= d[side]["start"].as_u64().unwrap());
            assert!(d[side]["startLoc"]["line"].as_u64().unwrap() >= 1);
            assert!(d[side]["endLoc"]["line"].as_u64().unwrap() >= 1);
        }
        assert!(d["fragment"].as_str().unwrap().contains("pub fn"));
        // firstFile is the shared anchor across the group's pairwise entries.
        assert_eq!(d["firstFile"]["name"], dups[0]["firstFile"]["name"]);
    }
}

#[test]
fn duplication_ratios_are_populated() {
    let dir = corpus();
    let report = scan(&dir);
    let s = &report.stats;
    assert!(s.total_lines > 0 && s.total_tokens > 0);
    assert!(
        s.duplicated_lines > 0,
        "the corpus is almost all duplication"
    );
    assert!(s.duplicated_lines_pct > 0.0 && s.duplicated_lines_pct <= 100.0);
    assert!(s.duplicated_tokens_pct > 0.0 && s.duplicated_tokens_pct <= 100.0);
    assert!(s.clones_per_kloc > 0.0);
}
