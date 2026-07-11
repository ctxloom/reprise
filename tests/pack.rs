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
//! - pack-dir resolution (the tmpfs-ENOSPC fix): default under `<root>/.reprise/tmp/`,
//!   config override wins, an unwritable root falls back to the process temp dir with
//!   a named warning.
//! - store failure is clean: a write failure never panics, never poisons the
//!   pack for subsequent callers, and propagates as a `PackStoreError`.

use reprise::config::Config;
use reprise::lang::Lang;
use reprise::pack::{Pack, resolve_pack_dir};
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
/// `dir` is the pack's backing directory (kept alive by the caller).
fn tree_pack(lru_bytes: u64, shards: usize, dir: &std::path::Path) -> Pack<NormNode> {
    let interner = reprise::intern::LabelInterner::new();
    Pack::with_decoder(lru_bytes, shards, dir, move |bytes| {
        let wire: reprise::tree::NormNodeWire =
            bincode::deserialize(bytes).expect("pack tree decodes");
        wire.into_real(&interner)
    })
    .expect("pack")
}

#[test]
fn roundtrip_tree_is_structurally_equal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pack: Pack<NormNode> = tree_pack(1 << 20, 4, dir.path());
    let t = tree_of("fn add(a: i32, b: i32) -> i32 { return a + b; }");
    let key = pack.store(&t).expect("store");
    let loaded = pack.load(key);
    assert_eq!(*loaded, t, "loaded tree != stored tree");
}

#[test]
fn roundtrip_seq_stream() {
    // The pack is one generic mechanism for both payloads (P3): trees for the
    // near-tier verify, per-unit token streams for the sequence tier.
    let dir = tempfile::tempdir().expect("tempdir");
    let pack: Pack<Vec<(u64, (u32, u32))>> = Pack::new(1 << 20, 2, dir.path()).expect("pack");
    let stream = vec![(42u64, (0u32, 7u32)), (7u64, (8u32, 12u32))];
    let key = pack.store(&stream).expect("store");
    assert_eq!(*pack.load(key), stream);
}

#[test]
fn byte_identical_values_dedup_to_one_entry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pack: Pack<NormNode> = tree_pack(1 << 20, 4, dir.path());
    let t = tree_of("fn f(x: u32) -> u32 { return x * 3; }");
    let k1 = pack.store(&t).expect("store");
    let bytes_after_first = pack.stored_bytes();
    let k2 = pack.store(&t.clone()).expect("store");
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
    let dir = tempfile::tempdir().expect("tempdir");
    let pack: Pack<NormNode> = tree_pack(1 << 20, 4, dir.path());
    let ka = pack.store(units[0].tree.expect_resident()).expect("store");
    let kb = pack.store(units[1].tree.expect_resident()).expect("store");
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
    let dir = tempfile::tempdir().expect("tempdir");
    let pack: Pack<Vec<(u64, (u32, u32))>> = Pack::new(0, 1, dir.path()).expect("pack");
    // Budget 0 still floors at 2× the largest entry (a verify pair must fit);
    // every value here has equal serialized size, so exactly 2 fit.
    let values: Vec<Vec<(u64, (u32, u32))>> = (0..4u64)
        .map(|i| vec![(i, (0, 1)), (i + 10, (2, 3)), (i + 20, (4, 5))])
        .collect();
    let keys: Vec<u128> = values
        .iter()
        .map(|v| pack.store(v).expect("store"))
        .collect();
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
    let dir = tempfile::tempdir().expect("tempdir");
    let pack: Pack<Vec<(u64, (u32, u32))>> = Pack::new(0, 1, dir.path()).expect("pack");
    let first = vec![(1u64, (0u32, 1u32))];
    let kf = pack.store(&first).expect("store");
    let held: Arc<Vec<(u64, (u32, u32))>> = pack.load(kf);
    for i in 0..16u64 {
        let filler: Vec<(u64, (u32, u32))> = (0..32).map(|j| (i * 100 + j, (0, 1))).collect();
        let k = pack.store(&filler).expect("store");
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
    let dir = tempfile::tempdir().expect("tempdir");
    let pack: Pack<Vec<(u64, (u32, u32))>> = Pack::new(1 << 16, 4, dir.path()).expect("pack");
    let pack = &pack;
    std::thread::scope(|s| {
        for t in 0..8 {
            s.spawn(move || {
                for i in 0..50u64 {
                    let v: Vec<(u64, (u32, u32))> = vec![(i % 10, (0, 1)), (t as u64, (2, 3))];
                    let k = pack.store(&v).expect("store");
                    assert_eq!(*pack.load(k), v);
                }
            });
        }
    });
}

