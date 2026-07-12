//! P7 tree tier (spec §5.6): candidate retrieval (§5.5.4 landmark pairs — the §7.4(b)
//! rivalry winner), offset-histogram verification, then anti-unification.
//!
//! Retrieval is the landmark retriever ALONE: **one candidate set, never a union.**
//! `bag_set` is NOT a retrieval layer — it is a persisted `UnitDigest` field consumed
//! by the retrieval bake-off's comparison retrievers (`examples/bakeoff.rs`), not by
//! [`Landmark`] itself. Every offset-sorted subtree peak is admitted into the landmark
//! constellation; there is no document-frequency gate on peaks (D51 — the historical
//! rarity gate never bound in practice and is deleted, not tuned).
//!
//! Phase 3: inline-expanded variants
//! (spec §5.4) enter retrieval alongside plain units; a verified pair
//! involving a variant becomes tier `inline-assisted`, reported against the
//! variants' base units, with the D3 tautology filter applied.

use crate::au::{self, AuOutcome};
use crate::config::Config;
use crate::fingerprint::{self, HashMode, Subtree};
use crate::lang::Lang;
use crate::report::Tier;
use crate::unit::{Unit, base_of};
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub struct NearGroup {
    /// Base-resolved member unit indices (a variant reports as its base).
    pub member_units: Vec<usize>,
    pub tier: Tier,
    pub template: String,
    /// Merkle hash of the group template tree — the stable baseline key for
    /// near/inline groups (spec §2/§6.1 `partialFingerprints`).
    pub template_hash: u128,
    pub template_tokens: u32,
    /// Seam C: token mass of recognized boilerplate idioms in the AU template
    /// (`ir::substance`). Discounts the ranking value only — hash-neutral.
    pub template_boilerplate: u32,
    pub divergence: f64,
    /// Inline chains of the participating variants (InlineAssisted only).
    pub inline_chains: Vec<String>,
}

#[derive(Default, Debug, Clone, serde::Serialize)]
pub struct RetrievalStats {
    // With a single retriever, `candidates_total == candidates_landmark` and "verified
    // only by landmark" == `verified_pairs`, both BY CONSTRUCTION — so neither is a
    // counter worth carrying.
    //
    // INVARIANT: a second candidate layer, if one is ever added here, MUST ship with a
    // unique-yield counter. A layer whose marginal contribution over the layers it runs
    // beside is not a live, per-scan number cannot be shown to earn its keep — and an
    // always-on join that yields nothing is indistinguishable from one that yields
    // everything until someone counts.
    pub candidates_landmark: usize,
    /// Landmark candidates dropped by the coverage-fraction gate (§0.3): the
    /// recall-neutral pre-AU flood cut. Reported so the gate's marginal effect is
    /// a live number, not an assumption.
    pub candidates_landmark_coverage_gated: usize,
    pub candidates_total: usize,
    /// Filter-cascade rejections, split per filter so the funnel decomposes (the
    /// short-circuiting AND attributes each reject to the FIRST filter that failed).
    pub offset_histogram_rejected: usize,
    pub htree_rejected: usize,
    /// Pairs that cleared every filter but exceeded `max_divergence` at anti_unify.
    pub divergence_rejected: usize,
    /// Pairs that cleared the filter cascade AND the divergence gate (counted at the
    /// one point — pre-acceptance — for both the plain and the inline path, so it is
    /// the true verify-survivor count, not the post-acceptance subset).
    pub verified_pairs: usize,
    /// Total constellation hashes across every unit — the landmark index's size, and the
    /// largest memory row in a large scan. Directly governed by `retrieval.landmark_fan_out`.
    pub landmark_index_size: usize,
}

/// Per-unit retrieval substrate, shared by every `Retriever` (candidate
/// generation) and the verify chain. Retriever-specific indexes (e.g. the
/// landmark constellation hashes) are built inside the retriever from this
/// substrate, not stored here — the substrate stays neutral.
///
/// Since the fused digest pass (`crate::digest`), the substrate itself lives in
/// each unit's [`crate::digest::UnitDigest`] (computed once, at unit creation);
/// a `RepData` BORROWS it rather than recomputing from the tree — near tier no
/// longer reads `Unit::tree` outside `anti_unify`, and building reps costs a
/// pointer copy, not a walk.
pub struct RepData<'d> {
    unit_idx: usize,
    bag_set: std::borrow::Cow<'d, [u128]>,
    /// Subtree hash → (pre-order offsets, tree depths), the two parallel per
    /// hash and in ascending-offset order, sorted by hash: the verify cascade
    /// intersects two of these by a single linear merge, no hashing (M3b/D22 —
    /// the per-candidate HashMap probes dominated 500k-LOC scans). Offsets feed
    /// the Shazam offset-delta diagonal; depths feed the H-tree-verify depth-delta
    /// criterion (§0.5) — both derived from the one shared-subtree evidence pass.
    offsets: std::borrow::Cow<'d, [SubtreeOffsetsEntry]>,
}

impl<'d> RepData<'d> {
    /// Index into the `units` slice this rep was built from.
    pub fn unit_idx(&self) -> usize {
        self.unit_idx
    }
    /// Floor-`bag_min_subtree_tokens` deduplicated subtree-hash set, sorted ascending.
    /// Not consumed by [`Landmark`] (which reads `offsets` directly); available to any
    /// `Retriever` building its own index, and to the bake-off's comparison retrievers.
    /// Not itself a retrieval layer.
    pub fn bag_set(&self) -> &[u128] {
        &self.bag_set
    }
    /// Hash → (pre-order offsets, tree depths) inventory, sorted by hash, the two
    /// parallel per hash. The verify substrate; also the source a retriever derives
    /// rare peaks / an ordered subtree stream from.
    pub fn offsets(&self) -> &[(u128, Vec<u32>, Vec<u16>)] {
        &self.offsets
    }
}

