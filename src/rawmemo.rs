//! WP-D (raw-trees elimination, session `woozy-uncut-comic`): a budget-bounded,
//! read-through memo over the D19 cache, replacing `CorpusUnits`'s corpus-wide
//! resident `Vec<NormNode>` (`raw_trees` — 38% of extraction's peak) for the
//! inliner's callee/caller tree fetches.
//!
//! Not the `SpillStore`/`Pack` seam (`src/store.rs`, `src/pack.rs`): those exist
//! for values a scan is *about to write* into a scan-scoped store at birth. Raw
//! trees are never born needing that decision — by the time `corpus_units_from`
//! has one in hand it is already durably written (or read back) via the D19
//! cache (`src/cache.rs`). The residency question here is "read it in, drop it,
//! maybe read it again," not "where do I first put this" — so the backing store
//! is the D19 cache itself, read through [`crate::cache::load`], with a
//! re-extract fallback (`crate::unit::extract_file_units_keep_raw`) for cache-off
//! or a rare mid-scan eviction.

use crate::intern::LabelInterner;
use crate::lang::Lang;
use crate::tree::NormNode;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// One file's retained coordinates for the raw-tree memo: everything needed to
/// rehydrate that file's `Vec<NormNode>` on demand. Extraction order ==
/// the order `FileUnits::raw_trees` was built in == the order this file's units
/// occupy in `CorpusUnits.units[unit_start..unit_start+unit_count]`.
#[derive(Debug, Clone)]
pub struct RawFileKey {
    pub file: PathBuf,
    pub lang: Lang,
    /// D19 cache key (`cache::key`), already computed at extraction — retained
    /// instead of discarded.
    pub cache_key: u128,
    pub unit_start: u32,
    pub unit_count: u32,
}

/// Approximate resident bytes of one file's decoded raw trees — `NormNode` has
/// no `size_of` today, so this approximates via `token_count()` × the SAME
/// per-token constant `memory.rs` already uses to estimate canonical-tree
/// residency (`memory::PER_TOKEN_TREE_BYTES`) — consistent with, not a second
/// independent guess at, the rest of the memory model.
fn estimate_bytes(trees: &[NormNode]) -> u64 {
    trees
        .iter()
        .map(|t| u64::from(t.token_count()) * crate::memory::PER_TOKEN_TREE_BYTES)
        .sum()
}

/// The LRU + its running byte total, behind ONE lock (never two locks that
/// could be taken out of order) — `insert`'s dedup-then-evict sequence needs
/// both consistent with each other at every step.
struct MemoState {
    lru: lru::LruCache<u128, Arc<Vec<NormNode>>>,
    used_bytes: u64,
}

