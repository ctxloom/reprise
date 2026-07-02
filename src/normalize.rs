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
use crate::lang::{Lang, LanguageProfile};
use crate::tree::{Bucket, Label, NormNode};
use std::collections::{HashMap, HashSet};

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
    collect_units(cst.root_node(), src, profile, path, &mut units);
    units
}

/// Test-facing convenience: (unit name, raw tree) pairs.
pub fn raw_units_from_source(src: &str, lang: Lang) -> Vec<(String, NormNode)> {
    extract_raw_units(src, lang, std::path::Path::new("memory.in"))
        .into_iter()
        .map(|u| (u.name, u.tree))
        .collect()
}

fn collect_units(
    node: tree_sitter::Node,
    src: &str,
    profile: &dyn LanguageProfile,
    path: &std::path::Path,
    out: &mut Vec<RawUnit>,
) {
    if profile.is_function_like(node.kind()) {
        let name = node
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(src.as_bytes()).ok())
            .unwrap_or("<anon>")
            .to_string();
        if let Some(tree) = convert(node, None, "", src, profile) {
            out.push(RawUnit {
                is_test: profile.unit_is_test(node, src, &name, path),
                name,
                byte_span: (node.start_byte() as u32, node.end_byte() as u32),
                line_span: (
                    node.start_position().row as u32 + 1,
                    node.end_position().row as u32 + 1,
                ),
                parse_degraded: node.has_error(),
                tree,
            });
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_units(child, src, profile, path, out);
    }
}

