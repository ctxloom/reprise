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
//!   (`tempfile::tempfile_in(dir)`, unlinked at creation — the pack cannot
//!   outlive the scan even on abnormal exit), `pread` at offset on miss. `dir`
//!   is scan-root-relative by default (`resolve_pack_dir`), never the process
//!   temp dir: on many systems `/tmp` is a RAM-backed tmpfs, so spilling
//!   "to disk" there both risks ENOSPC on a large scan and keeps the bytes in
//!   RAM — defeating the point of spilling.
//! - **Residency**: a `HashMap<key, (offset, len)>` index (tens of bytes per
//!   entry) plus a byte-bounded, sharded, `Sync` LRU of decoded `Arc<T>` values.
//!   Eviction only drops the cache's own `Arc`; a caller-held `Arc` from an
//!   in-flight verify stays valid — Rust ownership, not cache policy, is the
//!   correctness mechanism. The byte bound floors at 2× the largest entry per
//!   shard so a verify PAIR can't thrash (a performance floor, not a safety one).
//! - **Failure**: a write failure (e.g. the backing volume fills) is NEVER a
//!   panic. The first I/O error on [`Pack::store`] latches a poisoned-state
//!   flag ([`PackStoreError`], held in a `OnceLock`) so every subsequent
//!   `store` fails fast with the SAME clean error instead of retrying a
//!   doomed write or touching a poisoned `Mutex` — the caller propagates it
//!   through the normal `anyhow` error path (no cascade, no silent exit).
//!
//! The pack is one generic mechanism: `Pack<NormNode>` for trees (plain trees
//! spill at gate-trip, variant trees at `finish_variant` — same store, P3
//! dissolves the "variants aren't cached" fork) and `Pack<Vec<…>>` for sequence
//! streams. Nothing here touches the durable D19 cache or its format.

use serde::Serialize;
use serde::de::DeserializeOwned;

/// The pluggable value decoder (see the struct doc).
type Decoder<T> = Box<dyn Fn(&[u8]) -> T + Send + Sync>;
/// The pluggable value encoder (see the struct doc) — the write-side counterpart of
/// [`Decoder`], needed for the same reason: post-interning, a `NormNode`'s
/// `Label::External`/`LitKept` ids resolve only through the scan's `LabelInterner`
/// (`NormNode::to_wire`), so `T: Serialize` alone can no longer produce the bytes —
/// the encoder closure captures that context instead (see [`Pack::with_codec`]).
type Encoder<T> = Box<dyn Fn(&T) -> Vec<u8> + Send + Sync>;
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

/// A content-addressed store of serialized `T` values in one scan-scoped temp
/// file, fronted by a byte-bounded sharded LRU of decoded values.
///
/// Encoding/decoding are pluggable ([`Pack::with_codec`]) because not every payload
/// can (de)serialize context-free: post-interning, a `NormNode`'s labels resolve
/// through the CURRENT scan's `LabelInterner` in both directions (`NormNode::
/// to_wire` / `NormNodeWire::into_real`) — the pack is scan-scoped, so capturing
/// that scan's interner in the codec is exactly right. Plain-data payloads (already
/// `Serialize + DeserializeOwned` with no external context) use [`Pack::new`].
pub struct Pack<T> {
    file: File,
    /// The directory the backing file lives under (or, for [`Pack::over_file`]
    /// test injection, a caller-supplied label) — surfaced in [`PackStoreError`]
    /// so a write failure names an actionable remedy.
    dir: PathBuf,
    /// key → (offset, len) for every stored value. Read-mostly.
    index: RwLock<HashMap<u128, (u64, u32)>>,
    /// Append cursor; also serializes writers (index double-check under this lock).
    append: Mutex<u64>,
    /// Set on the FIRST write failure; every later `store` returns this same
    /// error immediately instead of re-attempting a doomed write or ever
    /// touching a poisoned `Mutex` (we never panic while `append` is held).
    failure: OnceLock<PackStoreError>,
    lru: ShardedLru<T>,
    hits: AtomicU64,
    misses: AtomicU64,
    encode: Encoder<T>,
    decode: Decoder<T>,
}

/// A pack write failure — clean, never a panic (see the module doc's
/// "Failure" bullet). Carries enough to make the scan's abort message
/// actionable: where the pack lives, how much it had written before the
/// failure, and the remedy (`[memory] pack_dir`).
#[derive(Debug, Clone)]
pub struct PackStoreError {
    pub dir: PathBuf,
    pub bytes_written: u64,
    source: String,
}

