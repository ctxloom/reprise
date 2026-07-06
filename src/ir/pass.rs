//! Language-agnostic IR passes (`docs/SIMILARITY-IR.md` §6): the algorithms that
//! were copied per profile now run **once** on the canonical IR. Increment 4:
//! identifier abstraction. Declared locals are structural — a `Var` in a
//! binding-field position (`param`/`target`) — so there is no per-language
//! `collect_declared` hook; the pass is fully language-agnostic.

use crate::ir::edit::Edit;
use crate::ir::kind;
use crate::ir::transform::{TransformKind, TransformLog, Witness};
use crate::tree::{Label, NormNode};
use std::collections::{HashMap, HashSet};

/// Fields whose `Var` occupant is a binding site (a declared local). Grows as the
/// IR grows (match-arm binds, for-pattern binds) — still structural, never per-grammar.
fn is_bind_field(field: Option<&str>) -> bool {
    matches!(field, Some("param") | Some("target"))
}

/// Spec §5.2.4 identifier abstraction as a detect/emit pass (D-IR-12): declared locals
/// become positional `Local(n)` by first occurrence; every other name becomes `External`.
/// Whole-tree and positional (the D2 cascade), so it is exactly ONE
/// [`AbstractIdents`](Edit::AbstractIdents) edit carrying the local map, from which
/// [`crate::ir::edit::apply`] relabels deterministically — an immutable tree in, so the
/// abstraction cannot happen without its event. Binding position is a field in the
/// canonical IR, so this is one algorithm for all languages — the per-profile
/// `collect_declared`/`always_external` split dissolves.
pub fn detect_abstract_idents(root: &NormNode) -> Vec<Edit> {
    if !contains_raw_var(root) {
        return Vec::new(); // nothing to relabel — no edit, tree unchanged
    }
    let mut declared = HashSet::new();
    collect_declared(root, &mut declared);
    // Compute the first-occurrence local order via the historical relabel walk on a
    // throwaway clone — the same traversal the applier uses, so the indices agree.
    let mut order: HashMap<Box<str>, u32> = HashMap::new();
    relabel(&mut root.clone(), &declared, &mut order);
    let mut map: Vec<(u32, Box<str>)> = order.into_iter().map(|(name, n)| (n, name)).collect();
    map.sort_unstable_by_key(|&(n, _)| n);
    vec![Edit::AbstractIdents { map }]
}

/// Convenience over the seam for callers that only want the abstracted tree (tests): a
/// disabled sink discards the (hash-excluded) event, so the tree is byte-identical to the
/// recording path. Not itself a mutator — it routes through [`crate::ir::edit::apply`].
pub fn abstract_idents(root: NormNode) -> NormNode {
    let edits = detect_abstract_idents(&root);
    crate::ir::edit::apply(root, &edits, &mut TransformLog::disabled())
}

/// Whether any `Raw` `Var` remains — i.e. relabelling would change the tree.
fn contains_raw_var(node: &NormNode) -> bool {
    (node.kind.as_ref() == kind::VAR && matches!(node.label, Some(Label::Raw(_))))
        || node.children.iter().any(contains_raw_var)
}

fn collect_declared(node: &NormNode, out: &mut HashSet<Box<str>>) {
    if node.kind.as_ref() == kind::VAR
        && is_bind_field(node.field.as_deref())
        && let Some(Label::Raw(t)) = &node.label
    {
        out.insert(t.clone());
    }
    for c in &node.children {
        collect_declared(c, out);
    }
}

fn relabel(node: &mut NormNode, declared: &HashSet<Box<str>>, order: &mut HashMap<Box<str>, u32>) {
    if node.kind.as_ref() == kind::VAR
        && let Some(Label::Raw(t)) = node.label.clone()
    {
        node.label = Some(if declared.contains(&t) {
            let next = order.len() as u32;
            Label::Local(*order.entry(t).or_insert(next))
        } else {
            Label::External(t)
        });
    }
    for c in &mut node.children {
        relabel(c, declared, order);
    }
}

/// The shared relabel [`crate::ir::edit::apply`] performs for an
/// [`AbstractIdents`](Edit::AbstractIdents): a `Raw` `Var` named in `locals` becomes its
/// positional `Local`, every other `Raw` `Var` becomes `External`. Reproduces [`relabel`]
/// from the precomputed map (same indices), so the two paths agree byte-for-byte.
pub(crate) fn relabel_from_map(node: &mut NormNode, locals: &HashMap<&str, u32>) {
    if node.kind.as_ref() == kind::VAR
        && let Some(Label::Raw(t)) = node.label.clone()
    {
        node.label = Some(match locals.get(t.as_ref()) {
            Some(&n) => Label::Local(n),
            None => Label::External(t),
        });
    }
    for c in &mut node.children {
        relabel_from_map(c, locals);
    }
}

/// Commutative operators (canonical IR spelling — both `&&` and Python `and`, etc.).
const COMMUTATIVE: &[&str] = &["+", "*", "&", "|", "^", "==", "!=", "&&", "||", "and", "or"];

/// A 3-child `Binop` on a commutative operator — the shape [`detect_comm_sort`]
/// canonicalizes and [`crate::ir::edit::apply`] reorders.
pub(crate) fn is_commutative_chain(node: &NormNode) -> bool {
    node.kind.as_ref() == kind::BINOP
        && node.children.len() == 3
        && COMMUTATIVE.contains(&node.children[1].kind.as_ref())
}

/// Spec §5.2.6 on the IR: describe the canonical sort of every commutative operator
/// chain (by field-stripped s-expression) so `a + b` and `b + a` converge. Detect-only
/// (D-IR-12): an immutable tree in, a [`CommSort`](Edit::CommSort) edit per chain out —
/// a pass cannot reorder without [`crate::ir::edit::apply`] recording the event, and
/// each edit's permutation records the ORIGINAL operand order so the sort is reversible
/// (D-IR-9). Runs *after* abstraction, so the sort key is the abstracted-label
/// s-expression the pipeline canonicalizes at this point. Language-agnostic — the
/// operator token is canonical in the IR. Unsound for floats / operator overloading
/// (accepted; the output is a report).
pub fn detect_comm_sort(node: &NormNode) -> Vec<Edit> {
    let mut edits = Vec::new();
    comm_sort_edits(node, &mut edits);
    edits
}

/// Bottom-up companion to [`detect_comm_sort`]: canonicalize a chain's operands before
/// keying the enclosing chain off their s-expressions (a nested chain must settle first
/// — mirrors the historical pass's post-order recursion), emit one edit per chain, and
/// return the canonicalized subtree so a caller can compute its own sort key.
fn comm_sort_edits(node: &NormNode, out: &mut Vec<Edit>) -> NormNode {
    if is_commutative_chain(node) {
        let op = node.children[1].clone();
        let mut operands = Vec::new();
        flatten_chain(node, op.kind.as_ref(), &mut operands);
        let operands: Vec<NormNode> = operands.iter().map(|o| comm_sort_edits(o, out)).collect();
        if operands.len() >= 2 {
            let order = sort_perm(&operands);
            let sorted = order
                .iter()
                .map(|&i| operands[i as usize].clone())
                .collect();
            out.push(Edit::CommSort {
                locus: node.span,
                order,
            });
            return rebuild_chain(sorted, op, node.field.clone());
        }
    }
    let mut n = node.clone();
    n.children = node
        .children
        .iter()
        .map(|c| comm_sort_edits(c, out))
        .collect();
    n
}

/// The stable permutation that sorts `operands` by field-stripped s-expression:
/// `perm[k]` is the pre-sort index of the operand now at position `k`. Stable (ties
/// keep source order), matching the historical `sort_by_cached_key`; recorded as the
/// witness so reversal restores the original order (D-IR-9).
fn sort_perm(operands: &[NormNode]) -> Vec<u32> {
    let keys: Vec<String> = operands.iter().map(crate::ir::render::to_sexpr).collect();
    let mut perm: Vec<u32> = (0..operands.len() as u32).collect();
    perm.sort_by(|&a, &b| keys[a as usize].cmp(&keys[b as usize]));
    perm
}

/// Collect operands of a same-operator commutative chain (left-assoc), field-stripped.
pub(crate) fn flatten_chain(node: &NormNode, op: &str, out: &mut Vec<NormNode>) {
    if node.kind.as_ref() == kind::BINOP
        && node.children.len() == 3
        && node.children[1].kind.as_ref() == op
    {
        flatten_chain(&node.children[0], op, out);
        let mut rhs = node.children[2].clone();
        rhs.field = None;
        out.push(rhs);
    } else {
        let mut n = node.clone();
        n.field = None;
        out.push(n);
    }
}

/// Rebuild a left-associative commutative chain over `operands` — the shared mutation
/// [`crate::ir::edit::apply`] performs for a `CommSort`.
pub(crate) fn rebuild_chain(
    mut operands: Vec<NormNode>,
    op: NormNode,
    field: Option<Box<str>>,
) -> NormNode {
    let span = operands[0].span;
    let mut acc = operands.remove(0);
    acc.field = Some("left".into());
    for mut next in operands {
        next.field = Some("right".into());
        let mut op_clone = op.clone();
        op_clone.field = Some("op".into());
        acc = NormNode::new(kind::BINOP, None, span, vec![acc, op_clone, next]);
        acc.children[0].field = Some("left".into());
    }
    acc.field = field;
    acc
}

// ---- Family A: relational/boolean canonicalization (§4.2, D-IR-13) ----

/// One boolean-normalization step: `(kind, locus, witness)` in application order — the
/// payload both [`detect_boolean_normalize`] turns into an [`Edit`] and
/// [`crate::ir::edit::apply`] replays while recording.
pub(crate) type BoolStep = (TransformKind, (u32, u32), Witness);

/// Family A (D-IR-13) as a detect/emit pass (D-IR-12): canonicalize relational/boolean
/// algebra so equivalent spellings converge. A1 orients every ordered comparison to the
/// canonical `<`/`<=` (`a > b` ≡ `b < a`) — the ordered-comparison analog of the commutative
/// sort that already handles the symmetric `==`/`!=`. A2 pushes a logical negation inward:
/// comparison inversion (`!(a<b)` → `a>=b`), De Morgan (`!(a&&b)` → `!a||!b`), and
/// double-negation (`!!x` → `x`) — the rewrite that makes the `break_guard`-synthesized
/// `!(i<n)` converge with a hand-rolled `loop { if i>=n { break } … }`. Detect-only: an
/// immutable tree in, one [`CmpOrient`](Edit::CmpOrient) / [`NotPush`](Edit::NotPush) edit per
/// reduction out, so a pass cannot rewrite without [`crate::ir::edit::apply`] recording the
/// event. Runs *after* `abstract_idents` (label-independent, so comm-sort's abstracted sort key
/// is unchanged) and *before* `detect_comm_sort` (A1 settles asymmetric operands before the
/// symmetric ones sort, and A2's De Morgan makes the `&&`/`||` chains comm-sort then sorts).
/// Unsound only under float/NaN (`!(a<b)` inversion) or `<`/`>`/`!` operator overloading — the
/// accepted commutative-sort soundness class (the output is a report).
pub fn detect_boolean_normalize(root: &NormNode) -> Vec<Edit> {
    let mut steps = Vec::new();
    let _ = bool_normalize(root.clone(), &mut steps);
    steps.into_iter().map(step_to_edit).collect()
}

