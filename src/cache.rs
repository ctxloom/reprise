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
    "tree-sitter-c/0.24.2",
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
///
/// v6: correctness fixes to the HISTORICAL per-language normalizers change their canonical
/// output for affected units — Go grouped parameters/`var`/`const` names (`func f(a, b int)`,
/// `var a, b int`) are now fully counted/declared instead of first-only (fixing a mislowered
/// grouped-param recursion and mis-abstracted trailing names); the TS switch `break` inside a
/// loop is no longer rewritten as the loop's `return`; and TS/Python/Kotlin recursion lowering
/// no longer descends into nested closures/local functions (which produced a `continue` outside
/// any loop). The IR node-set is unchanged (so `ir::kind::SCHEME_VERSION` stays put), but these
/// historical fingerprints move, so the cache must invalidate: a one-time global cache regen.
pub const EXTRACTION_VERSION: u32 = 6;

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
    // `.as_str()` yields exactly "ir"/"historical" — the same bytes the pre-enum
    // String field fed here, so this refactor keeps the key byte-identical per
    // normalizer (warm caches are NOT invalidated).
    buf.extend_from_slice(cfg.normalize.normalizer.as_str().as_bytes());
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
/// (the key covers the relative path, so a moved root still hits). `label_interner`
/// is the CURRENT scan's per-scan interner (interning WP) — a cache hit re-interns
/// the stored `Label::External`/`LitKept` text into it (fresh ids; a different scan's
/// interner, by design — see the interning preamble's cache-serde section). Bytes are
/// read through [`crate::unit::FileUnitsWire`], which is field-for-field identical to
/// the pre-interning `Box<str>`-based `FileUnits` shape, so this reads today's D19
/// blobs (and any future ones written by [`store`]) without a format change.
pub fn load(
    root: &Path,
    key: u128,
    path: &Path,
    label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
) -> Option<FileUnits> {
    let bytes = std::fs::read(entry_path(root, key)).ok()?;
    let wire: crate::unit::FileUnitsWire = bincode::deserialize(&bytes).ok()?;
    let mut cached: FileUnits = wire.into_real(label_interner);
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
    use super::{key, load, store};
    use crate::config::{Config, Normalizer};

    /// Interning WP end-to-end: `store` (real per-scan interner) → `load` (a FRESH
    /// per-scan interner, simulating a new process reusing a warm cache) through the
    /// ACTUAL `cache::store`/`cache::load` functions (not a hand-rolled
    /// serialize/deserialize) — a cache hit must be byte-identical to a cold
    /// extraction in every observable way (fingerprint, tree shape), matching this
    /// module's own doc-comment contract.
    #[test]
    fn store_then_load_round_trips_through_a_fresh_label_interner() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let cfg = Config::default();
        let src =
            "func f(a int) int {\n\tfor i := 0; i < a; i++ {\n\t\ta += i\n\t}\n\treturn a\n}\n";
        let path = std::path::Path::new("f.go");

        let write_interner = crate::intern::LabelInterner::new();
        let original = crate::unit::extract_file_units_keep_raw(
            path,
            src,
            crate::lang::Lang::Go,
            &cfg,
            &write_interner,
        );
        assert!(!original.units.is_empty(), "fixture produced no units");
        let k = key("f.go", src, &cfg);
        store(root, k, &original);

        let read_interner = crate::intern::LabelInterner::new();
        let reloaded = load(root, k, path, &read_interner).expect("cache hit");

        assert_eq!(original.units.len(), reloaded.units.len());
        for (o, r) in original.units.iter().zip(reloaded.units.iter()) {
            assert_eq!(o.fingerprint, r.fingerprint);
            assert_eq!(o.tree, r.tree);
        }
        assert_eq!(original.raw_trees, reloaded.raw_trees);
    }

    #[test]
    fn key_distinguishes_the_normalizer_selector() {
        // Same file, different normalizer ⇒ different canonical form ⇒ different key,
        // so a warm "historical" cache can never serve an "ir" scan (or vice-versa).
        let src = "fn f(a: i32) -> i32 { a + 1 }";
        let mut hist = Config::default();
        hist.normalize.normalizer = Normalizer::Historical;
        let mut ir = Config::default();
        ir.normalize.normalizer = Normalizer::Ir;
        assert_ne!(key("f.rs", src, &hist), key("f.rs", src, &ir));
        // ...and identical config still keys identically (a warm cache actually hits).
        assert_eq!(key("f.rs", src, &ir), key("f.rs", src, &ir));
    }

    #[test]
    fn key_is_byte_identical_to_the_pre_enum_string_bytes() {
        // The `Normalizer` refactor must NOT invalidate warm caches: `as_str()` must feed
        // the exact bytes the old `String` field did ("ir"/"historical"). Reconstruct the
        // key with the raw string bytes and assert equality against the live key.
        let src = "fn f(a: i32) -> i32 { a + 1 }";
        for (norm, s) in [
            (Normalizer::Ir, "ir"),
            (Normalizer::Historical, "historical"),
        ] {
            let mut cfg = Config::default();
            cfg.normalize.normalizer = norm;
            assert_eq!(
                norm.as_str(),
                s,
                "as_str must match the historical String value"
            );
            // The live key uses `norm.as_str().as_bytes()`; `s` is the literal old value.
            assert_eq!(norm.as_str().as_bytes(), s.as_bytes());
            // And a warm re-key with the same normalizer still hits.
            assert_eq!(key("f.rs", src, &cfg), key("f.rs", src, &cfg));
        }
    }
}
