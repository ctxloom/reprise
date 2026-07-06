//! Offline retrieval bake-off for reprise's near-tier candidate generation.
//!
//! GOAL: answer with data whether the novel landmark/Shazam retriever earns its keep
//! versus canonical alternatives, measured against an oracle INDEPENDENT of any single
//! retriever. Generalizes `examples/lm_validate.rs`: the landmark reconstruction, the
//! rep/feature substrate, and the trusted `size_gate -> offset_histogram -> anti_unify`
//! verify chain are reused verbatim; a swappable `candidates(reps) -> Vec<(rep,rep)>`
//! slot lets each retriever be evaluated on the SAME features + SAME labeling.
//!
//! Retrievers (all on the shared IR subtree bag substrate):
//!   1. reprise-landmark  — incumbent: rare-peak constellation triples (matchtree.rs verbatim).
//!   2. minhash-lsh       — bag-Jaccard over the floor-6 bag_set, MinHash sig + LSH banding.
//!   3. winnowing         — MOSS-style k-gram winnowing over the floor-3 ordered subtree stream.
//!   4. sourcerer-rare    — inverted rare-feature overlap (landmark's rare peaks WITHOUT the
//!      triples): the ablation isolating whether the constellation earns its keep.
//!
//! Oracles (retriever-independent):
//!   (A) verify-on-union — union all four retrievers' candidates, run the trusted verify chain;
//!       pairs that ACCEPT = the positive set. recall = fraction of that verified-union surfaced.
//!   (B) synthetic — bench-mutations seed<->mutant pairs, known clones by construction.
//!
//! Usage: cargo run --release --example bakeoff -- [<root> ...]   (default root: src)

use rayon::prelude::*;
use reprise::au;
use reprise::config::Config;
use reprise::fingerprint::{self, HashMode, Subtree};
use reprise::lang::Lang;
use reprise::unit::{self, Unit};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;
use xxhash_rust::xxh3::xxh3_128;

/// A set of unit-index pairs (canonical min < max).
type PairSet = HashSet<(usize, usize)>;
/// A list of rep-index pairs (a retriever's candidate output).
type RepPairs = Vec<(u32, u32)>;

/// Whether `lang` extracts via the IR normalizer (mirrors the crate-private `unit::is_ir`).
fn is_ir(lang: Lang, cfg: &Config) -> bool {
    cfg.normalize.normalizer == "ir" && reprise::frontend::has_ir_frontend(lang)
}

// ============================ shared feature substrate ============================

/// Per-eligible-unit features. ALL retrievers draw from the one subtree inventory
/// (`subtree_inventory(tree, 3, MaskedLocals)`); which slice each uses is documented
/// per field so any feature difference is attributable to the retrieval *scheme*.
struct Rep {
    unit_idx: usize,
    /// Floor-6 dedup subtree-hash set (matchtree's `bag_set`, the D16 bag layer's features).
    /// Substrate for MinHash+LSH.
    bag_set: Vec<u128>,
    /// Floor-3 subtree-hash -> sorted offsets, hash-sorted: the histogram-verify inventory
    /// (shared, retriever-independent — it is part of the trusted oracle chain, not retrieval).
    offsets: Vec<(u128, Vec<u32>)>,
    /// Floor-3 rare peaks `(offset, hash)` with `df <= rare_cap` (df over floor-6 bag_set),
    /// sorted by offset. The EXACT set matchtree pairs into triples. Substrate for landmark
    /// (constellation) and sourcerer (bare overlap) — the ablation shares this substrate exactly.
    rare_peaks: Vec<(u32, u128)>,
    /// Dedup sorted rare-peak hashes. Substrate for sourcerer.
    rare_set: Vec<u128>,
    /// Floor-3 subtree hashes in pre-order (offset order), folded to u64. Substrate for winnowing.
    stream: Vec<u64>,
}

/// One language partition: the rep set plus the unit-index<->rep-index maps.
struct LangCorpus {
    lang: Lang,
    ir: bool,
    reps: Vec<Rep>,
}

fn fold128(h: u128) -> u64 {
    (h as u64) ^ ((h >> 64) as u64)
}

