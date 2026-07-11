//! Go language profile (spec §3, M3c). Node kinds and lowered shapes taken from
//! `examples/probe3.rs` output (DECISIONS.md D9). Go wraps block bodies in a
//! `statement_list`; `splice_kind` hoists those away so blocks hold statements
//! directly, matching the Rust/Python shape the passes assume.

use super::{
    LanguageProfile, child_field, count_self_calls, is_raw_ident, path_is_testy, raw_name,
    synth_call, synth_ident,
};
use crate::intern::{Field, Kind};
use crate::tree::{Bucket, Label, NormNode};
use std::path::Path;
use std::sync::LazyLock;

// ---------------------------------------------------------------------------
// Interned grammar-kind/field constants (interning-id-conversion WP, mechanism
// rule 2): every tree-sitter-go node-kind / field-name / operator-token literal
// this profile compares against gets ONE `LazyLock<Kind>`/`LazyLock<Field>`
// here, interned once ever, then every comparison site is a bare id equality.
// A handful of Go's native field names (`"left"`, `"right"`, `"body"`,
// `"name"`) happen to spell the same as the canonical IR-synthesized field
// vocabulary (`crate::ir::field::id`) — per that module's doc comment this is
// harmless textual collision (same shared `Field` table, `Field::intern` is
// idempotent), so those four use the pre-registered `id::*` consts directly
// instead of a redundant local static.
// ---------------------------------------------------------------------------

static IDENTIFIER: LazyLock<Kind> = LazyLock::new(|| Kind::intern("identifier"));
static FIELD_IDENTIFIER: LazyLock<Kind> = LazyLock::new(|| Kind::intern("field_identifier"));
static TYPE_IDENTIFIER: LazyLock<Kind> = LazyLock::new(|| Kind::intern("type_identifier"));
static PACKAGE_IDENTIFIER: LazyLock<Kind> = LazyLock::new(|| Kind::intern("package_identifier"));

static STATEMENT_LIST: LazyLock<Kind> = LazyLock::new(|| Kind::intern("statement_list"));
static BLOCK: LazyLock<Kind> = LazyLock::new(|| Kind::intern("block"));
static FOR_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("for_statement"));
static FOR_CLAUSE: LazyLock<Kind> = LazyLock::new(|| Kind::intern("for_clause"));
static RANGE_CLAUSE: LazyLock<Kind> = LazyLock::new(|| Kind::intern("range_clause"));
static CONTINUE_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("continue_statement"));
static BREAK_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("break_statement"));
static RETURN_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("return_statement"));
static EXPRESSION_LIST: LazyLock<Kind> = LazyLock::new(|| Kind::intern("expression_list"));
static SHORT_VAR_DECLARATION: LazyLock<Kind> =
    LazyLock::new(|| Kind::intern("short_var_declaration"));
static VAR_DECLARATION: LazyLock<Kind> = LazyLock::new(|| Kind::intern("var_declaration"));
static CONST_DECLARATION: LazyLock<Kind> = LazyLock::new(|| Kind::intern("const_declaration"));
static ASSIGNMENT_STATEMENT: LazyLock<Kind> =
    LazyLock::new(|| Kind::intern("assignment_statement"));
static INC_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("inc_statement"));
static DEC_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("dec_statement"));
static IF_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("if_statement"));
static GO_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("go_statement"));
static DEFER_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("defer_statement"));
static LABELED_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("labeled_statement"));
static EXPRESSION_SWITCH_STATEMENT: LazyLock<Kind> =
    LazyLock::new(|| Kind::intern("expression_switch_statement"));
static TYPE_SWITCH_STATEMENT: LazyLock<Kind> =
    LazyLock::new(|| Kind::intern("type_switch_statement"));
static SEND_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("send_statement"));
static EXPRESSION_STATEMENT: LazyLock<Kind> =
    LazyLock::new(|| Kind::intern("expression_statement"));
static BINARY_EXPRESSION: LazyLock<Kind> = LazyLock::new(|| Kind::intern("binary_expression"));
static LT_OP: LazyLock<Kind> = LazyLock::new(|| Kind::intern("<"));
static CALL_EXPRESSION: LazyLock<Kind> = LazyLock::new(|| Kind::intern("call_expression"));
static INDEX_EXPRESSION: LazyLock<Kind> = LazyLock::new(|| Kind::intern("index_expression"));
static PARAMETER_DECLARATION: LazyLock<Kind> =
    LazyLock::new(|| Kind::intern("parameter_declaration"));
