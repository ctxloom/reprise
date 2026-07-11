//! Dev utility: explain why a pair of units does or doesn't converge.
//! Usage: cargo run --example debug_near -- a.py b.py

use reprise::config::{Config, Normalizer};
use reprise::fingerprint::{self, HashMode};
use reprise::lang::Lang;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cfg = Config::default();
    let read = |p: &str| std::fs::read_to_string(p).unwrap();
    let lang = if args[1].ends_with(".rs") {
        Lang::Rust
    } else {
        Lang::Python
    };
    // Interning-id-conversion WP: `ua`/`ub` must share ONE `LabelInterner` — they get
    // anti-unified against each other below, which compares `Label`s directly (an
    // `LSym` id is meaningless across two independently-scoped interners).
    let label_interner = reprise::intern::LabelInterner::new();
    let ua = &reprise::unit::units_from_source_with_interner(
        &read(&args[1]),
        lang,
        &cfg,
        &label_interner,
    )[0];
    let ub = &reprise::unit::units_from_source_with_interner(
        &read(&args[2]),
        lang,
        &cfg,
        &label_interner,
    )[0];
    println!(
        "A: {} tokens={} fp={:x}",
        ua.name, ua.token_count, ua.fingerprint
    );
    println!(
        "B: {} tokens={} fp={:x}",
        ub.name, ub.token_count, ub.fingerprint
    );
    if ua.fingerprint == ub.fingerprint {
        println!("EXACT MATCH");
        return;
    }
    let inv = |u: &reprise::Unit| {
        fingerprint::subtree_inventory(
            u.tree.expect_resident(),
            cfg.thresholds.bag_min_subtree_tokens,
            HashMode::MaskedLocals,
            &label_interner,
        )
    };
    let (ia, ib) = (inv(ua), inv(ub));
    let mut sa: Vec<u128> = ia.iter().map(|s| s.hash).collect();
    let mut sb: Vec<u128> = ib.iter().map(|s| s.hash).collect();
    sa.sort_unstable();
    sa.dedup();
    sb.sort_unstable();
    sb.dedup();
    let shared = sa.iter().filter(|h| sb.binary_search(h).is_ok()).count();
    let union = sa.len() + sb.len() - shared;
    println!(
        "bag: |A|={} |B|={} shared={} jaccard={:.3} (threshold {})",
        sa.len(),
        sb.len(),
        shared,
        shared as f64 / union as f64,
        cfg.thresholds.candidate_sim
    );
    let ir = cfg.normalize.normalizer == Normalizer::Ir && reprise::frontend::has_ir_frontend(lang);
    let profile = (!ir).then(|| lang.profile());
    let outcome = reprise::au::anti_unify(
        ua.tree.expect_resident(),
        ub.tree.expect_resident(),
        profile,
        ir,
        &label_interner,
    );
    println!(
        "AU: divergence={:.3} (max {}), holes={} (max {}), factorable={}",
        outcome.divergence,
        cfg.thresholds.max_divergence,
        outcome.holes.len(),
        cfg.thresholds.max_holes,
        outcome.factorable
    );
    for (i, h) in outcome.holes.iter().enumerate() {
        println!(
            "  hole {}: a={} b={} factorable={}",
            i + 1,
            h.tokens_a,
            h.tokens_b,
            h.factorable
        );
    }
    println!(
        "template:\n{}",
        reprise::au::render_template(&outcome.template, &label_interner)
    );
}
