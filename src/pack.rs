//! Scan-scoped, content-addressed pack (memory architecture P3): the reload
//! substrate for details that spill over the memory gate — canonical trees for
//! near-tier verify pairs, per-unit token streams for the sequence tier.
//!
//! - **Key**: xxh3-128 of the value's serialized bytes — an EXACT-content hash.
//!   `Unit::fingerprint` / `subtree_inventory` exist to make *similar* things
//!   collide (masked locals) and MUST NOT key storage; hashing the value bytes
//!   makes collision semantics correct by construction and gives byte-identical
//!   values (a clone detector's staple) free dedup.
//! - **Value**: bincode bytes appended to one anonymous temp file
//!   (`tempfile::tempfile()`, unlinked at creation — the pack cannot outlive the
//!   scan even on abnormal exit), `pread` at offset on miss.
//! - **Residency**: a `HashMap<key, (offset, len)>` index (tens of bytes per
//!   entry) plus a byte-bounded, sharded, `Sync` LRU of decoded `Arc<T>` values.
//!   Eviction only drops the cache's own `Arc`; a caller-held `Arc` from an
//!   in-flight verify stays valid — Rust ownership, not cache policy, is the
//!   correctness mechanism. The byte bound floors at 2× the largest entry per
//!   shard so a verify PAIR can't thrash (a performance floor, not a safety one).
//!
//! The pack is one generic mechanism: `Pack<NormNode>` for trees (plain trees
//! spill at gate-trip, variant trees at `finish_variant` — same store, P3
//! dissolves the "variants aren't cached" fork) and `Pack<Vec<…>>` for sequence
//! streams. Nothing here touches the durable D19 cache or its format.

use serde::Serialize;
use serde::de::DeserializeOwned;

/// The pluggable value decoder (see the struct doc).
type Decoder<T> = Box<dyn Fn(&[u8]) -> T + Send + Sync>;
use std::collections::HashMap;
use std::fs::File;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

/// A content-addressed store of serialized `T` values in one scan-scoped temp
/// file, fronted by a byte-bounded sharded LRU of decoded values.
///
/// Decoding is pluggable ([`Pack::with_decoder`]) because not every payload can
/// derive a context-free `Deserialize`: post-interning, a `NormNode`'s labels
/// re-intern through the CURRENT scan's `LabelInterner` (`NormNodeWire::
/// into_real`) — the pack is scan-scoped, so capturing that scan's interner in
/// the decoder is exactly right. Plain-data payloads use [`Pack::new`].
pub struct Pack<T> {
    file: File,
    /// key → (offset, len) for every stored value. Read-mostly.
    index: RwLock<HashMap<u128, (u64, u32)>>,
    /// Append cursor; also serializes writers (index double-check under this lock).
    append: Mutex<u64>,
    lru: ShardedLru<T>,
    hits: AtomicU64,
    misses: AtomicU64,
    decode: Decoder<T>,
}

impl<T: Serialize + DeserializeOwned + Send + Sync> Pack<T> {
    /// `lru_bytes` bounds the decoded-value cache (split across `shards`; each
    /// shard floors at 2× its largest entry so a verify pair always fits);
    /// `shards` bounds lock contention from parallel verify workers.
    pub fn new(lru_bytes: u64, shards: usize) -> std::io::Result<Self> {
        Self::with_decoder(lru_bytes, shards, |bytes| {
            bincode::deserialize(bytes).expect("pack value decodes")
        })
    }
}

impl<T: Serialize + Send + Sync> Pack<T> {
    /// [`Pack::new`] with an explicit decoder — for payloads whose deserialize
    /// needs context a derive cannot reach (the interned-tree wire path).
    pub fn with_decoder(
        lru_bytes: u64,
        shards: usize,
        decode: impl Fn(&[u8]) -> T + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        Ok(Pack {
            file: tempfile::tempfile()?,
            index: RwLock::new(HashMap::new()),
            append: Mutex::new(0),
            lru: ShardedLru::new(lru_bytes, shards),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            decode: Box::new(decode),
        })
    }

    /// Serialize, key by exact content, and store (byte-identical values dedup
    /// to the existing entry). Returns the pack key.
    pub fn store(&self, value: &T) -> u128 {
        let bytes = bincode::serialize(value).expect("pack value serializes");
        let key = xxhash_rust::xxh3::xxh3_128(&bytes);
        if self.index.read().expect("pack index").contains_key(&key) {
            return key; // dedup: same bytes, same key, stored once
        }
        let mut end = self.append.lock().expect("pack append");
        // Double-check under the append lock: a racing writer may have stored
        // the same bytes between the read above and taking this lock.
        if self.index.read().expect("pack index").contains_key(&key) {
            return key;
        }
        let offset = *end;
        write_all_at(&self.file, &bytes, offset).expect("pack append write");
        *end += bytes.len() as u64;
        self.index
            .write()
            .expect("pack index")
            .insert(key, (offset, bytes.len() as u32));
        key
    }

