//! Phase-2 integration contract (spec §5.2 desugaring, §5.3 folding, §5.6
//! sequence + near-miss tiers, §5.1 test policy), all at `scan` level.

use reprise::config::{Config, Normalizer};
use reprise::report::ScanReport;
use std::fs;
use tempfile::TempDir;

fn scan_with(ext: &str, sources: &[&str], cfg: &Config) -> ScanReport {
    let dir = TempDir::new().unwrap();
    for (i, src) in sources.iter().enumerate() {
        fs::write(dir.path().join(format!("m{i}.{ext}")), src).unwrap();
    }
    reprise::scan(dir.path(), cfg).unwrap()
}

fn scan_snippets(ext: &str, sources: &[&str]) -> ScanReport {
    scan_with(ext, sources, &Config::default())
}

/// Same as [`scan_snippets`] but on the IR normalizer path (`[normalize] normalizer = "ir"`) —
/// the switch-over recall gate for the near/region tiers under the canonical IR.
fn scan_snippets_ir(ext: &str, sources: &[&str]) -> ScanReport {
    let mut cfg = Config::default();
    cfg.normalize.normalizer = Normalizer::Ir;
    scan_with(ext, sources, &cfg)
}

/// Same as [`scan_snippets`] but pinned to the still-supported historical normalizer
/// (`[normalize] normalizer = "historical"`) — for contracts calibrated to the larger
/// historical tree, where a fixed-size difference stays a smaller fraction of the whole than
/// under the ~18%-more-compact IR default (so a near-miss stays a whole-unit near match rather
/// than tightening to an exact-region residual).
fn scan_snippets_historical(ext: &str, sources: &[&str]) -> ScanReport {
    let mut cfg = Config::default();
    cfg.normalize.normalizer = Normalizer::Historical;
    scan_with(ext, sources, &cfg)
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
    // Designed-to-fail control (spec §7.1): non-linear recursion must NOT lower. Pinned to the
    // historical normalizer: under the IR default `PY_ITER`/`PY_TREE` share a genuine ~30-token
    // `log.append` fragment that clears `min_seq_tokens` and reports as an ACCEPTED exact-region
    // residual, so a whole-scan "does not converge" assertion would trip on it. The IR unit-level
    // control is `tree_recursion_control_stays_distinct_ir` (the `fp_ir` convention).
    assert_no_pair(&scan_snippets_historical("py", &[PY_ITER, PY_TREE]));
}

/// Fingerprint under the IR normalizer (`[normalize] normalizer = "ir"`).
fn fp_ir(src: &str, lang: reprise::lang::Lang) -> u128 {
    let mut cfg = Config::default();
    cfg.normalize.normalizer = Normalizer::Ir;
    let units = reprise::units_from_source(src, lang, &cfg);
    assert_eq!(units.len(), 1, "expected exactly one unit in:\n{src}");
    units[0].fingerprint
}

#[test]
fn py_tail_recursion_converges_with_iteration_ir() {
    // The same coupled `a, b = b, a%b` reassignment as `PY_TAIL`/`PY_ITER`, now converging
    // EXACTLY (fingerprint-equal) on the IR path: tail-rec reassignment lowers to the same
    // parallel `Assign` the iterative multi-assign has, and both decompose identically.
    use reprise::lang::Lang;
    assert_eq!(
        fp_ir(PY_ITER, Lang::Python),
        fp_ir(PY_TAIL, Lang::Python),
        "Python tail recursion must converge with iteration on the IR path",
    );
}

#[test]
fn rust_tail_recursion_converges_with_iteration_ir() {
    // Explicit `return a;` in the iterative form (not Rust's implicit tail expression `a`): the
    // bare-tail-expression-vs-`Return` modeling gap is pre-existing and orthogonal to the
    // multi-assign decomposition under test (same reason the Go convergence tests use `return`).
    use reprise::lang::Lang;
    let iter = "fn reduce_pair(mut a: u64, mut b: u64, log: &mut Vec<String>) -> u64 {\n    while b != 0 {\n        log.push(format!(\"step: {a} {b}\"));\n        (a, b) = (b, a % b);\n    }\n    return a;\n}\n";
    let tail = "fn reduce_pair(a: u64, b: u64, log: &mut Vec<String>) -> u64 {\n    if b == 0 {\n        return a;\n    }\n    log.push(format!(\"step: {a} {b}\"));\n    return reduce_pair(b, a % b, log);\n}\n";
    assert_eq!(
        fp_ir(iter, Lang::Rust),
        fp_ir(tail, Lang::Rust),
        "Rust tail recursion must converge with iteration on the IR path",
    );
}

#[test]
fn tree_recursion_control_stays_distinct_ir() {
    // Regression (IR path): `PY_TREE` (two self-calls, neither in tail position — non-linear
    // recursion) must NOT collapse into the iterative `PY_ITER`'s shape. `lower_tail_recursion`
    // now lowers ONLY a lone tail self-call, so tree/branchy recursion stays recursion and the
    // two units keep distinct fingerprints (the algorithm, not the incidental shared guard+log
    // fragment, is what the control tests). Genuine tail recursion still converges — see
    // `py_tail_recursion_converges_with_iteration_ir`, which must stay green alongside this.
    use reprise::lang::Lang;
    assert_ne!(
        fp_ir(PY_ITER, Lang::Python),
        fp_ir(PY_TREE, Lang::Python),
        "iterative gcd must not converge (as a unit) with the tree-recursive control on IR",
    );
}

