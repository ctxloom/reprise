//! Content-addressed scan-scoped pack (memory architecture P3) — unit tests.
//!
//! Contract under test:
//! - roundtrip: serialize → exact-content key → store → load → structurally equal;
//! - keys are xxh3-128 of the serialized VALUE BYTES (never a masked matching hash:
//!   fingerprint-equal trees with different bytes must get different keys);
//! - dedup: byte-identical values → one key, one stored entry;
//! - LRU: byte-bounded and deterministic under forced tiny bounds; an eviction can
//!   never invalidate a caller-held `Arc` (Rust ownership, not cache policy, is the
//!   correctness mechanism — eviction only drops the cache's own reference).

use reprise::config::Config;
use reprise::lang::Lang;
use reprise::pack::Pack;
use reprise::tree::NormNode;
use std::sync::Arc;

fn tree_of(src: &str) -> NormNode {
    let cfg = Config::default();
    let units = reprise::unit::units_from_source(src, Lang::Rust, &cfg);
    assert_eq!(units.len(), 1, "fixture must extract exactly one unit");
    units.into_iter().next().unwrap().tree.into_resident()
}

/// A tree pack wired exactly as `scan()` wires it post-interning: values decode
/// through the wire type and re-intern labels via a per-scan `LabelInterner`.
fn tree_pack(lru_bytes: u64, shards: usize) -> Pack<NormNode> {
    let interner = reprise::intern::LabelInterner::new();
    Pack::with_decoder(lru_bytes, shards, move |bytes| {
        let wire: reprise::tree::NormNodeWire =
            bincode::deserialize(bytes).expect("pack tree decodes");
        wire.into_real(&interner)
    })
    .expect("pack")
}

#[test]
fn roundtrip_tree_is_structurally_equal() {
    let pack: Pack<NormNode> = tree_pack(1 << 20, 4);
    let t = tree_of("fn add(a: i32, b: i32) -> i32 { return a + b; }");
    let key = pack.store(&t);
    let loaded = pack.load(key);
    assert_eq!(*loaded, t, "loaded tree != stored tree");
}

#[test]
fn roundtrip_seq_stream() {
    // The pack is one generic mechanism for both payloads (P3): trees for the
    // near-tier verify, per-unit token streams for the sequence tier.
    let pack: Pack<Vec<(u64, (u32, u32))>> = Pack::new(1 << 20, 2).expect("pack");
    let stream = vec![(42u64, (0u32, 7u32)), (7u64, (8u32, 12u32))];
    let key = pack.store(&stream);
    assert_eq!(*pack.load(key), stream);
}

#[test]
fn byte_identical_values_dedup_to_one_entry() {
    let pack: Pack<NormNode> = tree_pack(1 << 20, 4);
    let t = tree_of("fn f(x: u32) -> u32 { return x * 3; }");
    let k1 = pack.store(&t);
    let bytes_after_first = pack.stored_bytes();
    let k2 = pack.store(&t.clone());
    assert_eq!(k1, k2, "byte-identical trees must share one key");
    assert_eq!(pack.entries(), 1, "dedup must not store a second entry");
    assert_eq!(
        pack.stored_bytes(),
        bytes_after_first,
        "dedup must not grow the pack file"
    );
}