static FUNCTION_DECLARATION: LazyLock<Kind> =
    LazyLock::new(|| Kind::intern("function_declaration"));
static VAR_SPEC: LazyLock<Kind> = LazyLock::new(|| Kind::intern("var_spec"));
static CONST_SPEC: LazyLock<Kind> = LazyLock::new(|| Kind::intern("const_spec"));
static LITERAL_VALUE: LazyLock<Kind> = LazyLock::new(|| Kind::intern("literal_value"));
static ARGUMENT_LIST: LazyLock<Kind> = LazyLock::new(|| Kind::intern("argument_list"));
static PARAMETER_LIST: LazyLock<Kind> = LazyLock::new(|| Kind::intern("parameter_list"));
static REPEAT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("REPEAT"));

static OPERATOR: LazyLock<Field> = LazyLock::new(|| Field::intern("operator"));
static KEY: LazyLock<Field> = LazyLock::new(|| Field::intern("key"));

pub struct GoProfile;

impl LanguageProfile for GoProfile {
    fn is_function_like(&self, kind: &str) -> bool {
        matches!(kind, "function_declaration" | "method_declaration")
    }

    fn is_comment(&self, kind: &str) -> bool {
        kind == "comment"
    }

    fn strip_kind(&self, _kind: &str) -> bool {
        false
    }

    fn splice_kind(&self, kind: Kind) -> bool {
        kind == *STATEMENT_LIST
    }

    fn is_identifier(&self, kind: &str) -> bool {
        matches!(
            kind,
            "identifier" | "field_identifier" | "type_identifier" | "package_identifier"
        )
    }

    fn literal_bucket(&self, kind: &str) -> Option<Bucket> {
        match kind {
            "int_literal" => Some(Bucket::Int),
            "float_literal" | "imaginary_literal" => Some(Bucket::Float),
            "interpreted_string_literal" | "raw_string_literal" => Some(Bucket::Str),
            "rune_literal" => Some(Bucket::Char),
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
            "binary_expression" | "unary_expression" | "assignment_statement"
        )
    }

    fn always_external(&self, kind: Kind, _field: Option<Field>, _parent_kind: Kind) -> bool {
        kind == *FIELD_IDENTIFIER || kind == *TYPE_IDENTIFIER || kind == *PACKAGE_IDENTIFIER
    }