#[test]
fn branchy_tail_recursion_stays_distinct_ir() {
    // The lone-tail-call guard: TWO tail-position self-calls is branchy (non-linear) recursion —
    // each call spawns its own continuation — so it must NOT lower to a single loop and thus must
    // not converge with the iterative accumulator form.
    use reprise::lang::Lang;
    let iter = "def walk(n, acc):\n    while n > 0:\n        acc = acc + n\n        n = n - 1\n    return acc\n";
    let branchy = "def walk(n, acc):\n    if n <= 0:\n        return acc\n    if n % 2 == 0:\n        return walk(n - 1, acc + n)\n    return walk(n - 2, acc + n)\n";
    assert_ne!(
        fp_ir(iter, Lang::Python),
        fp_ir(branchy, Lang::Python),
        "branchy (two-tail-call) recursion must not lower to a loop on IR",
    );
}

#[test]
fn tuple_index_loop_converges_with_tuple_foreach_ir() {
    // Regression (IR path): the tuple foreach `for label, score in entries` and the tuple index
    // loop `for i in range(len(entries)): label, score = entries[i]` converge EXACTLY on IR — the
    // destructure targets abstract (declared locals), and the foreach's element temp matches the
    // index form's `i = __next(...)` + `(label, score) = i` two-step shape.
    use reprise::lang::Lang;
    let foreach = "def f(entries):\n    out = []\n    for label, score in entries:\n        out.append(label + str(score))\n    return out\n";
    let index = "def f(entries):\n    out = []\n    for i in range(len(entries)):\n        label, score = entries[i]\n        out.append(label + str(score))\n    return out\n";
    assert_eq!(
        fp_ir(foreach, Lang::Python),
        fp_ir(index, Lang::Python),
        "tuple foreach must converge with the tuple index loop on IR",
    );
}

#[test]
fn rust_tuple_index_loop_converges_with_tuple_foreach_ir() {
    // The Rust while-counter form (`let (label, score) = &entries[i]; i += 1;`, the increment NOT
    // last) rewrites to the same iteration-protocol loop as the tuple `for`; the sole residual is
    // the `&` reference on the destructured element, so the pair lands in the near tier.
    let foreach = "pub fn f(entries: &[(String, i64)]) -> i64 {\n    let mut t = 0;\n    for (label, score) in entries {\n        t += score;\n    }\n    t\n}\n";
    let index = "pub fn f(entries: &[(String, i64)]) -> i64 {\n    let mut t = 0;\n    let mut i = 0;\n    while i < entries.len() {\n        let (label, score) = &entries[i];\n        i += 1;\n        t += score;\n    }\n    t\n}\n";
    assert_pair(
        &scan_snippets_ir("rs", &[foreach, index]),
        &["exact-normalized", "near-normalized"],
    );
}

#[test]
fn unrolled_loop_converges_with_rolled_ir() {
    // Regression (IR path): a folded `REPEAT` (the unrolled body) converges with the rolled loop's
    // lowered form via the AU REPEAT-vs-loop-core special case — which needs the IR `Loop` kind
    // recognized as the loop core (the historical profile only knows `while True`).
    assert_pair(
        &scan_snippets_ir("py", &[PY_ROLLED, PY_UNROLLED]),
        &["exact-normalized", "near-normalized"],
    );
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
    // Pinned to the historical normalizer. The single differing subtree (`+ row.bonus / 2`) is a
    // ~16-token hole; on the compact IR tree that pushes the whole-unit divergence to ~0.21, past
    // `max_divergence` (0.18), so under the IR default the pair still converges but tightens to an
    // exact-region finding over the shared loop body (recall preserved — not a regression). This
    // asserts the historical whole-unit near-miss calibration; IR near-miss recall is covered by
    // `reordered_statements_converge_near` (IR default) and the phase3 template-evidence tests.
    assert_pair(
        &scan_snippets_historical("py", &[a, b]),
        &["near-normalized"],
    );
}

#[test]
fn near_group_carries_template_with_holes() {
    let a = "def score_rows(rows, cutoff):\n    out = []\n    for row in rows:\n        weight = row.base * row.factor\n        if weight > cutoff:\n            out.append((row.id, weight))\n        else:\n            out.append((row.id, 0))\n        tally(row, weight)\n    return out\n";
    let b = "def score_rows(rows, cutoff):\n    out = []\n    for row in rows:\n        weight = row.base * row.factor + row.bonus / 2\n        if weight > cutoff:\n            out.append((row.id, weight))\n        else:\n            out.append((row.id, 0))\n        tally(row, weight)\n    return out\n";
    // Pinned to the historical normalizer for the same reason as
    // `single_subtree_substitution_converges_near`: this fixture's whole-unit near match tightens
    // to an exact-region (which carries no hole template) under the compact IR default. This keeps
    // the historical near-miss template-hole rendering gated; IR template evidence is covered by
    // the phase3 api-finding tests.
    let report = scan_snippets_historical("py", &[a, b]);
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
