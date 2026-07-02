//! Kotlin profile contract (spec §3, M3c), grammar `tree-sitter-kotlin-ng`
//! (DECISIONS.md D23). Exact-tier convergence at the fingerprint level;
//! recursion↔iteration and @Test recognition at extraction / `scan` level.

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
    let (units, _) = reprise::unit::extract_file_units(
        Path::new(filename),
        src,
        Lang::Kotlin,
        &Config::default(),
    );
    assert_eq!(units.len(), 1);
    units[0].is_test
}

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

// ---------- unit_is_test recognition (@Test annotation) ----------

#[test]
fn kt_test_recognition() {
    let annotated = "@Test\nfun checkSummarize() {\n    val r = summarize(1)\n    assertEquals(2, r)\n    assertTrue(r > 0)\n}\n";
    let plain = "fun checkSummarize(): Int {\n    val r = summarize(1)\n    return r\n}\n";
    assert!(is_test_unit(annotated, "Widget.kt"), "@Test → test");
    assert!(!is_test_unit(plain, "Widget.kt"), "no @Test → not test");
}
