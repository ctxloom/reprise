//! Rust language profile.

use super::{
    LanguageProfile, child_field, collect_pattern_idents, count_self_calls, is_raw_ident,
    path_is_testy, raw_name, synth_call, synth_ident,
};
use crate::tree::{Bucket, Label, NormNode};
use std::path::Path;

pub struct RustProfile;

impl LanguageProfile for RustProfile {
    fn is_function_like(&self, kind: &str) -> bool {
        kind == "function_item"
    }

    fn is_comment(&self, kind: &str) -> bool {
        matches!(kind, "line_comment" | "block_comment")
    }

    fn strip_kind(&self, kind: &str) -> bool {
        matches!(
            kind,
            "attribute_item" | "inner_attribute_item" | "mutable_specifier" | "lifetime"
        )
    }

    fn is_identifier(&self, kind: &str) -> bool {
        matches!(
            kind,
            "identifier"
                | "field_identifier"
                | "type_identifier"
                | "primitive_type"
                | "shorthand_field_identifier"
        )
    }

    fn literal_bucket(&self, kind: &str) -> Option<Bucket> {
        match kind {
            "integer_literal" => Some(Bucket::Int),
            "float_literal" => Some(Bucket::Float),
            "string_literal" | "raw_string_literal" => Some(Bucket::Str),
            "char_literal" => Some(Bucket::Char),
            "boolean_literal" => Some(Bucket::Bool),
            _ => None,
        }
    }

    fn unwrap_kind(&self, kind: &str) -> bool {
        kind == "parenthesized_expression"
    }

    fn keep_anon_parent(&self, parent_kind: &str) -> bool {
        matches!(
            parent_kind,
            "binary_expression"
                | "compound_assignment_expr"
                | "unary_expression"
                | "range_expression"
        )
    }

    fn always_external(&self, kind: &str, _field: Option<&str>, _parent_kind: &str) -> bool {
        matches!(
            kind,
            "field_identifier"
                | "type_identifier"
                | "primitive_type"
                | "shorthand_field_identifier"
        )
    }

    fn collect_declared(&self, root: &NormNode, out: &mut Vec<Box<str>>) {
        fn walk(node: &NormNode, out: &mut Vec<Box<str>>) {
            match node.kind.as_ref() {
                "parameters" | "closure_parameters" => collect_pattern_idents(node, out),
                "let_declaration" | "let_condition" | "match_arm" => {
                    if let Some(pat) = child_field(node, "pattern") {
                        collect_pattern_idents(pat, out);
                    }
                }
                "function_item" => {
                    if let Some(name) = child_field(node, "name")
                        && let Some(Label::Raw(text)) = &name.label
                    {
                        out.push(text.clone());
                    }
                }
                _ => {}
            }
            for child in &node.children {
                walk(child, out);
            }
        }
        walk(root, out);
    }

    fn lower_loops(
        &self,
        node: NormNode,
        label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
    ) -> NormNode {
        lower(node, label_interner)
    }

    fn lower_recursion(&self, root: NormNode) -> NormNode {
        lower_recursion(root)
    }

    fn rewrite_iteration(
        &self,
        root: NormNode,
        _label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
    ) -> NormNode {
        rewrite_iteration(root)
    }

    fn normalize_loop_exit(&self, root: NormNode) -> NormNode {
        normalize_loop_exit(self, root)
    }

