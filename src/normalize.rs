//! P1–P3: parsing, unit extraction, and normalization (spec §5.1–§5.2).
//!
//! Phase-1 pass set (DECISIONS.md D5): strip (during conversion), loop
//! decomposition, identifier abstraction, literal abstraction, dead-syntax
//! removal. Desugaring (§5.2.3) and order canonicalization (§5.2.6) are Phase 2.
//!
//! Passes are idempotent by construction: conversion produces transient
//! `Raw`/`RawLit` labels; each pass only rewrites what a previous application
//! would already have consumed.

use crate::config::Config;
use crate::intern::{Field, Kind};
use crate::lang::{Lang, LanguageProfile};
use crate::tree::{Bucket, Label, NormNode};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

static PASS_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("pass_statement"));
static ELSE_CLAUSE: LazyLock<Kind> = LazyLock::new(|| Kind::intern("else_clause"));
static BLOCK_LOWER: LazyLock<Kind> = LazyLock::new(|| Kind::intern("block"));
static BODY_FIELD: LazyLock<Field> = LazyLock::new(|| Field::intern("body"));

/// CST-recursion depth cap for extraction — the untrusted-input DoS guard, shared by all
/// three recursive descents of the parsed CST: the two unit-*finders* ([`collect_units`] and
/// the IR path's `collect_ir_units`) and the two unit-*lowerers* ([`convert`] and the IR
/// frontend's `lower_node` wrapper, [`crate::frontend`]). Scanned source is untrusted: a
/// generated/minified file with a several-thousand-term left-assoc operator chain (`a+b+c+…`)
/// or deeply nested literals parses to a CST that deep, and our recursive descent of it
/// (tree-sitter's own parse is iterative and safe) would exhaust a rayon worker's default
/// 2 MiB stack and SIGSEGV the whole scan. Past this depth we stop descending — the lowerers
/// truncate the subtree and flag the unit `parse_degraded` (never a silent drop, spec
/// §5.1/§12); the finders stop searching (a unit nested this deep is itself pathological).
/// Every downstream walk (passes, fold, fingerprint) then runs on a ≤-cap-deep tree and
/// cannot overflow either — this is the ONE guard the whole pipeline needs.
///
/// Value: 150. Two forces set it. Ceiling: real hand-written code essentially never nests
/// expressions/blocks past a few dozen levels (deeply *generated* code is wide, not deep), so
/// 150 (~3× the deepest realistic source) never fires on genuine input; a rare over-150 unit
/// still gets indexed + flagged, so even a false trigger only degrades, never drops. Floor:
/// extraction must be safe on the smallest stack that ever runs it — rayon's 2 MiB default
/// workers AND library-API callers on 2 MiB spawned threads — and in a *debug* build the IR
/// lowering burns ~4 `NormNode`-holding frames per CST level, so the empirical 2 MiB overflow
/// cliff sits near ~250–300 levels (measured: 200 survives, 350 overflows). 150 keeps a ~2×
/// margin under that cliff, covering debug frames, cross-grammar/-platform variance, and the
/// worst case where a deep finder descent and a deep lowering descent stack.
pub const MAX_EXTRACTION_DEPTH: u32 = 150;

pub struct RawUnit {
    pub name: String,
    pub byte_span: (u32, u32),
    pub line_span: (u32, u32),
    pub parse_degraded: bool,
    pub is_test: bool,
    pub tree: NormNode,
}

/// Parse a source file and extract every function-like unit as a raw
/// (pre-pass) normalized tree. Files with ERROR nodes are still processed
/// (spec §5.1): units are flagged, never dropped.
pub fn extract_raw_units(src: &str, lang: Lang, path: &std::path::Path) -> Vec<RawUnit> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&lang.ts_language()).is_err() {
        return Vec::new();
    }
    let Some(cst) = parser.parse(src, None) else {
        return Vec::new();
    };
    let profile = lang.profile();
    let mut units = Vec::new();
    collect_units(cst.root_node(), src, profile, path, &mut units, 0);
    units
}

