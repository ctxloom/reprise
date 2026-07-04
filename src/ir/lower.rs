//! Per-language frontends: tree-sitter CST → canonical IR (`docs/SIMILARITY-IR.md`
//! §6). Increment 2 is a minimal **Rust** frontend over a core subset (Unit / Block /
//! Call / Return / Assign / Binop / Var / Lit) with the transform log wired in —
//! `ParenDrop` demonstrated end-to-end. Loop/recursion lowering, the identifier /
//! literal abstraction passes, and the Python frontend land in later increments; the
//! per-language `Frontend` trait emerges when the second language shares this code.

use crate::ir::kind;
use crate::ir::transform::{TransformKind, TransformLog, Witness};
use crate::tree::{Bucket, Label, NormNode};
use tree_sitter::Node;

/// Lower a Rust `function_item` CST node to a canonical IR tree + its transform log.
pub fn lower_rust(node: Node, src: &str) -> (NormNode, TransformLog) {
    let mut log = TransformLog::new();
    let root = lower_node(node, None, src, &mut log)
        .unwrap_or_else(|| NormNode::new(kind::UNIT, None, span_of(node), Vec::new()));
    (root, log)
}

/// Convenience: parse Rust `src` and lower its first `function_item`. `None` if the
/// source contains no function.
pub fn lower_rust_source(src: &str) -> Option<(NormNode, TransformLog)> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&crate::lang::Lang::Rust.ts_language())
        .ok()?;
    let tree = parser.parse(src, None)?;
    let func = find_function_item(tree.root_node())?;
    Some(lower_rust(func, src))
}

fn find_function_item<'a>(node: Node<'a>) -> Option<Node<'a>> {
    if node.kind() == "function_item" {
        return Some(node);
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find_map(find_function_item)
}

fn span_of(node: Node) -> (u32, u32) {
    (node.start_byte() as u32, node.end_byte() as u32)
}

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    node.utf8_text(src.as_bytes()).unwrap_or_default()
}