fn step_to_edit((kind, locus, _witness): BoolStep) -> Edit {
    match kind {
        TransformKind::CmpOrient => Edit::CmpOrient { locus },
        TransformKind::NotPush => Edit::NotPush { locus },
        _ => unreachable!("bool_normalize emits only CmpOrient / NotPush"),
    }
}

/// Convenience over the seam (tests): disabled sink → identical tree. See [`abstract_idents`].
pub fn boolean_normalize(node: NormNode) -> NormNode {
    let edits = detect_boolean_normalize(&node);
    crate::ir::edit::apply(node, &edits, &mut TransformLog::disabled())
}

/// The shared boolean-normalization core [`crate::ir::edit::apply`] replays for the
/// [`CmpOrient`](Edit::CmpOrient) edits: fully normalize `node` bottom-up (children first,
/// then reduce the node to a fixpoint), appending each reduction step to `steps`. Pure and
/// deterministic, so the detector (which turns `steps` into edits) and the applier (which
/// replays and records) stay in lockstep by construction.
pub(crate) fn bool_normalize(node: NormNode, steps: &mut Vec<BoolStep>) -> NormNode {
    let mut node = node;
    node.children = node
        .children
        .into_iter()
        .map(|c| bool_normalize(c, steps))
        .collect();
    reduce_top(node, steps)
}

/// Reduce a single node whose children are already normalized, to a boolean-normal fixpoint.
/// Recurses on its own output so a rewrite that exposes a further one settles here (A2's
/// comparison-inversion yields a `>=` that A1 must then orient — the plan's fixpoint, so
/// `!(i<n)` and a hand-rolled `if i>=n` reach the SAME `n<=i` spelling).
fn reduce_top(node: NormNode, steps: &mut Vec<BoolStep>) -> NormNode {
    // A2 · push a logical negation inward (comparison inversion / De Morgan / double-neg).
    if let Some(red) = not_operand(&node).and_then(classify_not) {
        return push_not(node, red, steps);
    }
    // A1 · orient an ordered comparison to `<`/`<=` (flip operands + operator together).
    if let Some(flipped) = binop_op(&node).and_then(orient_flip) {
        let locus = node.span;
        let node = swap_binop(node, flipped);
        steps.push((TransformKind::CmpOrient, locus, Witness::Order(vec![1, 0])));
        return reduce_top(node, steps);
    }
    node
}

/// The reduction A2 applies to a `Unop{ !, x }`, decided from `x`'s already-normalized shape.
enum NotReduction {
    /// `x` is a comparison — invert the operator (operands unchanged).
    Invert(&'static str),
    /// `x` is a `&&`/`||` chain — De Morgan (flip the operator, negate both operands).
    DeMorgan(&'static str),
    /// `x` is itself `!y` — collapse the double negation to `y`.
    DoubleNeg,
}

/// Classify how (if at all) `operand` — the `x` of a `Unop{ !, x }` — reduces under A2.
fn classify_not(operand: &NormNode) -> Option<NotReduction> {
    if let Some((_, op, _)) = binop_parts(operand) {
        if let Some(inv) = invert_cmp(op) {
            return Some(NotReduction::Invert(inv));
        }
        if let Some(flip) = demorgan_flip(op) {
            return Some(NotReduction::DeMorgan(flip));
        }
    }
    not_operand(operand)
        .is_some()
        .then_some(NotReduction::DoubleNeg)
}

/// Perform an A2 negation-push on `node` (a `Unop{ !, x }`), recording the `NotPush` step,
/// then re-reduce so the exposed form settles (an inverted `>` orients; a De-Morgan'd operand
/// negation pushes further). The single `NotPush` event stands for the whole outer push;
/// child negations record their own events as they recurse.
fn push_not(node: NormNode, red: NotReduction, steps: &mut Vec<BoolStep>) -> NormNode {
    let locus = node.span;
    let field = node.field.clone();
    steps.push((TransformKind::NotPush, locus, Witness::None));
    let inner = take_operand(node); // the operand `x`, dropping the `!`
    let reduced = match red {
        NotReduction::Invert(inv) => {
            let mut b = set_binop_op(inner, inv);
            b.field = field;
            b
        }
        NotReduction::DeMorgan(flip) => {
            let span = inner.span;
            let mut it = inner.children.into_iter();
            let l = it.next().expect("binop left");
            let op = it.next().expect("binop op");
            let r = it.next().expect("binop right");
            let op = NormNode::new(flip, Some("op"), op.span, Vec::new());
            // Negate each operand and push inward (De Morgan recurses on both sides).
            let nl = reduce_top(negate(l), steps);
            let nr = reduce_top(negate(r), steps);
            let mut b = NormNode::new(kind::BINOP, None, span, vec![nl, op, nr]);
            b.field = field;
            b
        }
        NotReduction::DoubleNeg => {
            let mut y = take_operand(inner); // `inner` is `Unop{ !, y }`
            y.field = field;
            y
        }
    };
    reduce_top(reduced, steps)
}

/// The operand of a logical negation `Unop{ !, x }` (the `@operand` child), else `None`.
fn not_operand(node: &NormNode) -> Option<&NormNode> {
    if node.kind.as_ref() != kind::UNOP {
        return None;
    }
    let is_not = node
        .children
        .iter()
        .any(|c| c.field.as_deref() == Some("op") && c.kind.as_ref() == "!");
    is_not
        .then(|| {
            node.children
                .iter()
                .find(|c| c.field.as_deref() == Some("operand"))
        })
        .flatten()
}

/// Remove and return the `@operand` child of a `Unop` (owned).
fn take_operand(mut node: NormNode) -> NormNode {
    let idx = node
        .children
        .iter()
        .position(|c| c.field.as_deref() == Some("operand"))
        .expect("unop operand");
    node.children.remove(idx)
}

/// Wrap `node` in a logical negation `Unop{ !, node }`, hoisting `node`'s field onto the Unop
/// so the wrapper sits in the same operand position (`@left`/`@right`).
fn negate(mut node: NormNode) -> NormNode {
    let span = node.span;
    let field = node.field.take();
    node.field = Some("operand".into());
    let bang = NormNode::new("!", Some("op"), span, Vec::new());
    let mut u = NormNode::new(kind::UNOP, None, span, vec![bang, node]);
    u.field = field;
    u
}

/// `(left, operator-token, right)` of a 3-child `Binop`, else `None`.
fn binop_parts(node: &NormNode) -> Option<(&NormNode, &str, &NormNode)> {
    (node.kind.as_ref() == kind::BINOP && node.children.len() == 3).then(|| {
        (
            &node.children[0],
            node.children[1].kind.as_ref(),
            &node.children[2],
        )
    })
}

/// Replace a `Binop`'s operator token in place (operands + their fields unchanged) — the
/// shared mutation for A2's comparison inversion (`!(a<b)` keeps `a`,`b`, flips `<`→`>=`).
fn set_binop_op(mut node: NormNode, new_op: &str) -> NormNode {
    let op_span = node.children[1].span;
    node.children[1] = NormNode::new(new_op, Some("op"), op_span, Vec::new());
    node
}

/// The operator token of a 3-child `Binop` (`children[1]`), else `None`.
fn binop_op(node: &NormNode) -> Option<&str> {
    (node.kind.as_ref() == kind::BINOP && node.children.len() == 3)
        .then(|| node.children[1].kind.as_ref())
}

/// `>` → `<`, `>=` → `<=` — the non-canonical orientations A1 flips (operands swap too).
fn orient_flip(op: &str) -> Option<&'static str> {
    match op {
        ">" => Some("<"),
        ">=" => Some("<="),
        _ => None,
    }
}

/// Comparison inversion under negation (A2): `<`↔`>=`, `>`↔`<=`, `==`↔`!=` (operands kept).
fn invert_cmp(op: &str) -> Option<&'static str> {
    Some(match op {
        "<" => ">=",
        "<=" => ">",
        ">" => "<=",
        ">=" => "<",
        "==" => "!=",
        "!=" => "==",
        _ => return None,
    })
}

/// De Morgan operator flip (A2): `&&`↔`||` (and the Python-canonical `and`↔`or`).
fn demorgan_flip(op: &str) -> Option<&'static str> {
    Some(match op {
        "&&" => "||",
        "||" => "&&",
        "and" => "or",
        "or" => "and",
        _ => return None,
    })
}

/// Swap a `Binop`'s operands (keeping the canonical `@left`/`@right` fields) and set its
/// operator token — the shared mutation for a `CmpOrient` (and A2's comparison inversion).
fn swap_binop(mut node: NormNode, new_op: &str) -> NormNode {
    let mut it = std::mem::take(&mut node.children).into_iter();
    let mut left = it.next().expect("binop left");
    let op = it.next().expect("binop op");
    let mut right = it.next().expect("binop right");
    left.field = Some("right".into());
    right.field = Some("left".into());
    let op = NormNode::new(new_op, Some("op"), op.span, Vec::new());
    node.children = vec![right, op, left];
    node
}

// ---- Family C: guard canonicalization (§4.2, D-IR-13) ----

/// Family C (D-IR-13) as a detect/emit pass (D-IR-12): canonicalize guard nesting so a
/// hand-refactor between nesting/conjunction and early-return converges. C1 merges a nested
/// single-arm, no-else `if a { if b { … } }` into one `if a && b { … }`; C2 drops a redundant
/// `else` after a diverging then-arm and hoists its body (`if c { return } else { Y }` →
/// `if c { return }; Y`). Both are sound only for their restricted shapes (§3). Runs BEFORE
/// `abstract_idents` (it moves whole guard/body subtrees, so keeping loci on `Raw` labels
/// matches the other structural passes) and BEFORE Family A (C1 synthesizes the `a && b` guard
/// that A then normalizes and comm-sort finally sorts). Detect-only — one
/// [`GuardMerge`](Edit::GuardMerge) / [`DropElse`](Edit::DropElse) edit per site, so a fold
/// cannot happen without [`crate::ir::edit::apply`] recording the event.
pub fn detect_guard_canonicalize(root: &NormNode) -> Vec<Edit> {
    let mut edits = Vec::new();
    guard_canon_edits(root, &mut edits);
    edits
}

/// Convenience over the seam (tests): disabled sink → identical tree. See [`abstract_idents`].
pub fn guard_canonicalize(node: NormNode) -> NormNode {
    let edits = detect_guard_canonicalize(&node);
    crate::ir::edit::apply(node, &edits, &mut TransformLog::disabled())
}

/// Bottom-up companion to [`detect_guard_canonicalize`]: canonicalize inner guards before this
/// one (a merged inner branch becomes this node's sole body child, itself then mergeable),
/// emit one edit per fold, and return the rewritten subtree so the walk stays in step with the
/// applier.
fn guard_canon_edits(node: &NormNode, out: &mut Vec<Edit>) -> NormNode {
    let mut n = node.clone();
    n.children = node
        .children
        .iter()
        .map(|c| guard_canon_edits(c, out))
        .collect();
    // C2 · redundant-else: a block-level splice, one edit per else dropped (keyed to the block).
    let locus = n.span;
    for _ in fold_dead_else(&mut n) {
        out.push(Edit::DropElse { locus });
    }
    // C1 · conjunction-merge (a node is a Block or a Branch, never both — no double-count).
    if let Some(merged) = try_merge_branch(&n) {
        out.push(Edit::GuardMerge { locus: n.span });
        return merged;
    }
    n
}

