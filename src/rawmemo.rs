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

use crate::lang::Lang;
use std::path::PathBuf;

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
