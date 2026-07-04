//! Language frontends: tree-sitter CST → canonical IR ([`crate::ir`]).
//!
//! **Distinct from the historical `src/lang/`.** Those profiles lower to *per-grammar*
//! node kinds and each re-implements the full normalization (loop lowering, recursion,
//! identifier abstraction, …) — the parallel-implementation duplication the IR exists to
//! kill. A frontend here lowers *directly to the shared canonical IR*, so the
//! normalization **algorithms live once** ([`crate::ir::pass`]) and each language is a
//! thin CST-kind → canonical-kind *mapping* (see [`rust`], [`python`]). Only the mapping
//! and the parameter shape are per-language; recursion, node synthesis (break-guard,
//! arms), literal bucketing, and every pass are shared below.

pub mod python;
pub mod rust;

pub use python::{lower_python, lower_python_source};
pub use rust::{lower_rust, lower_rust_source};

use crate::ir::kind;
use crate::ir::transform::{TransformKind, TransformLog, Witness};
use crate::lang::Lang;
use crate::tree::{Bucket, Label, NormNode};
use tree_sitter::Node;

/// A per-language CST → canonical-IR mapping — the only per-language surface.
pub(crate) trait Frontend {
    /// Map one CST node to a canonical IR node (or `None` if dropped).
    fn lower_node(
        &self,
        node: Node,
        field: Option<&str>,
        src: &str,
        log: &mut TransformLog,
    ) -> Option<NormNode>;

    /// Push the unit's parameters as `Var@param` (binding shape differs per grammar).
    fn lower_params(
        &self,
        params: Node,
        src: &str,
        log: &mut TransformLog,
        out: &mut Vec<NormNode>,
    );

    /// A wrapper kind whose children are spliced directly into the parent statement
    /// list (Go's `statement_list` inside `block`), so blocks hold statements directly
    /// — the canonical shape every shared helper assumes. Mirrors the historical
    /// `LanguageProfile::splice_kind` (D23). Default: never splice.
    fn splice_kind(&self, _kind: &str) -> bool {
        false
    }
}

pub(crate) fn lower_unit(fe: &dyn Frontend, node: Node, src: &str) -> (NormNode, TransformLog) {
    let mut log = TransformLog::new();
    let root = fe
        .lower_node(node, None, src, &mut log)
        .unwrap_or_else(|| NormNode::new(kind::UNIT, None, span_of(node), Vec::new()));
    (root, log)
}

pub(crate) fn lower_source(
    fe: &dyn Frontend,
    lang: Lang,
    root_kind: &str,
    src: &str,
) -> Option<(NormNode, TransformLog)> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang.ts_language()).ok()?;
    let tree = parser.parse(src, None)?;
    let func = find_kind(tree.root_node(), root_kind)?;
    Some(lower_unit(fe, func, src))
}

fn find_kind<'a>(node: Node<'a>, k: &str) -> Option<Node<'a>> {
    if node.kind() == k {
        return Some(node);
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find_map(|c| find_kind(c, k))
}

// ---- shared lowering helpers (language-agnostic; the algorithm, written once) ----

fn span_of(node: Node) -> (u32, u32) {
    (node.start_byte() as u32, node.end_byte() as u32)
}

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    node.utf8_text(src.as_bytes()).unwrap_or_default()
}

pub(crate) fn leaf(k: &str, field: Option<&str>, span: (u32, u32)) -> NormNode {
    NormNode::new(k, field, span, Vec::new())
}

pub(crate) fn var(node: Node, field: Option<&str>, span: (u32, u32), src: &str) -> NormNode {
    NormNode::new(kind::VAR, field, span, Vec::new()).with_label(Label::Raw(text(node, src).into()))
}

/// Literal → typed bucket, recording the value as a witness (§15): two clones that
/// differ only in a constant converge, yet the divergence stays recoverable.
pub(crate) fn lit(
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    bucket: Bucket,
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    log.record(
        TransformKind::LitBucket,
        span,
        Witness::Literal(text(node, src).into()),
    );
    NormNode::new(kind::LIT, field, span, Vec::new()).with_label(Label::LitBucket(bucket))
}

/// Unmodeled construct → opaque, resolution-free escape hatch (§12.1); the tree-sitter
/// kind is the discriminating tag, children still lowered.
pub(crate) fn native(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    NormNode::new(
        kind::NATIVE_STMT,
        field,
        span,
        lower_stmts(fe, node, src, log),
    )
    .with_label(Label::External(node.kind().into()))
}

pub(crate) fn block(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    NormNode::new(kind::BLOCK, field, span, lower_stmts(fe, node, src, log))
}

fn lower_stmts(fe: &dyn Frontend, node: Node, src: &str, log: &mut TransformLog) -> Vec<NormNode> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for c in node.named_children(&mut cursor) {
        if fe.splice_kind(c.kind()) {
            // Hoist the wrapper's children into this list (Go `statement_list`).
            out.extend(lower_stmts(fe, c, src, log));
        } else if let Some(n) = fe.lower_node(c, None, src, log) {
            out.push(n);
        }
    }
    out
}