/// C1 conjunction-merge — the shared fold [`crate::ir::edit::apply`] performs for a
/// [`GuardMerge`](Edit::GuardMerge): if `node` is a single-arm, no-else
/// `Branch{ Arm{ g_outer, Block{ inner } } }` whose sole body statement `inner` is itself a
/// single-arm, no-else `Branch{ Arm{ g_inner, body } }`, return the merged
/// `Branch{ Arm{ g_outer && g_inner, body } }`. Returns `None` (no fold) for any other shape —
/// an `else` on either branch, or extra statements around the inner `if`, where the merge would
/// be unsound.
pub(crate) fn try_merge_branch(node: &NormNode) -> Option<NormNode> {
    let outer_arm = single_guarded_arm(node)?;
    let outer_guard = arm_field(outer_arm, "guard")?;
    let outer_body = arm_field(outer_arm, "body")?;
    if outer_body.kind.as_ref() != kind::BLOCK || outer_body.children.len() != 1 {
        return None;
    }
    let inner_arm = single_guarded_arm(&outer_body.children[0])?;
    let inner_guard = arm_field(inner_arm, "guard")?;
    let inner_body = arm_field(inner_arm, "body")?;
    let span = node.span;
    let guard = and_guard(outer_guard.clone(), inner_guard.clone(), span);
    let mut body = inner_body.clone();
    body.field = Some("body".into());
    let arm = NormNode::new(kind::ARM, Some("arm"), span, vec![guard, body]);
    Some(NormNode::new(
        kind::BRANCH,
        node.field.as_deref(),
        span,
        vec![arm],
    ))
}

/// The sole `Arm` of a single-arm, no-else `Branch` (an `if` with no `else`): the branch has
/// exactly one arm and that arm carries a `@guard`. Else `None`.
fn single_guarded_arm(branch: &NormNode) -> Option<&NormNode> {
    if branch.kind.as_ref() != kind::BRANCH || branch.children.len() != 1 {
        return None;
    }
    let arm = &branch.children[0];
    (arm.kind.as_ref() == kind::ARM && arm_field(arm, "guard").is_some()).then_some(arm)
}

/// The child of an `Arm` occupying `field` (`guard` / `body`).
fn arm_field<'a>(arm: &'a NormNode, field: &str) -> Option<&'a NormNode> {
    arm.children
        .iter()
        .find(|c| c.field.as_deref() == Some(field))
}

/// Build a conjunction guard `Binop{ l && r }@guard` — byte-identical to how a source
/// `if l && r` lowers, so the merged nest converges with the hand-written conjunction.
fn and_guard(mut l: NormNode, mut r: NormNode, span: (u32, u32)) -> NormNode {
    l.field = Some("left".into());
    r.field = Some("right".into());
    let op = NormNode::new("&&", Some("op"), span, Vec::new());
    NormNode::new(kind::BINOP, Some("guard"), span, vec![l, op, r])
}

/// C2 redundant-else fold — the shared block rebuild [`crate::ir::edit::apply`] performs for a
/// [`DropElse`](Edit::DropElse). For each child that is a two-arm `Branch [Arm(g, diverging
/// body), Arm(else body)]` (a plain `if/else` whose then-arm provably [`diverges`]), drop the
/// else arm and splice its body's statements in as siblings right after the branch — so
/// `if c { return } else { Y }` converges with `if c { return }; Y`. Returns the hoisted
/// else-bodies in order (the witnesses that let reversal re-wrap them); empty ⇒ nothing folded.
/// A no-op on non-`Block` nodes, so the walk can call it at every node.
pub(crate) fn fold_dead_else(block: &mut NormNode) -> Vec<NormNode> {
    if block.kind.as_ref() != kind::BLOCK {
        return Vec::new();
    }
    let mut hoisted = Vec::new();
    let mut out = Vec::with_capacity(block.children.len());
    for child in std::mem::take(&mut block.children) {
        match split_redundant_else(&child) {
            Some((trimmed, else_body)) => {
                out.push(trimmed);
                out.extend(else_stmts(else_body.clone()));
                hoisted.push(else_body);
            }
            None => out.push(child),
        }
    }
    block.children = out;
    hoisted
}

/// If `branch` is a plain two-arm `if/else` whose then-arm diverges, return
/// `(the then-only branch, the else body)`. `None` for any other shape (an `else if` chain,
/// a non-diverging then-arm, or no else) — where dropping the else would be unsound.
fn split_redundant_else(branch: &NormNode) -> Option<(NormNode, NormNode)> {
    if branch.kind.as_ref() != kind::BRANCH || branch.children.len() != 2 {
        return None;
    }
    let then_arm = &branch.children[0];
    let else_arm = &branch.children[1];
    // then-arm: a guarded `if` whose body provably diverges.
    arm_field(then_arm, "guard")?;
    if !diverges(arm_field(then_arm, "body")?) {
        return None;
    }
    // else-arm: a plain `else` (trivial guard) — not an `else if` (which would carry a guard).
    if arm_field(else_arm, "guard").is_some() {
        return None;
    }
    let else_body = arm_field(else_arm, "body")?.clone();
    let mut trimmed = branch.clone();
    trimmed.children.truncate(1); // keep only the then-arm
    Some((trimmed, else_body))
}

/// The statements an else body contributes when spliced as siblings: a `Block`'s children
/// (the else statements), or the body itself as a lone statement.
fn else_stmts(else_body: NormNode) -> Vec<NormNode> {
    if else_body.kind.as_ref() == kind::BLOCK {
        else_body.children
    } else {
        vec![else_body]
    }
}

/// Whether `body` definitely diverges: its last statement is a `Return`/`Break`/`Continue`, or a
/// `Branch` that is total (has an else) and all of whose arm bodies diverge. Pure, detector-side
/// — the soundness precondition for C2's redundant-else drop, reusable by future §13-rung-2 work.
pub(crate) fn diverges(body: &NormNode) -> bool {
    let last = if body.kind.as_ref() == kind::BLOCK {
        body.children.last()
    } else {
        Some(body)
    };
    let Some(last) = last else {
        return false;
    };
    match last.kind.as_ref() {
        kind::RETURN | kind::BREAK | kind::CONTINUE => true,
        kind::BRANCH => {
            let arms = &last.children;
            !arms.is_empty()
                && arms.iter().any(|a| arm_field(a, "guard").is_none()) // total (has an else)
                && arms
                    .iter()
                    .all(|a| arm_field(a, "body").is_some_and(diverges))
        }
        _ => false,
    }
}

/// Spec §5.2.7 dead-syntax removal as a detect/emit pass (D-IR-12): drop a redundant
/// trailing `Continue` at a loop-body tail (a source `continue` at a loop's end is a no-op,
/// and lowered recursion emits one), so `for x in xs { f(x); continue }` converges with
/// `for x in xs { f(x) }`. Immutable tree in, one [`DropDead`](Edit::DropDead) edit per
/// removed node out — the removal cannot happen without [`crate::ir::edit::apply`] recording
/// it (the removed subtree is the witness).
pub fn detect_dead(node: &NormNode) -> Vec<Edit> {
    let mut edits = Vec::new();
    dead_edits(node, &mut edits);
    edits
}

/// Bottom-up companion to [`detect_dead`]: strip inner loops before this one (mirrors the
/// historical post-order recursion), popping each trailing `Continue` and recording its
/// locus, and return the stripped subtree so the walk stays in step with the applier.
fn dead_edits(node: &NormNode, out: &mut Vec<Edit>) -> NormNode {
    let mut n = node.clone();
    n.children = node.children.iter().map(|c| dead_edits(c, out)).collect();
    if n.kind.as_ref() == kind::LOOP
        && let Some(body) = n
            .children
            .iter_mut()
            .find(|c| c.field.as_deref() == Some("body"))
    {
        while body
            .children
            .last()
            .is_some_and(|c| c.kind.as_ref() == kind::CONTINUE)
        {
            let removed = body.children.pop().expect("checked last() is Some");
            out.push(Edit::DropDead {
                locus: removed.span,
            });
        }
    }
    n
}

/// Convenience over the seam (tests): disabled sink → identical tree. See [`abstract_idents`].
pub fn strip_dead(node: NormNode) -> NormNode {
    let edits = detect_dead(&node);
    crate::ir::edit::apply(node, &edits, &mut TransformLog::disabled())
}

/// Spec §5.2.2 companion rule (the historical `normalize_loop_exit`, now **once** for all
/// languages on the canonical forms): a block tail `Loop{…}; Return{E}` folds by
/// replacing every top-level bare `Break` in the loop with `Return{E}` and dropping the
/// trailing return. Sound — `loop { … break … } ; return E` ≡ `loop { … return E … }`,
/// and it converges a `while c { … } ; return E` (whose guard-break becomes the return)
/// with the hand-written early-return form. Nested loops own their own breaks (skipped).
pub fn detect_loop_exit(node: &NormNode) -> Vec<Edit> {
    let mut edits = Vec::new();
    loop_exit_edits(node, &mut edits);
    edits
}

/// Bottom-up companion to [`detect_loop_exit`]: fold inner blocks before this one (mirrors
/// the historical post-order recursion) and return the folded subtree so the walk stays in
/// step with the applier.
fn loop_exit_edits(node: &NormNode, out: &mut Vec<Edit>) -> NormNode {
    let mut n = node.clone();
    n.children = node
        .children
        .iter()
        .map(|c| loop_exit_edits(c, out))
        .collect();
    if n.kind.as_ref() == kind::BLOCK && fold_loop_exit(&mut n) {
        out.push(Edit::LoopExit { locus: n.span });
    }
    n
}

/// Convenience over the seam (tests): disabled sink → identical tree. See [`abstract_idents`].
pub fn normalize_loop_exit(node: NormNode) -> NormNode {
    let edits = detect_loop_exit(&node);
    crate::ir::edit::apply(node, &edits, &mut TransformLog::disabled())
}

/// The shared loop-exit fold [`crate::ir::edit::apply`] performs for a
/// [`LoopExit`](Edit::LoopExit): if `block`'s tail is `Loop{…}; Return{E}` and the loop has a
/// top-level bare break, replace those breaks with `Return{E}` and drop the trailing return.
/// Returns whether it fired (a firing block is exactly a detected site).
pub(crate) fn fold_loop_exit(block: &mut NormNode) -> bool {
    let n = block.children.len();
    if n < 2 {
        return false;
    }
    let last = &block.children[n - 1];
    let is_return_value = last.kind.as_ref() == kind::RETURN && last.children.len() == 1;
    if !is_return_value || block.children[n - 2].kind.as_ref() != kind::LOOP {
        return false;
    }
    let value = block.children[n - 1].children[0].clone();
    let mut replaced = 0;
    replace_breaks(&mut block.children[n - 2], &value, &mut replaced, true);
    if replaced > 0 {
        block.children.truncate(n - 1);
        return true;
    }
    false
}

