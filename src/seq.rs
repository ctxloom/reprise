//! Sequence tier (spec §5.6): suffix array + LCP over the normalized token
//! stream; maximal repeated sequences ≥ `min_seq_tokens` become exact-region
//! findings. Baker's `dup` lineage — every exact duplicated run is found.

use crate::stream::Corpus;
use rayon::prelude::*;

/// Suffix array by prefix doubling — O(n log² n) with parallel sorts over
/// materialized (rank, rank₂, index) keys (M3b: recomputing keys inside the
/// sort comparator dominated 500k-LOC scans). The result is deterministic:
/// per-unit separators make all suffixes distinct, and within a round ties
/// break on the suffix index.
pub fn suffix_array(tokens: &[u32]) -> Vec<u32> {
    let n = tokens.len();
    if n == 0 {
        return Vec::new();
    }
    // Keys pack (rank, rank₂, index) into one u128: ranks and indices are
    // bounded by n (corpora are ≪ 2³² tokens), and flat u128 sorts measurably
    // faster than tuple comparisons at ~4M suffixes.
    assert!(n < u32::MAX as usize, "corpus exceeds u32 suffix space");
    let mut rank: Vec<u64> = tokens.iter().map(|&t| u64::from(t)).collect();
    let mut keyed: Vec<u128> = Vec::new();
    let mut tmp = vec![0u64; n];
    let mut k = 1usize;
    loop {
        (0..n)
            .into_par_iter()
            .map(|i| {
                let second = if i + k < n { rank[i + k] + 1 } else { 0 };
                (u128::from(rank[i] as u32) << 64) | (u128::from(second as u32) << 32) | i as u128
            })
            .collect_into_vec(&mut keyed);
        keyed.par_sort_unstable();
        let idx = |key: u128| (key & 0xFFFF_FFFF) as usize;
        let rank_part = |key: u128| (key >> 32) as u64; // (rank ‖ rank₂)
        tmp[idx(keyed[0])] = 0;
        let mut distinct = true;
        for w in 1..n {
            let equal = rank_part(keyed[w - 1]) == rank_part(keyed[w]);
            distinct &= !equal;
            tmp[idx(keyed[w])] = tmp[idx(keyed[w - 1])] + u64::from(!equal);
        }
        std::mem::swap(&mut rank, &mut tmp);
        k *= 2;
        if distinct || k >= n {
            break;
        }
    }
    keyed
        .iter()
        .map(|&key| (key & 0xFFFF_FFFF) as u32)
        .collect()
}

