//! Inliner expansion-budget WP (session `stark-mixed-front`,
//! `inliner-expansion-budget.plan.md` §6.5): the SCC round must stop after
//! `max_scc_depth` partner splices (the module doc's documented "inline SCC
//! partner ONCE" intent, `src/inline.rs:9-13`), and the aggregate
//! `max_expansion_nodes` budget is a pure backstop under policy `skip` — it
//! only ever selects a unit's variant presence/absence, never its content.
//!
//! Low-level tests call `corpus_units` + `inline::DefTable::build` +
//! `inline::expand_unit` directly (bypassing the grouping/matching layer) so
//! the assertions read the walk's own bookkeeping (`Expansion::chain`,
//! `calls_inlined`, `tree`) — the exact record of which callees were spliced —
//! rather than a derived proxy.

use reprise::config::Config;
use reprise::inline::{DefTable, expand_unit};
use reprise::{CorpusUnits, corpus_units};
use std::fs;
use tempfile::TempDir;

/// Build a corpus from `(filename, source)` pairs under a fresh tempdir.
fn build_corpus(files: &[(&str, &str)], cfg: &Config) -> (TempDir, CorpusUnits) {
    let dir = TempDir::new().unwrap();
    for (name, src) in files {
        fs::write(dir.path().join(name), src).unwrap();
    }
    let corpus = corpus_units(dir.path(), cfg).unwrap();
    (dir, corpus)
}

fn unit_idx(corpus: &CorpusUnits, name: &str) -> usize {
    corpus
        .units
        .iter()
        .position(|u| u.name == name)
        .unwrap_or_else(|| {
            panic!(
                "no unit named {name:?}: {:?}",
                corpus.units.iter().map(|u| &u.name).collect::<Vec<_>>()
            )
        })
}

/// A padded body: `acc` gets 40 trivial increments (each several IR nodes),
/// comfortably over `max_callee_tokens` (120) on its own — so an SCC partner
/// with this body can splice ONLY via the SCC bypass, never via the ordinary
/// caps. Ends in a Rust block-tail call to `next`, a statement-level splice
/// site (spec §5.4).
fn padded_fn(name: &str, next: &str) -> String {
    let mut body = format!("fn {name}(acc: i64) -> i64 {{\n    let mut acc = acc;\n");
    for i in 0..40 {
        body.push_str(&format!("    acc += {i};\n"));
    }
    body.push_str(&format!("    {next}(acc)\n}}\n"));
    body
}

/// Four-member mutual-recursion SCC: p0 -> p1 -> p2 -> p3 -> p0 (the cycle
/// closes on p0, which `resolve_policy`'s self-recursion guard always skips —
/// so the reachable partner chain from p0 is at most [p1, p2, p3]).
fn scc4_source() -> String {
    let mut src = String::from("fn p0(x: i64) -> i64 {\n    let a = x + 1;\n    p1(a)\n}\n\n");
    src.push_str(&padded_fn("p1", "p2"));
    src.push('\n');
    src.push_str(&padded_fn("p2", "p3"));
    src.push('\n');
    src.push_str(&padded_fn("p3", "p0"));
    src
}

/// Three-member mutual-recursion SCC: q0 -> q1 -> q2 -> q0.
fn scc3_source() -> String {
    let mut src = String::from("fn q0(x: i64) -> i64 {\n    let a = x + 1;\n    q1(a)\n}\n\n");
    src.push_str(&padded_fn("q1", "q2"));
    src.push('\n');
    src.push_str(&padded_fn("q2", "q0"));
    src
}

// ---------- P1: SCC round bounded to `max_scc_depth` ----------