fn replace_breaks(node: &mut NormNode, value: &NormNode, replaced: &mut u32, top: bool) {
    if !top && node.kind.as_ref() == kind::LOOP {
        return; // a nested loop owns its own breaks
    }
    let mut i = 0;
    while i < node.children.len() {
        if node.children[i].kind.as_ref() == kind::BREAK && node.children[i].children.is_empty() {
            let span = node.children[i].span;
            let mut v = value.clone();
            v.field = Some("value".into());
            node.children[i] = NormNode::new(kind::RETURN, None, span, vec![v]);
            *replaced += 1;
        } else {
            replace_breaks(&mut node.children[i], value, replaced, false);
        }
        i += 1;
    }
}

/// Spec §5.2.2 iteration-protocol rewrite (the historical per-profile `rewrite_iteration`,
/// now **once** on the canonical protocol form): an index loop `for i in 0..len(xs)` whose
/// body touches `i` only as `xs[i]` is the same computation as `for x in xs`. Because both
/// forms already lowered to the shared `__has_next`/`__next` `Loop`, the rewrite is uniform:
/// swap the range iterator for `xs` in both protocol calls and replace `xs[i]` with `i`.
/// Runs on `Raw` labels (before abstraction). Only the range/len *matcher* is
/// language-shaped (Rust `0..xs.len()` vs Python `range(len(xs))`); the rewrite is shared.
pub fn detect_iter_protocol(node: &NormNode) -> Vec<Edit> {
    let mut edits = Vec::new();
    iter_protocol_edits(node, &mut edits);
    edits
}

/// Bottom-up companion to [`detect_iter_protocol`]: rewrite inner loops before matching this
/// one (mirrors the historical post-order recursion) and return the rewritten subtree, so a
/// nested rewrite settles first and the walk stays in step with the applier.
fn iter_protocol_edits(node: &NormNode, out: &mut Vec<Edit>) -> NormNode {
    let mut n = node.clone();
    n.children = node
        .children
        .iter()
        .map(|c| iter_protocol_edits(c, out))
        .collect();
    let matched = (n.kind.as_ref() == kind::LOOP)
        .then(|| {
            n.children
                .iter()
                .find(|c| c.field.as_deref() == Some("body"))
                .and_then(index_loop_match)
        })
        .flatten();
    if let Some((ivar, coll, span)) = matched {
        out.push(Edit::IterProtocol {
            locus: span,
            coll: coll.clone(),
            ivar: ivar.clone(),
        });
        rewrite_index_loop(&mut n, &ivar, &coll, span);
    }
    n
}

/// Convenience over the seam (tests): disabled sink → identical tree. See [`abstract_idents`].
pub fn rewrite_iteration(node: NormNode) -> NormNode {
    let edits = detect_iter_protocol(&node);
    crate::ir::edit::apply(node, &edits, &mut TransformLog::disabled())
}

/// The shared iteration-protocol mutation [`crate::ir::edit::apply`] performs for an
/// [`IterProtocol`](Edit::IterProtocol): on the matched index `Loop`, retarget the range
/// iterator to `coll` in both protocol calls and replace every `coll[ivar]` with a bare
/// `ivar` (the element var). `span` is the range iterator's span (reused for the synthesized
/// nodes).
pub(crate) fn rewrite_index_loop(node: &mut NormNode, ivar: &str, coll: &str, span: (u32, u32)) {
    let elem = |field: Option<&str>| {
        NormNode::new(kind::VAR, field, span, Vec::new()).with_label(Label::Raw(coll.into()))
    };
    let Some(body) = node
        .children
        .iter_mut()
        .find(|c| c.field.as_deref() == Some("body"))
    else {
        return;
    };
    for stmt in body.children.iter_mut() {
        retarget_iterator(stmt, ivar, coll, span);
    }
    for stmt in body.children.iter_mut().skip(2) {
        *stmt = replace_index(std::mem::replace(stmt, elem(None)), ivar, coll);
    }
}

/// `(index var, collection, iterator span)` — the result of matching an index loop.
pub(crate) type IndexLoop = (Box<str>, Box<str>, (u32, u32));

/// If `body` is the canonical index-loop protocol, return `(index var, collection, span)`.
pub(crate) fn index_loop_match(body: &NormNode) -> Option<IndexLoop> {
    // body[1] = `Assign{ Var@target ivar, Call@value __next(ITER) }`.
    let bind = body.children.get(1)?;
    if bind.kind.as_ref() != kind::ASSIGN || bind.children.len() != 2 {
        return None;
    }
    let ivar = match (&bind.children[0].kind, &bind.children[0].label) {
        (k, Some(Label::Raw(t))) if k.as_ref() == kind::VAR => t.clone(),
        _ => return None,
    };
    let iter = call_arg(&bind.children[1], "__next")?;
    let coll = range_len_collection(iter)?;
    // Every use of `ivar` past the protocol prelude must be exactly `coll[ivar]`.
    if !body
        .children
        .iter()
        .skip(2)
        .all(|s| index_uses_only(s, &ivar, &coll))
    {
        return None;
    }
    Some((ivar, coll, iter.span))
}

/// A callee's name, matched whether still `Raw` (a plain `range`/`len` identifier — this
/// pass runs pre-abstraction) or already `External` (synthesized `__next`, Rust `.len`).
fn label_name_is(label: &Option<Label>, name: &str) -> bool {
    matches!(label, Some(Label::External(t) | Label::Raw(t)) if t.as_ref() == name)
}

/// The single argument of a `Call` whose callee is named `name` (`__next` / `__has_next`).
fn call_arg<'a>(node: &'a NormNode, name: &str) -> Option<&'a NormNode> {
    call_named_arg(node, name)
}

/// Match `0..xs.len()` (Rust: `Binop{Lit 0, .., Call{Field{xs,len}}}`) or `range(len(xs))`
/// (Python: `Call{range, Call{len, xs}}`), returning the collection name.
fn range_len_collection(iter: &NormNode) -> Option<Box<str>> {
    // Rust range.
    if iter.kind.as_ref() == kind::BINOP && iter.children.len() == 3 {
        let is_zero =
            matches!(&iter.children[0].label, Some(Label::LitKept(t)) if t.as_ref() == "0");
        if is_zero && iter.children[1].kind.as_ref() == ".." {
            return len_call_collection(&iter.children[2]);
        }
    }
    // Python `range(len(xs))`.
    if let Some(inner) = call_named_arg(iter, "range") {
        return len_call_collection_py(inner);
    }
    None
}

/// Rust `xs.len()` → `Call{ callee: Field{ Var xs, len }, }` → `xs`.
fn len_call_collection(node: &NormNode) -> Option<Box<str>> {
    if node.kind.as_ref() != kind::CALL {
        return None;
    }
    let field = node
        .children
        .iter()
        .find(|c| c.field.as_deref() == Some("callee"))?;
    if field.kind.as_ref() != kind::FIELD {
        return None;
    }
    let name = field
        .children
        .iter()
        .find(|c| c.field.as_deref() == Some("name"))?;
    if !label_name_is(&name.label, "len") {
        return None;
    }
    let base = field
        .children
        .iter()
        .find(|c| c.field.as_deref() == Some("base"))?;
    raw_text(base)
}

/// Python `len(xs)` → `xs`.
fn len_call_collection_py(node: &NormNode) -> Option<Box<str>> {
    call_named_arg(node, "len").and_then(raw_text)
}

/// The single `@arg` of a `Call` whose callee is named `name` (`Raw` or `External`).
fn call_named_arg<'a>(node: &'a NormNode, name: &str) -> Option<&'a NormNode> {
    if node.kind.as_ref() != kind::CALL {
        return None;
    }
    let callee = node
        .children
        .iter()
        .find(|c| c.field.as_deref() == Some("callee"))?;
    if !label_name_is(&callee.label, name) {
        return None;
    }
    node.children
        .iter()
        .find(|c| c.field.as_deref() == Some("arg"))
}

fn raw_text(node: &NormNode) -> Option<Box<str>> {
    match &node.label {
        Some(Label::Raw(t)) if node.kind.as_ref() == kind::VAR => Some(t.clone()),
        _ => None,
    }
}

fn is_index(node: &NormNode, ivar: &str, coll: &str) -> bool {
    let child_is = |field: &str, name: &str| {
        node.children
            .iter()
            .find(|c| c.field.as_deref() == Some(field))
            .and_then(raw_text)
            .is_some_and(|t| t.as_ref() == name)
    };
    node.kind.as_ref() == kind::INDEX && child_is("base", coll) && child_is("idx", ivar)
}

/// Every use of `ivar` in `node` is exactly `coll[ivar]` (a bare `ivar` fails the match).
fn index_uses_only(node: &NormNode, ivar: &str, coll: &str) -> bool {
    if is_index(node, ivar, coll) {
        return true;
    }
    if matches!(&node.label, Some(Label::Raw(t)) if t.as_ref() == ivar) {
        return false;
    }
    node.children.iter().all(|c| index_uses_only(c, ivar, coll))
}

/// Swap the range iterator inside a protocol `__has_next(ITER)` / `__next(ITER)` call for a
/// bare `Var coll` (leaving the arg's field intact).
fn retarget_iterator(node: &mut NormNode, _ivar: &str, coll: &str, span: (u32, u32)) {
    let is_protocol = node.kind.as_ref() == kind::CALL
        && node.children.iter().any(|c| {
            c.field.as_deref() == Some("callee")
                && matches!(&c.label, Some(Label::External(t)) if matches!(t.as_ref(), "__has_next" | "__next"))
        });
    if is_protocol {
        if let Some(arg) = node
            .children
            .iter_mut()
            .find(|c| c.field.as_deref() == Some("arg"))
        {
            *arg = NormNode::new(kind::VAR, Some("arg"), span, Vec::new())
                .with_label(Label::Raw(coll.into()));
        }
        return;
    }
    for c in &mut node.children {
        retarget_iterator(c, _ivar, coll, span);
    }
}

/// Replace every `coll[ivar]` in `node` with a bare `Var ivar` (keeping the field).
fn replace_index(mut node: NormNode, ivar: &str, coll: &str) -> NormNode {
    if is_index(&node, ivar, coll) {
        return NormNode::new(kind::VAR, node.field.as_deref(), node.span, Vec::new())
            .with_label(Label::Raw(ivar.into()));
    }
    node.children = node
        .children
        .into_iter()
        .map(|c| replace_index(c, ivar, coll))
        .collect();
    node
}

// ---- C-style counter-loop iteration rewrite (spec §5.2.2, same canonicalization as
// `detect_iter_protocol` reached from a different source syntax) ----

/// The Go `for i := 0; i < len(coll); i++ { … coll[i] … }` / Rust `let mut i = 0; while i <
/// coll.len() { … coll[i] …; i += 1 }` counter loop is the same computation as `for x in coll`
/// — but where [`detect_iter_protocol`] rewrites the *range-protocol* loop (whose `__has_next`/
/// `__next` already iterate a `0..len` range), the C-style form is a **block-level** shape: an
/// init sibling `i = 0` BEFORE the loop, a break-guard `!(i < len(coll))`, and a trailing
/// increment `i = i + 1` INSIDE it. This pass recognizes that shape and rewrites it to the
/// SAME canonical iteration-protocol foreach form, so Go counter ≡ Go range ≡ Rust foreach ≡
/// Rust while-index all converge. Runs in the same pipeline slot as (right after)
/// `detect_iter_protocol`, on `Raw` labels (pre-abstraction, pre-Family-A) — the two matchers
/// are disjoint (a counter loop has no `__next` bind; a range loop has no `i = 0` init sibling),
/// so order between them is immaterial. Detect-only (D-IR-12): one [`CounterIter`](Edit::CounterIter)
/// edit per rewritten loop, so the block-level splice cannot happen without
/// [`crate::ir::edit::apply`] recording the event.
pub fn detect_counter_iter(node: &NormNode) -> Vec<Edit> {
    let mut edits = Vec::new();
    counter_iter_edits(node, &mut edits);
    edits
}

