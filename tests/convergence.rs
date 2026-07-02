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