#[test]
fn scc_round_does_not_nest_past_max_scc_depth() {
    // Design doc measured on redis: pre-fix, a chain of DISTINCT SCC members
    // nests as deep as the SCC has members (31, against max_depth=2), because
    // `resolve_policy` skips BOTH `max_depth` and `max_callee_tokens` for every
    // SCC partner unconditionally. Every partner body here is >120 tokens
    // (over `max_callee_tokens`), so nothing but the (buggy, unbounded) SCC
    // bypass could ever let p2/p3 splice.
    let cfg = Config::default();
    let src = scc4_source();
    let (_dir, corpus) = build_corpus(&[("scc4.rs", &src)], &cfg);
    let table = DefTable::build(
        &corpus.units,
        &corpus.call_sites,
        &cfg,
        &corpus.label_interner,
    );
    let idx = unit_idx(&corpus, "p0");
    let exp = expand_unit(
        idx,
        &corpus.raw_trees[idx],
        &corpus.raw_trees,
        &corpus.call_sites,
        &corpus.units,
        &table,
        &cfg,
        &corpus.label_interner,
    );

    assert!(exp.scc, "p0 must be recognized as an SCC member");
    // `max_scc_depth` default is 1: exactly one partner splice, not a chain
    // through the whole SCC. This is the direct, non-proxy observation — the
    // deduped list of callees the walk actually spliced (`splice_body` pushes
    // a name onto `chain` on ITS OWN completion, i.e. post-order, so a bounded
    // round's chain is a single-element list regardless of push order).
    assert_eq!(
        exp.chain,
        vec!["p1".to_string()],
        "SCC round must stop after ONE partner splice (max_scc_depth=1), not nest \
         through the whole SCC: {:?}",
        exp.chain
    );
    assert_eq!(
        exp.calls_inlined, 1,
        "exactly one splice (p1); p2/p3 must fall back to the ordinary max_callee_tokens \
         cap and be refused, since their bodies exceed it"
    );
}

// ---------- The documented "inline SCC partner once" behavior ----------

#[test]
fn inlined_scc_partner_does_not_recursively_splice_its_own_partners() {
    // module doc (src/inline.rs:9-13): "inline SCC partner ONCE -> direct
    // self-recursion -> Rev 5 lowering -> loop core". q1 (the spliced
    // partner) must appear in the chain; q1's own SCC partner q2 must NOT.
    let cfg = Config::default();
    let src = scc3_source();
    let (_dir, corpus) = build_corpus(&[("scc3.rs", &src)], &cfg);
    let table = DefTable::build(
        &corpus.units,
        &corpus.call_sites,
        &cfg,
        &corpus.label_interner,
    );
    let idx = unit_idx(&corpus, "q0");
    let exp = expand_unit(
        idx,
        &corpus.raw_trees[idx],
        &corpus.raw_trees,
        &corpus.call_sites,
        &corpus.units,
        &table,
        &cfg,
        &corpus.label_interner,
    );

    assert!(
        exp.chain.iter().any(|c| c == "q1"),
        "q1 (the SCC partner) must be inlined: {:?}",
        exp.chain
    );
    assert!(
        !exp.chain.iter().any(|c| c == "q2"),
        "q1's own SCC partner q2 must NOT be recursively spliced in the same round: {:?}",
        exp.chain
    );
}

// ---------- P2: aggregate expansion-node budget, policy = skip ----------

/// Two ordinary (non-SCC) helper calls, each cheap enough to inline under the
/// default caps — used to exercise the aggregate budget in isolation from the
/// SCC round.
fn breadth_source() -> String {
    r#"
fn helper_a(x: i64) -> i64 {
    let v = x + 1;
    v + 2
}

fn root(x: i64) -> i64 {
    let a = helper_a(x);
    a + helper_a(a)
}
"#
    .to_string()
}

