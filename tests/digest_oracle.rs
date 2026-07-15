//! The permanent "digests never drift from trees" invariant (Tier-1 drop-trees WP).
//!
//! Every `UnitDigest` field must equal the value obtained by calling the EXISTING
//! per-field function on the unit's retained canonical tree — the fused digest pass
//! is a residency change, never a semantic one. This oracle runs over the real-code
//! `benches/wild` corpus (multi-language, both normalizer paths reachable) and stays
//! in the suite permanently.

use reprise::config::Config;
use reprise::digest::{self, UnitDigest};
use reprise::fingerprint::{self, HashMode};
use reprise::tree::NormNode;
use std::path::PathBuf;

fn wild_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("benches/wild")
}

/// Test-side depth oracle: pre-order depth per node offset, numbered exactly as
/// `fingerprint::walk_inventory` (node before children). Deliberately re-derived
/// here (a differential oracle), independent of the production walker.
fn depth_oracle(root: &NormNode) -> Vec<u16> {
    fn walk(n: &NormNode, depth: u16, out: &mut Vec<u16>) {
        out.push(depth);
        for c in &n.children {
            walk(c, depth.saturating_add(1), out);
        }
    }
    let mut out = Vec::new();
    walk(root, 0, &mut out);
    out
}

fn assert_digest_matches_tree(
    d: &UnitDigest,
    tree: &NormNode,
    _cfg: &Config,
    name: &str,
    li: &reprise::intern::LabelInterner,
) {
    // boilerplate: the existing `ir::substance` function, directly.
    assert_eq!(
        d.boilerplate_mass,
        reprise::ir::substance::boilerplate_mass(tree, li),
        "boilerplate_mass drift on {name}"
    );

    // RepData substrate: decompose against the existing `subtree_inventory` (floor 3,
    // MaskedLocals — the exact build_reps parameters) + an independent depth oracle.
    let inv = fingerprint::subtree_inventory(tree, 3, HashMode::MaskedLocals, li);
    let mut inv_flat: Vec<(u128, u32)> = inv.iter().map(|s| (s.hash, s.offset)).collect();
    inv_flat.sort_unstable();
    let mut got_flat: Vec<(u128, u32)> = d
        .offsets
        .iter()
        .flat_map(|(h, offs, _)| offs.iter().map(move |o| (h, *o)))
        .collect();
    got_flat.sort_unstable();
    assert_eq!(got_flat, inv_flat, "offsets inventory drift on {name}");
    let depths = depth_oracle(tree);
    for (h, offs, deps) in d.offsets.iter() {
        assert_eq!(offs.len(), deps.len(), "ragged offsets/depths on {name}");
        assert!(
            offs.is_sorted(),
            "offsets not ascending for hash {h:x} on {name}"
        );
        for (o, dep) in offs.iter().zip(deps) {
            assert_eq!(
                *dep, depths[*o as usize],
                "depth drift at offset {o} on {name}"
            );
        }
    }
}

#[test]
fn corpus_units_digests_match_the_tree_functions() {
    // Digests are computed only over the memory gate (the approved lazy
    // semantics — P2's under-budget path does zero new work), so the oracle
    // forces the gate: trees are still resident at the corpus_units boundary,
    // which is exactly what lets it compare digest fields against the trees.
    let root = wild_root();
    for normalizer in ["ir", "historical"] {
        let mut cfg = Config::default();
        cfg.cache.enabled = false;
        cfg.memory.force_gate = "always".into();
        if normalizer == "historical" {
            cfg.normalize.normalizer = reprise::config::Normalizer::Historical;
        }
        // Historical refuses C files (IR-frontend-only) — scan a C-free subdir set
        // for that path; the IR path scans the whole tree including C.
        let roots: Vec<PathBuf> = if normalizer == "historical" {
            ["w1_gin_marshalxml", "w4_serde_end", "w6_flask_maxprops"]
                .iter()
                .map(|d| root.join(d))
                .collect()
        } else {
            vec![root.clone()]
        };
        for r in roots {
            let corpus = reprise::corpus_units(&r, &cfg).expect("corpus_units");
            assert_eq!(
                corpus.digests.len(),
                corpus.units.len(),
                "digests must be index-aligned with units ({normalizer})"
            );
            assert!(!corpus.units.is_empty(), "empty fixture corpus {r:?}");
            for (u, d) in corpus.units.iter().zip(&corpus.digests) {
                // The stored digest is exactly what the fused pass computes...
                let fresh = digest::compute(
                    u.tree.expect_resident(),
                    u.lang,
                    &cfg,
                    u.variant.is_none(),
                    &corpus.label_interner,
                );
                assert_eq!(d, &fresh, "stored digest != recompute for {}", u.name);
                // ...and what it computes matches the existing per-field functions.
                assert_digest_matches_tree(
                    d,
                    u.tree.expect_resident(),
                    &cfg,
                    &u.name,
                    &corpus.label_interner,
                );
                // Sequence/api fields exist exactly for plain units (the two tiers
                // that filter to `variant.is_none()`).
                assert_eq!(d.seq_tokens.resident().is_some(), u.variant.is_none());
                assert_eq!(d.api_elems.is_some(), u.variant.is_none());
                // seq_tokens length is the D1 token count (one token per node).
                assert_eq!(
                    d.seq_tokens.resident().map(<[_]>::len),
                    Some(u.token_count as usize),
                    "seq stream length != token_count for {}",
                    u.name
                );
            }
        }
    }
}

#[test]
fn variant_digest_carries_no_plain_only_fields() {
    // Variants are invisible to the sequence and api tiers by construction
    // (both filter `variant.is_none()`); their digests must not carry those fields.
    let cfg = Config::default();
    let label_interner = reprise::intern::LabelInterner::new();
    let units = reprise::unit::units_from_source_with_interner(
        "fn add(a: i32) -> i32 { return a + 1; }",
        reprise::lang::Lang::Rust,
        &cfg,
        &label_interner,
    );
    let d = digest::compute(
        units[0].tree.expect_resident(),
        units[0].lang,
        &cfg,
        false,
        &label_interner,
    );
    assert_eq!(d.seq_tokens, digest::SeqSlot::Absent);
    assert!(d.api_elems.is_none());
    assert_digest_matches_tree(
        &d,
        units[0].tree.expect_resident(),
        &cfg,
        "variant-shaped digest",
        &label_interner,
    );
}