/// Lower one CST node into a canonical IR node (or `None` if it is dropped).
fn lower_node(
    node: Node,
    field: Option<&str>,
    src: &str,
    log: &mut TransformLog,
) -> Option<NormNode> {
    let span = span_of(node);
    match node.kind() {
        "function_item" => {
            let mut children = Vec::new();
            if let Some(params) = node.child_by_field_name("parameters") {
                lower_params(params, src, log, &mut children);
            }
            let body = node
                .child_by_field_name("body")
                .and_then(|b| lower_node(b, Some("body"), src, log))
                .unwrap_or_else(|| NormNode::new(kind::BLOCK, Some("body"), span, Vec::new()));
            children.push(body);
            Some(NormNode::new(kind::UNIT, field, span, children))
        }
        "block" => Some(NormNode::new(
            kind::BLOCK,
            field,
            span,
            lower_stmts(node, src, log),
        )),
        "loop_expression" => {
            // Already the canonical core — no transform recorded.
            let body = lower_body(node, src, log, span);
            Some(NormNode::new(kind::LOOP, field, span, vec![body]))
        }
        "while_expression" => {
            // `while cond { body }` → `Loop { break-guard; body }` (§5.2.2). Lossy:
            // record LoopLower with the source form so it reverses for display, and
            // so the log diff explains a while↔loop match (§15.2).
            let cond = node
                .child_by_field_name("condition")
                .and_then(|c| lower_node(c, None, src, log));
            let mut body = lower_body(node, src, log, span);
            if let Some(cond) = cond {
                body.children.insert(0, break_guard(cond, span));
            }
            log.record(
                TransformKind::LoopLower,
                span,
                Witness::LoopForm("while".into()),
            );
            Some(NormNode::new(kind::LOOP, field, span, vec![body]))
        }
        "if_expression" => {
            let guard = node
                .child_by_field_name("condition")
                .and_then(|c| lower_node(c, Some("guard"), src, log));
            let body = node
                .child_by_field_name("consequence")
                .and_then(|c| lower_node(c, Some("body"), src, log));
            let mut arms = vec![make_arm(guard, body, span)];
            if let Some(alt) = node.child_by_field_name("alternative") {
                // `else_clause` wraps a block or a nested `if`; the else arm has no guard.
                let mut cursor = alt.walk();
                let inner = alt
                    .named_children(&mut cursor)
                    .find_map(|c| lower_node(c, Some("body"), src, log));
                if inner.is_some() {
                    arms.push(make_arm(None, inner, span));
                }
            }
            Some(NormNode::new(kind::BRANCH, field, span, arms))
        }
        "unary_expression" => {
            let mut op = None;
            let mut operand = None;
            let mut cursor = node.walk();
            for c in node.children(&mut cursor) {
                if c.is_named() {
                    operand = lower_node(c, Some("operand"), src, log);
                } else if op.is_none() {
                    op = Some(NormNode::new(
                        text(c, src),
                        Some("op"),
                        span_of(c),
                        Vec::new(),
                    ));
                }
            }
            let children = op.into_iter().chain(operand).collect();
            Some(NormNode::new(kind::UNOP, field, span, children))
        }
        "break_expression" => Some(NormNode::new(kind::BREAK, field, span, Vec::new())),
        "continue_expression" => Some(NormNode::new(kind::CONTINUE, field, span, Vec::new())),
        "expression_statement" => {
            // Unwrap: lower the single inner expression, keeping the stmt's field.
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .find_map(|c| lower_node(c, field, src, log))
        }
        "call_expression" => {
            let mut children = Vec::new();
            if let Some(c) = node
                .child_by_field_name("function")
                .and_then(|n| lower_node(n, Some("callee"), src, log))
            {
                children.push(c);
            }
            if let Some(args) = node.child_by_field_name("arguments") {
                let mut cursor = args.walk();
                for a in args.named_children(&mut cursor) {
                    if let Some(c) = lower_node(a, Some("arg"), src, log) {
                        children.push(c);
                    }
                }
            }
            Some(NormNode::new(kind::CALL, field, span, children))
        }
        "return_expression" => {
            let mut cursor = node.walk();
            let value = node
                .named_children(&mut cursor)
                .find_map(|c| lower_node(c, Some("value"), src, log));
            Some(NormNode::new(
                kind::RETURN,
                field,
                span,
                value.into_iter().collect(),
            ))
        }
        "parenthesized_expression" => {
            // Redundant grouping: drop it, record the (bijective) transform, and
            // return the inner expression inheriting this node's field.
            log.record(TransformKind::ParenDrop, span, Witness::None);
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .find_map(|c| lower_node(c, field, src, log))
        }
        "binary_expression" => {
            let mut children = Vec::new();
            if let Some(l) = node
                .child_by_field_name("left")
                .and_then(|n| lower_node(n, Some("left"), src, log))
            {
                children.push(l);
            }
            if let Some(op) = node.child_by_field_name("operator") {
                children.push(NormNode::new(
                    text(op, src),
                    Some("op"),
                    span_of(op),
                    Vec::new(),
                ));
            }
            if let Some(r) = node
                .child_by_field_name("right")
                .and_then(|n| lower_node(n, Some("right"), src, log))
            {
                children.push(r);
            }
            Some(NormNode::new(kind::BINOP, field, span, children))
        }
        "let_declaration" => {
            let mut children = Vec::new();
            if let Some(t) = node
                .child_by_field_name("pattern")
                .and_then(|n| lower_node(n, Some("target"), src, log))
            {
                children.push(t);
            }
            if let Some(v) = node
                .child_by_field_name("value")
                .and_then(|n| lower_node(n, Some("value"), src, log))
            {
                children.push(v);
            }
            Some(NormNode::new(kind::ASSIGN, field, span, children))
        }
        "integer_literal" | "float_literal" | "string_literal" | "raw_string_literal"
        | "char_literal" | "boolean_literal" => {
            // Literal → typed bucket. The per-language kind→bucket map is a frontend
            // concern; the keep-list refinement (§5.2.5) is a later increment.
            let bucket = match node.kind() {
                "integer_literal" => Bucket::Int,
                "float_literal" => Bucket::Float,
                "char_literal" => Bucket::Char,
                "boolean_literal" => Bucket::Bool,
                _ => Bucket::Str,
            };
            Some(
                NormNode::new(kind::LIT, field, span, Vec::new())
                    .with_label(Label::LitBucket(bucket)),
            )
        }
        "identifier" | "field_identifier" | "type_identifier" | "shorthand_field_identifier" => {
            Some(
                NormNode::new(kind::VAR, field, span, Vec::new())
                    .with_label(Label::Raw(text(node, src).into())),
            )
        }
        _ => {
            // Unmodeled construct → the opaque, resolution-free escape hatch
            // (§12.1). The tree-sitter kind is the discriminating tag; children are
            // lowered so nested modeled constructs still surface. Refined later.
            Some(
                NormNode::new(kind::NATIVE_STMT, field, span, lower_stmts(node, src, log))
                    .with_label(Label::External(node.kind().into())),
            )
        }
    }
}

