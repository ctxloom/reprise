//! D19 cache wire-format compatibility net (Tier-1 drop-trees WP).
//!
//! `fixtures/cache_compat/fileunits_e84d4b5.bin` is a real `.reprise/cache` blob
//! written by the UNMODIFIED e84d4b5 binary scanning `fixtures/cache_compat/blobsrc.rs`
//! (captured before any of this WP's changes — it cannot be regenerated afterwards).
//! P4 caps this campaign at one `EXTRACTION_VERSION` bump, and neither this WP nor the
//! interning WP is it: the on-disk `FileUnits` bytes must stay EXACTLY as e84d4b5
//! wrote them, even though `Unit.tree`'s in-memory representation changed twice
//! (interning's `Kind`/`Field`/`LSym` resolve-to-string serde, and this WP's
//! `TreeSlot` serialize shim).

use reprise::config::Config;
use reprise::lang::Lang;
use reprise::unit::{FileUnits, FileUnitsWire};
use std::path::Path;

const BLOB: &[u8] = include_bytes!("fixtures/cache_compat/fileunits_e84d4b5.bin");

/// Deserialize the e84d4b5 blob through the interning WP's wire path (the only
/// deserialize path since `FileUnits` stopped deriving `Deserialize`),
/// re-interning labels into a fresh per-scan interner — exactly what
/// `cache::load` does. Returns the interner too (interning-id-conversion WP):
/// `FileUnits` no longer self-serializes (an id-based `LSym` can't resolve
/// without it), so a caller that re-serializes what it loaded must feed back
/// the SAME interner instance via `to_wire`.
fn load_blob() -> (FileUnits, std::sync::Arc<reprise::intern::LabelInterner>) {
    let wire: FileUnitsWire =
        bincode::deserialize(BLOB).expect("e84d4b5-format cache blob no longer deserializes");
    let interner = reprise::intern::LabelInterner::new();
    let fu = wire.into_real(&interner);
    (fu, interner)
}

#[test]
fn e84d4b5_cache_blob_deserializes_and_roundtrips_byte_identically() {
    // Deserialize compatibility: the old-format bytes must still load.
    let (fu, interner) = load_blob();
    // Serialize compatibility: re-serializing what we loaded must reproduce the
    // e84d4b5 bytes exactly — proves the current serializer (through both the
    // interning WP's resolve-to-string serde AND this WP's TreeSlot shim) still
    // writes the old wire format (no silent D19 format drift, no
    // EXTRACTION_VERSION bump needed).
    let wire = fu.to_wire(&interner);
    let re = bincode::serialize(&wire).expect("re-serialize");
    assert_eq!(
        re.as_slice(),
        BLOB,
        "FileUnits wire format drifted from the e84d4b5 cache format — this requires an \
         EXTRACTION_VERSION bump, which P4 forbids for this WP"
    );
}

#[test]
fn e84d4b5_cache_blob_matches_a_fresh_extraction() {
    // Semantic compatibility (the D19 hit-≡-cold contract): the cached units must
    // equal a fresh extraction of the same committed source. Both sides MUST share
    // one `LabelInterner` (interning-id-conversion WP): `Label::External`/`LitKept`'s
    // `LSym` is a bare per-scan id, and two independently-scoped interners assign
    // different ids to the same string when it's first touched in a different
    // traversal order — comparing across them would spuriously fail even when the
    // trees are semantically identical. A SHARED interner is content-addressed (the
    // same string always resolves to the same id, whichever side interns it first),
    // so this makes the comparison meaningful again.
    let (fu, interner) = load_blob();
    let src = include_str!("fixtures/cache_compat/blobsrc.rs");
    let fresh = reprise::unit::extract_file_units_keep_raw(
        Path::new("blobsrc.rs"),
        src,
        Lang::Rust,
        &Config::default(),
        &interner,
    );
    assert_eq!(fu.units.len(), fresh.units.len(), "unit count");
    for (a, b) in fu.units.iter().zip(&fresh.units) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.fingerprint, b.fingerprint, "fingerprint of {}", a.name);
        assert_eq!(a.token_count, b.token_count);
        assert_eq!(a.tree, b.tree, "canonical tree of {}", a.name);
    }
    assert_eq!(fu.raw_trees, fresh.raw_trees, "raw trees");
    assert_eq!(fu.suppressed, fresh.suppressed);
}
