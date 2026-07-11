//! Characterization net for the Tier-1 drop-trees WP (memory work): pins the FULL
//! report (every section, fingerprints, values, members, stats) produced by scanning
//! the real-code `benches/wild` fixtures, against committed snapshot files.
//!
//! The memory-gate invariant is "the gate changes performance, never output" — this
//! suite is the unit-test-grain regression net for that: it must be green BEFORE and
//! AFTER every step of the digest/spill work, and (once the gate exists) green with
//! the gate forced on. Regenerate a *reviewed* diff with:
//!
//!   UPDATE_CHARACTERIZATION=1 cargo test --test characterization
//!
//! (mirrors the `UPDATE_GRAMMAR_SNAPSHOTS` pattern in `src/frontend`).

use reprise::ScanReport;
use reprise::config::{Config, Normalizer};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn wild_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("benches/wild")
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/characterization")
        .join(name)
}

/// Deterministic, human-diffable dump of everything in a report that the memory
/// work must not change. Excludes only wall-clock noise (`phase_ms`, `duration_ms`)
/// and — by never printing them — the WP's own additive `memory_*` stats (the one
/// approved output addition; they legitimately differ across gate states).
fn canonical(report: &ScanReport, root: &Path) -> String {
    let rel = |p: &Path| -> String {
        p.strip_prefix(root)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/")
    };
    let mut out = String::new();
    for (section, groups) in [
        ("groups", &report.groups),
        ("test_groups", &report.test_groups),
        ("api_groups", &report.api_groups),
        ("weak_groups", &report.weak_groups),
    ] {
        writeln!(out, "== {section} ({}) ==", groups.len()).unwrap();
        for g in groups {
            writeln!(
                out,
                "{} tier={} fp={} tokens={} value={:?} div={:?} note={:?} chain={:?} template_hash={:?}",
                g.id,
                g.tier,
                g.fingerprint,
                g.token_count,
                g.value,
                g.divergence,
                g.note,
                g.inline_chain,
                g.template
                    .as_deref()
                    .map(|t| xxhash_rust::xxh3::xxh3_64(t.as_bytes())),
            )
            .unwrap();
            for m in &g.members {
                writeln!(
                    out,
                    "  member {}:{} span={:?} lang={} degraded={}",
                    rel(&m.file),
                    m.name,
                    m.line_span,
                    m.lang,
                    m.parse_degraded
                )
                .unwrap();
            }
        }
    }
    writeln!(out, "== unit_index ({}) ==", report.unit_index.len()).unwrap();
    for u in &report.unit_index {
        writeln!(
            out,
            "{}:{} span={:?} fp={} accept_drift={}",
            rel(&u.file),
            u.name,
            u.line_span,
            u.fingerprint,
            u.accept_drift
        )
        .unwrap();
    }
    let s = &report.stats;
    writeln!(out, "== stats ==").unwrap();
    writeln!(out, "files_scanned={}", s.files_scanned).unwrap();
    writeln!(out, "files_skipped_generated={}", s.files_skipped_generated).unwrap();
    writeln!(out, "files_unreadable={}", s.files_unreadable).unwrap();
    writeln!(out, "units_indexed={}", s.units_indexed).unwrap();
    writeln!(out, "units_below_floor={}", s.units_below_floor).unwrap();
    writeln!(out, "parse_degraded_units={}", s.parse_degraded_units).unwrap();
    writeln!(out, "test_units={}", s.test_units).unwrap();
    writeln!(out, "findings_by_tier={:?}", s.findings_by_tier).unwrap();
    writeln!(out, "sequence_regions_found={}", s.sequence_regions_found).unwrap();
    writeln!(
        out,
        "sequence_regions_subsumed={}",
        s.sequence_regions_subsumed
    )
    .unwrap();
    writeln!(out, "regions_substantial={}", s.regions_substantial).unwrap();
    writeln!(out, "groups_subsumed_subset={}", s.groups_subsumed_subset).unwrap();
    writeln!(out, "inline_variants={}", s.inline_variants).unwrap();
    writeln!(out, "ambiguity_skips={}", s.ambiguity_skips).unwrap();
    writeln!(out, "calls_inlined={}", s.calls_inlined).unwrap();
    writeln!(out, "scc_units={}", s.scc_units).unwrap();
    writeln!(out, "api_signatures={}", s.api_signatures).unwrap();
    writeln!(out, "suppressed_units={}", s.suppressed_units).unwrap();
    writeln!(out, "retrieval={:?}", s.retrieval).unwrap();
    writeln!(out, "total_lines={}", s.total_lines).unwrap();
    writeln!(out, "total_tokens={}", s.total_tokens).unwrap();
    writeln!(out, "duplicated_lines={}", s.duplicated_lines).unwrap();
    writeln!(out, "duplicated_lines_pct={:?}", s.duplicated_lines_pct).unwrap();
    writeln!(out, "duplicated_tokens={}", s.duplicated_tokens).unwrap();
    writeln!(out, "duplicated_tokens_pct={:?}", s.duplicated_tokens_pct).unwrap();
    writeln!(out, "clones_per_kloc={:?}", s.clones_per_kloc).unwrap();
    out
}

