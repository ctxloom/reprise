//! The canonicalization seam (`docs/SIMILARITY-IR.md` §15.3, D-IR-12): every
//! front-half normalization is a described [`Edit`], and [`apply`] is the *only*
//! thing that performs one — recording its [`TransformLog`] event in the same step.
//!
//! The structural guarantee lives in the detector signature: a detector is
//! `fn detect_*(&NormNode) -> Vec<Edit>` (see [`crate::ir::pass`]), with no `&mut`,
//! so a pass *cannot* mutate the tree without routing through here — "sort without
//! recording" does not type-check. The log and witnesses are hash-excluded, so
//! completing the event stream never moves the canonical tree (the parity-neutral
//! invariant): threading a [`TransformLog::disabled`] sink makes recording a no-op
//! and yields the identical tree.

use crate::ir::pass;
use crate::ir::render::to_sexpr;
use crate::ir::transform::{TransformKind, TransformLog, Witness};
use crate::tree::NormNode;
use std::collections::HashMap;

/// A described mutation. Loci are source byte spans, never child indices (indices
/// shift under sibling removals/reorders — `docs/transform-seam.md` §6). Detectors
/// emit these; only [`apply`] performs them, recording the matching event.
pub enum Edit {
    /// Reorder a commutative operator chain into canonical (sorted) order. `order`
    /// is the sorting permutation — `order[k]` is the pre-sort index of the operand
    /// now at position `k` — so it doubles as the witness that restores the ORIGINAL
    /// order on reversal (D-IR-9).
    CommSort { locus: (u32, u32), order: Vec<u32> },
    /// Rewrite a canonical index loop (`for i in 0..len(xs)` using `i` only as `xs[i]`)
    /// to iterate `xs` directly: swap the range iterator for `coll` in both protocol
    /// calls and every `coll[ivar]` for a bare `ivar`. `coll`/`ivar` drive the mutation
    /// and are the witness (the original `coll[ivar]` index form).
    IterProtocol {
        locus: (u32, u32),
        coll: Box<str>,
        ivar: Box<str>,
    },
    /// Rewrite a block-level C-style counter loop (`i = 0; while i < len(coll) { … coll[i] …;
    /// i = i + 1 }`) to the canonical iteration-protocol foreach form — the SAME
    /// canonicalization as [`IterProtocol`](Edit::IterProtocol), reached from a different source
    /// syntax (so it records a `TransformKind::IterProtocol` event). `locus` is the ENCLOSING
    /// `Block` (a block-level splice: the init sibling is dropped, the loop rewritten — like
    /// `LoopExit`); `coll`/`ivar` drive the synthesis and are the witness.
    CounterIter {
        locus: (u32, u32),
        coll: Box<str>,
        ivar: Box<str>,
    },
    /// Fold a block tail `Loop{…}; Return{E}` by replacing the loop's top-level bare
    /// breaks with `Return{E}` and dropping the trailing return. Locus = the block.
    LoopExit { locus: (u32, u32) },
    /// Remove a dead node (a redundant trailing `Continue` at a loop-body tail). Locus =
    /// the removed node; its subtree is materialized as the witness only when recording.
    DropDead { locus: (u32, u32) },
    /// Relabel every `Raw` identifier whole-tree: a declared local becomes `Local(n)` by
    /// the positional `map` (first occurrence), any other name becomes `External`. One
    /// edit for the whole pass (positional / global — the D2 cascade); the map is both the
    /// mutation input and the witness (position→original-name).
    AbstractIdents { map: Vec<(u32, Box<str>)> },
    /// Orient an ordered comparison to `<`/`<=` (Family A1): swap `left`/`right` and flip
    /// the operator (`>`→`<`, `>=`→`<=`). Bijective; the mutation is re-derived from the
    /// tree at `locus`, and reversal restores the orientation from the `Order([1,0])` witness.
    CmpOrient { locus: (u32, u32) },
    /// Push a logical negation inward (Family A2): comparison inversion (`!(a<b)`→`a>=b`),
    /// De Morgan (`!(a&&b)`→`!a||!b`), or double-negation (`!!x`→`x`). Re-derived from the
    /// tree at `locus`; reversal re-wraps the `!` (bijective, no discriminator witness).
    NotPush { locus: (u32, u32) },
    /// Merge a nested single-arm, no-else `if a { if b { … } }` into one `if a && b { … }`
    /// (Family C1). Re-derived from the tree at `locus` (the outer `Branch`); reversal re-nests
    /// the two guards from the recorded depth. `op_tok` is the conjunction token to synthesize —
    /// recorded on the edit (like `CounterIter`/`AbstractIdents` carry witness data) so the
    /// lang-less applier can REPLAY the language's spelling (`&&` / Python `and`) without a `Lang`
    /// ever reaching [`apply`]: the detector learns the language, the applier replays the token.
    GuardMerge { locus: (u32, u32), op_tok: Box<str> },
    /// Drop a redundant `else` after a diverging then-arm and splice its body as siblings
    /// (Family C2). `locus` is the ENCLOSING `Block` (the splice target — a block-level rebuild,
    /// like `LoopExit`); the hoisted else-body rides in the witness so reversal re-wraps it.
    DropElse { locus: (u32, u32) },
    /// Decompose a parallel multi-target `Assign{ t*, v* }` into a minimal sequence of single
    /// assigns, inserting a temp only to break a read-after-write cycle. `locus` is the
    /// ENCLOSING `Block` (a block-level splice — the multi-assign is replaced by the sequence,
    /// like `CounterIter`); the original multi-assign rides in the witness so reversal restores it.
    MultiAssign { locus: (u32, u32) },
}