    fn commutative_ops(&self) -> &'static [&'static str] {
        &["+", "*", "&", "|", "^", "==", "!=", "&&", "||"]
    }

    fn binary_fields(
        &self,
        kind: &str,
    ) -> Option<(
        Option<&'static str>,
        Option<&'static str>,
        Option<&'static str>,
    )> {
        match kind {
            "binary_expression" => Some((Some("left"), Some("operator"), Some("right"))),
            _ => None,
        }
    }

    fn sortable_pair_kind(&self, kind: &str) -> Option<&'static str> {
        match kind {
            "field_initializer_list" => Some("field"),
            _ => None,
        }
    }

    fn is_list_kind(&self, kind: &str) -> bool {
        matches!(
            kind,
            "block"
                | "arguments"
                | "parameters"
                | "match_block"
                | "tuple_expression"
                | "array_expression"
                | "token_tree"
                | "REPEAT"
        )
    }

    fn is_dispatch_arm(&self, kind: &str) -> bool {
        kind == "match_arm"
    }

    fn is_statement_kind(&self, kind: &str) -> bool {
        matches!(
            kind,
            "expression_statement" | "let_declaration" | "empty_statement" | "macro_invocation"
        )
    }

    fn is_loop_core(&self, node: &NormNode) -> bool {
        node.kind.as_ref() == "loop_expression"
    }

    fn is_continue_stmt(&self, node: &NormNode) -> bool {
        node.kind.as_ref() == "expression_statement"
            && node.children.len() == 1
            && node.children[0].kind.as_ref() == "continue_expression"
    }

    fn unit_is_test(&self, node: tree_sitter::Node, src: &str, name: &str, path: &Path) -> bool {
        if name.starts_with("test_") || path_is_testy(path) {
            return true;
        }
        // #[test]-style attributes are preceding siblings of the fn item.
        let mut prev = node.prev_named_sibling();
        while let Some(sib) = prev {
            if sib.kind() != "attribute_item" {
                break;
            }
            let text = sib.utf8_text(src.as_bytes()).unwrap_or_default();
            if text.contains("test") || text.contains("bench") {
                return true;
            }
            prev = sib.prev_named_sibling();
        }
        false
    }

    // ---- Phase 3: inliner hooks (spec §5.4) ----

    fn call_kind(&self) -> &'static str {
        "call_expression"
    }

    fn inline_params(&self, root: &NormNode) -> Option<Vec<Box<str>>> {
        simple_params(root)
    }

    fn return_value<'a>(&self, stmt: &'a NormNode) -> Option<&'a NormNode> {
        if stmt.kind.as_ref() == "expression_statement"
            && stmt.children.len() == 1
            && stmt.children[0].kind.as_ref() == "return_expression"
            && stmt.children[0].children.len() == 1
        {
            return Some(&stmt.children[0].children[0]);
        }
        None
    }

    fn make_return(&self, mut value: NormNode) -> NormNode {
        let span = value.span;
        value.field = None;
        let ret = NormNode::new("return_expression", None, span, vec![value]);
        NormNode::new("expression_statement", None, span, vec![ret])
    }

    fn make_expr_stmt(&self, mut expr: NormNode) -> NormNode {
        let span = expr.span;
        expr.field = None;
        NormNode::new("expression_statement", None, span, vec![expr])
    }

    fn make_expr_block(&self, span: (u32, u32), children: Vec<NormNode>) -> Option<NormNode> {
        // Rust blocks are expressions — a native kind fits (spec §5.2.3).
        Some(NormNode::new("block", None, span, children))
    }
}

/// Lower `while` / `for` to the minimal loop core (spec §5.2.2):
/// `loop { if !(cond) { break; } bind; body }` — using the exact tree shapes
/// tree-sitter produces for the manually written form, so both converge.
fn lower(
    mut node: NormNode,
    label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
) -> NormNode {
    node.children = node
        .children
        .into_iter()
        .map(|c| lower(c, label_interner))
        .collect();
    match node.kind.as_ref() {
        "while_expression" => {
            let field = node.field.as_deref().map(str::to_owned);
            let span = node.span;
            let Some(mut cond) = node.take_field("condition") else {
                return node;
            };
            let Some(mut body) = node.take_field("body") else {
                return node;
            };
            cond.field = None;
            body.children.insert(0, break_unless(cond));
            NormNode::new("loop_expression", field.as_deref(), span, vec![body])
        }
        "for_expression" => {
            let field = node.field.as_deref().map(str::to_owned);
            let span = node.span;
            let (Some(mut pat), Some(mut value), Some(mut body)) = (
                node.take_field("pattern"),
                node.take_field("value"),
                node.take_field("body"),
            ) else {
                return node;
            };
            value.field = None;
            let guard = break_unless(call("__has_next", None, value.clone(), label_interner));
            pat.field = Some("pattern".into());
            let next = call("__next", Some("value"), value, label_interner);
            let span_pat = pat.span;
            let let_decl = NormNode::new("let_declaration", None, span_pat, vec![pat, next]);
            body.children.insert(0, guard);
            body.children.insert(1, let_decl);
            NormNode::new("loop_expression", field.as_deref(), span, vec![body])
        }
        _ => node,
    }
}