impl std::fmt::Display for PackStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "pack write failed after {} bytes written (pack dir: {}): {} — if this directory is \
             small or RAM-backed (e.g. a tmpfs /tmp), point the pack at real disk with \
             `[memory] pack_dir` in reprise.toml",
            self.bytes_written,
            self.dir.display(),
            self.source,
        )
    }
}

impl std::error::Error for PackStoreError {}

impl<T: Serialize + DeserializeOwned + Send + Sync> Pack<T> {
    /// `lru_bytes` bounds the decoded-value cache (split across `shards`; each
    /// shard floors at 2× its largest entry so a verify pair always fits);
    /// `shards` bounds lock contention from parallel verify workers. `dir` is
    /// the pack's backing directory (see `resolve_pack_dir`) — the file itself
    /// is anonymous (`tempfile_in`), `dir` only chooses its volume.
    pub fn new(lru_bytes: u64, shards: usize, dir: &Path) -> std::io::Result<Self> {
        Self::with_codec(
            lru_bytes,
            shards,
            dir,
            |v| bincode::serialize(v).expect("pack value serializes"),
            |bytes| bincode::deserialize(bytes).expect("pack value decodes"),
        )
    }

    /// Test/diagnostic hook: build a pack directly over an already-open file,
    /// bypassing `tempfile_in` entirely — lets tests exercise the store-failure
    /// path deterministically (e.g. against `/dev/full`, which always errors
    /// ENOSPC on write) without needing a real full filesystem. `dir` is used
    /// only for the error message (see [`PackStoreError`]).
    pub fn over_file(file: File, dir: impl Into<PathBuf>, lru_bytes: u64, shards: usize) -> Self {
        Self::over_file_with_codec(
            file,
            dir,
            lru_bytes,
            shards,
            |v| bincode::serialize(v).expect("pack value serializes"),
            |bytes| bincode::deserialize(bytes).expect("pack value decodes"),
        )
    }
}

impl<T: Send + Sync> Pack<T> {
    /// [`Pack::new`]/[`Pack::over_file`] with explicit encode/decode closures —
    /// for payloads whose (de)serialization needs context a derive cannot reach
    /// (the interned-tree wire path: `NormNode::to_wire`/`NormNodeWire::into_real`
    /// against the current scan's `LabelInterner`).
    pub fn with_codec(
        lru_bytes: u64,
        shards: usize,
        dir: &Path,
        encode: impl Fn(&T) -> Vec<u8> + Send + Sync + 'static,
        decode: impl Fn(&[u8]) -> T + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        let file = tempfile::tempfile_in(dir)?;
        Ok(Self::from_parts(
            file,
            dir.to_path_buf(),
            lru_bytes,
            shards,
            encode,
            decode,
        ))
    }

    /// [`Pack::over_file`] with explicit codec (see [`Pack::with_codec`]).
    pub fn over_file_with_codec(
        file: File,
        dir: impl Into<PathBuf>,
        lru_bytes: u64,
        shards: usize,
        encode: impl Fn(&T) -> Vec<u8> + Send + Sync + 'static,
        decode: impl Fn(&[u8]) -> T + Send + Sync + 'static,
    ) -> Self {
        Self::from_parts(file, dir.into(), lru_bytes, shards, encode, decode)
    }

    fn from_parts(
        file: File,
        dir: PathBuf,
        lru_bytes: u64,
        shards: usize,
        encode: impl Fn(&T) -> Vec<u8> + Send + Sync + 'static,
        decode: impl Fn(&[u8]) -> T + Send + Sync + 'static,
    ) -> Self {
        Pack {
            file,
            dir,
            index: RwLock::new(HashMap::new()),
            append: Mutex::new(0),
            failure: OnceLock::new(),
            lru: ShardedLru::new(lru_bytes, shards),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            encode: Box::new(encode),
            decode: Box::new(decode),
        }
    }