/// A histogram-and-AU-verified pair, keyed on unit indices.
struct VerifiedPair {
    a: usize,
    b: usize,
    outcome: AuOutcome,
    chains: Vec<String>,
}

pub fn find_near_groups(
    units: &[Unit],
    digests: Option<&[crate::digest::UnitDigest]>,
    cfg: &Config,
    stats: &mut RetrievalStats,
    exact_pairs: &HashSet<(usize, usize)>,
    tree_pack: Option<&crate::pack::Pack<crate::tree::NormNode>>,
    li: &crate::intern::LabelInterner,
) -> Vec<NearGroup> {
    let mut groups = Vec::new();
    let langs: HashSet<Lang> = units.iter().map(|u| u.lang).collect();
    let mut langs: Vec<Lang> = langs.into_iter().collect();
    langs.sort();
    for lang in langs {
        groups.extend(near_groups_for_lang(
            units,
            digests,
            lang,
            cfg,
            stats,
            exact_pairs,
            tree_pack,
            li,
        ));
    }
    groups
}

/// Build the shared per-unit retrieval substrate for one language: one [`RepData`]
/// per eligible unit (token floor met, one representative per exact fingerprint).
/// This is the neutral input every [`Retriever`] and the verify chain read; it is
/// public so the retrieval bake-off races the REAL substrate + real retrievers,
/// not a reconstruction. Returns fewer than 2 reps when there is nothing to pair.
/// Over the memory gate the substrate is BORROWED from the units' fused digests
/// (index-aligned with `units`, carrying [`rep_substrate`] computed at digest
/// time — trees may already be spilled); under the gate there are no digests and
/// the substrate is computed here from the resident trees, exactly as before the
/// memory work (same [`rep_substrate`], zero new work for the common case — P2).
pub fn build_reps<'d>(
    units: &[Unit],
    digests: Option<&'d [crate::digest::UnitDigest]>,
    lang: Lang,
    cfg: &Config,
    li: &crate::intern::LabelInterner,
) -> Vec<RepData<'d>> {
    let floor = cfg.min_unit_floor();
    // One representative per exact fingerprint (exact tiers own equal units;
    // plain units precede variants in `units`, so a variant that exactly
    // matches a plain unit defers to the inline-exact grouping).
    let mut seen_fp = HashSet::new();
    let eligible: Vec<usize> = units
        .iter()
        .enumerate()
        .filter(|(_, u)| u.lang == lang && u.token_count >= floor)
        .filter(|(_, u)| seen_fp.insert(u.fingerprint))
        .map(|(idx, _)| idx)
        .collect();
    match digests {
        Some(digests) => eligible
            .into_iter()
            .map(|idx| RepData {
                unit_idx: idx,
                bag_set: std::borrow::Cow::Borrowed(&digests[idx].bag_set),
                offsets: std::borrow::Cow::Borrowed(&digests[idx].offsets),
            })
            .collect(),
        None => eligible
            .par_iter()
            .map(|&idx| {
                let (bag_set, offsets) = rep_substrate(units[idx].tree.expect_resident(), cfg, li);
                RepData {
                    unit_idx: idx,
                    bag_set: std::borrow::Cow::Owned(bag_set),
                    offsets: std::borrow::Cow::Owned(offsets),
                }
            })
            .collect(),
    }
}

/// One shared-subtree entry: (hash, pre-order offsets, tree depths).
pub type SubtreeOffsetsEntry = (u128, Vec<u32>, Vec<u16>);

/// Subtree hash → (pre-order offsets, tree depths) inventory, sorted by hash —
/// the verify-substrate half of a rep (see [`RepData::offsets`]).
pub type SubtreeOffsets = Vec<SubtreeOffsetsEntry>;

/// One unit's retrieval substrate — the (bag_set, offsets) pair [`RepData`] carries —
/// from its canonical tree. Lifted verbatim out of [`build_reps`] so the fused digest
/// pass ([`crate::digest`]) and `build_reps` share the ONE implementation (the digest
/// invariant: existing functions, never a reimplementation).
pub fn rep_substrate(
    tree: &crate::tree::NormNode,
    cfg: &Config,
    li: &crate::intern::LabelInterner,
) -> (Vec<u128>, SubtreeOffsets) {
    // Histogram offsets use a finer inventory (floor 3) than `bag_set`
    // (small units would otherwise starve the vote count); `bag_set`
    // itself keeps the §9 floor.
    let inv: Vec<Subtree> = fingerprint::subtree_inventory(tree, 3, HashMode::MaskedLocals, li);
    // Pre-order depth per node offset (same numbering as `walk_inventory`:
    // node before children), so each subtree's root depth is `depths[offset]`.
    let depth_by_offset = preorder_depths(tree);
    // (hash, offset, depth) sorted by hash then offset — depth rides the
    // offset it belongs to so the two stay paired through the grouping.
    let mut flat: Vec<(u128, u32, u16)> = inv
        .iter()
        .map(|s| (s.hash, s.offset, depth_by_offset[s.offset as usize]))
        .collect();
    flat.sort_unstable();
    let mut offsets: Vec<(u128, Vec<u32>, Vec<u16>)> = Vec::new();
    for (h, o, d) in flat {
        match offsets.last_mut() {
            Some((last, offs, deps)) if *last == h => {
                offs.push(o);
                deps.push(d);
            }
            _ => offsets.push((h, vec![o], vec![d])),
        }
    }
    let mut bag_set: Vec<u128> = inv
        .iter()
        .filter(|s| s.tokens >= cfg.thresholds.bag_min_subtree_tokens)
        .map(|s| s.hash)
        .collect();
    bag_set.sort_unstable();
    bag_set.dedup();
    (bag_set, offsets)
}

