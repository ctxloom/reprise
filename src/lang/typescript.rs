//! TypeScript / TSX language profile (spec §3, M3c). Both dialects share this
//! profile — their node-kind space is identical (TSX = TS + JSX). Node kinds
//! and lowered shapes are taken from `examples/probe3.rs` output (DECISIONS.md
//! D9: never a kind string unseen in probe output).

use super::{
    LanguageProfile, child_field, collect_pattern_idents, count_self_calls, is_raw_ident,
    path_is_testy, raw_name, synth_call, synth_ident,
};
use crate::tree::{Bucket, Label, NormNode};
use std::path::Path;

pub struct TypeScriptProfile;

impl LanguageProfile for TypeScriptProfile {
    fn is_function_like(&self, kind: &str) -> bool {
        matches!(kind, "function_declaration" | "method_definition")
    }

    fn binding_unit<'tree>(
        &self,
        node: tree_sitter::Node<'tree>,
        src: &str,
    ) -> Option<(String, tree_sitter::Node<'tree>)> {
        // `const f = (x) => …` / `let f = function () {…}` — a named callable
        // bound in a declaration. tree-sitter shape: variable_declarator with a
        // `name` identifier and a `value` that is an arrow_function or
        // function_expression. Anything else (object values, calls, inline
        // callbacks in argument position) is not a variable_declarator here, so
        // it is never extracted — decl-only, matching Rust/Python (D25 gap).
        if node.kind() != "variable_declarator" {
            return None;
        }
        let value = node.child_by_field_name("value")?;
        if !matches!(value.kind(), "arrow_function" | "function_expression") {
            return None;
        }
        let name = node
            .child_by_field_name("name")?
            .utf8_text(src.as_bytes())
            .ok()?
            .to_string();
        Some((name, value))
    }

    fn is_comment(&self, kind: &str) -> bool {
        kind == "comment"
    }

    fn strip_kind(&self, kind: &str) -> bool {
        // Type-level nodes are behaviorally neutral for clone detection (like
        // Rust lifetimes) and inflate/drift the tree; drop them wholesale.
        matches!(
            kind,
            "type_annotation"
                | "type_parameters"
                | "type_arguments"
                | "decorator"
                | "accessibility_modifier"
        )
    }

    fn is_identifier(&self, kind: &str) -> bool {
        matches!(
            kind,
            "identifier"
                | "property_identifier"
                | "type_identifier"
                | "shorthand_property_identifier"
                | "shorthand_property_identifier_pattern"
        )
    }

    fn literal_bucket(&self, kind: &str) -> Option<Bucket> {
        match kind {
            // TS has one `number` kind for int and float literals alike.
            "number" => Some(Bucket::Int),
            "string" | "template_string" => Some(Bucket::Str),
            "true" | "false" => Some(Bucket::Bool),
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
                | "unary_expression"
                | "augmented_assignment_expression"
                | "update_expression"
        )
    }

    fn always_external(&self, kind: &str, _field: Option<&str>, _parent_kind: &str) -> bool {
        matches!(
            kind,
            "property_identifier"
                | "type_identifier"
                | "shorthand_property_identifier"
                | "shorthand_property_identifier_pattern"
        )
    }

    fn collect_declared(&self, root: &NormNode, out: &mut Vec<Box<str>>) {
        fn walk(node: &NormNode, out: &mut Vec<Box<str>>) {
            match node.kind.as_ref() {
                "function_declaration"
                | "function_expression"
                | "arrow_function"
                | "method_definition" => {
                    if let Some(name) = child_field(node, "name")
                        && let Some(Label::Raw(text)) = &name.label
                    {
                        out.push(text.clone());
                    }
                    if let Some(params) = child_field(node, "parameters") {
                        collect_pattern_idents(params, out);
                    }
                }
                "variable_declarator" => {
                    if let Some(name) = child_field(node, "name") {
                        collect_pattern_idents(name, out);
                    }
                }
                "for_in_statement" => {
                    if let Some(left) = child_field(node, "left") {
                        collect_pattern_idents(left, out);
                    }
                }
                "catch_clause" => {
                    if let Some(param) = child_field(node, "parameter") {
                        collect_pattern_idents(param, out);
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
        &[
            "+", "*", "&", "|", "^", "&&", "||", "===", "!==", "==", "!=",
        ]
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
            "object" => Some("key"),
            _ => None,
        }
    }

    fn is_list_kind(&self, kind: &str) -> bool {
        matches!(
            kind,
            "statement_block" | "arguments" | "formal_parameters" | "array" | "object" | "REPEAT"
        )
    }

    fn is_statement_kind(&self, kind: &str) -> bool {
        matches!(
            kind,
            "expression_statement"
                | "lexical_declaration"
                | "variable_declaration"
                | "if_statement"
                | "for_statement"
                | "for_in_statement"
                | "while_statement"
                | "do_statement"
                | "return_statement"
                | "break_statement"
                | "continue_statement"
                | "switch_statement"
                | "try_statement"
                | "throw_statement"
                | "empty_statement"
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
        let fname = path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or_default();
        if fname.contains(".test.") || fname.contains(".spec.") || path_is_testy(path) {
            return true;
        }
        if name.starts_with("test") {
            return true;
        }
        // Wrapped in a describe/it/test callback (spec §5.1).
        let mut p = node.parent();
        while let Some(n) = p {
            if n.kind() == "call_expression"
                && let Some(f) = n.child_by_field_name("function")
                && matches!(
                    f.utf8_text(src.as_bytes()).unwrap_or_default(),
                    "describe" | "it" | "test" | "beforeEach" | "afterEach" | "suite"
                )
            {
                return true;
            }
            p = n.parent();
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
        // TS statement_block is not an expression; multi-statement inline
        // splices have no native landing spot (as Python, DECISIONS.md D17).
        None
    }
}

fn true_node(span: (u32, u32)) -> NormNode {
    NormNode::new("true", Some("condition"), span, Vec::new())
        .with_label(Label::RawLit("true".into()))
}

/// `if (!(cond)) { break; }` in the exact shape tree-sitter produces for the
/// manual core (probe `cores`): `unary_expression{ !, argument }` under an
/// `if_statement` with a `statement_block` consequence.
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
    let brk = NormNode::new("break_statement", None, span, Vec::new());
    let consequence = NormNode::new("statement_block", Some("consequence"), span, vec![brk]);
    NormNode::new("if_statement", None, span, vec![negated, consequence])
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

/// Lower while / do / for-of / for-in to the `while (true)` core; hoist
/// three-clause `for` inits and append their steps at the statement level.
fn lower(
    mut node: NormNode,
    label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
) -> NormNode {
    node.children = node
        .children
        .into_iter()
        .map(|c| lower(c, label_interner))
        .collect();
    // Expand three-clause fors inside statement containers (a for produces
    // init + while, which lower() alone cannot return as one node).
    if node.kind.as_ref() == "statement_block" {
        node.children = expand_for_clauses(node.children);
    }
    match node.kind.as_ref() {
        "while_statement" => {
            // Already the core (`while (true)`) — don't re-lower (else a
            // manually written core diverges from a lowered one).
            if node
                .children
                .iter()
                .any(|c| c.field.as_deref() == Some("condition") && c.kind.as_ref() == "true")
            {
                return node;
            }
            let field = node.field.as_deref().map(str::to_owned);
            let span = node.span;
            let Some(cond) = node.take_field("condition") else {
                return node;
            };
            let Some(mut body) = node.take_field("body") else {
                return node;
            };
            body.children.insert(0, break_unless(cond));
            body.field = Some("body".into());
            NormNode::new(
                "while_statement",
                field.as_deref(),
                span,
                vec![true_node(span), body],
            )
        }
        "do_statement" => {
            let field = node.field.as_deref().map(str::to_owned);
            let span = node.span;
            let Some(cond) = node.take_field("condition") else {
                return node;
            };
            let Some(mut body) = node.take_field("body") else {
                return node;
            };
            body.children.push(break_unless(cond)); // post-test: break at tail
            body.field = Some("body".into());
            NormNode::new(
                "while_statement",
                field.as_deref(),
                span,
                vec![true_node(span), body],
            )
        }
        "for_in_statement" => lower_for_in(node, label_interner),
        // Arrow → function expression (spec §5.2.3): `(x) => x+1` converges
        // with `function(x){ return x+1 }`. Only parenthesized-param arrows
        // are canonicalized; bare-param arrows (`x => …`) are left as-is
        // (DECISIONS.md D23 records the residual gap).
        "arrow_function" => desugar_arrow(node),
        _ => node,
    }
}

fn desugar_arrow(mut node: NormNode) -> NormNode {
    let span = node.span;
    let field = node.field.as_deref().map(str::to_owned);
    let Some(params) = node.take_field("parameters") else {
        return node;
    };
    let Some(mut body) = node.take_field("body") else {
        return node;
    };
    if body.kind.as_ref() != "statement_block" {
        // Expression body → `{ return <expr>; }`.
        body.field = None;
        let ret = NormNode::new("return_statement", None, body.span, vec![body]);
        body = NormNode::new("statement_block", None, span, vec![ret]);
    }
    body.field = Some("body".into());
    NormNode::new(
        "function_expression",
        field.as_deref(),
        span,
        vec![params, body],
    )
}

/// `for (const x of xs) { body }` / `for (const k in obj) { body }` →
/// `while (true) { if (!__has_next(xs)) break; const x = __next(xs); body }`.
/// (of/in are dropped anon tokens post-convert, so both forms lower alike —
/// accepted, DECISIONS.md D23.)
fn lower_for_in(
    mut node: NormNode,
    label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
) -> NormNode {
    let field = node.field.as_deref().map(str::to_owned);
    let span = node.span;
    let (Some(mut left), Some(mut right), Some(mut body)) = (
        node.take_field("left"),
        node.take_field("right"),
        node.take_field("body"),
    ) else {
        return node;
    };
    right.field = None;
    let guard = break_unless(call("__has_next", None, right.clone(), label_interner));
    left.field = Some("name".into());
    let next = call("__next", Some("value"), right, label_interner);
    let declr = NormNode::new("variable_declarator", None, span, vec![left, next]);
    let bind = NormNode::new("lexical_declaration", None, span, vec![declr]);
    body.children.insert(0, guard);
    body.children.insert(1, bind);
    body.field = Some("body".into());
    NormNode::new(
        "while_statement",
        field.as_deref(),
        span,
        vec![true_node(span), body],
    )
}

/// Replace each three-clause `for_statement` in a statement list with
/// `[initializer, while(true){ break-guard; body; increment }]`.
fn expand_for_clauses(children: Vec<NormNode>) -> Vec<NormNode> {
    let mut out = Vec::with_capacity(children.len());
    for child in children {
        if child.kind.as_ref() == "for_statement" {
            out.extend(lower_three_clause(child));
        } else {
            out.push(child);
        }
    }
    out
}

fn lower_three_clause(mut node: NormNode) -> Vec<NormNode> {
    let span = node.span;
    let init = node.take_field("initializer");
    let cond = node.take_field("condition");
    let incr = node.take_field("increment");
    let Some(mut body) = node.take_field("body") else {
        return vec![node];
    };
    if let Some(mut incr) = incr {
        incr.field = None;
        body.children.push(NormNode::new(
            "expression_statement",
            None,
            incr.span,
            vec![incr],
        ));
    }
    if let Some(cond) = cond {
        body.children.insert(0, break_unless(cond));
    }
    body.field = Some("body".into());
    let loop_node = NormNode::new("while_statement", None, span, vec![true_node(span), body]);
    match init {
        Some(mut init) => {
            init.field = None;
            vec![init, loop_node]
        }
        None => vec![loop_node],
    }
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
    let loop_node = NormNode::new("while_statement", None, span, vec![true_node(span), inner]);
    let wrapper = NormNode::new("statement_block", Some("body"), span, vec![loop_node]);
    root.children.insert(body_idx, wrapper);
    root
}

fn simple_params(root: &NormNode) -> Option<Vec<Box<str>>> {
    let params = child_field(root, "parameters")?;
    let mut out = Vec::new();
    for p in &params.children {
        if p.kind.as_ref() != "required_parameter" {
            return None; // optional / rest / destructured: bail
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

/// Nested callable kinds — a `return <self-call>` inside one of these is the
/// INNER function's tail position, not the outer unit's, so recursion lowering
/// must not reach into it (else it emits `continue` outside any loop, reassigns
/// the outer params, and the shared `count_self_calls` census still counts the
/// nested call so `replaced == total` and the bad lowering commits). Stopping
/// descent here leaves the nested call uncounted → `replaced != total` → the
/// whole unit safely bails out of recursion lowering. Runs pre-desugar, so
/// arrows are still `arrow_function`.
fn is_nested_callable(kind: &str) -> bool {
    matches!(
        kind,
        "arrow_function"
            | "function_expression"
            | "function_declaration"
            | "generator_function"
            | "generator_function_declaration"
            | "method_definition"
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
    if node.kind.as_ref() == "statement_block" {
        let mut out = Vec::with_capacity(node.children.len());
        for child in node.children {
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

/// `[p1, p2] = [a1, a2]; continue;` (array-destructuring assignment), identity
/// pairs dropped — matching the shape an iterative rewrite carries.
fn reassign_stmts(call: NormNode, params: &[Box<str>], replaced: &mut u32) -> Vec<NormNode> {
    let span = call.span;
    let args: Vec<NormNode> = child_field(&call, "arguments")
        .map(|a| a.children.clone())
        .unwrap_or_default();
    if args.len() != params.len() {
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
        let left = NormNode::new("array_pattern", Some("left"), span, lefts);
        let right = NormNode::new("array", Some("right"), span, rights);
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
    NormNode::new("continue_statement", None, span, Vec::new())
}

// ---- iteration-protocol rewrite: `for (let i=0; i<xs.length; i++)` → for-of ----

fn rewrite_iteration(mut node: NormNode) -> NormNode {
    node.children = node.children.into_iter().map(rewrite_iteration).collect();
    if node.kind.as_ref() != "for_statement" {
        return node;
    }
    let Some((ivar, coll)) = index_for_match(&node) else {
        return node;
    };
    let Some(mut body) = node.take_field("body") else {
        return node;
    };
    let span = node.span;
    body = replace_subscripts(body, &ivar, &coll);
    body.field = Some("body".into());
    NormNode::new(
        "for_in_statement",
        node.field.as_deref(),
        span,
        vec![
            synth_ident(&ivar, Some("left"), span),
            synth_ident(&coll, Some("right"), span),
            body,
        ],
    )
}

/// Matches `for (let i = 0; i < coll.length; i++) { …only coll[i]… }`.
fn index_for_match(node: &NormNode) -> Option<(Box<str>, Box<str>)> {
    let init = child_field(node, "initializer")?;
    if init.kind.as_ref() != "lexical_declaration" || init.children.len() != 1 {
        return None;
    }
    let declr = &init.children[0];
    let ivar = match &child_field(declr, "name")?.label {
        Some(Label::Raw(t)) => t.clone(),
        _ => return None,
    };
    let zero = child_field(declr, "value")?;
    if !matches!(&zero.label, Some(Label::RawLit(t)) if t.as_ref() == "0") {
        return None;
    }
    // condition: i < coll.length
    let cond = child_field(node, "condition")?;
    if cond.kind.as_ref() != "binary_expression" || cond.children.len() != 3 {
        return None;
    }
    if !is_raw_ident(&cond.children[0], &ivar) || cond.children[1].kind.as_ref() != "<" {
        return None;
    }
    let member = &cond.children[2];
    if member.kind.as_ref() != "member_expression" {
        return None;
    }
    let coll = match &child_field(member, "object")?.label {
        Some(Label::Raw(t)) => t.clone(),
        _ => return None,
    };
    match &child_field(member, "property")?.label {
        Some(Label::Raw(t)) if t.as_ref() == "length" => {}
        _ => return None,
    }
    // increment: i++
    let incr = child_field(node, "increment")?;
    if incr.kind.as_ref() != "update_expression" || !is_raw_ident(&incr.children[0], &ivar) {
        return None;
    }
    let body = child_field(node, "body")?;
    if !index_uses_only(body, &ivar, &coll) {
        return None;
    }
    Some((ivar, coll))
}

fn is_target_subscript(node: &NormNode, ivar: &str, coll: &str) -> bool {
    node.kind.as_ref() == "subscript_expression"
        && child_field(node, "object").is_some_and(|v| is_raw_ident(v, coll))
        && child_field(node, "index").is_some_and(|s| is_raw_ident(s, ivar))
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

fn normalize_loop_exit(profile: &TypeScriptProfile, mut root: NormNode) -> NormNode {
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
    profile: &TypeScriptProfile,
    node: &mut NormNode,
    value: &NormNode,
    replaced: &mut u32,
    top: bool,
) {
    if !top && profile.is_loop_core(node) {
        return;
    }
    // A `break` inside a switch targets the SWITCH, not the loop, so it must
    // stay a switch exit — never become the loop's `return`. Don't descend into
    // switch bodies (the lowered nested loops are already stopped above; a
    // labeled `break foo` carries a label child, so the `is_empty` guard below
    // never rewrites it either).
    if node.kind.as_ref() == "switch_statement" {
        return;
    }
    let mut i = 0;
    while i < node.children.len() {
        if node.children[i].kind.as_ref() == "break_statement"
            && node.children[i].children.is_empty()
        {
            let span = node.children[i].span;
            let mut v = value.clone();
            v.field = None;
            node.children[i] = NormNode::new("return_statement", None, span, vec![v]);
            *replaced += 1;
        } else {
            replace_breaks(profile, &mut node.children[i], value, replaced, false);
        }
        i += 1;
    }
}
