//! Where a value lives, and who decides — the counterpart to `source.rs`'s
//! "where do the bytes come from".
//!
//! These test the GENERIC contract on a plain payload. `NormNode` rides the same
//! store through `Pack::with_codec` (it needs the scan's interner to serialize),
//! and that path is pinned by the byte-identity gates on real corpora.

use super::*;

fn store_of(lru: u64, dir: &std::path::Path) -> SpillStore<String> {
    SpillStore::new(lru, 2, dir).unwrap()
}

/// The calm path: a resident slot is BORROWED, never copied or loaded. reprise's
/// standing contract is "under budget, zero new work" — the guard must not tax
/// the common case for the benefit of the rare one.
#[test]
fn a_resident_slot_is_borrowed_not_loaded() {
    let slot = Slot::Resident("hello".to_string());
    let got = slot.get(None);
    assert!(matches!(got, Ref::Borrowed(_)), "resident slot was copied");
    assert_eq!(&*got, "hello");
}

/// A spilled slot round-trips through the store and reads back equal.
#[test]
fn a_spilled_slot_round_trips_through_the_store() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = store_of(1 << 20, dir.path());

    let slot = store.admit("payload".to_string()).unwrap();
    assert!(
        matches!(slot, Slot::Spilled(_)),
        "an admitting store must spill"
    );
    assert_eq!(&*store.get(&slot), "payload");
}

/// **The load-bearing property — and the reason a pressure-driven guard is
/// admissible at all.** A reader cannot tell where the value lived. Residency is
/// a MEMORY decision, and a memory decision must never change what reprise
/// reports.
///
/// This is what makes a NON-DETERMINISTIC spill safe: the choice is among
/// provably equivalent paths. Byte-identity constrains the PATHS, not the TIMING.
/// (Independently confirmed end-to-end: `FORCE_GATE=never` vs `=always` is
/// byte-identical on real corpora, with the forced run really spilling.)
#[test]
fn a_reader_cannot_tell_where_the_value_lived() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = store_of(1 << 20, dir.path());

    let resident = Slot::Resident("same bytes".to_string());
    let spilled = store.admit("same bytes".to_string()).unwrap();

    assert_eq!(
        &*store.get(&resident),
        &*store.get(&spilled),
        "residency leaked into what a reader sees"
    );
}

/// Content-addressed: equal values collapse to one stored copy. In a clone
/// detector, duplicate content is not an edge case — it is the whole premise of
/// the product, so this is the common case, not a nicety.
#[test]
fn admitting_equal_values_twice_stores_one_copy() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = store_of(1 << 20, dir.path());

    let a = store.admit("dup".to_string()).unwrap();
    let b = store.admit("dup".to_string()).unwrap();
    match (a, b) {
        (Slot::Spilled(ka), Slot::Spilled(kb)) => assert_eq!(ka, kb, "same content, different key"),
        _ => panic!("expected both spilled"),
    }
    assert_eq!(store.entries(), 1, "identical values stored twice");
}

/// An in-flight read survives eviction of its own slot. Near-tier verify holds
/// TWO values at once while the LRU may evict either; a handle that could be
/// invalidated underneath a reader would be a use-after-free in safe clothing.
#[test]
fn an_in_flight_handle_survives_eviction() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = store_of(1 << 10, dir.path()); // tiny LRU: eviction is certain

    let first = store.admit("first".to_string()).unwrap();
    let held = store.get(&first); // in-flight handle
    for i in 0..256 {
        let _ = store.admit(format!("filler-{i}")).unwrap();
    }
    assert_eq!(
        &*held, "first",
        "in-flight handle was invalidated by eviction"
    );
}

/// A store that is NOT admitting keeps the value resident. This is the seam the
/// reactive guard turns on: today the decision is global (spill everything, or
/// nothing); tomorrow it is per-value and pressure-driven. The CALL SITE does
/// not change — only what `admit` decides.
#[test]
fn a_store_that_declines_keeps_the_value_resident() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut store = store_of(1 << 20, dir.path());
    store.set_admitting(false);

    let slot = store.admit("kept".to_string()).unwrap();
    assert!(
        matches!(slot, Slot::Resident(_)),
        "a declining store must keep the value resident"
    );
    assert_eq!(store.entries(), 0, "a declining store wrote to disk");
    assert_eq!(&*store.get(&slot), "kept");
}
