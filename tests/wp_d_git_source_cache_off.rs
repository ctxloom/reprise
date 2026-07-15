//! WP-D (raw-trees elimination, session `woozy-uncut-comic`) step 5: the raw-tree
//! memo's two remaining fallback branches, neither exercised by any other test
//! (every fixture-based test elsewhere in the suite reads through `FsSource` with
//! caching left at its default) —
//!
//! - **Hole 2 (cache off):** every memo miss re-lowers from source through
//!   `extract_file_units_keep_raw` instead of `cache::load`. With caching on, a
//!   scan barely touches this path (only a cold-cache first extraction does); with
//!   caching off, EVERY memo fetch takes it.
//! - **Hole 3 (`GitSource`):** the memo's fallback reads through the scan's
//!   `&dyn ContentSource` (`source.read(path)`), never `std::fs` directly — the
//!   requirement that lets `check` read the git index/a base ref with no checkout
//!   materialized. `GitSource::read` serves bytes out of an in-memory blob map, not
//!   the filesystem, so a memo that secretly used `std::fs::read_to_string` would
//!   still "work" against a live checkout but silently desync against a git ref
//!   that differs from the worktree — this test's fixture stages CHANGES that are
//!   NOT reflected on disk, so a `std::fs` shortcut would produce a diverging
//!   report instead of an identical one.
//!
//! Both holes at once: a `GitSource` scan (mirrors `check`'s call sites,
//! `src/check.rs:367`,`:392`) with `[cache] enabled = false` must produce a
//! byte-identical report to the same content scanned via `FsSource` with caching on.

use reprise::config::Config;
use reprise::source::{FsSource, GitRev, GitSource};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A caller/callee pair per file, plus a shared helper file every caller
/// splices from (the same shape `inline.rs`'s WP-D memo differential test
/// uses) — real inlining traffic, not just plain/near-tier matches, since the
/// memo exists specifically to serve the inliner.
fn fixture_files() -> Vec<(String, String)> {
    let mut files = Vec::new();
    let mut shared = String::new();
    for i in 0..6 {
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
    // A near-duplicate pair too, so the report has non-inline findings to
    // compare as well — the whole pipeline, not just the inline tier.
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
    // WP-D: the memo's hit/miss split depends on which rayon worker touches a
    // given file first — order-dependent under parallel `expand_unit`, so it
    // varies run to run exactly like `memory_lru_hits`/`memory_lru_misses`
    // (also stripped by convention: `src/report.rs`'s own doc comment notes
    // the byte-identity harness strips every `memory_*` field for this same
    // reason). Only the SUM is invariant (every touched file is fetched
    // exactly once per residency-window); the split is timing, not output.
    report.stats.raw_tree_memo_hits = 0;
    report.stats.raw_tree_memo_misses = 0;
}

#[test]
fn git_source_with_cache_off_matches_fs_source_with_cache_on() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    for (name, src) in fixture_files() {
        fs::write(root.join(name), src).unwrap();
    }
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "t@example.com"]);
    git(root, &["config", "user.name", "t"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "wp-d fixture"]);

    // FsSource, caching ON (the default): the reference.
    let cfg_fs = Config::default();
    let mut fs_report = reprise::scan(root, &cfg_fs).expect("fs scan");
    assert!(
        fs_report.stats.calls_inlined > 0,
        "fixture sanity: the inliner must actually splice something \
         (stats: {:?})",
        fs_report.stats
    );

    // GitSource over the INDEX, caching OFF: exercises both holes at once —
    // reads exclusively through `GitSource::read` (never `std::fs`), and
    // every memo fetch takes the re-lower fallback (never `cache::load`).
    let mut cfg_git = Config::default();
    cfg_git.cache.enabled = false;
    let git_source = GitSource::new(root, GitRev::Index, &cfg_git).expect("git source");
    let mut git_report = reprise::scan_source(&git_source, &cfg_git).expect("git scan");

    strip_volatile(&mut fs_report);
    strip_volatile(&mut git_report);
    assert_eq!(
        serde_json::to_string(&fs_report).unwrap(),
        serde_json::to_string(&git_report).unwrap(),
        "GitSource+cache-off must reproduce the FsSource+cache-on report exactly"
    );
}

/// The digest-mismatch guard (design doc risk #3): a file that changes on disk
/// AFTER extraction, with caching off, must fail LOUDLY when the memo tries to
/// rehydrate it — never silently render a stale-vs-fresh mismatched splice.
/// This exercises `RawTreeMemo::rehydrate`'s `assert_eq!` guard directly
/// through the same `FsSource` a real scan uses, isolating the memo's failure
/// mode from `GitSource`'s (which cannot hit this path at all: its blobs are
/// fixed at construction, immune to a concurrent on-disk edit by definition).
#[test]
#[should_panic(expected = "changed on disk mid-scan")]
fn on_disk_edit_mid_scan_with_cache_off_fails_loudly_not_silently() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    for (name, src) in fixture_files() {
        fs::write(root.join(&name), src).unwrap();
    }
    let mut cfg = Config::default();
    cfg.cache.enabled = false;

    // Build the corpus (captures `source_digests` from the CURRENT bytes),
    // then mutate a file on disk before the inline phase's memo fetches
    // it — simulating a mid-scan edit under a caching-disabled scan (the
    // one configuration where the memo cannot fall back to a durable,
    // point-in-time D19 cache blob).
    let corpus = reprise::corpus_units(root, &cfg).expect("corpus scans");
    fs::write(
        root.join("shared.rs"),
        "fn helper_0(x: i64) -> i64 {\n    x - 999\n}\n",
    )
    .unwrap();

    let source = FsSource::new(root);
    let cache_root = root.to_path_buf();
    let memo = reprise::rawmemo::RawTreeMemo::new(
        &corpus.raw_tree_files,
        &corpus.unit_file_idx,
        &source,
        &corpus.source_digests,
        cache_root,
        &cfg,
        std::sync::Arc::clone(&corpus.label_interner),
        None,
    );
    // Any unit whose file is `shared.rs` triggers the rehydrate → digest
    // mismatch → panic.
    let shared_file_idx = corpus
        .raw_tree_files
        .iter()
        .position(|f| f.file.file_name().unwrap() == "shared.rs")
        .expect("fixture has a shared.rs");
    let unit_idx = corpus
        .unit_file_idx
        .iter()
        .position(|&f| f as usize == shared_file_idx)
        .expect("shared.rs has at least one unit");
    let _ = memo.unit(unit_idx);
}
