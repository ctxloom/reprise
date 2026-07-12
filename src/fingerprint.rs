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

/// Node-hash op counter — the load-immune instrument for the hashing hot path.
///
/// "Instrument first. Understand. Then change." (CLAUDE.md): the near tier's cost is a
/// COUNT of node-hashes, not a wall time, so the count is what we measure. Wall time on
/// this box drifts ±50% batch-to-batch; this counter does not drift at all.
///
/// Compiled ONLY in test builds (`cfg(test)`) and under the opt-in `hash-counter`
/// feature. A production build carries no counter, no atomic, and no branch — the
/// `bump()` call sites vanish entirely.
///
/// Two counters, because the two consumers need different things:
/// - `local()` is a THREAD-LOCAL count, so a unit test can assert an EXACT op count even
///   while other tests run concurrently in the same process. It cannot be polluted.
/// - `global()` is the process-wide sum across rayon's workers — the scan-level figure.
#[cfg(any(test, feature = "hash-counter"))]
pub mod ops {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU64, Ordering};

    static GLOBAL: AtomicU64 = AtomicU64::new(0);
    static AU_NODES: AtomicU64 = AtomicU64::new(0);

    thread_local! {
        static LOCAL: Cell<u64> = const { Cell::new(0) };
    }

    pub(super) fn bump() {
        #[cfg(feature = "hash-counter")]
        GLOBAL.fetch_add(1, Ordering::Relaxed);
        #[cfg(test)]
        LOCAL.with(|c| c.set(c.get() + 1));
    }

    /// Nodes walked by `tree_hashes` — i.e. the near tier's OWN hashing work, summed over
    /// every `anti_unify` call. Separates the near tier's share of `global()` from
    /// extraction's (`subtree_inventory`/`fold`/`normalize` hash the corpus too, and this
    /// patch does not touch them). Also the denominator for the cross-pair-cache question:
    /// this counts a unit's nodes once per PAIR it appears in.
    pub(super) fn bump_au_nodes() {
        #[cfg(feature = "hash-counter")]
        AU_NODES.fetch_add(1, Ordering::Relaxed);
    }

    /// Node-hashes computed by THIS thread so far (exact; immune to other tests).
    pub fn local() -> u64 {
        LOCAL.with(Cell::get)
    }

    /// Node-hashes computed by every thread so far (the scan-wide figure).
    pub fn global() -> u64 {
        GLOBAL.load(Ordering::Relaxed)
    }

    /// Nodes the near tier hashed, scan-wide (2 hashes each: `Exact` + `MaskedAll`).
    pub fn au_nodes() -> u64 {
        AU_NODES.load(Ordering::Relaxed)
    }
}

fn hash_from_parts(
    node: &NormNode,
    mode: HashMode,
    child_hashes: &[u128],
    li: &LabelInterner,
) -> u128 {
    #[cfg(any(test, feature = "hash-counter"))]
    ops::bump();
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

/// Every node's hashes, computed ONCE, bottom-up — the near tier's (`au.rs`) view of an
/// input tree.
///
/// All three vectors are indexed by PRE-ORDER position, and pre-order has the property
/// this type is built on: a node's subtree is a CONTIGUOUS range. Node `i`'s subtree is
/// exactly `i .. i + sizes[i]`, so "the multiset of hashes under node `i`" is a slice —
/// no walk, no re-hashing.
///
/// Why this exists: `au::Ctx::node_info` needs, for many nodes of the two units under
/// comparison, that node's exact hash and the multiset of `MaskedAll` hashes beneath it.
/// It used to get them by calling the fully-recursive `merkle_mode` at every node of the
/// subtree — so each node was re-hashed once per ANCESTOR (O(k * depth)). On the `fs`
/// corpus that was 925.7M node-hashes to serve 12.3M nodes. Hashing each node once and
/// slicing makes it O(k), and is a pure memoization of a deterministic function: the
/// hashes are BIT-IDENTICAL to the recursive ones (each node still hashes exactly
/// `hash_from_parts(node, mode, its children's hashes)` — the same function of the same
/// inputs, just not recomputed). `au::tests` pins that equality node-by-node.
///
/// This is the same children-once-accumulated-upward shape `walk_inventory` below already
/// uses; the two are the file's one hashing pattern, not two.
pub struct TreeHashes {
    /// `HashMode::MaskedAll` hash per node, pre-order.
    pub masked: Vec<u128>,
    /// `HashMode::Exact` hash per node, pre-order.
    pub exact: Vec<u128>,
    /// Subtree node count per node, pre-order — node `i` spans `i .. i + sizes[i]`.
    pub sizes: Vec<u32>,
    /// Node ADDRESS per node, pre-order, so a caller can map a `&NormNode` it holds back
    /// to its index. Sound only while the trees are immovable (see `au::Ctx`).
    pub addrs: Vec<usize>,
}

/// Hash every node of each root in `roots`, bottom-up, in one pass (spec §5.5).
///
/// Roots are laid out consecutively in one set of arrays: each root's pre-order block
/// follows the previous one, so subtree ranges stay contiguous and valid across roots.
pub fn tree_hashes(roots: &[&NormNode], li: &LabelInterner) -> TreeHashes {
    let mut th = TreeHashes {
        masked: Vec::new(),
        exact: Vec::new(),
        sizes: Vec::new(),
        addrs: Vec::new(),
    };
    for root in roots {
        walk_tree_hashes(root, li, &mut th);
    }
    th
}

/// Children first, then the parent from its children's hashes — so each node is hashed
/// exactly once per mode. Returns `(masked, exact, subtree_size)` to its parent.
fn walk_tree_hashes(node: &NormNode, li: &LabelInterner, th: &mut TreeHashes) -> (u128, u128, u32) {
    #[cfg(any(test, feature = "hash-counter"))]
    ops::bump_au_nodes();
    // Claim this node's pre-order slot BEFORE descending, so children land after it.
    let idx = th.masked.len();
    th.masked.push(0);
    th.exact.push(0);
    th.sizes.push(0);
    th.addrs.push(std::ptr::from_ref(node) as usize);

    let mut child_masked = Vec::with_capacity(node.children.len());
    let mut child_exact = Vec::with_capacity(node.children.len());
    let mut size = 1u32;
    for child in &node.children {
        let (m, e, s) = walk_tree_hashes(child, li, th);
        child_masked.push(m);
        child_exact.push(e);
        size += s;
    }

    let masked = hash_from_parts(node, HashMode::MaskedAll, &child_masked, li);
    let exact = hash_from_parts(node, HashMode::Exact, &child_exact, li);
    th.masked[idx] = masked;
    th.exact[idx] = exact;
    th.sizes[idx] = size;
    (masked, exact, size)
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
