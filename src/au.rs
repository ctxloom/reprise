//! P7 verification: anti-unification (spec §5.6).
//!
//! The anti-unifier of two trees is their shared template; the substitutions
//! ARE the divergences, factored out as complete subtrees. Consistent
//! substitution pairs map to the SAME hole (Plotkin/Reynolds lgg), so
//! `f(a, a)` vs `f(b, b)` costs one hole, not two. List-valued nodes align by
//! graded (similarity-scored) Needleman-Wunsch/Smith-Waterman rather than
//! binary LCS: a clone whose every statement was lightly edited still aligns.

use crate::intern::{Field, Kind, LSym, LabelInterner};
use crate::lang::LanguageProfile;
use crate::tree::{Label, NormNode};
use std::borrow::Borrow;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::LazyLock;

/// Shared once-interned `Kind`s for literal comparisons this file makes against
/// grammar/synthetic kind names (interning-id-conversion WP mechanism rule 3: a
/// `LazyLock<Kind>` interns a compile-time literal ONCE, so every subsequent
/// comparison is a bare `u16` equality, never a string compare).
static REPEAT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("REPEAT"));
static HOLE: LazyLock<Kind> = LazyLock::new(|| Kind::intern("HOLE"));
static EXPRESSION_STATEMENT: LazyLock<Kind> =
    LazyLock::new(|| Kind::intern("expression_statement"));
static BLOCK_LOWER: LazyLock<Kind> = LazyLock::new(|| Kind::intern("block"));
static ARGUMENTS: LazyLock<Kind> = LazyLock::new(|| Kind::intern("arguments"));
static ARGUMENT_LIST: LazyLock<Kind> = LazyLock::new(|| Kind::intern("argument_list"));
static PARAMETERS: LazyLock<Kind> = LazyLock::new(|| Kind::intern("parameters"));
static LEFT: LazyLock<Field> = LazyLock::new(|| Field::intern("left"));
static OP: LazyLock<Field> = LazyLock::new(|| Field::intern("op"));
static RIGHT: LazyLock<Field> = LazyLock::new(|| Field::intern("right"));

#[derive(Debug, Clone)]
pub struct Hole {
    pub tokens_a: u32,
    pub tokens_b: u32,
    pub factorable: bool,
    /// False for zero-cost holes: consistent Local↔Local renames (alpha-
    /// renaming discovered post hoc) and synthetic loop-machinery gaps
    /// (`__has_next`/`__next` — canonicalization artifacts, not divergence).
    pub counted: bool,
}

#[derive(Debug, Clone)]
pub struct AuOutcome {
    pub template: NormNode,
    pub template_tokens: u32,
    pub holes: Vec<Hole>,
    pub divergence: f64,
    pub factorable: bool,
}