/// Build the shared substrate for one language, mirroring matchtree.rs / lm_validate.rs.
fn build_lang_corpus(units: &[Unit], lang: Lang, cfg: &Config) -> Option<LangCorpus> {
    let ir = is_ir(lang, cfg);
    let floor = cfg.min_unit_floor();
    let mut seen_fp = HashSet::new();
    let eligible: Vec<usize> = units
        .iter()
        .enumerate()
        .filter(|(_, u)| u.lang == lang && u.token_count >= floor)
        .filter(|(_, u)| seen_fp.insert(u.fingerprint))
        .map(|(idx, _)| idx)
        .collect();
    if eligible.len() < 2 {
        return None;
    }

    // reps: floor-3 inventory -> offsets + stream; floor-6 -> bag_set.
    struct Partial {
        unit_idx: usize,
        bag_set: Vec<u128>,
        offsets: Vec<(u128, Vec<u32>)>,
        stream: Vec<u64>,
    }
    let mut partials: Vec<Partial> = eligible
        .par_iter()
        .map(|&idx| {
            let inv: Vec<Subtree> =
                fingerprint::subtree_inventory(&units[idx].tree, 3, HashMode::MaskedLocals);
            // offsets (hash-sorted, hash -> offsets)
            let mut flat: Vec<(u128, u32)> = inv.iter().map(|s| (s.hash, s.offset)).collect();
            flat.sort_unstable();
            let mut offsets: Vec<(u128, Vec<u32>)> = Vec::new();
            for (h, o) in &flat {
                match offsets.last_mut() {
                    Some((last, offs)) if last == h => offs.push(*o),
                    _ => offsets.push((*h, vec![*o])),
                }
            }
            // stream: floor-3 hashes in pre-order (offset order)
            let mut by_off: Vec<(u32, u128)> = inv.iter().map(|s| (s.offset, s.hash)).collect();
            by_off.sort_unstable();
            let stream: Vec<u64> = by_off.iter().map(|(_, h)| fold128(*h)).collect();
            // bag_set: floor-6
            let mut bag_set: Vec<u128> = inv
                .iter()
                .filter(|s| s.tokens >= cfg.thresholds.bag_min_subtree_tokens)
                .map(|s| s.hash)
                .collect();
            bag_set.sort_unstable();
            bag_set.dedup();
            Partial {
                unit_idx: idx,
                bag_set,
                offsets,
                stream,
            }
        })
        .collect();

    // corpus df over floor-6 bag_set; rare_cap identical to matchtree.
    let mut df: HashMap<u128, u32> = HashMap::new();
    for p in &partials {
        for h in &p.bag_set {
            *df.entry(*h).or_insert(0) += 1;
        }
    }
    let rare_cap = 3.max(partials.len() as u32 / 20);

    let reps: Vec<Rep> = partials
        .drain(..)
        .map(|p| {
            let mut rare_peaks: Vec<(u32, u128)> = p
                .offsets
                .iter()
                .filter(|(h, _)| df.get(h).copied().unwrap_or(0) <= rare_cap)
                .flat_map(|(h, offs)| offs.iter().map(move |o| (*o, *h)))
                .collect();
            rare_peaks.sort_unstable();
            let mut rare_set: Vec<u128> = rare_peaks.iter().map(|(_, h)| *h).collect();
            rare_set.sort_unstable();
            rare_set.dedup();
            Rep {
                unit_idx: p.unit_idx,
                bag_set: p.bag_set,
                offsets: p.offsets,
                rare_peaks,
                rare_set,
                stream: p.stream,
            }
        })
        .collect();

    Some(LangCorpus { lang, ir, reps })
}

// ============================ generic shared-count retrieval ============================

/// Candidate pairs sharing >= `min_shared` features, skipping features owned by more than
/// `df_cap` units. Full clique per feature (matchtree uses owner_pair_window=0 by default).
/// Returns rep-index pairs (i < j).
fn shared_count_pairs<F>(
    reps: &[Rep],
    feats: F,
    df_cap: usize,
    min_shared: usize,
) -> Vec<(u32, u32)>
where
    F: Fn(&Rep) -> &[u128],
{
    let mut owners: HashMap<u128, Vec<u32>> = HashMap::new();
    for (i, rep) in reps.iter().enumerate() {
        for &h in feats(rep) {
            owners.entry(h).or_default().push(i as u32);
        }
    }
    let mut counts: HashMap<(u32, u32), u32> = HashMap::new();
    for own in owners.values() {
        if own.len() < 2 || own.len() > df_cap {
            continue;
        }
        for a in 0..own.len() {
            for b in a + 1..own.len() {
                *counts.entry((own[a], own[b])).or_insert(0) += 1;
            }
        }
    }
    counts
        .into_iter()
        .filter(|&(_, c)| c as usize >= min_shared)
        .map(|(p, _)| p)
        .collect()
}

// ---- retriever 1: reprise-landmark (matchtree.rs verbatim) ----