/// CST → raw normalized tree. Performs spec §5.2.1 (strip) and the
/// paren-flattening half of §5.2.7 on the way through.
fn convert(
    node: tree_sitter::Node,
    field: Option<&str>,
    parent_kind: &str,
    src: &str,
    profile: &dyn LanguageProfile,
) -> Option<NormNode> {
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
            if let Some(inner) = convert(child, field, parent_kind, src, profile) {
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
            if let Some(converted) = convert(child, child_field, kind, src, profile) {
                // Splice-through wrappers (Go `statement_list`): hoist the
                // wrapper's children into this node so blocks hold statements
                // directly, matching the Rust/Python shape the passes expect.
                if profile.splice_kind(&converted.kind) {
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
pub fn apply_passes(tree: NormNode, lang: Lang, cfg: &Config) -> NormNode {
    let profile = lang.profile();
    let tree = profile.lower_recursion(tree);
    let tree = profile.rewrite_iteration(tree);
    let tree = profile.lower_loops(tree);
    let tree = profile.normalize_loop_exit(tree);
    let tree = abstract_idents(tree, profile);
    let tree = abstract_literals(tree, profile, cfg);
    let tree = canonicalize_order(tree, profile);
    remove_dead(tree, profile)
}

/// Spec §5.2.6: sort operand chains of commutative operators (by exact-mode
/// subtree hash) and key/value pair lists by key. Unsound for floats and
/// overloading — accepted, output is a report.
fn canonicalize_order(mut node: NormNode, profile: &dyn LanguageProfile) -> NormNode {
    node.children = node
        .children
        .into_iter()
        .map(|c| canonicalize_order(c, profile))
        .collect();

    if let Some((lf, of, rf)) = profile.binary_fields(&node.kind) {
        let op_kind = node
            .children
            .iter()
            .find(|c| {
                c.children.is_empty()
                    && c.label.is_none()
                    && profile.commutative_ops().contains(&c.kind.as_ref())
            })
            .map(|c| c.kind.to_string());
        if let Some(op) = op_kind
            && node.children.len() == 3
        {
            let mut operands = Vec::new();
            flatten_chain(&node, &node.kind.clone(), &op, &mut operands);
            if operands.len() >= 2 {
                let op_node = node
                    .children
                    .iter()
                    .find(|c| c.kind.as_ref() == op)
                    .unwrap()
                    .clone();
                // Masked hash first (stable when local indices shift — the D2
                // cascade), exact hash as tie-break (so `a*b` vs `b*a` still
                // sorts consistently).
                operands.sort_by_key(|o| {
                    (
                        crate::fingerprint::merkle_mode(
                            o,
                            crate::fingerprint::HashMode::MaskedLocals,
                        ),
                        crate::fingerprint::merkle(o),
                    )
                });
                let field = node.field.clone();
                let span = node.span;
                let kind = node.kind.clone();
                let mut acc = operands.remove(0);
                acc.field = lf.map(Into::into);
                for mut next in operands {
                    next.field = rf.map(Into::into);
                    let mut op_clone = op_node.clone();
                    op_clone.field = of.map(Into::into);
                    let mut merged = NormNode::new(&kind, None, span, vec![acc, op_clone, next]);
                    merged.children[0].field = lf.map(Into::into);
                    acc = merged;
                }
                acc.field = field;
                return acc;
            }
        }
    }

    if let Some(key_field) = profile.sortable_pair_kind(&node.kind)
        && node.children.len() >= 2
        && node
            .children
            .iter()
            .all(|c| crate::lang::child_field(c, key_field).is_some())
    {
        node.children.sort_by_key(|pair| {
            crate::lang::child_field(pair, key_field)
                .map(crate::fingerprint::merkle)
                .unwrap_or(0)
        });
    }
    node
}

/// Collect operands of a same-kind, same-operator chain (left-assoc parses).
fn flatten_chain(node: &NormNode, kind: &str, op: &str, out: &mut Vec<NormNode>) {
    if node.kind.as_ref() == kind
        && node.children.len() == 3
        && node.children[1].kind.as_ref() == op
    {
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
fn abstract_idents(mut root: NormNode, profile: &dyn LanguageProfile) -> NormNode {
    let mut declared_list = Vec::new();
    profile.collect_declared(&root, &mut declared_list);
    let declared: HashSet<Box<str>> = declared_list.into_iter().collect();
    let mut order: HashMap<Box<str>, u32> = HashMap::new();

    fn walk(
        node: &mut NormNode,
        parent_kind: &str,
        profile: &dyn LanguageProfile,
        declared: &HashSet<Box<str>>,
        order: &mut HashMap<Box<str>, u32>,
    ) {
        if let Some(Label::Raw(text)) = node.label.clone() {
            let field = node.field.as_deref();
            let label = if profile.always_external(&node.kind, field, parent_kind) {
                Label::External(text)
            } else if declared.contains(&text) {
                let next = order.len() as u32;
                Label::Local(*order.entry(text).or_insert(next))
            } else {
                Label::External(text)
            };
            node.label = Some(label);
        }
        let kind = node.kind.clone();
        for child in &mut node.children {
            walk(child, &kind, profile, declared, order);
        }
    }
    walk(&mut root, "", profile, &declared, &mut order);
    root
}

/// Spec §5.2.5: typed buckets, except keep-list literals whose identity is
/// structural.
fn abstract_literals(mut root: NormNode, profile: &dyn LanguageProfile, cfg: &Config) -> NormNode {
    fn walk(node: &mut NormNode, profile: &dyn LanguageProfile, keep: &[String]) {
        if let Some(Label::RawLit(text)) = node.label.clone() {
            let bucket = profile.literal_bucket(&node.kind).unwrap_or(Bucket::Str);
            let key = match bucket {
                Bucket::Str | Bucket::Char => inner_text(&text),
                _ => text.trim().to_string(),
            };
            node.label = Some(if keep.iter().any(|k| k == &key) {
                Label::LitKept(key.into())
            } else {
                Label::LitBucket(bucket)
            });
        }
        for child in &mut node.children {
            walk(child, profile, keep);
        }
    }
    walk(&mut root, profile, &cfg.normalize.literal_keep);
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
            match child.kind.as_ref() {
                "pass_statement" => None,
                "else_clause" if is_structurally_empty(&child) => None,
                _ => Some(child),
            }
        })
        .collect();
    if profile.is_loop_core(&node)
        && let Some(body) = node
            .children
            .iter_mut()
            .find(|c| c.field.as_deref() == Some("body"))
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
        && matches!(node.kind.as_ref(), "block" | "else_clause")
}
