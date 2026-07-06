//! Go profile contract (spec §3, M3c). Exact-tier convergence at the
//! fingerprint level; recursion↔iteration and test policy at `scan` level.
//! The canonical Go index loop (`for i:=0; i<len(xs); i++`) gets the
//! iteration-protocol rewrite (spec §5.2.2).

use reprise::config::Config;
use reprise::lang::Lang;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn fp(src: &str, lang: Lang) -> u128 {
    let units = reprise::units_from_source(src, lang, &Config::default());
    assert_eq!(units.len(), 1, "expected exactly one unit in:\n{src}");
    units[0].fingerprint
}

/// Fingerprint under the IR normalizer (`[normalize] normalizer = "ir"`).
fn fp_ir(src: &str, lang: Lang) -> u128 {
    let mut cfg = Config::default();
    cfg.normalize.normalizer = "ir".into();
    let units = reprise::units_from_source(src, lang, &cfg);
    assert_eq!(units.len(), 1, "expected exactly one unit in:\n{src}");
    units[0].fingerprint
}

fn pair_tier(sources: &[(&str, &str)]) -> Option<String> {
    let dir = TempDir::new().unwrap();
    for (name, src) in sources {
        fs::write(dir.path().join(name), src).unwrap();
    }
    let report = reprise::scan(dir.path(), &Config::default()).unwrap();
    report
        .groups
        .iter()
        .filter(|g| {
            let fs: Vec<_> = g
                .members
                .iter()
                .map(|m| m.file.to_string_lossy().to_string())
                .collect();
            fs.iter().any(|f| f.contains("aa.")) && fs.iter().any(|f| f.contains("bb."))
        })
        .map(|g| g.tier.to_string())
        .next()
}

fn is_test_unit(src: &str, filename: &str) -> bool {
    let (units, _) =
        reprise::unit::extract_file_units(Path::new(filename), src, Lang::Go, &Config::default());
    assert_eq!(units.len(), 1);
    units[0].is_test
}

// ---------- t1: comments + whitespace ----------

const T1_A: &str = "package main\nfunc total(xs []int, floor int) int {\n\tacc := 0\n\tfor _, x := range xs {\n\t\tif x > floor {\n\t\t\tacc += x\n\t\t}\n\t}\n\treturn acc\n}\n";
const T1_B: &str = "package main\n// running total\nfunc total(xs []int, floor int) int {\n\n\tacc := 0 // accumulator\n\n\tfor _, x := range xs {\n\t\t// skip small\n\t\tif x > floor {\n\t\t\tacc += x\n\t\t}\n\t}\n\treturn acc\n}\n";

#[test]
fn go_type1_comments_whitespace_converge() {
    assert_eq!(fp(T1_A, Lang::Go), fp(T1_B, Lang::Go));
}

// ---------- t2: rename + literals ----------

#[test]
fn go_type2_rename_and_literals_converge() {
    let a = "package main\nfunc label(n int) string {\n\tbase := 10\n\tif n > 90 {\n\t\treturn \"hot\"\n\t}\n\treturn \"cold\"\n}\n";
    let b = "package main\nfunc grade(v int) string {\n\tseed := 42\n\tif v > 55 {\n\t\treturn \"warm\"\n\t}\n\treturn \"chilly\"\n}\n";
    assert_eq!(fp(a, Lang::Go), fp(b, Lang::Go));
}

// ---------- while-form ↔ loop-core (`for cond` ↔ `for {}`) ----------

#[test]
fn go_while_form_and_loop_core_converge() {
    let a = "package main\nfunc drain(n int) int {\n\tk := n\n\ts := 0\n\tfor k > 0 {\n\t\tk -= 2\n\t\ts += 1\n\t}\n\treturn s\n}\n";
    let b = "package main\nfunc drain(n int) int {\n\tk := n\n\ts := 0\n\tfor {\n\t\tif !(k > 0) {\n\t\t\tbreak\n\t\t}\n\t\tk -= 2\n\t\ts += 1\n\t}\n\treturn s\n}\n";
    assert_eq!(fp(a, Lang::Go), fp(b, Lang::Go));
}

// ---------- three-clause for ↔ while+init ----------

#[test]
fn go_three_clause_for_converges_with_while() {
    let a = "package main\nfunc drain(n int) int {\n\ts := 0\n\tfor i := 0; i < n; i++ {\n\t\ts += i\n\t}\n\treturn s\n}\n";
    let b = "package main\nfunc drain(n int) int {\n\ts := 0\n\ti := 0\n\tfor i < n {\n\t\ts += i\n\t\ti++\n\t}\n\treturn s\n}\n";
    assert_eq!(fp(a, Lang::Go), fp(b, Lang::Go));
}

// ---------- for ↔ index-loop (THE canonical Go pattern, spec §5.2.2) ----------

#[test]
fn go_index_loop_converges_with_range() {
    let a = "package main\nfunc total(xs []int) int {\n\tacc := 0\n\tfor i := 0; i < len(xs); i++ {\n\t\tacc += xs[i]\n\t}\n\treturn acc\n}\n";
    let b = "package main\nfunc total(xs []int) int {\n\tacc := 0\n\tfor _, x := range xs {\n\t\tacc += x\n\t}\n\treturn acc\n}\n";
    assert_eq!(fp(a, Lang::Go), fp(b, Lang::Go));
}