struct Ctx<'a> {
    /// The historical `LanguageProfile`, consulted ONLY on the historical path (`ir == false`,
    /// the three `!self.ir` predicate branches below). `None` on the IR path — canonical-IR
    /// kinds answer those predicates directly — so the IR path holds no `LanguageProfile` and
    /// its callers pass `None`. Invariant: `profile.is_some() == !ir`.
    profile: Option<&'a dyn LanguageProfile>,
    /// The units under comparison were extracted by the IR normalizer (canonical-IR kinds
    /// `Loop`/`Block`/`Binop`/…), not the historical grammar. The three structural predicates
    /// AU consults — loop core, list kind, binary shape — are answered on IR kinds instead of
    /// the per-grammar `LanguageProfile` (whose kinds never appear in an IR tree). Historical
    /// units keep the exact `LanguageProfile` answers, so their AU output is byte-identical.
    ir: bool,
    /// The scan's per-scan label interner — resolves `Label::External`/`LitKept` `LSym`
    /// ids for fingerprint hashing (`fingerprint::tree_hashes`) and for the two fixed
    /// synthetic-marker comparisons below (`has_next_sym`/`next_sym`).
    li: &'a LabelInterner,
    /// `li.intern("__has_next")`/`li.intern("__next")`, computed ONCE per `anti_unify`
    /// call (not per comparison): `is_synthetic` then compares by bare id equality, never
    /// re-interning or resolving on its (per-DP-cell-reachable) hot path.
    has_next_sym: LSym,
    next_sym: LSym,
    holes: HashMap<(u128, u128), (u32, Hole)>,
    next_hole: u32,
    /// Every node of BOTH input trees, hashed ONCE, bottom-up, before the DP starts
    /// (`fingerprint::tree_hashes`). Pre-order, so a node's subtree is a contiguous
    /// range — `node_info` SLICES it instead of re-walking and re-hashing it.
    ///
    /// This replaces a per-node `merkle`/`collect_hashes` recomputation whose cost was
    /// O(k * depth): `collect_hashes` called the fully-recursive `merkle_mode` at EVERY
    /// node of a subtree, so every subtree was re-hashed once per ANCESTOR. On `fs` that
    /// was 925.7M node-hashes to serve 12.3M nodes (~75x redundant), and `anti_unify` was
    /// ~25% of the entire scan. Hashing bottom-up is a pure memoization — the same
    /// function of the same inputs — so the hashes are BIT-IDENTICAL; `tests` below pins
    /// that node-by-node, and scan output is byte-identical.
    hashes: crate::fingerprint::TreeHashes,
    /// Node ADDRESS -> its pre-order index in `hashes`. Sound ONLY because every node
    /// ever passed to `node_info` is borrowed from one of the two original input trees,
    /// which are immovable for the whole `anti_unify` call — so no address is ever reused
    /// for a different node mid-call. This invariant is load-bearing: `flatten_operands`
    /// therefore collects `&NormNode` INTO the input tree (never clones into a transient
    /// `Vec` that would free — freeing under a stale entry lets the allocator hand the
    /// address to a later node and return the wrong hash/token-count). `tests/au.rs` is
    /// the regression guard for exactly that.
    index: HashMap<usize, u32>,
    /// Memo of the SORTED MaskedAll multiset per node — the form the Jaccard similarity
    /// consumes, and whose len is the D1 token count. Keyed by the same node address.
    /// The hashes themselves are no longer recomputed here (they are sliced from
    /// `hashes`); this now saves only the re-slice and re-sort for a subtree that several
    /// DP CELLS of the similarity matrix ask about.
    memo: HashMap<usize, (u128, Rc<Vec<u128>>)>,
}

impl Ctx<'_> {
    fn node_info(&mut self, node: &NormNode) -> (u128, Rc<Vec<u128>>) {
        let key = std::ptr::from_ref(node) as usize;
        if let Some((exact, vec)) = self.memo.get(&key) {
            return (*exact, Rc::clone(vec));
        }
        let (exact, mut hashes) = match self.index.get(&key) {
            // The node's subtree is the contiguous pre-order range `i .. i + size`.
            Some(&i) => {
                let i = i as usize;
                let end = i + self.hashes.sizes[i] as usize;
                (self.hashes.exact[i], self.hashes.masked[i..end].to_vec())
            }
            None => {
                // Unreachable under the `index` invariant above. Kept as a from-scratch
                // fallback rather than a panic so a future caller that violates it is
                // merely SLOW, never WRONG; `debug_assert` makes it a hard failure in
                // tests, so the violation cannot land silently.
                debug_assert!(
                    false,
                    "node_info on a node outside the two input trees — the address-keyed \
                     index cannot see it (see Ctx::index)"
                );
                let th = crate::fingerprint::tree_hashes(&[node], self.li);
                (th.exact[0], th.masked)
            }
        };
        hashes.sort_unstable();
        let vec = Rc::new(hashes);
        self.memo.insert(key, (exact, Rc::clone(&vec)));
        (exact, vec)
    }

    /// D1 token count via the memo (one MaskedAll hash per node).
    fn tokens(&mut self, node: &NormNode) -> u32 {
        self.node_info(node).1.len() as u32
    }

    fn is_machinery(&mut self, node: &NormNode) -> bool {
        self.tokens(node) <= 12 && is_synthetic(node, self.has_next_sym, self.next_sym)
    }

    /// The historical profile — present whenever `ir == false` (its only consumers are the
    /// `!self.ir` branches below). The invariant `profile.is_some() == !ir` is established at
    /// construction, so reaching this on the IR path would be a caller bug.
    fn hist_profile(&self) -> &dyn LanguageProfile {
        self.profile
            .expect("historical AU (ir == false) requires a LanguageProfile")
    }

    /// The lowered loop core — the IR `Loop` node, or the historical `while True` core.
    fn is_loop_core(&self, node: &NormNode) -> bool {
        if self.ir {
            node.kind == crate::ir::kind::id::LOOP
        } else {
            self.hist_profile().is_loop_core(node)
        }
    }

    /// A variable-length child list AU aligns with graded Smith-Waterman: IR statement `Block`s,
    /// `Branch` arm-lists, and folded `REPEAT` runs (mirrors [`crate::ir::FoldRules`]).
    fn is_list_kind(&self, kind: Kind) -> bool {
        if self.ir {
            kind == crate::ir::kind::id::BLOCK
                || kind == crate::ir::kind::id::BRANCH
                || kind == *REPEAT
        } else {
            self.hist_profile().is_list_kind(kind)
        }
    }

    /// `(left, op, right)` fields of a rebuildable binary kind — the IR `Binop` shape, or the
    /// per-grammar binary kinds.
    #[allow(clippy::type_complexity)]
    fn binary_fields(&self, kind: Kind) -> Option<(Option<Field>, Option<Field>, Option<Field>)> {
        if self.ir {
            (kind == crate::ir::kind::id::BINOP).then_some((Some(*LEFT), Some(*OP), Some(*RIGHT)))
        } else {
            self.hist_profile().binary_fields(kind)
        }
    }
}

