//! P7 tree tier (spec §5.6): candidate retrieval (subtree-hash bags + §5.5.4
//! landmark pairs — the §7.4(b) rivalry winner), offset-histogram
//! verification, then anti-unification. Phase 3: inline-expanded variants
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
use std::collections::{BTreeMap, HashMap, HashSet};

pub struct NearGroup {
    /// Base-resolved member unit indices (a variant reports as its base).
    pub member_units: Vec<usize>,
    pub tier: Tier,
    pub template: String,
    /// Merkle hash of the group template tree — the stable baseline key for
    /// near/inline groups (spec §2/§6.1 `partialFingerprints`).
    pub template_hash: u128,
    pub template_tokens: u32,
    pub divergence: f64,
    /// Inline chains of the participating variants (InlineAssisted only).
    pub inline_chains: Vec<String>,
}

#[derive(Default, Debug, Clone, serde::Serialize)]
pub struct RetrievalStats {
    pub candidates_bag: usize,
    pub candidates_landmark: usize,
    pub candidates_total: usize,
    pub histogram_rejected: usize,
    pub verified_pairs: usize,
    /// Verified pairs proposed only by landmark pairs (§7.4b record).
    pub verified_only_landmark: usize,
    pub landmark_index_size: usize,
}

struct RepData {
    unit_idx: usize,
    bag_set: Vec<u128>,
    /// Subtree hash → pre-order offsets, sorted by hash: the histogram
    /// intersects two of these by linear merge, no hashing (M3b/D22 — the
    /// per-candidate HashMap probes dominated 500k-LOC scans).
    offsets: Vec<(u128, Vec<u32>)>,
    landmarks: Vec<u128>, // pair hashes
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
    cfg: &Config,
    stats: &mut RetrievalStats,
    exact_pairs: &HashSet<(usize, usize)>,
) -> Vec<NearGroup> {
    let mut groups = Vec::new();
    let langs: HashSet<Lang> = units.iter().map(|u| u.lang).collect();
    let mut langs: Vec<Lang> = langs.into_iter().collect();
    langs.sort();
    for lang in langs {
        groups.extend(near_groups_for_lang(units, lang, cfg, stats, exact_pairs));
    }
    groups
}