/// Bottom-up companion to [`detect_counter_iter`]: rewrite inner blocks before this one (a
/// nested counter loop settles first) and return the rewritten subtree, so the walk stays in
/// step with the applier. Both sides drive the shared [`fold_counter_loops`], so the matches —
/// and the emitted/consumed edit order — agree by construction.
fn counter_iter_edits(node: &NormNode, out: &mut Vec<Edit>) -> NormNode {
    let mut n = node.clone();
    n.children = node
        .children
        .iter()
        .map(|c| counter_iter_edits(c, out))
        .collect();
    let locus = n.span;
    for (coll, ivar, _span) in fold_counter_loops(&mut n) {
        out.push(Edit::CounterIter { locus, coll, ivar });
    }
    n
}

/// Convenience over the seam (tests): disabled sink → identical tree. See [`abstract_idents`].
pub fn rewrite_counter_iteration(node: NormNode) -> NormNode {
    let edits = detect_counter_iter(&node);
    crate::ir::edit::apply(node, &edits, &mut TransformLog::disabled())
}

/// One recognized counter loop: `(collection, index var, span)` — the [`CounterIter`](Edit::CounterIter)
/// payload / witness (the original `coll[ivar]` index form).
pub(crate) type CounterMatch = (Box<str>, Box<str>, (u32, u32));

/// The shared block-level fold [`crate::ir::edit::apply`] performs for a
/// [`CounterIter`](Edit::CounterIter): for each `Block` child that is an init `i = 0`
/// immediately followed by a matching C-style counter `Loop`, drop the init and rewrite the
/// loop to the canonical iteration-protocol foreach form (byte-identical to how a `for x in
/// coll` lowers). Returns one `(coll, ivar, span)` per rewritten loop, in block order — a
/// firing pair is exactly a detected site, so detector and applier stay in lockstep (like
/// [`fold_loop_exit`]). A no-op on non-`Block` nodes.
pub(crate) fn fold_counter_loops(block: &mut NormNode) -> Vec<CounterMatch> {
    if block.kind.as_ref() != kind::BLOCK {
        return Vec::new();
    }
    let children = std::mem::take(&mut block.children);
    // Match (init@k, loop@k+1) pairs immutably first — the "index var used only here"
    // safety check needs to see every sibling, so a single streaming pass won't do.
    let matched: Vec<Option<CounterMatch>> = (0..children.len())
        .map(|k| {
            (k + 1 < children.len())
                .then(|| counter_loop_pair(&children, k))
                .flatten()
        })
        .collect();
    let mut matches = Vec::new();
    let mut out = Vec::with_capacity(children.len());
    for (k, child) in children.into_iter().enumerate() {
        if matched[k].is_some() {
            continue; // the init sibling — dropped (the loop at k+1 carries the iteration)
        }
        if k > 0
            && let Some((coll, ivar, span)) = matched[k - 1].clone()
        {
            let mut loop_node = child;
            rewrite_counter_loop(&mut loop_node, &ivar, &coll, span);
            matches.push((coll, ivar, span));
            out.push(loop_node);
        } else {
            out.push(child);
        }
    }
    block.children = out;
    matches
}

/// Match the `(init@k, loop@k+1)` counter-loop shape, returning `(coll, ivar, span)`. The
/// safety restrictions (mirroring the range form's `index_uses_only`, plus the counter-loop
/// specifics): the loop's guard is `!(ivar < len(coll))`, its tail is the unit increment
/// `ivar = ivar + 1`, the init is `ivar = 0`, every use of `ivar` between guard and increment
/// is exactly `coll[ivar]`, and `ivar` appears NOWHERE else in the block (a loop-local
/// counter) — so a var mutated elsewhere, a different indexed collection, a non-unit stride,
/// or an index used for its own sake all fail the match and stay distinct.
fn counter_loop_pair(children: &[NormNode], k: usize) -> Option<CounterMatch> {
    let loop_node = &children[k + 1];
    if loop_node.kind.as_ref() != kind::LOOP {
        return None;
    }
    let body = loop_node
        .children
        .iter()
        .find(|c| c.field.as_deref() == Some("body"))?;
    if body.children.len() < 2 {
        return None;
    }
    let (ivar, coll) = counter_guard(&body.children[0])?;
    // The unit increment `ivar = ivar + 1` may sit ANYWHERE in the body, not only last: a
    // read-then-advance loop puts it right after the element read (`x = coll[i]; i += 1; …`),
    // and it must still converge with a `for x in coll`. Require exactly one such increment
    // (a single stride site); its position is dropped by the rewrite.
    let inc = increment_position_in(&body.children, &ivar)?;
    if !counter_init(&children[k], &ivar) {
        return None;
    }
    // Every use of `ivar` in the loop body (excluding the guard[0] and the increment) must be
    // exactly `coll[ivar]` — the same restriction the range form enforces.
    if !body
        .children
        .iter()
        .enumerate()
        .filter(|(j, _)| *j != 0 && *j != inc)
        .all(|(_, s)| index_uses_only(s, &ivar, &coll))
    {
        return None;
    }
    // `ivar` is a loop-local counter: it must not appear in any other sibling (before the
    // init or after the loop) — else the rewrite would drop a live use.
    if !children
        .iter()
        .enumerate()
        .all(|(j, c)| j == k || j == k + 1 || !mentions_raw(c, &ivar))
    {
        return None;
    }
    Some((coll, ivar, loop_node.span))
}

/// A break-guard `Branch{ Arm{ !(ivar < len(coll)) → { Break } } }` → `(ivar, coll)`.
fn counter_guard(branch: &NormNode) -> Option<(Box<str>, Box<str>)> {
    if branch.kind.as_ref() != kind::BRANCH || branch.children.len() != 1 {
        return None;
    }
    let arm = &branch.children[0];
    if arm.kind.as_ref() != kind::ARM {
        return None;
    }
    let guard = arm
        .children
        .iter()
        .find(|c| c.field.as_deref() == Some("guard"))?;
    let (left, op, right) = binop_parts(not_operand(guard)?)?;
    if op != "<" {
        return None;
    }
    Some((raw_text(left)?, len_collection(right)?))
}

/// The collection of a `len` call, either Rust `coll.len()` or Go/Python `len(coll)`.
fn len_collection(node: &NormNode) -> Option<Box<str>> {
    len_call_collection(node).or_else(|| len_call_collection_py(node))
}

/// The index of the single unit increment `ivar = ivar + 1`, if there is exactly one (a lone
/// stride site) among `stmts`; `None` for zero or several. The increment need not be last — a
/// read-then-advance loop puts it right after the element read.
fn increment_position_in(stmts: &[NormNode], ivar: &str) -> Option<usize> {
    let mut found = None;
    for (i, stmt) in stmts.iter().enumerate() {
        if counter_increment(stmt, ivar) {
            if found.is_some() {
                return None; // more than one stride site — not a plain counter
            }
            found = Some(i);
        }
    }
    found
}

/// `stmt` is the unit increment `ivar = ivar + 1` (`ivar` in any assign-target position).
fn counter_increment(stmt: &NormNode, ivar: &str) -> bool {
    if stmt.kind.as_ref() != kind::ASSIGN || stmt.children.len() != 2 {
        return false;
    }
    let Some((left, op, right)) = binop_parts(&stmt.children[1]) else {
        return false;
    };
    is_raw_ident(&stmt.children[0], ivar)
        && op == "+"
        && is_raw_ident(left, ivar)
        && is_lit(right, "1")
}

/// `stmt` is the loop init `ivar = 0`.
fn counter_init(stmt: &NormNode, ivar: &str) -> bool {
    stmt.kind.as_ref() == kind::ASSIGN
        && stmt.children.len() == 2
        && is_raw_ident(&stmt.children[0], ivar)
        && is_lit(&stmt.children[1], "0")
}

/// A kept structural literal (`0`/`1`) with value `val` — the loop init/step constants.
fn is_lit(node: &NormNode, val: &str) -> bool {
    node.kind.as_ref() == kind::LIT
        && matches!(&node.label, Some(Label::LitKept(t)) if t.as_ref() == val)
}

/// Any `Raw` `Var` named `name` occurs anywhere in `node`.
fn mentions_raw(node: &NormNode, name: &str) -> bool {
    (node.kind.as_ref() == kind::VAR
        && matches!(&node.label, Some(Label::Raw(t)) if t.as_ref() == name))
        || node.children.iter().any(|c| mentions_raw(c, name))
}

/// Rewrite a matched counter `Loop` in place to the canonical iteration-protocol form — the
/// shared mutation [`crate::ir::edit::apply`] performs for a [`CounterIter`](Edit::CounterIter).
/// The body becomes `[ !__has_next(coll) → break; ivar = __next(coll); …coll[ivar]→ivar… ]`,
/// dropping the old guard (first) and increment (last) and retargeting each `coll[ivar]` to a
/// bare `ivar`. Reuses the frontend's `break_guard`/`call_ext`/`make_assign` so the synthesized
/// nodes are byte-identical to a lowered `for x in coll` (guaranteeing convergence).
pub(crate) fn rewrite_counter_loop(
    loop_node: &mut NormNode,
    ivar: &str,
    coll: &str,
    span: (u32, u32),
) {
    use crate::frontend::{break_guard, call_ext, make_assign};
    let coll_var = |field: Option<&str>| {
        NormNode::new(kind::VAR, field, span, Vec::new()).with_label(Label::Raw(coll.into()))
    };
    let Some(body) = loop_node
        .children
        .iter_mut()
        .find(|c| c.field.as_deref() == Some("body"))
    else {
        return;
    };
    let old = std::mem::take(&mut body.children);
    let inc = increment_position_in(&old, ivar); // the increment — dropped (any position)
    let target = NormNode::new(kind::VAR, Some("target"), span, Vec::new())
        .with_label(Label::Raw(ivar.into()));
    let mut new_body = Vec::with_capacity(old.len());
    new_body.push(break_guard(
        call_ext("__has_next", coll_var(None), span),
        span,
    ));
    new_body.push(make_assign(
        target,
        call_ext("__next", coll_var(None), span),
        span,
    ));
    for (i, stmt) in old.into_iter().enumerate() {
        if i == 0 || Some(i) == inc {
            continue; // old break-guard / increment
        }
        new_body.push(replace_index(stmt, ivar, coll));
    }
    body.children = new_body;
}

// ---- parallel multi-assign decomposition (§13 decomposition family; ONLY the
// multi-assign/parallel-assign rung, not the full ANF ladder) ----