/// Anti-unify two normalized units. `profile` is the historical `LanguageProfile`, required
/// ONLY when `ir == false` (the historical path's structural predicates); IR-path callers pass
/// `None` — the canonical-IR kinds answer those predicates directly, so the IR path carries no
/// `LanguageProfile` dependency. Invariant: `profile.is_some() == !ir`. `li` is the scan's
/// per-scan `LabelInterner` (interning-id-conversion WP) — resolves `Label` `LSym` ids for
/// fingerprint hashing and the fixed synthetic-marker comparisons.
pub fn anti_unify(
    a: &NormNode,
    b: &NormNode,
    profile: Option<&dyn LanguageProfile>,
    ir: bool,
    li: &LabelInterner,
) -> AuOutcome {
    // Hash both input trees ONCE, bottom-up, before the DP runs (see `Ctx::hashes`).
    // Every node `node_info` will ever be asked about lives in one of these two trees, so
    // this single O(k) pass replaces the old per-node, per-DP-cell re-hashing.
    let hashes = crate::fingerprint::tree_hashes(&[a, b], li);
    let index: HashMap<usize, u32> = hashes
        .addrs
        .iter()
        .enumerate()
        .map(|(i, &addr)| (addr, i as u32))
        .collect();
    let mut ctx = Ctx {
        profile,
        ir,
        li,
        has_next_sym: li.intern("__has_next"),
        next_sym: li.intern("__next"),
        holes: HashMap::new(),
        next_hole: 0,
        hashes,
        index,
        memo: HashMap::new(),
    };
    let template = au(a, b, &mut ctx);
    let template_tokens = template.token_count();
    let holes: Vec<Hole> = {
        let mut hs: Vec<(u32, Hole)> = ctx.holes.into_values().collect();
        hs.sort_by_key(|(id, _)| *id);
        hs.into_iter().map(|(_, h)| h).collect()
    };
    let sub_tokens: u32 = holes
        .iter()
        .filter(|h| h.counted)
        .map(|h| h.tokens_a + h.tokens_b)
        .sum();
    let divergence = if template_tokens == 0 {
        1.0
    } else {
        f64::from(sub_tokens) / (2.0 * f64::from(template_tokens))
    };
    let factorable = holes.iter().all(|h| h.factorable);
    let holes: Vec<Hole> = holes.into_iter().filter(|h| h.counted).collect();
    AuOutcome {
        template,
        template_tokens,
        holes,
        divergence,
        factorable,
    }
}

fn labels_equal(a: &NormNode, b: &NormNode) -> bool {
    a.label == b.label
}

/// Rust statements wrap in `expression_statement`; comparisons against the
/// loop core must look through the wrapper.
fn unwrap_stmt(node: &NormNode) -> &NormNode {
    if node.kind == *EXPRESSION_STATEMENT && node.children.len() == 1 {
        &node.children[0]
    } else {
        node
    }
}

