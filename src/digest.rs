//! Fused per-unit digest pass (memory architecture P1: candidate-narrowing runs on
//! resident summaries; details load on demand).
//!
//! Four of the five production `.tree` consumers are pure single-tree functions
//! (exact-tier `boilerplate_mass`, near-tier `RepData` substrate, the sequence-tier
//! token stream, the api-profile call multiset). Computing all four in one pass at
//! unit-creation time — extraction for plain units, the `finish_variant` tail for
//! inline variants — lets every downstream tier read these digests instead of the
//! tree, so trees can spill over the memory gate (near-tier verify, the one true
//! pair-of-trees consumer, materializes through the scan-scoped pack).
//!
//! Invariant (pinned by `tests/digest_oracle.rs`, kept permanently): every field
//! equals the value of the EXISTING per-field function applied to the unit's tree —
//! `ir::substance::boilerplate_mass`, `matchtree::rep_substrate` (the lifted
//! `build_reps` substrate: `fingerprint::subtree_inventory` + pre-order depths),
//! `stream::unit_stream` (the sequence serializer), `api::call_elems`. This module
//! composes those functions; it reimplements nothing.

use crate::config::Config;
use crate::lang::Lang;
use crate::tree::NormNode;
use std::collections::BTreeMap;

/// Everything the single-tree consumers read off one unit, computed once at the
/// unit's creation. Index-aligned with `units` (see [`crate::CorpusUnits`]).
#[derive(Debug, Clone, PartialEq)]
pub struct UnitDigest {
    /// Token mass of recognized boilerplate idioms (`ir::substance`) — the exact
    /// and inline-exact tiers' ranking discount (they read `members[0]` only, but
    /// any unit — plain or variant — can be a bucket representative).
    pub boilerplate_mass: u32,
    /// Near-tier bag layer: floor-`bag_min_subtree_tokens` deduplicated subtree
    /// hashes, sorted ascending (`RepData::bag_set`).
    pub bag_set: Vec<u128>,
    /// Near-tier verify substrate: subtree hash → (pre-order offsets, tree depths),
    /// sorted by hash (`RepData::offsets`).
    pub offsets: crate::matchtree::SubtreeOffsets,
    /// Sequence-tier per-unit token stream (`stream::unit_stream`): one
    /// (content hash, byte span) per node, pre-order. `Absent` for variants —
    /// the sequence tier filters to `variant.is_none()`.
    pub seq_tokens: SeqSlot,
    /// api-profile call multiset (`api::call_elems`): (callee, control context) →
    /// count. `None` for variants — the api tier filters to `variant.is_none()`.
    pub api_elems: Option<BTreeMap<crate::api::Elem, u32>>,
}

/// One unit's sequence stream: (content hash, byte span) per node, pre-order
/// (`stream::unit_stream`'s output shape).
pub type SeqStream = Vec<(u64, (u32, u32))>;

/// A plain unit's sequence stream across the memory gate's residency states —
/// the coarser-grained sibling of [`crate::unit::TreeSlot`] (memory
/// architecture P3): streams are consumed strictly per language partition, so
/// over the gate they spill at digest time and bulk-load per partition (no LRU
/// needed — one load, one use, dropped with the partition).
#[derive(Debug, Clone, PartialEq)]
pub enum SeqSlot {
    /// Variant unit — the sequence tier never reads it.
    Absent,
    Resident(Vec<(u64, (u32, u32))>),
    /// Exact-content key into the scan-scoped seq pack.
    Spilled(u128),
}

impl SeqSlot {
    /// The stream, when resident.
    pub fn resident(&self) -> Option<&[(u64, (u32, u32))]> {
        match self {
            SeqSlot::Resident(v) => Some(v),
            _ => None,
        }
    }
}

/// The fused pass: one digest from one canonical tree. `plain` is
/// `unit.variant.is_none()` — variants skip the sequence/api fields entirely
/// (those tiers never read variants).
pub fn compute(
    tree: &NormNode,
    lang: Lang,
    cfg: &Config,
    plain: bool,
    li: &crate::intern::LabelInterner,
) -> UnitDigest {
    let (bag_set, offsets) = crate::matchtree::rep_substrate(tree, cfg, li);
    UnitDigest {
        boilerplate_mass: crate::ir::substance::boilerplate_mass(tree, li),
        bag_set,
        offsets,
        seq_tokens: if plain {
            SeqSlot::Resident(crate::stream::unit_stream(tree, li))
        } else {
            SeqSlot::Absent
        },
        api_elems: plain.then(|| crate::api::call_elems(tree, lang, cfg, li)),
    }
}