    /// Serialize, key by exact content, and store (byte-identical values dedup
    /// to the existing entry). Returns the pack key, or a latched
    /// [`PackStoreError`] on write failure (clean — never a panic; see the
    /// module doc's "Failure" bullet).
    pub fn store(&self, value: &T) -> Result<u128, PackStoreError> {
        if let Some(err) = self.failure.get() {
            return Err(err.clone()); // already doomed — fail fast, no I/O retry
        }
        let bytes = (self.encode)(value);
        let key = xxhash_rust::xxh3::xxh3_128(&bytes);
        if self.index.read().expect("pack index").contains_key(&key) {
            return Ok(key); // dedup: same bytes, same key, stored once
        }
        let mut end = self.append.lock().expect("pack append");
        // Double-check under the append lock: a racing writer may have stored
        // the same bytes (or latched a failure) between the checks above and
        // taking this lock.
        if let Some(err) = self.failure.get() {
            return Err(err.clone());
        }
        if self.index.read().expect("pack index").contains_key(&key) {
            return Ok(key);
        }
        let offset = *end;
        if let Err(e) = write_all_at(&self.file, &bytes, offset) {
            let err = PackStoreError {
                dir: self.dir.clone(),
                bytes_written: offset,
                source: e.to_string(),
            };
            // Best-effort latch: if a racing writer set it first, reuse theirs
            // (both describe the same doomed pack).
            let _ = self.failure.set(err.clone());
            return Err(self.failure.get().cloned().unwrap_or(err));
        }
        *end += bytes.len() as u64;
        self.index
            .write()
            .expect("pack index")
            .insert(key, (offset, bytes.len() as u32));
        Ok(key)
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

/// Where a scan's pack should resolve, and (if the default location was
/// unreachable) the warning to print before the scan proceeds.
#[derive(Debug, Clone)]
pub struct ResolvedPackDir {
    pub dir: PathBuf,
    /// `Some` exactly when the default `<root>/.reprise/tmp/` directory could
    /// not be created (typically a read-only scan root) AND no `pack_dir`
    /// override was configured — the caller should print this to stderr
    /// once, before the scan proceeds on the `std::env::temp_dir()` fallback.
    pub warning: Option<String>,
    /// True exactly when `dir` is the scan-owned default (`<root>/.reprise/tmp`)
    /// — never an explicit `pack_dir` override, never the `temp_dir()`
    /// fallback (neither is ours to remove). The caller should best-effort
    /// `remove_dir` it once the scan's packs are done (it will only actually
    /// disappear if empty), so a scan never leaves scratch directories behind
    /// in a scanned repo — unlike the durable `.reprise/cache`, this dir is
    /// pure scan-scoped scratch space.
    pub owned: bool,
}

/// Resolve the scan-scoped pack's backing directory (the fix for the
/// tmpfs-ENOSPC failure mode: `src/pack.rs`'s previous `tempfile::tempfile()`
/// always used the process temp dir, which on many systems is a RAM-backed
/// tmpfs — spilling "to disk" there both risks ENOSPC on a large scan and
/// keeps the bytes in RAM, defeating the point of spilling).
///
/// Mirrors the D19 cache's root convention (`crate::cache`): same
/// scan-root-relative default, same `.reprise/` volume. Unlike the cache
/// (which degrades silently to a cold scan on a read-only root — a cache is
/// optional), a pack MUST have somewhere to write, so an unwritable root
/// falls back to `std::env::temp_dir()` with a NAMED warning instead of
/// silence.
///
/// - `override_dir` (`cfg.memory.pack_dir`) wins unconditionally when set —
///   trusted as-is; if it turns out unwritable, `Pack::new`/`with_decoder`
///   surfaces that as a clean `io::Error` (never a silent fallback of an
///   explicit setting).
/// - Otherwise `<root>/.reprise/tmp/` is created and used.
/// - If that creation fails (unwritable root, no override), fall back to
///   `std::env::temp_dir()` and return a warning naming the risk.
pub fn resolve_pack_dir(root: &Path, override_dir: Option<&Path>) -> ResolvedPackDir {
    if let Some(dir) = override_dir {
        return ResolvedPackDir {
            dir: dir.to_path_buf(),
            warning: None,
            owned: false,
        };
    }
    let default_dir = root.join(".reprise").join("tmp");
    if std::fs::create_dir_all(&default_dir).is_ok() {
        return ResolvedPackDir {
            dir: default_dir,
            warning: None,
            owned: true,
        };
    }
    let fallback = std::env::temp_dir();
    let warning = format!(
        "pack falling back to {}; if this is tmpfs, spilled trees still occupy RAM and large \
         scans may fail with ENOSPC — set [memory] pack_dir",
        fallback.display(),
    );
    ResolvedPackDir {
        dir: fallback,
        warning: Some(warning),
        owned: false,
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
