//! Offline retrieval bake-off for reprise's near-tier candidate generation.
//!
//! GOAL: answer with data whether the novel landmark/Shazam retriever earns its keep
//! versus canonical alternatives, measured against an oracle INDEPENDENT of any single
//! retriever.
//!
//! FIDELITY IS NOW AUTOMATIC. The incumbent races the REAL production code: candidates
//! come from `matchtree::Landmark` (the shipping retriever) over `matchtree::build_reps`
//! (the shipping substrate) — no reconstruction to drift. The alternatives implement the
//! SAME `matchtree::Retriever` trait bench-side, so every entrant is invoked through the
//! identical seam the pipeline uses.
//!
//! Retrievers (all `matchtree::Retriever` over the shared `RepData` substrate):
//!   1. reprise-landmark  — incumbent: the real `matchtree::Landmark` (rare-peak
//!      constellation triples), invoked through the production trait.
//!   2. minhash-lsh       — bag-Jaccard over `RepData::bag_set`, MinHash sig + LSH banding.
//!   3. winnowing         — MOSS-style k-gram winnowing over the floor-3 ordered subtree
//!      stream derived from `RepData::offsets`.
//!   4. sourcerer-rare    — inverted rare-feature overlap (landmark's rare peaks WITHOUT the
//!      triples): the ablation isolating whether the constellation earns its keep.
//!
//! Oracles (retriever-independent):
//!   (A) verify-on-union — union all retrievers' candidates, run the trusted verify chain
//!       (size gate -> offset histogram -> anti_unify); pairs that ACCEPT = the positive
//!       set. recall = fraction of that verified-union surfaced.
//!   (B) synthetic — bench-mutations seed<->mutant pairs, known clones by construction.
//!
//! Usage: cargo run --release --example bakeoff -- [<root> ...]   (default root: src)

use rayon::prelude::*;
use reprise::au;
use reprise::config::{Config, Normalizer};
use reprise::lang::Lang;
use reprise::matchtree::{self, RepData, RetrievalStats, Retriever};
use reprise::unit::{self, Unit};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;
use xxhash_rust::xxh3::xxh3_128;

/// A list of rep-index pairs (a retriever's candidate output).
type RepPairs = Vec<(usize, usize)>;

fn fold128(h: u128) -> u64 {
    (h as u64) ^ ((h >> 64) as u64)
}

// ============================ shared substrate: the REAL RepData ============================

/// One language partition: the REAL production reps (`matchtree::build_reps`) plus the
/// language metadata the oracle's anti_unify needs. Every retriever and the oracle read
/// this same substrate, so any difference is attributable to the retrieval *scheme*.
struct LangCorpus {
    lang: Lang,
    ir: bool,
    reps: Vec<RepData>,
}

fn build_lang_corpus(units: &[Unit], lang: Lang, cfg: &Config) -> Option<LangCorpus> {
    let reps = matchtree::build_reps(units, lang, cfg);
    if reps.len() < 2 {
        return None;
    }
    let ir = cfg.normalize.normalizer == Normalizer::Ir && reprise::frontend::has_ir_frontend(lang);
    Some(LangCorpus { lang, ir, reps })
}

// ============================ bench-side alternative retrievers ============================
//
// Each implements the SAME `matchtree::Retriever` trait the production landmark retriever
// implements, reading the shared `RepData` substrate via its public accessors. They live
// bench-side (not shipped) but race through the identical trait seam.

