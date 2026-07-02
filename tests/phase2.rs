//! Phase-2 integration contract (spec §5.2 desugaring, §5.3 folding, §5.6
//! sequence + near-miss tiers, §5.1 test policy), all at `scan` level.

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

/// Strongest tier of any group joining files 0 and 1, if any.
fn pair_tier(report: &ScanReport) -> Option<String> {
    report
        .groups
        .iter()
        .filter(|g| {
            let fs: Vec<_> = g
                .members
                .iter()
                .map(|m| m.file.to_string_lossy().to_string())
                .collect();
            fs.iter().any(|f| f.contains("m0.")) && fs.iter().any(|f| f.contains("m1."))
        })
        .map(|g| g.tier.to_string())
        .next()
}

fn assert_pair(report: &ScanReport, expect: &[&str]) {
    let tier = pair_tier(report);
    assert!(
        tier.as_deref().is_some_and(|t| expect.contains(&t)),
        "expected pair tier in {expect:?}, got {tier:?}; groups: {:#?}",
        report.groups
    );
}

fn assert_no_pair(report: &ScanReport) {
    assert!(
        pair_tier(report).is_none(),
        "expected NO pair group, got: {:#?}",
        report.groups
    );
}

// ---------- iteration-protocol rewrite (spec §5.2.2) ----------

#[test]
fn py_index_loop_converges_with_for_loop() {
    let a = "def total(xs, floor):\n    acc = 0\n    for x in xs:\n        if x > floor:\n            acc += x\n        else:\n            acc -= 1\n        acc *= 2\n    return acc\n";
    let b = "def total(xs, floor):\n    acc = 0\n    for i in range(len(xs)):\n        if xs[i] > floor:\n            acc += xs[i]\n        else:\n            acc -= 1\n        acc *= 2\n    return acc\n";
    assert_pair(&scan_snippets("py", &[a, b]), &["exact-normalized"]);
}

#[test]
fn rust_index_loop_converges_with_for_loop() {
    let a = "fn total(xs: &[i64], floor: i64) -> i64 {\n    let mut acc = 0;\n    for x in xs {\n        if *x > floor {\n            acc += x;\n        } else {\n            acc -= 1;\n        }\n        acc *= 2;\n    }\n    acc\n}\n";
    let b = "fn total(xs: &[i64], floor: i64) -> i64 {\n    let mut acc = 0;\n    for i in 0..xs.len() {\n        if *xs[i] > floor {\n            acc += xs[i];\n        } else {\n            acc -= 1;\n        }\n        acc *= 2;\n    }\n    acc\n}\n";
    assert_pair(
        &scan_snippets("rs", &[a, b]),
        &["exact-normalized", "near-normalized"],
    );
}

// ---------- order canonicalization (spec §5.2.6) ----------

#[test]
fn commutative_operand_order_converges() {
    let a = "def blend(a, b, c, d):\n    total = a * b + c * d\n    flag = total > 10 and a > 0 and d > 0\n    if flag:\n        total += a * d + b * c\n        total *= 3\n    return total\n";
    let b = "def blend(a, b, c, d):\n    total = d * c + b * a\n    flag = d > 0 and a > 0 and total > 10\n    if flag:\n        total += c * b + d * a\n        total *= 3\n    return total\n";
    assert_pair(&scan_snippets("py", &[a, b]), &["exact-normalized"]);
}

// ---------- recursion lowering (spec §5.2.2, Rev 5) ----------

const PY_ITER: &str = "def reduce_pair(a, b, log):\n    while b != 0:\n        log.append(\"step: \" + str(a) + \" \" + str(b))\n        a, b = b, a % b\n    return a\n";
const PY_TAIL: &str = "def reduce_pair(a, b, log):\n    if b == 0:\n        return a\n    log.append(\"step: \" + str(a) + \" \" + str(b))\n    return reduce_pair(b, a % b, log)\n";
const PY_TREE: &str = "def reduce_pair(a, b, log):\n    if b == 0:\n        return a\n    log.append(\"step: \" + str(a) + \" \" + str(b))\n    left = reduce_pair(b, a % b, log)\n    right = reduce_pair(b % 3, a, log)\n    return max(left, right)\n";

#[test]
fn py_tail_recursion_converges_with_iteration() {
    assert_pair(
        &scan_snippets("py", &[PY_ITER, PY_TAIL]),
        &["exact-normalized", "near-normalized"],
    );
}