/// Kasai LCP: lcp[i] = longest common prefix of sa[i-1] and sa[i].
pub fn lcp_array(tokens: &[u32], sa: &[u32]) -> Vec<u32> {
    let n = tokens.len();
    let mut rank = vec![0u32; n];
    for (i, &s) in sa.iter().enumerate() {
        rank[s as usize] = i as u32;
    }
    let mut lcp = vec![0u32; n];
    let mut h = 0usize;
    for i in 0..n {
        if rank[i] > 0 {
            let j = sa[rank[i] as usize - 1] as usize;
            while i + h < n && j + h < n && tokens[i + h] == tokens[j + h] {
                h += 1;
            }
            lcp[rank[i] as usize] = h as u32;
            h = h.saturating_sub(1);
        } else {
            h = 0;
        }
    }
    lcp
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionPair {
    pub unit_a: u32,
    pub tok_a: (usize, usize),
    pub unit_b: u32,
    pub tok_b: (usize, usize),
    pub len: usize,
}

/// Left-maximal repeated runs between DIFFERENT units, deduped per unit pair
/// (containment: keep the longest covering run).
pub fn maximal_repeats(corpus: &Corpus, min_len: usize) -> Vec<RegionPair> {
    let debug_timing = std::env::var_os("REPRISE_TIMING").is_some();
    let mut t = std::time::Instant::now();
    let mut mark = |label: &str| {
        if debug_timing {
            eprintln!("  seq {label}: {}ms", t.elapsed().as_millis());
        }
        t = std::time::Instant::now();
    };
    let tokens = &corpus.tokens;
    let sa = suffix_array(tokens);
    mark("suffix-array");
    let lcp = lcp_array(tokens, &sa);
    mark("lcp");
    let mut pairs: Vec<RegionPair> = Vec::new();
    for i in 1..sa.len() {
        let l = lcp[i] as usize;
        if l < min_len {
            continue;
        }
        let a = sa[i - 1] as usize;
        let b = sa[i] as usize;
        // Left-maximality: extendable-left repeats are covered by their parent.
        if a > 0 && b > 0 && tokens[a - 1] == tokens[b - 1] {
            continue;
        }
        let (ua, ub) = (corpus.unit_of[a], corpus.unit_of[b]);
        if ua == u32::MAX || ub == u32::MAX || ua == ub {
            continue;
        }
        let (ua, a, ub, b) = if ua <= ub {
            (ua, a, ub, b)
        } else {
            (ub, b, ua, a)
        };
        pairs.push(RegionPair {
            unit_a: ua,
            tok_a: (a, a + l),
            unit_b: ub,
            tok_b: (b, b + l),
            len: l,
        });
    }
    dedup_contained(pairs)
}

/// Containment dedup per unit pair (spec §5.6): keep runs not covered by a
/// longer run. Formerly `kept.iter().any(...)` over ALL previously-kept
/// regions for every candidate — O(pairs × kept) — even though the covering
/// predicate requires `q.unit_a == p.unit_a && q.unit_b == p.unit_b`, so it
/// can only ever hold between regions of the SAME unit pair (see the
/// brute-force oracle kept as a differential-test reference below). Sorting
/// by `(unit_a, unit_b, Reverse(len))` makes a unit pair's regions
/// contiguous in `pairs`, and since survivors are appended in that same
/// order, they land contiguously in `kept` too — so scoping the scan to the
/// current segment (`kept[segment_start..]`) is behavior-preserving and
/// drops the unit-pair equality check as redundant (the segment already
/// guarantees it).
fn dedup_contained(mut pairs: Vec<RegionPair>) -> Vec<RegionPair> {
    pairs.sort_by_key(|p| (p.unit_a, p.unit_b, std::cmp::Reverse(p.len)));
    let mut kept: Vec<RegionPair> = Vec::new();
    let mut segment_start = 0usize;
    let mut current_pair: Option<(u32, u32)> = None;
    for p in pairs {
        let key = (p.unit_a, p.unit_b);
        if current_pair != Some(key) {
            segment_start = kept.len();
            current_pair = Some(key);
        }
        let covered = kept[segment_start..].iter().any(|q| {
            q.tok_a.0 <= p.tok_a.0
                && p.tok_a.1 <= q.tok_a.1
                && q.tok_b.0 <= p.tok_b.0
                && p.tok_b.1 <= q.tok_b.1
        });
        if !covered {
            kept.push(p);
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The original O(pairs × kept) all-pairs scan, kept only here as the
    /// differential-test oracle for the segment-scoped `dedup_contained`.
    fn brute_dedup_contained(mut pairs: Vec<RegionPair>) -> Vec<RegionPair> {
        pairs.sort_by_key(|p| (p.unit_a, p.unit_b, std::cmp::Reverse(p.len)));
        let mut kept: Vec<RegionPair> = Vec::new();
        for p in pairs {
            let covered = kept.iter().any(|q| {
                q.unit_a == p.unit_a
                    && q.unit_b == p.unit_b
                    && q.tok_a.0 <= p.tok_a.0
                    && p.tok_a.1 <= q.tok_a.1
                    && q.tok_b.0 <= p.tok_b.0
                    && p.tok_b.1 <= q.tok_b.1
            });
            if !covered {
                kept.push(p);
            }
        }
        kept
    }

    #[test]
    fn dedup_contained_matches_brute_force_oracle() {
        // Deterministic LCG (no external rand dependency) exercising many
        // random unit-pair/span shapes against the O(n²) oracle above —
        // mirrors group.rs's `contained_flags_matches_brute_force_oracle`.
        use crate::test_utils::Lcg;

        for seed in 0..50u64 {
            let mut rng = Lcg(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1));
            let unit_count = 2 + rng.range(3);
            let pair_count = 4 + rng.range(20);
            let mut pairs = Vec::new();
            for _ in 0..pair_count {
                let ua = rng.range(unit_count);
                let mut ub = rng.range(unit_count);
                while ub == ua {
                    ub = rng.range(unit_count);
                }
                let (unit_a, unit_b) = (ua.min(ub), ua.max(ub));
                let start_a = rng.range(20) as usize;
                let start_b = rng.range(20) as usize;
                let len = 1 + rng.range(10) as usize;
                pairs.push(RegionPair {
                    unit_a,
                    tok_a: (start_a, start_a + len),
                    unit_b,
                    tok_b: (start_b, start_b + len),
                    len,
                });
            }
            let expected = brute_dedup_contained(pairs.clone());
            let actual = dedup_contained(pairs);
            assert_eq!(actual, expected, "seed {seed}");
        }
    }
}