/// Candidate pairs sharing >= `min_shared` features, skipping features owned by more than
/// `df_cap` units. Full clique per feature (matches the incumbent's owner_pair_window=0).
fn shared_count_pairs(feats: &[Vec<u128>], df_cap: usize, min_shared: usize) -> RepPairs {
    let mut owners: HashMap<u128, Vec<u32>> = HashMap::new();
    for (i, f) in feats.iter().enumerate() {
        for &h in f {
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
        .map(|((a, b), _)| (a as usize, b as usize))
        .collect()
}

/// Floor-3 rare peaks for one rep: `(offset, hash)` with df <= rare_cap, offset-sorted.
/// df is over the floor-6 bag_set (the incumbent's rare test, reproduced from the public
/// substrate).
fn rare_peaks(rep: &RepData, df: &HashMap<u128, u32>, rare_cap: u32) -> Vec<(u32, u128)> {
    let mut peaks: Vec<(u32, u128)> = rep
        .offsets()
        .iter()
        .filter(|(h, _, _)| df.get(h).copied().unwrap_or(0) <= rare_cap)
        .flat_map(|(h, offs, _)| offs.iter().map(move |o| (*o, *h)))
        .collect();
    peaks.sort_unstable();
    peaks
}

/// Corpus df over the floor-6 bag_set + the incumbent rare_cap.
fn corpus_df(reps: &[RepData]) -> (HashMap<u128, u32>, u32) {
    let mut df: HashMap<u128, u32> = HashMap::new();
    for rep in reps {
        for &h in rep.bag_set() {
            *df.entry(h).or_insert(0) += 1;
        }
    }
    (df, 3.max(reps.len() as u32 / 20))
}

// ---- sourcerer-rare (ablation: rare peaks, no triples) ----

struct SourcererRare {
    min_shared: usize,
}
impl Retriever for SourcererRare {
    fn name(&self) -> &'static str {
        "sourcerer-rare"
    }
    fn candidates(&self, reps: &[RepData], _cfg: &Config, _stats: &mut RetrievalStats) -> RepPairs {
        let (df, rare_cap) = corpus_df(reps);
        let feats: Vec<Vec<u128>> = reps
            .par_iter()
            .map(|rep| {
                let mut set: Vec<u128> = rare_peaks(rep, &df, rare_cap)
                    .iter()
                    .map(|(_, h)| *h)
                    .collect();
                set.sort_unstable();
                set.dedup();
                set
            })
            .collect();
        shared_count_pairs(&feats, 50, self.min_shared)
    }
}

// ---- winnowing (MOSS-style k-gram) ----

struct Winnowing {
    k: usize,
    w: usize,
    min_shared: usize,
}

/// Floor-3 subtree hashes in pre-order (offset order), folded to u64 — the winnowing stream.
fn stream(rep: &RepData) -> Vec<u64> {
    let mut by_off: Vec<(u32, u128)> = rep
        .offsets()
        .iter()
        .flat_map(|(h, offs, _)| offs.iter().map(move |o| (*o, *h)))
        .collect();
    by_off.sort_unstable();
    by_off.iter().map(|(_, h)| fold128(*h)).collect()
}

/// Winnowing fingerprints: k-grams hashed, min selected per window of `w` (rightmost min).
fn winnow(stream: &[u64], k: usize, w: usize) -> Vec<u128> {
    if stream.is_empty() {
        return Vec::new();
    }
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
        let mut out = vec![*grams.iter().min().unwrap()];
        out.sort_unstable();
        out.dedup();
        return out;
    }
    let mut positions: Vec<usize> = Vec::new();
    for start in 0..=grams.len() - w {
        let window = &grams[start..start + w];
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

impl Retriever for Winnowing {
    fn name(&self) -> &'static str {
        "winnowing"
    }
    fn candidates(&self, reps: &[RepData], _cfg: &Config, _stats: &mut RetrievalStats) -> RepPairs {
        let feats: Vec<Vec<u128>> = reps
            .par_iter()
            .map(|rep| winnow(&stream(rep), self.k, self.w))
            .collect();
        shared_count_pairs(&feats, 50, self.min_shared)
    }
}

// ---- minhash + LSH ----

const MINHASH_K: usize = 128;

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}
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

struct MinHashLsh {
    bands: usize,
    rows: usize,
}
impl Retriever for MinHashLsh {
    fn name(&self) -> &'static str {
        "minhash-lsh"
    }
    fn candidates(&self, reps: &[RepData], _cfg: &Config, _stats: &mut RetrievalStats) -> RepPairs {
        assert!(self.bands * self.rows <= MINHASH_K);
        let (a, b) = minhash_params();
        let sigs: Vec<Vec<u64>> = reps
            .par_iter()
            .map(|r| minhash_sig(r.bag_set(), &a, &b))
            .collect();
        let mut buckets: HashMap<u128, Vec<u32>> = HashMap::new();
        for (i, sig) in sigs.iter().enumerate() {
            for band in 0..self.bands {
                let mut buf = Vec::with_capacity(self.rows * 8 + 2);
                buf.extend_from_slice(&(band as u16).to_le_bytes());
                for r in 0..self.rows {
                    buf.extend_from_slice(&sig[band * self.rows + r].to_le_bytes());
                }
                buckets.entry(xxh3_128(&buf)).or_default().push(i as u32);
            }
        }
        let df_cap = 50usize;
        let mut pairs: HashSet<(usize, usize)> = HashSet::new();
        for own in buckets.values() {
            if own.len() < 2 || own.len() > df_cap {
                continue;
            }
            for x in 0..own.len() {
                for y in x + 1..own.len() {
                    let (i, j) = (own[x] as usize, own[y] as usize);
                    pairs.insert((i.min(j), i.max(j)));
                }
            }
        }
        pairs.into_iter().collect()
    }
}

