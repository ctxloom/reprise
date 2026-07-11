//! Kotlin language profile (spec §3, M3c), grammar `tree-sitter-kotlin-ng`
//! (DECISIONS.md D23 records the crate choice). Node kinds and lowered shapes
//! taken from `examples/probe3.rs` output (DECISIONS.md D9).
//!
//! Kotlin quirks handled here: the function body is `function_body > block`
//! with no `body` field (navigated by kind); `break`/`continue`/`true` parse as
//! bare `identifier`s; navigation targets are relabeled External by a pre-pass
//! (no distinct field/kind distinguishes them positionally).

use super::{
    LanguageProfile, child_field, is_raw_ident, path_is_testy, raw_name, synth_call, synth_ident,
};
use crate::tree::{Bucket, Label, NormNode};
use std::path::Path;

pub struct KotlinProfile;

impl LanguageProfile for KotlinProfile {
    fn is_function_like(&self, kind: &str) -> bool {
        kind == "function_declaration"
    }

    fn is_comment(&self, kind: &str) -> bool {
        matches!(kind, "line_comment" | "block_comment" | "comment")
    }

    fn strip_kind(&self, kind: &str) -> bool {
        // Annotations + visibility/other modifiers: noise for structural
        // matching (@Test is still read from the raw CST by unit_is_test).
        kind == "modifiers"
    }

    fn splice_kind(&self, kind: &str) -> bool {
        // Hoist the function body's block out of its `function_body` wrapper so
        // it becomes a direct child of `function_declaration`.
        kind == "function_body"
    }

    fn is_identifier(&self, kind: &str) -> bool {
        kind == "identifier"
    }

    fn literal_bucket(&self, kind: &str) -> Option<Bucket> {
        match kind {
            "number_literal" | "integer_literal" | "hex_literal" | "bin_literal" => {
                Some(Bucket::Int)
            }
            "float_literal" | "real_literal" => Some(Bucket::Float),
            "string_literal" | "line_string_literal" => Some(Bucket::Str),
            "character_literal" => Some(Bucket::Char),
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
            "binary_expression" | "unary_expression" | "assignment"
        )
    }

    fn always_external(&self, _kind: &str, _field: Option<&str>, _parent_kind: &str) -> bool {
        // Navigation targets are handled by `externalize_nav` (no positional
        // field distinguishes them); nothing else is kind-based external.
        false
    }