#[test]
fn rust_tail_recursion_converges_with_iteration() {
    let a = "fn reduce_pair(mut a: u64, mut b: u64, log: &mut Vec<String>) -> u64 {\n    while b != 0 {\n        log.push(format!(\"step: {a} {b}\"));\n        (a, b) = (b, a % b);\n    }\n    a\n}\n";
    let b = "fn reduce_pair(a: u64, b: u64, log: &mut Vec<String>) -> u64 {\n    if b == 0 {\n        return a;\n    }\n    log.push(format!(\"step: {a} {b}\"));\n    reduce_pair(b, a % b, log)\n}\n";
    assert_pair(
        &scan_snippets("rs", &[a, b]),
        &["exact-normalized", "near-normalized"],
    );
}

#[test]
fn tree_recursion_control_does_not_converge() {
    // Designed-to-fail control (spec §7.1): non-linear recursion must NOT lower.
    assert_no_pair(&scan_snippets("py", &[PY_ITER, PY_TREE]));
}

// ---------- near-miss tier: anti-unification (spec §5.6) ----------

#[test]
fn reordered_statements_converge_near() {
    let a = "def setup(cfg, name):\n    host = cfg.get(\"host\")\n    port = cfg.get(\"port\")\n    retries = cfg.get(\"retries\")\n    label = name.strip().lower()\n    conn = connect(host, port, retries)\n    register(conn, label)\n    if conn.ok:\n        conn.ping()\n        announce(conn, label)\n    return conn\n";
    let b = "def setup(cfg, name):\n    port = cfg.get(\"port\")\n    host = cfg.get(\"host\")\n    retries = cfg.get(\"retries\")\n    label = name.strip().lower()\n    conn = connect(host, port, retries)\n    register(conn, label)\n    if conn.ok:\n        conn.ping()\n        announce(conn, label)\n    return conn\n";
    assert_pair(&scan_snippets("py", &[a, b]), &["near-normalized"]);
}

#[test]
fn single_subtree_substitution_converges_near() {
    let a = "def score_rows(rows, cutoff):\n    out = []\n    for row in rows:\n        weight = row.base * row.factor\n        if weight > cutoff:\n            out.append((row.id, weight))\n        else:\n            out.append((row.id, 0))\n        tally(row, weight)\n    return out\n";
    let b = "def score_rows(rows, cutoff):\n    out = []\n    for row in rows:\n        weight = row.base * row.factor + row.bonus / 2\n        if weight > cutoff:\n            out.append((row.id, weight))\n        else:\n            out.append((row.id, 0))\n        tally(row, weight)\n    return out\n";
    assert_pair(&scan_snippets("py", &[a, b]), &["near-normalized"]);
}

#[test]
fn near_group_carries_template_with_holes() {
    let a = "def score_rows(rows, cutoff):\n    out = []\n    for row in rows:\n        weight = row.base * row.factor\n        if weight > cutoff:\n            out.append((row.id, weight))\n        else:\n            out.append((row.id, 0))\n        tally(row, weight)\n    return out\n";
    let b = "def score_rows(rows, cutoff):\n    out = []\n    for row in rows:\n        weight = row.base * row.factor + row.bonus / 2\n        if weight > cutoff:\n            out.append((row.id, weight))\n        else:\n            out.append((row.id, 0))\n        tally(row, weight)\n    return out\n";
    let report = scan_snippets("py", &[a, b]);
    let group = report
        .groups
        .iter()
        .find(|g| g.tier.to_string() == "near-normalized")
        .expect("near group");
    let template = group.template.as_deref().expect("template rendered");
    assert!(
        template.contains("⟨"),
        "template must mark holes: {template}"
    );
    let max = Config::default().thresholds.max_divergence;
    assert!(group.divergence > 0.0 && group.divergence <= max);
}

#[test]
fn unrelated_functions_do_not_converge() {
    let a = "def parse_headers(lines):\n    out = {}\n    for line in lines:\n        if \":\" not in line:\n            continue\n        key, _, val = line.partition(\":\")\n        out[key.strip().lower()] = val.strip()\n    return out\n";
    let b = "def fib_window(n, size):\n    a, b = 0, 1\n    window = []\n    while n > 0:\n        window.append(a)\n        a, b = b, a + b\n        if len(window) > size:\n            window.pop(0)\n        n -= 1\n    return window\n";
    assert_no_pair(&scan_snippets("py", &[a, b]));
}

// ---------- folding (spec §5.3) ----------