fn au(a: &NormNode, b: &NormNode, ctx: &mut Ctx) -> NormNode {
    // REPEAT vs rolled loop core: align the template against the loop body —
    // the guard/bind machinery becomes holes (spec §5.3 canonicalization).
    let (ua, ub) = (unwrap_stmt(a), unwrap_stmt(b));
    if ua.kind == *REPEAT
        && ctx.is_loop_core(ub)
        && let Some(body) = crate::lang::child_field(ub, "body")
    {
        let children = au_list(&ua.children, &body.children, ctx);
        return NormNode::with_kind(*REPEAT, None, a.span, children);
    }
    if ub.kind == *REPEAT
        && ctx.is_loop_core(ua)
        && let Some(body) = crate::lang::child_field(ua, "body")
    {
        let children = au_list(&body.children, &ub.children, ctx);
        return NormNode::with_kind(*REPEAT, None, a.span, children);
    }

    if a.kind != b.kind || !labels_equal(a, b) {
        return hole(a, b, ctx);
    }
    // Binary-operator nodes: a differing operator makes the WHOLE expression
    // one factorable hole (an operator-token hole is a partial construct);
    // a matching operator aligns the flattened operand chains, because order
    // canonicalization may have sorted divergent operands differently.
    let is_operator_slot = |n: &NormNode| n.children.is_empty() && n.label.is_none(); // incl. word ops (and/or)
    if ctx.binary_fields(a.kind).is_some()
        && a.children.len() == 3
        && b.children.len() == 3
        && is_operator_slot(&a.children[1])
        && is_operator_slot(&b.children[1])
    {
        if a.children[1].kind != b.children[1].kind {
            return hole(a, b, ctx);
        }
        let mut ops_a: Vec<&NormNode> = Vec::new();
        let mut ops_b: Vec<&NormNode> = Vec::new();
        flatten_operands(a, a.kind, a.children[1].kind, &mut ops_a);
        flatten_operands(b, b.kind, b.children[1].kind, &mut ops_b);
        if ops_a.len() > 2 || ops_b.len() > 2 || ops_a.len() != ops_b.len() {
            let mut children = vec![a.children[1].clone()];
            children.extend(au_list(&ops_a, &ops_b, ctx));
            let mut node = NormNode::with_kind(a.kind, a.field, a.span, children);
            node.label = a.label.clone();
            return node;
        }
    }
    let children = if ctx.is_list_kind(a.kind) || a.children.len() != b.children.len() {
        au_list(&a.children, &b.children, ctx)
    } else {
        a.children
            .iter()
            .zip(&b.children)
            .map(|(ca, cb)| au(ca, cb, ctx))
            .collect()
    };
    let mut node = NormNode::with_kind(a.kind, a.field, a.span, children);
    node.label = a.label.clone();
    node
}

/// Graded alignment of child lists (NW with gap penalty; matched pairs recurse).
///
/// Generic over `Borrow<NormNode>` so the same code aligns owned child slices
/// (`&[NormNode]`) and borrowed operand chains (`&[&NormNode]` from
/// `flatten_operands`) — the borrowed form is mandatory for memo soundness (see
/// `Ctx::memo`); it monomorphizes to the identity borrow for the owned case.
fn au_list<T: Borrow<NormNode>>(xs: &[T], ys: &[T], ctx: &mut Ctx) -> Vec<NormNode> {
    const GAP: f64 = -0.30;
    // A per-pair match-quality bias, NOT a gap-preference threshold: a pair scores
    // `sim + MATCH_BIAS`, a gap-gap trade scores `2*GAP`, so a pair wins whenever
    // `sim > 2*GAP - MATCH_BIAS = -0.25` — i.e. ALWAYS for sim ∈ [0,1]. MATCH_BIAS
    // therefore only breaks ties between near-equal alignments and shrinks the
    // template of a marginal pairing; it never makes a real pair lose to gaps.
    const MATCH_BIAS: f64 = -0.35;
    let n = xs.len();
    let m = ys.len();
    if n * m > 40_000 {
        // Degenerate size: single two-sided hole.
        let ra: Vec<&NormNode> = xs.iter().map(Borrow::borrow).collect();
        let rb: Vec<&NormNode> = ys.iter().map(Borrow::borrow).collect();
        return vec![run_hole(&ra, &rb, ctx)];
    }
    let sims: Vec<Vec<f64>> = xs
        .iter()
        .map(|x| {
            ys.iter()
                .map(|y| similarity(x.borrow(), y.borrow(), ctx))
                .collect()
        })
        .collect();
    let mut dp = vec![vec![0.0f64; m + 1]; n + 1];
    for i in 1..=n {
        dp[i][0] = dp[i - 1][0] + GAP;
    }
    for j in 1..=m {
        dp[0][j] = dp[0][j - 1] + GAP;
    }
    for i in 1..=n {
        for j in 1..=m {
            let diag = dp[i - 1][j - 1] + sims[i - 1][j - 1] + MATCH_BIAS;
            dp[i][j] = diag.max(dp[i - 1][j] + GAP).max(dp[i][j - 1] + GAP);
        }
    }
    // Traceback into (Some(i), Some(j)) / one-sided ops.
    let mut ops: Vec<(Option<usize>, Option<usize>)> = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        if i > 0
            && j > 0
            && (dp[i][j] - (dp[i - 1][j - 1] + sims[i - 1][j - 1] + MATCH_BIAS)).abs() < 1e-9
        {
            ops.push((Some(i - 1), Some(j - 1)));
            i -= 1;
            j -= 1;
        } else if i > 0 && (dp[i][j] - (dp[i - 1][j] + GAP)).abs() < 1e-9 {
            ops.push((Some(i - 1), None));
            i -= 1;
        } else {
            ops.push((None, Some(j - 1)));
            j -= 1;
        }
    }
    ops.reverse();

    // Merge consecutive one-sided runs into single list-holes.
    fn flush<'a>(
        out: &mut Vec<NormNode>,
        gap_a: &mut Vec<&'a NormNode>,
        gap_b: &mut Vec<&'a NormNode>,
        ctx: &mut Ctx,
    ) {
        if !gap_a.is_empty() || !gap_b.is_empty() {
            out.push(run_hole(gap_a, gap_b, ctx));
            gap_a.clear();
            gap_b.clear();
        }
    }
    let mut out = Vec::new();
    let mut gap_a: Vec<&NormNode> = Vec::new();
    let mut gap_b: Vec<&NormNode> = Vec::new();
    for (oi, oj) in ops {
        match (oi, oj) {
            (Some(x), Some(y)) => {
                flush(&mut out, &mut gap_a, &mut gap_b, ctx);
                out.push(au(xs[x].borrow(), ys[y].borrow(), ctx));
            }
            (Some(x), None) => gap_a.push(xs[x].borrow()),
            (None, Some(y)) => gap_b.push(ys[y].borrow()),
            (None, None) => unreachable!(),
        }
    }
    flush(&mut out, &mut gap_a, &mut gap_b, ctx);
    out
}