/// `expression_statement > if_expression(condition: !cond, consequence: { break; })`
fn break_unless(cond: NormNode) -> NormNode {
    let span = cond.span;
    let bang = NormNode::token("!", span);
    let negated = NormNode::new(
        "unary_expression",
        Some("condition"),
        span,
        vec![bang, cond],
    );
    let brk = NormNode::new("break_expression", None, span, Vec::new());
    let brk_stmt = NormNode::new("expression_statement", None, span, vec![brk]);
    let consequence = NormNode::new("block", Some("consequence"), span, vec![brk_stmt]);
    let iff = NormNode::new("if_expression", None, span, vec![negated, consequence]);
    NormNode::new("expression_statement", None, span, vec![iff])
}

fn call(
    name: &str,
    field: Option<&str>,
    arg: NormNode,
    label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
) -> NormNode {
    synth_call(
        "call_expression",
        "arguments",
        name,
        field,
        arg,
        label_interner,
    )
}

// ---- recursion lowering (spec §5.2.2 Rev 5) ----

fn lower_recursion(root: NormNode) -> NormNode {
    let Some(name) = raw_name(&root) else {
        return root;
    };
    // Simple identifier params only; anything fancier bails (heuristic, §5.2.4).
    let Some(params) = simple_params(&root) else {
        return root;
    };
    if params.is_empty() {
        return root;
    }
    let total = child_field(&root, "body")
        .map(|b| count_self_calls(b, "call_expression", &name))
        .unwrap_or(0);
    if total == 0 {
        return root;
    }

    let mut root = root;
    let Some(body_idx) = root
        .children
        .iter()
        .position(|c| c.field.as_deref() == Some("body"))
    else {
        return root;
    };
    // Transform a clone; commit only if EVERY self-call was a tail site
    // (mixed tail/non-tail = non-linear recursion, which must not lower).
    let mut replaced = 0u32;
    let new_body = rewrite_tail_sites(
        root.children[body_idx].clone(),
        &name,
        &params,
        &mut replaced,
    );
    if replaced != total {
        return root;
    }
    root.children.remove(body_idx);

    let span = new_body.span;
    let mut inner = new_body;
    inner.field = Some("body".into());
    let loop_node = NormNode::new("loop_expression", None, span, vec![inner]);
    let loop_stmt = NormNode::new("expression_statement", None, span, vec![loop_node]);
    let wrapper = NormNode::new("block", Some("body"), span, vec![loop_stmt]);
    root.children.insert(body_idx, wrapper);
    root
}

fn simple_params(root: &NormNode) -> Option<Vec<Box<str>>> {
    let params = child_field(root, "parameters")?;
    let mut out = Vec::new();
    for p in &params.children {
        if p.kind.as_ref() == "self_parameter" {
            return None;
        }
        if p.kind.as_ref() != "parameter" {
            continue;
        }
        let pat = child_field(p, "pattern")?;
        match &pat.label {
            Some(Label::Raw(text)) if pat.kind.as_ref() == "identifier" => out.push(text.clone()),
            _ => return None,
        }
    }
    Some(out)
}

fn is_self_call(node: &NormNode, name: &str) -> bool {
    node.kind.as_ref() == "call_expression"
        && child_field(node, "function").is_some_and(|f| is_raw_ident(f, name))
}

/// Replace tail self-call sites with param reassignment + continue.
fn rewrite_tail_sites(
    mut node: NormNode,
    name: &str,
    params: &[Box<str>],
    replaced: &mut u32,
) -> NormNode {
    if node.kind.as_ref() == "block" {
        let mut out = Vec::with_capacity(node.children.len());
        for child in node.children {
            // `return f(...);`
            let is_return_site = child.kind.as_ref() == "expression_statement"
                && child.children.len() == 1
                && child.children[0].kind.as_ref() == "return_expression"
                && child.children[0].children.len() == 1
                && is_self_call(&child.children[0].children[0], name);
            // block-tail `f(...)`
            let is_tail_site = is_self_call(&child, name);
            if is_return_site {
                let call = child
                    .children
                    .into_iter()
                    .next()
                    .unwrap()
                    .children
                    .into_iter()
                    .next()
                    .unwrap();
                out.extend(reassign_stmts(call, params, replaced));
            } else if is_tail_site {
                out.extend(reassign_stmts(child, params, replaced));
            } else {
                out.push(rewrite_tail_sites(child, name, params, replaced));
            }
        }
        node.children = out;
        node
    } else {
        node.children = node
            .children
            .into_iter()
            .map(|c| rewrite_tail_sites(c, name, params, replaced))
            .collect();
        node
    }
}

