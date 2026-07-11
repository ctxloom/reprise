//! Interning WP (session `stark-mixed-front`, `interning.plan.md`) — characterization
//! tests pinned BEFORE any production change (Stage 2 step 0). These MUST stay green
//! through every wave of the refactor:
//!
//! - `fingerprint_snapshot_is_stable`: every unit's fingerprint across the wild
//!   fixtures, under both normalizers, hashed into one literal. A change here means a
//!   fingerprint moved — forbidden by this WP (no `FINGERPRINT_SCHEME`/`SCHEME_VERSION`
//!   bump is in scope).
//! - `d19_blob_bytes_are_stable`: the D19 on-disk cache format (what `cache::store`
//!   would write) for one fixed fixture file, hashed into a literal. Whether this stays
//!   green AFTER the refactor is the FACTUAL test that decides the `EXTRACTION_VERSION`
//!   bump question (P4, one-bump-campaign-wide) — not eyeballing/opinion.

use reprise::config::{Config, Normalizer};
use std::path::PathBuf;
use xxhash_rust::xxh3::xxh3_128;

fn wild_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("benches/wild")
}

/// Non-C wild fixture dirs (historical normalizer supports these).
const WILD_DIRS: &[&str] = &[
    "w1_gin_marshalxml",
    "w2_ripgrep_bytecount",
    "w3_ripgrep_sort",
    "w4_serde_end",
    "w5_serde_tagorcontent",
    "w6_flask_maxprops",
    "w7_click_chunkpump",
    "w8_ripgrep_convert",
    "w9_ripgrep_cpufeatures",
];

/// C-only wild fixture dirs (IR normalizer only — C has no historical profile).
const WILD_C_DIRS: &[&str] = &["wc1_ext4_extspaceroot", "wc2_ext4_mbbits"];

/// Extract every unit's fingerprint from `dirs` under `normalizer`, sorted into a
/// deterministic order, then fold into one xxh3-128 over the concatenated
/// little-endian fingerprint bytes (spec: one hash over the sorted list, not one
/// assertion per unit — keeps this test small and still exhaustive).
fn fingerprint_digest(dirs: &[&str], normalizer: Normalizer) -> u128 {
    let mut cfg = Config::default();
    cfg.normalize.normalizer = normalizer;
    cfg.cache.enabled = false;
    let mut fps: Vec<(String, (u32, u32), String, u128)> = Vec::new();
    for dir in dirs {
        let root = wild_root().join(dir);
        assert!(root.is_dir(), "missing wild fixture {dir}");
        let corpus = reprise::corpus_units(&root, &cfg)
            .unwrap_or_else(|e| panic!("corpus_units({dir}) under {normalizer:?} failed: {e}"));
        for u in &corpus.units {
            fps.push((
                u.file.to_string_lossy().into_owned(),
                u.byte_span,
                u.name.clone(),
                u.fingerprint,
            ));
        }
    }
    fps.sort();
    assert!(!fps.is_empty(), "no units extracted for {dirs:?}");
    let mut buf = Vec::with_capacity(fps.len() * 16);
    for (_, _, _, fp) in &fps {
        buf.extend_from_slice(&fp.to_le_bytes());
    }
    xxh3_128(&buf)
}

#[test]
fn fingerprint_snapshot_is_stable_historical() {
    let digest = fingerprint_digest(WILD_DIRS, Normalizer::Historical);
    assert_eq!(
        format!("{digest:032x}"),
        "ddaf9973c1de4b713f5ccaf10c59a80e",
        "historical-normalizer fingerprint digest drifted — the interning WP must not \
         move any fingerprint; if this fails after a change, that change broke the \
         byte-identical-fingerprints invariant, not this test"
    );
}

#[test]
fn fingerprint_snapshot_is_stable_ir() {
    let mut dirs: Vec<&str> = WILD_DIRS.to_vec();
    dirs.extend_from_slice(WILD_C_DIRS);
    let digest = fingerprint_digest(&dirs, Normalizer::Ir);
    assert_eq!(
        format!("{digest:032x}"),
        "76f12660cdc2797ed251ab3c30f5e5ce",
        "ir-normalizer fingerprint digest drifted — the interning WP must not move any \
         fingerprint; if this fails after a change, that change broke the \
         byte-identical-fingerprints invariant, not this test"
    );
}