/// Decompose a parallel multi-target assignment (`a, b = X, Y`) into a canonical **sequence of
/// single assigns** as a detect/emit pass (D-IR-12), so all its spellings converge on ONE form:
/// the parallel `a, b = X, Y`, the two adjacent single assigns `a = X; b = Y`, and a tail-rec
/// reassignment (`return f(b, a%b)`, which lowers to the SAME parallel `Assign`) all reduce here.
///
/// The decomposition is **minimal-temp** (the classic parallel-copy sequentialization): a single
/// assign `a_i = v_i` is emitted only once every OTHER pair that still reads the old `a_i` has
/// been emitted; a temp is introduced solely to break a read-after-write cycle (a swap `a, b =
/// b, a`). An INDEPENDENT parallel assign (`x, y = x+1, y*2`) therefore emits NO temp and is
/// byte-identical to the literal two adjacent assigns, while the coupled `a, b = b, a%b` emits
/// `t = a; a = b; b = t%b` — distinct from the semantically-different sequential `a = b; b = a%b`.
///
/// Runs BEFORE `abstract_idents` (the synthetic temps are `Var@target` binding sites, so
/// `collect_declared` picks them up and they become positional `Local`s, D2) and on `Raw`
/// labels. Detect-only: one [`MultiAssign`](Edit::MultiAssign) edit per decomposed site — the
/// block-level splice cannot happen without [`crate::ir::edit::apply`] recording the event, and
/// the original multi-assign is the witness (D-IR-9).
pub fn detect_multi_assign(root: &NormNode) -> Vec<Edit> {
    let mut edits = Vec::new();
    multi_assign_edits(root, &mut edits);
    edits
}

/// Bottom-up companion to [`detect_multi_assign`]: decompose inner blocks before this one and
/// return the rewritten subtree, so the walk stays in step with the applier. Both sides drive
/// the shared [`fold_multi_assign`], so the matches — and the emitted/consumed edit order — agree.
fn multi_assign_edits(node: &NormNode, out: &mut Vec<Edit>) -> NormNode {
    let mut n = node.clone();
    n.children = node
        .children
        .iter()
        .map(|c| multi_assign_edits(c, out))
        .collect();
    let locus = n.span;
    for _ in fold_multi_assign(&mut n) {
        out.push(Edit::MultiAssign { locus });
    }
    n
}

/// Convenience over the seam (tests): disabled sink → identical tree. See [`abstract_idents`].
pub fn decompose_multi_assign(node: NormNode) -> NormNode {
    let edits = detect_multi_assign(&node);
    crate::ir::edit::apply(node, &edits, &mut TransformLog::disabled())
}

/// The shared block-level fold [`crate::ir::edit::apply`] performs for a
/// [`MultiAssign`](Edit::MultiAssign): for each `Block` child that is a decomposable parallel
/// multi-target `Assign`, replace it by its minimal single-assign sequence (a block-level
/// splice, like [`fold_counter_loops`]). Returns the original multi-assign node per decomposed
/// site, in block order — a firing child is exactly a detected site, so detector and applier
/// stay in lockstep. A no-op on non-`Block` nodes.
pub(crate) fn fold_multi_assign(block: &mut NormNode) -> Vec<NormNode> {
    if block.kind.as_ref() != kind::BLOCK {
        return Vec::new();
    }
    let children = std::mem::take(&mut block.children);
    let mut out = Vec::with_capacity(children.len());
    let mut originals = Vec::new();
    for child in children {
        match decompose_parallel_assign(&child) {
            Some(seq) => {
                originals.push(child);
                out.extend(seq);
            }
            None => out.push(child),
        }
    }
    block.children = out;
    originals
}

/// If `assign` is a decomposable parallel multi-target assignment — a canonical `Assign` whose
/// children are N target slots (`@place`/`@target`) followed by N value slots (`@value`), with
/// N ≥ 2, all targets simple distinct `Raw` `Var`s — return its minimal single-assign sequence.
/// `None` (leave untouched) for a single assign, a count mismatch (a destructure `a, b = f()` —
/// ANF territory, out of scope), or a non-simple target (`x[i]`, `x.y`) where the name-level
/// read/write analysis would be unsound.
pub(crate) fn decompose_parallel_assign(assign: &NormNode) -> Option<Vec<NormNode>> {
    if assign.kind.as_ref() != kind::ASSIGN {
        return None;
    }
    let mut targets = Vec::new();
    let mut values = Vec::new();
    for c in &assign.children {
        match c.field.as_deref() {
            Some("place") | Some("target") => targets.push(c.clone()),
            Some("value") => values.push(c.clone()),
            _ => return None, // an unexpected slot — not a plain parallel assign
        }
    }
    let n = targets.len();
    if n < 2 || values.len() != n {
        return None;
    }
    // Every target must be a name-addressable simple local (`Raw` `Var`), and all distinct —
    // else the by-name read/write analysis below (and thus the temp placement) is unsound.
    let mut names: Vec<Box<str>> = Vec::with_capacity(n);
    for t in &targets {
        match &t.label {
            Some(Label::Raw(s)) if t.kind.as_ref() == kind::VAR && t.children.is_empty() => {
                names.push(s.clone());
            }
            _ => return None,
        }
    }
    let mut seen = HashSet::new();
    if !names.iter().all(|nm| seen.insert(nm.clone())) {
        return None; // duplicate target name — bail (unsound to sequentialize)
    }
    Some(sequentialize_parallel_assign(
        targets,
        values,
        names,
        assign.span,
    ))
}

/// Minimal-temp parallel-copy sequentialization (the crux). Emits single assigns so that a value
/// is always computed before its target is overwritten; introduces a temp only to break a
/// read-after-write cycle. Deterministic (first-ready-in-index-order; break the lowest-index
/// remaining pair), and identity pairs (`a_i = a_i`, a no-op) drop — so tail-rec reassignment,
/// which carries every param (incl. the unchanged ones), converges with the iterative multi-assign
/// that only names the changed targets.
fn sequentialize_parallel_assign(
    targets: Vec<NormNode>,
    values: Vec<NormNode>,
    names: Vec<Box<str>>,
    span: (u32, u32),
) -> Vec<NormNode> {
    // Drop identity pairs (`a_i = a_i`): a genuine no-op whose target is never disturbed, so it
    // imposes no ordering constraint and dropping it is always sound.
    let mut tgt = Vec::new();
    let mut val = Vec::new();
    let mut name = Vec::new();
    for ((t, v), nm) in targets.into_iter().zip(values).zip(names) {
        if is_bare_var_named(&v, &nm) {
            continue;
        }
        tgt.push(t);
        val.push(v);
        name.push(nm);
    }
    let mut remaining: Vec<usize> = (0..name.len()).collect();
    let mut out = Vec::new();
    let mut temp_ctr = 0u32;
    while !remaining.is_empty() {
        // A pair is READY when overwriting its target corrupts no one: no OTHER remaining value
        // still reads that target's (old) name.
        let ready = remaining.iter().position(|&i| {
            remaining
                .iter()
                .all(|&j| j == i || !mentions_raw(&val[j], &name[i]))
        });
        if let Some(pos) = ready {
            let i = remaining.remove(pos);
            out.push(single_assign(tgt[i].clone(), val[i].clone(), span));
        } else {
            // Deadlock ⇒ a read-after-write cycle. Break it: save the lowest-index remaining
            // target to a fresh temp, then read that temp everywhere it was read — freeing the
            // target to be overwritten (the pair becomes ready next round).
            let i = *remaining.first().expect("remaining is non-empty");
            let temp: Box<str> = format!("__mt{temp_ctr}").into();
            temp_ctr += 1;
            out.push(temp_save(&temp, &name[i], span));
            for &j in &remaining {
                substitute_raw(&mut val[j], &name[i], &temp);
            }
        }
    }
    out
}

/// `target = value` as a single `Assign`, the target keeping its `@place`/`@target` field and the
/// value its `@value` field (both already set on the multi-assign slots we clone from).
fn single_assign(target: NormNode, value: NormNode, span: (u32, u32)) -> NormNode {
    NormNode::new(kind::ASSIGN, None, span, vec![target, value])
}

/// The temp-save `temp = saved` — a fresh `@target` binding (so `abstract_idents` treats `temp`
/// as a declared local) reading the old value of `saved`.
fn temp_save(temp: &str, saved: &str, span: (u32, u32)) -> NormNode {
    let t = NormNode::new(kind::VAR, Some("target"), span, Vec::new())
        .with_label(Label::Raw(temp.into()));
    let v = NormNode::new(kind::VAR, Some("value"), span, Vec::new())
        .with_label(Label::Raw(saved.into()));
    NormNode::new(kind::ASSIGN, None, span, vec![t, v])
}

/// `v` is a bare `Var` whose `Raw` name is `name` (a parallel-assign identity pair `a = a`).
fn is_bare_var_named(v: &NormNode, name: &str) -> bool {
    v.kind.as_ref() == kind::VAR
        && v.children.is_empty()
        && matches!(&v.label, Some(Label::Raw(t)) if t.as_ref() == name)
}

/// Rename every `Raw` `Var` named `from` to `to`, in place (the cycle-breaking substitution).
fn substitute_raw(node: &mut NormNode, from: &str, to: &str) {
    if node.kind.as_ref() == kind::VAR
        && matches!(&node.label, Some(Label::Raw(t)) if t.as_ref() == from)
    {
        node.label = Some(Label::Raw(to.into()));
    }
    for c in &mut node.children {
        substitute_raw(c, from, to);
    }
}

// ---- tail-recursion lowering (spec §5.2.2 Rev 5) ----

/// Rewrite linear tail recursion to a loop: `f(a,b){ …; return f(a',b') }` →
/// `f(a,b){ loop { …; a=a'; b=b'; continue } }`. Called by the frontend's
/// `lower_function` with the function name + simple param names (both from the CST).
/// Bails — returns `body` unchanged — unless EVERY self-call is a block-level tail
/// site (mixed / non-tail recursion, e.g. tree recursion, must NOT lower).
pub fn lower_tail_recursion(
    name: &str,
    params: &[Box<str>],
    body: NormNode,
    log: &mut TransformLog,
) -> NormNode {
    if params.is_empty() {
        return body;
    }
    // Only GENUINE linear tail recursion lowers to a loop: exactly one self-call, and it
    // in tail/return position. Two or more self-calls is tree / branchy recursion (each
    // invocation spawns more than one continuation) — lowering it to a single loop would
    // collapse a tree-recursive shape into the iterative one, so it must stay recursion
    // (spec §7.1 control `ctl-tree-recursion`). The `replaced != total` check below then
    // rejects the remaining non-tail single-call case (a self-call buried in an assignment
    // or a larger expression is not a tail site).
    let total = count_self_calls(&body, name);
    if total != 1 {
        return body; // no self-call, or tree/branchy recursion (≥2) — not linear tail rec
    }
    let mut rewritten = body.clone();
    let mut replaced = 0;
    rewrite_tail_sites(&mut rewritten, name, params, &mut replaced);
    if replaced != total {
        return body; // the lone self-call was not a tail site — not linear recursion
    }
    let span = rewritten.span;
    rewritten.field = Some("body".into());
    let loop_node = NormNode::new(kind::LOOP, None, span, vec![rewritten]);
    log.record(TransformKind::RecursionLower, span, Witness::None);
    NormNode::new(kind::BLOCK, Some("body"), span, vec![loop_node])
}

fn is_raw_ident(node: &NormNode, name: &str) -> bool {
    node.kind.as_ref() == kind::VAR
        && matches!(&node.label, Some(Label::Raw(t)) if t.as_ref() == name)
}

