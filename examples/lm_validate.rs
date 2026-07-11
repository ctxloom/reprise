//! Landmark-thread-A validation (throwaway harness; see docs/substantiality-metric.md §0.3).
//!
//! Faithfully reconstructs the `src/matchtree.rs` landmark layer + verification over a REAL
//! corpus, and measures two pair-level signals as false-positive filters for the near-clone
//! tier:
//!   - coverage-fraction  = |shared landmarks| / min(|A landmarks|, |B landmarks|)
//!   - shared-landmark-df = mean corpus document-frequency of the shared landmark hashes
//!
//! Labels come from the REAL pipeline decisions: a landmark candidate pair is POSITIVE iff it
//! survives the exact `size_gate -> offset_histogram -> anti_unify(divergence/holes/factorable)`
//! chain (an accepted near-clone), NEGATIVE iff any stage rejects it. Class-3 (genuine shared
//! fragment) is labeled independently from the sequence tier's exact-region recurrence, NOT from
//! landmark-df, so the 3-class test is non-circular.
//!
//! Usage: cargo run --release --example lm_validate -- <root> [<root> ...]

use rayon::prelude::*;
use reprise::au;
use reprise::config::{Config, Normalizer};
use reprise::fingerprint::{self, HashMode, Subtree};
use reprise::intern::LabelInterner;
use reprise::lang::Lang;
use reprise::seq;
use reprise::stream;
use reprise::unit::{self, Unit};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Landmark identity == its Shazam triple (anchor hash, target hash, bucketed Δoffset).
/// Equality/df on the triple is identical to matchtree's xxh3_128 of the same bytes,
/// modulo negligible collisions — and we only need equality here, not the hash value.
type Lm = (u128, u128, u32);

struct Rep {
    unit_idx: usize,
    bag_set: Vec<u128>,
    offsets: Vec<(u128, Vec<u32>)>,
    landmarks: Vec<Lm>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verdict {
    SizeRej,
    HistRej,
    DivRej,
    Weak,
    Accept,
}

struct Record {
    coverage: f64,
    shared_lm_df: f64,
    verdict: Verdict,
    // Class-3 labeling (independent of landmark-df): does this unit pair share an exact
    // token run >= min_seq_tokens, and in how many corpus units does that fragment recur?
    has_region: bool,
    region_frag_df: u32, // distinct units containing the pair's largest shared fragment
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let roots: Vec<PathBuf> = if args.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        args.iter().map(PathBuf::from).collect()
    };
    let mut cfg = Config::default();
    cfg.cache.enabled = false; // no .reprise/ litter

