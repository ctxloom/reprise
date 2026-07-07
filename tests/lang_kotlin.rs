//! Kotlin profile contract (spec §3, M3c), grammar `tree-sitter-kotlin-ng`
//! (DECISIONS.md D23). Exact-tier convergence at the fingerprint level;
//! recursion↔iteration and @Test recognition at extraction / `scan` level.

use reprise::config::{Config, Normalizer};
use reprise::lang::Lang;

mod common;
use common::{assert_no_orphan_continue, fp, is_test_unit, pair_tier, pair_tier_cfg};

// ---------- t1: comments + whitespace ----------

const T1_A: &str = "fun total(xs: List<Int>, floor: Int): Int {\n    var acc = 0\n    for (x in xs) {\n        if (x > floor) { acc += x }\n    }\n    return acc\n}\n";
const T1_B: &str = "// running total\nfun total(xs: List<Int>, floor: Int): Int {\n\n    var acc = 0 // accumulator\n\n    for (x in xs) {\n        /* skip small */\n        if (x > floor) { acc += x }\n    }\n    return acc\n}\n";

#[test]
fn kt_type1_comments_whitespace_converge() {
    assert_eq!(fp(T1_A, Lang::Kotlin), fp(T1_B, Lang::Kotlin));
}

// ---------- t2: rename + literals ----------

#[test]
fn kt_type2_rename_and_literals_converge() {
    let a = "fun label(n: Int): String {\n    val base = 10\n    if (n > 90) {\n        return \"hot\"\n    }\n    return \"cold\"\n}\n";
    let b = "fun grade(v: Int): String {\n    val seed = 42\n    if (v > 55) {\n        return \"warm\"\n    }\n    return \"chilly\"\n}\n";
    assert_eq!(fp(a, Lang::Kotlin), fp(b, Lang::Kotlin));
}

// ---------- while ↔ loop-core ----------

#[test]
fn kt_while_and_loop_core_converge() {
    let a = "fun drain(n: Int): Int {\n    var k = n\n    var s = 0\n    while (k > 0) { k -= 2; s += 1 }\n    return s\n}\n";
    let b = "fun drain(n: Int): Int {\n    var k = n\n    var s = 0\n    while (true) { if (!(k > 0)) { break }\n        k -= 2; s += 1 }\n    return s\n}\n";
    assert_eq!(fp(a, Lang::Kotlin), fp(b, Lang::Kotlin));
}

#[test]
fn kt_do_while_converges_with_core() {
    let a = "fun grow(k: Int): Int {\n    var n = k\n    do { n = n + step(n) } while (n < 100)\n    return n\n}\n";
    let b = "fun grow(k: Int): Int {\n    var n = k\n    while (true) { n = n + step(n); if (!(n < 100)) { break } }\n    return n\n}\n";
    assert_eq!(fp(a, Lang::Kotlin), fp(b, Lang::Kotlin));
}

// ---------- for ↔ index-loop (`xs.indices` and `0 until xs.size`) ----------

#[test]
fn kt_index_loop_indices_converges_with_for_in() {
    let a = "fun total(xs: List<Int>): Int {\n    var acc = 0\n    for (i in xs.indices) { acc += xs[i] }\n    return acc\n}\n";
    let b = "fun total(xs: List<Int>): Int {\n    var acc = 0\n    for (x in xs) { acc += x }\n    return acc\n}\n";
    assert_eq!(fp(a, Lang::Kotlin), fp(b, Lang::Kotlin));
}

#[test]
fn kt_index_loop_until_size_converges_with_for_in() {
    let a = "fun total(xs: List<Int>): Int {\n    var acc = 0\n    for (i in 0 until xs.size) { acc += xs[i] }\n    return acc\n}\n";
    let b = "fun total(xs: List<Int>): Int {\n    var acc = 0\n    for (x in xs) { acc += x }\n    return acc\n}\n";
    assert_eq!(fp(a, Lang::Kotlin), fp(b, Lang::Kotlin));
}

#[test]
fn kt_index_loop_near_misses_stay_distinct() {
    // Precision controls for the index-loop→foreach rewrite (mirrors the Rust/Go controls,
    // e.g. `go_counter_loop_near_misses_stay_distinct_ir`): shapes that are NOT a clean
    // index-iteration must NOT collapse to the plain foreach.
    let foreach = "fun total(xs: List<Int>): Int {\n    var acc = 0\n    for (x in xs) { acc += x }\n    return acc\n}\n";
    // (i) non-unit stride (`step 2`).
    let stride = "fun total(xs: List<Int>): Int {\n    var acc = 0\n    for (i in 0 until xs.size step 2) { acc += xs[i] }\n    return acc\n}\n";
    // (ii) the index is used for its own sake (not only as `xs[i]`).
    let use_index = "fun total(xs: List<Int>): Int {\n    var acc = 0\n    for (i in xs.indices) { acc += xs[i] + i }\n    return acc\n}\n";
    // (iii) a DIFFERENT collection is indexed (`ys[i]` while iterating over xs's range).
    let other = "fun total(xs: List<Int>, ys: List<Int>): Int {\n    var acc = 0\n    for (i in xs.indices) { acc += ys[i] }\n    return acc\n}\n";
    assert_ne!(
        fp(foreach, Lang::Kotlin),
        fp(stride, Lang::Kotlin),
        "non-unit stride must not collapse to the foreach"
    );
    assert_ne!(
        fp(foreach, Lang::Kotlin),
        fp(use_index, Lang::Kotlin),
        "index used for its own sake must not collapse to the foreach"
    );
    assert_ne!(
        fp(foreach, Lang::Kotlin),
        fp(other, Lang::Kotlin),
        "indexing a different collection must not collapse to the foreach"
    );
}