/// Subtree similarity in [0,1]: exact-equal → 1; else Jaccard over all
/// MaskedAll subtree hashes (the graded S-W substitution cost, spec §5.6).
/// Hashes come from the per-call memo (`Ctx::node_info`) — computed once per
/// node, not once per DP cell.
fn similarity(a: &NormNode, b: &NormNode, ctx: &mut Ctx) -> f64 {
    // Synthetic lowering machinery (the small guard/bind statements) vs real code:
    // zero the pair similarity to DISCOURAGE pairing them. This is a soft tie-break,
    // NOT an enforced gap — with the NW constants in `au_list` a pair still beats a
    // gap-gap trade for any sim ≥ 0 (a pair scores sim+MATCH_BIAS, gap-gap scores
    // 2*GAP, and 2*GAP−MATCH_BIAS = −0.25 < 0), so equal-length lists still ALIGN the
    // machinery against real code positionally. The rule is fully effective only where
    // list lengths already force the machinery into a ONE-SIDED, all-synthetic gap run,
    // which `run_hole` leaves uncounted (its `synthetic` branch); zeroing sim only
    // nudges the DP toward that gap when it is otherwise a close call. (A whole loop
    // CONTAINS the markers but is not itself machinery, so it still pairs, e.g. with
    // REPEAT.) Making the gap unconditionally achievable — a negative sentinel here, so
    // gap-gap beats pairing — was tried and produced NO mutation-recall or full-suite
    // change (the recall benches sit at a 100% ceiling), so enforcing it is a DEFERRED
    // behavioral question: it needs its own recall+precision study on real corpora.
    if ctx.is_machinery(a) != ctx.is_machinery(b) {
        return 0.0;
    }
    if a.kind != b.kind {
        // Folded REPEAT and the rolled loop core must PAIR in alignment so the
        // au() special case can compare template against loop body (§5.3).
        let (ua, ub) = (unwrap_stmt(a), unwrap_stmt(b));
        let repeat_loop = (ua.kind == *REPEAT && ctx.is_loop_core(ub))
            || (ub.kind == *REPEAT && ctx.is_loop_core(ua));
        return if repeat_loop { 0.6 } else { 0.0 };
    }
    let (ea, ha) = ctx.node_info(a);
    let (eb, hb) = ctx.node_info(b);
    if ea == eb {
        return 1.0;
    }
    let (mut i, mut j, mut shared) = (0usize, 0usize, 0usize);
    while i < ha.len() && j < hb.len() {
        match ha[i].cmp(&hb[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                shared += 1;
                i += 1;
                j += 1;
            }
        }
    }
    let union = ha.len() + hb.len() - shared;
    if union == 0 {
        1.0
    } else {
        shared as f64 / union as f64
    }
}

