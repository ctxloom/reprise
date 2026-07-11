//! Python language profile.

use super::{
    LanguageProfile, child_field, collect_pattern_idents, count_self_calls, is_raw_ident,
    path_is_testy, raw_name, synth_call, synth_ident,
};
use crate::intern::{Field, Kind};
use crate::tree::{Bucket, Label, NormNode};
use std::path::Path;
use std::sync::LazyLock;

// ---- file-local interned kind/field statics (interning-id-conversion WP) ----
//
// None of these are the canonical IR vocabulary (`crate::ir::kind`/`crate::ir::field`)
// — this file is the HISTORICAL Python grammar profile, entirely in tree-sitter's own
// node-kind/field-name space, so every literal below gets its own `LazyLock`
// (mechanism rule 2), even where a literal happens to share text with a pre-registered
// IR const (e.g. `"left"`/`"body"`/`"name"`): `Kind`/`Field::intern` is idempotent, so
// these resolve to the same id as any IR usage, but this file has no business reaching
// into `ir::kind::id`/`ir::field::id` for a grammar-native meaning it doesn't share.

// -- kinds --
static FUNCTION_DEFINITION: LazyLock<Kind> = LazyLock::new(|| Kind::intern("function_definition"));
static IDENTIFIER: LazyLock<Kind> = LazyLock::new(|| Kind::intern("identifier"));
static KEYWORD_ARGUMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("keyword_argument"));
static BINARY_OPERATOR: LazyLock<Kind> = LazyLock::new(|| Kind::intern("binary_operator"));
static BOOLEAN_OPERATOR: LazyLock<Kind> = LazyLock::new(|| Kind::intern("boolean_operator"));
static COMPARISON_OPERATOR: LazyLock<Kind> = LazyLock::new(|| Kind::intern("comparison_operator"));
static DICTIONARY: LazyLock<Kind> = LazyLock::new(|| Kind::intern("dictionary"));
static BLOCK: LazyLock<Kind> = LazyLock::new(|| Kind::intern("block"));
static ARGUMENT_LIST: LazyLock<Kind> = LazyLock::new(|| Kind::intern("argument_list"));
static PARAMETERS: LazyLock<Kind> = LazyLock::new(|| Kind::intern("parameters"));
static PATTERN_LIST: LazyLock<Kind> = LazyLock::new(|| Kind::intern("pattern_list"));
static EXPRESSION_LIST: LazyLock<Kind> = LazyLock::new(|| Kind::intern("expression_list"));
static LIST: LazyLock<Kind> = LazyLock::new(|| Kind::intern("list"));
static TUPLE: LazyLock<Kind> = LazyLock::new(|| Kind::intern("tuple"));
static SET: LazyLock<Kind> = LazyLock::new(|| Kind::intern("set"));
static REPEAT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("REPEAT"));
static CASE_CLAUSE: LazyLock<Kind> = LazyLock::new(|| Kind::intern("case_clause"));
static EXPRESSION_STATEMENT: LazyLock<Kind> =
    LazyLock::new(|| Kind::intern("expression_statement"));
static IF_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("if_statement"));
static WHILE_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("while_statement"));
static FOR_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("for_statement"));
static RETURN_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("return_statement"));
static TRY_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("try_statement"));
static WITH_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("with_statement"));
static BREAK_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("break_statement"));
static CONTINUE_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("continue_statement"));
static ASSERT_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("assert_statement"));
static RAISE_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("raise_statement"));
static PASS_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("pass_statement"));
static DELETE_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("delete_statement"));
static TRUE_KIND: LazyLock<Kind> = LazyLock::new(|| Kind::intern("true"));
static CALL: LazyLock<Kind> = LazyLock::new(|| Kind::intern("call"));
static GLOBAL_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("global_statement"));
static NONLOCAL_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("nonlocal_statement"));
static LAMBDA_PARAMETERS: LazyLock<Kind> = LazyLock::new(|| Kind::intern("lambda_parameters"));
static AUGMENTED_ASSIGNMENT: LazyLock<Kind> =
    LazyLock::new(|| Kind::intern("augmented_assignment"));