/// The D19 on-disk cache format for one fixed fixture file (`w1_gin_marshalxml/a.go`,
/// IR normalizer, default config) — exactly the bytes `cache::store` would write
/// (`bincode::serialize(&FileUnits)`). Hashed rather than stored verbatim (the blob is
/// several KB). This test's continued greenness AFTER the interning refactor is the
/// factual evidence for the `EXTRACTION_VERSION` bump decision (P4): stays green ⇒
/// wire format is byte-identical ⇒ no bump; goes red ⇒ format changed ⇒ bump to 7.
#[test]
fn d19_blob_bytes_are_stable() {
    let src = std::fs::read_to_string(wild_root().join("w1_gin_marshalxml").join("a.go"))
        .expect("read fixture");
    // The serialized `FileUnits` embeds `Unit.file` verbatim, so the blob's bytes are
    // a function of the path passed in — extract under a FIXED synthetic path, or this
    // pin only holds in the checkout where it was captured (a worktree broke it once).
    let path = PathBuf::from("pinned/w1_gin_marshalxml/a.go");
    let cfg = Config::default();
    let label_interner = reprise::intern::LabelInterner::new();
    let file_units = reprise::unit::extract_file_units_keep_raw(
        &path,
        &src,
        reprise::lang::Lang::Go,
        &cfg,
        &label_interner,
    );
    assert!(!file_units.units.is_empty(), "fixture produced no units");
    let bytes = bincode::serialize(&file_units).expect("serialize FileUnits");
    let digest = xxh3_128(&bytes);
    assert_eq!(
        (bytes.len(), format!("{digest:032x}")),
        (11470usize, "4889d43f9aefe767b24efd4ac8b9373f".to_string()),
        "D19 FileUnits serialization for w1_gin_marshalxml/a.go drifted; see this test's \
         doc comment for what that means for EXTRACTION_VERSION"
    );
}

/// Cache serde roundtrip (Stage 2 item 3 of the plan): store with one scan's
/// `LabelInterner` population, load into a FRESH `LabelInterner` (simulating a brand
/// new process/scan reusing a warm D19 cache) — the trees must come back
/// STRUCTURALLY EQUAL and fingerprint-equal, even though `External`/`LitKept` symbols
/// necessarily get different raw ids in the fresh interner (content-equality, not
/// identity-equality — see `intern::LSym`'s `PartialEq`).
#[test]
fn cache_roundtrip_survives_a_fresh_label_interner() {
    let path = wild_root().join("w1_gin_marshalxml").join("a.go");
    let src = std::fs::read_to_string(&path).expect("read fixture");
    let cfg = Config::default();

    let write_interner = reprise::intern::LabelInterner::new();
    let original = reprise::unit::extract_file_units_keep_raw(
        &path,
        &src,
        reprise::lang::Lang::Go,
        &cfg,
        &write_interner,
    );
    assert!(!original.units.is_empty(), "fixture produced no units");
    let bytes = bincode::serialize(&original).expect("serialize FileUnits");

    // A fresh interner — as if this were a brand new process reusing a warm D19 cache
    // (the MCP-server-longevity property: no shared state with `write_interner`).
    let read_interner = reprise::intern::LabelInterner::new();
    let wire: reprise::unit::FileUnitsWire =
        bincode::deserialize(&bytes).expect("deserialize FileUnitsWire");
    let reloaded = wire.into_real(&read_interner);

    assert_eq!(
        original.units.len(),
        reloaded.units.len(),
        "unit count changed across the cache roundtrip"
    );
    for (o, r) in original.units.iter().zip(reloaded.units.iter()) {
        assert_eq!(
            o.fingerprint, r.fingerprint,
            "fingerprint changed across the cache roundtrip for unit `{}`",
            o.name
        );
        assert_eq!(
            o.tree, r.tree,
            "tree not structurally equal across the cache roundtrip for unit `{}` \
             (LSym equality is content-based, so a fresh interner must not matter)",
            o.name
        );
    }
    assert_eq!(
        original.raw_trees, reloaded.raw_trees,
        "raw_trees not structurally equal across the cache roundtrip"
    );
}