/// Test-facing convenience: (unit name, raw tree) pairs.
pub fn raw_units_from_source(src: &str, lang: Lang) -> Vec<(String, NormNode)> {
    extract_raw_units(src, lang, std::path::Path::new("memory.in"))
        .into_iter()
        .map(|u| (u.name, u.tree))
        .collect()
}

/// `depth` bounds the unit-*finder*'s own CST descent (see the IR path's `collect_ir_units`):
/// this recursion hunts every named child for nested units and would itself overflow on a
/// pathologically deep file. Past [`MAX_EXTRACTION_DEPTH`] we stop searching deeper.
fn collect_units(
    node: tree_sitter::Node,
    src: &str,
    profile: &dyn LanguageProfile,
    path: &std::path::Path,
    out: &mut Vec<RawUnit>,
    depth: u32,
) {
    if profile.is_function_like(node.kind()) {
        let name = node
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(src.as_bytes()).ok())
            .unwrap_or("<anon>")
            .to_string();
        push_unit(node, name, src, profile, path, out);
    } else if let Some((name, callable)) = profile.binding_unit(node, src) {
        // A named callable bound in a declaration (`const f = () => …`). The
        // callable node is the unit root; the binding name rides along for
        // reporting/inliner naming. The recursion still descends into the
        // callable body, so a nested binding is extracted too.
        push_unit(callable, name, src, profile, path, out);
    }
    if depth >= MAX_EXTRACTION_DEPTH {
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_units(child, src, profile, path, out, depth + 1);
    }
}

/// Convert `node` and, if it yields a tree, push it as a `RawUnit` named
/// `name`. Shared by the `is_function_like` and `binding_unit` extraction
/// paths (spec §5.1).
fn push_unit(
    node: tree_sitter::Node,
    name: String,
    src: &str,
    profile: &dyn LanguageProfile,
    path: &std::path::Path,
    out: &mut Vec<RawUnit>,
) {
    let mut truncated = false;
    if let Some(tree) = convert(node, None, "", src, profile, 0, &mut truncated) {
        out.push(RawUnit {
            is_test: profile.unit_is_test(node, src, &name, path),
            name,
            byte_span: (node.start_byte() as u32, node.end_byte() as u32),
            line_span: (
                node.start_position().row as u32 + 1,
                node.end_position().row as u32 + 1,
            ),
            // A depth-truncated unit is flagged like a parse-degraded one (never dropped):
            // both mean the extracted tree is not a faithful, complete lowering (spec §5.1/§12).
            parse_degraded: node.has_error() || truncated,
            tree,
        });
    }
}

/// CST → raw normalized tree. Performs spec §5.2.1 (strip) and the
/// paren-flattening half of §5.2.7 on the way through.
///
/// `depth` is the current CST-recursion depth and `truncated` the out-signal for the
/// untrusted-input DoS guard ([`MAX_EXTRACTION_DEPTH`]): past the cap we stop descending
/// and drop the over-deep subtree (returning `None`, which every caller already treats as a
/// dropped node), setting `*truncated` so the unit is flagged `parse_degraded`.
fn convert(
    node: tree_sitter::Node,
    field: Option<&str>,
    parent_kind: &str,
    src: &str,
    profile: &dyn LanguageProfile,
    depth: u32,
    truncated: &mut bool,
) -> Option<NormNode> {
    if depth >= MAX_EXTRACTION_DEPTH {
        *truncated = true;
        return None;
    }
    let kind = node.kind();
    let span = (node.start_byte() as u32, node.end_byte() as u32);
    let text = || node.utf8_text(src.as_bytes()).unwrap_or_default();

    if !node.is_named() {
        // Anonymous tokens are punctuation/keywords implied by the parent kind,
        // except operators, which are semantic (kept as their own kind).
        if profile.keep_anon_parent(parent_kind) {
            return Some(NormNode::token(text(), span));
        }
        return None;
    }
    if profile.is_comment(kind) || profile.strip_kind(kind) {
        return None;
    }
    if profile.unwrap_kind(kind) {
        // Redundant grouping: replace with the inner expression, which inherits
        // the wrapper's field.
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if let Some(inner) = convert(
                child,
                field,
                parent_kind,
                src,
                profile,
                depth + 1,
                truncated,
            ) {
                return Some(inner);
            }
        }
        return None;
    }
    if profile.literal_bucket(kind).is_some() {
        // Literals are leaves: their internal structure (quote tokens, content
        // pieces) is irrelevant once bucketed.
        return Some(
            NormNode::new(kind, field, span, Vec::new()).with_label(Label::RawLit(text().into())),
        );
    }
    if profile.is_identifier(kind) {
        return Some(
            NormNode::new(kind, field, span, Vec::new()).with_label(Label::Raw(text().into())),
        );
    }

    let mut children = Vec::new();
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            let child_field = cursor.field_name();
            if let Some(converted) =
                convert(child, child_field, kind, src, profile, depth + 1, truncated)
            {
                // Splice-through wrappers (Go `statement_list`): hoist the
                // wrapper's children into this node so blocks hold statements
                // directly, matching the Rust/Python shape the passes expect.
                if profile.splice_kind(converted.kind) {
                    children.extend(converted.children);
                } else {
                    children.push(converted);
                }
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    Some(NormNode::new(kind, field, span, children))
}

