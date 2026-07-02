//! Python language profile.

use super::{
    LanguageProfile, child_field, collect_pattern_idents, count_self_calls, is_raw_ident,
    path_is_testy, raw_name, synth_call, synth_ident,
};
use crate::tree::{Bucket, Label, NormNode};
use std::path::Path;

pub struct PythonProfile;

impl LanguageProfile for PythonProfile {
    fn is_function_like(&self, kind: &str) -> bool {
        kind == "function_definition"
    }

    fn is_comment(&self, kind: &str) -> bool {
        kind == "comment"
    }

    fn strip_kind(&self, kind: &str) -> bool {
        kind == "decorator"
    }

    fn is_identifier(&self, kind: &str) -> bool {
        kind == "identifier"
    }

    fn literal_bucket(&self, kind: &str) -> Option<Bucket> {
        match kind {
            "integer" => Some(Bucket::Int),
            "float" => Some(Bucket::Float),
            "string" | "concatenated_string" => Some(Bucket::Str),
            "true" | "false" => Some(Bucket::Bool),
            _ => None,
        }
    }

    fn unwrap_kind(&self, kind: &str) -> bool {
        kind == "parenthesized_expression"
    }

    fn keep_anon_parent(&self, parent_kind: &str) -> bool {
        // `not_operator` is deliberately absent: its kind implies the operator,
        // and the loop lowering synthesizes token-less `not_operator` nodes.
        matches!(
            parent_kind,
            "comparison_operator"
                | "binary_operator"
                | "boolean_operator"
                | "unary_operator"
                | "augmented_assignment"
        )
    }

    fn always_external(&self, _kind: &str, field: Option<&str>, parent_kind: &str) -> bool {
        field == Some("attribute") || (parent_kind == "keyword_argument" && field == Some("name"))
    }

    fn collect_declared(&self, root: &NormNode, out: &mut Vec<Box<str>>) {
        fn push_field(node: &NormNode, field: &str, out: &mut Vec<Box<str>>) {
            if let Some(child) = child_field(node, field) {
                collect_pattern_idents(child, out);
            }
        }
        fn walk(node: &NormNode, out: &mut Vec<Box<str>>) {
            match node.kind.as_ref() {
                "global_statement" | "nonlocal_statement" => return,
                "function_definition" => push_field(node, "name", out),
                "parameters" | "lambda_parameters" => collect_param_idents(node, out),
                "assignment" | "augmented_assignment" | "for_statement" | "for_in_clause" => {
                    push_field(node, "left", out);
                }
                "named_expression" => push_field(node, "name", out),
                "as_pattern_target" => collect_pattern_idents(node, out),
                _ => {}
            }
            for child in &node.children {
                walk(child, out);
            }
        }
        walk(root, out);
    }

    fn lower_loops(&self, node: NormNode) -> NormNode {
        lower(node)
    }

    fn lower_recursion(&self, root: NormNode) -> NormNode {
        lower_recursion(root)
    }

    fn rewrite_iteration(&self, root: NormNode) -> NormNode {
        rewrite_iteration(root)
    }

    fn normalize_loop_exit(&self, root: NormNode) -> NormNode {
        normalize_loop_exit(self, root)
    }