/// The sole front-half canonicalization mutator: perform each described edit AND
/// record its event, atomically. A [`TransformLog::disabled`] sink turns recording
/// into a no-op that allocates nothing, without changing the resulting tree.
///
/// One call carries a single pass's edits (all the same variant — the pipeline runs
/// `apply(tree, &detect_x(&tree), log)` per pass), so the first edit selects the walk.
pub fn apply(tree: NormNode, edits: &[Edit], log: &mut TransformLog) -> NormNode {
    let Some(first) = edits.first() else {
        return tree;
    };
    let mut edits = edits.iter();
    match first {
        Edit::CommSort { .. } => apply_comm_sort(tree, &mut edits, log),
        Edit::IterProtocol { .. } => apply_iter_protocol(tree, &mut edits, log),
        Edit::CounterIter { .. } => {
            // The counter's liveness check is function-scoped, so `fold_counter_loops` needs the
            // whole unit `root`. The applier walks/mutates the tree bottom-up (no immutable root
            // in hand), so snapshot it once at entry — identical to the tree the detector saw, so
            // both make the same rewrite decisions. Only reached when there IS a counter edit.
            let root = tree.clone();
            apply_counter_iter(tree, &root, &mut edits, log)
        }
        Edit::LoopExit { .. } => apply_loop_exit(tree, &mut edits, log),
        Edit::DropDead { .. } => apply_drop_dead(tree, &mut edits, log),
        Edit::AbstractIdents { .. } => apply_abstract_idents(tree, &mut edits, log),
        Edit::CmpOrient { .. } | Edit::NotPush { .. } => {
            apply_boolean_normalize(tree, &mut edits, log)
        }
        Edit::GuardMerge { .. } | Edit::DropElse { .. } => {
            apply_guard_canonicalize(tree, &mut edits, log)
        }
        Edit::MultiAssign { .. } => apply_multi_assign(tree, &mut edits, log),
    }
}

/// Replay the parallel multi-assign decomposition: a bottom-up walk that re-detects each site
/// via the shared [`pass::fold_multi_assign`] (block-level, like `apply_counter_iter`) and, per
/// decomposed multi-assign, consumes the matching edit and records a `MultiAssign` event whose
/// witness is the original multi-assign — so the block-level splice stays reversible for display.
fn apply_multi_assign(
    node: NormNode,
    edits: &mut std::slice::Iter<'_, Edit>,
    log: &mut TransformLog,
) -> NormNode {
    let mut node = node;
    node.children = node
        .children
        .into_iter()
        .map(|c| apply_multi_assign(c, edits, log))
        .collect();
    let locus = node.span;
    for original in pass::fold_multi_assign(&mut node) {
        let Some(Edit::MultiAssign { .. }) = edits.next() else {
            unreachable!("apply routed a non-MultiAssign edit into apply_multi_assign");
        };
        if log.enabled() {
            log.record(
                TransformKind::MultiAssign,
                locus,
                Witness::Note(to_sexpr(&original).into()),
            );
        }
    }
    node
}

