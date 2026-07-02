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

#[derive(Debug, Clone, Copy)]
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
    // Containment dedup per unit pair: keep runs not covered by a longer run.
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