/// Constellation triple hashes for one rep (matchtree.rs:154-176 verbatim).
fn landmark_hashes(rep: &Rep) -> Vec<u128> {
    const FAN_OUT: usize = 3;
    let rare = &rep.rare_peaks; // already offset-sorted
    let mut out = Vec::new();
    for i in 0..rare.len() {
        for j in i + 1..(i + 1 + FAN_OUT).min(rare.len()) {
            let delta = (rare[j].0 - rare[i].0) / 8;
            let mut buf = Vec::with_capacity(36);
            buf.extend_from_slice(&rare[i].1.to_le_bytes());
            buf.extend_from_slice(&rare[j].1.to_le_bytes());
            buf.extend_from_slice(&delta.to_le_bytes());
            out.push(xxh3_128(&buf));
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Landmark retrieval at a configurable `min_shared` (matchtree default 2, D22 used 4).
fn retr_landmark_ms(reps: &[Rep], min_shared: usize) -> (Vec<(u32, u32)>, usize) {
    let lms: Vec<Vec<u128>> = reps.par_iter().map(landmark_hashes).collect();
    let entries: usize = lms.iter().map(|l| l.len()).sum();
    let mut owners: HashMap<u128, Vec<u32>> = HashMap::new();
    for (i, l) in lms.iter().enumerate() {
        for &h in l {
            owners.entry(h).or_default().push(i as u32);
        }
    }
    let df_cap = 50usize;
    let mut counts: HashMap<(u32, u32), u32> = HashMap::new();
    for own in owners.values() {
        if own.len() < 2 || own.len() > df_cap {
            continue;
        }
        for a in 0..own.len() {
            for b in a + 1..own.len() {
                *counts.entry((own[a], own[b])).or_insert(0) += 1;
            }
        }
    }
    let pairs = counts
        .into_iter()
        .filter(|&(_, c)| c as usize >= min_shared)
        .map(|(p, _)| p)
        .collect();
    (pairs, entries)
}

fn retr_landmark(reps: &[Rep]) -> (Vec<(u32, u32)>, usize) {
    retr_landmark_ms(reps, 2)
}

// ---- retriever 4: sourcerer-rare (ablation: rare peaks, no triples) ----

fn retr_sourcerer(reps: &[Rep], min_shared: usize) -> (Vec<(u32, u32)>, usize) {
    let entries: usize = reps.iter().map(|r| r.rare_set.len()).sum();
    let pairs = shared_count_pairs(reps, |r| &r.rare_set, 50, min_shared);
    (pairs, entries)
}

// ---- retriever 3: winnowing (MOSS-style k-gram) ----

/// Winnowing fingerprints of a stream: k-grams hashed, min selected per window of `w`
/// (rightmost minimum). Returns dedup selected gram hashes.
fn winnow(stream: &[u64], k: usize, w: usize) -> Vec<u128> {
    if stream.is_empty() {
        return Vec::new();
    }
    // k-gram hashes (numeric, used directly for min selection).
    let grams: Vec<u128> = if stream.len() < k {
        let mut buf = Vec::with_capacity(stream.len() * 8);
        for &x in stream {
            buf.extend_from_slice(&x.to_le_bytes());
        }
        vec![xxh3_128(&buf)]
    } else {
        (0..=stream.len() - k)
            .map(|i| {
                let mut buf = Vec::with_capacity(k * 8);
                for &x in &stream[i..i + k] {
                    buf.extend_from_slice(&x.to_le_bytes());
                }
                xxh3_128(&buf)
            })
            .collect()
    };
    if grams.len() <= w {
        // one window: select the single minimum
        let mut sel = *grams.iter().min().unwrap();
        // dedup trivial
        let mut out = vec![std::mem::take(&mut sel)];
        out.sort_unstable();
        out.dedup();
        return out;
    }
    let mut positions: Vec<usize> = Vec::new();
    for start in 0..=grams.len() - w {
        let window = &grams[start..start + w];
        // rightmost minimum
        let mut min_i = 0;
        for i in 1..w {
            if window[i] <= window[min_i] {
                min_i = i;
            }
        }
        let pos = start + min_i;
        if positions.last() != Some(&pos) {
            positions.push(pos);
        }
    }
    positions.sort_unstable();
    positions.dedup();
    let mut out: Vec<u128> = positions.iter().map(|&p| grams[p]).collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn retr_winnow(reps: &[Rep], k: usize, w: usize, min_shared: usize) -> (Vec<(u32, u32)>, usize) {
    let grams: Vec<Vec<u128>> = reps.par_iter().map(|r| winnow(&r.stream, k, w)).collect();
    let entries: usize = grams.iter().map(|g| g.len()).sum();
    let mut owners: HashMap<u128, Vec<u32>> = HashMap::new();
    for (i, g) in grams.iter().enumerate() {
        for &h in g {
            owners.entry(h).or_default().push(i as u32);
        }
    }
    let df_cap = 50usize;
    let mut counts: HashMap<(u32, u32), u32> = HashMap::new();
    for own in owners.values() {
        if own.len() < 2 || own.len() > df_cap {
            continue;
        }
        for a in 0..own.len() {
            for b in a + 1..own.len() {
                *counts.entry((own[a], own[b])).or_insert(0) += 1;
            }
        }
    }
    let pairs = counts
        .into_iter()
        .filter(|&(_, c)| c as usize >= min_shared)
        .map(|(p, _)| p)
        .collect();
    (pairs, entries)
}

// ---- retriever 2: minhash + LSH ----

const MINHASH_K: usize = 128;

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

/// Deterministic MinHash permutation params (a odd, b arbitrary).
fn minhash_params() -> (Vec<u64>, Vec<u64>) {
    let mut s = 0xD1B54A32D192ED03u64;
    let mut a = Vec::with_capacity(MINHASH_K);
    let mut b = Vec::with_capacity(MINHASH_K);
    for _ in 0..MINHASH_K {
        a.push(splitmix64(&mut s) | 1);
        b.push(splitmix64(&mut s));
    }
    (a, b)
}

fn minhash_sig(bag: &[u128], a: &[u64], b: &[u64]) -> Vec<u64> {
    let mut sig = vec![u64::MAX; MINHASH_K];
    for &e in bag {
        let fe = fold128(e);
        for k in 0..MINHASH_K {
            let hv = a[k].wrapping_mul(fe).wrapping_add(b[k]);
            if hv < sig[k] {
                sig[k] = hv;
            }
        }
    }
    sig
}

/// MinHash+LSH: `bands` bands of `rows` rows (bands*rows = K). Two units are candidates
/// when they collide in any band. Approx Jaccard threshold (1/bands)^(1/rows).
fn retr_minhash(reps: &[Rep], bands: usize, rows: usize) -> (Vec<(u32, u32)>, usize) {
    assert!(bands * rows <= MINHASH_K);
    let (a, b) = minhash_params();
    let sigs: Vec<Vec<u64>> = reps
        .par_iter()
        .map(|r| minhash_sig(&r.bag_set, &a, &b))
        .collect();
    // index entries = signatures (n*K) as the stored-index footprint.
    let entries = sigs.len() * MINHASH_K;
    let mut buckets: HashMap<u128, Vec<u32>> = HashMap::new();
    for (i, sig) in sigs.iter().enumerate() {
        for band in 0..bands {
            let mut buf = Vec::with_capacity(rows * 8 + 2);
            buf.extend_from_slice(&(band as u16).to_le_bytes());
            for r in 0..rows {
                buf.extend_from_slice(&sig[band * rows + r].to_le_bytes());
            }
            buckets.entry(xxh3_128(&buf)).or_default().push(i as u32);
        }
    }
    let df_cap = 50usize; // ubiquitous-signature guard (parity with the other retrievers' df_cap)
    let mut pairs: HashSet<(u32, u32)> = HashSet::new();
    for own in buckets.values() {
        if own.len() < 2 || own.len() > df_cap {
            continue;
        }
        for x in 0..own.len() {
            for y in x + 1..own.len() {
                pairs.insert((own[x].min(own[y]), own[x].max(own[y])));
            }
        }
    }
    (pairs.into_iter().collect(), entries)
}

// ============================ trusted verify chain (lm_validate.rs verbatim) ============================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verdict {
    SizeRej,
    HistRej,
    DivRej,
    Weak,
    Accept,
}

fn verify(
    ua: &Unit,
    ub: &Unit,
    a: &Rep,
    b: &Rep,
    profile: &'static dyn reprise::lang::LanguageProfile,
    ir: bool,
    cfg: &Config,
) -> Verdict {
    if !size_gate_passes(ua.token_count, ub.token_count, cfg) {
        return Verdict::SizeRej;
    }
    if !offset_histogram_passes(a, b, cfg) {
        return Verdict::HistRej;
    }
    let outcome = au::anti_unify(&ua.tree, &ub.tree, profile, ir);
    if outcome.divergence > cfg.thresholds.max_divergence {
        return Verdict::DivRej;
    }
    if outcome.holes.len() > cfg.thresholds.max_holes as usize || !outcome.factorable {
        return Verdict::Weak;
    }
    Verdict::Accept
}

fn size_gate_passes(ta: u32, tb: u32, cfg: &Config) -> bool {
    let (lo, hi) = (u64::from(ta.min(tb)), u64::from(ta.max(tb)));
    let ratio = 1.0 + 2.0 * cfg.thresholds.max_divergence + 0.15;
    hi as f64 <= lo as f64 * ratio + 64.0
}

fn offset_histogram_passes(a: &Rep, b: &Rep, cfg: &Config) -> bool {
    let min_inventory = a.offsets.len().min(b.offsets.len()) as u32;
    let needed = cfg.histogram_min_votes().max(min_inventory / 8);
    let mut bins: HashMap<i64, u32> = HashMap::new();
    let mut best = 0u32;
    let mut hash_bins: Vec<i64> = Vec::with_capacity(16);
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.offsets.len() && j < b.offsets.len() {
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

// ============================ retriever registry + runner ============================

const RETRIEVERS: [&str; 4] = [
    "reprise-landmark",
    "minhash-lsh",
    "winnowing",
    "sourcerer-rare",
];

#[derive(Default, Clone)]
struct Cost {
    build_ms: f64,
    query_ms: f64,
    index_entries: usize,
}

/// One retriever's candidate set for one lang, as unit-index pairs (min,max) + timing.
struct RetrOut {
    pairs: HashSet<(usize, usize)>, // unit-index pairs
    cost: Cost,
}

fn run_retriever(name: &str, lc: &LangCorpus) -> RetrOut {
    let reps = &lc.reps;
    let to_units = |raw: Vec<(u32, u32)>| -> HashSet<(usize, usize)> {
        raw.into_iter()
            .map(|(i, j)| {
                let (a, b) = (reps[i as usize].unit_idx, reps[j as usize].unit_idx);
                (a.min(b), a.max(b))
            })
            .collect()
    };
    let t = Instant::now();
    let (raw, entries) = match name {
        "reprise-landmark" => retr_landmark(reps),
        "minhash-lsh" => retr_minhash(reps, 32, 4), // thr ~ (1/32)^(1/4) ~ 0.42
        "winnowing" => retr_winnow(reps, 5, 4, 2),
        "sourcerer-rare" => retr_sourcerer(reps, 2),
        _ => unreachable!(),
    };
    let elapsed = t.elapsed().as_secs_f64() * 1000.0;
    RetrOut {
        pairs: to_units(raw),
        cost: Cost {
            // build+query aren't separable cheaply here; report combined as query_ms,
            // and index footprint as index_entries.
            build_ms: 0.0,
            query_ms: elapsed,
            index_entries: entries,
        },
    }
}

/// All retrievers over a corpus, per lang. Returns (per-retriever unit-pair set, per-retriever
/// cost) aggregated across langs, plus the lang corpora (for verify) and rep-index maps.
struct CorpusRun {
    langs: Vec<LangCorpus>,
    // retriever name -> unit-pair set (union across langs)
    retr_pairs: HashMap<String, HashSet<(usize, usize)>>,
    retr_cost: HashMap<String, Cost>,
}

fn run_corpus(units: &[Unit], cfg: &Config) -> CorpusRun {
    let mut langs_present: Vec<Lang> = units.iter().map(|u| u.lang).collect();
    langs_present.sort();
    langs_present.dedup();

    let langs: Vec<LangCorpus> = langs_present
        .iter()
        .filter_map(|&l| build_lang_corpus(units, l, cfg))
        .collect();

    let mut retr_pairs: HashMap<String, HashSet<(usize, usize)>> = HashMap::new();
    let mut retr_cost: HashMap<String, Cost> = HashMap::new();
    for name in RETRIEVERS {
        let mut all = HashSet::new();
        let mut cost = Cost::default();
        for lc in &langs {
            let out = run_retriever(name, lc);
            all.extend(out.pairs);
            cost.build_ms += out.cost.build_ms;
            cost.query_ms += out.cost.query_ms;
            cost.index_entries += out.cost.index_entries;
        }
        retr_pairs.insert(name.to_string(), all);
        retr_cost.insert(name.to_string(), cost);
    }
    CorpusRun {
        langs,
        retr_pairs,
        retr_cost,
    }
}

/// Verify the union of candidate pairs (verify-on-union oracle A). Returns the ACCEPT and
/// WEAK positive sets as unit-pair sets, plus a per-pair verdict memo (for precision).
fn verify_union(units: &[Unit], run: &CorpusRun) -> (PairSet, PairSet) {
    // rep lookup per lang
    let mut rep_of_unit: HashMap<usize, (usize, usize)> = HashMap::new(); // unit_idx -> (lang_i, rep_i)
    for (li, lc) in run.langs.iter().enumerate() {
        for (ri, rep) in lc.reps.iter().enumerate() {
            rep_of_unit.insert(rep.unit_idx, (li, ri));
        }
    }
    let mut union: HashSet<(usize, usize)> = HashSet::new();
    for set in run.retr_pairs.values() {
        union.extend(set.iter().copied());
    }
    let union_vec: Vec<(usize, usize)> = union.into_iter().collect();
    let verdicts: Vec<Verdict> = union_vec
        .par_iter()
        .map(|&(ua, ub)| {
            let (la, ra) = rep_of_unit[&ua];
            let (_lb, rb) = rep_of_unit[&ub];
            let lc = &run.langs[la];
            let profile = lc.lang.profile();
            verify(
                &units[ua],
                &units[ub],
                &lc.reps[ra],
                &lc.reps[rb],
                profile,
                lc.ir,
                &Config::default(),
            )
        })
        .collect();
    let mut accept = HashSet::new();
    let mut weak = HashSet::new();
    for (&pair, &v) in union_vec.iter().zip(verdicts.iter()) {
        match v {
            Verdict::Accept => {
                accept.insert(pair);
            }
            Verdict::Weak => {
                weak.insert(pair);
            }
            _ => {}
        }
    }
    (accept, weak)
}

// ============================ oracle B: synthetic mutation corpus ============================

#[derive(serde::Deserialize)]
struct SeedMeta {
    #[serde(default)]
    rename: BTreeMap<String, String>,
    #[serde(default)]
    literals: BTreeMap<String, String>,
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}
fn replace_words(src: &str, map: &BTreeMap<String, String>) -> String {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort_by_key(|k| std::cmp::Reverse(k.len()));
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    'outer: while i < bytes.len() {
        let prev_ok = i == 0 || !is_word(bytes[i - 1] as char);
        if prev_ok {
            for key in &keys {
                let k = key.as_bytes();
                if bytes[i..].starts_with(k) {
                    let next = i + k.len();
                    let next_ok = next >= bytes.len() || !is_word(bytes[next] as char);
                    if next_ok {
                        out.push_str(&map[*key]);
                        i = next;
                        continue 'outer;
                    }
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}
fn replace_literals(src: &str, map: &BTreeMap<String, String>) -> String {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort_by_key(|k| std::cmp::Reverse(k.len()));
    let mut out = src.to_string();
    for key in keys {
        out = out.replace(key.as_str(), &map[key]);
    }
    out
}
fn mutate_whitespace(src: &str, ext: &str) -> String {
    let mut out = String::new();
    for line in src.lines() {
        out.push_str(line);
        if !line.trim().is_empty() {
            out.push_str("   ");
        }
        out.push('\n');
        out.push('\n');
    }
    if ext == "rs" {
        out = out
            .lines()
            .map(|l| {
                if l.trim().is_empty() {
                    String::new()
                } else {
                    format!("  {l}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        out.push('\n');
    }
    out
}
fn mutate_comments(src: &str, ext: &str) -> String {
    let marker = if ext == "py" {
        "# mutation noise"
    } else {
        "// mutation noise"
    };
    let mut out = String::new();
    for line in src.lines() {
        if !line.trim().is_empty() {
            let indent: String = line.chars().take_while(|c| *c == ' ').collect();
            out.push_str(&indent);
            out.push_str(marker);
            out.push('\n');
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

struct SynPair {
    seed_unit: usize,
    mutant_unit: usize,
    class: String,
}

/// Build the synthetic corpus: every seed + its programmatic (t1/t2) mutants + curated
/// (t3/t2r/t4) variant files, all langs. Returns the units and the known seed<->mutant pairs.
fn build_synthetic(cfg: &Config) -> (Vec<Unit>, Vec<SynPair>) {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut units: Vec<Unit> = Vec::new();
    let mut pairs: Vec<SynPair> = Vec::new();

    // (file, source) -> extract first eligible unit index; push all its units, return the
    // index of the primary (largest-token) unit, or None.
    let floor = cfg.min_unit_floor();
    let push_file = |units: &mut Vec<Unit>, name: &str, src: &str, lang: Lang| -> Option<usize> {
        let path = PathBuf::from(format!("syn/{name}"));
        let (fus, _) = unit::extract_file_units(&path, src, lang, cfg);
        let start = units.len();
        let mut best: Option<usize> = None;
        for (k, u) in fus.into_iter().enumerate() {
            let idx = start + k;
            let tok = u.token_count;
            units.push(u);
            if tok >= floor && best.is_none_or(|b| units[b].token_count < tok) {
                best = Some(idx);
            }
        }
        best
    };

    for (dir, ext, lang) in [
        ("rust", "rs", Lang::Rust),
        ("python", "py", Lang::Python),
        ("go", "go", Lang::Go),
        ("typescript", "ts", Lang::TypeScript),
        ("kotlin", "kt", Lang::Kotlin),
    ] {
        let seed_dir = manifest.join("benches/mutations/seeds").join(dir);
        let var_dir = manifest.join("benches/mutations/variants").join(dir);
        let mut seed_paths: Vec<PathBuf> = match std::fs::read_dir(&seed_dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(ext))
                .collect(),
            Err(_) => continue,
        };
        seed_paths.sort();
        for sp in seed_paths {
            let stem = sp.file_stem().unwrap().to_str().unwrap().to_string();
            let src = std::fs::read_to_string(&sp).unwrap();
            let meta: SeedMeta = toml::from_str(
                &std::fs::read_to_string(sp.with_extension("toml")).unwrap_or_default(),
            )
            .unwrap_or(SeedMeta {
                rename: BTreeMap::new(),
                literals: BTreeMap::new(),
            });
            let Some(seed_idx) = push_file(&mut units, &format!("{dir}_{stem}.{ext}"), &src, lang)
            else {
                continue;
            };
            // programmatic mutants
            let progs: Vec<(&str, String)> = vec![
                ("t1-whitespace", mutate_whitespace(&src, ext)),
                ("t1-comments", mutate_comments(&src, ext)),
                ("t2-rename", replace_words(&src, &meta.rename)),
                ("t2-literals", replace_literals(&src, &meta.literals)),
            ];
            for (class, msrc) in progs {
                if msrc == src {
                    continue;
                }
                if let Some(mi) = push_file(
                    &mut units,
                    &format!("{dir}_{stem}.{class}.{ext}"),
                    &msrc,
                    lang,
                ) {
                    pairs.push(SynPair {
                        seed_unit: seed_idx,
                        mutant_unit: mi,
                        class: class.to_string(),
                    });
                }
            }
            // curated variant files
            if let Ok(rd) = std::fs::read_dir(&var_dir) {
                let mut vpaths: Vec<PathBuf> =
                    rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
                vpaths.sort();
                for vp in vpaths {
                    let fname = vp.file_name().unwrap().to_str().unwrap();
                    let prefix = format!("{stem}.");
                    let suffix = format!(".{ext}");
                    let Some(rest) = fname.strip_prefix(&prefix) else {
                        continue;
                    };
                    let Some(class) = rest.strip_suffix(&suffix) else {
                        continue;
                    };
                    if class.ends_with(".helper") {
                        continue;
                    }
                    let vsrc = std::fs::read_to_string(&vp).unwrap();
                    if let Some(mi) = push_file(
                        &mut units,
                        &format!("{dir}_{stem}.{class}.{ext}"),
                        &vsrc,
                        lang,
                    ) {
                        pairs.push(SynPair {
                            seed_unit: seed_idx,
                            mutant_unit: mi,
                            class: class.to_string(),
                        });
                    }
                }
            }
        }
    }
    (units, pairs)
}

// ============================ fidelity vs reprise::scan ============================

/// Re-prove the landmark reconstruction against the shipping pipeline: run reprise::scan
/// with inline disabled (both see plain units) and compare candidate + verify accounting.
fn fidelity_check(root: &Path) -> String {
    let mut cfg = Config::default();
    cfg.cache.enabled = false;
    cfg.inline.enabled = false; // both reconstruction and scan see plain units only
    let report = match reprise::scan(root, &cfg) {
        Ok(r) => r,
        Err(e) => return format!("  scan failed: {e}"),
    };
    let rs = &report.stats.retrieval;

    // reconstruct landmark candidates + verify over the SAME corpus
    let mut units: Vec<Unit> = Vec::new();
    let files = reprise::walk::collect_files(root, &cfg).expect("collect_files");
    for (path, lang) in files {
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        if reprise::walk::is_generated(&src, &cfg) {
            continue;
        }
        let (fus, _rep) = unit::extract_file_units(&path, &src, lang, &cfg);
        units.extend(fus);
    }
    let mut langs: Vec<Lang> = units.iter().map(|u| u.lang).collect();
    langs.sort();
    langs.dedup();

    let mut recon_landmark = 0usize;
    let mut recon_hist_rej = 0usize;
    let mut recon_verified = 0usize; // accept + weak (matchtree counts both as verified)
    for lang in langs {
        let Some(lc) = build_lang_corpus(&units, lang, &cfg) else {
            continue;
        };
        let (raw, _entries) = retr_landmark(&lc.reps);
        recon_landmark += raw.len();
        let profile = lang.profile();
        let verdicts: Vec<Verdict> = raw
            .par_iter()
            .map(|&(i, j)| {
                verify(
                    &units[lc.reps[i as usize].unit_idx],
                    &units[lc.reps[j as usize].unit_idx],
                    &lc.reps[i as usize],
                    &lc.reps[j as usize],
                    profile,
                    lc.ir,
                    &cfg,
                )
            })
            .collect();
        for v in verdicts {
            match v {
                Verdict::HistRej => recon_hist_rej += 1,
                Verdict::DivRej => {}
                Verdict::Weak | Verdict::Accept => recon_verified += 1,
                Verdict::SizeRej => {}
            }
        }
    }
    let cand_ok = recon_landmark == rs.candidates_landmark;
    let hist_ok = recon_hist_rej == rs.histogram_rejected;
    let ver_ok = recon_verified == rs.verified_pairs;
    format!(
        "  candidates_landmark: recon {} vs scan {} -> {}\n  \
         histogram_rejected:  recon {} vs scan {} -> {}\n  \
         verified_pairs:      recon {} vs scan {} -> {}\n  \
         OVERALL: {}",
        recon_landmark,
        rs.candidates_landmark,
        if cand_ok { "MATCH" } else { "DIFF" },
        recon_hist_rej,
        rs.histogram_rejected,
        if hist_ok { "MATCH" } else { "DIFF" },
        recon_verified,
        rs.verified_pairs,
        if ver_ok { "MATCH" } else { "DIFF" },
        if cand_ok && hist_ok && ver_ok {
            "FIDELITY PROVEN"
        } else {
            "FIDELITY MISMATCH (see above)"
        },
    )
}

// ============================ main ============================

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let roots: Vec<PathBuf> = if args.is_empty() {
        vec![PathBuf::from("src")]
    } else {
        args.iter().map(PathBuf::from).collect()
    };
    let mut cfg = Config::default();
    cfg.cache.enabled = false;
    cfg.inline.enabled = false; // plain units only, matches the fidelity-proven substrate

    // ---- fidelity ----
    println!("=== FIDELITY: landmark reconstruction vs reprise::scan (inline off) ===");
    println!("{}", fidelity_check(&roots[0]));

    // ---- collect real corpus ----
    let mut units: Vec<Unit> = Vec::new();
    for root in &roots {
        let files = reprise::walk::collect_files(root, &cfg).expect("collect_files");
        for (path, lang) in files {
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            if reprise::walk::is_generated(&src, &cfg) {
                continue;
            }
            let (fus, _rep) = unit::extract_file_units(&path, &src, lang, &cfg);
            units.extend(fus);
        }
    }
    let n_units = units.len();
    let run = run_corpus(&units, &cfg);
    let (accept, weak) = verify_union(&units, &run);

    println!(
        "\n=== ORACLE A: verify-on-union  (corpus: {} plain units, {:?}) ===",
        n_units, roots
    );
    let union_size: usize = {
        let mut u = HashSet::new();
        for s in run.retr_pairs.values() {
            u.extend(s.iter().copied());
        }
        u.len()
    };
    println!(
        "  union candidate pairs: {}   verified ACCEPT: {}   WEAK: {}",
        union_size,
        accept.len(),
        weak.len()
    );

    // per-retriever oracle-A metrics
    println!("\n  retriever         flood     recallA   precision   idx-entries   query-ms");
    let mut rowsa: Vec<(String, usize, f64, f64, usize, f64)> = Vec::new();
    for name in RETRIEVERS {
        let set = &run.retr_pairs[name];
        let cost = &run.retr_cost[name];
        let flood = set.len();
        let surfaced = set.iter().filter(|p| accept.contains(*p)).count();
        let recall = surfaced as f64 / accept.len().max(1) as f64;
        // raw precision = candidates that verify ACCEPT / candidates
        let prec = surfaced as f64 / flood.max(1) as f64;
        rowsa.push((
            name.to_string(),
            flood,
            recall,
            prec,
            cost.index_entries,
            cost.query_ms,
        ));
        println!(
            "  {name:<16} {flood:>7}    {recall:>6.3}    {prec:>7.4}   {:>11}   {:>7.1}",
            cost.index_entries, cost.query_ms
        );
    }

    // ---- oracle B ----
    let (syn_units, syn_pairs) = build_synthetic(&cfg);
    let syn_run = run_corpus(&syn_units, &cfg);
    // classify known pairs
    #[derive(Default)]
    struct BClass {
        near: Vec<(usize, usize, String)>, // distinct-fp near pairs
        exact: usize,
        inline: usize,
        excluded: usize,
        ctl: Vec<(usize, usize)>,
    }
    let mut bc = BClass::default();
    for p in &syn_pairs {
        let key = (
            p.seed_unit.min(p.mutant_unit),
            p.seed_unit.max(p.mutant_unit),
        );
        if p.class.starts_with("t4-") {
            bc.inline += 1;
            continue;
        }
        if p.class.starts_with("ctl-") {
            bc.ctl.push(key);
            continue;
        }
        let su = &syn_units[p.seed_unit];
        let mu = &syn_units[p.mutant_unit];
        let floor = cfg.min_unit_floor();
        if su.token_count < floor || mu.token_count < floor {
            bc.excluded += 1;
        } else if su.fingerprint == mu.fingerprint {
            bc.exact += 1;
        } else {
            bc.near.push((key.0, key.1, p.class.clone()));
        }
    }
    println!(
        "\n=== ORACLE B: synthetic mutation clones  ({} seed corpus units) ===",
        syn_units.len()
    );
    println!(
        "  known seed<->mutant pairs: near(distinct-fp)={}  exact(same-fp, exact-tier)={}  \
         inline-tier(t4)={}  below-floor/excluded={}  control(ctl)={}",
        bc.near.len(),
        bc.exact,
        bc.inline,
        bc.excluded,
        bc.ctl.len()
    );
    println!("\n  retriever         recallB   surfaced/near   ctl-surfaced(FP)");
    let mut rowsb: Vec<(String, f64)> = Vec::new();
    for name in RETRIEVERS {
        let set = &syn_run.retr_pairs[name];
        let surfaced = bc
            .near
            .iter()
            .filter(|(a, b, _)| set.contains(&(*a, *b)))
            .count();
        let ctl_fp = bc.ctl.iter().filter(|k| set.contains(k)).count();
        let recall = surfaced as f64 / bc.near.len().max(1) as f64;
        rowsb.push((name.to_string(), recall));
        println!(
            "  {name:<16} {recall:>6.3}      {surfaced:>3}/{:<3}          {ctl_fp}",
            bc.near.len()
        );
    }

    // ---- knob sweep: fair operating points (don't cripple a challenger) ----
    // For each variant compute flood + recallA (vs verify-union ACCEPT) + recallB (vs synthetic
    // near). Answers: can a tuned challenger reach landmark's recall, and at what flood?
    let variant_pairs = |langs: &[LangCorpus], f: &dyn Fn(&[Rep]) -> RepPairs| -> PairSet {
        let mut all = HashSet::new();
        for lc in langs {
            for (i, j) in f(&lc.reps) {
                let (a, b) = (lc.reps[i as usize].unit_idx, lc.reps[j as usize].unit_idx);
                all.insert((a.min(b), a.max(b)));
            }
        }
        all
    };
    let eval = |label: &str, real: &HashSet<(usize, usize)>, syn: &HashSet<(usize, usize)>| {
        let flood = real.len();
        let ra =
            real.iter().filter(|p| accept.contains(*p)).count() as f64 / accept.len().max(1) as f64;
        let rb = bc
            .near
            .iter()
            .filter(|(a, b, _)| syn.contains(&(*a, *b)))
            .count() as f64
            / bc.near.len().max(1) as f64;
        println!("    {label:<34} floodA {flood:>7}   recallA {ra:>6.3}   recallB {rb:>6.3}");
    };
    println!("\n=== KNOB SWEEP (recall-flood curves — fair operating points) ===");
    println!("  -- reprise-landmark (min_shared) [incumbent; default 2] --");
    for ms in [2usize, 3, 4, 6, 8] {
        let r = variant_pairs(&run.langs, &move |reps| retr_landmark_ms(reps, ms).0);
        let s = variant_pairs(&syn_run.langs, &move |reps| retr_landmark_ms(reps, ms).0);
        eval(&format!("min_shared={ms}"), &r, &s);
    }
    println!("  -- minhash-lsh (bands x rows, approx Jaccard thr) --");
    for (bnd, row, thr) in [
        (16usize, 8usize, "0.68"),
        (32, 4, "0.42"),
        (64, 2, "0.125"),
        (128, 1, "0.008"),
    ] {
        let r = variant_pairs(&run.langs, &move |reps| retr_minhash(reps, bnd, row).0);
        let s = variant_pairs(&syn_run.langs, &move |reps| retr_minhash(reps, bnd, row).0);
        eval(&format!("b={bnd} r={row} (thr~{thr})"), &r, &s);
    }
    println!("  -- winnowing (k,w,min_shared) --");
    for (k, w, ms) in [(5usize, 4usize, 1usize), (5, 4, 2), (4, 3, 1), (3, 2, 1)] {
        let r = variant_pairs(&run.langs, &move |reps| retr_winnow(reps, k, w, ms).0);
        let s = variant_pairs(&syn_run.langs, &move |reps| retr_winnow(reps, k, w, ms).0);
        eval(&format!("k={k} w={w} min_shared={ms}"), &r, &s);
    }
    println!("  -- sourcerer-rare (min_shared) [the ablation: landmark rare peaks, NO triples] --");
    for ms in [2usize, 3, 4, 6, 8, 12] {
        let r = variant_pairs(&run.langs, &move |reps| retr_sourcerer(reps, ms).0);
        let s = variant_pairs(&syn_run.langs, &move |reps| retr_sourcerer(reps, ms).0);
        eval(&format!("min_shared={ms}"), &r, &s);
    }

    // ---- verdict ----
    println!("\n=== VERDICT ===");
    let lm = &run.retr_pairs["reprise-landmark"];
    // (1) does landmark miss ACCEPT pairs the canonical schemes catch?
    let missed: Vec<(usize, usize)> = accept
        .iter()
        .filter(|p| !lm.contains(*p))
        .copied()
        .collect();
    println!(
        "(1) landmark recall vs verify-union ACCEPT: {}/{} = {:.3}",
        accept.len() - missed.len(),
        accept.len(),
        (accept.len() - missed.len()) as f64 / accept.len().max(1) as f64
    );
    if missed.is_empty() {
        println!("    landmark misses ZERO verified-union ACCEPT pairs.");
    } else {
        println!(
            "    landmark MISSES {} verified ACCEPT pair(s):",
            missed.len()
        );
        for &(a, b) in &missed {
            let catchers: Vec<&str> = RETRIEVERS
                .iter()
                .filter(|&&r| r != "reprise-landmark" && run.retr_pairs[r].contains(&(a, b)))
                .copied()
                .collect();
            println!(
                "      {}  <->  {}   [caught by: {}]",
                unit_label(&units[a]),
                unit_label(&units[b]),
                if catchers.is_empty() {
                    "NONE".to_string()
                } else {
                    catchers.join(", ")
                }
            );
        }
    }
    // pairs ONLY landmark caught (its unique contribution)
    let only_lm: Vec<(usize, usize)> = accept
        .iter()
        .filter(|p| {
            lm.contains(*p)
                && RETRIEVERS
                    .iter()
                    .filter(|&&r| r != "reprise-landmark")
                    .all(|&r| !run.retr_pairs[r].contains(*p))
        })
        .copied()
        .collect();
    println!(
        "\n(2) verified ACCEPT pairs caught ONLY by landmark (no canonical scheme): {}",
        only_lm.len()
    );
    for &(a, b) in only_lm.iter().take(20) {
        println!(
            "      {}  <->  {}",
            unit_label(&units[a]),
            unit_label(&units[b])
        );
    }

    // (3) can a canonical scheme match landmark recall at lower flood/cost?
    let lm_flood = lm.len();
    let lm_recall = rowsa[0].2;
    println!(
        "\n(3) does any canonical scheme match landmark recall ({:.3}) at lower flood ({})?",
        lm_recall, lm_flood
    );
    for (name, flood, recall, _prec, _idx, _q) in rowsa.iter().skip(1) {
        let verdict = if *recall >= lm_recall - 1e-9 && *flood < lm_flood {
            "YES — matches/beats recall at LOWER flood (grounds to reconsider landmark)"
        } else if *recall >= lm_recall - 1e-9 {
            "recall-match but NOT lower flood"
        } else {
            "no (lower recall)"
        };
        println!("    {name:<16} recall {recall:.3}  flood {flood:>7}  -> {verdict}");
    }
}

fn unit_label(u: &Unit) -> String {
    format!(
        "{}:{} L{}",
        u.file.file_name().and_then(|s| s.to_str()).unwrap_or("?"),
        u.name,
        u.line_span.0
    )
}
