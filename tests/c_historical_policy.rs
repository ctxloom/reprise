//! WP-K1a historical-mode policy (THE CHANGE point 8): C has no historical `LanguageProfile`
//! (CLAUDE.md — the historical per-grammar layer is retired for new languages). Selecting
//! `normalizer = "historical"` over a C file must fail LOUDLY with a clear, actionable message
//! — never silently produce wrong/empty output, and never panic through `Lang::C.profile()`
//! with an unrelated message deep in the pipeline. The lower-level panic-based guard (for
//! callers that bypass `corpus_units`) is covered by `src/unit.rs`'s own
//! `c_under_historical_normalizer_fails_loudly_not_silently` test.

use reprise::config::{Config, Normalizer};
use std::fs;
use tempfile::TempDir;

const C_SRC: &str = "int add(int a) {\n    return a + 1;\n}\n";

fn c_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("a.c"), C_SRC).unwrap();
    dir
}

#[test]
fn corpus_units_refuses_historical_normalizer_for_c_with_a_clean_error() {
    let dir = c_repo();
    let mut cfg = Config::default();
    cfg.normalize.normalizer = Normalizer::Historical;

    let result = reprise::corpus_units(dir.path(), &cfg);
    let Err(err) = result else {
        panic!("historical normalizer must be refused for a C file, not silently accepted");
    };
    let msg = err.to_string();
    assert!(msg.contains("historical"), "{msg}");
    assert!(msg.to_lowercase().contains("c"), "{msg}");
}

#[test]
fn scan_refuses_historical_normalizer_for_c_with_a_clean_error() {
    // `scan()` calls `corpus_units` for its extract half, so the same clean-error contract
    // holds at the top-level entry point a CLI invocation actually uses.
    let dir = c_repo();
    let mut cfg = Config::default();
    cfg.normalize.normalizer = Normalizer::Historical;

    let err = reprise::scan(dir.path(), &cfg).expect_err("scan must refuse, not panic or hang");
    assert!(err.to_string().contains("historical"));
}

#[test]
fn ir_normalizer_scans_c_normally() {
    // Sanity: the refusal is specific to `historical`, not a blanket "C never scans" bug.
    let dir = c_repo();
    let cfg = Config::default(); // normalizer = "ir" is the default
    let report = reprise::scan(dir.path(), &cfg).expect("ir normalizer must scan C files fine");
    assert_eq!(report.stats.files_scanned, 1);
    assert_eq!(report.stats.units_indexed, 1);
}