/// `(p1, p2) = (a1, a2); continue;` — identity pairs dropped so the shape
/// matches the iterative counterpart's update statement exactly.
fn reassign_stmts(call: NormNode, params: &[Box<str>], replaced: &mut u32) -> Vec<NormNode> {
    let span = call.span;
    let args: Vec<NormNode> = child_field(&call, "arguments")
        .map(|a| a.children.clone())
        .unwrap_or_default();
    if args.len() != params.len() {
        // Arity mismatch (not our function after all) — leave a continue so the
        // site count still balances; noise tolerated by design.
        *replaced += 1;
        return vec![continue_stmt(span)];
    }
    *replaced += 1;
    let pairs: Vec<(usize, NormNode)> = args
        .into_iter()
        .enumerate()
        .filter(|(i, arg)| !is_raw_ident(arg, &params[*i]))
        .collect();
    let mut out = Vec::new();
    if pairs.len() == 1 {
        let (i, mut arg) = pairs.into_iter().next().unwrap();
        arg.field = Some("right".into());
        let left = synth_ident(&params[i], Some("left"), span);
        let assign = NormNode::new("assignment_expression", None, span, vec![left, arg]);
        out.push(NormNode::new(
            "expression_statement",
            None,
            span,
            vec![assign],
        ));
    } else if pairs.len() > 1 {
        let mut lefts = Vec::new();
        let mut rights = Vec::new();
        for (i, mut arg) in pairs {
            arg.field = None;
            lefts.push(synth_ident(&params[i], None, span));
            rights.push(arg);
        }
        let left = NormNode::new("tuple_expression", Some("left"), span, lefts);
        let right = NormNode::new("tuple_expression", Some("right"), span, rights);
        let assign = NormNode::new("assignment_expression", None, span, vec![left, right]);
        out.push(NormNode::new(
            "expression_statement",
            None,
            span,
            vec![assign],
        ));
    }
    out.push(continue_stmt(span));
    out
}

fn continue_stmt(span: (u32, u32)) -> NormNode {
    let c = NormNode::new("continue_expression", None, span, Vec::new());
    NormNode::new("expression_statement", None, span, vec![c])
}

// ---- iteration-protocol rewrite (spec §5.2.2): index-over-length → iterated ----
// Handles `for i in 0..xs.len() { xs[i] }` and the while-index analog
// `let mut i = 0; while i < xs.len() { ...xs[i]...; i += 1; }`.

fn rewrite_iteration(mut node: NormNode) -> NormNode {
    node.children = node.children.into_iter().map(rewrite_iteration).collect();
    if node.kind.as_ref() == "block" {
        node = rewrite_while_index(node);
    }
    if node.kind.as_ref() != "for_expression" {
        return node;
    }
    let (Some(pat), Some(value)) = (child_field(&node, "pattern"), child_field(&node, "value"))
    else {
        return node;
    };
    let Some(Label::Raw(ivar)) = pat.label.clone() else {
        return node;
    };
    let Some(collection) = range_len_collection(value) else {
        return node;
    };
    let Some(body) = child_field(&node, "body") else {
        return node;
    };
    if !index_uses_only(body, &ivar, &collection) {
        return node;
    }
    // Rewrite: value → collection; xs[i] → i.
    let span = value.span;
    let new_value = synth_ident(&collection, Some("value"), span);
    let body_idx = node
        .children
        .iter()
        .position(|c| c.field.as_deref() == Some("body"))
        .unwrap();
    let body = node.children.remove(body_idx);
    let new_body = replace_index_exprs(body, &ivar, &collection);
    let value_idx = node
        .children
        .iter()
        .position(|c| c.field.as_deref() == Some("value"))
        .unwrap();
    node.children[value_idx] = new_value;
    node.children
        .insert(body_idx.min(node.children.len()), new_body);
    node
}