// ---- pack-dir resolution (the tmpfs-ENOSPC fix) ----

#[test]
fn default_pack_dir_is_under_the_scan_root() {
    let root = tempfile::tempdir().expect("tempdir");
    let resolved = resolve_pack_dir(root.path(), None);
    assert_eq!(resolved.dir, root.path().join(".reprise").join("tmp"));
    assert!(
        resolved.dir.is_dir(),
        "resolve_pack_dir must create the directory"
    );
    assert!(resolved.warning.is_none(), "a writable root must not warn");
    assert!(
        resolved.owned,
        "the scan-root default is ours to clean up when the scan ends"
    );
}

#[test]
fn configured_pack_dir_override_wins() {
    let root = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let resolved = resolve_pack_dir(root.path(), Some(elsewhere.path()));
    assert_eq!(
        resolved.dir,
        elsewhere.path(),
        "an explicit pack_dir must outrank the scan-root default"
    );
    assert!(
        resolved.warning.is_none(),
        "an explicit override is trusted, never warned about"
    );
    assert!(
        !resolved.owned,
        "an explicit override is never ours to remove"
    );
}

#[cfg(unix)]
#[test]
fn unwritable_root_falls_back_to_temp_dir_and_warns() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().expect("tempdir");
    // chmod the root read-only (no write/execute for the owner): creating
    // `<root>/.reprise/tmp` must fail — exactly the RO-root scenario Fix 1
    // must degrade gracefully from (no override configured).
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o555)).expect("chmod");
    let resolved = resolve_pack_dir(root.path(), None);
    // Restore write perms so the tempdir can clean itself up.
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o755)).expect("chmod restore");

    assert_eq!(
        resolved.dir,
        std::env::temp_dir(),
        "an unwritable root with no override must fall back to the process temp dir"
    );
    let warning = resolved
        .warning
        .expect("an unwritable-root fallback must warn (never silent — a pack must write)");
    assert!(
        warning.contains("pack_dir"),
        "the warning must name the [memory] pack_dir remedy: {warning}"
    );
    assert!(
        !resolved.owned,
        "the temp_dir() fallback is the system's directory, never ours to remove"
    );
}

// ---- store failure is clean (never a panic, never a poisoned mutex) ----

#[cfg(unix)]
#[test]
fn store_failure_on_a_full_device_is_a_clean_err_not_a_panic() {
    // `/dev/full` always returns ENOSPC on write — a deterministic stand-in
    // for "the pack's volume filled up" without needing a real full
    // filesystem. `Pack::over_file` is the test hook that lets us point the
    // pack's backing file straight at it.
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .expect("/dev/full must exist on Linux");
    let pack: Pack<Vec<(u64, (u32, u32))>> = Pack::over_file(file, "/dev/full", 1 << 20, 4);
    let value = vec![(1u64, (0u32, 1u32))];

    let err1 = pack
        .store(&value)
        .expect_err("a write failure must be a clean Err, never a panic");
    assert!(
        err1.to_string().contains("/dev/full"),
        "the error must name the pack path: {err1}"
    );
    assert!(
        err1.to_string().contains("pack_dir"),
        "the error must name the [memory] pack_dir remedy: {err1}"
    );

    // The regression this pins: the OLD code panicked on the first write
    // failure (`.expect("pack append write")`) while holding the append
    // `Mutex`, poisoning it — every subsequent `store` then panicked with
    // `PoisonError` instead of returning a clean `Err`. This second call, on
    // the SAME pack, must fail fast and cleanly, with the SAME latched error.
    let err2 = pack
        .store(&value)
        .expect_err("a second store on a doomed pack must also fail cleanly, not panic");
    assert_eq!(
        err1.to_string(),
        err2.to_string(),
        "a doomed pack must reuse the latched failure verbatim, not retry the write"
    );
}