fn lower_params(params: Node, src: &str, log: &mut TransformLog, out: &mut Vec<NormNode>) {
    let mut cursor = params.walk();
    for p in params.named_children(&mut cursor) {
        if p.kind() == "parameter"
            && let Some(pat) = p.child_by_field_name("pattern")
            && let Some(v) = lower_node(pat, Some("param"), src, log)
        {
            out.push(v);
        }
    }
}

fn lower_stmts(node: Node, src: &str, log: &mut TransformLog) -> Vec<NormNode> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter_map(|c| lower_node(c, None, src, log))
        .collect()
}

/// Lower a node's `body` block, defaulting to an empty `Block`.
fn lower_body(node: Node, src: &str, log: &mut TransformLog, span: (u32, u32)) -> NormNode {
    node.child_by_field_name("body")
        .and_then(|b| lower_node(b, Some("body"), src, log))
        .unwrap_or_else(|| NormNode::new(kind::BLOCK, Some("body"), span, Vec::new()))
}

/// `Arm[ guard?, body? ]` — a member of a canonical `Branch` (§14). The else/default
/// arm carries no guard.
fn make_arm(guard: Option<NormNode>, body: Option<NormNode>, span: (u32, u32)) -> NormNode {
    let children = guard.into_iter().chain(body).collect();
    NormNode::new(kind::ARM, Some("arm"), span, children)
}

/// The canonical break-guard `Branch[ Arm[ !cond → { Break } ] ]` — built to be
/// byte-identical to a hand-written `if !cond { break }` lowered through the
/// `if_expression` arm, which is exactly what makes `while` converge with an explicit
/// `loop` (only the transform log differs).
fn break_guard(cond: NormNode, span: (u32, u32)) -> NormNode {
    let mut operand = cond;
    operand.field = Some("operand".into());
    let bang = NormNode::new("!", Some("op"), span, Vec::new());
    let guard = NormNode::new(kind::UNOP, Some("guard"), span, vec![bang, operand]);
    let brk = NormNode::new(kind::BREAK, None, span, Vec::new());
    let body = NormNode::new(kind::BLOCK, Some("body"), span, vec![brk]);
    let arm = make_arm(Some(guard), Some(body), span);
    NormNode::new(kind::BRANCH, None, span, vec![arm])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::render::to_sexpr;

    fn lower_first_fn(src: &str) -> (NormNode, TransformLog) {
        lower_rust_source(src).expect("a function_item")
    }

    #[test]
    fn lowers_core_subset_and_records_paren_drop() {
        let (ir, log) = lower_first_fn("fn f(x: i32) { return (g(x)); }");
        assert_eq!(
            to_sexpr(&ir),
            "(Unit (Var@param x) (Block@body (Return (Call@value (Var@callee g) (Var@arg x)))))"
        );
        // The redundant parens around g(x) were dropped and recorded — one event.
        assert_eq!(log.len(), 1);
        assert_eq!(log.events()[0].kind, TransformKind::ParenDrop);
    }

    #[test]
    fn while_and_loop_converge_with_differing_logs() {
        let (while_ir, while_log) = lower_first_fn("fn w() { while c() { s(); } }");
        let (loop_ir, loop_log) = lower_first_fn("fn l() { loop { if !c() { break; } s(); } }");
        // The flagship convergence: a `while` and the equivalent hand-written
        // `loop { if !cond { break } … }` lower to byte-identical canonical IR …
        assert_eq!(to_sexpr(&while_ir), to_sexpr(&loop_ir));
        // … while their transform logs differ — the match-explanation property
        // (§15.2): the `while` recorded a LoopLower, the explicit `loop` did not.
        assert!(
            while_log
                .events()
                .iter()
                .any(|e| e.kind == TransformKind::LoopLower)
        );
        assert!(loop_log.is_empty());
    }
}