    fn collect_declared(&self, root: &NormNode, out: &mut Vec<Box<str>>) {
        fn direct_idents(node: &NormNode, out: &mut Vec<Box<str>>) {
            for c in &node.children {
                if c.kind.as_ref() == "identifier"
                    && let Some(Label::Raw(text)) = &c.label
                {
                    out.push(text.clone());
                }
            }
        }
        fn walk(node: &NormNode, out: &mut Vec<Box<str>>) {
            match node.kind.as_ref() {
                "function_declaration" => {
                    if let Some(name) = child_field(node, "name")
                        && let Some(Label::Raw(text)) = &name.label
                    {
                        out.push(text.clone());
                    }
                }
                // `parameter { identifier name, user_type }` — the direct
                // identifier is the name; type identifiers are nested deeper.
                "parameter" => direct_idents(node, out),
                // val/var/for-loop binder.
                "variable_declaration" => direct_idents(node, out),
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
        lower(externalize_nav(node, label_interner), label_interner)
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
        &["+", "*", "==", "!=", "&&", "||"]
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

    fn sortable_pair_kind(&self, _kind: &str) -> Option<&'static str> {
        None
    }

    fn is_list_kind(&self, kind: &str) -> bool {
        matches!(
            kind,
            "block" | "value_arguments" | "function_value_parameters" | "REPEAT"
        )
    }

    fn is_statement_kind(&self, kind: &str) -> bool {
        matches!(
            kind,
            "property_declaration"
                | "assignment"
                | "for_statement"
                | "while_statement"
                | "do_while_statement"
                | "if_expression"
                | "when_expression"
                | "return_expression"
                | "call_expression"
        )
    }

    fn is_loop_core(&self, node: &NormNode) -> bool {
        node.kind.as_ref() == "while_statement"
            && node
                .children
                .iter()
                .any(|c| c.field.as_deref() == Some("condition") && is_true_ident(c))
    }

    fn is_continue_stmt(&self, node: &NormNode) -> bool {
        node.kind.as_ref() == "identifier" && ident_text_is(node, "continue")
    }

    fn unit_is_test(&self, node: tree_sitter::Node, src: &str, name: &str, path: &Path) -> bool {
        if name.starts_with("test") || path_is_testy(path) {
            return true;
        }
        // @Test / @ParameterizedTest live in the leading `modifiers` child.
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "modifiers" {
                let text = child.utf8_text(src.as_bytes()).unwrap_or_default();
                if text.contains("Test") {
                    return true;
                }
                break;
            }
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
        if stmt.kind.as_ref() == "return_expression" && stmt.children.len() == 1 {
            return Some(&stmt.children[0]);
        }
        None
    }

    fn make_return(&self, mut value: NormNode) -> NormNode {
        let span = value.span;
        value.field = None;
        NormNode::new("return_expression", None, span, vec![value])
    }

    fn make_expr_stmt(&self, mut expr: NormNode) -> NormNode {
        // Kotlin statements are bare expressions (no wrapper node).
        expr.field = None;
        expr
    }

    fn make_expr_block(&self, _span: (u32, u32), _children: Vec<NormNode>) -> Option<NormNode> {
        None
    }
}

fn ident_text_is(node: &NormNode, text: &str) -> bool {
    match &node.label {
        Some(Label::Raw(t)) => t.as_ref() == text,
        Some(Label::External(t)) => t.as_ref() == text,
        _ => false,
    }
}

fn is_true_ident(node: &NormNode) -> bool {
    node.kind.as_ref() == "identifier" && ident_text_is(node, "true")
}

fn true_node(span: (u32, u32)) -> NormNode {
    synth_ident("true", Some("condition"), span)
}

fn eq_token(span: (u32, u32)) -> NormNode {
    NormNode::new("=", Some("operator"), span, Vec::new())
}

/// Relabel navigation targets (`.field`) as External — no field/kind positions
/// them, so a bare undeclared→External default would misfire when a target name
/// collides with a declared local.
fn externalize_nav(
    mut node: NormNode,
    label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
) -> NormNode {
    node.children = node
        .children
        .into_iter()
        .map(|c| externalize_nav(c, label_interner))
        .collect();
    if node.kind.as_ref() == "navigation_expression"
        && let Some(last) = node.children.last_mut()
        && last.kind.as_ref() == "identifier"
        && let Some(Label::Raw(t)) = &last.label
    {
        last.label = Some(Label::External(label_interner.intern(t)));
    }
    node
}

/// `if (!(cond)) { break }` in the manual shape (probe `cores`): Kotlin uses
/// `if_expression` with a fieldless `block` and a bare `identifier "break"`.
fn break_unless(mut cond: NormNode) -> NormNode {
    let span = cond.span;
    cond.field = Some("argument".into());
    let bang = NormNode::token("!", span);
    let negated = NormNode::new(
        "unary_expression",
        Some("condition"),
        span,
        vec![bang, cond],
    );
    let brk = synth_ident("break", None, span);
    let block = NormNode::new("block", None, span, vec![brk]);
    NormNode::new("if_expression", None, span, vec![negated, block])
}

fn call(
    name: &str,
    arg: NormNode,
    label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
) -> NormNode {
    synth_call(
        "call_expression",
        "value_arguments",
        name,
        None,
        arg,
        label_interner,
    )
}

/// The function body block, navigated by kind (`function_body` was spliced, so
/// the block is a direct child with no field).
fn body_block_idx(root: &NormNode) -> Option<usize> {
    root.children
        .iter()
        .position(|c| c.kind.as_ref() == "block")
}

/// Lower while / do-while / for-in to the `while (true)` core (spec §5.2.2).
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
        "while_statement" => {
            // Already the core (`while (true)`) — don't re-lower.
            if node
                .children
                .iter()
                .any(|c| c.field.as_deref() == Some("condition") && is_true_ident(c))
            {
                return node;
            }
            let span = node.span;
            let field = node.field.as_deref().map(str::to_owned);
            let Some(cond) = node.take_field("condition") else {
                return node;
            };
            let Some(block_idx) = node
                .children
                .iter()
                .position(|c| c.kind.as_ref() == "block")
            else {
                return node;
            };
            node.children[block_idx]
                .children
                .insert(0, break_unless(cond));
            let block = node.children.remove(block_idx);
            NormNode::new(
                "while_statement",
                field.as_deref(),
                span,
                vec![true_node(span), block],
            )
        }
        "do_while_statement" => {
            let span = node.span;
            let field = node.field.as_deref().map(str::to_owned);
            let Some(cond) = node.take_field("condition") else {
                return node;
            };
            let Some(block_idx) = node
                .children
                .iter()
                .position(|c| c.kind.as_ref() == "block")
            else {
                return node;
            };
            node.children[block_idx].children.push(break_unless(cond)); // post-test
            let block = node.children.remove(block_idx);
            NormNode::new(
                "while_statement",
                field.as_deref(),
                span,
                vec![true_node(span), block],
            )
        }
        "for_statement" => lower_for_in(node, label_interner),
        _ => node,
    }
}

