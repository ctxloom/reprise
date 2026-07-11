//! P6: structural fingerprints (spec §5.5).
//!
//! Three hash modes (DECISIONS.md D2):
//! - `Exact`: positional locals — the whole-unit exact tier (§5.5.1).
//! - `MaskedLocals`: locals collapse to one marker — retrieval layers, so a
//!   single inserted declaration can't cascade through every later v-index.
//! - `MaskedAll`: locals AND all literals masked — sibling-run folding (§5.3).

use crate::intern::LabelInterner;
use crate::tree::{Label, NormNode};
use xxhash_rust::xxh3::xxh3_128;

/// Fingerprint-scheme version (spec §6.1/§12): covers the D1 token definition,
/// the normalization pass set, and every hash construction in this module.
/// Bump on ANY change that shifts fingerprints; baselines carry it (a mismatch
/// demands re-baseline, never silent comparison) and the D8 cache keys embed
/// it (a bump invalidates the whole index).
pub const FINGERPRINT_SCHEME: u32 = 1;

/// Canonical rendering of a 128-bit fingerprint for baselines and reports.
pub fn hex(fp: u128) -> String {
    format!("{fp:032x}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashMode {
    Exact,
    MaskedLocals,
    MaskedAll,
}

/// Push `label`'s hash contribution. `li` resolves `Label::External`/`LitKept`'s
/// `LSym` id back to its text (fingerprints stay byte-identical to pre-interning:
/// the RESOLVED string bytes are what get hashed, never the id — see the
/// interning-id-conversion WP's mechanism rule 4). This is the one place on the
/// fingerprinting path that touches the label interner; it is NOT a per-node
/// matching COMPARISON (those compare `Label`/`LSym` by id, upstream of here) —
/// hashing inherently needs the resolved bytes regardless of `Label`'s in-memory
/// representation, exactly as it did before this WP.
fn push_label(buf: &mut Vec<u8>, label: &Option<Label>, mode: HashMode, li: &LabelInterner) {
    match label {
        None => buf.push(b'_'),
        Some(Label::External(sym)) => {
            buf.push(b'E');
            buf.extend_from_slice(li.resolve(*sym).as_bytes());
        }
        Some(Label::Local(index)) => {
            buf.push(b'L');
            if mode == HashMode::Exact {
                buf.extend_from_slice(&index.to_le_bytes());
            }
        }
        Some(Label::LitKept(sym)) => match mode {
            HashMode::MaskedAll => buf.push(b'M'),
            _ => {
                buf.push(b'K');
                buf.extend_from_slice(li.resolve(*sym).as_bytes());
            }
        },
        Some(Label::LitBucket(bucket)) => match mode {
            HashMode::MaskedAll => buf.push(b'M'),
            _ => {
                buf.push(b'B');
                buf.extend_from_slice(bucket.name().as_bytes());
            }
        },
        Some(Label::Raw(text)) | Some(Label::RawLit(text)) => {
            debug_assert!(false, "transient Raw label survived normalization: {text}");
            buf.push(b'R');
            buf.extend_from_slice(text.as_bytes());
        }
    }
}

fn hash_from_parts(
    node: &NormNode,
    mode: HashMode,
    child_hashes: &[u128],
    li: &LabelInterner,
) -> u128 {
    let mut buf = Vec::with_capacity(64 + 16 * child_hashes.len());
    buf.extend_from_slice(node.kind.as_str().as_bytes());
    buf.push(0);
    if let Some(field) = &node.field {
        buf.extend_from_slice(field.as_str().as_bytes());
    }
    buf.push(0);
    push_label(&mut buf, &node.label, mode, li);
    buf.push(0);
    for h in child_hashes {
        buf.extend_from_slice(&h.to_le_bytes());
    }
    xxh3_128(&buf)
}

pub fn merkle_mode(node: &NormNode, mode: HashMode, li: &LabelInterner) -> u128 {
    let child_hashes: Vec<u128> = node
        .children
        .iter()
        .map(|c| merkle_mode(c, mode, li))
        .collect();
    hash_from_parts(node, mode, &child_hashes, li)
}

/// Whole-unit exact structural hash (tier `exact-normalized`).
pub fn merkle(node: &NormNode, li: &LabelInterner) -> u128 {
    merkle_mode(node, HashMode::Exact, li)
}

/// One subtree of the bag inventory: hash + pre-order token offset + size.
/// Offsets feed the offset-histogram verification (spec §5.6).
#[derive(Debug, Clone, Copy)]
pub struct Subtree {
    pub hash: u128,
    pub offset: u32,
    pub tokens: u32,
}

/// All subtrees ≥ `min_tokens`, in the given mode (spec §5.5.2).
pub fn subtree_inventory(
    node: &NormNode,
    min_tokens: u32,
    mode: HashMode,
    li: &LabelInterner,
) -> Vec<Subtree> {
    let mut out = Vec::new();
    let mut offset = 0u32;
    walk_inventory(node, min_tokens, mode, &mut offset, &mut out, li);
    out
}

fn walk_inventory(
    node: &NormNode,
    min_tokens: u32,
    mode: HashMode,
    offset: &mut u32,
    out: &mut Vec<Subtree>,
    li: &LabelInterner,
) -> (u128, u32) {
    let my_offset = *offset;
    *offset += 1;
    let mut child_hashes = Vec::with_capacity(node.children.len());
    let mut tokens = 1u32;
    for child in &node.children {
        let (h, t) = walk_inventory(child, min_tokens, mode, offset, out, li);
        child_hashes.push(h);
        tokens += t;
    }
    let hash = hash_from_parts(node, mode, &child_hashes, li);
    if tokens >= min_tokens {
        out.push(Subtree {
            hash,
            offset: my_offset,
            tokens,
        });
    }
    (hash, tokens)
}