    fn collect_declared(&self, root: &NormNode, out: &mut Vec<Box<str>>) {
        fn push_idents(node: &NormNode, out: &mut Vec<Box<str>>) {
            if let Some(Label::Raw(text)) = &node.label
                && node.kind == *IDENTIFIER
            {
                out.push(text.clone());
            }
            for c in &node.children {
                push_idents(c, out);
            }
        }
        fn walk(node: &NormNode, out: &mut Vec<Box<str>>) {
            if node.kind == *FUNCTION_DECLARATION {
                if let Some(name) = child_field(node, "name")
                    && let Some(Label::Raw(text)) = &name.label
                {
                    out.push(text.clone());
                }
            } else if node.kind == *PARAMETER_DECLARATION {
                // `name` is a REPEATED field (`func f(a, b int)` groups both
                // under one node) — collect every name, not `child_field`'s
                // first-only (undercount → grouped names bound as external).
                for name in name_fields(node) {
                    push_idents(name, out);
                }
            } else if node.kind == *SHORT_VAR_DECLARATION || node.kind == *RANGE_CLAUSE {
                if let Some(left) = child_field(node, "left") {
                    push_idents(left, out);
                }
            } else if node.kind == *VAR_SPEC || node.kind == *CONST_SPEC {
                // `name` is a REPEATED field here too (`var a, b int`).
                for name in name_fields(node) {
                    push_idents(name, out);
                }
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
        &["+", "*", "&", "|", "^", "&&", "||", "==", "!="]
    }

    fn binary_fields(&self, kind: Kind) -> Option<(Option<Field>, Option<Field>, Option<Field>)> {
        if kind == *BINARY_EXPRESSION {
            Some((
                Some(crate::ir::field::id::LEFT),
                Some(*OPERATOR),
                Some(crate::ir::field::id::RIGHT),
            ))
        } else {
            None
        }
    }

    fn sortable_pair_kind(&self, kind: Kind) -> Option<Field> {
        if kind == *LITERAL_VALUE {
            Some(*KEY)
        } else {
            None
        }
    }

    fn is_list_kind(&self, kind: Kind) -> bool {
        kind == *BLOCK
            || kind == *ARGUMENT_LIST
            || kind == *PARAMETER_LIST
            || kind == *EXPRESSION_LIST
            || kind == *LITERAL_VALUE
            || kind == *REPEAT
    }

    fn is_statement_kind(&self, kind: Kind) -> bool {
        kind == *EXPRESSION_STATEMENT
            || kind == *SHORT_VAR_DECLARATION
            || kind == *VAR_DECLARATION
            || kind == *CONST_DECLARATION
            || kind == *ASSIGNMENT_STATEMENT
            || kind == *INC_STATEMENT
            || kind == *DEC_STATEMENT
            || kind == *IF_STATEMENT
            || kind == *FOR_STATEMENT
            || kind == *RETURN_STATEMENT
            || kind == *BREAK_STATEMENT
            || kind == *CONTINUE_STATEMENT
            || kind == *GO_STATEMENT
            || kind == *DEFER_STATEMENT
            || kind == *LABELED_STATEMENT
            || kind == *EXPRESSION_SWITCH_STATEMENT
            || kind == *TYPE_SWITCH_STATEMENT
            || kind == *SEND_STATEMENT
    }

    fn is_loop_core(&self, node: &NormNode) -> bool {
        node.kind == *FOR_STATEMENT
            && node.children.len() == 1
            && node.children[0].field == Some(crate::ir::field::id::BODY)
    }

    fn is_continue_stmt(&self, node: &NormNode) -> bool {
        node.kind == *CONTINUE_STATEMENT
    }

    fn unit_is_test(&self, _node: tree_sitter::Node, _src: &str, _name: &str, path: &Path) -> bool {
        // Go tests are exactly the funcs in `_test.go` files (the `TestXxx`
        // convention is only meaningful there); a `TestXxx` in a normal file is
        // an ordinary function.
        let fname = path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or_default();
        fname.ends_with("_test.go") || path_is_testy(path)
    }

    // ---- Phase 3: inliner hooks (spec §5.4) ----

    fn call_kind(&self) -> Kind {
        *CALL_EXPRESSION
    }

    fn inline_params(&self, root: &NormNode) -> Option<Vec<Box<str>>> {
        simple_params(root)
    }

    fn return_value<'a>(&self, stmt: &'a NormNode) -> Option<&'a NormNode> {
        if stmt.kind == *RETURN_STATEMENT
            && stmt.children.len() == 1
            && stmt.children[0].kind == *EXPRESSION_LIST
            && stmt.children[0].children.len() == 1
        {
            return Some(&stmt.children[0].children[0]);
        }
        None
    }

    fn make_return(&self, mut value: NormNode) -> NormNode {
        let span = value.span;
        value.field = None;
        let list = NormNode::new("expression_list", None, span, vec![value]);
        NormNode::new("return_statement", None, span, vec![list])
    }

    fn make_expr_stmt(&self, mut expr: NormNode) -> NormNode {
        let span = expr.span;
        expr.field = None;
        NormNode::new("expression_statement", None, span, vec![expr])
    }

    fn make_expr_block(&self, _span: (u32, u32), _children: Vec<NormNode>) -> Option<NormNode> {
        None // Go has no expression block (DECISIONS.md D17).
    }
}

fn eq_token(span: (u32, u32)) -> NormNode {
    NormNode::new("=", Some("operator"), span, Vec::new())
}

/// `if !(cond) { break }` in the manual shape (probe `cores`): Go unary uses
/// the `operand` field and a `block` consequence.
fn break_unless(mut cond: NormNode) -> NormNode {
    let span = cond.span;
    cond.field = Some("operand".into());
    let bang = NormNode::token("!", span);
    let negated = NormNode::new(
        "unary_expression",
        Some("condition"),
        span,
        vec![bang, cond],
    );
    let brk = NormNode::new("break_statement", None, span, Vec::new());
    let consequence = NormNode::new("block", Some("consequence"), span, vec![brk]);
    NormNode::new("if_statement", None, span, vec![negated, consequence])
}