/// Replay Family C's guard canonicalization (C1 conjunction-merge + C2 redundant-else): a
/// bottom-up walk that re-detects each site via the shared [`pass::fold_dead_else`] /
/// [`pass::try_merge_branch`] and, where one fires, consumes the matching edit and records the
/// event. Mirrors `apply_loop_exit` — the detector and this walk both drive the same folds, so a
/// firing node is exactly a detected site and the loci stay valid without index-addressing (§6).
/// C2 (block-level) is checked before C1 (branch-level) — a node is one or the other, never both.
fn apply_guard_canonicalize(
    node: NormNode,
    edits: &mut std::slice::Iter<'_, Edit>,
    log: &mut TransformLog,
) -> NormNode {
    let mut node = node;
    node.children = node
        .children
        .into_iter()
        .map(|c| apply_guard_canonicalize(c, edits, log))
        .collect();
    // C2 · redundant-else: a block-level rebuild, one edit per else dropped.
    let locus = node.span;
    for hoisted in pass::fold_dead_else(&mut node) {
        let Some(Edit::DropElse { .. }) = edits.next() else {
            unreachable!("apply routed a non-DropElse edit into apply_guard_canonicalize");
        };
        if log.enabled() {
            log.record(
                TransformKind::DeadElse,
                locus,
                Witness::Note(to_sexpr(&hoisted).into()),
            );
        }
    }
    // C1 · conjunction-merge: a branch-level fold, one edit per merged nest. Mergeability is
    // SHAPE-ONLY (independent of the token), so peek the shape first, then consume the edit and
    // REPLAY its recorded `op_tok` — the applier never learns the `Lang`, it just spends the
    // token the detector wrote down.
    if pass::is_mergeable_branch(&node) {
        let Some(Edit::GuardMerge { locus, op_tok }) = edits.next() else {
            unreachable!("apply routed a non-GuardMerge edit into apply_guard_canonicalize");
        };
        let merged =
            pass::try_merge_branch(&node, op_tok).expect("mergeability shape checked above");
        if log.enabled() {
            log.record(
                TransformKind::GuardMerge,
                *locus,
                Witness::Note("depth=2".into()),
            );
        }
        return merged;
    }
    node
}

/// Replay Family A's boolean normalization (A1 orient, A2 not-push): the pure
/// [`pass::bool_normalize`] core produces the normalized tree AND the ordered reduction
/// steps; because the detector drove the *same* core to emit the routed edits, the steps
/// line up with `edits` one-for-one — consume each (the seam's per-site accounting) and
/// record its event. The mutation is re-derived here (the edits carry only a locus), which
/// is what lets A2's fixpoint compose without index-addressing.
fn apply_boolean_normalize(
    tree: NormNode,
    edits: &mut std::slice::Iter<'_, Edit>,
    log: &mut TransformLog,
) -> NormNode {
    let mut steps = Vec::new();
    let out = pass::bool_normalize(tree, &mut steps);
    for (kind, locus, witness) in steps {
        let consumed = edits.next();
        debug_assert!(
            matches!(
                consumed,
                Some(Edit::CmpOrient { .. } | Edit::NotPush { .. })
            ),
            "apply routed a non-boolean edit into apply_boolean_normalize"
        );
        if log.enabled() {
            log.record(kind, locus, witness);
        }
    }
    out
}

fn apply_comm_sort(
    node: NormNode,
    edits: &mut std::slice::Iter<'_, Edit>,
    log: &mut TransformLog,
) -> NormNode {
    if pass::is_commutative_chain(&node) {
        let op = node.children[1].clone();
        let mut operands = Vec::new();
        pass::flatten_chain(&node, op.kind.as_ref(), &mut operands);
        let operands: Vec<NormNode> = operands
            .into_iter()
            .map(|o| apply_comm_sort(o, edits, log))
            .collect();
        if operands.len() >= 2 {
            let span = node.span;
            // Recompute the sort to decide whether the detector emitted an edit for
            // this chain: an identity permutation is already canonical, so the detector
            // skipped it and there is NO edit to consume (peeking `edits.next()` here
            // would steal a later chain's edit — the detector and applier must agree on
            // which chains produce an edit). `to_sexpr` (the sort key) excludes spans,
            // so this recomputation matches the detector's on the identical operands.
            let recomputed = pass::sort_perm(&operands);
            if pass::order_is_identity(&recomputed) {
                // No reorder, no edit: rebuild in place, preserving the original span.
                return pass::rebuild_chain(operands, op, node.field, span);
            }
            let Some(Edit::CommSort { locus, order }) = edits.next() else {
                unreachable!("apply routed a non-CommSort edit into apply_comm_sort");
            };
            let sorted = order
                .iter()
                .map(|&i| operands[i as usize].clone())
                .collect();
            if log.enabled() {
                log.record(
                    TransformKind::CommSort,
                    *locus,
                    Witness::Order(order.clone()),
                );
            }
            return pass::rebuild_chain(sorted, op, node.field, span);
        }
    }
    let mut node = node;
    node.children = node
        .children
        .into_iter()
        .map(|c| apply_comm_sort(c, edits, log))
        .collect();
    node
}