    /// Materialize a stored value: LRU hit, or `pread` + decode on miss (then
    /// cached). Panics on an unknown key — every load site loads keys it stored.
    pub fn load(&self, key: u128) -> Arc<T> {
        if let Some(v) = self.lru.get(key) {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return v;
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        let (offset, len) = *self
            .index
            .read()
            .expect("pack index")
            .get(&key)
            .expect("pack key was never stored");
        let mut buf = vec![0u8; len as usize];
        read_exact_at(&self.file, &mut buf, offset).expect("pack read");
        let arc = Arc::new((self.decode)(&buf));
        self.lru.insert(key, Arc::clone(&arc), u64::from(len));
        arc
    }

    /// Distinct stored entries (post-dedup).
    pub fn entries(&self) -> usize {
        self.index.read().expect("pack index").len()
    }
    /// Total serialized bytes in the pack file (post-dedup).
    pub fn stored_bytes(&self) -> u64 {
        *self.append.lock().expect("pack append")
    }
    pub fn lru_hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }
    pub fn lru_misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }
}

/// Positional write, cursor-independent (parallel readers never seek this file).
#[cfg(unix)]
fn write_all_at(file: &File, buf: &[u8], offset: u64) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, buf, offset)
}
#[cfg(unix)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}

/// Windows (and any non-unix) fallback: `seek_write`/`seek_read` move the file
/// pointer, but nothing here relies on the pointer — every access is positional.
#[cfg(windows)]
fn write_all_at(file: &File, mut buf: &[u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = file.seek_write(buf, offset)?;
        buf = &buf[n..];
        offset += n as u64;
    }
    Ok(())
}
#[cfg(windows)]
fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = file.seek_read(buf, offset)?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "pack read past end",
            ));
        }
        buf = &mut buf[n..];
        offset += n as u64;
    }
    Ok(())
}

/// Byte-bounded LRU of decoded values, sharded by key for `Sync` access from
/// parallel verify workers (rayon work-stealing has no worker↔key affinity, so
/// key-sharded mutexes beat per-worker caches on both memory and contention).
struct ShardedLru<T> {
    shards: Vec<Mutex<Shard<T>>>,
    shard_budget: u64,
}

struct Shard<T> {
    map: lru::LruCache<u128, (Arc<T>, u64)>,
    bytes: u64,
    /// Largest entry seen: the eviction budget floors at 2× this, so the two
    /// trees of one in-flight verify pair always fit (anti-thrash floor).
    max_entry: u64,
}

impl<T> ShardedLru<T> {
    fn new(total_bytes: u64, shards: usize) -> Self {
        let shards = shards.max(1);
        ShardedLru {
            shard_budget: total_bytes / shards as u64,
            shards: (0..shards)
                .map(|_| {
                    Mutex::new(Shard {
                        map: lru::LruCache::unbounded(),
                        bytes: 0,
                        max_entry: 0,
                    })
                })
                .collect(),
        }
    }

    fn shard(&self, key: u128) -> &Mutex<Shard<T>> {
        &self.shards[(key % self.shards.len() as u128) as usize]
    }

    fn get(&self, key: u128) -> Option<Arc<T>> {
        let mut shard = self.shard(key).lock().expect("lru shard");
        shard.map.get(&key).map(|(v, _)| Arc::clone(v))
    }

    fn insert(&self, key: u128, value: Arc<T>, cost: u64) {
        let mut shard = self.shard(key).lock().expect("lru shard");
        if shard.map.contains(&key) {
            return; // a racing loader beat us; keep the resident entry's recency
        }
        shard.map.push(key, (value, cost));
        shard.bytes += cost;
        shard.max_entry = shard.max_entry.max(cost);
        // Anti-thrash floor: never bound below 2× the largest entry (a verify
        // pair), and always keep at least the entry just inserted.
        let budget = self.shard_budget.max(2 * shard.max_entry);
        while shard.bytes > budget && shard.map.len() > 1 {
            if let Some((_, (_, c))) = shard.map.pop_lru() {
                shard.bytes -= c;
            } else {
                break;
            }
        }
    }
}