#[allow(clippy::too_many_arguments)]
fn near_groups_for_lang(
    units: &[Unit],
    digests: Option<&[crate::digest::UnitDigest]>,
    lang: Lang,
    cfg: &Config,
    stats: &mut RetrievalStats,
    exact_pairs: &HashSet<(usize, usize)>,
    tree_pack: Option<&crate::pack::Pack<crate::tree::NormNode>>,
    li: &crate::intern::LabelInterner,
) -> Vec<NearGroup> {
    let ir = crate::unit::is_ir(lang, cfg);
    // The historical `LanguageProfile` is anti_unify's structural oracle ONLY on the historical
    // path; the IR path answers those predicates from canonical-IR kinds, so bind a profile
    // solely when `!ir` and pass `None` otherwise (keeps the IR path off `LanguageProfile`).
    let profile = (!ir).then(|| lang.profile());
    let debug_timing = std::env::var_os("REPRISE_TIMING").is_some();
    let mut t = std::time::Instant::now();
    let mut mark = |label: &str| {
        if debug_timing {
            eprintln!("  near[{lang:?}] {label}: {}ms", t.elapsed().as_millis());
        }
        t = std::time::Instant::now();
    };

    let reps = build_reps(units, digests, lang, cfg, li);
    if reps.len() < 2 {
        return Vec::new();
    }

    mark("reps");
    // ---- candidate retrieval (the swappable `Retriever`, default `Landmark`) ----
    // Candidate generation only — the verify chain below is identical for whichever
    // retriever proposes the pairs.
    //
    // ONE candidate set. There is no second layer to union with: the retriever's output IS
    // the candidate set.
    //
    // `bag_set` is NOT a candidate layer: it is a persisted `UnitDigest` field, consumed
    // only by the retrieval bake-off's comparison retrievers, not by [`Landmark`].
    let mut candidates: BTreeSet<(usize, usize)> = BTreeSet::new();
    if cfg.retrieval.landmark_pairs {
        let retriever = select_retriever(&cfg.retrieval.retriever);
        candidates.extend(retriever.candidates(&reps, cfg, stats));
    }
    stats.candidates_total += candidates.len();
    mark("pair-counting");

    // ---- verify: the `retrieval.filters` cascade, then anti_unify. Plain pairs
    // first, then variant pairs (inline pairs are excluded when a stronger tier
    // already grouped the bases, which needs the plain outcomes known). Verification
    // is per-pair independent — parallelized with order-preserving collects, so
    // grouping stays deterministic. `coverage` already ran in the landmark retriever
    // (its landmark-candidacy-scoped phase); the remaining filters run here. ----
    let filters = active_filters(cfg);
    enum Verify {
        /// Rejected by a filter; carries the canonical name of the FIRST filter that
        /// failed (the cascade short-circuits), so the funnel splits per filter.
        FilterRejected(&'static str),
        DivergenceRejected,
        Verified(AuOutcome),
    }
    let verify = |i: usize, j: usize| -> Verify {
        // Filter cascade: pure predicates over the shared per-pair evidence, a
        // short-circuiting AND (order is result-invariant — a conjunction — so it
        // sets only cost). Evidence is memoized in Ctx, computed once for all filters.
        let ctx = Ctx::new(&reps[i], &reps[j], cfg);
        for &(name, f) in &filters {
            if !f(&ctx) {
                return Verify::FilterRejected(name);
            }
        }
        // ── FIXED TERMINAL VERIFY: anti_unify ────────────────────────────────────
        // anti_unify is deliberately kept OUT of `retrieval.filters` and pinned here
        // as the terminal, for two reasons — both of which a future edit tempted to
        // "just add it to the list" must reckon with:
        //   1. It is a PRODUCER, not a predicate: it yields the match itself (the AU
        //      template + divergence), not a pass/fail. Leaving it out keeps the
        //      filter list homogeneous — pure predicates only — with no producer/
        //      filter type-union or capability flag, and nothing can misorder it.
        //   2. It is by far the DOMINANT per-pair cost. `au::anti_unify` recursively
        //      aligns the two units' trees — a Needleman-Wunsch DP (`au::au_list`),
        //      O(n·m) in the aligned child-list lengths at each level — orders of
        //      magnitude heavier than the cheap filters (hash-set intersections for
        //      coverage, one-vote-per-bin histogram counts for offset-histogram /
        //      h-tree). The ENTIRE filter cascade exists to minimize how many pairs
        //      reach this call: on self, 24.6k raw candidates prune to 8.1k AU calls
        //      yielding 453 verified pairs. Coverage does nearly all of the cutting
        //      (≈ −44% of the flood). h-tree, STACKED behind the offset-histogram as it
        //      ships, adds only ≈2% more (recall-neutral) — most of what it can reject
        //      the histogram has already rejected. It is kept because that residue is
        //      genuinely orthogonal (Δdepth, not Δoffset) and near-free, not because it
        //      is a major cut. So AU runs ONCE, last, only on filter-survivors.
        let (ua, ub) = (reps[i].unit_idx, reps[j].unit_idx);
        // Materialize the PAIR (the one true pair-of-trees consumer): a plain
        // borrow under the gate, an `Arc` handle out of the pack LRU over it.
        // The handles live for exactly this call — eviction can't invalidate
        // them, and dropping them returns the memory bound to the LRU.
        let ta = units[ua].tree.materialize(tree_pack);
        let tb = units[ub].tree.materialize(tree_pack);
        let outcome = au::anti_unify(&ta, &tb, profile, ir, li);
        if outcome.divergence > cfg.thresholds.max_divergence {
            return Verify::DivergenceRejected;
        }
        Verify::Verified(outcome)
    };

    let mut plain_cands: Vec<(usize, usize)> = Vec::new();
    let mut inline_cands: Vec<(usize, usize)> = Vec::new();
    for &(i, j) in &candidates {
        let (ua, ub) = (reps[i].unit_idx, reps[j].unit_idx);
        if !size_gate_passes(units[ua].token_count, units[ub].token_count, cfg) {
            continue;
        }
        if units[ua].variant.is_some() || units[ub].variant.is_some() {
            inline_cands.push((i, j));
        } else {
            plain_cands.push((i, j));
        }
    }

    let mut accepted: Vec<VerifiedPair> = Vec::new();
    let mut weak: Vec<VerifiedPair> = Vec::new();
    let plain_verified: Vec<Verify> = plain_cands.par_iter().map(|&(i, j)| verify(i, j)).collect();
    for (&(i, j), v) in plain_cands.iter().zip(plain_verified) {
        let outcome = match v {
            Verify::FilterRejected(name) => {
                record_filter_reject(stats, name);
                continue;
            }
            Verify::DivergenceRejected => {
                stats.divergence_rejected += 1;
                continue;
            }
            Verify::Verified(outcome) => outcome,
        };
        stats.verified_pairs += 1;
        let pair = VerifiedPair {
            a: reps[i].unit_idx,
            b: reps[j].unit_idx,
            outcome,
            chains: Vec::new(),
        };
        if pair.outcome.holes.len() <= cfg.thresholds.max_holes as usize && pair.outcome.factorable
        {
            accepted.push(pair);
        } else {
            weak.push(pair);
        }
    }

    let plain_keys: HashSet<(usize, usize)> = accepted
        .iter()
        .map(|p| (p.a.min(p.b), p.a.max(p.b)))
        .collect();
    // Cheap exclusions first, then parallel histogram+AU, then the best
    // outcome per base pair (several variant pairings can map to one).
    inline_cands.retain(|&(i, j)| {
        let (ua, ub) = (reps[i].unit_idx, reps[j].unit_idx);
        let (ba, bb) = (base_of(units, ua), base_of(units, ub));
        if ba == bb {
            return false; // a unit never matches its own inlined variant (§5.6)
        }
        let key = (ba.min(bb), ba.max(bb));
        // A stronger tier already owns the pair, or it is a D3 tautology.
        !exact_pairs.contains(&key) && !plain_keys.contains(&key) && !tautological(units, ua, ub)
    });
    let inline_verified: Vec<Verify> = inline_cands
        .par_iter()
        .map(|&(i, j)| verify(i, j))
        .collect();
    let mut inline_best: BTreeMap<(usize, usize), VerifiedPair> = BTreeMap::new();
    for (&(i, j), v) in inline_cands.iter().zip(inline_verified) {
        let outcome = match v {
            Verify::FilterRejected(name) => {
                record_filter_reject(stats, name);
                continue;
            }
            Verify::DivergenceRejected => {
                stats.divergence_rejected += 1;
                continue;
            }
            Verify::Verified(outcome) => outcome,
        };
        // Count the verify-survivor here — pre-acceptance, the SAME point as the
        // plain path — so `verified_pairs` means the same thing on both paths.
        stats.verified_pairs += 1;
        // Near-normalized acceptance criteria; no weak demotion — a weak
        // inline match is noise, not a verbose-only finding.
        if outcome.holes.len() > cfg.thresholds.max_holes as usize || !outcome.factorable {
            continue;
        }
        let (ua, ub) = (reps[i].unit_idx, reps[j].unit_idx);
        let (ba, bb) = (base_of(units, ua), base_of(units, ub));
        let key = (ba.min(bb), ba.max(bb));
        let chains: Vec<String> = [ua, ub]
            .iter()
            .filter_map(|&u| units[u].chain_line())
            .collect();
        let better = inline_best
            .get(&key)
            .is_none_or(|prev| outcome.divergence < prev.outcome.divergence);
        if better {
            inline_best.insert(
                key,
                VerifiedPair {
                    a: ba,
                    b: bb,
                    outcome,
                    chains,
                },
            );
        }
    }

    mark("verify");
    // ---- union-find into clone classes ----
    let mut out = Vec::new();
    out.extend(build_groups(accepted, Tier::NearNormalized, units, li));
    out.extend(build_groups(
        inline_best.into_values().collect(),
        Tier::InlineAssisted,
        units,
        li,
    ));
    out.extend(build_groups(weak, Tier::WeakSimilarity, units, li));
    out
}

/// Candidate-generation seam (spec §5.5, §7.4b). A retriever reads the shared
/// [`RepData`] substrate and emits candidate rep-index pairs `(i, j)` with `i < j`
/// for the shared verify chain (`histogram_passes` → `anti_unify`). It is
/// candidate generation ONLY — it never verifies. Retrieval is post-fingerprint,
/// so which retriever runs is hash-neutral (it does not enter the extraction cache
/// key). The default is [`Landmark`]; the bake-off registers alternatives
/// (minhash-lsh, winnowing, sourcerer-rare) against this same trait so the bench
/// races real production code.
pub trait Retriever: Sync {
    /// Stable identifier, equal to the `retrieval.retriever` selector value.
    fn name(&self) -> &'static str;
    /// Candidate rep-index pairs `(i, j)`, `i < j`, drawn from the shared substrate.
    /// May update the retriever's own accounting in `stats` (candidate/index counts);
    /// must not touch verify counts (those belong to the shared verify chain).
    fn candidates(
        &self,
        reps: &[RepData],
        cfg: &Config,
        stats: &mut RetrievalStats,
    ) -> Vec<(usize, usize)>;
}

/// Resolve the `retrieval.retriever` selector to a retriever. String-keyed and
/// plugin-extensible (mirrors `normalize.normalizer`); production ships only the
/// landmark retriever, so any value resolves to [`Landmark`] for now.
fn select_retriever(name: &str) -> Box<dyn Retriever> {
    match name {
        "landmark" => Box::new(Landmark),
        _ => Box::new(Landmark),
    }
}

/// The §5.5.4 constellation retriever — the §7.4(b) rivalry winner and the default.
/// Offset-sorted subtree peaks are paired combinatorially with bucketed structural
/// offsets into landmark hashes; two units are candidates when they share ≥
/// `shared_landmarks_min` landmarks and clear the §0.3 coverage-fraction gate.
///
/// Every peak is admitted — there is no document-frequency gate here (D51). A prior
/// rarity gate capped a peak's corpus document frequency before admission, but
/// measurement showed it never bound in practice (it rejected 3 of 754,254 hashes on
/// a real corpus) and could not be tuned into a useful one: making it bind removed
/// real clone families (tight, low-divergence families of small units) at every
/// threshold, with no precision benefit — `df` says nothing about whether the pair an
/// anchor mints is real. Flood control belongs at the candidate level below (the
/// coverage-fraction gate), where pairwise evidence exists to weigh a match; a rarity
/// gate acts on anchors, before any pair can be weighed.
pub struct Landmark;

impl Retriever for Landmark {
    fn name(&self) -> &'static str {
        "landmark"
    }

    fn candidates(
        &self,
        reps: &[RepData],
        cfg: &Config,
        stats: &mut RetrievalStats,
    ) -> Vec<(usize, usize)> {
        // Landmark pairs (§5.5.4): offset-sorted subtree peaks paired combinatorially
        // with bucketed structural offsets. Built here from the shared substrate and
        // owned by the retriever (not stored on RepData).
        //
        // `fan_out` is CONFIG (`retrieval.landmark_fan_out`), not a constant: it sets the
        // size of the constellation index, which is the largest memory row in a large
        // scan and ~41–46% of near-phase wall.
        let fan_out = cfg.retrieval.landmark_fan_out;
        let landmarks: Vec<Vec<u128>> = reps
            .par_iter()
            .map(|rep| {
                let mut peaks: Vec<(u32, u128)> = rep
                    .offsets
                    .iter()
                    .flat_map(|(h, offs, _)| offs.iter().map(move |o| (*o, *h)))
                    .collect();
                peaks.sort_unstable();
                let mut lms = Vec::new();
                for i in 0..peaks.len() {
                    for j in i + 1..(i + 1 + fan_out).min(peaks.len()) {
                        let delta = (peaks[j].0 - peaks[i].0) / 8;
                        let mut buf = Vec::with_capacity(36);
                        buf.extend_from_slice(&peaks[i].1.to_le_bytes());
                        buf.extend_from_slice(&peaks[j].1.to_le_bytes());
                        buf.extend_from_slice(&delta.to_le_bytes());
                        lms.push(xxhash_rust::xxh3::xxh3_128(&buf));
                    }
                }
                // shared_count_pairs expects sorted, deduplicated hash lists.
                lms.sort_unstable();
                lms.dedup();
                lms
            })
            .collect();

        let df_cap = 50usize;
        let window = cfg.retrieval.owner_pair_window;
        // ≥4 shared pair hashes (was 2): the Phase-2 watch item fired at
        // 500k LOC — shared-2 admitted ~30x more candidates than survive
        // histogram verification (D22). True clones share constellations
        // (dozens of pairs; audio ID works from 1–2% of thousands), so the
        // recall cost is nil on the benchmark, measured per §7.4(b).
        let shared_landmarks_min = cfg.retrieval.shared_landmarks_min;
        // The `coverage` pipeline stage gates this landmark-scoped filter; its
        // threshold is `landmark_coverage_min`. Absent from the pipeline ⇒ off.
        let coverage_min = if coverage_active(cfg) {
            cfg.retrieval.landmark_coverage_min
        } else {
            0.0
        };
        stats.landmark_index_size += landmarks.iter().map(Vec::len).sum::<usize>();
        let mut out = Vec::new();
        for ((i, j), shared) in
            shared_count_pairs(landmarks.len(), |k| landmarks[k].as_slice(), df_cap, window)
        {
            if shared < shared_landmarks_min {
                continue;
            }
            // Coverage-fraction candidate gate (docs/substantiality-metric.md
            // §0.3): the shared constellation as a fraction of the smaller unit's
            // landmark set. A whole-unit clone covers most of each unit; a
            // coincidental boilerplate region covers little — exactly the flood
            // mechanism. Uses the FULL (df-uncapped) landmark intersection so the
            // gate is never MORE aggressive than the validated definition. Pre-AU, so it
            // saves the O(n·m) anti-unification on the coincidences it drops.
            //
            // The gate is UNCONDITIONAL: every candidate is coverage-exposed, with no
            // second layer to re-propose a pair it drops. Recall-neutral — leaving every
            // pair coverage-exposed produces byte-identical group sets on self/fs/net.
            if coverage_min > 0.0 {
                let denom = landmarks[i].len().min(landmarks[j].len()).max(1) as f64;
                let full_shared = intersect_count(&landmarks[i], &landmarks[j]);
                if (full_shared as f64) / denom < coverage_min {
                    stats.candidates_landmark_coverage_gated += 1;
                    continue;
                }
            }
            stats.candidates_landmark += 1;
            out.push((i, j));
        }
        out
    }
}