fn call(
    name: &str,
    arg: NormNode,
    label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
) -> NormNode {
    synth_call(
        "call_expression",
        "argument_list",
        name,
        None,
        arg,
        label_interner,
    )
}

/// Lower the Go `for` forms to the infinite-`for` core (spec §5.2.2). Range and
/// condition-only forms transform in place; the three-clause form is hoisted at
/// the block level (init + core), which a single-node rewrite cannot express.
fn lower(
    mut node: NormNode,
    label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
) -> NormNode {
    node.children = node
        .children
        .into_iter()
        .map(|c| lower(c, label_interner))
        .collect();
    if node.kind == *BLOCK {
        node.children = expand_for_clauses(node.children);
    }
    if node.kind != *FOR_STATEMENT {
        return node;
    }
    let clause_kind = node
        .children
        .iter()
        .find(|c| c.field != Some(crate::ir::field::id::BODY))
        .map(|c| c.kind);
    if clause_kind == Some(*FOR_CLAUSE) {
        node // hoisted at block level
    } else if clause_kind == Some(*RANGE_CLAUSE) {
        lower_range(node, label_interner)
    } else if clause_kind.is_some() {
        lower_while_form(node)
    } else {
        node // infinite: already the core
    }
}

/// `for cond { body }` → `for { if !(cond) { break }; body }`.
fn lower_while_form(mut node: NormNode) -> NormNode {
    let span = node.span;
    let field = node.field.map(|f| f.as_str());
    let Some(mut body) = node.take_field("body") else {
        return node;
    };
    // The remaining child is the bare condition expression.
    let Some(cond_idx) = node.children.iter().position(|c| c.field.is_none()) else {
        return node;
    };
    let cond = node.children.remove(cond_idx);
    body.children.insert(0, break_unless(cond));
    body.field = Some("body".into());
    NormNode::new("for_statement", field, span, vec![body])
}

/// `for left := range xs { body }` → `for { if !__has_next(xs) break; left = __next(xs); body }`.
fn lower_range(
    mut node: NormNode,
    label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
) -> NormNode {
    let span = node.span;
    let field = node.field.map(|f| f.as_str());
    let Some(mut body) = node.take_field("body") else {
        return node;
    };
    let Some(clause_idx) = node.children.iter().position(|c| c.kind == *RANGE_CLAUSE) else {
        return node;
    };
    let mut clause = node.children.remove(clause_idx);
    let (Some(mut left), Some(mut right)) = (clause.take_field("left"), clause.take_field("right"))
    else {
        return node;
    };
    right.field = None;
    let guard = break_unless(call("__has_next", right.clone(), label_interner));
    left.field = Some("left".into());
    let next = call("__next", right, label_interner);
    let rhs = NormNode::new("expression_list", Some("right"), span, vec![next]);
    // `:=` (short_var_declaration), not `=`, so `collect_declared` binds the
    // range variables as locals (the range var is genuinely declared here).
    let bind = NormNode::new("short_var_declaration", None, span, vec![left, rhs]);
    body.children.insert(0, guard);
    body.children.insert(1, bind);
    body.field = Some("body".into());
    NormNode::new("for_statement", field, span, vec![body])
}

/// Replace each three-clause `for` (a `for_statement` carrying a `for_clause`)
/// with `[initializer, for { break-guard; body; update }]`.
fn expand_for_clauses(children: Vec<NormNode>) -> Vec<NormNode> {
    let mut out = Vec::with_capacity(children.len());
    for child in children {
        let is_three_clause =
            child.kind == *FOR_STATEMENT && child.children.iter().any(|c| c.kind == *FOR_CLAUSE);
        if is_three_clause {
            out.extend(lower_three_clause(child));
        } else {
            out.push(child);
        }
    }
    out
}