/// Collect operands of a same-kind, same-operator chain (mirrors the
/// normalize-pass flatten; template-only shape, rebuilding is not needed).
///
/// Collects BORROWS into the input tree, never clones: every operand handed on
/// to `au_list`/`node_info` must outlive the whole `anti_unify` call so the
/// address-keyed memo stays sound (see `Ctx::memo`). Cloning into a transient
/// `Vec` — which then frees — is a use-after-free of the memo key.
fn flatten_operands<'a>(node: &'a NormNode, kind: Kind, op: Kind, out: &mut Vec<&'a NormNode>) {
    if node.kind == kind && node.children.len() == 3 && node.children[1].kind == op {
        flatten_operands(&node.children[0], kind, op, out);
        out.push(&node.children[2]);
    } else {
        out.push(node);
    }
}

/// See `Ctx::is_machinery`: a small statement that IS lowering machinery
/// (guard `if !__has_next(..)` / bind `x = __next(..)`), as opposed to real
/// code containing a lowered loop. `has_next`/`next` are `li.intern(..)`'d ONCE
/// per `anti_unify` call (`Ctx::has_next_sym`/`next_sym`) — this compares by bare
/// id equality, never re-interning or resolving on this (DP-cell-reachable) path.
fn is_synthetic(node: &NormNode, has_next: LSym, next: LSym) -> bool {
    if matches!(&node.label, Some(Label::External(s)) if *s == has_next || *s == next) {
        return true;
    }
    node.children
        .iter()
        .any(|c| is_synthetic(c, has_next, next))
}

fn is_op_token(node: &NormNode) -> bool {
    node.children.is_empty()
        && node.label.is_none()
        && !node.kind.as_str().chars().any(char::is_alphanumeric)
}

fn hole(a: &NormNode, b: &NormNode, ctx: &mut Ctx) -> NormNode {
    let key = (ctx.node_info(a).0, ctx.node_info(b).0);
    let id = match ctx.holes.get(&key) {
        Some((id, _)) => *id,
        None => {
            let id = ctx.next_hole;
            ctx.next_hole += 1;
            // Holes at bare operator tokens capture partial constructs (§5.6).
            let factorable = !(is_op_token(a) || is_op_token(b));
            // Consistent Local↔Local leaf pairs are alpha-renaming, not drift.
            let rename_only = a.children.is_empty()
                && b.children.is_empty()
                && matches!(&a.label, Some(Label::Local(_)))
                && matches!(&b.label, Some(Label::Local(_)));
            let (tokens_a, tokens_b) = (ctx.tokens(a), ctx.tokens(b));
            ctx.holes.insert(
                key,
                (
                    id,
                    Hole {
                        tokens_a,
                        tokens_b,
                        factorable,
                        counted: !rename_only,
                    },
                ),
            );
            id
        }
    };
    NormNode::with_kind(*HOLE, a.field, a.span, Vec::new()).with_label(Label::Local(id))
}

/// One-sided (or two-sided) gap run → a single hole.
fn run_hole(gap_a: &[&NormNode], gap_b: &[&NormNode], ctx: &mut Ctx) -> NormNode {
    let mut hash_side = |items: &[&NormNode]| -> u128 {
        let mut buf = Vec::new();
        for it in items {
            buf.extend_from_slice(&ctx.node_info(it).0.to_le_bytes());
        }
        xxhash_rust::xxh3::xxh3_128(&buf)
    };
    let key = (hash_side(gap_a), hash_side(gap_b));
    let span = gap_a
        .first()
        .or_else(|| gap_b.first())
        .map(|n| n.span)
        .unwrap_or((0, 0));
    let id = match ctx.holes.get(&key) {
        Some((id, _)) => *id,
        None => {
            let id = ctx.next_hole;
            ctx.next_hole += 1;
            // One-sided gaps made only of synthetic lowering machinery
            // (guard/bind around __has_next/__next) are canonicalization
            // artifacts of the REPEAT↔loop comparison, not real divergence.
            let synthetic = (gap_a.is_empty() && gap_b.iter().all(|n| ctx.is_machinery(n)))
                || (gap_b.is_empty() && gap_a.iter().all(|n| ctx.is_machinery(n)));
            let tokens_a = gap_a.iter().map(|n| ctx.tokens(n)).sum::<u32>();
            let tokens_b = gap_b.iter().map(|n| ctx.tokens(n)).sum::<u32>();
            ctx.holes.insert(
                key,
                (
                    id,
                    Hole {
                        tokens_a,
                        tokens_b,
                        // Gap runs are whole child-list elements: statements or
                        // list items — mechanically consolidable.
                        factorable: true,
                        counted: !synthetic,
                    },
                ),
            );
            id
        }
    };
    NormNode::with_kind(*HOLE, None, span, Vec::new()).with_label(Label::Local(id))
}

