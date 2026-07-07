//! Untrusted-input robustness (DoS): a pathologically deep CST — a several-thousand-term
//! left-associative operator chain, as a generated/minified file produces — must NOT
//! overflow the extraction stack and SIGSEGV the whole scan. The depth guard
//! (`normalize::MAX_EXTRACTION_DEPTH`) truncates the over-deep subtree and flags the unit
//! `parse_degraded`, so the scan COMPLETES and the truncation is visible, never a silent
//! drop (spec §5.1/§12).
//!
//! Reproduction note: with the guard REMOVED, the recursive `convert` / `lower_node` descent
//! of a chain this deep exhausts the worker stack and aborts the *process* (SIGSEGV) — an
//! uncatchable crash that takes down `cargo test` itself. That process-abort IS the red
//! signal; these tests assert the post-fix contract (completes + flagged) and, by running the
//! extraction on a thread with rayon's default 2 MiB worker stack, prove the *guarded* path
//! fits the production stack.

use reprise::config::{Config, Normalizer};
use reprise::lang::Lang;
use std::fs;
use tempfile::TempDir;

/// rayon's default worker stack (`src/lib.rs` / `src/matchtree.rs` `par_iter` paths run
/// extraction here). Mirroring it makes these tests a faithful regression guard: if the
/// guarded, cap-deep recursion did NOT fit 2 MiB, the spawned thread would overflow and
/// abort — so a green run proves the cap is safe for the real scan.
const WORKER_STACK: usize = 2 * 1024 * 1024;

fn on_worker_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(WORKER_STACK)
        .spawn(f)
        .expect("spawn worker-stack thread")
        .join()
        .expect("worker-stack thread completed without a stack overflow")
}

/// A Rust unit whose body is a left-associative `1 + 1 + … + 1` chain of `terms` operands —
/// tree-sitter parses it into a ~`terms`-deep left-nested `binary_expression`. Formatted one
/// term per line so no line reaches the 512-char minified signature: this file is NOT screened
/// out by `walk::is_generated`, so it actually reaches extraction (the case that screen misses).
fn deep_rust_chain(terms: usize) -> String {
    let mut s = String::with_capacity(terms * 8 + 32);
    s.push_str("fn deep() -> i64 {\n    1\n");
    for _ in 1..terms {
        s.push_str("    + 1\n");
    }
    s.push_str("}\n");
    s
}

/// A Python analog (nested-list literals `[[[…]]]`): exercises the IR frontend's `lower_node`
/// recursion on a different, deeply-nested construct.
fn deep_python_nest(depth: usize) -> String {
    let mut s = String::from("def deep():\n    return ");
    s.push_str(&"[".repeat(depth));
    s.push('0');
    s.push_str(&"]".repeat(depth));
    s.push('\n');
    s
}

// Depth far past MAX_EXTRACTION_DEPTH (150) and deep enough that the UNGUARDED descent
// overflows a 2 MiB stack — the pathological input the guard exists to survive.
const DEEP: usize = 3000;

#[test]
fn deep_operator_chain_completes_and_is_flagged_ir() {
    // Default config == the IR normalizer (the shipping path).
    let src = deep_rust_chain(DEEP);
    let units =
        on_worker_stack(move || reprise::units_from_source(&src, Lang::Rust, &Config::default()));
    assert_eq!(
        units.len(),
        1,
        "the deep unit must still be extracted, not dropped"
    );
    assert!(
        units[0].parse_degraded,
        "a depth-truncated unit must be flagged parse_degraded (visible, never silent)",
    );
}

#[test]
fn deep_operator_chain_completes_and_is_flagged_historical() {
    // The still-selectable historical normalizer runs the `normalize::convert` descent.
    let src = deep_rust_chain(DEEP);
    let units = on_worker_stack(move || {
        let mut cfg = Config::default();
        cfg.normalize.normalizer = Normalizer::Historical;
        reprise::units_from_source(&src, Lang::Rust, &cfg)
    });
    assert_eq!(units.len(), 1);
    assert!(
        units[0].parse_degraded,
        "historical-path depth truncation must also flag parse_degraded",
    );
}

#[test]
fn deep_nested_python_literal_completes_and_is_flagged() {
    let src = deep_python_nest(DEEP);
    let units =
        on_worker_stack(move || reprise::units_from_source(&src, Lang::Python, &Config::default()));
    assert_eq!(units.len(), 1);
    assert!(
        units[0].parse_degraded,
        "deep nested literal must be flagged"
    );
}

#[test]
fn full_scan_survives_a_pathologically_deep_file() {
    // The end-to-end DoS assertion: a whole-repo scan over a directory containing the
    // pathological file must COMPLETE (extraction runs on real rayon workers) and surface the
    // truncated unit in the stats, rather than aborting the entire scan.
    let dir = TempDir::new().expect("tempdir");
    fs::write(dir.path().join("deep.rs"), deep_rust_chain(DEEP)).expect("write deep file");
    fs::write(
        dir.path().join("normal.rs"),
        "fn add(a: i64, b: i64) -> i64 { a + b }\n",
    )
    .expect("write normal file");

    let root = dir.path().to_path_buf();
    let report =
        on_worker_stack(move || reprise::scan(&root, &Config::default()).expect("scan ok"));

    assert!(
        report.stats.files_scanned >= 2,
        "scan must complete over the whole directory, not abort on the deep file",
    );
    assert!(
        report.stats.parse_degraded_units >= 1,
        "the truncated deep unit must be counted as parse-degraded in the scan summary",
    );
}