    fn commutative_ops(&self) -> &'static [&'static str] {
        &["+", "*", "&", "|", "^", "==", "!=", "and", "or"]
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
            "binary_operator" | "boolean_operator" => {
                Some((Some("left"), Some("operator"), Some("right")))
            }
            "comparison_operator" => Some((None, Some("operators"), None)),
            _ => None,
        }
    }

    fn sortable_pair_kind(&self, kind: &str) -> Option<&'static str> {
        match kind {
            "dictionary" => Some("key"),
            _ => None,
        }
    }

    fn is_list_kind(&self, kind: &str) -> bool {
        matches!(
            kind,
            "block"
                | "argument_list"
                | "parameters"
                | "pattern_list"
                | "expression_list"
                | "list"
                | "tuple"
                | "set"
                | "dictionary"
                | "REPEAT"
        )
    }

    fn is_dispatch_arm(&self, kind: &str) -> bool {
        kind == "case_clause"
    }

    fn is_statement_kind(&self, kind: &str) -> bool {
        matches!(
            kind,
            "expression_statement"
                | "if_statement"
                | "while_statement"
                | "for_statement"
                | "return_statement"
                | "try_statement"
                | "with_statement"
                | "break_statement"
                | "continue_statement"
                | "assert_statement"
                | "raise_statement"
                | "pass_statement"
                | "delete_statement"
        )
    }

    fn is_loop_core(&self, node: &NormNode) -> bool {
        node.kind.as_ref() == "while_statement"
            && node
                .children
                .iter()
                .any(|c| c.field.as_deref() == Some("condition") && c.kind.as_ref() == "true")
    }

    fn is_continue_stmt(&self, node: &NormNode) -> bool {
        node.kind.as_ref() == "continue_statement"
    }

    fn unit_is_test(&self, node: tree_sitter::Node, src: &str, name: &str, path: &Path) -> bool {
        if name.starts_with("test_") || name.ends_with("_test") || path_is_testy(path) {
            return true;
        }
        // Decorators live on the wrapping decorated_definition.
        if let Some(parent) = node.parent()
            && parent.kind() == "decorated_definition"
        {
            let text = parent
                .utf8_text(src.as_bytes())
                .unwrap_or_default()
                .lines()
                .take_while(|l| l.trim_start().starts_with('@'))
                .collect::<String>();
            if text.contains("pytest") || text.contains("fixture") || text.contains("test") {
                return true;
            }
        }
        false
    }

    // ---- Phase 3: inliner hooks (spec §5.4) ----

    fn call_kind(&self) -> &'static str {
        "call"
    }

    fn inline_params(&self, root: &NormNode) -> Option<Vec<Box<str>>> {
        simple_params(root)
    }

    fn return_value<'a>(&self, stmt: &'a NormNode) -> Option<&'a NormNode> {
        if stmt.kind.as_ref() == "return_statement" && stmt.children.len() == 1 {
            return Some(&stmt.children[0]);
        }
        None
    }

    fn make_return(&self, mut value: NormNode) -> NormNode {
        let span = value.span;
        value.field = None;
        NormNode::new("return_statement", None, span, vec![value])
    }

    fn make_expr_stmt(&self, mut expr: NormNode) -> NormNode {
        let span = expr.span;
        expr.field = None;
        NormNode::new("expression_statement", None, span, vec![expr])
    }

    fn make_expr_block(&self, _span: (u32, u32), _children: Vec<NormNode>) -> Option<NormNode> {
        // Python has no expression block; such call sites are skipped rather
        // than spliced as a synthetic kind nothing can converge with (D17).
        None
    }
}

fn collect_param_idents(node: &NormNode, out: &mut Vec<Box<str>>) {
    match node.field.as_deref() {
        Some("type") | Some("value") => return,
        _ => {}
    }
    if let Some(Label::Raw(text)) = &node.label
        && node.kind.as_ref() == "identifier"
    {
        out.push(text.clone());
    }
    for child in &node.children {
        collect_param_idents(child, out);
    }
}

/// Lower `while cond` / `for x in xs` to the `while True` core (spec §5.2.2),
/// mirroring the exact shapes tree-sitter produces for the manual form.
fn lower(mut node: NormNode) -> NormNode {
    node.children = node.children.into_iter().map(lower).collect();
    match node.kind.as_ref() {
        "while_statement" => {
            let is_true_cond = node
                .children
                .iter()
                .any(|c| c.field.as_deref() == Some("condition") && c.kind.as_ref() == "true");
            if is_true_cond {
                return node;
            }
            let Some(mut cond) = node.take_field("condition") else {
                return node;
            };
            let span = cond.span;
            cond.field = None;
            if let Some(body) = node
                .children
                .iter_mut()
                .find(|c| c.field.as_deref() == Some("body"))
            {
                body.children.insert(0, break_unless(cond));
            }
            node.children.insert(0, true_node(span));
            node
        }
        "for_statement" => {
            let span = node.span;
            let (Some(mut left), Some(mut right), Some(mut body)) = (
                node.take_field("left"),
                node.take_field("right"),
                node.take_field("body"),
            ) else {
                return node;
            };
            right.field = None;
            let guard = break_unless(call("__has_next", None, right.clone()));
            left.field = Some("left".into());
            let next = call("__next", Some("right"), right);
            let span_left = left.span;
            let assign = NormNode::new("assignment", None, span_left, vec![left, next]);
            let assign_stmt = NormNode::new("expression_statement", None, span_left, vec![assign]);
            body.children.insert(0, guard);
            body.children.insert(1, assign_stmt);
            let mut children = vec![true_node(span), body];
            children.append(&mut node.children);
            NormNode::new("while_statement", node.field.as_deref(), span, children)
        }
        _ => node,
    }
}