// ============================ retriever registry (all real-trait) ============================

fn retrievers() -> Vec<(&'static str, Box<dyn Retriever>)> {
    vec![
        ("reprise-landmark", Box::new(matchtree::Landmark)),
        (
            "minhash-lsh",
            Box::new(MinHashLsh { bands: 32, rows: 4 }), // thr ~ (1/32)^(1/4) ~ 0.42
        ),
        (
            "winnowing",
            Box::new(Winnowing {
                k: 5,
                w: 4,
                min_shared: 2,
            }),
        ),
        ("sourcerer-rare", Box::new(SourcererRare { min_shared: 2 })),
    ]
}

// ============================ trusted verify chain (the retriever-independent oracle) ============================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verdict {
    SizeRej,
    HistRej,
    DivRej,
    Weak,
    Accept,
}

fn size_gate_passes(ta: u32, tb: u32, cfg: &Config) -> bool {
    let (lo, hi) = (u64::from(ta.min(tb)), u64::from(ta.max(tb)));
    let ratio = 1.0 + 2.0 * cfg.thresholds.max_divergence + 0.15;
    hi as f64 <= lo as f64 * ratio + 64.0
}

/// Offset-delta diagonal over the shared floor-3 subtrees (the trusted §5.6 verify, the
/// SAME logic the pipeline's `offset-histogram` filter runs — reproduced here over the
/// public `RepData::offsets` so the oracle is retriever-independent).
fn offset_histogram_passes(a: &RepData, b: &RepData, cfg: &Config) -> bool {
    let (oa, ob) = (a.offsets(), b.offsets());
    let min_inventory = oa.len().min(ob.len()) as u32;
    let needed = cfg.histogram_min_votes().max(min_inventory / 8);
    let mut bins: HashMap<i64, u32> = HashMap::new();
    let mut best = 0u32;
    let mut hash_bins: Vec<i64> = Vec::with_capacity(16);
    let (mut i, mut j) = (0usize, 0usize);
    while i < oa.len() && j < ob.len() {
        if best >= needed {
            return true;
        }
        match oa[i].0.cmp(&ob[j].0) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                hash_bins.clear();
                for &va in oa[i].1.iter().take(4) {
                    for &vb in ob[j].1.iter().take(4) {
                        let bin = (i64::from(va) - i64::from(vb)) / 4;
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

fn verify(
    ua: &Unit,
    ub: &Unit,
    a: &RepData,
    b: &RepData,
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

// ============================ runner ============================

type PairSet = HashSet<(usize, usize)>;

#[derive(Default, Clone)]
struct Cost {
    query_ms: f64,
    index_entries: usize,
}

struct CorpusRun {
    langs: Vec<LangCorpus>,
    retr_pairs: HashMap<String, PairSet>, // retriever name -> unit-pair set (union across langs)
    retr_cost: HashMap<String, Cost>,
}

/// Run one retriever over one lang's reps, returning unit-index pairs (min,max) + timing.
fn run_retriever(r: &dyn Retriever, lc: &LangCorpus, cfg: &Config) -> (PairSet, Cost) {
    let mut stats = RetrievalStats::default();
    let t = Instant::now();
    let raw = r.candidates(&lc.reps, cfg, &mut stats);
    let query_ms = t.elapsed().as_secs_f64() * 1000.0;
    let pairs: PairSet = raw
        .into_iter()
        .map(|(i, j)| {
            let (a, b) = (lc.reps[i].unit_idx(), lc.reps[j].unit_idx());
            (a.min(b), a.max(b))
        })
        .collect();
    (
        pairs,
        Cost {
            query_ms,
            index_entries: stats.landmark_index_size,
        },
    )
}

fn run_corpus(units: &[Unit], cfg: &Config) -> CorpusRun {
    let mut langs_present: Vec<Lang> = units.iter().map(|u| u.lang).collect();
    langs_present.sort();
    langs_present.dedup();
    let langs: Vec<LangCorpus> = langs_present
        .iter()
        .filter_map(|&l| build_lang_corpus(units, l, cfg))
        .collect();

    let mut retr_pairs: HashMap<String, PairSet> = HashMap::new();
    let mut retr_cost: HashMap<String, Cost> = HashMap::new();
    for (name, r) in retrievers() {
        let mut all = PairSet::new();
        let mut cost = Cost::default();
        for lc in &langs {
            let (pairs, c) = run_retriever(r.as_ref(), lc, cfg);
            all.extend(pairs);
            cost.query_ms += c.query_ms;
            cost.index_entries += c.index_entries;
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
/// WEAK positive sets as unit-pair sets.
fn verify_union(units: &[Unit], run: &CorpusRun, cfg: &Config) -> (PairSet, PairSet) {
    let mut rep_of_unit: HashMap<usize, (usize, usize)> = HashMap::new();
    for (li, lc) in run.langs.iter().enumerate() {
        for (ri, rep) in lc.reps.iter().enumerate() {
            rep_of_unit.insert(rep.unit_idx(), (li, ri));
        }
    }
    let mut union: PairSet = HashSet::new();
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
            verify(
                &units[ua],
                &units[ub],
                &lc.reps[ra],
                &lc.reps[rb],
                lc.lang.profile(),
                lc.ir,
                cfg,
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

// ============================ oracle B: synthetic mutation corpus (verbatim) ============================

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
    cfg.inline.enabled = false; // plain units only

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
    let (accept, weak) = verify_union(&units, &run, &cfg);
    let names: Vec<String> = retrievers().iter().map(|(n, _)| n.to_string()).collect();

    println!(
        "=== ORACLE A: verify-on-union (real Retriever trait)  (corpus: {} plain units, {:?}) ===",
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
    println!("\n  retriever         flood     recallA   precision   idx-entries   query-ms");
    let mut rowsa: Vec<(String, usize, f64, f64)> = Vec::new();
    for name in &names {
        let set = &run.retr_pairs[name];
        let cost = &run.retr_cost[name];
        let flood = set.len();
        let surfaced = set.iter().filter(|p| accept.contains(*p)).count();
        let recall = surfaced as f64 / accept.len().max(1) as f64;
        let prec = surfaced as f64 / flood.max(1) as f64;
        rowsa.push((name.clone(), flood, recall, prec));
        println!(
            "  {name:<16} {flood:>7}    {recall:>6.3}    {prec:>7.4}   {:>11}   {:>7.1}",
            cost.index_entries, cost.query_ms
        );
    }

    // ---- oracle B ----
    let (syn_units, syn_pairs) = build_synthetic(&cfg);
    let syn_run = run_corpus(&syn_units, &cfg);
    #[derive(Default)]
    struct BClass {
        near: Vec<(usize, usize)>,
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
            bc.near.push(key);
        }
    }
    println!(
        "\n=== ORACLE B: synthetic mutation clones  ({} seed corpus units) ===",
        syn_units.len()
    );
    println!(
        "  known seed<->mutant pairs: near(distinct-fp)={}  exact(same-fp)={}  \
         inline-tier(t4)={}  below-floor/excluded={}  control(ctl)={}",
        bc.near.len(),
        bc.exact,
        bc.inline,
        bc.excluded,
        bc.ctl.len()
    );
    println!("\n  retriever         recallB   surfaced/near   ctl-surfaced(FP)");
    for name in &names {
        let set = &syn_run.retr_pairs[name];
        let surfaced = bc.near.iter().filter(|k| set.contains(k)).count();
        let ctl_fp = bc.ctl.iter().filter(|k| set.contains(k)).count();
        let recall = surfaced as f64 / bc.near.len().max(1) as f64;
        println!(
            "  {name:<16} {recall:>6.3}      {surfaced:>3}/{:<3}          {ctl_fp}",
            bc.near.len()
        );
    }

    // ---- verdict ----
    println!("\n=== VERDICT ===");
    let lm = &run.retr_pairs["reprise-landmark"];
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
            let catchers: Vec<&str> = names
                .iter()
                .filter(|n| {
                    n.as_str() != "reprise-landmark" && run.retr_pairs[*n].contains(&(a, b))
                })
                .map(|n| n.as_str())
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
    let only_lm = accept
        .iter()
        .filter(|p| {
            lm.contains(*p)
                && names
                    .iter()
                    .filter(|n| n.as_str() != "reprise-landmark")
                    .all(|n| !run.retr_pairs[n].contains(*p))
        })
        .count();
    println!(
        "\n(2) verified ACCEPT pairs caught ONLY by landmark (no canonical scheme): {only_lm}"
    );
    let lm_flood = lm.len();
    let lm_recall = rowsa[0].2;
    println!(
        "\n(3) does any canonical scheme match landmark recall ({lm_recall:.3}) at lower flood ({lm_flood})?"
    );
    for (name, flood, recall, _prec) in rowsa.iter().skip(1) {
        let verdict = if *recall >= lm_recall - 1e-9 && *flood < lm_flood {
            "YES — matches/beats recall at LOWER flood"
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
