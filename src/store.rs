//! Where a value lives — the counterpart to [`crate::source`]'s "where do the
//! bytes come from".
//!
//! Extraction used to hand its trees straight into plain `Vec`s (`lib.rs`), and
//! the decision to spill was a one-shot branch inside `scan()` at a phase
//! boundary. Nothing sat between the producer and the storage able to say *"not
//! this one, not now"* — so the guard could only fire **after** extraction had
//! already built everything it might have refused, and only for the one
//! structure someone had wired it into.
//!
//! This is that missing seam, and it is an object with named methods rather than
//! a branch: [`SpillStore::admit`] takes a value at its birth and returns a
//! [`Slot`] saying where it now lives; [`SpillStore::get`] reads it back without
//! the caller knowing or caring. Both are generic, because the residency
//! question is the same one for every large value a scan holds — [`crate::pack::Pack`]
//! was already generic, and only the *slot* was welded to `NormNode`.
//!
//! **The invariant everything rests on: a reader cannot tell where a value
//! lived.** Residency is a memory decision, and a memory decision must never
//! change what reprise reports. That is what makes a *non-deterministic*,
//! pressure-driven guard admissible: the choice is among provably equivalent
//! paths — byte-identity constrains the PATHS, not the TIMING.

use crate::pack::{Pack, PackStoreError};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::path::Path;
use std::sync::Arc;

/// Where a value lives. `Spilled` carries the pack's exact-content key (an
/// xxh3-128 of the serialized bytes — never a masked matching hash).
#[derive(Debug, Clone, PartialEq)]
pub enum Slot<T> {
    Resident(T),
    Spilled(u128),
}

impl<T> Slot<T> {
    /// The value, when resident.
    pub fn resident(&self) -> Option<&T> {
        match self {
            Slot::Resident(v) => Some(v),
            Slot::Spilled(_) => None,
        }
    }

    /// The value, for callers that run strictly before any spill can have
    /// happened. Panics on a spilled slot: reaching one here is a
    /// pipeline-ordering bug, not a recoverable state.
    #[track_caller]
    pub fn expect_resident(&self) -> &T {
        self.resident()
            .expect("value was spilled before a resident-only consumer read it")
    }

    /// Owning variant of [`Self::expect_resident`].
    #[track_caller]
    pub fn into_resident(self) -> T {
        match self {
            Slot::Resident(v) => v,
            Slot::Spilled(_) => panic!("value was spilled; no resident value to take"),
        }
    }
}

impl<T: Send + Sync> Slot<T> {
    /// The value, wherever it lives: borrowed when resident (**zero new work** —
    /// the calm path pays nothing), or materialized through the pack's LRU when
    /// spilled. Panics if a spilled slot meets no pack: the pack is created in
    /// the same branch that spills, so that pairing is a wiring invariant.
    #[track_caller]
    pub fn get<'a>(&'a self, pack: Option<&Pack<T>>) -> Ref<'a, T> {
        match self {
            Slot::Resident(v) => Ref::Borrowed(v),
            Slot::Spilled(key) => Ref::Loaded(
                pack.expect("spilled value but no pack — guard wiring bug")
                    .load(*key),
            ),
        }
    }
}

/// A materialized value: a plain borrow (resident) or a shared handle out of the
/// pack's LRU (spilled). `Deref`s either way, so consumers are residency-blind.
///
/// Holding a `Loaded` handle keeps the decoded value alive **regardless of LRU
/// eviction** — near-tier verify holds two at once while the LRU may evict
/// either, and a handle invalidated underneath a reader would be a use-after-free
/// in safe clothing.
pub enum Ref<'a, T> {
    Borrowed(&'a T),
    Loaded(Arc<T>),
}

impl<T> std::ops::Deref for Ref<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        match self {
            Ref::Borrowed(v) => v,
            Ref::Loaded(a) => a,
        }
    }
}

/// The seam: it owns the pack, and it owns the **decision**.
///
/// `admit` is asked **once per value, at the value's birth** — never per node. A
/// scan builds hundreds of millions of nodes and hundreds of thousands of units,
/// so the decision must ride the coarse boundary or the guard costs more than it
/// saves.
pub struct SpillStore<T> {
    pack: Pack<T>,
    /// Whether `admit` spills. Today `scan()` sets this once, from the Gate-2
    /// decision, reproducing exactly the previous behavior: over the gate every
    /// value spills, under it the store is never built at all.
    ///
    /// **This flag is the seam the reactive guard turns on.** When admission
    /// becomes pressure-driven it becomes a live reading rather than a constant,
    /// and *no call site changes* — only what `admit` decides.
    admitting: bool,
}

impl<T: Serialize + DeserializeOwned + Send + Sync> SpillStore<T> {
    /// A store over plain-data payloads. `NormNode` cannot come this way — its
    /// interned ids need the scan's `LabelInterner` to serialize, which is
    /// exactly why `Pack::with_codec` exists — so it arrives via
    /// [`Self::with_pack`]. That split is the pack's, and the store inherits it.
    pub fn new(lru_bytes: u64, shards: usize, dir: &Path) -> std::io::Result<Self> {
        Ok(Self::with_pack(Pack::new(lru_bytes, shards, dir)?))
    }
}

impl<T: Send + Sync> SpillStore<T> {
    /// A store over an already-built pack (the `with_codec` path: payloads whose
    /// encoding needs scan context).
    pub fn with_pack(pack: Pack<T>) -> Self {
        SpillStore {
            pack,
            admitting: true,
        }
    }

    /// Whether this store spills what it is given.
    pub fn set_admitting(&mut self, admitting: bool) {
        self.admitting = admitting;
    }

    /// **The guard.** Take a value at its birth; decide where it lives.
    ///
    /// `&self`, not `&mut self`: extraction is rayon-parallel and admits from
    /// every worker at once. The pack is already `Sync` and content-addressed, so
    /// concurrent admissions of equal values collapse to one stored copy.
    pub fn admit(&self, value: T) -> Result<Slot<T>, PackStoreError> {
        if !self.admitting {
            return Ok(Slot::Resident(value));
        }
        let key = self.pack.store(&value)?;
        Ok(Slot::Spilled(key))
    }

    /// Read a slot back, wherever it lives.
    pub fn get<'a>(&self, slot: &'a Slot<T>) -> Ref<'a, T> {
        slot.get(Some(&self.pack))
    }

    /// The underlying pack — for the stats surface and for consumers that hold a
    /// `Slot` without the store in hand.
    pub fn pack(&self) -> &Pack<T> {
        &self.pack
    }

    /// Distinct values stored (content-addressed, so equal values count once).
    pub fn entries(&self) -> usize {
        self.pack.entries()
    }
}

#[cfg(test)]
#[path = "store.test.rs"]
mod store_tests;
