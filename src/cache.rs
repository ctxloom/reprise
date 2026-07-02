//! Version-keyed on-disk cache (spec §4, DECISIONS.md D8/D19): flat bincode
//! files under `<root>/.reprise/cache/`, one per source file, named by
//! xxh3-128 of (relative path ‖ file content ‖ fingerprint scheme ‖ tool
//! version ‖ grammar crate versions ‖ extraction-relevant config). A tool or
//! grammar upgrade changes every key and thereby invalidates the whole index;
//! stale fingerprints must never feed baseline or drift comparisons (§12).
//!
//! Correctness contract: a cache hit is byte-identical to a cold extraction
//! (asserted by tests/check_baseline.rs). Writes are best-effort — a
//! read-only scan root degrades to cold scans, never to an error.

use crate::config::Config;
use crate::unit::FileUnits;
use std::path::{Path, PathBuf};
use xxhash_rust::xxh3::xxh3_128;

/// Grammar crate versions, keyed into every cache entry (spec §4). Kept as a
/// hardcoded constant list adjacent to the Cargo.toml pins because Cargo does
/// not expose dependency versions at build time (D19); update alongside any
/// grammar bump — the pinned versions are load-bearing (DECISIONS.md D9).
pub const GRAMMAR_VERSIONS: &[&str] = &[
    "tree-sitter/0.26",
    "tree-sitter-rust/0.24.2",
    "tree-sitter-python/0.25.0",
    "tree-sitter-typescript/0.23.2",
    "tree-sitter-go/0.25.0",
    "tree-sitter-kotlin-ng/1.1.0",
];

/// Extraction-logic version: bump on ANY change to what `FileUnits` contains
/// for identical input (normalization passes, fold findings, suppression
/// rules) that does not already bump `FINGERPRINT_SCHEME`. Discovered the
/// hard way in M4a: the D30 dispatch-arm suppression changed cached repeats
/// while every key component stayed put, so warm scans served pre-D30
/// findings (D30 note).
pub const EXTRACTION_VERSION: u32 = 2;

/// Cache key for one source file (D8: the version-keying IS the filename).
pub fn key(rel_path: &str, content: &str, cfg: &Config) -> u128 {
    let mut buf = Vec::with_capacity(content.len() + 256);
    buf.extend_from_slice(rel_path.as_bytes());
    buf.push(0);
    buf.extend_from_slice(content.as_bytes());
    buf.push(0);
    buf.extend_from_slice(&EXTRACTION_VERSION.to_le_bytes());
    buf.extend_from_slice(&crate::fingerprint::FINGERPRINT_SCHEME.to_le_bytes());
    buf.extend_from_slice(env!("CARGO_PKG_VERSION").as_bytes());
    buf.push(0);
    for v in GRAMMAR_VERSIONS {
        buf.extend_from_slice(v.as_bytes());
        buf.push(0);
    }
    // Extraction-relevant config: anything that changes FileUnits content.
    for lit in &cfg.normalize.literal_keep {
        buf.extend_from_slice(lit.as_bytes());
        buf.push(1);
    }
    buf.extend_from_slice(&cfg.thresholds.fold_min_repeats.to_le_bytes());
    buf.extend_from_slice(&cfg.thresholds.min_seq_tokens.to_le_bytes());
    xxh3_128(&buf)
}

fn entry_path(root: &Path, key: u128) -> PathBuf {
    root.join(".reprise")
        .join("cache")
        .join(format!("{key:032x}.bin"))
}

/// Load a cached extraction; `path` re-anchors the stored file coordinates
/// (the key covers the relative path, so a moved root still hits).
pub fn load(root: &Path, key: u128, path: &Path) -> Option<FileUnits> {
    let bytes = std::fs::read(entry_path(root, key)).ok()?;
    let mut cached: FileUnits = bincode::deserialize(&bytes).ok()?;
    for unit in &mut cached.units {
        unit.file = path.to_path_buf();
    }
    for repeat in &mut cached.repeats {
        repeat.file = path.to_path_buf();
    }
    Some(cached)
}

/// Best-effort write; failures (read-only root, races) are silently ignored.
pub fn store(root: &Path, key: u128, value: &FileUnits) {
    let dir = root.join(".reprise").join("cache");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    // Self-gitignoring cache dir (like `target/`): scanned repos need no
    // .gitignore edit for `.reprise/` to stay out of version control.
    let ignore = root.join(".reprise").join(".gitignore");
    if !ignore.exists() {
        let _ = std::fs::write(&ignore, "*\n");
    }
    let Ok(bytes) = bincode::serialize(value) else {
        return;
    };
    // Write-then-rename so a concurrent reader never sees a torn entry.
    let tmp = dir.join(format!("{key:032x}.tmp"));
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, entry_path(root, key));
    }
}