fn apply_iter_protocol(
    node: NormNode,
    edits: &mut std::slice::Iter<'_, Edit>,
    log: &mut TransformLog,
) -> NormNode {
    let mut node = node;
    node.children = node
        .children
        .into_iter()
        .map(|c| apply_iter_protocol(c, edits, log))
        .collect();
    // Re-identify the site the detector matched (an index `Loop`), consuming one edit
    // in the same post-order so loci stay valid without index-addressing (§6).
    let matched = (node.kind.as_ref() == crate::ir::kind::LOOP)
        .then(|| {
            node.children
                .iter()
                .find(|c| c.field.as_deref() == Some("body"))
                .and_then(pass::index_loop_match)
        })
        .flatten();
    if matched.is_some() {
        let Some(Edit::IterProtocol { locus, coll, ivar }) = edits.next() else {
            unreachable!("apply routed a non-IterProtocol edit into apply_iter_protocol");
        };
        pass::rewrite_index_loop(&mut node, ivar, coll, *locus);
        if log.enabled() {
            log.record(
                TransformKind::IterProtocol,
                *locus,
                Witness::Note(format!("{coll}[{ivar}]").into()),
            );
        }
    }
    node
}

/// Replay the C-style counter-loop rewrite: a bottom-up walk that re-detects each site via the
/// shared [`pass::fold_counter_loops`] (block-level, like `apply_loop_exit`) and, per rewritten
/// loop, consumes the matching edit and records an `IterProtocol` event — the counter loop and
/// the range-protocol loop are the same canonicalization, so they share the event vocabulary.
fn apply_counter_iter(
    node: NormNode,
    root: &NormNode,
    edits: &mut std::slice::Iter<'_, Edit>,
    log: &mut TransformLog,
) -> NormNode {
    let mut node = node;
    node.children = node
        .children
        .into_iter()
        .map(|c| apply_counter_iter(c, root, edits, log))
        .collect();
    let locus = node.span;
    for (coll, ivar, _span) in pass::fold_counter_loops(&mut node, root, log.label_interner()) {
        let Some(Edit::CounterIter { .. }) = edits.next() else {
            unreachable!("apply routed a non-CounterIter edit into apply_counter_iter");
        };
        if log.enabled() {
            log.record(
                TransformKind::IterProtocol,
                locus,
                Witness::Note(format!("{coll}[{ivar}]").into()),
            );
        }
    }
    node
}

fn apply_loop_exit(
    node: NormNode,
    edits: &mut std::slice::Iter<'_, Edit>,
    log: &mut TransformLog,
) -> NormNode {
    let mut node = node;
    node.children = node
        .children
        .into_iter()
        .map(|c| apply_loop_exit(c, edits, log))
        .collect();
    if node.kind.as_ref() == crate::ir::kind::BLOCK {
        let locus = node.span;
        // `fold_loop_exit` performs the fold in place and reports the fold COUNT (it folds to a
        // fixpoint, so a nested loop-exit yields more than one). Each fold is exactly one
        // detected site, so it consumes that many edits — detector and applier stay in lockstep.
        for _ in 0..pass::fold_loop_exit(&mut node) {
            let Some(Edit::LoopExit { locus: _ }) = edits.next() else {
                unreachable!("apply routed a non-LoopExit edit into apply_loop_exit");
            };
            if log.enabled() {
                log.record(TransformKind::LoopExit, locus, Witness::None);
            }
        }
    }
    node
}

fn apply_drop_dead(
    node: NormNode,
    edits: &mut std::slice::Iter<'_, Edit>,
    log: &mut TransformLog,
) -> NormNode {
    let mut node = node;
    node.children = node
        .children
        .into_iter()
        .map(|c| apply_drop_dead(c, edits, log))
        .collect();
    if node.kind.as_ref() == crate::ir::kind::LOOP
        && let Some(body) = node
            .children
            .iter_mut()
            .find(|c| c.field.as_deref() == Some("body"))
    {
        while body
            .children
            .last()
            .is_some_and(|c| c.kind.as_ref() == crate::ir::kind::CONTINUE)
        {
            let removed = body.children.pop().expect("checked last() is Some");
            let Some(Edit::DropDead { locus }) = edits.next() else {
                unreachable!("apply routed a non-DropDead edit into apply_drop_dead");
            };
            if log.enabled() {
                // The removed subtree is the witness — materialized only when recording.
                log.record(
                    TransformKind::DeadStrip,
                    *locus,
                    Witness::Note(to_sexpr(&removed).into()),
                );
            }
        }
    }
    node
}

fn apply_abstract_idents(
    node: NormNode,
    edits: &mut std::slice::Iter<'_, Edit>,
    log: &mut TransformLog,
) -> NormNode {
    let Some(Edit::AbstractIdents { map }) = edits.next() else {
        unreachable!("apply routed a non-AbstractIdents edit into apply_abstract_idents");
    };
    let locals: HashMap<&str, u32> = map.iter().map(|(n, name)| (name.as_ref(), *n)).collect();
    let mut node = node;
    pass::relabel_from_map(&mut node, &locals, log.label_interner());
    if log.enabled() {
        let span = node.span;
        let witness: Vec<String> = map.iter().map(|(n, name)| format!("v{n}={name}")).collect();
        log.record(
            TransformKind::AbstractIdents,
            span,
            Witness::Note(witness.join(" ").into()),
        );
    }
    node
}