#[test]
fn fingerprint_equal_but_byte_different_trees_get_distinct_keys() {
    // Two copies of the same function at different byte offsets: the MASKED
    // matching fingerprint collides by design (that is what makes them a clone),
    // but their serialized bytes differ (spans) — the pack key is an EXACT-content
    // hash and must separate them. Keying storage on the matching hash would
    // silently alias distinct trees; this pins the invariant.
    let cfg = Config::default();
    let src = "fn f(a: i32) -> i32 { return a + 1; }\n\nfn g(a: i32) -> i32 { return a + 1; }\n";
    let units = reprise::unit::units_from_source(src, Lang::Rust, &cfg);
    assert_eq!(units.len(), 2);
    assert_eq!(
        units[0].fingerprint, units[1].fingerprint,
        "fixture must be a masked-hash collision (a clone pair)"
    );
    assert_ne!(
        units[0].tree.expect_resident(),
        units[1].tree.expect_resident(),
        "fixture trees must differ in bytes (spans)"
    );
    let pack: Pack<NormNode> = tree_pack(1 << 20, 4);
    let ka = pack.store(units[0].tree.expect_resident());
    let kb = pack.store(units[1].tree.expect_resident());
    assert_ne!(
        ka, kb,
        "exact-content keys must not collide for byte-different trees"
    );
    assert_eq!(pack.entries(), 2);
}

#[test]
fn lru_respects_byte_bound_deterministically() {
    // Single shard + a budget sized for ~2 streams: after loading 4 distinct
    // entries the two least-recently-used are evicted; re-loading them is a miss,
    // re-loading the recent ones is a hit. Deterministic — no timing involved.
    let pack: Pack<Vec<(u64, (u32, u32))>> = Pack::new(0, 1).expect("pack");
    // Budget 0 still floors at 2× the largest entry (a verify pair must fit);
    // every value here has equal serialized size, so exactly 2 fit.
    let values: Vec<Vec<(u64, (u32, u32))>> = (0..4u64)
        .map(|i| vec![(i, (0, 1)), (i + 10, (2, 3)), (i + 20, (4, 5))])
        .collect();
    let keys: Vec<u128> = values.iter().map(|v| pack.store(v)).collect();
    for k in &keys {
        pack.load(*k); // cold: 4 misses; LRU ends holding the last two
    }
    assert_eq!(pack.lru_misses(), 4);
    assert_eq!(pack.lru_hits(), 0);
    pack.load(keys[3]); // hit
    pack.load(keys[2]); // hit
    assert_eq!(pack.lru_hits(), 2);
    pack.load(keys[0]); // evicted earlier -> miss (and evicts keys[3])
    assert_eq!(pack.lru_misses(), 5);
    assert_eq!(*pack.load(keys[0]), values[0], "reloaded value intact");
}

#[test]
fn eviction_never_invalidates_a_held_arc() {
    // The in-flight-verify guarantee: eviction drops the CACHE's reference, never
    // the caller's. Hold an Arc, force the entry out by loading larger traffic,
    // then check the held Arc still reads correctly and a fresh load re-decodes.
    let pack: Pack<Vec<(u64, (u32, u32))>> = Pack::new(0, 1).expect("pack");
    let first = vec![(1u64, (0u32, 1u32))];
    let kf = pack.store(&first);
    let held: Arc<Vec<(u64, (u32, u32))>> = pack.load(kf);
    for i in 0..16u64 {
        let filler: Vec<(u64, (u32, u32))> = (0..32).map(|j| (i * 100 + j, (0, 1))).collect();
        let k = pack.store(&filler);
        pack.load(k);
    }
    // `first` is long evicted; the held Arc must still be valid and correct.
    assert_eq!(*held, first, "held Arc corrupted by eviction");
    assert_eq!(*pack.load(kf), first, "re-load after eviction");
}

#[test]
fn pack_is_shared_across_threads() {
    // Near-tier verify materializes pairs from rayon workers: Pack must be Sync,
    // and concurrent store/load of overlapping keys must stay consistent.
    let pack: Pack<Vec<(u64, (u32, u32))>> = Pack::new(1 << 16, 4).expect("pack");
    let pack = &pack;
    std::thread::scope(|s| {
        for t in 0..8 {
            s.spawn(move || {
                for i in 0..50u64 {
                    let v: Vec<(u64, (u32, u32))> = vec![(i % 10, (0, 1)), (t as u64, (2, 3))];
                    let k = pack.store(&v);
                    assert_eq!(*pack.load(k), v);
                }
            });
        }
    });
}
