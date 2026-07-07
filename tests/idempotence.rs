//! Spec §5.2 property tests: normalization is deterministic and idempotent
//! (`normalize(normalize(t)) == normalize(t)`), and source spans survive.
//! Runs over the mutation-benchmark seeds plus inline snippets.

use reprise::config::Config;
use reprise::frontend::{extract_ir_units, has_ir_frontend, run_passes};
use reprise::ir::TransformLog;
use reprise::lang::Lang;
use reprise::normalize::{apply_passes, raw_units_from_source};
use reprise::tree::NormNode;
use std::fs;
use std::path::{Path, PathBuf};

fn seed_sources() -> Vec<(String, Lang)> {
    let mut out = Vec::new();
    for (dir, lang, ext) in [
        ("rust", Lang::Rust, "rs"),
        ("python", Lang::Python, "py"),
        ("typescript", Lang::TypeScript, "ts"),
        ("go", Lang::Go, "go"),
        ("kotlin", Lang::Kotlin, "kt"),
    ] {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("benches/mutations/seeds")
            .join(dir);
        // Track a per-directory count: a single `out.len() >= 4` floor lets an *emptied*
        // language dir slip through silently (four Rust seeds would satisfy it while Go
        // has none). Assert every language contributes at least one seed instead.
        let mut count = 0;
        for entry in fs::read_dir(&root).expect("seed dir") {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) == Some(ext) {
                out.push((fs::read_to_string(&path).unwrap(), lang));
                count += 1;
            }
        }
        assert!(
            count >= 1,
            "seed dir `{dir}` has no `.{ext}` seeds ({lang:?})"
        );
    }
    out
}

fn inline_sources() -> Vec<(String, Lang)> {
    vec![
        (
            "fn f(xs: &[i64]) -> i64 {\n    let mut acc = 0;\n    while acc < 10 {\n        acc += 1;\n    }\n    for x in xs {\n        acc += x;\n    }\n    acc\n}\n".to_string(),
            Lang::Rust,
        ),
        (
            "def f(xs):\n    acc = 0\n    while acc < 10:\n        acc += 1\n    for x in xs:\n        acc += x\n    return acc\n".to_string(),
            Lang::Python,
        ),
    ]
}

/// The seed + inline corpus restricted to languages with an IR frontend (Rust/Python/Go);
/// TypeScript/Kotlin have none and fall back to the historical normalizer under `ir`.
fn ir_sources() -> Vec<(String, Lang)> {
    seed_sources()
        .into_iter()
        .chain(inline_sources())
        .filter(|(_, lang)| has_ir_frontend(*lang))
        .collect()
}

/// Spec §4 hard requirement (PLAN.md §4): every normalized node keeps a byte span that maps
/// back into the source. A plain in-bounds check accepts a degenerate `(0,0)` span, so a
/// synthesized node that lost its provenance passes trivially. Tighten: every node stays in
/// bounds, and every node **below the unit root** must additionally carry real provenance —
/// either a non-degenerate span (`start < end`) OR a span nested inside its parent's (a
/// synthetic node legitimately inherits its parent's source region, and loop-protocol nodes
/// take the whole loop's span, which is non-degenerate). A `(0,0)` span on a deep node is
/// neither, so it is caught as provenance loss.
fn assert_spans_survive(root: &NormNode, src_len: usize, name: &str, lang: Lang) {
    let mut stack: Vec<(&NormNode, Option<(u32, u32)>)> = vec![(root, None)];
    while let Some((node, parent)) = stack.pop() {
        let (start, end) = node.span;
        assert!(
            (start as usize) <= src_len && (end as usize) <= src_len && start <= end,
            "node span ({start},{end}) escapes source (len {src_len}) in unit `{name}` ({lang:?})"
        );
        if let Some((ps, pe)) = parent {
            let non_degenerate = start < end;
            let within_parent = start >= ps && end <= pe;
            assert!(
                non_degenerate || within_parent,
                "degenerate span ({start},{end}) not nested in parent ({ps},{pe}) — \
                 provenance lost for a node in unit `{name}` ({lang:?})"
            );
        }
        for c in &node.children {
            stack.push((c, Some(node.span)));
        }
    }
}

#[test]
fn normalization_is_idempotent() {
    let cfg = Config::default();
    for (src, lang) in seed_sources().into_iter().chain(inline_sources()) {
        for (name, raw) in raw_units_from_source(&src, lang) {
            let once = apply_passes(raw.clone(), lang, &cfg);
            let twice = apply_passes(once.clone(), lang, &cfg);
            assert_eq!(
                once, twice,
                "normalize not idempotent for unit `{name}` ({lang:?})"
            );
        }
    }
}

#[test]
fn normalization_is_deterministic() {
    let cfg = Config::default();
    for (src, lang) in seed_sources().into_iter().chain(inline_sources()) {
        let a: Vec<_> = raw_units_from_source(&src, lang)
            .into_iter()
            .map(|(_, t)| apply_passes(t, lang, &cfg))
            .collect();
        let b: Vec<_> = raw_units_from_source(&src, lang)
            .into_iter()
            .map(|(_, t)| apply_passes(t, lang, &cfg))
            .collect();
        assert_eq!(a, b, "normalize not deterministic ({lang:?})");
    }
}