// ---------- template rendering (pseudo-source, spec §6) ----------

/// Compact pseudo-source rendering of a template. Not a pretty-printer:
/// structure + labels + ⟨hole⟩ markers, readable enough to judge a finding. `li`
/// resolves `Label::External`/`LitKept` ids (test/report rendering only — never
/// on a matching comparison path).
pub fn render_template(node: &NormNode, li: &LabelInterner) -> String {
    let mut out = String::new();
    render(node, 0, &mut out, li);
    out
}

fn render(node: &NormNode, indent: usize, out: &mut String, li: &LabelInterner) {
    if node.kind == *HOLE {
        let id = match &node.label {
            Some(Label::Local(i)) => *i + 1,
            _ => 0,
        };
        out.push_str(&format!("⟨h{id}⟩"));
        return;
    }
    if node.kind == *REPEAT {
        out.push_str("repeat× {");
        for child in &node.children {
            newline(indent + 1, out);
            render(child, indent + 1, out, li);
        }
        newline(indent, out);
        out.push('}');
        return;
    }
    if node.kind == *BLOCK_LOWER {
        out.push('{');
        for child in &node.children {
            newline(indent + 1, out);
            render(child, indent + 1, out, li);
        }
        newline(indent, out);
        out.push('}');
        return;
    }
    if let Some(label) = &node.label {
        let text = match label {
            Label::External(s) => li.resolve(*s).to_string(),
            Label::Local(i) => format!("v{i}"),
            Label::LitKept(s) => li.resolve(*s).to_string(),
            Label::LitBucket(b) => b.name().to_string(),
            Label::Raw(s) | Label::RawLit(s) => s.to_string(),
        };
        out.push_str(&text);
        return;
    }
    if node.children.is_empty() {
        out.push_str(node.kind.as_str());
        return;
    }
    if let Some(prefix) = kind_keyword(node.kind) {
        out.push_str(prefix);
        out.push(' ');
    }
    let parenthesized =
        node.kind == *ARGUMENTS || node.kind == *ARGUMENT_LIST || node.kind == *PARAMETERS;
    if parenthesized {
        out.push('(');
    }
    let mut first = true;
    for child in &node.children {
        if !first {
            out.push_str(if parenthesized { ", " } else { " " });
        }
        first = false;
        render(child, indent, out, li);
    }
    if parenthesized {
        out.push(')');
    }
}

fn newline(indent: usize, out: &mut String) {
    out.push('\n');
    for _ in 0..indent {
        out.push_str("  ");
    }
}

fn kind_keyword(kind: Kind) -> Option<&'static str> {
    static KEYWORDS: LazyLock<HashMap<Kind, &'static str>> = LazyLock::new(|| {
        [
            ("if_statement", "if"),
            ("if_expression", "if"),
            ("while_statement", "while"),
            ("loop_expression", "loop"),
            ("return_statement", "return"),
            ("return_expression", "return"),
            ("break_statement", "break"),
            ("break_expression", "break"),
            ("continue_statement", "continue"),
            ("continue_expression", "continue"),
            ("else_clause", "else"),
            ("elif_clause", "elif"),
            ("match_expression", "match"),
            ("try_statement", "try"),
            ("not_operator", "not"),
            ("let_declaration", "let"),
            ("function_item", "fn"),
            ("function_definition", "fn"),
        ]
        .into_iter()
        .map(|(k, v)| (Kind::intern(k), v))
        .collect()
    });
    KEYWORDS.get(&kind).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Normalizer};
    use crate::fingerprint::{HashMode, merkle_mode, ops};
    use crate::lang::Lang;

    /// A tree deep enough that the OLD per-node `merkle_mode` walk pays the depth
    /// factor visibly: each nested level re-hashes everything below it.
    const DEEP_A: &str = "\
def g(a, b, c, d):
    if a > b:
        for i in range(c):
            while d > 0:
                r = a + b * c - d
                s = (a + b) * (c - d)
                d = d - r + s
    return d
";
    const DEEP_B: &str = "\
def h(a, b, c, d):
    if a > b:
        for i in range(c):
            while d > 0:
                r = a + b * c - d
                s = (a - b) * (c + d)
                d = d + r - s
    return d