/// D3 tautology filter, minimum viable rule: an inlined variant never groups
/// with the callee it inlined, nor with units exact-equal to that callee.
/// Extension (D18): a pair whose BASES are exact-equal is also suppressed —
/// the plain exact tier owns that pair, or the floor deliberately excluded
/// it, and equal bases inlining equal callees match by construction.
fn tautological(units: &[Unit], a: usize, b: usize) -> bool {
    let one_way = |x: usize, y: usize| -> bool {
        let Some(tag) = &units[x].variant else {
            return false;
        };
        let y_base = base_of(units, y);
        tag.base == y_base
            || tag.expanded_fps.contains(&units[y].fingerprint)
            || tag.expanded_fps.contains(&units[y_base].fingerprint)
    };
    let (ba, bb) = (base_of(units, a), base_of(units, b));
    units[ba].fingerprint == units[bb].fingerprint || one_way(a, b) || one_way(b, a)
}

/// Count of shared hashes between two sorted, deduplicated hash lists (a linear
/// merge). Used by the coverage-fraction gate: the FULL landmark intersection
/// (no df cap), matching the validated §0.3 coverage definition.
fn intersect_count(a: &[u128], b: &[u128]) -> usize {
    let (mut i, mut j, mut n) = (0usize, 0usize, 0usize);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                n += 1;
                i += 1;
                j += 1;
            }
        }
    }
    n
}