#[test]
fn spans_survive_normalization() {
    // Spec §4 hard requirement: every normalized node maps back into the source.
    let cfg = Config::default();
    for (src, lang) in seed_sources().into_iter().chain(inline_sources()) {
        for (name, raw) in raw_units_from_source(&src, lang) {
            let normed = apply_passes(raw, lang, &cfg);
            assert_spans_survive(&normed, src.len(), &name, lang);
        }
    }
}

#[test]
fn ir_loop_exit_folds_to_fixpoint_and_is_idempotent() {
    // vile-apron: `loop { loop {…} break } return E` must fold the outer break AND the nested
    // break the fold exposes in ONE pass (a fixpoint), so re-running the loop-exit pass on its
    // own output changes NOTHING, and the nested break-form converges with the early-return form.
    use reprise::frontend::lower_rust_source;
    use reprise::ir::{normalize_loop_exit, to_sexpr};
    let (ir, _) = lower_rust_source(
        "fn f() -> i32 { loop { loop { h(); if c() { break; } } break; } return 5; }",
    )
    .unwrap();
    let once = normalize_loop_exit(ir);
    let twice = normalize_loop_exit(once.clone());
    assert_eq!(
        to_sexpr(&once),
        to_sexpr(&twice),
        "the loop-exit fold must reach a fixpoint in one pass (idempotent tree shape)"
    );
    let (hand, _) =
        lower_rust_source("fn f() -> i32 { loop { loop { h(); if c() { return 5; } } } }").unwrap();
    assert_eq!(
        to_sexpr(&once),
        to_sexpr(&normalize_loop_exit(hand)),
        "the nested break-then-return must converge with the hand-written early-return form"
    );
}

// ---------- IR-path property tests (the now-DEFAULT normalizer, spec §5.2) ----------
//
// The historical property tests above run `apply_passes`; these are their IR-path analogs
// over the SAME corpus, for the IR-frontend languages (Rust/Python/Go). Each asserts a
// property that must already hold — a failure is a real non-idempotence/nondeterminism/
// span bug, to be reported rather than masked.

#[test]
fn ir_normalization_is_idempotent() {
    // The IR pass pipeline must be a fixpoint: re-applying the passes to an already-normalized
    // tree changes NOTHING (tree-shape fixpoint). We assert the applied TREE is identical on a
    // second run — NOT that the detectors return an empty edit-set, because some passes
    // legitimately emit no-op-detectable edits on canonical trees (comm_sort's
    // identity-permutation skip still leaves other no-op-safe edits possible). Tree identity is
    // the robust invariant.
    let path = Path::new("seed");
    for (src, lang) in ir_sources() {
        for unit in extract_ir_units(&src, lang, path) {
            // `unit.tree` is already `run_passes(raw)`; re-run the passes on it.
            let once = unit.tree;
            let mut log = TransformLog::disabled();
            let twice = run_passes(once.clone(), lang, &mut log);
            assert_eq!(
                once, twice,
                "IR passes not idempotent for unit `{}` ({lang:?})",
                unit.name
            );
        }
    }
}

#[test]
fn ir_normalization_is_deterministic() {
    // Lowering + the pass pipeline are pure: the same source lowered twice yields byte-identical
    // IR trees (same fingerprint).
    let path = Path::new("seed");
    for (src, lang) in ir_sources() {
        let a = extract_ir_units(&src, lang, path);
        let b = extract_ir_units(&src, lang, path);
        assert_eq!(
            a.len(),
            b.len(),
            "IR unit count not deterministic ({lang:?})"
        );
        for (ua, ub) in a.iter().zip(b.iter()) {
            assert_eq!(
                ua.tree, ub.tree,
                "IR lowering not deterministic for unit `{}` ({lang:?})",
                ua.name
            );
        }
    }
}

#[test]
fn ir_spans_survive_normalization() {
    // Spec §4 on the IR path: every node of the IR-lowered + normalized tree — including the
    // synthesized loop-protocol / break-guard / temp nodes — carries a span that maps back
    // into the source (same invariant as the historical span test, via `assert_spans_survive`).
    let path = Path::new("seed");
    for (src, lang) in ir_sources() {
        for unit in extract_ir_units(&src, lang, path) {
            assert_spans_survive(&unit.tree, src.len(), &unit.name, lang);
        }
    }
}

#[test]
fn units_meet_phase1_floor() {
    // Guard for the benchmark itself: seeds must clear `min_unit_tokens`,
    // otherwise mutation recall silently tests nothing (DECISIONS.md D6). Pinned to the
    // historical normalizer: it asserts the historical floor (40) against historical-path
    // token counts. The IR default uses a lower, compaction-scaled floor
    // (`min_unit_tokens_ir` = 33) and more compact counts (e.g. `reduce_pair` is 36 IR
    // tokens — above the IR floor but below 40); the IR seed-floor guard rides on
    // `just bench-mutations-ir`.
    let mut cfg = Config::default();
    cfg.normalize.normalizer = "historical".into();
    for (src, lang) in seed_sources() {
        let units = reprise::units_from_source(&src, lang, &cfg);
        assert_eq!(units.len(), 1);
        assert!(
            units[0].token_count >= cfg.thresholds.min_unit_tokens,
            "seed unit `{}` has {} tokens, below floor {}",
            units[0].name,
            units[0].token_count,
            cfg.thresholds.min_unit_tokens
        );
    }
}