fn lower_three_clause(mut node: NormNode) -> Vec<NormNode> {
    let span = node.span;
    let Some(mut body) = node.take_field("body") else {
        return vec![node];
    };
    let Some(clause_idx) = node.children.iter().position(|c| c.kind == *FOR_CLAUSE) else {
        return vec![node];
    };
    let mut clause = node.children.remove(clause_idx);
    let init = clause.take_field("initializer");
    let cond = clause.take_field("condition");
    let update = clause.take_field("update");
    if let Some(mut update) = update {
        update.field = None;
        body.children.push(update); // inc_statement is itself a statement
    }
    if let Some(cond) = cond {
        body.children.insert(0, break_unless(cond));
    }
    body.field = Some("body".into());
    let loop_node = NormNode::new("for_statement", None, span, vec![body]);
    match init {
        Some(mut init) => {
            init.field = None;
            vec![init, loop_node]
        }
        None => vec![loop_node],
    }
}

// ---- iteration-protocol rewrite (spec §5.2.2): the canonical Go index loop ----
// `for i := 0; i < len(xs); i++ { …xs[i]… }` → `for _, i := range xs { …i… }`.

fn rewrite_iteration(mut node: NormNode) -> NormNode {
    node.children = node.children.into_iter().map(rewrite_iteration).collect();
    if node.kind != *FOR_STATEMENT {
        return node;
    }
    let Some((ivar, coll)) = index_for_match(&node) else {
        return node;
    };
    let Some(mut body) = node.take_field("body") else {
        return node;
    };
    let span = node.span;
    body = replace_index(body, &ivar, &coll);
    body.field = Some("body".into());
    let left = NormNode::new(
        "expression_list",
        Some("left"),
        span,
        vec![synth_ident("_", None, span), synth_ident(&ivar, None, span)],
    );
    let right = synth_ident(&coll, Some("right"), span);
    let range = NormNode::new("range_clause", None, span, vec![left, right]);
    NormNode::new(
        "for_statement",
        node.field.map(|f| f.as_str()),
        span,
        vec![range, body],
    )
}

fn index_for_match(node: &NormNode) -> Option<(Box<str>, Box<str>)> {
    let clause = node.children.iter().find(|c| c.kind == *FOR_CLAUSE)?;
    // init: i := 0  (short_var_declaration, left=[i], right=[0])
    let init = child_field(clause, "initializer")?;
    if init.kind != *SHORT_VAR_DECLARATION {
        return None;
    }
    let left = child_field(init, "left")?;
    if left.children.len() != 1 {
        return None;
    }
    let ivar = match &left.children[0].label {
        Some(Label::Raw(t)) if left.children[0].kind == *IDENTIFIER => t.clone(),
        _ => return None,
    };
    let right = child_field(init, "right")?;
    if right.children.len() != 1
        || !matches!(&right.children[0].label, Some(Label::RawLit(t)) if t.as_ref() == "0")
    {
        return None;
    }
    // cond: i < len(coll)
    let cond = child_field(clause, "condition")?;
    if cond.kind != *BINARY_EXPRESSION || cond.children.len() != 3 {
        return None;
    }
    if !is_raw_ident(&cond.children[0], &ivar) || cond.children[1].kind != *LT_OP {
        return None;
    }
    let lencall = &cond.children[2];
    if lencall.kind != *CALL_EXPRESSION
        || !child_field(lencall, "function").is_some_and(|f| is_raw_ident(f, "len"))
    {
        return None;
    }
    let args = child_field(lencall, "arguments")?;
    if args.children.len() != 1 {
        return None;
    }
    let coll = match &args.children[0].label {
        Some(Label::Raw(t)) if args.children[0].kind == *IDENTIFIER => t.clone(),
        _ => return None,
    };
    // update: i++
    let update = child_field(clause, "update")?;
    if update.kind != *INC_STATEMENT || !is_raw_ident(&update.children[0], &ivar) {
        return None;
    }
    let body = child_field(node, "body")?;
    if !index_uses_only(body, &ivar, &coll) {
        return None;
    }
    Some((ivar, coll))
}

fn is_target_index(node: &NormNode, ivar: &str, coll: &str) -> bool {
    node.kind == *INDEX_EXPRESSION
        && child_field(node, "operand").is_some_and(|v| is_raw_ident(v, coll))
        && child_field(node, "index").is_some_and(|s| is_raw_ident(s, ivar))
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
        let field = node.field.map(|f| f.as_str());
        return synth_ident(ivar, field, node.span);
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
        .position(|c| c.field == Some(crate::ir::field::id::BODY))
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
    let loop_node = NormNode::new("for_statement", None, span, vec![inner]);
    let wrapper = NormNode::new("block", Some("body"), span, vec![loop_node]);
    root.children.insert(body_idx, wrapper);
    root
}