/// `let mut i = 0; while i < xs.len() { body…; i += 1; }` → `for i in xs { body' }`
/// (i becomes the element variable; `xs[i]` → `i`; the step statement drops).
fn rewrite_while_index(mut block: NormNode) -> NormNode {
    let mut k = 0;
    while k + 1 < block.children.len() {
        if let Some((ivar, coll)) = while_index_match(&block.children[k], &block.children[k + 1]) {
            let while_stmt = block.children.remove(k + 1);
            block.children.remove(k);
            let mut we = while_stmt.children.into_iter().next().unwrap();
            let Some(mut body) = we.take_field("body") else {
                break;
            };
            // Remove exactly the `i += 1;` step at body top level.
            body.children.retain(|c| !is_step_stmt(c, &ivar));
            let body = replace_index_exprs(body, &ivar, &coll);
            let span = body.span;
            let for_expr = NormNode::new(
                "for_expression",
                None,
                span,
                vec![
                    synth_ident(&ivar, Some("pattern"), span),
                    synth_ident(&coll, Some("value"), span),
                    body,
                ],
            );
            let stmt = NormNode::new("expression_statement", None, span, vec![for_expr]);
            block.children.insert(k, stmt);
        }
        k += 1;
    }
    block
}

/// Matches (`let mut i = 0;`, `while i < xs.len() {…}`) where all other uses
/// of `i` in the body are `xs[i]` and exactly the step `i += 1` exists.
fn while_index_match(decl: &NormNode, stmt: &NormNode) -> Option<(Box<str>, Box<str>)> {
    if decl.kind.as_ref() != "let_declaration" {
        return None;
    }
    let pat = child_field(decl, "pattern")?;
    let Some(Label::Raw(ivar)) = pat.label.clone() else {
        return None;
    };
    let zero = child_field(decl, "value")?;
    if !matches!(&zero.label, Some(Label::RawLit(t)) if t.as_ref() == "0") {
        return None;
    }
    if stmt.kind.as_ref() != "expression_statement" || stmt.children.len() != 1 {
        return None;
    }
    let we = &stmt.children[0];
    if we.kind.as_ref() != "while_expression" {
        return None;
    }
    let cond = child_field(we, "condition")?;
    if cond.kind.as_ref() != "binary_expression" || cond.children.len() != 3 {
        return None;
    }
    if !is_raw_ident(&cond.children[0], &ivar) || cond.children[1].kind.as_ref() != "<" {
        return None;
    }
    // xs.len()
    let call = &cond.children[2];
    if call.kind.as_ref() != "call_expression" {
        return None;
    }
    let fexpr = child_field(call, "function")?;
    if fexpr.kind.as_ref() != "field_expression" {
        return None;
    }
    let recv = child_field(fexpr, "value")?;
    let method = child_field(fexpr, "field")?;
    if !matches!(&method.label, Some(Label::Raw(t)) if t.as_ref() == "len") {
        return None;
    }
    let Some(Label::Raw(coll)) = recv.label.clone() else {
        return None;
    };
    // Body: exactly one step statement; every other `i` use is `xs[i]`.
    let body = child_field(we, "body")?;
    let steps = body
        .children
        .iter()
        .filter(|c| is_step_stmt(c, &ivar))
        .count();
    if steps != 1 {
        return None;
    }
    let ok = body
        .children
        .iter()
        .filter(|c| !is_step_stmt(c, &ivar))
        .all(|c| index_uses_only(c, &ivar, &coll));
    if !ok {
        return None;
    }
    Some((ivar, coll))
}

fn is_step_stmt(node: &NormNode, ivar: &str) -> bool {
    node.kind.as_ref() == "expression_statement" && node.children.len() == 1 && {
        let c = &node.children[0];
        c.kind.as_ref() == "compound_assignment_expr"
            && c.children.len() == 3
            && is_raw_ident(&c.children[0], ivar)
            && c.children[1].kind.as_ref() == "+="
            && matches!(&c.children[2].label, Some(Label::RawLit(t)) if t.as_ref() == "1")
    }
}

