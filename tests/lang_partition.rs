//! M3c language partitioning (spec §3, §5.5.5): matching is same-language only.
//! A TypeScript clone pair and a Go clone pair in one corpus must produce two
//! separate groups and never mix languages — even though the two languages'
//! normalized trees look superficially similar.

use reprise::config::Config;
use std::fs;
use tempfile::TempDir;

const TS_A: &str = "function summarize(scores: number[], threshold: number): number {\n  let total = 0;\n  let count = 0;\n  for (const s of scores) {\n    if (s < threshold) {\n      continue;\n    }\n    total += s;\n    count += 1;\n  }\n  if (count > 5) {\n    total += 100;\n  }\n  return total;\n}\n";
const TS_B: &str = "function collect(points: number[], cutoff: number): number {\n  let acc = 0;\n  let n = 0;\n  for (const p of points) {\n    if (p < cutoff) {\n      continue;\n    }\n    acc += p;\n    n += 1;\n  }\n  if (n > 5) {\n    acc += 100;\n  }\n  return acc;\n}\n";

const GO_A: &str = "package main\nfunc summarize(scores []int, threshold int) int {\n\ttotal := 0\n\tcount := 0\n\tfor _, s := range scores {\n\t\tif s < threshold {\n\t\t\tcontinue\n\t\t}\n\t\ttotal += s\n\t\tcount += 1\n\t}\n\tif count > 5 {\n\t\ttotal += 100\n\t}\n\treturn total\n}\n";
const GO_B: &str = "package main\nfunc collect(points []int, cutoff int) int {\n\tacc := 0\n\tn := 0\n\tfor _, p := range points {\n\t\tif p < cutoff {\n\t\t\tcontinue\n\t\t}\n\t\tacc += p\n\t\tn += 1\n\t}\n\tif n > 5 {\n\t\tacc += 100\n\t}\n\treturn acc\n}\n";

#[test]
fn ts_and_go_clone_pairs_stay_partitioned() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.ts"), TS_A).unwrap();
    fs::write(dir.path().join("b.ts"), TS_B).unwrap();
    fs::write(dir.path().join("a.go"), GO_A).unwrap();
    fs::write(dir.path().join("b.go"), GO_B).unwrap();

    let report = reprise::scan(dir.path(), &Config::default()).unwrap();

    // Exactly two groups: one all-TS, one all-Go.
    let pair_groups: Vec<_> = report
        .groups
        .iter()
        .filter(|g| g.members.len() == 2)
        .collect();
    assert_eq!(
        pair_groups.len(),
        2,
        "expected one TS + one Go group, got: {:#?}",
        report.groups
    );

    let mut langs_seen = Vec::new();
    for g in &pair_groups {
        let langs: Vec<_> = g.members.iter().map(|m| m.lang.clone()).collect();
        assert!(
            langs.iter().all(|l| l == &langs[0]),
            "group mixes languages: {langs:?}"
        );
        langs_seen.push(langs[0].clone());
    }
    langs_seen.sort();
    assert_eq!(langs_seen, vec!["go".to_string(), "typescript".to_string()]);
}
