//! Phase-1 convergence contract (spec §5.2 steps 1/4/5, §5.5.1):
//! Type-1/Type-2 variants must produce equal exact structural hashes;
//! genuinely different code must not.

use reprise::config::Config;
use reprise::lang::Lang;

fn fp(src: &str, lang: Lang) -> u128 {
    let cfg = Config::default();
    let units = reprise::units_from_source(src, lang, &cfg);
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

// ---------- Type-1: whitespace + comments ----------

#[test]
fn rust_type1_comments_and_whitespace_converge() {
    let a = r#"
fn total(xs: &[i64], floor: i64) -> i64 {
    let mut acc = 0;
    for x in xs {
        if *x > floor {
            acc += x;
        }
    }
    acc
}
"#;
    let b = r#"
// Computes a running total.
fn total(xs: &[i64], floor: i64) -> i64 {

    let mut acc = 0; // accumulator

    for x in xs {
        /* skip small values */
        if *x > floor {
            acc += x;
        }
    }
    acc
}
"#;
    assert_eq!(fp(a, Lang::Rust), fp(b, Lang::Rust));
}

#[test]
fn python_type1_comments_and_whitespace_converge() {
    let a = "def total(xs, floor):\n    acc = 0\n    for x in xs:\n        if x > floor:\n            acc += x\n    return acc\n";
    let b = "def total(xs, floor):\n    # accumulator\n    acc = 0\n\n    for x in xs:\n\n        if x > floor:  # skip small values\n            acc += x\n    return acc\n";
    assert_eq!(fp(a, Lang::Python), fp(b, Lang::Python));
}

// ---------- Type-2: identifier renames (incl. unit name) ----------

#[test]
fn rust_type2_rename_converges() {
    let a = r#"
fn total(xs: &[i64], floor: i64) -> i64 {
    let mut acc = 0;
    for x in xs {
        if *x > floor {
            acc += x;
        }
    }
    acc
}
"#;
    let b = r#"
fn sum_above(values: &[i64], cutoff: i64) -> i64 {
    let mut result = 0;
    for v in values {
        if *v > cutoff {
            result += v;
        }
    }
    result
}
"#;
    assert_eq!(fp(a, Lang::Rust), fp(b, Lang::Rust));
}

#[test]
fn python_type2_rename_converges() {
    let a = "def total(xs, floor):\n    acc = 0\n    for x in xs:\n        if x > floor:\n            acc += x\n    return acc\n";
    let b = "def sum_above(values, cutoff):\n    result = 0\n    for v in values:\n        if v > cutoff:\n            result += v\n    return result\n";
    assert_eq!(fp(a, Lang::Python), fp(b, Lang::Python));
}

// ---------- Type-2: literal changes (non-keep-list) ----------

#[test]
fn rust_type2_literals_converge() {
    let a = r#"fn label(n: i64) -> &'static str { if n > 90 { "hot" } else { "cold" } }"#;
    let b = r#"fn label(n: i64) -> &'static str { if n > 40 { "warm" } else { "chilly" } }"#;
    assert_eq!(fp(a, Lang::Rust), fp(b, Lang::Rust));
}

#[test]
fn python_type2_literals_converge() {
    let a = "def label(n):\n    if n > 90:\n        return \"hot\"\n    return \"cold\"\n";
    let b = "def label(n):\n    if n > 40:\n        return \"warm\"\n    return \"chilly\"\n";
    assert_eq!(fp(a, Lang::Python), fp(b, Lang::Python));
}

// ---------- Keep-list literals ARE structural (spec §5.2.5) ----------

#[test]
fn keep_list_literal_change_diverges() {
    // 0 vs 1 as the accumulator seed: keep-list identity must survive abstraction.
    let a = "def f(xs):\n    acc = 0\n    for x in xs:\n        acc += x\n    return acc\n";
    let b = "def f(xs):\n    acc = 1\n    for x in xs:\n        acc += x\n    return acc\n";
    assert_ne!(fp(a, Lang::Python), fp(b, Lang::Python));
}

// ---------- External call names are semantic (spec §5.2.4 exception) ----------

#[test]
fn external_callee_change_diverges() {
    let a = "def f(xs):\n    out = []\n    for x in xs:\n        out.append(parse(x))\n    return out\n";
    let b = "def f(xs):\n    out = []\n    for x in xs:\n        out.append(render(x))\n    return out\n";
    assert_ne!(fp(a, Lang::Python), fp(b, Lang::Python));
}

// ---------- Different structure diverges ----------

#[test]
fn structural_change_diverges() {
    let a = r#"
fn f(xs: &[i64]) -> i64 {
    let mut acc = 0;
    for x in xs {
        acc += x;
    }
    acc
}
"#;
    let b = r#"
fn f(xs: &[i64]) -> i64 {
    let mut acc = 0;
    for x in xs {
        if *x > 0 {
            acc += x;
        }
    }
    acc
}
"#;
    assert_ne!(fp(a, Lang::Rust), fp(b, Lang::Rust));
}

// ---------- Operators are structural ----------

#[test]
fn operator_change_diverges() {
    let a = "def f(a, b):\n    if a > b:\n        return a - b\n    return b - a\n";
    let b = "def f(a, b):\n    if a < b:\n        return a - b\n    return b - a\n";
    assert_ne!(fp(a, Lang::Python), fp(b, Lang::Python));
}

// ---------- Loop lowering: `while cond` and `loop { if !cond break }` converge (Rust) ----------

#[test]
fn rust_while_and_loop_break_converge() {
    let a = r#"
fn drain(n: i64) -> i64 {
    let mut k = n;
    let mut steps = 0;
    while k > 0 {
        k -= 2;
        steps += 1;
    }
    steps
}
"#;
    let b = r#"
fn drain(n: i64) -> i64 {
    let mut k = n;
    let mut steps = 0;
    loop {
        if !(k > 0) {
            break;
        }
        k -= 2;
        steps += 1;
    }
    steps
}
"#;
    assert_eq!(fp(a, Lang::Rust), fp(b, Lang::Rust));
}

// ---------- Python: `while cond` and `while True: if not cond: break` converge ----------

#[test]
fn python_while_forms_converge() {
    let a = "def drain(n):\n    k = n\n    steps = 0\n    while k > 0:\n        k -= 2\n        steps += 1\n    return steps\n";
    let b = "def drain(n):\n    k = n\n    steps = 0\n    while True:\n        if not (k > 0):\n            break\n        k -= 2\n        steps += 1\n    return steps\n";
    assert_eq!(fp(a, Lang::Python), fp(b, Lang::Python));
}

// ---------- Counter-loop iteration rewrite (IR path, spec §5.2.2) ----------
//
// The C-style counter loop (`i = 0; while i < len(coll) { … coll[i] …; i += 1 }`) is the
// same iteration as `for x in coll`. Under the IR normalizer, Go counter ≡ Go range ≡ Rust
// foreach ≡ Rust while-index all converge to one canonical iteration-protocol form.

const RUST_FOREACH: &str =
    "fn total(xs: &[i64]) -> i64 { let mut acc = 0; for x in xs { acc += x; } acc }";

#[test]
fn rust_while_index_converges_with_foreach_ir() {
    let while_index = "fn total(xs: &[i64]) -> i64 { let mut acc = 0; let mut i = 0; while i < xs.len() { acc += xs[i]; i += 1; } acc }";
    assert_eq!(
        fp_ir(while_index, Lang::Rust),
        fp_ir(RUST_FOREACH, Lang::Rust),
        "Rust while-index loop must converge with the foreach form",
    );
}

#[test]
fn rust_range_index_converges_with_foreach_ir() {
    let range = "fn total(xs: &[i64]) -> i64 { let mut acc = 0; for i in 0..xs.len() { acc += xs[i]; } acc }";
    assert_eq!(
        fp_ir(range, Lang::Rust),
        fp_ir(RUST_FOREACH, Lang::Rust),
        "Rust range-index loop must converge with the foreach form",
    );
}

#[test]
fn go_counter_loop_converges_with_rust_foreach_ir() {
    // Cross-language: a Go C-style counter loop and a Rust foreach lower to the SAME
    // canonical iteration-protocol tree (same fingerprint). Both use an explicit `return`
    // so the block tails match (Go has no implicit tail-expression form — a pre-existing
    // return-modeling gap, orthogonal to the loop rewrite).
    let go_counter = "package main\nfunc total(xs []int64) int64 {\n\tacc := 0\n\tfor i := 0; i < len(xs); i++ {\n\t\tacc += xs[i]\n\t}\n\treturn acc\n}\n";
    let rust_foreach =
        "fn total(xs: &[i64]) -> i64 { let mut acc = 0; for x in xs { acc += x; } return acc; }";
    assert_eq!(
        fp_ir(go_counter, Lang::Go),
        fp_ir(rust_foreach, Lang::Rust),
        "Go counter loop must converge cross-language with the Rust foreach",
    );
}

#[test]
fn rust_while_index_near_misses_stay_distinct_ir() {
    // Precision: a mutated index, a non-unit stride, or an index used beyond `xs[i]` must
    // NOT collapse to the foreach form.
    let mutated = "fn total(xs: &[i64]) -> i64 { let mut acc = 0; let mut i = 0; while i < xs.len() { i = i + 3; acc += xs[i]; i += 1; } acc }";
    let stride = "fn total(xs: &[i64]) -> i64 { let mut acc = 0; let mut i = 0; while i < xs.len() { acc += xs[i]; i += 2; } acc }";
    let use_index = "fn total(xs: &[i64]) -> i64 { let mut acc = 0; let mut i = 0; while i < xs.len() { acc += xs[i] + (i as i64); i += 1; } acc }";
    assert_ne!(
        fp_ir(mutated, Lang::Rust),
        fp_ir(RUST_FOREACH, Lang::Rust),
        "mutated index",
    );
    assert_ne!(
        fp_ir(stride, Lang::Rust),
        fp_ir(RUST_FOREACH, Lang::Rust),
        "non-unit stride",
    );
    assert_ne!(
        fp_ir(use_index, Lang::Rust),
        fp_ir(RUST_FOREACH, Lang::Rust),
        "index used for its own sake",
    );
}

// ---------- Self-referential assignment is a mutation (IR path, cross-language) ----------
//
// A target read in its own RHS (`i = i + 1`) is provably a mutation of an already-bound
// variable, so its target takes `@place` — the same model Go/Rust use for `=`/`op=`.
// Python's `i = i + 1` / `i += 1` now converge within Python AND cross-language with the
// Go/Rust equivalents (one canonical fingerprint under the IR normalizer).

#[test]
fn self_referential_assignment_converges_cross_language_ir() {
    let py_explicit = "def f(i):\n    i = i + 1\n    return i\n";
    let py_aug = "def f(i):\n    i += 1\n    return i\n";
    let go_explicit = "package main\nfunc f(i int64) int64 {\n\ti = i + 1\n\treturn i\n}\n";
    let go_aug = "package main\nfunc f(i int64) int64 {\n\ti += 1\n\treturn i\n}\n";
    let rust_explicit = "fn f(mut i: i64) -> i64 { i = i + 1; return i; }";
    let rust_aug = "fn f(mut i: i64) -> i64 { i += 1; return i; }";

    let target = fp_ir(py_explicit, Lang::Python);
    // Within Python: explicit self-assign ≡ aug-assign.
    assert_eq!(
        fp_ir(py_aug, Lang::Python),
        target,
        "python i += 1 vs i = i + 1"
    );
    // Cross-language: Go and Rust, both spellings, all converge to the same fingerprint.
    assert_eq!(fp_ir(go_explicit, Lang::Go), target, "go i = i + 1");
    assert_eq!(fp_ir(go_aug, Lang::Go), target, "go i += 1");
    assert_eq!(fp_ir(rust_explicit, Lang::Rust), target, "rust i = i + 1");
    assert_eq!(fp_ir(rust_aug, Lang::Rust), target, "rust i += 1");
}

#[test]
fn accumulator_idiom_converges_cross_language_ir() {
    // The accumulator (`total = total + x`) is the same self-referential mutation as the
    // counter — Python's explicit and augmented forms converge with the Go/Rust equivalents.
    let py = "def f(total, x):\n    total = total + x\n    return total\n";
    let py_aug = "def f(total, x):\n    total += x\n    return total\n";
    let go = "package main\nfunc f(total int64, x int64) int64 {\n\ttotal = total + x\n\treturn total\n}\n";
    let rust = "fn f(mut total: i64, x: i64) -> i64 { total = total + x; return total; }";

    let t = fp_ir(py, Lang::Python);
    assert_eq!(fp_ir(py_aug, Lang::Python), t, "python total += x");
    assert_eq!(fp_ir(go, Lang::Go), t, "go accumulator");
    assert_eq!(fp_ir(rust, Lang::Rust), t, "rust accumulator");
}

#[test]
fn fresh_binding_stays_distinct_from_mutation_ir() {
    // Precision: a genuine fresh binding (`x = 5`, non-self-referential → `@target`) must NOT
    // be mislabeled as a mutation, so it stays distinct from the self-referential `i = i + 1`.
    let fresh = "def f():\n    x = 5\n    return x\n";
    let mutation = "def f(i):\n    i = i + 1\n    return i\n";
    assert_ne!(
        fp_ir(fresh, Lang::Python),
        fp_ir(mutation, Lang::Python),
        "a fresh binding must not converge with a self-referential mutation",
    );
}

// ---------- Parallel multi-assign decomposition (IR path, spec §13 multi-assign rung) ----------
//
// A parallel multi-target assignment (`a, b = X, Y`) decomposes to a canonical **sequence of
// single assigns** with minimal temps: an INDEPENDENT assign needs none (byte-identical to the
// two adjacent single assigns), a COUPLED one (the gcd swap) gets exactly one cycle-breaking
// temp. Tail-rec reassignment lowers to the same parallel `Assign`, so it converges too.

#[test]
fn independent_multi_assign_converges_with_two_adjacent_assigns_ir() {
    // No cross-dependency ⇒ no temp ⇒ the decomposition IS the two adjacent single assigns.
    let py_parallel = "def f(x, y):\n    x, y = x + 1, y * 2\n    return x\n";
    let py_adjacent = "def f(x, y):\n    x = x + 1\n    y = y * 2\n    return x\n";
    assert_eq!(
        fp_ir(py_parallel, Lang::Python),
        fp_ir(py_adjacent, Lang::Python),
        "independent `x, y = x+1, y*2` must be identical to `x = x+1; y = y*2`",
    );
    // Same in Rust (tuple assignment `(x, y) = (x+1, y*2)`).
    let rs_parallel = "fn f(mut x: i64, mut y: i64) -> i64 { (x, y) = (x + 1, y * 2); return x; }";
    let rs_adjacent = "fn f(mut x: i64, mut y: i64) -> i64 { x = x + 1; y = y * 2; return x; }";
    assert_eq!(
        fp_ir(rs_parallel, Lang::Rust),
        fp_ir(rs_adjacent, Lang::Rust),
        "Rust independent tuple assign must equal the two adjacent assigns",
    );
}

#[test]
fn coupled_multi_assign_converges_cross_language_ir() {
    // The gcd swap `a, b = b, a%b` gets one cycle-breaking temp and converges across all three
    // IR frontends (Python / Go / Rust) to ONE canonical fingerprint.
    let py = "def f(a, b):\n    a, b = b, a % b\n    return a\n";
    let go = "package main\nfunc f(a int64, b int64) int64 {\n\ta, b = b, a%b\n\treturn a\n}\n";
    let rs = "fn f(mut a: i64, mut b: i64) -> i64 { (a, b) = (b, a % b); return a; }";
    let t = fp_ir(py, Lang::Python);
    assert_eq!(fp_ir(go, Lang::Go), t, "go coupled multi-assign");
    assert_eq!(fp_ir(rs, Lang::Rust), t, "rust coupled multi-assign");
}

#[test]
fn gcd_multi_assign_converges_with_tail_recursion_ir() {
    // The driving case: iterative gcd (`while b != 0 { a, b = b, a%b }`) ≡ tail-rec gcd
    // (`return gcd(b, a%b)`), because the tail-rec reassignment lowers to the SAME parallel
    // `Assign` the iterative form has — both decompose to `t=a; a=b; b=t%b`.
    let py_iter = "def gcd(a, b):\n    while b != 0:\n        a, b = b, a % b\n    return a\n";
    let py_tail = "def gcd(a, b):\n    if b == 0:\n        return a\n    return gcd(b, a % b)\n";
    assert_eq!(
        fp_ir(py_iter, Lang::Python),
        fp_ir(py_tail, Lang::Python),
        "Python iterative gcd must converge with tail-rec gcd on the IR path",
    );
    let go_iter = "package main\nfunc gcd(a int, b int) int {\n\tfor b != 0 {\n\t\ta, b = b, a%b\n\t}\n\treturn a\n}\n";
    let go_tail = "package main\nfunc gcd(a int, b int) int {\n\tif b == 0 {\n\t\treturn a\n\t}\n\treturn gcd(b, a%b)\n}\n";
    assert_eq!(
        fp_ir(go_iter, Lang::Go),
        fp_ir(go_tail, Lang::Go),
        "Go iterative gcd must converge with tail-rec gcd on the IR path",
    );
}

#[test]
fn sequential_assigns_stay_distinct_from_parallel_ir() {
    // Discrimination: the semantically-DIFFERENT sequential `a = b; b = a%b` (b reads the NEW a)
    // must NOT converge with the parallel `a, b = b, a%b` (b reads the OLD a, hence the temp).
    let sequential = "def f(a, b):\n    a = b\n    b = a % b\n    return a\n";
    let parallel = "def f(a, b):\n    a, b = b, a % b\n    return a\n";
    assert_ne!(
        fp_ir(sequential, Lang::Python),
        fp_ir(parallel, Lang::Python),
        "sequential `a=b; b=a%b` must stay distinct from parallel `a, b = b, a%b`",
    );
}

#[test]
fn independent_decomposition_does_not_collide_with_unrelated_pair_ir() {
    // Discrimination: an independent decomposition (`x, y = x+1, y*2` → `x=x+1; y=y*2`) must not
    // collide with an unrelated pair of assigns (`x = y+1; y = x*2` — different reads).
    let a = "def f(x, y):\n    x, y = x + 1, y * 2\n    return x\n";
    let b = "def f(x, y):\n    x = y + 1\n    y = x * 2\n    return x\n";
    assert_ne!(
        fp_ir(a, Lang::Python),
        fp_ir(b, Lang::Python),
        "an independent decomposition must not collide with an unrelated assign pair",
    );
}

// ---------- Rust implicit tail-return ≡ explicit return (IR path, spec §5.2) ----------
//
// A Rust function-body tail expression is an implicit `return` (`fn f() -> T { … expr }` ≡
// `{ … return expr; }`). The IR Rust frontend lowers the return-position tail to an explicit
// `Return`, recursing through tail `if`/`match` arms — so the two spellings converge, WITHOUT
// touching a `;`-terminated statement or a `let x = { … }` value block (careful scoping).

#[test]
fn rust_implicit_tail_return_converges_with_explicit_ir() {
    let implicit =
        "fn f(xs: &[i64], a: i64) -> i64 { let mut s = 0; for x in xs { s += x; } s + a }";
    let explicit =
        "fn f(xs: &[i64], a: i64) -> i64 { let mut s = 0; for x in xs { s += x; } return s + a; }";
    assert_eq!(
        fp_ir(implicit, Lang::Rust),
        fp_ir(explicit, Lang::Rust),
        "an implicit tail expression must converge with the explicit return",
    );
}

#[test]
fn rust_tail_branch_arms_converge_with_early_returns_ir() {
    // A tail `if`/`match` recurses: each arm's own tail becomes a `Return`, so
    // `{ if c { a } else { b } }` ≡ `{ if c { return a; } else { return b; } }`.
    let implicit_if = "fn f(c: bool, a: i64, b: i64) -> i64 { if c { a } else { b } }";
    let explicit_if =
        "fn f(c: bool, a: i64, b: i64) -> i64 { if c { return a; } else { return b; } }";
    assert_eq!(
        fp_ir(implicit_if, Lang::Rust),
        fp_ir(explicit_if, Lang::Rust),
        "tail if arms must converge with per-arm early returns",
    );
    let implicit_match = "fn f(n: i64) -> i64 { match n { 0 => g(), _ => h() } }";
    let explicit_match = "fn f(n: i64) -> i64 { match n { 0 => return g(), _ => return h() } }";
    assert_eq!(
        fp_ir(implicit_match, Lang::Rust),
        fp_ir(explicit_match, Lang::Rust),
        "tail match arms must converge with per-arm early returns",
    );
}

#[test]
fn rust_tail_expression_stays_distinct_from_statement_ir() {
    // Discrimination: a `;`-terminated call (`g();`, value `()`) is NOT an implicit return, so it
    // must stay distinct from the tail expression `g()` (which returns g()'s value). The `;`
    // distinction — the whole soundness of the tail-return wrapping — must survive.
    let statement = "fn f() { g(); }";
    let tail = "fn f() -> i64 { g() }";
    assert_ne!(
        fp_ir(statement, Lang::Rust),
        fp_ir(tail, Lang::Rust),
        "a `;`-terminated statement must not converge with a tail return",
    );
}