fn near_groups_for_lang(
    units: &[Unit],
    lang: Lang,
    cfg: &Config,
    stats: &mut RetrievalStats,
    exact_pairs: &HashSet<(usize, usize)>,
) -> Vec<NearGroup> {
    let profile = lang.profile();
    let floor = cfg.thresholds.min_unit_tokens;
    let debug_timing = std::env::var_os("REPRISE_TIMING").is_some();
    let mut t = std::time::Instant::now();
    let mut mark = |label: &str| {
        if debug_timing {
            eprintln!("  near[{lang:?}] {label}: {}ms", t.elapsed().as_millis());
        }
        t = std::time::Instant::now();
    };

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
    if eligible.len() < 2 {
        return Vec::new();
    }
    let mut reps: Vec<RepData> = eligible
        .par_iter()
        .map(|&idx| {
            // Histogram offsets use a finer inventory (floor 3) than the bag
            // (small units would otherwise starve the vote count); the bag
            // itself keeps the §9 floor.
            let inv: Vec<Subtree> =
                fingerprint::subtree_inventory(&units[idx].tree, 3, HashMode::MaskedLocals);
            let mut flat: Vec<(u128, u32)> = inv.iter().map(|s| (s.hash, s.offset)).collect();
            flat.sort_unstable();
            let mut offsets: Vec<(u128, Vec<u32>)> = Vec::new();
            for (h, o) in flat {
                match offsets.last_mut() {
                    Some((last, offs)) if *last == h => offs.push(o),
                    _ => offsets.push((h, vec![o])),
                }
            }
            let mut bag_set: Vec<u128> = inv
                .iter()
                .filter(|s| s.tokens >= cfg.thresholds.bag_min_subtree_tokens)
                .map(|s| s.hash)
                .collect();
            bag_set.sort_unstable();
            bag_set.dedup();
            RepData {
                unit_idx: idx,
                bag_set,
                offsets,
                landmarks: Vec::new(),
            }
        })
        .collect();

    mark("reps");
    // Landmark pairs (§5.5.4): rare subtrees (low document frequency) paired
    // combinatorially with bucketed structural offsets.
    if cfg.retrieval.landmark_pairs {
        let mut df: HashMap<u128, u32> = HashMap::new();
        for rep in &reps {
            for h in &rep.bag_set {
                *df.entry(*h).or_insert(0) += 1;
            }
        }
        let rare_cap = 3.max(reps.len() as u32 / 20);
        reps.par_iter_mut().for_each(|rep| {
            let mut rare: Vec<(u32, u128)> = rep
                .offsets
                .iter()
                .filter(|(h, _)| df.get(h).copied().unwrap_or(0) <= rare_cap)
                .flat_map(|(h, offs)| offs.iter().map(move |o| (*o, *h)))
                .collect();
            rare.sort_unstable();
            const FAN_OUT: usize = 3;
            for i in 0..rare.len() {
                for j in i + 1..(i + 1 + FAN_OUT).min(rare.len()) {
                    let delta = (rare[j].0 - rare[i].0) / 8;
                    let mut buf = Vec::with_capacity(36);
                    buf.extend_from_slice(&rare[i].1.to_le_bytes());
                    buf.extend_from_slice(&rare[j].1.to_le_bytes());
                    buf.extend_from_slice(&delta.to_le_bytes());
                    rep.landmarks.push(xxhash_rust::xxh3::xxh3_128(&buf));
                }
            }
            // shared_count_pairs expects sorted, deduplicated hash lists.
            rep.landmarks.sort_unstable();
            rep.landmarks.dedup();
        });
    }

    mark("landmarks");
    // ---- candidate retrieval (union of layers; membership tracked for §7.4b) ----
    let df_cap = 50usize;
    let window = cfg.retrieval.owner_pair_window;
    let bag_pairs = shared_count_pairs(&reps, |r| &r.bag_set, df_cap, window);
    let mut candidates: BTreeMap<(usize, usize), (bool, bool)> = BTreeMap::new();
    for ((i, j), shared) in bag_pairs {
        let union = reps[i].bag_set.len() + reps[j].bag_set.len() - shared;
        if union > 0 && shared as f64 / union as f64 >= cfg.thresholds.candidate_sim {
            candidates.entry((i, j)).or_default().0 = true;
        }
    }
    stats.candidates_bag += candidates.len();
    if cfg.retrieval.landmark_pairs {
        // ≥4 shared pair hashes (was 2): the Phase-2 watch item fired at
        // 500k LOC — shared-2 admitted ~30x more candidates than survive
        // histogram verification (D22). True clones share constellations
        // (dozens of pairs; audio ID works from 1–2% of thousands), so the
        // recall cost is nil on the benchmark, measured per §7.4(b).
        let shared_landmarks_min = cfg.retrieval.shared_landmarks_min;
        stats.landmark_index_size += reps.iter().map(|r| r.landmarks.len()).sum::<usize>();
        for ((i, j), shared) in shared_count_pairs(&reps, |r| &r.landmarks, df_cap, window) {
            if shared >= shared_landmarks_min {
                let entry = candidates.entry((i, j)).or_default();
                entry.1 = true;
                stats.candidates_landmark += 1;
            }
        }
    }
    stats.candidates_total += candidates.len();
    mark("pair-counting");

    // ---- histogram verification + AU: plain pairs first, then variant pairs
    // (inline pairs are excluded when a stronger tier already grouped the
    // bases, which needs the plain outcomes known). Verification is per-pair
    // independent — parallelized with order-preserving collects, so grouping
    // stays deterministic. ----
    enum Verify {
        HistogramRejected,
        DivergenceRejected,
        Verified(AuOutcome),
    }
    let verify = |i: usize, j: usize| -> Verify {
        if !offset_histogram_passes(&reps[i], &reps[j], cfg) {
            return Verify::HistogramRejected;
        }
        let (ua, ub) = (reps[i].unit_idx, reps[j].unit_idx);
        let outcome = au::anti_unify(&units[ua].tree, &units[ub].tree, profile);
        if outcome.divergence > cfg.thresholds.max_divergence {
            return Verify::DivergenceRejected;
        }
        Verify::Verified(outcome)
    };

    let mut plain_cands: Vec<(usize, usize, bool, bool)> = Vec::new();
    let mut inline_cands: Vec<(usize, usize, bool, bool)> = Vec::new();
    for (&(i, j), &(by_bag, by_lm)) in &candidates {
        let (ua, ub) = (reps[i].unit_idx, reps[j].unit_idx);
        if !size_gate_passes(units[ua].token_count, units[ub].token_count, cfg) {
            continue;
        }
        if units[ua].variant.is_some() || units[ub].variant.is_some() {
            inline_cands.push((i, j, by_bag, by_lm));
        } else {
            plain_cands.push((i, j, by_bag, by_lm));
        }
    }

    let mut accepted: Vec<VerifiedPair> = Vec::new();
    let mut weak: Vec<VerifiedPair> = Vec::new();
    let plain_verified: Vec<Verify> = plain_cands
        .par_iter()
        .map(|&(i, j, _, _)| verify(i, j))
        .collect();
    for (&(i, j, by_bag, by_lm), v) in plain_cands.iter().zip(plain_verified) {
        let outcome = match v {
            Verify::HistogramRejected => {
                stats.histogram_rejected += 1;
                continue;
            }
            Verify::DivergenceRejected => continue,
            Verify::Verified(outcome) => outcome,
        };
        stats.verified_pairs += 1;
        if by_lm && !by_bag {
            stats.verified_only_landmark += 1;
        }
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
    inline_cands.retain(|&(i, j, _, _)| {
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
        .map(|&(i, j, _, _)| verify(i, j))
        .collect();
    let mut inline_best: BTreeMap<(usize, usize), VerifiedPair> = BTreeMap::new();
    for (&(i, j, by_bag, by_lm), v) in inline_cands.iter().zip(inline_verified) {
        let outcome = match v {
            Verify::HistogramRejected => {
                stats.histogram_rejected += 1;
                continue;
            }
            Verify::DivergenceRejected => continue,
            Verify::Verified(outcome) => outcome,
        };
        // Near-normalized acceptance criteria; no weak demotion — a weak
        // inline match is noise, not a verbose-only finding.
        if outcome.holes.len() > cfg.thresholds.max_holes as usize || !outcome.factorable {
            continue;
        }
        stats.verified_pairs += 1;
        if by_lm && !by_bag {
            stats.verified_only_landmark += 1;
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
    out.extend(build_groups(accepted, Tier::NearNormalized, units));
    out.extend(build_groups(
        inline_best.into_values().collect(),
        Tier::InlineAssisted,
        units,
    ));
    out.extend(build_groups(weak, Tier::WeakSimilarity, units));
    out
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

/// Shared-hash counts for all pairs (skip hashes with df > cap). Fully
/// sort-based (M3b/D22): at 500k-LOC scale the landmark index holds ~10⁷
/// hashes, and both a HashMap-of-owner-lists index and a map per pair event
/// dominated the scan. Callers provide per-rep hash lists that are already
/// sorted and deduplicated.
fn shared_count_pairs<'a, F>(
    reps: &'a [RepData],
    hashes: F,
    df_cap: usize,
    owner_pair_window: usize,
) -> Vec<((usize, usize), usize)>
where
    F: Fn(&'a RepData) -> &'a Vec<u128>,
{
    let total: usize = reps.iter().map(|r| hashes(r).len()).sum();
    let mut entries: Vec<(u128, u32)> = Vec::with_capacity(total);
    for (i, rep) in reps.iter().enumerate() {
        debug_assert!(hashes(rep).is_sorted());
        for &h in hashes(rep) {
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

/// Shazam-style diagonal check: shared fingerprints vote Δoffset; a genuine
/// clone concentrates in one bin, coincidence scatters (spec §5.6).
///
/// M3b hardening (D22): each shared hash contributes at most ONE vote per
/// bin (a repeated small idiom must not fake a diagonal by multiplicity),
/// and the acceptance threshold scales with unit size — a clone that can
/// survive the `max_divergence` gate shares a material fraction of the
/// smaller unit's subtree inventory, so a fixed 5-vote bar on a 250-token
/// unit (2% coverage) admits pairs AU is guaranteed to reject, at O(n·m)
/// AU cost per admission. Wang's original scores by aligned-cluster SIZE
/// for the same reason.
fn offset_histogram_passes(a: &RepData, b: &RepData, cfg: &Config) -> bool {
    let min_inventory = a.offsets.len().min(b.offsets.len()) as u32;
    let needed = cfg.thresholds.histogram_min_votes.max(min_inventory / 8);
    let mut bins: HashMap<i64, u32> = HashMap::new();
    let mut best = 0u32;
    let mut hash_bins: Vec<i64> = Vec::with_capacity(16);
    // Linear merge of the hash-sorted offset inventories.
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.offsets.len() && j < b.offsets.len() {
        // Lossless early exits (M4a): a bin's count only grows by shared
        // hashes, and at most one vote per remaining shared hash can land in
        // any bin — once the winner is decided either way, stop merging.
        if best >= needed {
            return true;
        }
        let remaining = (a.offsets.len() - i).min(b.offsets.len() - j) as u32;
        if best + remaining < needed {
            return false;
        }
        let (ha, offs_a) = &a.offsets[i];
        let (hb, offs_b) = &b.offsets[j];
        match ha.cmp(hb) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                hash_bins.clear();
                for &oa in offs_a.iter().take(4) {
                    for &ob in offs_b.iter().take(4) {
                        let bin = (i64::from(oa) - i64::from(ob)) / 4;
                        if !hash_bins.contains(&bin) {
                            hash_bins.push(bin);
                        }
                    }
                }
                for &bin in &hash_bins {
                    let v = bins.entry(bin).or_insert(0);
                    *v += 1;
                    best = best.max(*v);
                }
                i += 1;
                j += 1;
            }
        }
    }
    best >= needed
}

fn build_groups(pairs: Vec<VerifiedPair>, tier: Tier, units: &[Unit]) -> Vec<NearGroup> {
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
                template: au::render_template(&best.template),
                template_hash: fingerprint::merkle(&best.template),
                template_tokens: best.template_tokens,
                divergence: acc.max_div,
                inline_chains: acc.chains,
            }
        })
        .collect()
}