/// Shared-hash counts for all pairs (skip hashes with df > cap). Fully
/// sort-based (M3b/D22): at 500k-LOC scale the landmark index holds ~10⁷
/// hashes, and both a HashMap-of-owner-lists index and a map per pair event
/// dominated the scan. `hashes(i)` returns unit `i`'s already-sorted,
/// deduplicated hash list — indexed rather than keyed on `RepData` so any layer
/// (bag substrate, retriever-owned landmark index, …) can drive the same join.
fn shared_count_pairs<'a, F>(
    n: usize,
    hashes: F,
    df_cap: usize,
    owner_pair_window: usize,
) -> Vec<((usize, usize), usize)>
where
    F: Fn(usize) -> &'a [u128],
{
    let total: usize = (0..n).map(|i| hashes(i).len()).sum();
    let mut entries: Vec<(u128, u32)> = Vec::with_capacity(total);
    for i in 0..n {
        debug_assert!(hashes(i).is_sorted());
        for &h in hashes(i) {
            entries.push((h, i as u32));
        }
    }
    entries.par_sort_unstable();

    let mut events: Vec<(u32, u32)> = Vec::new();
    let mut run_start = 0usize;
    for k in 0..=entries.len() {
        if k < entries.len() && entries[k].0 == entries[run_start].0 {
            continue;
        }
        // Owners of one hash, ascending (entries sort by (hash, owner)).
        let owners: Vec<u32> = entries[run_start..k].iter().map(|e| e.1).collect();
        run_start = k;
        if owners.len() < 2 || owners.len() > df_cap {
            continue;
        }
        // Full clique for small owner sets; a sliding window for common
        // hashes: grouping is transitive via union-find, so adjacent-owner
        // chains connect a large clone family without paying O(owners²)
        // events per hash. Owner order is unit order (file-sorted), so
        // windows are deterministic.
        let window = if owner_pair_window == 0 {
            usize::MAX
        } else {
            owner_pair_window
        };
        if owner_pair_window == 0 || owners.len() <= 8 {
            for x in 0..owners.len() {
                for y in x + 1..owners.len() {
                    events.push((owners[x], owners[y]));
                }
            }
        } else {
            for x in 0..owners.len() {
                for y in x + 1..(x + 1).saturating_add(window).min(owners.len()) {
                    events.push((owners[x], owners[y]));
                }
            }
        }
    }
    events.par_sort_unstable();
    let mut counts: Vec<((usize, usize), usize)> = Vec::new();
    for pair in events {
        match counts.last_mut() {
            Some((last, n)) if *last == (pair.0 as usize, pair.1 as usize) => *n += 1,
            _ => counts.push(((pair.0 as usize, pair.1 as usize), 1)),
        }
    }
    counts
}