/// Budget-bounded, file-granularity, read-through cache over the D19 cache
/// (module doc) with a re-lower fallback, reading through a [`crate::source::
/// ContentSource`] so `check`'s `GitSource` never touches `std::fs` directly.
/// Construction borrows everything it needs for the rest of the inline
/// phase's lifetime — this is built fresh inside `scan_source`'s inline block
/// and dropped at the end of it, the same lifetime the old `raw_trees` Vec
/// had.
pub struct RawTreeMemo<'s> {
    files: &'s [RawFileKey],
    /// Per PLAIN unit, which `files` entry it belongs to — kept INSIDE the
    /// memo (not a second parameter every caller must thread) so both
    /// `expand_unit`'s caller fetch and `splice_body`'s callee fetch share one
    /// `unit(unit_idx)` lookup path knowing only a unit index, exactly what
    /// both already have in hand.
    unit_file_idx: &'s [u32],
    source: &'s dyn crate::source::ContentSource,
    source_digests: &'s HashMap<PathBuf, u128>,
    cache_root: PathBuf,
    cfg: &'s crate::config::Config,
    li: Arc<LabelInterner>,
    state: Mutex<MemoState>,
    /// `None` = unbounded (P2's "under budget, zero new work" — every
    /// rehydrated file just stays resident for the rest of the phase).
    /// `Some(n)` = evict LRU-first past `n` approximate bytes.
    budget: Option<u64>,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl<'s> RawTreeMemo<'s> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        files: &'s [RawFileKey],
        unit_file_idx: &'s [u32],
        source: &'s dyn crate::source::ContentSource,
        source_digests: &'s HashMap<PathBuf, u128>,
        cache_root: PathBuf,
        cfg: &'s crate::config::Config,
        li: Arc<LabelInterner>,
        budget: Option<u64>,
    ) -> Self {
        RawTreeMemo {
            files,
            unit_file_idx,
            source,
            source_digests,
            cache_root,
            cfg,
            li,
            state: Mutex::new(MemoState {
                lru: lru::LruCache::unbounded(),
                used_bytes: 0,
            }),
            budget,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// One unit's raw tree, cloned out of its file's cached `Vec<NormNode>`.
    #[track_caller]
    pub fn unit(&self, unit_idx: usize) -> NormNode {
        let file_idx = self.unit_file_idx[unit_idx];
        let key = &self.files[file_idx as usize];
        let trees = self.file(file_idx);
        trees[unit_idx - key.unit_start as usize].clone()
    }

    fn file(&self, file_idx: u32) -> Arc<Vec<NormNode>> {
        let key = &self.files[file_idx as usize];
        {
            let mut state = self.state.lock().expect("raw-tree memo state");
            if let Some(hit) = state.lru.get(&key.cache_key) {
                self.hits.fetch_add(1, Ordering::Relaxed);
                return Arc::clone(hit);
            }
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        let trees = self.rehydrate(key);
        let arc = Arc::new(trees);
        self.insert(key.cache_key, Arc::clone(&arc));
        arc
    }

    /// D19 cache hit, else re-lower from source through the SAME
    /// `ContentSource` the scan is reading (so `check`'s `GitSource` never
    /// touches `std::fs` directly), verified against the digest captured at
    /// extraction — a mismatch is a mid-scan edit with caching disabled, and
    /// it fails LOUDLY (a scan cannot reproduce a consistent result over
    /// content that changed underneath it).
    fn rehydrate(&self, key: &RawFileKey) -> Vec<NormNode> {
        if self.cfg.cache.enabled
            && let Some(fu) =
                crate::cache::load(&self.cache_root, key.cache_key, &key.file, &self.li)
        {
            return fu.raw_trees;
        }
        let src = self.source.read(&key.file).unwrap_or_else(|| {
            panic!(
                "reprise: {} was readable at extraction but is not now — the raw-tree \
                 memo cannot rehydrate it",
                key.file.display()
            )
        });
        let digest = xxhash_rust::xxh3::xxh3_128(src.as_bytes());
        let expected = self.source_digests.get(&key.file).copied();
        assert_eq!(
            Some(digest),
            expected,
            "reprise: {} changed on disk mid-scan with caching disabled — the raw-tree \
             memo cannot reproduce a consistent scan; re-run the scan",
            key.file.display(),
        );
        crate::unit::extract_file_units_keep_raw(&key.file, &src, key.lang, self.cfg, &self.li)
            .raw_trees
    }

    /// Insert, then evict LRU-first back under budget (anti-thrash floor:
    /// never below one entry, mirrors `pack.rs`'s `ShardedLru::insert`). A
    /// racing loader that beat us to the same key is left alone — its entry's
    /// recency is at least as good as ours, and re-inserting identical
    /// content (extraction is deterministic) would be pure waste.
    fn insert(&self, key: u128, value: Arc<Vec<NormNode>>) {
        let mut state = self.state.lock().expect("raw-tree memo state");
        if state.lru.contains(&key) {
            return;
        }
        let bytes = estimate_bytes(&value);
        state.lru.push(key, value);
        state.used_bytes += bytes;
        let Some(budget) = self.budget else {
            return; // unbounded: P2's "under budget, zero new work"
        };
        while state.used_bytes > budget && state.lru.len() > 1 {
            let Some((_, evicted)) = state.lru.pop_lru() else {
                break;
            };
            state.used_bytes = state.used_bytes.saturating_sub(estimate_bytes(&evicted));
        }
    }

    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }
    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }
}