static FOR_IN_CLAUSE: LazyLock<Kind> = LazyLock::new(|| Kind::intern("for_in_clause"));
static NAMED_EXPRESSION: LazyLock<Kind> = LazyLock::new(|| Kind::intern("named_expression"));
static AS_PATTERN_TARGET: LazyLock<Kind> = LazyLock::new(|| Kind::intern("as_pattern_target"));
static ASSIGNMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("assignment"));
static LAMBDA: LazyLock<Kind> = LazyLock::new(|| Kind::intern("lambda"));
static SUBSCRIPT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("subscript"));

// -- fields --
static ATTRIBUTE: LazyLock<Field> = LazyLock::new(|| Field::intern("attribute"));
static NAME_FIELD: LazyLock<Field> = LazyLock::new(|| Field::intern("name"));
static LEFT_FIELD: LazyLock<Field> = LazyLock::new(|| Field::intern("left"));
static RIGHT_FIELD: LazyLock<Field> = LazyLock::new(|| Field::intern("right"));
static OPERATOR_FIELD: LazyLock<Field> = LazyLock::new(|| Field::intern("operator"));
static OPERATORS_FIELD: LazyLock<Field> = LazyLock::new(|| Field::intern("operators"));
static KEY_FIELD: LazyLock<Field> = LazyLock::new(|| Field::intern("key"));
static CONDITION_FIELD: LazyLock<Field> = LazyLock::new(|| Field::intern("condition"));
static BODY_FIELD: LazyLock<Field> = LazyLock::new(|| Field::intern("body"));
static TYPE_FIELD: LazyLock<Field> = LazyLock::new(|| Field::intern("type"));
static VALUE_FIELD: LazyLock<Field> = LazyLock::new(|| Field::intern("value"));

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

    fn always_external(&self, _kind: Kind, field: Option<Field>, parent_kind: Kind) -> bool {
        field == Some(*ATTRIBUTE)
            || (parent_kind == *KEYWORD_ARGUMENT && field == Some(*NAME_FIELD))
    }

    fn collect_declared(&self, root: &NormNode, out: &mut Vec<Box<str>>) {
        fn push_field(node: &NormNode, field: &str, out: &mut Vec<Box<str>>) {
            if let Some(child) = child_field(node, field) {
                collect_pattern_idents(child, out);
            }
        }
        fn walk(node: &NormNode, out: &mut Vec<Box<str>>) {
            match node.kind {
                k if k == *GLOBAL_STATEMENT || k == *NONLOCAL_STATEMENT => return,
                k if k == *FUNCTION_DEFINITION => push_field(node, "name", out),
                k if k == *PARAMETERS || k == *LAMBDA_PARAMETERS => {
                    collect_param_idents(node, out);
                }
                k if k == *ASSIGNMENT
                    || k == *AUGMENTED_ASSIGNMENT
                    || k == *FOR_STATEMENT
                    || k == *FOR_IN_CLAUSE =>
                {
                    push_field(node, "left", out);
                }
                k if k == *NAMED_EXPRESSION => push_field(node, "name", out),
                k if k == *AS_PATTERN_TARGET => collect_pattern_idents(node, out),
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
        &["+", "*", "&", "|", "^", "==", "!=", "and", "or"]
    }

    fn binary_fields(&self, kind: Kind) -> Option<(Option<Field>, Option<Field>, Option<Field>)> {
        if kind == *BINARY_OPERATOR || kind == *BOOLEAN_OPERATOR {
            Some((Some(*LEFT_FIELD), Some(*OPERATOR_FIELD), Some(*RIGHT_FIELD)))
        } else if kind == *COMPARISON_OPERATOR {
            Some((None, Some(*OPERATORS_FIELD), None))
        } else {
            None
        }
    }

    fn sortable_pair_kind(&self, kind: Kind) -> Option<Field> {
        if kind == *DICTIONARY {
            Some(*KEY_FIELD)
        } else {
            None
        }
    }

    fn is_list_kind(&self, kind: Kind) -> bool {
        kind == *BLOCK
            || kind == *ARGUMENT_LIST
            || kind == *PARAMETERS
            || kind == *PATTERN_LIST
            || kind == *EXPRESSION_LIST
            || kind == *LIST
            || kind == *TUPLE
            || kind == *SET
            || kind == *DICTIONARY
            || kind == *REPEAT
    }

    fn is_dispatch_arm(&self, kind: Kind) -> bool {
        kind == *CASE_CLAUSE
    }

    fn is_statement_kind(&self, kind: Kind) -> bool {
        kind == *EXPRESSION_STATEMENT
            || kind == *IF_STATEMENT
            || kind == *WHILE_STATEMENT
            || kind == *FOR_STATEMENT
            || kind == *RETURN_STATEMENT
            || kind == *TRY_STATEMENT
            || kind == *WITH_STATEMENT
            || kind == *BREAK_STATEMENT
            || kind == *CONTINUE_STATEMENT
            || kind == *ASSERT_STATEMENT
            || kind == *RAISE_STATEMENT
            || kind == *PASS_STATEMENT
            || kind == *DELETE_STATEMENT
    }

    fn is_loop_core(&self, node: &NormNode) -> bool {
        node.kind == *WHILE_STATEMENT
            && node
                .children
                .iter()
                .any(|c| c.field == Some(*CONDITION_FIELD) && c.kind == *TRUE_KIND)
    }

    fn is_continue_stmt(&self, node: &NormNode) -> bool {
        node.kind == *CONTINUE_STATEMENT
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

    fn call_kind(&self) -> Kind {
        *CALL
    }

    fn inline_params(&self, root: &NormNode) -> Option<Vec<Box<str>>> {
        simple_params(root)
    }

    fn return_value<'a>(&self, stmt: &'a NormNode) -> Option<&'a NormNode> {
        if stmt.kind == *RETURN_STATEMENT && stmt.children.len() == 1 {
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
    match node.field {
        Some(f) if f == *TYPE_FIELD || f == *VALUE_FIELD => return,
        _ => {}
    }
    if let Some(Label::Raw(text)) = &node.label
        && node.kind == *IDENTIFIER
    {
        out.push(text.clone());
    }
    for child in &node.children {
        collect_param_idents(child, out);
    }
}

/// Lower `while cond` / `for x in xs` to the `while True` core (spec §5.2.2),
/// mirroring the exact shapes tree-sitter produces for the manual form.
fn lower(
    mut node: NormNode,
    label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
) -> NormNode {
    node.children = node
        .children
        .into_iter()
        .map(|c| lower(c, label_interner))
        .collect();
    if node.kind == *WHILE_STATEMENT {
        let is_true_cond = node
            .children
            .iter()
            .any(|c| c.field == Some(*CONDITION_FIELD) && c.kind == *TRUE_KIND);
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
            .find(|c| c.field == Some(*BODY_FIELD))
        {
            body.children.insert(0, break_unless(cond));
        }
        node.children.insert(0, true_node(span));
        node
    } else if node.kind == *FOR_STATEMENT {
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
        left.field = Some(*LEFT_FIELD);
        let next = call("__next", Some("right"), right, label_interner);
        let span_left = left.span;
        let assign = NormNode::new("assignment", None, span_left, vec![left, next]);
        let assign_stmt = NormNode::new("expression_statement", None, span_left, vec![assign]);
        body.children.insert(0, guard);
        body.children.insert(1, assign_stmt);
        let mut children = vec![true_node(span), body];
        children.append(&mut node.children);
        NormNode::with_kind(*WHILE_STATEMENT, node.field, span, children)
    } else {
        node
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

fn call(
    name: &str,
    field: Option<&str>,
    arg: NormNode,
    label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
) -> NormNode {
    synth_call("call", "argument_list", name, field, arg, label_interner)
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
        .position(|c| c.field == Some(*BODY_FIELD))
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
    inner.field = Some(*BODY_FIELD);
    let while_node =
        NormNode::with_kind(*WHILE_STATEMENT, None, span, vec![true_node(span), inner]);
    let wrapper = NormNode::new("block", Some("body"), span, vec![while_node]);
    root.children.insert(body_idx, wrapper);
    root
}

fn simple_params(root: &NormNode) -> Option<Vec<Box<str>>> {
    let params = child_field(root, "parameters")?;
    let mut out = Vec::new();
    for p in &params.children {
        match (&p.kind, &p.label) {
            (k, Some(Label::Raw(text))) if *k == *IDENTIFIER => out.push(text.clone()),
            _ => return None, // defaults / *args / typed params: bail
        }
    }
    Some(out)
}

fn is_self_call(node: &NormNode, name: &str) -> bool {
    node.kind == *CALL && child_field(node, "function").is_some_and(|f| is_raw_ident(f, name))
}

/// Nested callable kinds — a `return <self-call>` inside a nested `def`/`lambda`
/// is the INNER function's tail position, not the outer unit's. Descending into
/// it would emit `continue` outside any loop and reassign the outer params,
/// while the shared `count_self_calls` census still counts the nested call
/// (`replaced == total`) so the bad lowering would commit. Stopping descent
/// leaves the nested call uncounted → `replaced != total` → the unit safely
/// bails out of recursion lowering.
fn is_nested_callable(kind: Kind) -> bool {
    kind == *FUNCTION_DEFINITION || kind == *LAMBDA
}

fn rewrite_tail_sites(
    mut node: NormNode,
    name: &str,
    params: &[Box<str>],
    replaced: &mut u32,
) -> NormNode {
    if is_nested_callable(node.kind) {
        return node; // a self-call inside a nested closure is not a tail site
    }
    if node.kind == *BLOCK {
        let mut out = Vec::with_capacity(node.children.len());
        for child in node.children {
            // `return f(...)`
            let is_return_site = child.kind == *RETURN_STATEMENT
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
    if node.kind != *FOR_STATEMENT {
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
        .position(|c| c.field == Some(*RIGHT_FIELD))
        .unwrap();
    node.children[right_idx] = synth_ident(&collection, Some("right"), span);
    let body_idx = node
        .children
        .iter()
        .position(|c| c.field == Some(*BODY_FIELD))
        .unwrap();
    let body = node.children.remove(body_idx);
    let new_body = replace_subscripts(body, &ivar, &collection);
    node.children.insert(body_idx, new_body);
    node
}

/// Matches `range(len(X))` → Some(X).
fn range_len_collection(right: &NormNode) -> Option<Box<str>> {
    if right.kind != *CALL {
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
    if inner.kind != *CALL {
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
        Some(Label::Raw(t)) if coll.kind == *IDENTIFIER => Some(t.clone()),
        _ => None,
    }
}

fn is_target_subscript(node: &NormNode, ivar: &str, coll: &str) -> bool {
    node.kind == *SUBSCRIPT
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
        let field = node.field.map(|f| f.as_str());
        return synth_ident(ivar, field, node.span);
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
        .find(|c| c.field == Some(*BODY_FIELD))
    else {
        return root;
    };
    let n = body.children.len();
    if n < 2 {
        return root;
    }
    let ret_value: Option<NormNode> = {
        let last = &body.children[n - 1];
        if last.kind == *RETURN_STATEMENT && last.children.len() == 1 {
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
        if node.children[i].kind == *BREAK_STATEMENT {
            let span = node.children[i].span;
            node.children[i] = NormNode::new("return_statement", None, span, vec![value.clone()]);
            *replaced += 1;
        } else {
            replace_breaks(profile, &mut node.children[i], value, replaced, false);
        }
        i += 1;
    }
}