fn lower_body(
    fe: &dyn Frontend,
    node: Node,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    node.child_by_field_name("body")
        .and_then(|b| fe.lower_node(b, Some("body"), src, log))
        .unwrap_or_else(|| NormNode::new(kind::BLOCK, Some("body"), span, Vec::new()))
}

pub(crate) fn lower_function(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let mut children = Vec::new();
    if let Some(params) = node.child_by_field_name("parameters") {
        fe.lower_params(params, src, log, &mut children);
    }
    children.push(lower_body(fe, node, span, src, log));
    NormNode::new(kind::UNIT, field, span, children)
}

pub(crate) fn lower_call(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    fields: (&str, &str),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let (fn_f, args_f) = fields;
    let mut children = Vec::new();
    if let Some(c) = node
        .child_by_field_name(fn_f)
        .and_then(|n| fe.lower_node(n, Some("callee"), src, log))
    {
        children.push(c);
    }
    if let Some(args) = node.child_by_field_name(args_f) {
        let mut cursor = args.walk();
        for a in args.named_children(&mut cursor) {
            if let Some(c) = fe.lower_node(a, Some("arg"), src, log) {
                children.push(c);
            }
        }
    }
    NormNode::new(kind::CALL, field, span, children)
}

pub(crate) fn lower_return(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let mut cursor = node.walk();
    let value = node
        .named_children(&mut cursor)
        .find_map(|c| fe.lower_node(c, Some("value"), src, log));
    NormNode::new(kind::RETURN, field, span, value.into_iter().collect())
}

pub(crate) fn drop_parens(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> Option<NormNode> {
    log.record(TransformKind::ParenDrop, span, Witness::None);
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find_map(|c| fe.lower_node(c, field, src, log))
}

pub(crate) fn lower_binary(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    fields: (&str, &str, &str),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let (l_f, op_f, r_f) = fields;
    let mut children = Vec::new();
    if let Some(l) = node
        .child_by_field_name(l_f)
        .and_then(|n| fe.lower_node(n, Some("left"), src, log))
    {
        children.push(l);
    }
    if let Some(op) = node.child_by_field_name(op_f) {
        children.push(NormNode::new(
            text(op, src),
            Some("op"),
            span_of(op),
            Vec::new(),
        ));
    }
    if let Some(r) = node
        .child_by_field_name(r_f)
        .and_then(|n| fe.lower_node(n, Some("right"), src, log))
    {
        children.push(r);
    }
    NormNode::new(kind::BINOP, field, span, children)
}

pub(crate) fn lower_assign(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    fields: (&str, &str),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let (target_f, value_f) = fields;
    let mut children = Vec::new();
    if let Some(t) = node
        .child_by_field_name(target_f)
        .and_then(|n| fe.lower_node(n, Some("target"), src, log))
    {
        children.push(t);
    }
    if let Some(v) = node
        .child_by_field_name(value_f)
        .and_then(|n| fe.lower_node(n, Some("value"), src, log))
    {
        children.push(v);
    }
    NormNode::new(kind::ASSIGN, field, span, children)
}

pub(crate) fn unwrap_stmt(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    src: &str,
    log: &mut TransformLog,
) -> Option<NormNode> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find_map(|c| fe.lower_node(c, field, src, log))
}

pub(crate) fn lower_loop(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    // Already the canonical core — no transform recorded.
    NormNode::new(
        kind::LOOP,
        field,
        span,
        vec![lower_body(fe, node, span, src, log)],
    )
}

pub(crate) fn lower_while(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    // `while cond { body }` → `Loop { break-guard; body }`. Lossy: record LoopLower with
    // the source form so it reverses for display and the log diff explains the match.
    let cond = node
        .child_by_field_name("condition")
        .and_then(|c| fe.lower_node(c, None, src, log));
    let mut body = lower_body(fe, node, span, src, log);
    if let Some(cond) = cond {
        body.children.insert(0, break_guard(cond, span));
    }
    log.record(
        TransformKind::LoopLower,
        span,
        Witness::LoopForm("while".into()),
    );
    NormNode::new(kind::LOOP, field, span, vec![body])
}

pub(crate) fn lower_for(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    fields: (&str, &str),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    // `for pat in iter { body }` → the canonical iterated loop (§5.2.2):
    //   Loop { if !__has_next(iter) { break }; pat = __next(iter); body }
    // The synthesized has_next/next protocol is identical across languages, so a Rust
    // `for` and a Python `for` converge to the same canonical form.
    let (pat_f, iter_f) = fields;
    let mut body = lower_body(fe, node, span, src, log);
    let iter = node
        .child_by_field_name(iter_f)
        .and_then(|n| fe.lower_node(n, None, src, log));
    let pat = node
        .child_by_field_name(pat_f)
        .and_then(|n| fe.lower_node(n, Some("target"), src, log));
    if let (Some(iter), Some(pat)) = (iter, pat) {
        let bind = make_assign(pat, call_ext("__next", iter.clone(), span), span);
        let guard = break_guard(call_ext("__has_next", iter, span), span);
        body.children.insert(0, bind);
        body.children.insert(0, guard);
    }
    log.record(
        TransformKind::LoopLower,
        span,
        Witness::LoopForm("for".into()),
    );
    NormNode::new(kind::LOOP, field, span, vec![body])
}