fn assert_or_update(name: &str, actual: &str) {
    let path = fixture_path(name);
    if std::env::var_os("UPDATE_CHARACTERIZATION").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "missing characterization snapshot {} ({e}); regenerate with \
             UPDATE_CHARACTERIZATION=1 cargo test --test characterization",
            path.display()
        )
    });
    if expected != actual {
        // Write the actual next to the snapshot for a reviewable diff.
        let got = path.with_extension("actual");
        std::fs::write(&got, actual).unwrap();
        panic!(
            "characterization drift in {name}: report output changed.\n\
             diff {} {}\n\
             If (and only if) the change is intended, regenerate with \
             UPDATE_CHARACTERIZATION=1.",
            path.display(),
            got.display()
        );
    }
}

/// Scan configurations the WP must hold byte-stable. Once the memory gate exists,
/// `gate_states()` grows a forced-over-gate variant so every snapshot is asserted
/// identical on BOTH sides of the gate (the P2 invariant), not just under it.
type GateStates = Vec<(&'static str, fn(&mut Config))>;

fn gate_states() -> GateStates {
    vec![
        ("under-gate", |_cfg: &mut Config| {}),
        ("forced-over-gate", |cfg: &mut Config| {
            cfg.memory.force_gate = "always".into();
        }),
    ]
}

#[test]
fn wild_corpus_report_is_pinned_ir() {
    // The whole benches/wild tree as ONE multi-language corpus, default (IR)
    // normalizer — every tier (exact/near/inline/sequence/api/internal-repeat)
    // exercised over real-world code, single snapshot.
    let root = wild_root();
    let mut base = Config::default();
    base.cache.enabled = false; // don't litter fixture dirs with .reprise/
    let mut snapshots: Vec<String> = Vec::new();
    for (state, mutate) in gate_states() {
        let mut cfg = base.clone();
        mutate(&mut cfg);
        let report = reprise::scan(&root, &cfg).expect("wild scan");
        let canon = canonical(&report, &root);
        assert!(
            snapshots.iter().all(|prev| prev == &canon),
            "gate state `{state}` changed the report — the gate must change \
             performance, never output (P2)"
        );
        snapshots.push(canon);
    }
    assert_or_update("wild_ir.expected", &snapshots[0]);
}

#[test]
fn wild_corpus_report_is_pinned_historical() {
    // Historical-normalizer path (C fixtures excluded: C is IR-frontend-only, so a
    // whole-tree historical scan would refuse; scan the historical-capable dirs
    // individually and concatenate).
    let dirs = [
        "w1_gin_marshalxml",
        "w2_ripgrep_bytecount",
        "w3_ripgrep_sort",
        "w4_serde_end",
        "w5_serde_tagorcontent",
        "w6_flask_maxprops",
        "w7_click_chunkpump",
        "w8_ripgrep_convert",
        "w9_ripgrep_cpufeatures",
    ];
    let mut base = Config::default();
    base.normalize.normalizer = Normalizer::Historical;
    base.cache.enabled = false;
    let mut snapshots: Vec<String> = Vec::new();
    for (state, mutate) in gate_states() {
        let mut cfg = base.clone();
        mutate(&mut cfg);
        let mut canon = String::new();
        for dir in dirs {
            let root = wild_root().join(dir);
            let report = reprise::scan(&root, &cfg).expect("wild historical scan");
            writeln!(canon, "#### {dir} ####").unwrap();
            canon.push_str(&canonical(&report, &root));
        }
        assert!(
            snapshots.iter().all(|prev| prev == &canon),
            "gate state `{state}` changed the historical report (P2 violation)"
        );
        snapshots.push(canon);
    }
    assert_or_update("wild_historical.expected", &snapshots[0]);
}