fn simple_params(root: &NormNode) -> Option<Vec<Box<str>>> {
    let params = child_field(root, "parameters")?;
    let mut out = Vec::new();
    for p in &params.children {
        if p.kind != *PARAMETER_DECLARATION {
            return None; // variadic / unnamed: bail
        }
        // `name` is a REPEATED field: `func f(a, b int)` groups both names under
        // one `parameter_declaration` (matching the IR frontend, frontend/go.rs).
        // Taking only the first (`child_field`) undercounts the params, so a
        // grouped-param self-call `f(x, y)` fails arity vs the 1 counted param
        // and mislowers the recursion.
        let before = out.len();
        for name in name_fields(p) {
            match &name.label {
                Some(Label::Raw(text)) if name.kind == *IDENTIFIER => out.push(text.clone()),
                _ => return None,
            }
        }
        if out.len() == before {
            return None; // unnamed parameter (no `name` field): bail
        }
    }
    Some(out)
}

/// Every child of `node` in the repeated `name` field. tree-sitter-go groups
/// `func f(a, b int)` / `var a, b int` / `const a, b = …` names under one node
/// via a REPEATED `name` field, so `child_field` (first-only) undercounts them.
fn name_fields(node: &NormNode) -> impl Iterator<Item = &NormNode> {
    node.children
        .iter()
        .filter(|c| c.field == Some(crate::ir::field::id::NAME))
}

fn is_self_call(node: &NormNode, name: &str) -> bool {
    node.kind == *CALL_EXPRESSION
        && child_field(node, "function").is_some_and(|f| is_raw_ident(f, name))
}

fn rewrite_tail_sites(
    mut node: NormNode,
    name: &str,
    params: &[Box<str>],
    replaced: &mut u32,
) -> NormNode {
    if node.kind == *BLOCK {
        let mut out = Vec::with_capacity(node.children.len());
        for child in node.children {
            // `return f(...)`: return_statement { expression_list { call } }
            let is_return_site = child.kind == *RETURN_STATEMENT
                && child.children.len() == 1
                && child.children[0].kind == *EXPRESSION_LIST
                && child.children[0].children.len() == 1
                && is_self_call(&child.children[0].children[0], name);
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

/// `p1, p2 = a1, a2; continue` — identity pairs dropped.
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
    if !pairs.is_empty() {
        let mut lefts = Vec::new();
        let mut rights = Vec::new();
        for (i, mut arg) in pairs {
            arg.field = None;
            lefts.push(synth_ident(&params[i], None, span));
            rights.push(arg);
        }
        let left = NormNode::new("expression_list", Some("left"), span, lefts);
        let right = NormNode::new("expression_list", Some("right"), span, rights);
        let assign = NormNode::new(
            "assignment_statement",
            None,
            span,
            vec![left, eq_token(span), right],
        );
        out.push(assign);
    }
    out.push(continue_stmt(span));
    out
}

fn continue_stmt(span: (u32, u32)) -> NormNode {
    NormNode::new("continue_statement", None, span, Vec::new())
}

// ---- loop-exit normalization ----

fn normalize_loop_exit(profile: &GoProfile, mut root: NormNode) -> NormNode {
    let Some(body) = root
        .children
        .iter_mut()
        .find(|c| c.field == Some(crate::ir::field::id::BODY))
    else {
        return root;
    };
    let n = body.children.len();
    if n < 2 {
        return root;
    }
    let ret_value: Option<NormNode> = {
        let last = &body.children[n - 1];
        if last.kind == *RETURN_STATEMENT
            && last.children.len() == 1
            && last.children[0].kind == *EXPRESSION_LIST
            && last.children[0].children.len() == 1
        {
            Some(last.children[0].children[0].clone())
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
    profile: &GoProfile,
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
        if node.children[i].kind == *BREAK_STATEMENT && node.children[i].children.is_empty() {
            let span = node.children[i].span;
            let mut v = value.clone();
            v.field = None;
            let list = NormNode::new("expression_list", None, span, vec![v]);
            node.children[i] = NormNode::new("return_statement", None, span, vec![list]);
            *replaced += 1;
        } else {
            replace_breaks(profile, &mut node.children[i], value, replaced, false);
        }
        i += 1;
    }
}
