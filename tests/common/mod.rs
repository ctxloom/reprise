//! Shared helpers for the language gate suites (`convergence`, `lang_go`, `lang_kotlin`,
//! `lang_typescript`), extracted from four drifting copies into one implementation. Each
//! integration-test binary pulls this in via `mod common;` and imports only the helpers it
//! uses; `#![allow(dead_code)]` covers helpers a given binary does not call (each test binary
//! compiles the whole module).
#![allow(dead_code)]

use reprise::config::{Config, Normalizer};
use reprise::lang::Lang;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

/// Fingerprint under the still-supported HISTORICAL normalizer. With the default flipped to IR,
/// pinning `fp` to `historical` keeps the historical Type-1/Type-2/loop-form contracts guarded
/// while the `fp_ir` tests cover the IR path — so both normalizers stay exercised. For a
/// language with no IR frontend (Kotlin/TS) the IR default already falls back to historical, so
/// this is the same fingerprint `Config::default()` would produce.
pub fn fp(src: &str, lang: Lang) -> u128 {
    let mut cfg = Config::default();
    cfg.normalize.normalizer = Normalizer::Historical;
    let units = reprise::units_from_source(src, lang, &cfg);
    assert_eq!(units.len(), 1, "expected exactly one unit in:\n{src}");
    units[0].fingerprint
}

/// Fingerprint under the IR normalizer (`[normalize] normalizer = "ir"`).
pub fn fp_ir(src: &str, lang: Lang) -> u128 {
    let mut cfg = Config::default();
    cfg.normalize.normalizer = Normalizer::Ir;
    let units = reprise::units_from_source(src, lang, &cfg);
    assert_eq!(units.len(), 1, "expected exactly one unit in:\n{src}");
    units[0].fingerprint
}

/// Strongest tier of any group joining files `aa` and `bb`, if any (default config).
pub fn pair_tier(sources: &[(&str, &str)]) -> Option<String> {
    pair_tier_cfg(sources, &Config::default())
}

/// As [`pair_tier`] but with an explicit config (e.g. an IR-pinned normalizer).
pub fn pair_tier_cfg(sources: &[(&str, &str)], cfg: &Config) -> Option<String> {
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

/// Whether the single unit extracted from `src` (named `filename`, language `lang`) is
/// classified as test code (spec §5.1). The `lang` parameter is the only per-suite variation
/// the four copies had — the rest of the body was identical.
pub fn is_test_unit(src: &str, filename: &str, lang: Lang) -> bool {
    let (units, _) =
        reprise::unit::extract_file_units(Path::new(filename), src, lang, &Config::default());
    assert_eq!(units.len(), 1);
    units[0].is_test
}

/// Assert the historical loop/recursion-lowered tree of the OUTER unit in `src` has no orphan
/// `continue` — one with no enclosing loop core. A nested callable resets the loop context (a
/// `continue` inside a closure never targets an outer loop), so a `continue` that lands inside
/// a nested closure/local function with no loop of its own is an invalid rewrite. This is the
/// nested-closure recursion-lowering defect: `return <self-call>` inside a nested closure is
/// the INNER callable's tail, not the outer unit's — lowering it emits `continue` outside any
/// loop. Runs the pre-abstraction passes so `continue` is still recognizable by kind
/// (`continue_statement`, TS/Python) or by raw label (`identifier "continue"`, Kotlin).
pub fn assert_no_orphan_continue(src: &str, lang: Lang) {
    use reprise::tree::{Label, NormNode};

    fn node_count(n: &NormNode) -> usize {
        1 + n.children.iter().map(node_count).sum::<usize>()
    }
    fn is_continue(n: &NormNode, li: &reprise::intern::LabelInterner) -> bool {
        if n.kind.as_str() != "identifier" {
            return n.kind.as_str() == "continue_statement";
        }
        match &n.label {
            Some(Label::Raw(t)) => t.as_ref() == "continue",
            Some(Label::External(t)) => li.resolve(*t) == "continue",
            _ => false,
        }
    }
    fn is_callable(kind: &str) -> bool {
        matches!(
            kind,
            "arrow_function"
                | "function_expression"
                | "function_declaration"
                | "method_definition"
                | "lambda_literal"
                | "anonymous_function"
                | "function_definition"
                | "lambda"
        )
    }
    fn is_loop(kind: &str) -> bool {
        matches!(
            kind,
            "while_statement" | "for_statement" | "for_in_statement"
        )
    }
    fn walk(n: &NormNode, loop_depth: u32, li: &reprise::intern::LabelInterner) {
        if is_callable(n.kind.as_str()) {
            for c in &n.children {
                walk(c, 0, li); // a nested callable resets the loop context
            }
            return;
        }
        if is_continue(n, li) {
            assert!(
                loop_depth > 0,
                "`continue` outside any loop — recursion lowering descended into a nested closure"
            );
        }
        let depth = if is_loop(n.kind.as_str()) {
            loop_depth + 1
        } else {
            loop_depth
        };
        for c in &n.children {
            walk(c, depth, li);
        }
    }

    let raw = reprise::normalize::raw_units_from_source(src, lang);
    assert!(!raw.is_empty(), "expected at least one unit in:\n{src}");
    // The outer unit contains the nested callable, so it has the most nodes (a nested `def`/
    // `fun`/`const g = …` is also extracted as its own unit).
    let outer = raw
        .into_iter()
        .map(|(_, t)| t)
        .max_by_key(node_count)
        .unwrap();
    let profile = lang.profile();
    let label_interner = reprise::intern::LabelInterner::new();
    let tree = profile.lower_recursion(outer);
    let tree = profile.rewrite_iteration(tree, &label_interner);
    let tree = profile.lower_loops(tree, &label_interner);
    let tree = profile.normalize_loop_exit(tree);
    walk(&tree, 0, &label_interner);
}
