//! Normalized token streams (DECISIONS.md D1/D4): the sequence tier's input is
//! the pre-order serialization of the SAME normalized trees the tree tier
//! hashes, with locals masked (D4 amendment: sub-unit runs must tolerate
//! positional-index drift across units) and per-unit unique separators.
//!
//! M3b: built in parallel — per-unit token hash streams first, then dense
//! u32 ids assigned by rank in the sorted unique-hash list (deterministic,
//! no shared interner).

use crate::tree::{Label, NormNode};
use rayon::prelude::*;

pub struct Corpus {
    pub tokens: Vec<u32>,
    /// Byte span per token position (separator positions map to (0,0)).
    pub spans: Vec<(u32, u32)>,
    /// Unit index per token position (u32::MAX for separators).
    pub unit_of: Vec<u32>,
    /// Interner-independent content hash per token (0 for separators): the
    /// stable identity a region's baseline fingerprint hashes over (spec §2 —
    /// dense token ids depend on the corpus, these don't).
    pub key_hash: Vec<u64>,
}

const SEP_BASE: u32 = u32::MAX / 2;

/// One unit's sequence stream: (content hash, byte span) per node, pre-order — the
/// exact per-unit slice [`build_corpus`] assembles. Public so the fused digest pass
/// ([`crate::digest`]) computes it with the SAME serializer at unit-creation time.
pub fn unit_stream(tree: &NormNode) -> Vec<(u64, (u32, u32))> {
    let mut out = Vec::new();
    let mut buf = String::with_capacity(64);
    serialize(tree, &mut buf, &mut out);
    out
}

/// Assemble one language partition's corpus from the units' per-unit streams
/// (`unit_stream`, carried by the fused digests since the drop-trees work —
/// this function no longer walks trees). `streams[i]`'s tokens are attributed
/// to unit index `i` within the partition, mirroring the old `&[&Unit]` order.
pub fn build_corpus(streams: &[&[(u64, (u32, u32))]]) -> Corpus {
    // Dense ids by rank in the sorted unique-hash list.
    let mut uniq: Vec<u64> = streams
        .par_iter()
        .flat_map_iter(|s| s.iter().map(|(h, _)| *h))
        .collect();
    uniq.par_sort_unstable();
    uniq.dedup();
    debug_assert!((uniq.len() as u32) < SEP_BASE, "token space overflow");

    let total: usize = streams.iter().map(|s| s.len() + 1).sum();
    let mut corpus = Corpus {
        tokens: Vec::with_capacity(total),
        spans: Vec::with_capacity(total),
        unit_of: Vec::with_capacity(total),
        key_hash: Vec::with_capacity(total),
    };
    for (idx, stream) in streams.iter().enumerate() {
        for &(h, span) in *stream {
            let id = uniq.binary_search(&h).expect("hash present") as u32;
            corpus.tokens.push(id);
            corpus.spans.push(span);
            corpus.unit_of.push(idx as u32);
            corpus.key_hash.push(h);
        }
        // Unique separator: no repeat can cross a unit boundary.
        corpus.tokens.push(SEP_BASE + idx as u32);
        corpus.spans.push((0, 0));
        corpus.unit_of.push(u32::MAX);
        corpus.key_hash.push(0);
    }
    corpus
}

fn serialize(node: &NormNode, buf: &mut String, out: &mut Vec<(u64, (u32, u32))>) {
    buf.clear();
    buf.push_str(&node.kind);
    buf.push('\u{1f}');
    if let Some(field) = &node.field {
        buf.push_str(field);
    }
    buf.push('\u{1f}');
    match &node.label {
        None => {}
        Some(Label::External(s)) => {
            buf.push('E');
            buf.push_str(s);
        }
        Some(Label::Local(_)) => buf.push('L'), // masked (D4)
        Some(Label::LitKept(s)) => {
            buf.push('K');
            buf.push_str(s);
        }
        Some(Label::LitBucket(b)) => {
            buf.push('B');
            buf.push_str(b.name());
        }
        Some(Label::Raw(s)) | Some(Label::RawLit(s)) => {
            buf.push('R');
            buf.push_str(s);
        }
    }
    out.push((xxhash_rust::xxh3::xxh3_64(buf.as_bytes()), node.span));
    for child in &node.children {
        serialize(child, buf, out);
    }
}
