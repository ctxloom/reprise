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
fn estimator_is_the_pinned_phase_max_model() {
    // The estimate is a pure, pinned function of (plain token sum, unit count,
    // inline on/off, landmark fan-out). The peak is a MAX over two phases, not a
    // single scalar — measured on fs/net/drivers (all three corpora, both gate
    // regimes; see the constants' doc comments):
    //
    //   extraction phase: EXTRACT × tokens                     (no index yet)
    //   near phase:       TREE × tokens + INDEX × fan_out × tokens
    //
    // plus the RepData substrate, times the variant inflation when inline runs.
    // No RSS sampling, no environment dependence.
    let tokens = 10_000u64;
    let units = 40usize;
    let fan_out = 3usize;

    let near = memory::PER_TOKEN_TREE_BYTES * tokens
        + memory::PER_FANOUT_TOKEN_INDEX_BYTES * fan_out as u64 * tokens;
    let extract = memory::PER_TOKEN_EXTRACT_BYTES * tokens;
    let expected_base = near.max(extract) + units as u64 * memory::REPDATA_PER_UNIT_BYTES;

    let base = memory::estimate_bytes(tokens, units, false, fan_out);
    assert_eq!(
        base, expected_base,
        "inline-off estimate must be the phase-max model"
    );

    let inflated = memory::estimate_bytes(tokens, units, true, fan_out);
    assert_eq!(
        inflated,
        (expected_base as f64 * memory::VARIANT_INFLATION).round() as u64,
        "inflation factor drifted"
    );

    // THE POINT OF THE MODEL: the landmark index is the largest memory row in a
    // large scan, and it scales with fan_out. The estimate MUST move when the dial
    // moves — the old single-scalar model was blind to it (a fan_out=3 constant
    // that never budged), which is exactly the defect this replaces.
    let f4 = memory::estimate_bytes(tokens, units, false, 4);
    assert!(
        f4 > base,
        "raising fan_out must raise the estimate — the index term is what makes \
         the gate able to see the index at all"
    );
    let f2 = memory::estimate_bytes(tokens, units, false, 2);
    assert!(f2 < base, "lowering fan_out must lower the estimate");
    // Linear in fan_out inside the near-bound regime: equal steps, equal deltas.
    assert_eq!(
        f4 - base,
        base - f2,
        "the index term must be linear in fan_out"
    );
    assert_eq!(
        f4 - base,
        memory::PER_FANOUT_TOKEN_INDEX_BYTES * tokens,
        "per-fan_out step must be exactly the measured index coefficient"
    );

    // fan_out = 0 disables landmark hashing entirely (no pairs, no index): the
    // estimate must fall back to the index-free extraction peak, NOT to a tree
    // term that silently omits a phase.
    assert_eq!(
        memory::estimate_bytes(tokens, units, false, 0),
        extract + units as u64 * memory::REPDATA_PER_UNIT_BYTES,
        "fan_out=0 must be extraction-bound"
    );

    // Degenerate corpus: zero estimate, never a panic.
    assert_eq!(memory::estimate_bytes(0, 0, true, 3), 0);
}

#[test]
fn estimator_tracks_measured_peaks_on_the_calibration_corpora() {
    // The constants are not taste — they are fits to measured VmHWM on fs, net and
    // drivers (linux 7.1), captured with a fan_out sweep so the index term is
    // separated from the tree term rather than fused into it. This test pins the
    // model against those measurements: the estimate must be CONSERVATIVE (never
    // under-predict a resident peak — under-prediction is an OOM, over-prediction is
    // merely an unnecessary spill) and must stay within a sane band (a model that
    // over-predicts by 2x would spill everything and is no better than the fused
    // constant it replaced).
    //
    // Measured under JEMALLOC, the shipped global allocator (src/main.rs). Measuring
    // from a harness that does not declare `#[global_allocator]` silently gets glibc
    // malloc instead, which reads ~4% low here — enough to make the model appear
    // conservative when it is in fact under-predicting. Re-measure with the real
    // binary, never an ad-hoc example.
    //
    // Measured resident VmHWM at the default fan_out = 3 (bytes):
    let cases = [
        // (corpus, plain_tokens, plain_units, measured_resident_vmhwm)
        ("fs", 4_268_773u64, 39_367usize, 2_273_840u64 * 1024),
        ("net", 3_780_068, 38_324, 1_829_892 * 1024),
    ];
    for (name, tokens, units, measured) in cases {
        let est = memory::estimate_bytes(tokens, units, true, 3);
        assert!(
            est >= measured,
            "{name}: estimate {est} UNDER-predicts measured resident peak {measured} — \
             the gate would stay resident and OOM"
        );
        let ratio = est as f64 / measured as f64;
        assert!(
            ratio < 1.25,
            "{name}: estimate is {ratio:.2}x measured — too loose to be useful"
        );
    }
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