#[test]
fn over_budget_unit_gets_no_variant_never_a_truncated_one() {
    let src = breadth_source();

    // Sanity: at the default budget, root DOES inline (establishes that this
    // fixture has nonzero expansion to be budget-limited in the first place).
    let default_cfg = Config::default();
    let (_dir_a, corpus_a) = build_corpus(&[("breadth.rs", &src)], &default_cfg);
    let table_a = DefTable::build(
        &corpus_a.units,
        &corpus_a.call_sites,
        &default_cfg,
        &corpus_a.label_interner,
    );
    let idx_a = unit_idx(&corpus_a, "root");
    let exp_a = expand_unit(
        idx_a,
        &corpus_a.raw_trees[idx_a],
        &corpus_a.raw_trees,
        &corpus_a.call_sites,
        &corpus_a.units,
        &table_a,
        &default_cfg,
        &corpus_a.label_interner,
    );
    assert!(
        exp_a.tree.is_some() && exp_a.calls_inlined > 0,
        "fixture must inline something at the default budget for this test to mean anything: \
         calls_inlined={}",
        exp_a.calls_inlined
    );

    // A deliberately zero expansion-node budget: ANY splice exceeds it.
    let mut tiny_cfg = Config::default();
    tiny_cfg.inline.max_expansion_nodes = 0;
    let (_dir_b, corpus_b) = build_corpus(&[("breadth.rs", &src)], &tiny_cfg);
    let table_b = DefTable::build(
        &corpus_b.units,
        &corpus_b.call_sites,
        &tiny_cfg,
        &corpus_b.label_interner,
    );
    let idx_b = unit_idx(&corpus_b, "root");
    let exp_b = expand_unit(
        idx_b,
        &corpus_b.raw_trees[idx_b],
        &corpus_b.raw_trees,
        &corpus_b.call_sites,
        &corpus_b.units,
        &table_b,
        &tiny_cfg,
        &corpus_b.label_interner,
    );
    assert!(
        exp_b.tree.is_none(),
        "over-budget unit must get NO variant, never a truncated one"
    );
    assert_eq!(
        exp_b.calls_inlined, 0,
        "policy=skip: the unit's variant is discarded wholesale, so calls_inlined reports 0 \
         even though a splice was attempted and refused"
    );
}

#[test]
fn budget_is_deterministic_across_runs() {
    let src = breadth_source();
    let mut cfg = Config::default();
    cfg.inline.max_expansion_nodes = 0;

    let render = || {
        let (_dir, corpus) = build_corpus(&[("breadth.rs", &src)], &cfg);
        let table = DefTable::build(
            &corpus.units,
            &corpus.call_sites,
            &cfg,
            &corpus.label_interner,
        );
        let idx = unit_idx(&corpus, "root");
        let exp = expand_unit(
            idx,
            &corpus.raw_trees[idx],
            &corpus.raw_trees,
            &corpus.call_sites,
            &corpus.units,
            &table,
            &cfg,
            &corpus.label_interner,
        );
        (exp.tree.is_some(), exp.calls_inlined)
    };
    assert_eq!(
        render(),
        render(),
        "over-budget skip must be deterministic run over run"
    );
}

// ---------- The budget is not fingerprint-determining under `skip` ----------

#[test]
fn budget_does_not_alter_emitted_variants() {
    // A unit that stays UNDER budget at both a small and a huge budget must
    // produce a bit-identical (equal-fingerprint) variant regardless of `B` —
    // the property that keeps `max_expansion_nodes` out of the byte-identity
    // gates (design doc §4, §8).
    let src = breadth_source();

    let mut cfg_default = Config::default();
    cfg_default.inline.max_expansion_nodes = 25_000;
    let (_dir_a, corpus_a) = build_corpus(&[("breadth.rs", &src)], &cfg_default);
    let table_a = DefTable::build(
        &corpus_a.units,
        &corpus_a.call_sites,
        &cfg_default,
        &corpus_a.label_interner,
    );
    let idx_a = unit_idx(&corpus_a, "root");
    let exp_a = expand_unit(
        idx_a,
        &corpus_a.raw_trees[idx_a],
        &corpus_a.raw_trees,
        &corpus_a.call_sites,
        &corpus_a.units,
        &table_a,
        &cfg_default,
        &corpus_a.label_interner,
    );

    let mut cfg_max = Config::default();
    cfg_max.inline.max_expansion_nodes = u32::MAX;
    let (_dir_b, corpus_b) = build_corpus(&[("breadth.rs", &src)], &cfg_max);
    let table_b = DefTable::build(
        &corpus_b.units,
        &corpus_b.call_sites,
        &cfg_max,
        &corpus_b.label_interner,
    );
    let idx_b = unit_idx(&corpus_b, "root");
    let exp_b = expand_unit(
        idx_b,
        &corpus_b.raw_trees[idx_b],
        &corpus_b.raw_trees,
        &corpus_b.call_sites,
        &corpus_b.units,
        &table_b,
        &cfg_max,
        &corpus_b.label_interner,
    );

    let tree_a = exp_a
        .tree
        .expect("under budget at 25_000: must produce a variant");
    let tree_b = exp_b
        .tree
        .expect("under budget at u32::MAX: must produce a variant");
    // Mirror `scan()`'s real pipeline (`src/lib.rs`): a raw `expand_unit` tree is not
    // fingerprint-ready on its own — it still needs the pass/fold step `finish_variant`
    // runs before merkle-hashing (`src/unit.rs:639`).
    let variant_a = reprise::unit::finish_variant(
        idx_a,
        &corpus_a.units[idx_a],
        tree_a,
        &cfg_default,
        &corpus_a.label_interner,
    )
    .expect("variant differs from the plain unit");
    let variant_b = reprise::unit::finish_variant(
        idx_b,
        &corpus_b.units[idx_b],
        tree_b,
        &cfg_max,
        &corpus_b.label_interner,
    )
    .expect("variant differs from the plain unit");
    assert_eq!(
        variant_a.fingerprint, variant_b.fingerprint,
        "a variant that stays under budget must be byte-identical regardless of B"
    );
    assert_eq!(exp_a.calls_inlined, exp_b.calls_inlined);
}