#[test]
fn kt_while_stays_distinct_from_do_while() {
    // A `while (cond) { body }` checks the guard BEFORE the body; a `do { body } while (cond)`
    // checks it AFTER (the body always runs at least once). They differ only in guard position,
    // so a normalizer that canonicalized guard position would wrongly collapse them. Identical
    // bodies — must stay distinct.
    let while_top = "fun drain(n: Int): Int {\n    var k = n\n    var s = 0\n    while (k > 0) { k -= 2; s += 1 }\n    return s\n}\n";
    let do_while = "fun drain(n: Int): Int {\n    var k = n\n    var s = 0\n    do { k -= 2; s += 1 } while (k > 0)\n    return s\n}\n";
    assert_ne!(
        fp(while_top, Lang::Kotlin),
        fp(do_while, Lang::Kotlin),
        "while (guard-first) must stay distinct from do-while (guard-last)"
    );
}

// ---------- recursion ↔ iteration (spec §5.2.2 Rev 5) ----------

const KT_ITER: &str = "fun reduce(n: Int, log: MutableList<Int>): Int {\n    while (n > 1) {\n        log.add(n)\n        n = n / 2\n    }\n    return n\n}\n";
const KT_TAIL: &str = "fun reduce(n: Int, log: MutableList<Int>): Int {\n    if (n <= 1) {\n        return n\n    }\n    log.add(n)\n    return reduce(n / 2, log)\n}\n";

#[test]
fn kt_tail_recursion_converges_with_iteration() {
    let tier = pair_tier(&[("aa.kt", KT_ITER), ("bb.kt", KT_TAIL)]);
    assert!(
        tier.as_deref()
            .is_some_and(|t| matches!(t, "exact-normalized" | "near-normalized" | "exact-region")),
        "tail recursion must converge with iteration; got {tier:?}"
    );
}

#[test]
fn kt_tail_recursion_converges_with_iteration_ir() {
    // Under `normalizer = "ir"` Kotlin has no IR frontend yet, so it falls back to the
    // historical normalizer (§9 per-language capability gate) — the recursion↔iteration
    // convergence still holds on the IR-selected path.
    let mut cfg = Config::default();
    cfg.normalize.normalizer = Normalizer::Ir;
    let tier = pair_tier_cfg(&[("aa.kt", KT_ITER), ("bb.kt", KT_TAIL)], &cfg);
    assert!(
        tier.as_deref()
            .is_some_and(|t| matches!(t, "exact-normalized" | "near-normalized" | "exact-region")),
        "tail recursion must converge with iteration on the IR path; got {tier:?}"
    );
}

// ---------- negative controls ----------

#[test]
fn kt_external_callee_change_diverges() {
    let a = "fun f(xs: List<Int>): List<Int> {\n    val out = mutableListOf<Int>()\n    for (x in xs) { out.add(parse(x)) }\n    return out\n}\n";
    let b = "fun f(xs: List<Int>): List<Int> {\n    val out = mutableListOf<Int>()\n    for (x in xs) { out.add(render(x)) }\n    return out\n}\n";
    assert_ne!(fp(a, Lang::Kotlin), fp(b, Lang::Kotlin));
}

#[test]
fn kt_unrelated_functions_do_not_converge() {
    let a = "fun parseHeaders(lines: List<String>): Map<String, String> {\n    val out = mutableMapOf<String, String>()\n    for (line in lines) {\n        if (!line.contains(\":\")) { continue }\n        val parts = line.split(\":\")\n        out[parts[0].trim().lowercase()] = parts[1].trim()\n    }\n    return out\n}\n";
    let b = "fun fibWindow(n: Int, size: Int): List<Int> {\n    var a = 0\n    var b = 1\n    val window = mutableListOf<Int>()\n    var k = n\n    while (k > 0) {\n        window.add(a)\n        val t = a + b\n        a = b\n        b = t\n        if (window.size > size) { window.removeAt(0) }\n        k -= 1\n    }\n    return window\n}\n";
    assert!(
        pair_tier(&[("aa.kt", a), ("bb.kt", b)]).is_none(),
        "unrelated functions must not converge"
    );
}

// ---------- recursion lowering must not reach into nested local functions ----------

#[test]
fn kt_nested_local_fun_self_call_is_not_a_tail_site() {
    // `return f(n - 1)` inside the nested local `fun g` is g's tail, not `f`'s.
    // Recursion lowering must not rewrite it into `n = n - 1; continue` — that emits
    // a `continue` with no enclosing loop. The unit safely bails out of lowering.
    let src = "fun f(n: Int): Int {\n    if (n <= 0) return 0\n    fun g(): Int { return f(n - 1) }\n    return f(n - 1)\n}\n";
    assert_no_orphan_continue(src, Lang::Kotlin);
}

// ---------- unit_is_test recognition (@Test annotation) ----------

#[test]
fn kt_test_recognition() {
    let annotated = "@Test\nfun checkSummarize() {\n    val r = summarize(1)\n    assertEquals(2, r)\n    assertTrue(r > 0)\n}\n";
    let plain = "fun checkSummarize(): Int {\n    val r = summarize(1)\n    return r\n}\n";
    assert!(
        is_test_unit(annotated, "Widget.kt", Lang::Kotlin),
        "@Test → test"
    );
    assert!(
        !is_test_unit(plain, "Widget.kt", Lang::Kotlin),
        "no @Test → not test"
    );
}