pub(crate) fn true_node(span: (u32, u32)) -> NormNode {
    NormNode::new("true", Some("condition"), span, Vec::new())
        .with_label(Label::RawLit("True".into()))
}

/// `if_statement(condition: not_operator(cond), consequence: block(break_statement))`
fn break_unless(cond: NormNode) -> NormNode {
    let span = cond.span;
    let mut inner = cond;
    // Manual `not (cond)` puts the operand in the `argument` field once the
    // parens flatten; the synthesized form must match exactly.
    inner.field = Some("argument".into());
    let negated = NormNode::new("not_operator", Some("condition"), span, vec![inner]);
    let brk = NormNode::new("break_statement", None, span, Vec::new());
    let consequence = NormNode::new("block", Some("consequence"), span, vec![brk]);
    NormNode::new("if_statement", None, span, vec![negated, consequence])
}

fn call(name: &str, field: Option<&str>, arg: NormNode) -> NormNode {
    synth_call("call", "argument_list", name, field, arg)
}

// ---- recursion lowering ----

fn lower_recursion(root: NormNode) -> NormNode {
    let Some(name) = raw_name(&root) else {
        return root;
    };
    let Some(params) = simple_params(&root) else {
        return root;
    };
    if params.is_empty() {
        return root;
    }
    let total = child_field(&root, "body")
        .map(|b| count_self_calls(b, "call", &name))
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
    let while_node = NormNode::new("while_statement", None, span, vec![true_node(span), inner]);
    let wrapper = NormNode::new("block", Some("body"), span, vec![while_node]);
    root.children.insert(body_idx, wrapper);
    root
}

fn simple_params(root: &NormNode) -> Option<Vec<Box<str>>> {
    let params = child_field(root, "parameters")?;
    let mut out = Vec::new();
    for p in &params.children {
        match (&p.kind, &p.label) {
            (k, Some(Label::Raw(text))) if k.as_ref() == "identifier" => out.push(text.clone()),
            _ => return None, // defaults / *args / typed params: bail
        }
    }
    Some(out)
}

fn is_self_call(node: &NormNode, name: &str) -> bool {
    node.kind.as_ref() == "call"
        && child_field(node, "function").is_some_and(|f| is_raw_ident(f, name))
}

