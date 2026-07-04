//! Python frontend: tree-sitter `function_definition` CST → canonical IR. A *mapping*
//! only — every synthesis and recursion helper is shared in [`super`], identical to the
//! Rust frontend's. Contrast the historical `src/lang/python.rs` (the full per-grammar
//! normalizer): this is a small fraction of it, and it re-implements *nothing*.

use super::{
    Frontend, block, drop_parens, leaf, lit, lower_assign, lower_binary, lower_call, lower_for,
    lower_function, lower_if, lower_return, lower_source, lower_unary_field, lower_unit,
    lower_while, native, unwrap_stmt, var,
};
use crate::ir::kind;
use crate::ir::transform::TransformLog;
use crate::lang::Lang;
use crate::tree::{Bucket, NormNode};
use tree_sitter::Node;

/// Lower a Python `function_definition` CST node to a canonical IR tree + transform log.
pub fn lower_python(node: Node, src: &str) -> (NormNode, TransformLog) {
    lower_unit(&Python, node, src)
}

/// Convenience: parse Python `src` and lower its first `function_definition`.
pub fn lower_python_source(src: &str) -> Option<(NormNode, TransformLog)> {
    lower_source(&Python, Lang::Python, "function_definition", src)
}

pub(crate) struct Python;

impl Frontend for Python {
    fn lower_node(
        &self,
        node: Node,
        field: Option<&str>,
        src: &str,
        log: &mut TransformLog,
    ) -> Option<NormNode> {
        let span = (node.start_byte() as u32, node.end_byte() as u32);
        match node.kind() {
            "function_definition" => Some(lower_function(self, node, field, span, src, log)),
            "block" => Some(block(self, node, field, span, src, log)),
            "while_statement" => Some(lower_while(self, node, field, span, src, log)),
            "for_statement" => Some(lower_for(
                self,
                node,
                field,
                span,
                ("left", "right"),
                src,
                log,
            )),
            "if_statement" => Some(lower_if(self, node, field, span, src, log)),
            "not_operator" => Some(lower_unary_field(
                self, node, field, span, "argument", src, log,
            )),
            "break_statement" => Some(leaf(kind::BREAK, field, span)),
            "continue_statement" => Some(leaf(kind::CONTINUE, field, span)),
            "pass_statement" => None,
            "expression_statement" => unwrap_stmt(self, node, field, src, log),
            "call" => Some(lower_call(
                self,
                node,
                field,
                span,
                ("function", "arguments"),
                src,
                log,
            )),
            "return_statement" => Some(lower_return(self, node, field, span, src, log)),
            "parenthesized_expression" => drop_parens(self, node, field, span, src, log),
            "binary_operator" => Some(lower_binary(
                self,
                node,
                field,
                span,
                ("left", "operator", "right"),
                src,
                log,
            )),
            "assignment" => Some(lower_assign(
                self,
                node,
                field,
                span,
                ("left", "right"),
                src,
                log,
            )),
            "integer" => Some(lit(node, field, span, Bucket::Int, src, log)),
            "float" => Some(lit(node, field, span, Bucket::Float, src, log)),
            "string" => Some(lit(node, field, span, Bucket::Str, src, log)),
            "true" | "false" => Some(lit(node, field, span, Bucket::Bool, src, log)),
            "identifier" => Some(var(node, field, span, src)),
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
            let ident = match p.kind() {
                "identifier" => Some(p),
                "typed_parameter" | "default_parameter" => {
                    let mut c = p.walk();
                    p.named_children(&mut c).find(|n| n.kind() == "identifier")
                }
                _ => None,
            };
            if let Some(id) = ident
                && let Some(v) = self.lower_node(id, Some("param"), src, log)
            {
                out.push(v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::lower_python_source;
    use crate::frontend::lower_rust_source;
    use crate::ir::render::to_sexpr;

    #[test]
    fn python_lowers_the_core_subset() {
        let (ir, _log) = lower_python_source("def add(a):\n    return a + 1\n").unwrap();
        assert_eq!(
            to_sexpr(&ir),
            "(Unit (Var@param a) (Block@body (Return (Binop@value (Var@left a) (+@op) (Lit@right INT)))))"
        );
    }

    #[test]
    fn python_pass_is_dropped() {
        let (ir, _) = lower_python_source("def f():\n    pass\n").unwrap();
        assert_eq!(to_sexpr(&ir), "(Unit (Block@body))");
    }

    #[test]
    fn rust_and_python_for_loops_converge() {
        // Both `for` forms lower through the shared has_next/next iterated Loop.
        let (r, _) = lower_rust_source("fn f() { for x in xs { g(x); } }").unwrap();
        let (p, _) = lower_python_source("def f():\n    for x in xs:\n        g(x)\n").unwrap();
        let rs = to_sexpr(&crate::ir::abstract_idents(r));
        let ps = to_sexpr(&crate::ir::abstract_idents(p));
        assert_eq!(rs, ps);
        assert!(
            rs.contains("__has_next") && rs.contains("__next") && rs.contains("v0"),
            "{rs}"
        );
    }

    #[test]
    fn rust_and_python_loops_converge_to_one_canonical_form() {
        // The SAME shared break-guard synthesis + loop lowering drives both languages —
        // the frontends agree on the canonical form (they are partitioned in production
        // per §3, but the shared vocabulary is the whole point).
        let (r, _) = lower_rust_source("fn w() { while c() { s(); } }").unwrap();
        let (p, _) = lower_python_source("def w():\n    while c():\n        s()\n").unwrap();
        assert_eq!(to_sexpr(&r), to_sexpr(&p));
    }
}