fn is_self_call(node: &NormNode, name: &str) -> bool {
    node.kind.as_ref() == kind::CALL
        && node
            .children
            .iter()
            .find(|c| c.field.as_deref() == Some("callee"))
            .is_some_and(|c| is_raw_ident(c, name))
}

fn count_self_calls(node: &NormNode, name: &str) -> u32 {
    u32::from(is_self_call(node, name))
        + node
            .children
            .iter()
            .map(|c| count_self_calls(c, name))
            .sum::<u32>()
}

fn rewrite_tail_sites(node: &mut NormNode, name: &str, params: &[Box<str>], replaced: &mut u32) {
    if node.kind.as_ref() == kind::BLOCK {
        let mut out = Vec::with_capacity(node.children.len());
        for child in std::mem::take(&mut node.children) {
            let is_return_self = child.kind.as_ref() == kind::RETURN
                && child.children.len() == 1
                && is_self_call(&child.children[0], name);
            if is_return_self {
                let call = child.children.into_iter().next().unwrap();
                out.extend(reassign_stmts(&call, params, replaced));
            } else if is_self_call(&child, name) {
                out.extend(reassign_stmts(&child, params, replaced));
            } else {
                let mut c = child;
                rewrite_tail_sites(&mut c, name, params, replaced);
                out.push(c);
            }
        }
        node.children = out;
    } else {
        for c in &mut node.children {
            rewrite_tail_sites(c, name, params, replaced);
        }
    }
}