    // ---- collect units across all roots, exactly as the pipeline extracts plain units ----
    // ONE shared interner for the whole run: every unit lands in this single `units` vec and
    // gets cross-compared within its lang partition (subtree_inventory/unit_stream/anti_unify),
    // so their `Label::External`/`LitKept` LSyms must all resolve against the same interner.
    let label_interner = reprise::intern::LabelInterner::new();
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
            let (fus, _rep) =
                unit::extract_file_units_with_interner(&path, &src, lang, &cfg, &label_interner);
            units.extend(fus);
        }
    }
    eprintln!(
        "corpus: {} plain units across {} root(s) {:?}",
        units.len(),
        roots.len(),
        roots
    );

    // Per-language partitions (the pipeline's near tier + seq tier both partition by lang).
    let mut langs: Vec<Lang> = units.iter().map(|u| u.lang).collect();
    langs.sort();
    langs.dedup();

    let mut records: Vec<Record> = Vec::new();
    let mut lang_lines: Vec<String> = Vec::new();

    for lang in langs {
        let ir =
            cfg.normalize.normalizer == Normalizer::Ir && reprise::frontend::has_ir_frontend(lang);
        let floor = cfg.min_unit_floor();

        // eligible set: this lang, token >= floor, deduped by exact fingerprint (matchtree.rs).
        let mut seen_fp = HashSet::new();
        let eligible: Vec<usize> = units
            .iter()
            .enumerate()
            .filter(|(_, u)| u.lang == lang && u.token_count >= floor)
            .filter(|(_, u)| seen_fp.insert(u.fingerprint))
            .map(|(idx, _)| idx)
            .collect();
        if eligible.len() < 2 {
            continue;
        }

        // ---- reps: offsets (floor 3), bag_set (floor bag_min_subtree_tokens) ----
        let mut reps: Vec<Rep> = eligible
            .par_iter()
            .map(|&idx| {
                let inv: Vec<Subtree> = fingerprint::subtree_inventory(
                    units[idx].tree.expect_resident(),
                    3,
                    HashMode::MaskedLocals,
                    &label_interner,
                );
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
                Rep {
                    unit_idx: idx,
                    bag_set,
                    offsets,
                    landmarks: Vec::new(),
                }
            })
            .collect();

        // ---- landmark pairs (matchtree.rs:146-177) ----
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
                    rep.landmarks.push((rare[i].1, rare[j].1, delta));
                }
            }
            rep.landmarks.sort_unstable();
            rep.landmarks.dedup();
        });

        // ---- landmark df (no cap): the "shared-landmark-df" signal source ----
        let mut lm_df: HashMap<Lm, u32> = HashMap::new();
        for rep in &reps {
            for lm in &rep.landmarks {
                *lm_df.entry(*lm).or_insert(0) += 1;
            }
        }

        // ---- candidate formation: shared landmark count (df<=50) >= shared_landmarks_min ----
        let df_cap = 50usize;
        let mut owners: HashMap<Lm, Vec<u32>> = HashMap::new();
        for (i, rep) in reps.iter().enumerate() {
            for lm in &rep.landmarks {
                owners.entry(*lm).or_default().push(i as u32);
            }
        }
        let mut shared_capped: HashMap<(u32, u32), u32> = HashMap::new();
        for own in owners.values() {
            if own.len() < 2 || own.len() > df_cap {
                continue;
            }
            for a in 0..own.len() {
                for b in a + 1..own.len() {
                    *shared_capped.entry((own[a], own[b])).or_insert(0) += 1;
                }
            }
        }
        let min_shared = cfg.retrieval.shared_landmarks_min as u32;
        let candidates: Vec<(u32, u32)> = shared_capped
            .iter()
            .filter(|&(_, &c)| c >= min_shared)
            .map(|(&p, _)| p)
            .collect();

        // ---- sequence tier: exact-region recurrence for independent class-3 labeling ----
        // frag_units[frag_fp] = distinct corpus units containing that exact run.
        let part = &eligible; // plain units only here (all variant.is_none)
        // build_corpus consumes per-unit streams since the drop-trees work;
        // derive them with the same production serializer.
        let unit_streams: Vec<Vec<(u64, (u32, u32))>> = eligible
            .iter()
            .map(|&i| stream::unit_stream(units[i].tree.expect_resident(), &label_interner))
            .collect();
        let streams: Vec<&[(u64, (u32, u32))]> = unit_streams.iter().map(Vec::as_slice).collect();
        let corpus = stream::build_corpus(&streams);
        // map (min rep-index, max rep-index) -> (best_len, frag_fp)
        let rep_of_unit: HashMap<usize, u32> = reps
            .iter()
            .enumerate()
            .map(|(ri, r)| (r.unit_idx, ri as u32))
            .collect();
        let mut frag_units: HashMap<u128, HashSet<usize>> = HashMap::new();
        let mut pair_region: HashMap<(u32, u32), (usize, u128)> = HashMap::new();
        for r in seq::maximal_repeats(&corpus, cfg.thresholds.min_seq_tokens as usize) {
            let ua = part[r.unit_a as usize];
            let ub = part[r.unit_b as usize];
            let mut buf = Vec::with_capacity(8 * r.len);
            for t in r.tok_a.0..r.tok_a.1 {
                buf.extend_from_slice(&corpus.key_hash[t].to_le_bytes());
            }
            let frag_fp = xxhash_rust::xxh3::xxh3_128(&buf);
            let fu = frag_units.entry(frag_fp).or_default();
            fu.insert(ua);
            fu.insert(ub);
            // key by rep index (candidates are rep-index pairs)
            if let (Some(&ri), Some(&rj)) = (rep_of_unit.get(&ua), rep_of_unit.get(&ub)) {
                let key = (ri.min(rj), ri.max(rj));
                let e = pair_region.entry(key).or_insert((0, 0));
                if r.len > e.0 {
                    *e = (r.len, frag_fp);
                }
            }
        }

        // ---- per-candidate: coverage, shared-landmark-df, verdict, region label ----
        let cfg_ref = &cfg;
        let reps_ref = &reps;
        let units_ref = &units;
        let lm_df_ref = &lm_df;
        let profile = lang.profile();
        let recs: Vec<(Record, Verdict)> = candidates
            .par_iter()
            .map(|&(i, j)| {
                let (ri, rj) = (i as usize, j as usize);
                let a = &reps_ref[ri];
                let b = &reps_ref[rj];
                let shared = intersect_sorted(&a.landmarks, &b.landmarks);
                let denom = a.landmarks.len().min(b.landmarks.len()).max(1) as f64;
                let coverage = shared.len() as f64 / denom;
                let df_sum: u64 = shared.iter().map(|lm| lm_df_ref[lm] as u64).sum();
                let shared_lm_df = if shared.is_empty() {
                    0.0
                } else {
                    df_sum as f64 / shared.len() as f64
                };
                let (ua, ub) = (a.unit_idx, b.unit_idx);
                let verdict = verify(
                    &units_ref[ua],
                    &units_ref[ub],
                    a,
                    b,
                    profile,
                    ir,
                    cfg_ref,
                    &label_interner,
                );
                let region = pair_region.get(&(i.min(j), i.max(j)));
                let (has_region, region_frag_df) = match region {
                    Some(&(_len, fp)) => (true, frag_units[&fp].len() as u32),
                    None => (false, 0),
                };
                (
                    Record {
                        coverage,
                        shared_lm_df,
                        verdict,
                        has_region,
                        region_frag_df,
                    },
                    verdict,
                )
            })
            .collect();

        let mut c_size = 0;
        let mut c_hist = 0;
        let mut c_div = 0;
        let mut c_weak = 0;
        let mut c_acc = 0;
        for (rec, v) in recs {
            match v {
                Verdict::SizeRej => c_size += 1,
                Verdict::HistRej => c_hist += 1,
                Verdict::DivRej => c_div += 1,
                Verdict::Weak => c_weak += 1,
                Verdict::Accept => c_acc += 1,
            }
            records.push(rec);
        }
        lang_lines.push(format!(
            "  {lang:?}: eligible={} landmark-candidates={} | size-rej={} hist-rej={} div-rej={} weak={} ACCEPT={}",
            eligible.len(),
            candidates.len(),
            c_size,
            c_hist,
            c_div,
            c_weak,
            c_acc
        ));
    }

    // ---------------- report ----------------
    println!("\n=== landmark-thread-A live validation ===");
    for l in &lang_lines {
        println!("{l}");
    }
    let total = records.len();
    let acc = records
        .iter()
        .filter(|r| r.verdict == Verdict::Accept)
        .count();
    let weak = records
        .iter()
        .filter(|r| r.verdict == Verdict::Weak)
        .count();
    let rej = total - acc - weak;
    println!(
        "\npopulation (all landmark candidates): {total}  | ACCEPT(near-clone)={acc}  WEAK={weak}  REJECT={rej}"
    );

    // --- AUC 1: coverage-fraction & shared-landmark-df, positive=Accept, negative=all-rejected ---
    let pos: Vec<&Record> = records
        .iter()
        .filter(|r| r.verdict == Verdict::Accept)
        .collect();
    let neg: Vec<&Record> = records
        .iter()
        .filter(|r| {
            matches!(
                r.verdict,
                Verdict::SizeRej | Verdict::HistRej | Verdict::DivRej
            )
        })
        .collect();
    println!(
        "\n[A] Accept vs Rejected  (pos={} neg={})",
        pos.len(),
        neg.len()
    );
    auc_report("coverage-fraction", &pos, &neg, |r| r.coverage);
    auc_report("shared-landmark-df", &pos, &neg, |r| -r.shared_lm_df); // higher df => more FP-like

    // --- AUC 2: reaching-verification only (drop size-gate pre-rejects) ---
    let neg_v: Vec<&Record> = records
        .iter()
        .filter(|r| matches!(r.verdict, Verdict::HistRej | Verdict::DivRej))
        .collect();
    println!(
        "\n[B] Accept vs verification-rejected only (pos={} neg={})",
        pos.len(),
        neg_v.len()
    );
    auc_report("coverage-fraction", &pos, &neg_v, |r| r.coverage);
    auc_report("shared-landmark-df", &pos, &neg_v, |r| -r.shared_lm_df);

    // --- operating point: coverage-fraction candidate gate (recall-neutral flood cut) ---
    println!("\n[C] coverage-fraction gate sweep (retain candidates with coverage >= t):");
    println!("     t     recall(ACCEPT kept)   flood-cut(REJECT dropped)   kept-precision");
    for &t in &[0.02, 0.05, 0.10, 0.15, 0.20, 0.30, 0.40, 0.50] {
        let pos_kept = pos.iter().filter(|r| r.coverage >= t).count();
        let neg_kept = neg.iter().filter(|r| r.coverage >= t).count();
        let recall = pos_kept as f64 / pos.len().max(1) as f64;
        let flood_cut = 1.0 - neg_kept as f64 / neg.len().max(1) as f64;
        let prec = pos_kept as f64 / (pos_kept + neg_kept).max(1) as f64;
        println!(
            "    {t:>4.2}      {recall:>6.3}                 {flood_cut:>6.3}                {prec:>6.4}"
        );
    }

    // --- operating point: shared-landmark-df filter (drop candidates with df >= t) ---
    println!("\n[C2] shared-landmark-df gate sweep (drop candidates with mean shared-df >= t):");
    println!("     t     recall(ACCEPT kept)   flood-cut(REJECT dropped)   kept-precision");
    for &t in &[8.0, 10.0, 12.0, 15.0, 20.0, 25.0, 30.0] {
        let pos_kept = pos.iter().filter(|r| r.shared_lm_df < t).count();
        let neg_kept = neg.iter().filter(|r| r.shared_lm_df < t).count();
        let recall = pos_kept as f64 / pos.len().max(1) as f64;
        let flood_cut = 1.0 - neg_kept as f64 / neg.len().max(1) as f64;
        let prec = pos_kept as f64 / (pos_kept + neg_kept).max(1) as f64;
        println!(
            "    {t:>5.1}     {recall:>6.3}                 {flood_cut:>6.3}                {prec:>6.4}"
        );
    }

    // --- 3-class separation (independent labels from exact-fragment recurrence) ---
    // class1 whole-clone   = Accept
    // class3 genuine-frag  = NOT accept, has exact region, fragment recurs in exactly 2 units
    // class2 boilerplate   = NOT accept, EITHER no distinctive region OR fragment recurs widely (>=6 units)
    let class1: Vec<&Record> = records
        .iter()
        .filter(|r| r.verdict == Verdict::Accept)
        .collect();
    let class3: Vec<&Record> = records
        .iter()
        .filter(|r| r.verdict != Verdict::Accept && r.has_region && r.region_frag_df == 2)
        .collect();
    let class2: Vec<&Record> = records
        .iter()
        .filter(|r| r.verdict != Verdict::Accept && (!r.has_region || r.region_frag_df >= 6))
        .collect();
    // Cleanest boiler-vs-genuine: BOTH have a real >=30-tok exact region, differ only in
    // corpus recurrence of that region (region_frag_df). No circularity with landmark-df.
    let class2_reg: Vec<&Record> = records
        .iter()
        .filter(|r| r.verdict != Verdict::Accept && r.has_region && r.region_frag_df >= 6)
        .collect();
    // Diagnostic: among REJECTED candidates that DO carry a >=30-tok exact region, how does
    // shared-landmark-df behave as the region's corpus recurrence (frag_df) grows? This is the
    // ambiguous middle where a df discount could wrongly fire on a genuine small clone family.
    println!(
        "\n[D0] rejected candidates WITH a >=30-tok exact region, bucketed by region_frag_df:"
    );
    let rej_reg: Vec<&Record> = records
        .iter()
        .filter(|r| r.verdict != Verdict::Accept && r.has_region)
        .collect();
    let no_reg = records
        .iter()
        .filter(|r| r.verdict != Verdict::Accept && !r.has_region)
        .count();
    println!("   rejected w/ NO region (pure scattered-landmark boilerplate): n={no_reg}");
    for (lo, hi, label) in [
        (2u32, 2u32, "df=2 (genuine, pair-unique)"),
        (3, 5, "df=3-5"),
        (6, 10, "df=6-10"),
        (11, u32::MAX, "df>=11"),
    ] {
        let b: Vec<&&Record> = rej_reg
            .iter()
            .filter(|r| r.region_frag_df >= lo && r.region_frag_df <= hi)
            .collect();
        if b.is_empty() {
            println!("   region {label:<28}: n=0");
            continue;
        }
        let cov: Vec<f64> = b.iter().map(|r| r.coverage).collect();
        let df: Vec<f64> = b.iter().map(|r| r.shared_lm_df).collect();
        println!(
            "   region {label:<28}: n={:>4}  coverage med={:.3}  shared-lm-df med={:.2} mean={:.2}",
            b.len(),
            median(&cov),
            median(&df),
            mean(&df)
        );
    }

    println!("\n[D] 3-class separation (independent exact-fragment recurrence labels):");
    class_stats("class1 whole-clone   (Accept)          ", &class1);
    class_stats("class2 boilerplate-FP (df>=6 or no reg)", &class2);
    class_stats("class2r boiler w/ region only (df>=6)  ", &class2_reg);
    class_stats("class3 genuine-frag  (exact reg, dfrag=2)", &class3);
    if !class2.is_empty() && !class3.is_empty() {
        println!("\n   separation shared-landmark-df  class2(boiler) vs class3(genuine):");
        auc_report("  df sep (AUC boiler>genuine)", &class2, &class3, |r| {
            r.shared_lm_df
        });
        println!("   separation coverage-fraction  class2(boiler) vs class3(genuine):");
        auc_report("  cov sep (AUC genuine>boiler)", &class3, &class2, |r| {
            r.coverage
        });
    }
    if !class2_reg.is_empty() && !class3.is_empty() {
        println!(
            "\n   CRISP (both have >=30-tok region): boiler-region(df>=6) vs genuine-region(df=2):"
        );
        auc_report("  df sep (AUC boiler>genuine)", &class2_reg, &class3, |r| {
            r.shared_lm_df
        });
    }
}