/// O(1) pre-AU gate: divergence ≤ `max_divergence` is impossible when token
/// counts differ by more than ~(1 + 2·max_divergence)×, because the size
/// difference alone must land in counted holes. The slack term absorbs
/// zero-cost machinery holes (D14). Skipping these candidates loses nothing
/// the divergence gate wouldn't reject, at none of the AU cost.
fn size_gate_passes(ta: u32, tb: u32, cfg: &Config) -> bool {
    let (lo, hi) = (u64::from(ta.min(tb)), u64::from(ta.max(tb)));
    let ratio = 1.0 + 2.0 * cfg.thresholds.max_divergence + 0.15;
    hi as f64 <= lo as f64 * ratio + 64.0
}

/// Pre-order depth per node offset, numbered exactly as `fingerprint::
/// walk_inventory` (node before its children), so `out[subtree.offset]` is the
/// tree depth of that subtree's root. Dense over `0..node_count`.
fn preorder_depths(root: &crate::tree::NormNode) -> Vec<u16> {
    fn walk(node: &crate::tree::NormNode, depth: u16, out: &mut Vec<u16>) {
        out.push(depth);
        for c in &node.children {
            walk(c, depth.saturating_add(1), out);
        }
    }
    let mut out = Vec::new();
    walk(root, 0, &mut out);
    out
}

/// One shared floor-3 subtree's positional evidence: the (≤4×4) offset lists and
/// depth lists from each unit. Every consistency criterion derives its own delta
/// bins from this — nothing here is retriever- or criterion-specific.
struct SharedSubtree<'a> {
    offs_a: &'a [u32],
    offs_b: &'a [u32],
    depths_a: &'a [u16],
    depths_b: &'a [u16],
}

/// THE shared-subtree evidence pass: a single linear merge of the two hash-sorted
/// inventories, yielding — per shared subtree — BOTH its offset lists and depth
/// lists (borrowed, no copy). Every verify criterion reads this one pass; there is
/// no second walk (the §0.5 "near-free" property the sweep relied on).
fn shared_subtree_evidence<'a>(a: &'a RepData, b: &'a RepData) -> Vec<SharedSubtree<'a>> {
    let mut ev = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.offsets.len() && j < b.offsets.len() {
        match a.offsets[i].0.cmp(&b.offsets[j].0) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                ev.push(SharedSubtree {
                    offs_a: &a.offsets[i].1,
                    offs_b: &b.offsets[j].1,
                    depths_a: &a.offsets[i].2,
                    depths_b: &b.offsets[j].2,
                });
                i += 1;
                j += 1;
            }
        }
    }
    ev
}

