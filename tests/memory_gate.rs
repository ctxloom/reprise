//! Gate 2 (memory architecture P2): the post-extraction estimator, the
//! `[memory]` config knob, the force override, and the end-to-end invariant —
//! **the gate changes performance, never output**.

use reprise::config::Config;
use reprise::memory;
use std::path::PathBuf;

fn wild_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("benches/wild")
}

#[test]
fn estimator_is_the_pinned_linear_model() {
    // The estimate is a pure, pinned function of (plain token sum, unit count,
    // inline on/off): tokens × the validated per-token tree constant, times the
    // measured variant inflation when the inliner will add variants, plus the
    // RepData substrate term. No RSS sampling, no environment dependence.
    let tokens = 10_000u64;
    let units = 40usize;
    let base = memory::estimate_bytes(tokens, units, false);
    assert_eq!(
        base,
        tokens * memory::PER_TOKEN_TREE_BYTES + units as u64 * memory::REPDATA_PER_UNIT_BYTES,
        "inline-off estimate must be the bare linear model"
    );
    let inflated = memory::estimate_bytes(tokens, units, true);
    assert!(
        inflated > base,
        "inline-on must inflate the estimate (variants add ~49-55% tree mass)"
    );
    let expected = (base as f64 * memory::VARIANT_INFLATION).round() as u64;
    assert_eq!(inflated, expected, "inflation factor drifted");
    // Degenerate corpus: zero estimate, never a panic.
    assert_eq!(memory::estimate_bytes(0, 0, true), 0);
}

#[test]
fn trip_point_has_hysteresis_headroom() {
    // Trips at ~70% of budget (P2's headroom for the estimator's known
    // undercounts), not at 100%.
    let budget = 1_000_000u64;
    let below = (budget as f64 * memory::TRIP_FRACTION) as u64 - 1;
    let at = (budget as f64 * memory::TRIP_FRACTION) as u64 + 1;
    assert!(!memory::trips(below, budget));
    assert!(memory::trips(at, budget));
    assert!(memory::trips(budget + 1, budget));
    // A zero budget (misconfiguration) fails safe: everything trips.
    assert!(memory::trips(1, 0));
}

#[test]
fn budget_resolves_pinned_bytes_over_fraction() {
    let mut cfg = Config::default();
    cfg.memory.budget_bytes = Some(123_456_789);
    assert_eq!(
        memory::resolve_budget(&cfg.memory),
        123_456_789,
        "explicit budget_bytes must win over the RAM fraction"
    );
    // Default path: a fraction of detected system RAM — positive on any
    // machine this suite runs on.
    let auto = memory::resolve_budget(&Config::default().memory);
    assert!(auto > 0, "RAM-derived default budget must be positive");
}

#[test]
fn force_gate_overrides_the_estimate_config_then_env() {
    // A tiny corpus can never trip the real estimator; force_gate = "always"
    // must trip it anyway (the CI/dev knob for exercising the spill path), and
    // "never" must hold it under even with an over-budget estimate. The env
    // check lives in the SAME test because env vars are process-global — a
    // parallel test setting REPRISE_MEMORY_FORCE_GATE would race the config
    // assertions.
    let units = reprise::unit::units_from_source(
        "fn add(a: i32) -> i32 { return a + 1; }",
        reprise::lang::Lang::Rust,
        &Config::default(),
    );
    let mut cfg = Config::default();
    cfg.memory.force_gate = "always".into();
    assert!(memory::decide(&units, &cfg).over);

    cfg.memory.force_gate = "never".into();
    cfg.memory.budget_bytes = Some(1); // absurdly over budget
    assert!(!memory::decide(&units, &cfg).over);

    cfg.memory.force_gate = "auto".into();
    assert!(
        memory::decide(&units, &cfg).over,
        "auto with a 1-byte budget must trip"
    );

    // REPRISE_MEMORY_FORCE_GATE outranks config — the identity harness forces
    // the spill path on a release binary without editing any config file.
    let plain = Config::default(); // force_gate = "auto", real budget
    unsafe { std::env::set_var("REPRISE_MEMORY_FORCE_GATE", "always") };
    let over = memory::decide(&units, &plain).over;
    unsafe { std::env::remove_var("REPRISE_MEMORY_FORCE_GATE") };
    assert!(over, "env override must force the gate on");
}

#[test]
fn digests_exist_exactly_when_the_gate_is_over() {
    // The approved lazy-digest semantics (P2: "under budget → today's path,
    // zero new work"): `corpus_units` computes the fused digests ONLY over the
    // gate — under it the vector is empty and every consumer walks the
    // resident trees exactly as before the drop-trees work. This is also the
    // Tier-2 join contract: digests exist exactly when a gate is over.
    let root = wild_root();
    let mut under = Config::default();
    under.cache.enabled = false;
    let corpus = reprise::corpus_units(&root, &under).expect("under-gate corpus");
    assert!(
        !corpus.gate.over,
        "wild fixtures must not trip a real budget"
    );
    assert!(
        corpus.digests.is_empty(),
        "under-gate corpus_units must not compute digests (zero new work)"
    );

    let mut over = Config::default();
    over.cache.enabled = false;
    over.memory.force_gate = "always".into();
    let corpus = reprise::corpus_units(&root, &over).expect("over-gate corpus");
    assert!(corpus.gate.over);
    assert_eq!(
        corpus.digests.len(),
        corpus.units.len(),
        "over-gate digests must be index-aligned with units"
    );
    // Trees are still resident at the corpus_units boundary either way —
    // spilling is scan()'s business, strictly after this returns.
    assert!(corpus.units.iter().all(|u| u.tree.resident().is_some()));
}

#[test]
fn forced_gate_scan_spills_and_reports_it() {
    // End-to-end over-gate scan of a real fixture corpus: the gate must be
    // observable in stats (which path ran, how much spilled) — measurements
    // need a live number, not an assumption.
    let mut cfg = Config::default();
    cfg.cache.enabled = false;
    cfg.memory.force_gate = "always".into();
    let report = reprise::scan(&wild_root(), &cfg).expect("forced-over-gate scan");
    let s = &report.stats;
    assert!(s.memory_gate_tripped, "forced gate must report as tripped");
    assert!(
        s.memory_spilled_trees > 0,
        "over-gate scan must spill trees to the pack"
    );
    assert!(
        s.memory_pack_bytes > 0,
        "spilled trees must occupy pack bytes"
    );
    assert!(s.memory_budget_bytes > 0);
    assert!(s.memory_estimated_bytes > 0);

    // Under-gate control: same corpus, defaults — nothing spills, and the
    // gate decision is still observable.
    let mut under = Config::default();
    under.cache.enabled = false;
    let control = reprise::scan(&wild_root(), &under).expect("under-gate scan");
    assert!(!control.stats.memory_gate_tripped);
    assert_eq!(control.stats.memory_spilled_trees, 0);
    assert_eq!(control.stats.memory_pack_bytes, 0);

    // P2's invariant at the finding level: identical groups both sides.
    // (tests/characterization.rs pins the full report; this is the quick
    // in-place cross-check.)
    let key = |r: &reprise::ScanReport| -> Vec<(String, String, u32)> {
        r.groups
            .iter()
            .chain(&r.test_groups)
            .chain(&r.api_groups)
            .chain(&r.weak_groups)
            .map(|g| (g.tier.to_string(), g.fingerprint.clone(), g.token_count))
            .collect()
    };
    assert_eq!(
        key(&report),
        key(&control),
        "the gate changed output — P2 violation"
    );
}
