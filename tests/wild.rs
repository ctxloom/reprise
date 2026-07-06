//! Wild recall/precision net (DECISIONS.md D6): fixtures under `benches/wild/`
//! are verbatim sources of hand-labeled findings from the Phase-4 stratified
//! sample (provenance in benches/wild/README.md). Tier-labeled fixtures must
//! keep converging; `"none"` fixtures are documented NON-findings (patterns
//! reprise deliberately suppresses, e.g. thin delegation wrappers, D37) and
//! must stay silent. Unlike the mutation benchmark this cannot be gamed by
//! rules that merely invert our own generators.

use reprise::config::Config;
use std::path::PathBuf;

const WILD: &[(&str, &str, usize)] = &[
    // (fixture dir, labeled tier, expected members)
    ("w1_gin_marshalxml", "near-normalized", 2),
    ("w2_ripgrep_bytecount", "exact-normalized", 2),
    ("w3_ripgrep_sort", "exact-normalized", 2),
    ("w4_serde_end", "exact-normalized", 2),
    ("w5_serde_tagorcontent", "near-normalized", 2),
    ("w6_flask_maxprops", "near-normalized", 2),
    ("w7_click_chunkpump", "exact-region", 2),
    // Relabeled TP→by-design non-finding (D37): usize/u64 are single-statement
    // wrappers around the already-extracted `str` helper — flagging residual
    // thin delegation is the tool flagging its own recommended fix pattern.
    ("w8_ripgrep_convert", "none", 0),
    ("w9_ripgrep_cpufeatures", "internal-repeat", 1),
];

fn wild_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("benches/wild")
}

#[test]
fn every_wild_pair_converges_at_its_labeled_tier() {
    // Pinned to the still-supported historical normalizer: the WILD table carries the
    // historical tier labels. IR wild recall parity is the separate gate
    // `wild_pairs_converge_under_the_ir_normalizer` (the only IR loss is w7, an exact-region run
    // held out by `min_seq_tokens`, Decision 4).
    let mut cfg = Config::default();
    cfg.normalize.normalizer = "historical".into();
    cfg.cache.enabled = false; // don't litter fixture dirs with .reprise/
    for (dir, tier, members) in WILD {
        let root = wild_root().join(dir);
        assert!(root.is_dir(), "missing wild fixture {dir}");
        let report = reprise::scan(&root, &cfg)
            .unwrap_or_else(|e| panic!("scan of wild fixture {dir} failed: {e}"));
        if *tier == "none" {
            assert!(
                report.groups.is_empty(),
                "wild non-finding {dir} started converging (groups: {:#?})",
                report.groups
            );
            continue;
        }
        let hit = report
            .groups
            .iter()
            .find(|g| g.tier.to_string() == *tier && g.members.len() >= *members);
        assert!(
            hit.is_some(),
            "wild pair {dir} no longer converges at {tier} (groups: {:#?})",
            report.groups
        );
    }
}

/// The IR path (`[normalize] normalizer = "ir"`) must keep converging the hand-labeled wild
/// pairs — the recall-parity gate for flipping the default normalizer (§8). The IR canonical
/// trees are ~18% more compact, so the size/vote floors are per-normalizer
/// (`min_unit_tokens_ir`, `histogram_min_votes_ir`); with those, every wild pair historical
/// finds also converges on IR EXCEPT w7 — an exact-region run that lands at exactly 25 IR
/// tokens, below the unrecalibrated `min_seq_tokens` floor (a reported precision/recall fork,
/// deliberately NOT applied: the compaction-scaled seq floor of 25 costs measurable
/// precision). w6 is the Gap-B recovery (`histogram_min_votes_ir`); w8 stays a non-finding.
#[test]
fn wild_pairs_converge_under_the_ir_normalizer() {
    let mut cfg = Config::default();
    cfg.cache.enabled = false;
    cfg.normalize.normalizer = "ir".into();
    // (fixture, converges under IR). w7: the residual `min_seq_tokens` fork. w8: non-finding.
    let ir_expect: &[(&str, bool)] = &[
        ("w1_gin_marshalxml", true),
        ("w2_ripgrep_bytecount", true),
        ("w3_ripgrep_sort", true),
        ("w4_serde_end", true),
        ("w5_serde_tagorcontent", true),
        ("w6_flask_maxprops", true),
        ("w7_click_chunkpump", false),
        ("w8_ripgrep_convert", false),
        ("w9_ripgrep_cpufeatures", true),
    ];
    for (dir, converges) in ir_expect {
        let root = wild_root().join(dir);
        let report = reprise::scan(&root, &cfg)
            .unwrap_or_else(|e| panic!("IR scan of wild fixture {dir} failed: {e}"));
        assert_eq!(
            !report.groups.is_empty(),
            *converges,
            "wild fixture {dir} under IR: expected converges={converges}, groups: {:#?}",
            report.groups
        );
    }
    // Gap B lock-in: flask's `max_content_length`/`max_form_memory_size` near-clone offers only
    // 4 shared aligned subtrees on the compact IR tree — below the historical 5-vote histogram
    // floor, but recovered by `histogram_min_votes_ir` (AU then accepts it at divergence ~0.06).
    let w6 = reprise::scan(&wild_root().join("w6_flask_maxprops"), &cfg).unwrap();
    assert!(
        w6.groups
            .iter()
            .any(|g| g.tier.to_string() == "near-normalized" && g.members.len() >= 2),
        "w6 must converge at near-normalized on the IR path: {:#?}",
        w6.groups
    );
}

/// The fixture table and the on-disk corpus must not drift apart silently.
#[test]
fn wild_corpus_has_no_untested_fixtures() {
    let tested: Vec<&str> = WILD.iter().map(|(d, ..)| *d).collect();
    for entry in std::fs::read_dir(wild_root()).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            // `licenses/` holds vendored upstream license texts for the copied
            // fixture sources (THIRD-PARTY-NOTICES.md), not a fixture pair.
            if name == "licenses" {
                continue;
            }
            assert!(
                tested.contains(&name.as_str()),
                "benches/wild/{name} exists but has no row in tests/wild.rs"
            );
        }
    }
}