/// The **parallel** param-reassignment `(p_0, …, p_k) = (arg_0, …, arg_k)` as one multi-target
/// `Assign{ p*@place, arg*@value }`, followed by `continue`. Emitting a *parallel* assign (not N
/// sequential single assigns) is what makes tail-rec reassignment sound AND convergent: it flows
/// through the SAME [`decompose_parallel_assign`] the iterative `a, b = b, a%b` does, so the two
/// reduce byte-identically (and a coupled swap gets its cycle-breaking temp instead of the old,
/// unsound sequential rewrite). Params are `@place` mutations (matching the iterative form); the
/// decomposition pass drops identity pairs and inserts minimal temps.
fn reassign_stmts(call: &NormNode, params: &[Box<str>], replaced: &mut u32) -> Vec<NormNode> {
    *replaced += 1;
    let span = call.span;
    let args: Vec<&NormNode> = call
        .children
        .iter()
        .filter(|c| c.field.as_deref() == Some("arg"))
        .collect();
    let mut out = Vec::new();
    if args.len() == params.len() {
        let mut children: Vec<NormNode> = params
            .iter()
            .map(|p| {
                NormNode::new(kind::VAR, Some("place"), span, Vec::new())
                    .with_label(Label::Raw(p.clone()))
            })
            .collect();
        for arg in args {
            let mut value = arg.clone();
            value.field = Some("value".into());
            children.push(value);
        }
        out.push(NormNode::new(kind::ASSIGN, None, span, children));
    }
    out.push(NormNode::new(kind::CONTINUE, None, span, Vec::new()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::{lower_python_source, lower_rust_source};
    use crate::ir::render::to_sexpr;

    fn abstracted(src: &str) -> String {
        let (ir, _log) = lower_rust_source(src).expect("a function");
        to_sexpr(&abstract_idents(ir))
    }

    #[test]
    fn one_pass_serves_both_languages() {
        // The SAME abstract_idents runs on Rust and Python IR: renamed params across
        // languages converge to the same canonical form — "one algorithm, all languages".
        let (r, _) = lower_rust_source("fn add(a: i32) { return a + 1; }").unwrap();
        let (p, _) = lower_python_source("def add(x):\n    return x + 1\n").unwrap();
        assert_eq!(to_sexpr(&abstract_idents(r)), to_sexpr(&abstract_idents(p)));
    }

    #[test]
    fn renamed_locals_and_bucketed_literals_converge() {
        // Two Type-2 clones (renamed locals, different literal) → one IR.
        let a = abstracted("fn f(a: i32) { return a + 5; }");
        let b = abstracted("fn g(b: i32) { return b + 7; }");
        assert_eq!(a, b, "renamed locals + bucketed literals must converge");
        assert_eq!(
            a,
            "(Unit (Var@param v0) (Block@body (Return (Binop@value (Var@left v0) (+@op) (Lit@right INT)))))"
        );
    }

    #[test]
    fn trailing_continue_is_stripped() {
        let strip = |src: &str| {
            let (ir, _) = lower_rust_source(src).unwrap();
            to_sexpr(&strip_dead(abstract_idents(ir)))
        };
        assert_eq!(
            strip("fn a() { for x in xs { f(x); continue; } }"),
            strip("fn b() { for x in xs { f(x); } }")
        );
    }

    fn comm_sorted(ir: NormNode, log: &mut TransformLog) -> NormNode {
        let tree = abstract_idents(ir);
        let edits = detect_comm_sort(&tree);
        crate::ir::edit::apply(tree, &edits, log)
    }

    #[test]
    fn commutative_operands_converge() {
        let canon = |src: &str| {
            let (ir, _) = lower_rust_source(src).unwrap();
            to_sexpr(&comm_sorted(ir, &mut TransformLog::disabled()))
        };
        // `a + b` and `b + a` sort to the same canonical order.
        assert_eq!(
            canon("fn f() { return a + b; }"),
            canon("fn g() { return b + a; }")
        );
    }

    #[test]
    fn comm_sort_records_the_original_order() {
        // D-IR-12/D-IR-9: reordering `b + a` → `a + b` emits a CommSort event whose
        // witness records the ORIGINAL operand order, so the sort stays reversible.
        let (ir, _) = lower_rust_source("fn f() { return b + a; }").unwrap();
        let mut log = TransformLog::new();
        let sorted = comm_sorted(ir, &mut log);
        assert!(
            to_sexpr(&sorted).contains("(Var@left a)"),
            "operands should converge to sorted order: {}",
            to_sexpr(&sorted)
        );
        let ev = log
            .events()
            .iter()
            .find(|e| e.kind == TransformKind::CommSort)
            .expect("a comm-sort event in the stream");
        assert_eq!(
            ev.witness,
            Witness::Order(vec![1, 0]),
            "the witness must record the pre-sort operand order"
        );
    }

    #[test]
    fn rust_index_loop_converges_with_foreach() {
        // `for i in 0..xs.len() { g(xs[i]) }` ≡ `for x in xs { g(x) }`.
        let canon = |src: &str| {
            let (ir, _) = lower_rust_source(src).unwrap();
            to_sexpr(&abstract_idents(rewrite_iteration(ir)))
        };
        assert_eq!(
            canon("fn f(xs: &[i32]) { for i in 0..xs.len() { g(xs[i]); } }"),
            canon("fn f(xs: &[i32]) { for x in xs { g(x); } }")
        );
    }

    #[test]
    fn python_index_loop_converges_with_foreach() {
        let canon = |src: &str| {
            let (ir, _) = lower_python_source(src).unwrap();
            to_sexpr(&abstract_idents(rewrite_iteration(ir)))
        };
        assert_eq!(
            canon("def f(xs):\n    for i in range(len(xs)):\n        g(xs[i])\n"),
            canon("def f(xs):\n    for x in xs:\n        g(x)\n")
        );
    }

    #[test]
    fn index_loop_using_the_index_directly_is_not_rewritten() {
        // `h(i)` uses the index for its own sake → must NOT collapse to a foreach.
        let (ir, _) =
            lower_rust_source("fn f(xs: &[i32]) { for i in 0..xs.len() { h(i); } }").unwrap();
        let s = to_sexpr(&abstract_idents(rewrite_iteration(ir)));
        // The range iterator survives (still `__has_next` over the range, not over xs).
        assert!(s.contains("(..@op)"), "index loop wrongly rewritten: {s}");
    }

    #[test]
    fn counter_loop_converges_with_the_range_form() {
        // The block-level C-style counter loop rewrites to the SAME canonical foreach shape the
        // range form (`for i in 0..xs.len()`) produces — so `let mut i = 0; while i < xs.len()
        // { g(xs[i]); i += 1; }` ≡ `for i in 0..xs.len() { g(xs[i]); }` ≡ `for x in xs { g(x); }`.
        let counter = {
            let (ir, _) = lower_rust_source(
                "fn f(xs: &[i32]) { let mut i = 0; while i < xs.len() { g(xs[i]); i += 1; } }",
            )
            .unwrap();
            to_sexpr(&abstract_idents(rewrite_counter_iteration(ir)))
        };
        let range = {
            let (ir, _) =
                lower_rust_source("fn f(xs: &[i32]) { for i in 0..xs.len() { g(xs[i]); } }")
                    .unwrap();
            to_sexpr(&abstract_idents(rewrite_iteration(ir)))
        };
        let foreach = {
            let (ir, _) = lower_rust_source("fn f(xs: &[i32]) { for x in xs { g(x); } }").unwrap();
            to_sexpr(&abstract_idents(ir))
        };
        assert_eq!(
            counter, range,
            "while-index counter must converge with the range form"
        );
        assert_eq!(
            counter, foreach,
            "while-index counter must converge with foreach"
        );
    }

    #[test]
    fn counter_loop_using_the_index_directly_is_not_rewritten() {
        // `h(i)` uses the index for its own sake → the counter loop must NOT collapse.
        let (ir, _) = lower_rust_source(
            "fn f(xs: &[i32]) { let mut i = 0; while i < xs.len() { h(i); i += 1; } }",
        )
        .unwrap();
        let s = to_sexpr(&abstract_idents(rewrite_counter_iteration(ir)));
        assert!(
            !s.contains("__has_next"),
            "counter loop wrongly rewritten: {s}"
        );
    }

    #[test]
    fn counter_iter_records_an_iter_protocol_event() {
        // The counter rewrite records an `IterProtocol` event (shared vocabulary with the range
        // form — the same canonicalization), whose note witness carries the original `coll[ivar]`
        // index form so the block-level splice stays reversible for display (D-IR-9/D-IR-12).
        let (ir, _) = lower_rust_source(
            "fn f(xs: &[i32]) { let mut i = 0; while i < xs.len() { g(xs[i]); i += 1; } }",
        )
        .unwrap();
        let mut log = TransformLog::new();
        let edits = detect_counter_iter(&ir);
        let out = crate::ir::edit::apply(ir, &edits, &mut log);
        let s = to_sexpr(&out);
        assert!(
            s.contains("__has_next") && s.contains("__next"),
            "counter loop not rewritten: {s}"
        );
        let ev = log
            .events()
            .iter()
            .find(|e| e.kind == TransformKind::IterProtocol)
            .expect("an iter-protocol event in the stream");
        assert!(
            matches!(&ev.witness, Witness::Note(n) if n.contains("xs[i]")),
            "witness must carry the index form for reversal: {:?}",
            ev.witness
        );
    }

    #[test]
    fn loop_exit_folds_break_then_return_into_return_at_break() {
        // `loop { if c { break } s() } return E` ≡ `loop { if c { return E } s() }`.
        let fold = |src: &str| {
            let (ir, _) = lower_rust_source(src).unwrap();
            to_sexpr(&abstract_idents(normalize_loop_exit(ir)))
        };
        let with_break = fold("fn f(a: i32) -> i32 { loop { if c() { break; } g(); } return a; }");
        let with_return = fold("fn h(a: i32) -> i32 { loop { if c() { return a; } g(); } }");
        assert_eq!(with_break, with_return);
        // The trailing return is gone and the break became a return.
        assert!(!with_break.contains("(Break)"), "{with_break}");
    }

    #[test]
    fn free_names_stay_external() {
        // a→v0, b→v1 (declared); called functions g/h stay external (by name).
        let s = abstracted("fn f(a: i32) { let b = g(a); return h(b); }");
        assert!(s.contains("(Var@param v0)"), "{s}");
        assert!(s.contains("(Var@target v1)"), "{s}");
        assert!(s.contains("(Var@callee g)"), "{s}");
        assert!(s.contains("(Var@callee h)"), "{s}");
    }

    fn bool_normalized(ir: NormNode, log: &mut TransformLog) -> NormNode {
        let tree = abstract_idents(ir);
        let edits = detect_boolean_normalize(&tree);
        crate::ir::edit::apply(tree, &edits, log)
    }

    #[test]
    fn a1_orient_records_a_reversible_order_witness() {
        // D-IR-9 reversal round-trip: orienting `b > a` → `a < b` emits a CmpOrient event
        // whose `Order([1,0])` witness records the operand swap, so the orientation is
        // recoverable for display (the same discipline as `comm_sort_records_the_original_order`).
        let (ir, _) = lower_rust_source("fn f() -> bool { b > a }").unwrap();
        let mut log = TransformLog::new();
        let oriented = bool_normalized(ir, &mut log);
        let s = to_sexpr(&oriented);
        assert!(s.contains("(<@op)"), "`>` not oriented to `<`: {s}");
        assert!(!s.contains("(>@op)"), "a stray `>` survived: {s}");
        let ev = log
            .events()
            .iter()
            .find(|e| e.kind == TransformKind::CmpOrient)
            .expect("a cmp-orient event in the stream");
        assert_eq!(
            ev.witness,
            Witness::Order(vec![1, 0]),
            "the witness must record the operand swap so reversal restores the orientation"
        );
    }

    #[test]
    fn a2_not_push_records_notpush_then_orient() {
        // `!(a < b)` reduces in two recorded steps: NotPush (invert `<`→`>=`, witness None —
        // reversal re-wraps the `!`) then CmpOrient (orient `>=`→`<=`). The event pair is what
        // makes the reduction reversible for display (D-IR-9).
        let (ir, _) = lower_rust_source("fn f() -> bool { !(a < b) }").unwrap();
        let mut log = TransformLog::new();
        let out = bool_normalized(ir, &mut log);
        let s = to_sexpr(&out);
        assert!(!s.contains("(Unop"), "the `!` was not pushed away: {s}");
        assert!(s.contains("(<=@op)"), "did not settle at `<=`: {s}");
        let kinds: Vec<_> = log.events().iter().map(|e| e.kind).collect();
        assert!(
            kinds.contains(&TransformKind::NotPush) && kinds.contains(&TransformKind::CmpOrient),
            "expected NotPush then CmpOrient, got {kinds:?}"
        );
        let np = log
            .events()
            .iter()
            .find(|e| e.kind == TransformKind::NotPush)
            .expect("a not-push event");
        assert_eq!(
            np.witness,
            Witness::None,
            "NotPush is bijective (no witness)"
        );
    }

    #[test]
    fn c1_merge_records_a_guard_merge_event() {
        // The nested-if merge emits a GuardMerge event keyed to the outer branch, whose
        // depth witness lets reversal re-nest the guards (D-IR-9). The fold cannot happen
        // silently — a merged tree implies a recorded event (the CQRS write-side guarantee).
        let (ir, _) = lower_rust_source("fn f() { if a { if b { g(); } } }").unwrap();
        let mut log = TransformLog::new();
        let merged = crate::ir::edit::apply(ir.clone(), &detect_guard_canonicalize(&ir), &mut log);
        let s = to_sexpr(&merged);
        assert!(s.contains("(&&@op)"), "guards not merged into `&&`: {s}");
        assert_eq!(
            s.matches("Branch").count(),
            1,
            "the nesting should collapse to a single Branch: {s}"
        );
        assert!(
            log.events()
                .iter()
                .any(|e| e.kind == TransformKind::GuardMerge),
            "guard-merge event missing"
        );
    }

    #[test]
    fn c2_drop_else_records_the_hoisted_body_witness() {
        // Dropping a redundant else emits a DeadElse event keyed to the enclosing block, whose
        // witness is the hoisted else-body — enough to re-wrap it as the else arm on reversal
        // (D-IR-9). The block-level rebuild cannot happen without the recorded event.
        let (ir, _) =
            lower_rust_source("fn f(c: bool) -> i32 { if c { return 1; } else { g(); } h() }")
                .unwrap();
        let mut log = TransformLog::new();
        let out = crate::ir::edit::apply(ir.clone(), &detect_guard_canonicalize(&ir), &mut log);
        let s = to_sexpr(&out);
        // One arm left on the branch (the else is gone) and `g()` now sits as a sibling.
        assert_eq!(s.matches("(Arm").count(), 1, "else arm not dropped: {s}");
        let ev = log
            .events()
            .iter()
            .find(|e| e.kind == TransformKind::DeadElse)
            .expect("a dead-else event");
        assert!(
            matches!(&ev.witness, Witness::Note(n) if n.contains("g")),
            "witness must carry the hoisted else body: {:?}",
            ev.witness
        );
    }

    #[test]
    fn diverges_recognizes_terminators_and_total_branches() {
        // The C2 soundness helper: a trailing return/break/continue diverges; a total if/else
        // whose arms all diverge diverges; a plain expression tail does not.
        let body = |src: &str| {
            let (ir, _) = lower_rust_source(src).unwrap();
            // Unit → [.. params.., Block@body]; the body block is the last child.
            ir.children.into_iter().next_back().unwrap()
        };
        assert!(
            diverges(&body("fn f() -> i32 { g(); return 1; }")),
            "return"
        );
        assert!(!diverges(&body("fn f() { g(); }")), "plain tail");
        assert!(
            diverges(&body(
                "fn f() -> i32 { if c { return 1; } else { return 2; } }"
            )),
            "total diverging branch"
        );
        // A no-else `if` followed by a `;`-terminated statement does not diverge (the tail
        // `h();` is a statement, not the implicit-return tail expression `h()` would be).
        assert!(
            !diverges(&body("fn f() { if c { return; } h(); }")),
            "a no-else branch does not guarantee divergence"
        );
    }

    #[test]
    fn independent_multi_assign_decomposes_without_a_temp() {
        // `x, y = x+1, y*2` has no cross-dependency → NO temp → byte-identical to the two
        // adjacent single assigns (the "identifiable as two adjacent assigns" requirement).
        let parallel = {
            let (ir, _) =
                lower_python_source("def f(x, y):\n    x, y = x + 1, y * 2\n    return x\n")
                    .unwrap();
            to_sexpr(&abstract_idents(decompose_multi_assign(ir)))
        };
        let adjacent = {
            let (ir, _) =
                lower_python_source("def f(x, y):\n    x = x + 1\n    y = y * 2\n    return x\n")
                    .unwrap();
            to_sexpr(&abstract_idents(ir))
        };
        assert_eq!(
            parallel, adjacent,
            "independent parallel assign must equal two adjacent assigns"
        );
        // Only the two params exist as locals — no synthetic temp introduced.
        assert!(!parallel.contains("v2"), "no temp expected: {parallel}");
    }

    #[test]
    fn coupled_multi_assign_inserts_one_cycle_breaking_temp() {
        // The gcd swap `a, b = b, a%b` is a read-after-write cycle → exactly ONE temp:
        // `t = a; a = b; b = t%b` (t is the third positional local, v2).
        let (ir, _) =
            lower_python_source("def f(a, b):\n    a, b = b, a % b\n    return a\n").unwrap();
        let s = to_sexpr(&abstract_idents(decompose_multi_assign(ir)));
        assert_eq!(
            s.matches("(Assign").count(),
            3,
            "expected 3 single assigns: {s}"
        );
        assert!(
            s.contains("(Assign (Var@target v2) (Var@value v0))"),
            "temp save `t = a` missing: {s}"
        );
        assert!(
            s.contains("(Assign (Var@place v0) (Var@value v1))"),
            "`a = b` missing: {s}"
        );
        assert!(
            s.contains("(Var@left v2) (%@op) (Var@right v1)"),
            "`b = t % b` (temp read) missing: {s}"
        );
    }

    #[test]
    fn sequential_single_assigns_are_left_untouched() {
        // Discrimination at the pass level: two adjacent single assigns are NOT a parallel
        // multi-assign — the decomposition must not fire (so `a=b; b=a%b` stays distinct from
        // the temped parallel form).
        let (ir, _) =
            lower_python_source("def f(a, b):\n    a = b\n    b = a % b\n    return a\n").unwrap();
        let before = to_sexpr(&ir);
        let after = to_sexpr(&decompose_multi_assign(ir));
        assert_eq!(before, after, "single assigns must be untouched: {after}");
    }

    #[test]
    fn multi_assign_decomposition_records_a_reversible_witness() {
        // D-IR-9/D-IR-12 reversal round-trip: decomposing a parallel assign records a
        // `MultiAssign` event whose witness is the ORIGINAL multi-assign — enough to restore it
        // for display. The block-level splice cannot happen without the recorded event.
        let (ir, _) =
            lower_python_source("def f(a, b):\n    a, b = b, a % b\n    return a\n").unwrap();
        let mut log = TransformLog::new();
        let edits = detect_multi_assign(&ir);
        let out = crate::ir::edit::apply(ir, &edits, &mut log);
        assert_eq!(
            to_sexpr(&out).matches("(Assign").count(),
            3,
            "not decomposed"
        );
        let ev = log
            .events()
            .iter()
            .find(|e| e.kind == TransformKind::MultiAssign)
            .expect("a multi-assign event in the stream");
        // The witness carries the original parallel `Assign` (two `@place` targets, the `%`),
        // so display reversal reconstructs the multi-assign.
        assert!(
            matches!(&ev.witness, Witness::Note(n)
                if n.contains("@place") && n.contains("(%@op)") && n.matches("@value").count() == 2),
            "witness must carry the original multi-assign: {:?}",
            ev.witness
        );
    }

    #[test]
    fn detect_emit_completes_the_event_stream() {
        // Completeness (§15.3, D-IR-12): the converted canonicalization passes now emit their
        // events under a recording sink, where the historical inlined passes were silent.
        let (ir, _) = lower_rust_source("fn a() { for x in xs { f(x); continue; } }").unwrap();
        let mut log = TransformLog::new();
        let t = crate::ir::edit::apply(ir.clone(), &detect_abstract_idents(&ir), &mut log);
        let _ = crate::ir::edit::apply(t.clone(), &detect_dead(&t), &mut log);
        assert!(
            log.events()
                .iter()
                .any(|e| e.kind == TransformKind::AbstractIdents),
            "abstract-idents event missing"
        );
        assert!(
            log.events()
                .iter()
                .any(|e| e.kind == TransformKind::DeadStrip),
            "dead-strip event missing"
        );
    }
}