/// P3 passes in spec §5.2 order: loop decomposition incl. recursion (2) →
/// identifiers (4) → literals (5) → order canonicalization (6) → dead
/// syntax (7). Iteration-protocol rewrite runs before loop lowering (it needs
/// intact `for` nodes); loop-exit normalization right after it.
pub fn apply_passes(
    tree: NormNode,
    lang: Lang,
    cfg: &Config,
    label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
) -> NormNode {
    let profile = lang.profile();
    let tree = profile.lower_recursion(tree);
    let tree = profile.rewrite_iteration(tree, label_interner);
    let tree = profile.lower_loops(tree, label_interner);
    let tree = profile.normalize_loop_exit(tree);
    let tree = abstract_idents(tree, profile, label_interner);
    let tree = abstract_literals(tree, profile, cfg, label_interner);
    let tree = canonicalize_order(tree, profile, label_interner, None);
    remove_dead(tree, profile)
}

/// Spec §5.2.6: sort operand chains of commutative operators (by exact-mode
/// subtree hash) and key/value pair lists by key. Unsound for floats and
/// overloading — accepted, output is a report.
///
/// `parent_chain` carries the enclosing commutative chain's `(kind, op)` down the left
/// spine. A left-assoc chain like `((a+b)+c)+d` parses as nested binary nodes; canonicalizing
/// bottom-up, every inner spine node would independently re-flatten and rebuild its whole
/// prefix → O(n²) subtree clones for an n-term chain. Only the chain HEAD (a node whose parent
/// is not the same commutative kind+op) needs to flatten the entire chain and rebuild — the
/// head's re-flatten discards every inner spine node's structure, so skipping the inner
/// rebuild is byte-identical. The head threads its chain context to its index-0 (left-operand)
/// child only: `flatten_chain` descends the left spine, while right operands are pushed whole
/// and so must canonicalize as chain heads in their own right (context `None`).
fn canonicalize_order(
    mut node: NormNode,
    profile: &dyn LanguageProfile,
    li: &crate::intern::LabelInterner,
    parent_chain: Option<(Kind, Kind)>,
) -> NormNode {
    // Is THIS node a flatten-able commutative chain node, and what is its operator? The op is
    // a childless, label-less leaf at the operator position (index 1 of a len-3 binary node),
    // unaffected by canonicalizing the operands — so this is stable whether computed before or
    // after the child recursion. We compute it before, to thread the chain context down the
    // spine. `Kind` is `Copy` (interned id), so this is a plain copy, not a heap-cloning `Box<str>`.
    let this_kind: Kind = node.kind;
    let this_op: Option<Kind> =
        if profile.binary_fields(node.kind).is_some() && node.children.len() == 3 {
            node.children
                .iter()
                .find(|c| {
                    c.children.is_empty()
                        && c.label.is_none()
                        && profile.commutative_ops().contains(&c.kind.as_str())
                })
                .map(|c| c.kind)
        } else {
            None
        };
    let this_chain: Option<(Kind, Kind)> = this_op.map(|op| (this_kind, op));

    // Canonicalize children bottom-up. The left operand (index 0) inherits this node's chain
    // context so a same-kind/op child recognizes itself as an inner spine node and skips its
    // rebuild; every other child (notably the right operand, which the head pushes whole) is a
    // chain head in its own right and gets `None`.
    node.children = node
        .children
        .into_iter()
        .enumerate()
        .map(|(i, c)| canonicalize_order(c, profile, li, if i == 0 { this_chain } else { None }))
        .collect();

    // An inner spine node is the index-0 child of a parent chain of its exact (kind, op) —
    // precisely the node the head's `flatten_chain` descends through. Its flatten+rebuild is
    // redundant (the head re-flattens and rebuilds the whole chain), so skip it.
    let is_inner_spine = parent_chain.is_some() && parent_chain == this_chain;

    if !is_inner_spine
        && let Some((lf, of, rf)) = profile.binary_fields(node.kind)
        && let Some(op) = this_op
    {
        let mut operands = Vec::new();
        flatten_chain(&node, node.kind, op, &mut operands);
        if operands.len() >= 2 {
            let op_node = node.children.iter().find(|c| c.kind == op).unwrap().clone();
            // Masked hash first (stable when local indices shift — the D2
            // cascade), exact hash as tie-break (so `a*b` vs `b*a` still
            // sorts consistently). `sort_by_cached_key` computes each element's
            // (two-merkle) key exactly once instead of on every comparison —
            // same sort result, no recomputation.
            operands.sort_by_cached_key(|o| {
                (
                    crate::fingerprint::merkle_mode(
                        o,
                        crate::fingerprint::HashMode::MaskedLocals,
                        li,
                    ),
                    crate::fingerprint::merkle(o, li),
                )
            });
            let field = node.field;
            let span = node.span;
            let kind = node.kind;
            let mut acc = operands.remove(0);
            acc.field = lf;
            for mut next in operands {
                next.field = rf;
                let mut op_clone = op_node.clone();
                op_clone.field = of;
                let mut merged = NormNode::with_kind(kind, None, span, vec![acc, op_clone, next]);
                merged.children[0].field = lf;
                acc = merged;
            }
            acc.field = field;
            return acc;
        }
    }

    if let Some(key_field) = profile.sortable_pair_kind(node.kind)
        && node.children.len() >= 2
        && node
            .children
            .iter()
            .all(|c| crate::lang::child_field_id(c, key_field).is_some())
    {
        node.children.sort_by_key(|pair| {
            crate::lang::child_field_id(pair, key_field)
                .map(|n| crate::fingerprint::merkle(n, li))
                .unwrap_or(0)
        });
    }
    node
}

