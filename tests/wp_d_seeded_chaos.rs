//! WP-D (raw-trees elimination, session `woozy-uncut-comic`) step 6: the
//! seeded-chaos identity test. `store.rs`'s own module-doc invariant —
//! "byte-identity constrains the PATHS, not the TIMING" — made concrete for the
//! raw-tree memo: the FULL `scan()` production path, run twice over the SAME
//! corpus with the memory gate forced to opposite states (`[memory] force_gate`,
//! never a `REPRISE_MEMORY_FORCE_GATE` env var — that's process-global and unsafe
//! under `cargo test`'s parallel test execution; the config field is the
//! thread-safe per-`Config` equivalent `src/memory.rs`'s `force_override` reads
//! first), must produce byte-identical reports. Forcing the gate ALWAYS trips
//! both the near-tier tree-pack spill AND (since this WP) caps the raw-tree
//! memo's LRU at the measured flat floor (`memory::RAW_TREE_MEMO_BYTES`,
//! `memory::raw_tree_memo_bytes`) — so this exercises the REAL, gate-driven
//! budget wiring `scan_source` uses in production, not a hand-rolled test-only
//! budget. (A deeper, unit-level version of the same claim — genuine forced
//! eviction via a 1-byte memo budget, `Expansion`-level equality — lives in
//! `src/inline.rs`'s `wp_d_memo_tests` module; this test complements it at full
//! pipeline granularity.)

use reprise::config::Config;
use std::fs;
use tempfile::TempDir;

/// Caller/callee pairs across many files plus a shared helper file (real
/// inline traffic) and a near-duplicate pair (non-inline-tier findings too) —
/// the same shape `tests/wp_d_git_source_cache_off.rs` uses, so both the
/// inline-assisted and near/exact tiers have real content to compare.
fn fixture_files() -> Vec<(String, String)> {
    let mut files = Vec::new();
    let mut shared = String::new();
    for i in 0..10 {
        shared.push_str(&format!(
            "fn helper_{i}(x: i64) -> i64 {{\n    let v = x + {i};\n    v * 2 - 1\n}}\n\n"
        ));
        files.push((
            format!("caller_{i}.rs"),
            format!(
                "fn caller_{i}(x: i64) -> i64 {{\n    let a = helper_{i}(x);\n    a + helper_{i}(a)\n}}\n"
            ),
        ));
    }
    files.push(("shared.rs".to_string(), shared));
    files.push((
        "dup_a.rs".to_string(),
        "fn compute_total(items: &[i64]) -> i64 {\n    let mut acc = 0;\n    for it in items {\n        acc += it * 2;\n    }\n    acc\n}\n".to_string(),
    ));
    files.push((
        "dup_b.rs".to_string(),
        "fn sum_doubled(values: &[i64]) -> i64 {\n    let mut total = 0;\n    for v in values {\n        total += v * 2;\n    }\n    total\n}\n".to_string(),
    ));
    files
}

fn strip_volatile(report: &mut reprise::ScanReport) {
    report.stats.duration_ms = 0;
    report.stats.phase_ms.clear();
    report.stats.cache_hits = 0;
    report.stats.cache_misses = 0;
    report.stats.memory_peak_bytes = 0;
    report.stats.memory_gate_rss_bytes = 0;
    report.stats.memory_extract_peak_bytes = 0;
    report.stats.memory_budget_bytes = 0;
    report.stats.memory_estimated_bytes = 0;
    report.stats.memory_gate_tripped = false;
    report.stats.memory_spilled_trees = 0;
    report.stats.memory_pack_bytes = 0;
    report.stats.memory_lru_hits = 0;
    report.stats.memory_lru_misses = 0;
    report.stats.raw_tree_memo_hits = 0;
    report.stats.raw_tree_memo_misses = 0;
}

#[test]
fn scan_is_byte_identical_whether_the_memory_gate_forces_spill_or_not() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    for (name, src) in fixture_files() {
        fs::write(root.join(name), src).unwrap();
    }

    let mut cfg_never = Config::default();
    cfg_never.cache.enabled = false;
    cfg_never.memory.force_gate = "never".to_string();
    let mut resident = reprise::scan(root, &cfg_never).expect("gate-never scan");
    assert!(
        resident.stats.calls_inlined > 0,
        "fixture sanity: the inliner must actually splice something \
         (stats: {:?})",
        resident.stats
    );
    assert!(
        !resident.stats.memory_gate_tripped,
        "gate must NOT be tripped under force_gate=never"
    );

    let mut cfg_always = Config::default();
    cfg_always.cache.enabled = false;
    cfg_always.memory.force_gate = "always".to_string();
    let mut spilled = reprise::scan(root, &cfg_always).expect("gate-always scan");
    assert!(
        spilled.stats.memory_gate_tripped,
        "gate must BE tripped under force_gate=always"
    );

    strip_volatile(&mut resident);
    strip_volatile(&mut spilled);
    assert_eq!(
        serde_json::to_string(&resident).unwrap(),
        serde_json::to_string(&spilled).unwrap(),
        "forcing the memory gate (tree-pack spill AND the raw-tree memo's \
         pressure-aware budget) must never change scan OUTPUT, only where the \
         bytes momentarily live"
    );
}

/// The memo's hit/miss counters (step 6's observability addition) must
/// actually MOVE — a zero-everywhere stat would mean the wiring silently
/// never ran, which the byte-identity test above cannot catch by itself
/// (identical zeros are still identical).
#[test]
fn raw_tree_memo_stats_are_populated_when_inlining_runs() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    for (name, src) in fixture_files() {
        fs::write(root.join(name), src).unwrap();
    }
    let mut cfg = Config::default();
    cfg.cache.enabled = false;
    let report = reprise::scan(root, &cfg).expect("scan");
    assert!(
        report.stats.raw_tree_memo_misses > 0,
        "the memo must have rehydrated at least one file's raw trees: {:?}",
        report.stats
    );
}

#[test]
fn raw_tree_memo_stats_are_zero_when_inlining_is_disabled() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    for (name, src) in fixture_files() {
        fs::write(root.join(name), src).unwrap();
    }
    let mut cfg = Config::default();
    cfg.cache.enabled = false;
    cfg.inline.enabled = false;
    let report = reprise::scan(root, &cfg).expect("scan");
    assert_eq!(report.stats.raw_tree_memo_hits, 0);
    assert_eq!(report.stats.raw_tree_memo_misses, 0);
}