fn rewrite_tail_sites(
    mut node: NormNode,
    name: &str,
    params: &[Box<str>],
    replaced: &mut u32,
) -> NormNode {
    if node.kind.as_ref() == "block" {
        let mut out = Vec::with_capacity(node.children.len());
        for child in node.children {
            // `return f(...)`
            let is_return_site = child.kind.as_ref() == "return_statement"
                && child.children.len() == 1
                && is_self_call(&child.children[0], name);
            if is_return_site {
                let call = child.children.into_iter().next().unwrap();
                out.extend(reassign_stmts(call, params, replaced));
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

/// `p1, p2 = a1, a2` + `continue` — identity pairs dropped.
fn reassign_stmts(call: NormNode, params: &[Box<str>], replaced: &mut u32) -> Vec<NormNode> {
    let span = call.span;
    let args: Vec<NormNode> = child_field(&call, "arguments")
        .map(|a| a.children.clone())
        .unwrap_or_default();
    if args.len() != params.len() {
        *replaced += 1;
        return vec![NormNode::new("continue_statement", None, span, Vec::new())];
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
        let assign = NormNode::new("assignment", None, span, vec![left, arg]);
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
        let left = NormNode::new("pattern_list", Some("left"), span, lefts);
        let right = NormNode::new("expression_list", Some("right"), span, rights);
        let assign = NormNode::new("assignment", None, span, vec![left, right]);
        out.push(NormNode::new(
            "expression_statement",
            None,
            span,
            vec![assign],
        ));
    }
    out.push(NormNode::new("continue_statement", None, span, Vec::new()));
    out
}

// ---- iteration-protocol rewrite: `for i in range(len(xs)): xs[i]` → `for i in xs: i` ----

fn rewrite_iteration(mut node: NormNode) -> NormNode {
    node.children = node.children.into_iter().map(rewrite_iteration).collect();
    if node.kind.as_ref() != "for_statement" {
        return node;
    }
    let (Some(left), Some(right)) = (child_field(&node, "left"), child_field(&node, "right"))
    else {
        return node;
    };
    let Some(Label::Raw(ivar)) = left.label.clone() else {
        return node;
    };
    let Some(collection) = range_len_collection(right) else {
        return node;
    };
    let Some(body) = child_field(&node, "body") else {
        return node;
    };
    if !index_uses_only(body, &ivar, &collection) {
        return node;
    }
    let span = right.span;
    let right_idx = node
        .children
        .iter()
        .position(|c| c.field.as_deref() == Some("right"))
        .unwrap();
    node.children[right_idx] = synth_ident(&collection, Some("right"), span);
    let body_idx = node
        .children
        .iter()
        .position(|c| c.field.as_deref() == Some("body"))
        .unwrap();
    let body = node.children.remove(body_idx);
    let new_body = replace_subscripts(body, &ivar, &collection);
    node.children.insert(body_idx, new_body);
    node
}

/// Matches `range(len(X))` → Some(X).
fn range_len_collection(right: &NormNode) -> Option<Box<str>> {
    if right.kind.as_ref() != "call" {
        return None;
    }
    let f = child_field(right, "function")?;
    if !is_raw_ident(f, "range") {
        return None;
    }
    let args = child_field(right, "arguments")?;
    if args.children.len() != 1 {
        return None;
    }
    let inner = &args.children[0];
    if inner.kind.as_ref() != "call" {
        return None;
    }
    let g = child_field(inner, "function")?;
    if !is_raw_ident(g, "len") {
        return None;
    }
    let inner_args = child_field(inner, "arguments")?;
    if inner_args.children.len() != 1 {
        return None;
    }
    let coll = &inner_args.children[0];
    match &coll.label {
        Some(Label::Raw(t)) if coll.kind.as_ref() == "identifier" => Some(t.clone()),
        _ => None,
    }
}

fn is_target_subscript(node: &NormNode, ivar: &str, coll: &str) -> bool {
    node.kind.as_ref() == "subscript"
        && child_field(node, "value").is_some_and(|v| is_raw_ident(v, coll))
        && child_field(node, "subscript").is_some_and(|s| is_raw_ident(s, ivar))
}

fn index_uses_only(node: &NormNode, ivar: &str, coll: &str) -> bool {
    if is_target_subscript(node, ivar, coll) {
        return true;
    }
    if is_raw_ident(node, ivar) {
        return false;
    }
    node.children.iter().all(|c| index_uses_only(c, ivar, coll))
}

fn replace_subscripts(mut node: NormNode, ivar: &str, coll: &str) -> NormNode {
    if is_target_subscript(&node, ivar, coll) {
        let field = node.field.as_deref().map(str::to_owned);
        return synth_ident(ivar, field.as_deref(), node.span);
    }
    node.children = node
        .children
        .into_iter()
        .map(|c| replace_subscripts(c, ivar, coll))
        .collect();
    node
}

// ---- loop-exit normalization ----

fn normalize_loop_exit(profile: &PythonProfile, mut root: NormNode) -> NormNode {
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
    let ret_value: Option<NormNode> = {
        let last = &body.children[n - 1];
        if last.kind.as_ref() == "return_statement" && last.children.len() == 1 {
            Some(last.children[0].clone())
        } else {
            None
        }
    };
    let Some(mut value) = ret_value else {
        return root;
    };
    value.field = None;
    let loop_stmt = &mut body.children[n - 2];
    if !profile.is_loop_core(loop_stmt) {
        return root;
    }
    let mut replaced = 0u32;
    replace_breaks(profile, loop_stmt, &value, &mut replaced, true);
    if replaced > 0 {
        body.children.truncate(n - 1);
    }
    root
}

fn replace_breaks(
    profile: &PythonProfile,
    node: &mut NormNode,
    value: &NormNode,
    replaced: &mut u32,
    top: bool,
) {
    if !top && profile.is_loop_core(node) {
        return;
    }
    let mut i = 0;
    while i < node.children.len() {
        if node.children[i].kind.as_ref() == "break_statement" {
            let span = node.children[i].span;
            node.children[i] = NormNode::new("return_statement", None, span, vec![value.clone()]);
            *replaced += 1;
        } else {
            replace_breaks(profile, &mut node.children[i], value, replaced, false);
        }
        i += 1;
    }
}
