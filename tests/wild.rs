//! Wild-pair recall net (DECISIONS.md D6): every fixture under `benches/wild/`
//! is the verbatim source of a hand-labeled TRUE-POSITIVE finding from the
//! Phase-4 stratified precision sample (provenance in benches/wild/README.md).
//! Each must keep converging at its labeled tier — a non-synthetic regression
//! guard that, unlike the mutation benchmark, cannot be gamed by rules that
//! merely invert our own generators.

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
    ("w8_ripgrep_convert", "inline-assisted", 2),
    ("w9_ripgrep_cpufeatures", "internal-repeat", 1),
];

fn wild_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("benches/wild")
}

#[test]
fn every_wild_pair_converges_at_its_labeled_tier() {
    let mut cfg = Config::default();
    cfg.cache.enabled = false; // don't litter fixture dirs with .reprise/
    for (dir, tier, members) in WILD {
        let root = wild_root().join(dir);
        assert!(root.is_dir(), "missing wild fixture {dir}");
        let report = reprise::scan(&root, &cfg)
            .unwrap_or_else(|e| panic!("scan of wild fixture {dir} failed: {e}"));
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

/// The fixture table and the on-disk corpus must not drift apart silently.
#[test]
fn wild_corpus_has_no_untested_fixtures() {
    let tested: Vec<&str> = WILD.iter().map(|(d, ..)| *d).collect();
    for entry in std::fs::read_dir(wild_root()).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            assert!(
                tested.contains(&name.as_str()),
                "benches/wild/{name} exists but has no row in tests/wild.rs"
            );
        }
    }
}