#[test]
fn go_index_loop_converges_with_range_ir() {
    // Same contract under the IR normalizer (the default-flip blocker): the block-level
    // C-style counter loop and the idiomatic blank-index `range` both rewrite to the one
    // canonical iteration-protocol foreach form (spec §5.2.2).
    let a = "package main\nfunc total(xs []int) int {\n\tacc := 0\n\tfor i := 0; i < len(xs); i++ {\n\t\tacc += xs[i]\n\t}\n\treturn acc\n}\n";
    let b = "package main\nfunc total(xs []int) int {\n\tacc := 0\n\tfor _, x := range xs {\n\t\tacc += x\n\t}\n\treturn acc\n}\n";
    assert_eq!(fp_ir(a, Lang::Go), fp_ir(b, Lang::Go));
}

#[test]
fn go_counter_loop_near_misses_stay_distinct_ir() {
    // Precision: shapes that are NOT a clean index-iteration must NOT collapse to the foreach.
    let foreach = "package main\nfunc total(xs []int) int {\n\tacc := 0\n\tfor _, x := range xs {\n\t\tacc += x\n\t}\n\treturn acc\n}\n";
    // Non-unit stride.
    let stride = "package main\nfunc total(xs []int) int {\n\tacc := 0\n\tfor i := 0; i < len(xs); i += 2 {\n\t\tacc += xs[i]\n\t}\n\treturn acc\n}\n";
    // The index is used for its own sake (not only as `xs[i]`).
    let use_index = "package main\nfunc total(xs []int) int {\n\tacc := 0\n\tfor i := 0; i < len(xs); i++ {\n\t\tacc += xs[i] + i\n\t}\n\treturn acc\n}\n";
    // A different collection is indexed.
    let other = "package main\nfunc total(xs []int, ys []int) int {\n\tacc := 0\n\tfor i := 0; i < len(xs); i++ {\n\t\tacc += ys[i]\n\t}\n\treturn acc\n}\n";
    assert_ne!(fp_ir(foreach, Lang::Go), fp_ir(stride, Lang::Go), "stride");
    assert_ne!(
        fp_ir(foreach, Lang::Go),
        fp_ir(use_index, Lang::Go),
        "index used"
    );
    assert_ne!(
        fp_ir(foreach, Lang::Go),
        fp_ir(other, Lang::Go),
        "other coll"
    );
}

// ---------- recursion ↔ iteration (spec §5.2.2 Rev 5) ----------

const GO_ITER: &str = "package main\nfunc reducePair(a int, b int, log []int) int {\n\tfor b != 0 {\n\t\tlog = append(log, a)\n\t\ta, b = b, a%b\n\t}\n\treturn a\n}\n";
const GO_TAIL: &str = "package main\nfunc reducePair(a int, b int, log []int) int {\n\tif b == 0 {\n\t\treturn a\n\t}\n\tlog = append(log, a)\n\treturn reducePair(b, a%b, log)\n}\n";

#[test]
fn go_tail_recursion_converges_with_iteration() {
    let tier = pair_tier(&[("aa.go", GO_ITER), ("bb.go", GO_TAIL)]);
    assert!(
        tier.as_deref()
            .is_some_and(|t| matches!(t, "exact-normalized" | "near-normalized" | "exact-region")),
        "tail recursion must converge with iteration; got {tier:?}"
    );
}

#[test]
fn go_tail_recursion_converges_with_iteration_ir() {
    // On the IR path the coupled `a, b = b, a%b` reassignment converges EXACTLY (fingerprint-
    // equal): the tail-rec form lowers to the same parallel `Assign` the iterative form has,
    // and both decompose to the same temped single-assign sequence.
    assert_eq!(
        fp_ir(GO_ITER, Lang::Go),
        fp_ir(GO_TAIL, Lang::Go),
        "Go tail recursion must converge with iteration on the IR path",
    );
}

// ---------- negative controls ----------

#[test]
fn go_external_callee_change_diverges() {
    let a = "package main\nfunc f(xs []int) []int {\n\tout := make([]int, 0)\n\tfor _, x := range xs {\n\t\tout = append(out, parse(x))\n\t}\n\treturn out\n}\n";
    let b = "package main\nfunc f(xs []int) []int {\n\tout := make([]int, 0)\n\tfor _, x := range xs {\n\t\tout = append(out, render(x))\n\t}\n\treturn out\n}\n";
    assert_ne!(fp(a, Lang::Go), fp(b, Lang::Go));
}

#[test]
fn go_unrelated_functions_do_not_converge() {
    let a = "package main\nfunc parseHeaders(lines []string) map[string]string {\n\tout := map[string]string{}\n\tfor _, line := range lines {\n\t\tif !strings.Contains(line, \":\") {\n\t\t\tcontinue\n\t\t}\n\t\tparts := strings.SplitN(line, \":\", 2)\n\t\tout[strings.ToLower(parts[0])] = strings.TrimSpace(parts[1])\n\t}\n\treturn out\n}\n";
    let b = "package main\nfunc fibWindow(n int, size int) []int {\n\ta, b := 0, 1\n\twindow := make([]int, 0)\n\tfor n > 0 {\n\t\twindow = append(window, a)\n\t\ta, b = b, a+b\n\t\tif len(window) > size {\n\t\t\twindow = window[1:]\n\t\t}\n\t\tn--\n\t}\n\treturn window\n}\n";
    assert!(
        pair_tier(&[("aa.go", a), ("bb.go", b)]).is_none(),
        "unrelated functions must not converge"
    );
}

// ---------- unit_is_test recognition (TestXxx in _test.go) ----------

#[test]
fn go_test_recognition() {
    let f = "package main\nfunc TestSummarize(t *testing.T) {\n\tgot := summarize(1)\n\tif got != 2 {\n\t\tt.Errorf(\"bad: %d\", got)\n\t}\n}\n";
    assert!(is_test_unit(f, "summary_test.go"), "_test.go → test");
    assert!(!is_test_unit(f, "summary.go"), "plain file → not test");
}
