//! Rust frontend: tree-sitter `function_item` CST → canonical IR. A *mapping* only —
//! every synthesis and recursion helper is shared in [`super`]. Contrast the historical
//! `src/lang/rust.rs` (the full per-grammar normalizer): this is a small fraction of it.

use super::{
    Frontend, block, drop_parens, leaf, lit, lower_assign, lower_binary, lower_call,
    lower_function, lower_if, lower_loop, lower_return, lower_source, lower_unary_positional,
    lower_unit, lower_while, native, unwrap_stmt, var,
};
use crate::ir::kind;
use crate::ir::transform::TransformLog;
use crate::lang::Lang;
use crate::tree::{Bucket, NormNode};
use tree_sitter::Node;

/// Lower a Rust `function_item` CST node to a canonical IR tree + its transform log.
pub fn lower_rust(node: Node, src: &str) -> (NormNode, TransformLog) {
    lower_unit(&Rust, node, src)
}

/// Convenience: parse Rust `src` and lower its first `function_item`.
pub fn lower_rust_source(src: &str) -> Option<(NormNode, TransformLog)> {
    lower_source(&Rust, Lang::Rust, "function_item", src)
}

pub(crate) struct Rust;

impl Frontend for Rust {
    fn lower_node(
        &self,
        node: Node,
        field: Option<&str>,
        src: &str,
        log: &mut TransformLog,
    ) -> Option<NormNode> {
        let span = (node.start_byte() as u32, node.end_byte() as u32);
        match node.kind() {
            "function_item" => Some(lower_function(self, node, field, span, src, log)),
            "block" => Some(block(self, node, field, span, src, log)),
            "loop_expression" => Some(lower_loop(self, node, field, span, src, log)),
            "while_expression" => Some(lower_while(self, node, field, span, src, log)),
            "if_expression" => Some(lower_if(self, node, field, span, src, log)),
            "unary_expression" => Some(lower_unary_positional(self, node, field, span, src, log)),
            "break_expression" => Some(leaf(kind::BREAK, field, span)),
            "continue_expression" => Some(leaf(kind::CONTINUE, field, span)),
            "expression_statement" => unwrap_stmt(self, node, field, src, log),
            "call_expression" => Some(lower_call(
                self,
                node,
                field,
                span,
                ("function", "arguments"),
                src,
                log,
            )),
            "return_expression" => Some(lower_return(self, node, field, span, src, log)),
            "parenthesized_expression" => drop_parens(self, node, field, span, src, log),
            "binary_expression" => Some(lower_binary(
                self,
                node,
                field,
                span,
                ("left", "operator", "right"),
                src,
                log,
            )),
            "let_declaration" => Some(lower_assign(
                self,
                node,
                field,
                span,
                ("pattern", "value"),
                src,
                log,
            )),
            "integer_literal" => Some(lit(node, field, span, Bucket::Int, src, log)),
            "float_literal" => Some(lit(node, field, span, Bucket::Float, src, log)),
            "string_literal" | "raw_string_literal" => {
                Some(lit(node, field, span, Bucket::Str, src, log))
            }
            "char_literal" => Some(lit(node, field, span, Bucket::Char, src, log)),
            "boolean_literal" => Some(lit(node, field, span, Bucket::Bool, src, log)),
            "identifier"
            | "field_identifier"
            | "type_identifier"
            | "shorthand_field_identifier" => Some(var(node, field, span, src)),
            _ => Some(native(self, node, field, span, src, log)),
        }
    }

    fn lower_params(
        &self,
        params: Node,
        src: &str,
        log: &mut TransformLog,
        out: &mut Vec<NormNode>,
    ) {
        let mut cursor = params.walk();
        for p in params.named_children(&mut cursor) {
            if p.kind() == "parameter"
                && let Some(pat) = p.child_by_field_name("pattern")
                && let Some(v) = self.lower_node(pat, Some("param"), src, log)
            {
                out.push(v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::lower_rust_source;
    use crate::ir::render::to_sexpr;
    use crate::ir::transform::{TransformKind, TransformLog, Witness};

    fn rust(src: &str) -> (crate::tree::NormNode, TransformLog) {
        lower_rust_source(src).expect("a rust function")
    }

    #[test]
    fn lowers_core_subset_and_records_paren_drop() {
        let (ir, log) = rust("fn f(x: i32) { return (g(x)); }");
        assert_eq!(
            to_sexpr(&ir),
            "(Unit (Var@param x) (Block@body (Return (Call@value (Var@callee g) (Var@arg x)))))"
        );
        assert!(
            log.events()
                .iter()
                .any(|e| e.kind == TransformKind::ParenDrop)
        );
    }

    #[test]
    fn while_and_loop_converge_with_differing_logs() {
        let (while_ir, while_log) = rust("fn w() { while c() { s(); } }");
        let (loop_ir, loop_log) = rust("fn l() { loop { if !c() { break; } s(); } }");
        assert_eq!(to_sexpr(&while_ir), to_sexpr(&loop_ir));
        assert!(
            while_log
                .events()
                .iter()
                .any(|e| e.kind == TransformKind::LoopLower)
        );
        assert!(loop_log.is_empty());
    }

    #[test]
    fn literals_bucket_for_matching_but_the_value_is_detected_via_witness() {
        let (ir1, log1) = rust("fn f() { x(1); }");
        let (ir2, log2) = rust("fn g() { x(2); }");
        assert_eq!(to_sexpr(&ir1), to_sexpr(&ir2));
        let value = |log: &TransformLog| {
            log.events().iter().find_map(|e| match &e.witness {
                Witness::Literal(t) => Some(t.to_string()),
                _ => None,
            })
        };
        assert_eq!(value(&log1).as_deref(), Some("1"));
        assert_eq!(value(&log2).as_deref(), Some("2"));
    }
}