/// `for (x in xs) { body }` → `while (true) { if (!__has_next(xs)) break; x = __next(xs); body }`.
/// for_statement children (post-convert): variable_declaration, iterable, block.
fn lower_for_in(
    mut node: NormNode,
    label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
) -> NormNode {
    let span = node.span;
    let field = node.field.as_deref().map(str::to_owned);
    let Some(block_idx) = node
        .children
        .iter()
        .position(|c| c.kind.as_ref() == "block")
    else {
        return node;
    };
    let mut block = node.children.remove(block_idx);
    let Some(var_idx) = node
        .children
        .iter()
        .position(|c| c.kind.as_ref() == "variable_declaration")
    else {
        return node;
    };
    let mut var = node.children.remove(var_idx);
    var.field = None;
    // The remaining non-block child is the iterable.
    let Some(iter_idx) = node.children.iter().position(|c| c.field.is_none()) else {
        return node;
    };
    let mut iterable = node.children.remove(iter_idx);
    iterable.field = None;
    let guard = break_unless(call("__has_next", iterable.clone(), label_interner));
    // Bind via `property_declaration` (a declaring form) so `collect_declared`
    // binds the loop variable as a local, like the natural `val x = …`.
    let next = call("__next", iterable, label_interner);
    let bind = NormNode::new("property_declaration", None, span, vec![var, next]);
    block.children.insert(0, guard);
    block.children.insert(1, bind);
    NormNode::new(
        "while_statement",
        field.as_deref(),
        span,
        vec![true_node(span), block],
    )
}

// ---- iteration-protocol rewrite: `for (i in xs.indices)` / `0 until xs.size` ----

fn rewrite_iteration(mut node: NormNode) -> NormNode {
    node.children = node.children.into_iter().map(rewrite_iteration).collect();
    if node.kind.as_ref() != "for_statement" {
        return node;
    }
    let Some((ivar, coll)) = index_for_match(&node) else {
        return node;
    };
    let span = node.span;
    let Some(block_idx) = node
        .children
        .iter()
        .position(|c| c.kind.as_ref() == "block")
    else {
        return node;
    };
    let mut block = node.children.remove(block_idx);
    block = replace_index(block, &ivar, &coll);
    // Rebuild `for (ivar in coll) { block }` — lower_for_in handles the rest.
    let var = NormNode::new(
        "variable_declaration",
        None,
        span,
        vec![synth_ident(&ivar, None, span)],
    );
    let iterable = synth_ident(&coll, None, span);
    NormNode::new(
        "for_statement",
        node.field.as_deref(),
        span,
        vec![var, iterable, block],
    )
}

/// Matches `for (i in coll.indices)` or `for (i in 0 until coll.size)` whose
/// body uses `i` only as `coll[i]`. Returns (ivar, coll).
fn index_for_match(node: &NormNode) -> Option<(Box<str>, Box<str>)> {
    let var = node
        .children
        .iter()
        .find(|c| c.kind.as_ref() == "variable_declaration")?;
    let ivar = match var.children.first().map(|c| (&c.kind, &c.label)) {
        Some((k, Some(Label::Raw(t)))) if k.as_ref() == "identifier" => t.clone(),
        _ => return None,
    };
    // The iterable is the fieldless, non-block, non-variable child.
    let iterable = node.children.iter().find(|c| {
        c.field.is_none() && c.kind.as_ref() != "block" && c.kind.as_ref() != "variable_declaration"
    })?;
    let coll = iterable_collection(iterable)?;
    let block = node.children.iter().find(|c| c.kind.as_ref() == "block")?;
    if !index_uses_only(block, &ivar, &coll) {
        return None;
    }
    Some((ivar, coll))
}