/// Collect operands of a same-kind, same-operator chain (left-assoc parses).
fn flatten_chain(node: &NormNode, kind: Kind, op: Kind, out: &mut Vec<NormNode>) {
    if node.kind == kind && node.children.len() == 3 && node.children[1].kind == op {
        flatten_chain(&node.children[0], kind, op, out);
        let mut rhs = node.children[2].clone();
        rhs.field = None;
        out.push(rhs);
    } else {
        let mut n = node.clone();
        n.field = None;
        out.push(n);
    }
}

/// Spec §5.2.4: locals become positional `Local(n)` by first occurrence;
/// everything else keeps its name as `External`.
///
/// `label_interner` (interning WP): the historical normalizer has no `TransformLog` to
/// piggyback the per-scan `LabelInterner` on (that's an IR-frontend-only seam), so it
/// is threaded explicitly here — a short chain contained to normalize.rs + unit.rs.
fn abstract_idents(
    mut root: NormNode,
    profile: &dyn LanguageProfile,
    label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
) -> NormNode {
    let mut declared_list = Vec::new();
    profile.collect_declared(&root, &mut declared_list);
    let declared: HashSet<Box<str>> = declared_list.into_iter().collect();
    let mut order: HashMap<Box<str>, u32> = HashMap::new();

    fn walk(
        node: &mut NormNode,
        parent_kind: Kind,
        profile: &dyn LanguageProfile,
        declared: &HashSet<Box<str>>,
        order: &mut HashMap<Box<str>, u32>,
        label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
    ) {
        if let Some(Label::Raw(text)) = node.label.clone() {
            let field = node.field;
            let label = if profile.always_external(node.kind, field, parent_kind) {
                Label::External(label_interner.intern(&text))
            } else if declared.contains(&text) {
                let next = order.len() as u32;
                Label::Local(*order.entry(text).or_insert(next))
            } else {
                Label::External(label_interner.intern(&text))
            };
            node.label = Some(label);
        }
        let kind = node.kind;
        for child in &mut node.children {
            walk(child, kind, profile, declared, order, label_interner);
        }
    }
    // Sentinel "no parent" kind at the root — mirrors `inline.rs`'s `substitute` root
    // call (both feed `LanguageProfile::always_external`'s `parent_kind`); no real
    // grammar/IR kind is ever the empty string, so it never collides.
    static EMPTY_KIND: LazyLock<Kind> = LazyLock::new(|| Kind::intern(""));
    walk(
        &mut root,
        *EMPTY_KIND,
        profile,
        &declared,
        &mut order,
        label_interner,
    );
    root
}