/// Faithful copy of matchtree.rs::size_gate_passes + offset_histogram_passes + acceptance.
#[allow(clippy::too_many_arguments)]
fn verify(
    ua: &Unit,
    ub: &Unit,
    a: &Rep,
    b: &Rep,
    profile: &'static dyn reprise::lang::LanguageProfile,
    ir: bool,
    cfg: &Config,
    li: &LabelInterner,
) -> Verdict {
    if !size_gate_passes(ua.token_count, ub.token_count, cfg) {
        return Verdict::SizeRej;
    }
    if !offset_histogram_passes(a, b, cfg) {
        return Verdict::HistRej;
    }
    let outcome = au::anti_unify(
        ua.tree.expect_resident(),
        ub.tree.expect_resident(),
        (!ir).then_some(profile),
        ir,
        li,
    );
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

fn intersect_sorted(a: &[Lm], b: &[Lm]) -> Vec<Lm> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                out.push(a[i]);
                i += 1;
                j += 1;
            }
        }
    }
    out
}

/// Mann-Whitney AUC = P(score_pos > score_neg), ties count as 0.5.
fn auc(pos: &[f64], neg: &[f64]) -> f64 {
    if pos.is_empty() || neg.is_empty() {
        return f64::NAN;
    }
    // rank-based with tie handling
    let mut all: Vec<(f64, bool)> = Vec::with_capacity(pos.len() + neg.len());
    all.extend(pos.iter().map(|&x| (x, true)));
    all.extend(neg.iter().map(|&x| (x, false)));
    all.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    // assign average ranks
    let n = all.len();
    let mut ranks = vec![0.0f64; n];
    let mut k = 0;
    while k < n {
        let mut m = k + 1;
        while m < n && all[m].0 == all[k].0 {
            m += 1;
        }
        let avg = ((k + 1 + m) as f64) / 2.0; // avg of ranks k+1..=m
        for r in ranks.iter_mut().take(m).skip(k) {
            *r = avg;
        }
        k = m;
    }
    let sum_pos_ranks: f64 = ranks
        .iter()
        .zip(all.iter())
        .filter(|(_, (_, is_pos))| *is_pos)
        .map(|(r, _)| *r)
        .sum();
    let np = pos.len() as f64;
    let nn = neg.len() as f64;
    (sum_pos_ranks - np * (np + 1.0) / 2.0) / (np * nn)
}

fn auc_report<F: Fn(&Record) -> f64>(name: &str, pos: &[&Record], neg: &[&Record], f: F) {
    let p: Vec<f64> = pos.iter().map(|r| f(r)).collect();
    let n: Vec<f64> = neg.iter().map(|r| f(r)).collect();
    let a = auc(&p, &n);
    println!("    {name:<30} AUC={a:.4}");
}

fn class_stats(name: &str, recs: &[&Record]) {
    if recs.is_empty() {
        println!("   {name}: n=0");
        return;
    }
    let cov: Vec<f64> = recs.iter().map(|r| r.coverage).collect();
    let df: Vec<f64> = recs.iter().map(|r| r.shared_lm_df).collect();
    println!(
        "   {name}: n={:>5}  coverage median={:.3} mean={:.3}   shared-lm-df median={:.2} mean={:.2}",
        recs.len(),
        median(&cov),
        mean(&cov),
        median(&df),
        mean(&df),
    );
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}
fn median(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = s.len();
    if n.is_multiple_of(2) {
        (s[n / 2 - 1] + s[n / 2]) / 2.0
    } else {
        s[n / 2]
    }
}
