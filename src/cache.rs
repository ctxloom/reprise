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
///
/// **The bump ritual (SP4 — `docs/sp4-grammar-lift-plan.md` §6).** These pins are
/// one leg of a triad that makes a grammar bump *loud, not silent*: (1) this list
/// keys the cache so a bump invalidates every stale fingerprint; (2) the frontend
/// conformance gate (`crate::frontend::frontend_table` + `every_referenced_construct_
/// exists_in_the_linked_grammar`) asserts every dispatched (kind, field) still exists
/// in the linked grammar — a rename fails loudly, naming the dead construct; (3) the
/// `grammar-*.snap` / `unmapped-*.snap` snapshots turn any grammar-surface change into
/// a reviewable diff. So when bumping a grammar crate: bump the Cargo.toml pin AND this
/// string → run `just test` → the gate stays green (no drift) or fails naming the dead
/// construct, and the schema snapshots diff. Regenerate a *reviewed* diff with
/// `UPDATE_GRAMMAR_SNAPSHOTS=1 cargo test`, and only bump `ir::kind::SCHEME_VERSION` if
/// the canonical form *legitimately* moved.
///
/// `tree-sitter` (the runtime, which executes every grammar) is pinned at the **resolved
/// patch** `0.26.10`, not the minor `0.26` — a patch bump can change parsing behaviour, so
/// it must move the cache key too (parity with the fully-patch-pinned grammar crates).
pub const GRAMMAR_VERSIONS: &[&str] = &[
    "tree-sitter/0.26.10",
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
///
/// v3: TS `const f = () => …` / `= function () {…}` are now extracted as units
/// (`binding_unit`, D43), so TS files' FileUnits changed with no other key move.
///
/// v4: the IR extraction path now produces inline-expanded variant units (spec §5.4)
/// and stores the pre-abstraction lowered tree in `raw_trees` (was the canonical
/// tree), so IR `FileUnits` changed while the plain-unit canonical form (and thus
/// `ir::kind::SCHEME_VERSION`) stayed put — a one-time global cache regen.
///
/// v5: extraction now caps CST-recursion depth at `normalize::MAX_EXTRACTION_DEPTH` (the
/// untrusted-input stack-overflow guard), truncating over-deep subtrees and flagging the
/// unit `parse_degraded`. Pathologically-deep files that *crashed* before can have no valid
/// cached artifact, but a file nesting between the cap and the (higher) pre-fix overflow
/// cliff previously extracted a *full* tree and now extracts a truncated one — so its cached
/// FileUnits could differ from a cold re-extraction. Bumped to keep the D19 hit-≡-cold
/// invariant: a one-time global cache regen.
pub const EXTRACTION_VERSION: u32 = 5;

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
    // The normalizer selector (D-IR-3) picks an entirely different canonical form, so it
    // MUST key the cache — else a warm "historical" cache would serve an "ir" scan (and
    // vice-versa). The IR vocabulary version rides along so a canonical-form change busts it.
    buf.extend_from_slice(cfg.normalize.normalizer.as_bytes());
    buf.push(1);
    buf.extend_from_slice(&crate::ir::kind::SCHEME_VERSION.to_le_bytes());
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

#[cfg(test)]
mod tests {
    use super::key;
    use crate::config::Config;

    #[test]
    fn key_distinguishes_the_normalizer_selector() {
        // Same file, different normalizer ⇒ different canonical form ⇒ different key,
        // so a warm "historical" cache can never serve an "ir" scan (or vice-versa).
        let src = "fn f(a: i32) -> i32 { a + 1 }";
        let mut hist = Config::default();
        hist.normalize.normalizer = "historical".into();
        let mut ir = Config::default();
        ir.normalize.normalizer = "ir".into();
        assert_ne!(key("f.rs", src, &hist), key("f.rs", src, &ir));
        // ...and identical config still keys identically (a warm cache actually hits).
        assert_eq!(key("f.rs", src, &ir), key("f.rs", src, &ir));
    }
}
