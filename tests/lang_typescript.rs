//! TypeScript / TSX profile contract (spec §3, M3c). Exact-tier convergence
//! (Type-1/Type-2, loop-form, index-loop, arrow desugaring) is asserted at the
//! fingerprint level; recursion↔iteration and test policy at `scan` level.

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

/// Strongest tier of any group joining files `aa` and `bb`, if any.
fn pair_tier(sources: &[(&str, &str)]) -> Option<String> {
    pair_tier_cfg(sources, &Config::default())
}

fn pair_tier_cfg(sources: &[(&str, &str)], cfg: &Config) -> Option<String> {
    let dir = TempDir::new().unwrap();
    for (name, src) in sources {
        fs::write(dir.path().join(name), src).unwrap();
    }
    let report = reprise::scan(dir.path(), cfg).unwrap();
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
    let cfg = Config::default();
    let (units, _) =
        reprise::unit::extract_file_units(Path::new(filename), src, Lang::TypeScript, &cfg);
    assert_eq!(units.len(), 1);
    units[0].is_test
}

// ---------- t1: comments + whitespace ----------

const T1_A: &str = "function total(xs: number[], floor: number): number {\n  let acc = 0;\n  for (const x of xs) {\n    if (x > floor) { acc += x; }\n  }\n  return acc;\n}\n";
const T1_B: &str = "// a running total\nfunction total(xs: number[], floor: number): number {\n\n  let acc = 0; // accumulator\n\n  for (const x of xs) {\n    /* skip small */\n    if (x > floor) { acc += x; }\n  }\n  return acc;\n}\n";

#[test]
fn ts_type1_comments_whitespace_converge() {
    assert_eq!(fp(T1_A, Lang::TypeScript), fp(T1_B, Lang::TypeScript));
}

// ---------- t2: rename + literals ----------

#[test]
fn ts_type2_rename_and_literals_converge() {
    let a = "function label(n: number): string {\n  const base = 10;\n  if (n > 90) { return \"hot\"; }\n  return \"cold\";\n}\n";
    let b = "function grade(v: number): string {\n  const seed = 42;\n  if (v > 55) { return \"warm\"; }\n  return \"chilly\";\n}\n";
    assert_eq!(fp(a, Lang::TypeScript), fp(b, Lang::TypeScript));
}

// ---------- while ↔ loop-core ----------

#[test]
fn ts_while_and_loop_core_converge() {
    let a = "function drain(n: number): number {\n  let k = n;\n  let s = 0;\n  while (k > 0) { k -= 2; s += 1; }\n  return s;\n}\n";
    let b = "function drain(n: number): number {\n  let k = n;\n  let s = 0;\n  while (true) { if (!(k > 0)) { break; } k -= 2; s += 1; }\n  return s;\n}\n";
    assert_eq!(fp(a, Lang::TypeScript), fp(b, Lang::TypeScript));
}

#[test]
fn ts_do_while_converges_with_core() {
    let a = "function grow(k: number): number {\n  let n = k;\n  do { n = n + step(n); } while (n < 100);\n  return n;\n}\n";
    let b = "function grow(k: number): number {\n  let n = k;\n  while (true) { n = n + step(n); if (!(n < 100)) { break; } }\n  return n;\n}\n";
    assert_eq!(fp(a, Lang::TypeScript), fp(b, Lang::TypeScript));
}

// ---------- for ↔ index-loop (iteration-protocol rewrite) ----------

#[test]
fn ts_index_loop_converges_with_for_of() {
    let a = "function total(xs: number[]): number {\n  let acc = 0;\n  for (let i = 0; i < xs.length; i++) { acc += xs[i]; }\n  return acc;\n}\n";
    let b = "function total(xs: number[]): number {\n  let acc = 0;\n  for (const x of xs) { acc += x; }\n  return acc;\n}\n";
    assert_eq!(fp(a, Lang::TypeScript), fp(b, Lang::TypeScript));
}

// ---------- arrow → function desugaring (spec §5.2.3) ----------

#[test]
fn ts_arrow_and_function_expression_converge() {
    // Arrow and function-expression surface of the same logic normalize alike.
    // Both forms are now extracted as first-class units (D43), so assert on the
    // bindings directly rather than on a wrapping function.
    let a = "const inc = (x: number): number => { return x + 1; };\n";
    let b = "const inc = function (x: number): number { return x + 1; };\n";
    assert_eq!(fp(a, Lang::TypeScript), fp(b, Lang::TypeScript));
}

// ---------- recursion ↔ iteration (spec §5.2.2 Rev 5) ----------

const TS_ITER: &str = "function reducePair(a: number, b: number, log: string[]): number {\n  while (b !== 0) {\n    log.push(\"step \" + a + \" \" + b);\n    const q = Math.floor(a / b);\n    log.push(\"quot \" + q);\n    [a, b] = [b, a % b];\n  }\n  log.push(\"done\");\n  return a;\n}\n";
const TS_TAIL: &str = "function reducePair(a: number, b: number, log: string[]): number {\n  if (b === 0) {\n    log.push(\"done\");\n    return a;\n  }\n  log.push(\"step \" + a + \" \" + b);\n  const q = Math.floor(a / b);\n  log.push(\"quot \" + q);\n  return reducePair(b, a % b, log);\n}\n";