/// `coll.indices` or `0 until coll.size` → Some(coll).
fn iterable_collection(node: &NormNode) -> Option<Box<str>> {
    if node.kind.as_ref() == "navigation_expression" {
        // coll.indices
        let base = navigation_base(node, "indices")?;
        return Some(base);
    }
    if node.kind.as_ref() == "infix_expression" && node.children.len() == 3 {
        // 0 until coll.size
        if !matches!(&node.children[0].label, Some(Label::RawLit(t)) if t.as_ref() == "0") {
            return None;
        }
        if !ident_raw_or_ext(&node.children[1], "until") {
            return None;
        }
        if node.children[2].kind.as_ref() == "navigation_expression" {
            return navigation_base(&node.children[2], "size");
        }
    }
    None
}

/// `base.<suffix>` where base is a plain identifier → Some(base text).
fn navigation_base(nav: &NormNode, suffix: &str) -> Option<Box<str>> {
    if nav.children.len() != 2 {
        return None;
    }
    if !ident_raw_or_ext(&nav.children[1], suffix) {
        return None;
    }
    match (&nav.children[0].kind, &nav.children[0].label) {
        (k, Some(Label::Raw(t))) if k.as_ref() == "identifier" => Some(t.clone()),
        _ => None,
    }
}

fn ident_raw_or_ext(node: &NormNode, text: &str) -> bool {
    node.kind.as_ref() == "identifier" && ident_text_is(node, text)
}

fn is_target_index(node: &NormNode, ivar: &str, coll: &str) -> bool {
    node.kind.as_ref() == "index_expression"
        && node.children.len() == 2
        && is_raw_ident(&node.children[0], coll)
        && is_raw_ident(&node.children[1], ivar)
}

fn index_uses_only(node: &NormNode, ivar: &str, coll: &str) -> bool {
    if is_target_index(node, ivar, coll) {
        return true;
    }
    if is_raw_ident(node, ivar) {
        return false;
    }
    node.children.iter().all(|c| index_uses_only(c, ivar, coll))
}

fn replace_index(mut node: NormNode, ivar: &str, coll: &str) -> NormNode {
    if is_target_index(&node, ivar, coll) {
        let field = node.field.as_deref().map(str::to_owned);
        return synth_ident(ivar, field.as_deref(), node.span);
    }
    node.children = node
        .children
        .into_iter()
        .map(|c| replace_index(c, ivar, coll))
        .collect();
    node
}

// ---- recursion lowering (spec §5.2.2 Rev 5) ----

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
    let Some(body_idx) = body_block_idx(&root) else {
        return root;
    };
    let total = count_self_calls(&root.children[body_idx], &name);
    if total == 0 {
        return root;
    }
    let mut root = root;
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
    let span = new_body.span;
    let loop_node = NormNode::new(
        "while_statement",
        None,
        span,
        vec![true_node(span), new_body],
    );
    // Function body is a `block` holding the single while-core statement.
    let wrapper = NormNode::new("block", None, span, vec![loop_node]);
    root.children[body_idx] = wrapper;
    root
}

fn simple_params(root: &NormNode) -> Option<Vec<Box<str>>> {
    let params = root
        .children
        .iter()
        .find(|c| c.kind.as_ref() == "function_value_parameters")?;
    let mut out = Vec::new();
    for p in &params.children {
        if p.kind.as_ref() != "parameter" {
            return None;
        }
        // First identifier child is the name (type identifiers are nested).
        match p.children.first().map(|c| (&c.kind, &c.label)) {
            Some((k, Some(Label::Raw(t)))) if k.as_ref() == "identifier" => out.push(t.clone()),
            _ => return None,
        }
    }
    Some(out)
}

/// Kotlin call callee is `children[0]` (no `function` field).
fn kt_is_self_call(node: &NormNode, name: &str) -> bool {
    node.kind.as_ref() == "call_expression"
        && node.children.first().is_some_and(|f| is_raw_ident(f, name))
}