const PY_ROLLED: &str = "def mix_rounds(state, seed):\n    acc = seed\n    for part in state:\n        acc = rotate(acc, 7)\n        acc = acc ^ mash(part, 31)\n        acc = acc + 1442695\n        acc = acc ^ (acc >> 13)\n        acc = acc * 636413\n    return acc\n";
const PY_UNROLLED: &str = "def mix_rounds(state, seed):\n    acc = seed\n    acc = rotate(acc, 7)\n    acc = acc ^ mash(state[0], 31)\n    acc = acc + 1442695\n    acc = acc ^ (acc >> 13)\n    acc = acc * 636413\n    acc = rotate(acc, 7)\n    acc = acc ^ mash(state[1], 31)\n    acc = acc + 1442695\n    acc = acc ^ (acc >> 13)\n    acc = acc * 636413\n    acc = rotate(acc, 7)\n    acc = acc ^ mash(state[2], 31)\n    acc = acc + 1442695\n    acc = acc ^ (acc >> 13)\n    acc = acc * 636413\n    return acc\n";

#[test]
fn unrolled_loop_converges_with_rolled() {
    assert_pair(
        &scan_snippets("py", &[PY_ROLLED, PY_UNROLLED]),
        &["exact-normalized", "near-normalized"],
    );
}

#[test]
fn internal_repeat_is_reported() {
    // The unrolled function alone: 3 near-identical statement groups →
    // internal duplication finding (spec §5.3 "direct finding").
    let report = scan_snippets("py", &[PY_UNROLLED]);
    assert!(
        report
            .groups
            .iter()
            .any(|g| g.tier.to_string() == "internal-repeat"),
        "expected internal-repeat finding: {:#?}",
        report.groups
    );
}

// ---------- sequence tier (spec §5.6) ----------

#[test]
fn sub_unit_exact_run_reported_as_region() {
    // Two different functions sharing a long contiguous statement run.
    let shared = "    buf = prepare(items)\n    buf.sort()\n    emit_header(buf, \"v2\")\n    for entry in buf:\n        validate(entry, STRICT)\n        write_row(entry, buf)\n        bump_metric(\"rows\", entry)\n    flush_all(buf)\n";
    let a = format!(
        "def export_daily(items, path):\n    log_start(\"daily\", path)\n{shared}    seal(path)\n    return len(items)\n"
    );
    let b = format!(
        "def export_weekly(items, dest, limit):\n    if limit < 1:\n        return None\n    rotate_old(dest, limit)\n{shared}    notify(dest)\n    return dest\n"
    );
    let report = scan_snippets("py", &[&a, &b]);
    let region = report
        .groups
        .iter()
        .find(|g| g.tier.to_string() == "exact-region")
        .unwrap_or_else(|| panic!("expected exact-region finding: {:#?}", report.groups));
    assert_eq!(region.members.len(), 2);
    for m in &region.members {
        assert!(m.line_span.1 - m.line_span.0 >= 5, "region spans the run");
    }
}

#[test]
fn whole_unit_exact_match_not_double_reported() {
    let f = "def total(xs, floor):\n    acc = 0\n    for x in xs:\n        if x > floor:\n            acc += x\n        else:\n            acc -= 1\n        acc *= 2\n    return acc\n";
    let report = scan_snippets("py", &[f, f]);
    assert_eq!(
        report.groups.len(),
        1,
        "one exact group only, no region/near echoes: {:#?}",
        report.groups
    );
    assert_eq!(report.groups[0].tier.to_string(), "exact-normalized");
}

// ---------- test-code policy (spec §5.1) ----------

#[test]
fn test_units_report_separately() {
    let a = "def test_summarize_basic():\n    rows = load_fixture(\"basic\")\n    result = summarize(rows, 10)\n    assert result.total == 55\n    assert result.count == 4\n    assert result.label == \"ok\"\n    record_case(\"basic\", result)\n";
    let b = "def test_summarize_edge():\n    rows = load_fixture(\"edge\")\n    result = summarize(rows, 99)\n    assert result.total == 12\n    assert result.count == 7\n    assert result.label == \"edge\"\n    record_case(\"edge\", result)\n";
    let report = scan_snippets("py", &[a, b]);
    assert!(
        pair_tier(&report).is_none(),
        "test-vs-test findings must not sit in the main section"
    );
    assert!(
        !report.test_groups.is_empty(),
        "test-vs-test findings go to the test section: {:#?}",
        report.test_groups
    );
}