/// Synthesize `name(arg)` with an `External` callee (the `__has_next`/`__next` protocol).
fn call_ext(name: &str, arg: NormNode, span: (u32, u32)) -> NormNode {
    let callee = NormNode::new(kind::VAR, Some("callee"), span, Vec::new())
        .with_label(Label::External(name.into()));
    let mut a = arg;
    a.field = Some("arg".into());
    NormNode::new(kind::CALL, None, span, vec![callee, a])
}

/// `target = value` where `target` already carries its `@target` field.
fn make_assign(target: NormNode, value: NormNode, span: (u32, u32)) -> NormNode {
    let mut v = value;
    v.field = Some("value".into());
    NormNode::new(kind::ASSIGN, None, span, vec![target, v])
}

/// Field access `base.name` → `Field{ base, name }`. The field name is **always
/// External** by structure (it is never a local), so the historical `always_external`
/// hook dissolves: abstraction leaves it alone.
pub(crate) fn lower_field(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    fields: (&str, &str),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let (base_f, name_f) = fields;
    let mut children = Vec::new();
    if let Some(b) = node
        .child_by_field_name(base_f)
        .and_then(|n| fe.lower_node(n, Some("base"), src, log))
    {
        children.push(b);
    }
    if let Some(name) = node.child_by_field_name(name_f) {
        children.push(
            NormNode::new(kind::VAR, Some("name"), span_of(name), Vec::new())
                .with_label(Label::External(text(name, src).into())),
        );
    }
    NormNode::new(kind::FIELD, field, span, children)
}

pub(crate) fn lower_if(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let guard = node
        .child_by_field_name("condition")
        .and_then(|c| fe.lower_node(c, Some("guard"), src, log));
    let body = node
        .child_by_field_name("consequence")
        .and_then(|c| fe.lower_node(c, Some("body"), src, log));
    let mut arms = vec![make_arm(guard, body, span)];
    if let Some(alt) = node.child_by_field_name("alternative") {
        let mut cursor = alt.walk();
        let inner = alt
            .named_children(&mut cursor)
            .find_map(|c| fe.lower_node(c, Some("body"), src, log));
        if inner.is_some() {
            arms.push(make_arm(None, inner, span));
        }
    }
    NormNode::new(kind::BRANCH, field, span, arms)
}

/// Rust-style unary: `<op-token> <operand>` (positional). Operator text kept as written.
pub(crate) fn lower_unary_positional(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let mut op = None;
    let mut operand = None;
    let mut cursor = node.walk();
    for c in node.children(&mut cursor) {
        if c.is_named() {
            operand = fe.lower_node(c, Some("operand"), src, log);
        } else if op.is_none() {
            op = Some(NormNode::new(
                text(c, src),
                Some("op"),
                span_of(c),
                Vec::new(),
            ));
        }
    }
    NormNode::new(
        kind::UNOP,
        field,
        span,
        op.into_iter().chain(operand).collect(),
    )
}

/// Python-style `not x`: canonicalize the negation operator to `!` so it converges with
/// the break-guard synthesis and with Rust's `!x`.
pub(crate) fn lower_unary_field(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    arg_f: &str,
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let operand = node
        .child_by_field_name(arg_f)
        .and_then(|n| fe.lower_node(n, Some("operand"), src, log));
    let op = NormNode::new("!", Some("op"), span, Vec::new());
    NormNode::new(
        kind::UNOP,
        field,
        span,
        std::iter::once(op).chain(operand).collect(),
    )
}

// ---- pure canonical-node synthesis (no frontend) ----

/// `Arm[ guard?, body? ]` — a member of a canonical `Branch` (§14).
fn make_arm(guard: Option<NormNode>, body: Option<NormNode>, span: (u32, u32)) -> NormNode {
    NormNode::new(
        kind::ARM,
        Some("arm"),
        span,
        guard.into_iter().chain(body).collect(),
    )
}

/// The canonical break-guard `Branch[ Arm[ !cond → { Break } ] ]` — built to be
/// byte-identical to a hand-written `if !cond { break }`, which is what makes `while`
/// converge with an explicit `loop` (and across languages: same synthesis for both).
fn break_guard(cond: NormNode, span: (u32, u32)) -> NormNode {
    let mut operand = cond;
    operand.field = Some("operand".into());
    let bang = NormNode::new("!", Some("op"), span, Vec::new());
    let guard = NormNode::new(kind::UNOP, Some("guard"), span, vec![bang, operand]);
    let brk = NormNode::new(kind::BREAK, None, span, Vec::new());
    let body = NormNode::new(kind::BLOCK, Some("body"), span, vec![brk]);
    NormNode::new(
        kind::BRANCH,
        None,
        span,
        vec![make_arm(Some(guard), Some(body), span)],
    )
}