";

    fn nodes(n: &NormNode) -> u64 {
        1 + n.children.iter().map(nodes).sum::<u64>()
    }

    struct Fixture {
        li: std::sync::Arc<crate::intern::LabelInterner>,
        ua: crate::unit::Unit,
        ub: crate::unit::Unit,
        ir: bool,
    }

    fn fixture() -> Fixture {
        let cfg = Config::default();
        let li = crate::intern::LabelInterner::new();
        let ua =
            crate::unit::units_from_source_with_interner(DEEP_A, Lang::Python, &cfg, &li).remove(0);
        let ub =
            crate::unit::units_from_source_with_interner(DEEP_B, Lang::Python, &cfg, &li).remove(0);
        let ir = cfg.normalize.normalizer == Normalizer::Ir
            && crate::frontend::has_ir_frontend(Lang::Python);
        Fixture { li, ua, ub, ir }
    }

    /// THE O(k) PROPERTY — the defect this test exists to pin.
    ///
    /// `anti_unify` must hash each node of its two input trees a BOUNDED number of
    /// times — once per hash mode (`Exact` + `MaskedAll`), from one bottom-up pass —
    /// NOT once per ancestor. The old `collect_hashes` called the fully-recursive
    /// `merkle_mode` at EVERY node of the subtree, so a node was re-hashed once for
    /// each of its ancestors: O(k * depth), which on `fs` meant 925.7M node-hashes to
    /// serve 12.3M nodes (~75x redundant).
    ///
    /// Asserted on a COUNTER, never on wall time: wall drifts ±50% batch-to-batch on
    /// this box, the op count does not drift at all.
    #[test]
    fn anti_unify_hashes_each_node_once_per_mode() {
        let f = fixture();
        let (ta, tb) = (f.ua.tree.expect_resident(), f.ub.tree.expect_resident());
        let profile = (!f.ir).then(|| Lang::Python.profile());
        let k = nodes(ta) + nodes(tb);

        // Snapshot AFTER extraction (which hashes too) so we count only anti_unify.
        let before = ops::local();
        let _ = anti_unify(ta, tb, profile, f.ir, &f.li);
        let used = ops::local() - before;

        // Exactly two hashes per node: one `Exact`, one `MaskedAll`, in a single
        // bottom-up pass over each input tree.
        assert_eq!(
            used,
            2 * k,
            "anti_unify computed {used} node-hashes for {k} nodes ({:.1}x per node); \
             a node must be hashed ONCE PER MODE (2 * {k} = {}). More than that means \
             a subtree is being re-hashed once per ancestor (the O(k*depth) defect).",
            used as f64 / k as f64,
            2 * k,
        );
    }

    /// The memoized bottom-up hashes must be BIT-IDENTICAL to the unmemoized
    /// recursive `merkle_mode` — for EVERY node, in BOTH modes. This is the whole
    /// safety claim: the change is a pure memoization of a deterministic function, so
    /// if any hash moves, the memo is wrong. (Byte-identity of scan output is the
    /// outer gate; this is the same invariant pinned at the unit.)
    #[test]
    fn bottom_up_hashes_equal_unmemoized_merkle_for_every_node() {
        let f = fixture();

        for root in [f.ua.tree.expect_resident(), f.ub.tree.expect_resident()] {
            let th = crate::fingerprint::tree_hashes(&[root], &f.li);
            assert_eq!(th.masked.len() as u64, nodes(root), "one slot per node");

            // Walk the tree in the SAME pre-order the pass uses, and check each node
            // against a from-scratch recursive hash of that node.
            fn check(
                node: &NormNode,
                i: &mut usize,
                th: &crate::fingerprint::TreeHashes,
                li: &crate::intern::LabelInterner,
            ) {
                let idx = *i;
                *i += 1;
                assert_eq!(
                    th.masked[idx],
                    merkle_mode(node, HashMode::MaskedAll, li),
                    "MaskedAll hash diverged at pre-order {idx} (kind {})",
                    node.kind.as_str(),
                );
                assert_eq!(
                    th.exact[idx],
                    merkle_mode(node, HashMode::Exact, li),
                    "Exact hash diverged at pre-order {idx} (kind {})",
                    node.kind.as_str(),
                );
                for c in &node.children {
                    check(c, i, th, li);
                }
                // The subtree range must be exactly this node's subtree.
                assert_eq!(
                    th.sizes[idx] as usize,
                    *i - idx,
                    "subtree size wrong at pre-order {idx}",
                );
            }
            check(root, &mut 0, &th, &f.li);
        }
    }
}