#[test]
fn ts_tail_recursion_converges_with_iteration() {
    let tier = pair_tier(&[("aa.ts", TS_ITER), ("bb.ts", TS_TAIL)]);
    assert!(
        tier.as_deref()
            .is_some_and(|t| matches!(t, "exact-normalized" | "near-normalized" | "exact-region")),
        "tail recursion must converge with iteration; got {tier:?}"
    );
}

#[test]
fn ts_tail_recursion_converges_with_iteration_ir() {
    // TypeScript has no IR frontend yet, so `normalizer = "ir"` falls back to the historical
    // normalizer (§9 capability gate) — the recursion↔iteration convergence holds on the
    // IR-selected path too.
    let mut cfg = Config::default();
    cfg.normalize.normalizer = "ir".into();
    let tier = pair_tier_cfg(&[("aa.ts", TS_ITER), ("bb.ts", TS_TAIL)], &cfg);
    assert!(
        tier.as_deref()
            .is_some_and(|t| matches!(t, "exact-normalized" | "near-normalized" | "exact-region")),
        "tail recursion must converge with iteration on the IR path; got {tier:?}"
    );
}

// ---------- negative controls ----------

#[test]
fn ts_external_callee_change_diverges() {
    let a = "function f(xs: number[]): number[] {\n  const out: number[] = [];\n  for (const x of xs) { out.push(parse(x)); }\n  return out;\n}\n";
    let b = "function f(xs: number[]): number[] {\n  const out: number[] = [];\n  for (const x of xs) { out.push(render(x)); }\n  return out;\n}\n";
    assert_ne!(fp(a, Lang::TypeScript), fp(b, Lang::TypeScript));
}

#[test]
fn ts_unrelated_functions_do_not_converge() {
    let a = "function parseHeaders(lines: string[]): Map<string, string> {\n  const out = new Map<string, string>();\n  for (const line of lines) {\n    if (!line.includes(\":\")) { continue; }\n    const [key, val] = line.split(\":\");\n    out.set(key.trim().toLowerCase(), val.trim());\n  }\n  return out;\n}\n";
    let b = "function fibWindow(n: number, size: number): number[] {\n  let a = 0;\n  let b = 1;\n  const window: number[] = [];\n  while (n > 0) {\n    window.push(a);\n    [a, b] = [b, a + b];\n    if (window.length > size) { window.shift(); }\n    n -= 1;\n  }\n  return window;\n}\n";
    assert!(
        pair_tier(&[("aa.ts", a), ("bb.ts", b)]).is_none(),
        "unrelated functions must not converge"
    );
}

// ---------- const-arrow / function-expression extraction (D43) ----------

/// A `const f = () => …` binding is extracted as exactly one unit, and inline
/// callbacks in its body (`.map(x => …)`) are NOT — decl-only, matching
/// Rust/Python. Previously the whole binding was invisible (D25).
#[test]
fn const_arrow_binding_is_one_unit_inline_callbacks_are_not() {
    let src = "const process = (xs: number[]): number[] => {\n  \
        return xs.map((x) => x + 1).filter((x) => x > 0);\n};\n";
    let units = reprise::units_from_source(src, Lang::TypeScript, &Config::default());
    assert_eq!(
        units.len(),
        1,
        "only the named const binding is a unit; the .map/.filter arrows are \
         inline callbacks, not declarations (got {})",
        units.len()
    );
}

/// `const f = function () {…}` (function-expression form) is also extracted.
#[test]
fn const_function_expression_is_extracted() {
    let src = "const handler = function (n: number): number {\n  return n * n + 1;\n};\n";
    let units = reprise::units_from_source(src, Lang::TypeScript, &Config::default());
    assert_eq!(
        units.len(),
        1,
        "const = function () {{}} should extract one unit"
    );
}

/// Two duplicated const-arrow functions (renamed locals + different binding
/// name) converge into a group. This is the D25 gap closed: before D43 neither
/// was extracted, so they could never match.
#[test]
fn duplicated_const_arrows_converge() {
    let a = "export const summarize = (scores: number[], threshold: number): string[] => {\n  \
        const out: string[] = [];\n  \
        let total = 0;\n  \
        for (const s of scores) {\n    \
            if (s < threshold) {\n      continue;\n    }\n    \
            total += s;\n    \
            out.push(s >= 90 ? \"high\" : \"low\");\n  }\n  \
        return out;\n};\n";
    let b = "export const collect = (values: number[], cutoff: number): string[] => {\n  \
        const acc: string[] = [];\n  \
        let sum = 0;\n  \
        for (const v of values) {\n    \
            if (v < cutoff) {\n      continue;\n    }\n    \
            sum += v;\n    \
            acc.push(v >= 90 ? \"high\" : \"low\");\n  }\n  \
        return acc;\n};\n";
    assert!(
        pair_tier(&[("aa.ts", a), ("bb.ts", b)]).is_some(),
        "two duplicated const-arrow functions should form a group"
    );
}

// ---------- unit_is_test recognition (spec §5.1) ----------

#[test]
fn ts_test_recognition() {
    let f = "function checkThing(): void {\n  const r = compute(2);\n  expect(r).toBe(3);\n  expect(r).toBeGreaterThan(1);\n}\n";
    assert!(
        is_test_unit(f, "widget.test.ts"),
        ".test.ts filename → test"
    );
    assert!(
        is_test_unit(f, "widget.spec.ts"),
        ".spec.ts filename → test"
    );
    assert!(!is_test_unit(f, "widget.ts"), "plain file → not test");
}