/// Spec §5.2.5: typed buckets, except keep-list literals whose identity is
/// structural.
fn abstract_literals(
    mut root: NormNode,
    profile: &dyn LanguageProfile,
    cfg: &Config,
    label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
) -> NormNode {
    fn walk(
        node: &mut NormNode,
        profile: &dyn LanguageProfile,
        keep: &[String],
        label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
    ) {
        if let Some(Label::RawLit(text)) = node.label.clone() {
            let bucket = profile
                .literal_bucket(node.kind.as_str())
                .unwrap_or(Bucket::Str);
            let key = match bucket {
                Bucket::Str | Bucket::Char => inner_text(&text),
                _ => text.trim().to_string(),
            };
            node.label = Some(if keep.iter().any(|k| k == &key) {
                Label::LitKept(label_interner.intern(&key))
            } else {
                Label::LitBucket(bucket)
            });
        }
        for child in &mut node.children {
            walk(child, profile, keep, label_interner);
        }
    }
    walk(
        &mut root,
        profile,
        &cfg.normalize.literal_keep,
        label_interner,
    );
    root
}

/// Strip prefix letters (r/b/f/u) and symmetric quotes to get literal content
/// for keep-list comparison.
fn inner_text(text: &str) -> String {
    let stripped = text.trim_start_matches(|c: char| c.is_ascii_alphabetic() || c == '#');
    let stripped = stripped.trim_end_matches('#');
    for quote in ["\"\"\"", "'''", "\"", "'"] {
        if stripped.len() >= 2 * quote.len()
            && stripped.starts_with(quote)
            && stripped.ends_with(quote)
        {
            return stripped[quote.len()..stripped.len() - quote.len()].to_string();
        }
    }
    stripped.to_string()
}

/// Spec §5.2.7: `pass` bodies, empty `else`, and loop-tail `continue`
/// (redundant, and lowered recursion emits one) normalized away.
fn remove_dead(mut node: NormNode, profile: &dyn LanguageProfile) -> NormNode {
    node.children = node
        .children
        .into_iter()
        .filter_map(|child| {
            let child = remove_dead(child, profile);
            let dead = child.kind == *PASS_STATEMENT
                || (child.kind == *ELSE_CLAUSE && is_structurally_empty(&child));
            if dead { None } else { Some(child) }
        })
        .collect();
    if profile.is_loop_core(&node)
        && let Some(body) = node
            .children
            .iter_mut()
            .find(|c| c.field == Some(*BODY_FIELD))
    {
        while body
            .children
            .last()
            .is_some_and(|c| profile.is_continue_stmt(c))
        {
            body.children.pop();
        }
    }
    node
}

fn is_structurally_empty(node: &NormNode) -> bool {
    node.label.is_none()
        && (node.children.is_empty() || node.children.iter().all(is_structurally_empty))
        && (node.kind == *BLOCK_LOWER || node.kind == *ELSE_CLAUSE)
}