/// Matches `0..X.len()` → Some(X).
fn range_len_collection(value: &NormNode) -> Option<Box<str>> {
    if value.kind.as_ref() != "range_expression" || value.children.len() != 3 {
        return None;
    }
    let zero = &value.children[0];
    if !matches!(&zero.label, Some(Label::RawLit(t)) if t.as_ref() == "0") {
        return None;
    }
    if value.children[1].kind.as_ref() != ".." {
        return None;
    }
    let call = &value.children[2];
    if call.kind.as_ref() != "call_expression" {
        return None;
    }
    let fexpr = child_field(call, "function")?;
    if fexpr.kind.as_ref() != "field_expression" {
        return None;
    }
    let recv = child_field(fexpr, "value")?;
    let method = child_field(fexpr, "field")?;
    if !matches!(&method.label, Some(Label::Raw(t)) if t.as_ref() == "len") {
        return None;
    }
    match &recv.label {
        Some(Label::Raw(t)) if recv.kind.as_ref() == "identifier" => Some(t.clone()),
        _ => None,
    }
}

/// Every use of `i` must be exactly `xs[i]`.
fn index_uses_only(node: &NormNode, ivar: &str, coll: &str) -> bool {
    if node.kind.as_ref() == "index_expression"
        && node.children.len() == 2
        && is_raw_ident(&node.children[0], coll)
        && is_raw_ident(&node.children[1], ivar)
    {
        return true; // this use is fine; don't descend into it
    }
    if is_raw_ident(node, ivar) {
        return false;
    }
    node.children.iter().all(|c| index_uses_only(c, ivar, coll))
}

fn replace_index_exprs(mut node: NormNode, ivar: &str, coll: &str) -> NormNode {
    if node.kind.as_ref() == "index_expression"
        && node.children.len() == 2
        && is_raw_ident(&node.children[0], coll)
        && is_raw_ident(&node.children[1], ivar)
    {
        let field = node.field.as_deref().map(str::to_owned);
        return synth_ident(ivar, field.as_deref(), node.span);
    }
    node.children = node
        .children
        .into_iter()
        .map(|c| replace_index_exprs(c, ivar, coll))
        .collect();
    node
}

// ---- loop-exit normalization ----

fn normalize_loop_exit(profile: &RustProfile, mut root: NormNode) -> NormNode {
    let Some(body) = root
        .children
        .iter_mut()
        .find(|c| c.field.as_deref() == Some("body"))
    else {
        return root;
    };
    let n = body.children.len();
    if n < 2 {
        return root;
    }
    // Trailing value: block-tail expression or `return E;`.
    let ret_value: Option<NormNode> = {
        let last = &body.children[n - 1];
        if last.kind.as_ref() == "expression_statement"
            && last.children.len() == 1
            && last.children[0].kind.as_ref() == "return_expression"
            && last.children[0].children.len() == 1
        {
            Some(last.children[0].children[0].clone())
        } else if !profile.is_statement_kind(&last.kind) && last.kind.as_ref() != "REPEAT" {
            Some(last.clone())
        } else {
            None
        }
    };
    let Some(mut value) = ret_value else {
        return root;
    };
    value.field = None;
    let loop_stmt = &mut body.children[n - 2];
    let is_loop = loop_stmt.kind.as_ref() == "expression_statement"
        && loop_stmt.children.len() == 1
        && loop_stmt.children[0].kind.as_ref() == "loop_expression";
    if !is_loop {
        return root;
    }
    let mut replaced = 0u32;
    replace_breaks(&mut loop_stmt.children[0], &value, &mut replaced, true);
    if replaced > 0 {
        body.children.truncate(n - 1);
    }
    root
}

fn replace_breaks(node: &mut NormNode, value: &NormNode, replaced: &mut u32, top: bool) {
    if !top && node.kind.as_ref() == "loop_expression" {
        return; // inner loops own their breaks
    }
    if node.kind.as_ref() == "expression_statement"
        && node.children.len() == 1
        && node.children[0].kind.as_ref() == "break_expression"
        && node.children[0].children.is_empty()
    {
        let span = node.children[0].span;
        let ret = NormNode::new("return_expression", None, span, vec![value.clone()]);
        node.children[0] = ret;
        *replaced += 1;
        return;
    }
    for child in &mut node.children {
        replace_breaks(child, value, replaced, false);
    }
}