// ---------- The backstop must be OBSERVABLE, not silent ----------

/// A budget-driven skip and a benign no-op skip (thin delegation / no resolvable
/// call) both produce `Expansion::skipped()` with `calls_inlined == 0`. Without a
/// distinct counter they are INDISTINGUISHABLE in the output, so a backstop that
/// drops a whole unit's inline-tier recall would do so with zero signal — a silent
/// recall loss in place of a loud crash. `Stats::inline_budget_skipped_units`
/// separates them.
mod budget_is_observable {
    use reprise::config::Config;
    use std::fs;
    use tempfile::TempDir;

    fn scan_with(src: &str, cfg: &Config) -> reprise::report::ScanReport {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("m0.rs"), src).unwrap();
        reprise::scan(dir.path(), cfg).unwrap()
    }

    /// Big enough to clear the scan's unit size floor (a below-floor unit is never
    /// indexed, so the inliner never sees it).
    const INLINES_SOMETHING: &str = r#"
fn helper_a(x: i64) -> i64 {
    let mut v = x + 1;
    v = v * 3 - 7;
    if v > 40 {
        v -= 11;
    }
    v + 2
}

fn root(x: i64) -> i64 {
    let mut total = 0;
    for i in 0..8 {
        let a = helper_a(x + i);
        total += a * 2;
        if total > 500 {
            total -= helper_a(a);
        }
    }
    total
}
"#;

    /// Thin delegation (D37) + a call that resolves to nothing: both benign skips.
    const BENIGN_SKIP_ONLY: &str = r#"
fn lonely(x: i64) -> i64 {
    let mut acc = x;
    for i in 0..6 {
        acc += unresolvable_external(i) * 3;
        if acc > 90 {
            acc = acc / 2 + 1;
        }
    }
    acc * 2
}