/// Vote budget shared by every consistency criterion: `histogram_min_votes` or
/// ⅛ of the smaller inventory, whichever is larger. A clone that survives the
/// `max_divergence` gate shares a material fraction of the smaller unit's subtree
/// inventory, so the bar scales with size — a fixed 5-vote bar on a 250-token unit
/// (2% coverage) admits pairs AU is guaranteed to reject (D22).
fn votes_needed(a: &RepData, b: &RepData, cfg: &Config) -> u32 {
    let min_inventory = a.offsets.len().min(b.offsets.len()) as u32;
    cfg.histogram_min_votes().max(min_inventory / 8)
}

/// Diagonal vote over shared subtrees: each subtree casts at most ONE vote per
/// distinct delta bin (D22 — a repeated small idiom must not fake a diagonal by
/// multiplicity); pass iff some bin reaches `needed`. The delta comes from `bin`.
/// Generic over the coordinate type so offset (u32, quantized /4) and depth (u16,
/// raw) consistency are the SAME mechanism on different axes.
fn diagonal_passes<'a, T: Copy + 'a>(
    ev: &[SharedSubtree<'a>],
    needed: u32,
    lists: impl Fn(&SharedSubtree<'a>) -> (&'a [T], &'a [T]),
    bin: impl Fn(T, T) -> i64,
) -> bool {
    let mut bins: HashMap<i64, u32> = HashMap::new();
    let mut best = 0u32;
    let mut local: Vec<i64> = Vec::with_capacity(16);
    for s in ev {
        if best >= needed {
            return true;
        }
        let (la, lb) = lists(s);
        local.clear();
        for &va in la.iter().take(4) {
            for &vb in lb.iter().take(4) {
                let b = bin(va, vb);
                if !local.contains(&b) {
                    local.push(b);
                }
            }
        }
        for &b in &local {
            let v = bins.entry(b).or_insert(0);
            *v += 1;
            best = best.max(*v);
        }
    }
    best >= needed
}

/// Shazam-style offset-delta diagonal (spec §5.6): shared subtrees vote
/// `(oa−ob)/4`; a genuine clone concentrates in one bin, coincidence scatters.
/// Always on — the incumbent verify criterion.
fn offset_consistency(ev: &[SharedSubtree], needed: u32) -> bool {
    diagonal_passes(
        ev,
        needed,
        |s| (s.offs_a, s.offs_b),
        |oa, ob| (i64::from(oa) - i64::from(ob)) / 4,
    )
}

/// H-tree-verify depth-delta diagonal (§0.5): shared subtrees vote `depth_a−
/// depth_b`. A genuine clone places its shared subtrees at consistent RELATIVE
/// depths; a coincidental landmark collision sits at inconsistent depths and
/// scatters. Selected by listing `h-tree` in `retrieval.filters` — a peer of
/// `offset_consistency`, reading the same evidence, not a wrapper around it.
fn depth_consistency(ev: &[SharedSubtree], needed: u32) -> bool {
    diagonal_passes(
        ev,
        needed,
        |s| (s.depths_a, s.depths_b),
        |da, db| i64::from(da) - i64::from(db),
    )
}

/// Per-candidate-pair context the consistency filters read — a `&Ctx` shared,
/// lazily-memoized view. The shared-subtree evidence (offset_delta + depth_delta per
/// shared floor-3 subtree) and the vote budget are each computed ONCE on first demand
/// and reused by every filter (`offset-histogram` + `h-tree`), so listing both costs
/// one merge-join, not one walk per filter. Filters are PURE predicates over this —
/// interior mutability (`OnceCell`) keeps memoization behind a shared `&Ctx`.
struct Ctx<'a> {
    a: &'a RepData<'a>,
    b: &'a RepData<'a>,
    cfg: &'a Config,
    evidence: std::cell::OnceCell<Vec<SharedSubtree<'a>>>,
    needed: std::cell::OnceCell<u32>,
}

impl<'a> Ctx<'a> {
    fn new(a: &'a RepData<'a>, b: &'a RepData<'a>, cfg: &'a Config) -> Self {
        Ctx {
            a,
            b,
            cfg,
            evidence: std::cell::OnceCell::new(),
            needed: std::cell::OnceCell::new(),
        }
    }
    fn evidence(&self) -> &[SharedSubtree<'a>] {
        self.evidence
            .get_or_init(|| shared_subtree_evidence(self.a, self.b))
    }
    fn needed(&self) -> u32 {
        *self
            .needed
            .get_or_init(|| votes_needed(self.a, self.b, self.cfg))
    }
}

/// A near-tier consistency filter: a PURE predicate over the shared per-pair context
/// (`true` = pass, `false` = reject). The `retrieval.filters` list is a homogeneous
/// short-circuiting AND-cascade of these; `anti_unify` is deliberately NOT one (it is
/// the fixed terminal producer — see its call site).
type Filter = fn(&Ctx) -> bool;

fn filter_offset_histogram(ctx: &Ctx) -> bool {
    offset_consistency(ctx.evidence(), ctx.needed())
}
fn filter_h_tree(ctx: &Ctx) -> bool {
    depth_consistency(ctx.evidence(), ctx.needed())
}

/// Resolve a `retrieval.filters` name to its canonical name + verify-phase [`Filter`]
/// predicate. `coverage` is a known filter but returns `None` here: it is landmark-
/// candidacy-scoped and runs in the landmark retriever's own phase (see
/// [`coverage_active`]), not as a per-pair `&Ctx` predicate — so it is dispatched
/// separately, not in the verify cascade. Unknown names are rejected at config load.
fn filter_by_name(name: &str) -> Option<(&'static str, Filter)> {
    match name {
        "offset-histogram" => Some(("offset-histogram", filter_offset_histogram)),
        "h-tree" => Some(("h-tree", filter_h_tree)),
        _ => None,
    }
}

/// Attribute a filter-cascade rejection to its per-filter [`RetrievalStats`] counter.
/// `coverage` never reaches here (it is retriever-scoped); an unknown name is ignored
/// rather than panicking, keeping the diagnostic path infallible.
fn record_filter_reject(stats: &mut RetrievalStats, name: &str) {
    match name {
        "offset-histogram" => stats.offset_histogram_rejected += 1,
        "h-tree" => stats.htree_rejected += 1,
        _ => {}
    }
}

/// The full set of known `retrieval.filters` names (for load-time validation),
/// including `coverage` (dispatched to the retriever, not a `&Ctx` predicate).
const KNOWN_FILTERS: &[&str] = &["coverage", "offset-histogram", "h-tree"];

/// Config load-time validation of the `retrieval.filters` list: every name is known.
/// Order is not validated — the cascade is a conjunction, so order is result-invariant
/// (it sets only short-circuit cost).
pub fn validate_filters(filters: &[String]) -> Result<(), String> {
    for name in filters {
        if !KNOWN_FILTERS.contains(&name.as_str()) {
            return Err(format!(
                "unknown filter `{name}` (known: {})",
                KNOWN_FILTERS.join(", ")
            ));
        }
    }
    Ok(())
}

/// Whether the `coverage` filter is listed — gates the landmark retriever's §0.3
/// coverage-fraction filter (landmark-candidacy-scoped, so it lives in that phase).
fn coverage_active(cfg: &Config) -> bool {
    cfg.retrieval.filters.iter().any(|n| n == "coverage")
}

/// The configured verify-phase filters as `(canonical name, predicate)`, in listed
/// order, resolved once per language scan (`coverage` excluded — it runs in the
/// retriever phase). The name rides along so a rejection is attributed to its filter.
fn active_filters(cfg: &Config) -> Vec<(&'static str, Filter)> {
    cfg.retrieval
        .filters
        .iter()
        .filter_map(|name| filter_by_name(name))
        .collect()
}

fn build_groups(
    pairs: Vec<VerifiedPair>,
    tier: Tier,
    units: &[Unit],
    li: &crate::intern::LabelInterner,
) -> Vec<NearGroup> {
    if pairs.is_empty() {
        return Vec::new();
    }
    // Union-find over the (sparse) unit indices the pairs mention.
    let mut involved: Vec<usize> = pairs.iter().flat_map(|p| [p.a, p.b]).collect();
    involved.sort_unstable();
    involved.dedup();
    let dense: HashMap<usize, usize> = involved.iter().enumerate().map(|(d, &u)| (u, d)).collect();
    let mut parent: Vec<usize> = (0..involved.len()).collect();
    fn find(parent: &mut [usize], x: usize) -> usize {
        let mut root = x;
        while parent[root] != root {
            root = parent[root];
        }
        let mut cur = x;
        while parent[cur] != root {
            let next = parent[cur];
            parent[cur] = root;
            cur = next;
        }
        root
    }
    for p in &pairs {
        let (ri, rj) = (
            find(&mut parent, dense[&p.a]),
            find(&mut parent, dense[&p.b]),
        );
        if ri != rj {
            parent[ri] = rj;
        }
    }
    // Representative outcome per group: the lowest-divergence pair.
    struct Acc {
        members: Vec<usize>,
        best: Option<AuOutcome>,
        max_div: f64,
        chains: Vec<String>,
    }
    let mut by_root: BTreeMap<usize, Acc> = BTreeMap::new();
    for p in pairs {
        let root = find(&mut parent, dense[&p.a]);
        let entry = by_root.entry(root).or_insert(Acc {
            members: Vec::new(),
            best: None,
            max_div: 0.0,
            chains: Vec::new(),
        });
        for member in [p.a, p.b] {
            if !entry.members.contains(&member) {
                entry.members.push(member);
            }
        }
        for chain in p.chains {
            if !entry.chains.contains(&chain) {
                entry.chains.push(chain);
            }
        }
        entry.max_div = entry.max_div.max(p.outcome.divergence);
        let better = entry
            .best
            .as_ref()
            .is_none_or(|best| p.outcome.divergence < best.divergence);
        if better {
            entry.best = Some(p.outcome);
        }
    }
    by_root
        .into_values()
        .map(|acc| {
            let best = acc.best.unwrap();
            let mut member_units = acc.members;
            member_units.sort_by_key(|&i| (units[i].file.clone(), units[i].byte_span));
            NearGroup {
                member_units,
                tier,
                template: au::render_template(&best.template, li),
                template_hash: fingerprint::merkle(&best.template, li),
                template_tokens: best.template_tokens,
                template_boilerplate: crate::ir::substance::boilerplate_mass(&best.template, li),
                divergence: acc.max_div,
                inline_chains: acc.chains,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_rejects_route_to_per_filter_counters() {
        // Item 1: the funnel decomposes — each verify-phase filter has its OWN counter
        // (`histogram_rejected` used to lump both), and `divergence_rejected` /
        // `verified_pairs` are distinct fields, so the retrieval knob analysis can read
        // decomposable numbers. `coverage` is retriever-scoped and never routed here.
        let mut stats = RetrievalStats::default();
        record_filter_reject(&mut stats, "offset-histogram");
        record_filter_reject(&mut stats, "offset-histogram");
        record_filter_reject(&mut stats, "h-tree");
        record_filter_reject(&mut stats, "coverage"); // ignored — not a verify-phase filter
        assert_eq!(stats.offset_histogram_rejected, 2);
        assert_eq!(stats.htree_rejected, 1);
        // Distinct funnel stages, untouched by filter rejections.
        assert_eq!(stats.divergence_rejected, 0);
        assert_eq!(stats.verified_pairs, 0);
    }

    #[test]
    fn active_filters_carry_canonical_names() {
        // The name rides along with each predicate so a rejection is attributable.
        let cfg = Config::default();
        let names: Vec<&str> = active_filters(&cfg).iter().map(|(n, _)| *n).collect();
        // `coverage` is dispatched to the retriever phase, not the verify cascade.
        assert_eq!(names, vec!["offset-histogram", "h-tree"]);
    }
}