fn count_self_calls(node: &NormNode, name: &str) -> u32 {
    let here = u32::from(kt_is_self_call(node, name));
    here + node
        .children
        .iter()
        .map(|c| count_self_calls(c, name))
        .sum::<u32>()
}

/// Nested callable kinds — a `return <self-call>` inside a nested local `fun`,
/// a lambda, or an anonymous function is the INNER callable's tail position, not
/// the outer unit's. Descending would emit `continue` outside any loop and
/// reassign the outer params, while the local `count_self_calls` census still
/// counts the nested call (`replaced == total`) so the bad lowering would
/// commit. Stopping descent leaves the nested call uncounted → `replaced !=
/// total` → the unit safely bails out of recursion lowering.
fn is_nested_callable(kind: &str) -> bool {
    matches!(
        kind,
        "function_declaration" | "lambda_literal" | "anonymous_function"
    )
}

fn rewrite_tail_sites(
    mut node: NormNode,
    name: &str,
    params: &[Box<str>],
    replaced: &mut u32,
) -> NormNode {
    if is_nested_callable(node.kind.as_ref()) {
        return node; // a self-call inside a nested closure is not a tail site
    }
    if node.kind.as_ref() == "block" {
        let mut out = Vec::with_capacity(node.children.len());
        for child in node.children {
            // `return f(...)`: return_expression { call_expression }
            let is_return_site = child.kind.as_ref() == "return_expression"
                && child.children.len() == 1
                && kt_is_self_call(&child.children[0], name);
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

/// `p_i = a_i` (one assignment per changed param — Kotlin has no simultaneous
/// multi-assignment; the sequential form is unsound for coupled updates but
/// tolerated for detection, DECISIONS.md D23) then `continue`.
fn reassign_stmts(call: NormNode, params: &[Box<str>], replaced: &mut u32) -> Vec<NormNode> {
    let span = call.span;
    let args: Vec<NormNode> = call
        .children
        .iter()
        .find(|c| c.kind.as_ref() == "value_arguments")
        .map(|a| a.children.iter().map(unwrap_arg).collect())
        .unwrap_or_default();
    if args.len() != params.len() {
        *replaced += 1;
        return vec![continue_ident(span)];
    }
    *replaced += 1;
    let mut out = Vec::new();
    for (i, arg) in args.into_iter().enumerate() {
        if is_raw_ident(&arg, &params[i]) {
            continue; // identity: skip
        }
        let mut arg = arg;
        arg.field = Some("right".into());
        let left = synth_ident(&params[i], Some("left"), span);
        out.push(NormNode::new(
            "assignment",
            None,
            span,
            vec![left, eq_token(span), arg],
        ));
    }
    out.push(continue_ident(span));
    out
}

/// `value_argument { <expr> }` → the inner expr (call args are wrapped).
fn unwrap_arg(node: &NormNode) -> NormNode {
    if node.kind.as_ref() == "value_argument" && node.children.len() == 1 {
        node.children[0].clone()
    } else {
        node.clone()
    }
}

fn continue_ident(span: (u32, u32)) -> NormNode {
    synth_ident("continue", None, span)
}

// ---- loop-exit normalization ----

fn normalize_loop_exit(profile: &KotlinProfile, mut root: NormNode) -> NormNode {
    let Some(body_idx) = body_block_idx(&root) else {
        return root;
    };
    let body = &mut root.children[body_idx];
    let n = body.children.len();
    if n < 2 {
        return root;
    }
    let ret_value: Option<NormNode> = {
        let last = &body.children[n - 1];
        if last.kind.as_ref() == "return_expression" && last.children.len() == 1 {
            Some(last.children[0].clone())
        } else {
            None
        }
    };
    let Some(value) = ret_value else {
        return root;
    };
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
    profile: &KotlinProfile,
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
        if node.children[i].kind.as_ref() == "identifier"
            && ident_text_is(&node.children[i], "break")
        {
            let span = node.children[i].span;
            let mut v = value.clone();
            v.field = None;
            node.children[i] = NormNode::new("return_expression", None, span, vec![v]);
            *replaced += 1;
        } else {
            replace_breaks(profile, &mut node.children[i], value, replaced, false);
        }
        i += 1;
    }
}