fn other(y: i64) -> i64 {
    let mut t = y;
    while t > 3 {
        t = t / 2 + some_other_external(t);
    }
    t + 9
}
"#;

    #[test]
    fn budget_skip_increments_the_counter() {
        // A REALISTIC trip: a budget small enough that the first splice fits but the
        // second blows it, so the budget fires mid-walk (the production path), not via
        // the degenerate zero-budget short-circuit.
        let mut cfg = Config::default();
        cfg.inline.max_expansion_nodes = 10;
        let report = scan_with(INLINES_SOMETHING, &cfg);
        assert!(
            report.stats.inline_budget_skipped_units >= 1,
            "a unit dropped by the expansion budget must be COUNTED, not silently \
             indistinguishable from a benign no-op: {:?}",
            report.stats
        );
        assert_eq!(
            report.stats.inline_variants, 0,
            "policy skip: the over-budget unit gets no variant"
        );

        // The degenerate zero-budget case exits through `any_resolvable_call`'s cheap
        // early return instead of the walk — it must STILL report a budget skip, not a
        // benign one.
        let mut zero = Config::default();
        zero.inline.max_expansion_nodes = 0;
        let report = scan_with(INLINES_SOMETHING, &zero);
        assert!(
            report.stats.inline_budget_skipped_units >= 1,
            "a zero budget skips via the early-exit path and must still be COUNTED: {:?}",
            report.stats
        );
    }

    #[test]
    fn benign_skips_do_not_increment_the_counter() {
        // Same fixture, default (non-binding) budget: the unit inlines, nothing is
        // budget-skipped.
        let report = scan_with(INLINES_SOMETHING, &Config::default());
        assert_eq!(
            report.stats.inline_budget_skipped_units, 0,
            "an under-budget unit must not be counted as budget-skipped"
        );

        // Thin delegation / unresolvable call: skipped, but for benign reasons.
        let report = scan_with(BENIGN_SKIP_ONLY, &Config::default());
        assert_eq!(
            report.stats.inline_variants, 0,
            "fixture sanity: these units produce no variant"
        );
        assert_eq!(
            report.stats.inline_budget_skipped_units, 0,
            "a thin-delegation / no-resolvable-call skip is NOT a budget skip — the two \
             must be distinguishable"
        );
    }

    #[test]
    fn counter_is_zero_and_identity_neutral_on_a_healthy_scan() {
        // The identity constraint: on a corpus that never trips the budget, the counter
        // is 0 and NOTHING else about the output moves as the budget varies.
        let a = scan_with(INLINES_SOMETHING, &Config::default());
        let mut huge = Config::default();
        huge.inline.max_expansion_nodes = u32::MAX;
        let b = scan_with(INLINES_SOMETHING, &huge);
        assert_eq!(a.stats.inline_budget_skipped_units, 0);
        assert_eq!(b.stats.inline_budget_skipped_units, 0);
        assert_eq!(a.stats.inline_variants, b.stats.inline_variants);
        assert_eq!(a.stats.calls_inlined, b.stats.calls_inlined);
        let fps = |r: &reprise::report::ScanReport| -> Vec<String> {
            r.groups.iter().map(|g| g.fingerprint.clone()).collect()
        };
        assert_eq!(fps(&a), fps(&b), "budget value must not perturb output");
    }
}

// ---------- The backstop is visible to a HUMAN, not just to --format json ----------

/// A backstop that silently drops a unit's inline-tier recall is worse than no backstop.
/// The terminal summary must say so when it fires — and must be byte-unchanged when it
/// does not (which is every healthy scan).
mod budget_is_visible_in_the_terminal {
    use reprise::config::Config;
    use std::fs;
    use tempfile::TempDir;

    fn render(src: &str, cfg: &Config) -> String {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("m0.rs"), src).unwrap();
        reprise::scan(dir.path(), cfg)
            .unwrap()
            .render_terminal(20, false)
    }

    const INLINES_SOMETHING: &str = r#"
fn helper_a(x: i64) -> i64 {
    let mut v = x + 1;
    v = v * 3 - 7;
    if v > 40 {
        v -= 11;
    }
    v + 2
}

fn root(x: i64) -> i64 {
    let mut total = 0;
    for i in 0..8 {
        let a = helper_a(x + i);
        total += a * 2;
        if total > 500 {
            total -= helper_a(a);
        }
    }
    total
}
"#;

    #[test]
    fn healthy_scan_summary_does_not_mention_the_budget() {
        // Zero budget skips (the default, on anything sane): the summary is exactly what
        // it always was — the backstop adds NO noise to the common case.
        let out = render(INLINES_SOMETHING, &Config::default());
        assert!(
            !out.contains("budget-skipped"),
            "a healthy scan must not mention the budget at all:\n{out}"
        );
        assert!(
            out.contains("ambiguity skips]"),
            "the inline [...] section closes right after ambiguity skips when nothing was \
             budget-skipped:\n{out}"
        );
    }

    #[test]
    fn a_budget_skip_is_reported_on_the_summary_line() {
        let mut cfg = Config::default();
        cfg.inline.max_expansion_nodes = 10; // trips mid-walk
        let out = render(INLINES_SOMETHING, &cfg);
        assert!(
            out.contains("budget-skipped"),
            "a unit dropped by the backstop MUST be visible to the human running the \
             scan, not only in --format json:\n{out}"
        );
        assert!(
            out.contains("1 budget-skipped]"),
            "the count is rendered inside the existing inline [...] section:\n{out}"
        );
    }
}
