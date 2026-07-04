//! Language-agnostic IR passes (`docs/SIMILARITY-IR.md` §6): the algorithms that
//! were copied per profile now run **once** on the canonical IR. Increment 4:
//! identifier abstraction. Declared locals are structural — a `Var` in a
//! binding-field position (`param`/`target`) — so there is no per-language
//! `collect_declared` hook; the pass is fully language-agnostic.

use crate::ir::kind;
use crate::tree::{Label, NormNode};
use std::collections::{HashMap, HashSet};

/// Fields whose `Var` occupant is a binding site (a declared local). Grows as the
/// IR grows (match-arm binds, for-pattern binds) — still structural, never per-grammar.
fn is_bind_field(field: Option<&str>) -> bool {
    matches!(field, Some("param") | Some("target"))
}

/// Spec §5.2.4 on the IR: declared locals become positional `Local(n)` by first
/// occurrence; every other name stays `External`. Binding position is a field in the
/// canonical IR, so this is one algorithm for all languages — the per-profile
/// `collect_declared`/`always_external` split dissolves.
pub fn abstract_idents(mut root: NormNode) -> NormNode {
    let mut declared = HashSet::new();
    collect_declared(&root, &mut declared);
    let mut order: HashMap<Box<str>, u32> = HashMap::new();
    relabel(&mut root, &declared, &mut order);
    root
}

fn collect_declared(node: &NormNode, out: &mut HashSet<Box<str>>) {
    if node.kind.as_ref() == kind::VAR
        && is_bind_field(node.field.as_deref())
        && let Some(Label::Raw(t)) = &node.label
    {
        out.insert(t.clone());
    }
    for c in &node.children {
        collect_declared(c, out);
    }
}

fn relabel(node: &mut NormNode, declared: &HashSet<Box<str>>, order: &mut HashMap<Box<str>, u32>) {
    if node.kind.as_ref() == kind::VAR
        && let Some(Label::Raw(t)) = node.label.clone()
    {
        node.label = Some(if declared.contains(&t) {
            let next = order.len() as u32;
            Label::Local(*order.entry(t).or_insert(next))
        } else {
            Label::External(t)
        });
    }
    for c in &mut node.children {
        relabel(c, declared, order);
    }
}

/// Commutative operators (canonical IR spelling — both `&&` and Python `and`, etc.).
const COMMUTATIVE: &[&str] = &["+", "*", "&", "|", "^", "==", "!=", "&&", "||", "and", "or"];

/// Spec §5.2.6 on the IR: sort the operands of a commutative operator chain into a
/// canonical order (by field-stripped s-expression), so `a + b` and `b + a` converge.
/// Language-agnostic — the operator token is canonical in the IR. Unsound for floats /
/// operator overloading (accepted; the output is a report). Run *after* abstraction.
pub fn canonicalize_order(mut node: NormNode) -> NormNode {
    node.children = node.children.into_iter().map(canonicalize_order).collect();
    if node.kind.as_ref() == kind::BINOP
        && node.children.len() == 3
        && COMMUTATIVE.contains(&node.children[1].kind.as_ref())
    {
        let op = node.children[1].clone();
        let mut operands = Vec::new();
        flatten_chain(&node, op.kind.as_ref(), &mut operands);
        if operands.len() >= 2 {
            operands.sort_by_cached_key(crate::ir::render::to_sexpr);
            return rebuild_chain(operands, op, node.field);
        }
    }
    node
}

/// Collect operands of a same-operator commutative chain (left-assoc), field-stripped.
fn flatten_chain(node: &NormNode, op: &str, out: &mut Vec<NormNode>) {
    if node.kind.as_ref() == kind::BINOP
        && node.children.len() == 3
        && node.children[1].kind.as_ref() == op
    {
        flatten_chain(&node.children[0], op, out);
        let mut rhs = node.children[2].clone();
        rhs.field = None;
        out.push(rhs);
    } else {
        let mut n = node.clone();
        n.field = None;
        out.push(n);
    }
}

fn rebuild_chain(mut operands: Vec<NormNode>, op: NormNode, field: Option<Box<str>>) -> NormNode {
    let span = operands[0].span;
    let mut acc = operands.remove(0);
    acc.field = Some("left".into());
    for mut next in operands {
        next.field = Some("right".into());
        let mut op_clone = op.clone();
        op_clone.field = Some("op".into());
        acc = NormNode::new(kind::BINOP, None, span, vec![acc, op_clone, next]);
        acc.children[0].field = Some("left".into());
    }
    acc.field = field;
    acc
}

/// Spec §5.2.7 on the IR: drop a redundant trailing `Continue` at a loop-body tail
/// (source `continue` at the end of a loop is a no-op, and lowered recursion emits
/// one), so `for x in xs { f(x); continue }` converges with `for x in xs { f(x) }`.
pub fn strip_dead(mut node: NormNode) -> NormNode {
    node.children = node.children.into_iter().map(strip_dead).collect();
    if node.kind.as_ref() == kind::LOOP
        && let Some(body) = node
            .children
            .iter_mut()
            .find(|c| c.field.as_deref() == Some("body"))
    {
        while body
            .children
            .last()
            .is_some_and(|c| c.kind.as_ref() == kind::CONTINUE)
        {
            body.children.pop();
        }
    }
    node
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::{lower_python_source, lower_rust_source};
    use crate::ir::render::to_sexpr;

    fn abstracted(src: &str) -> String {
        let (ir, _log) = lower_rust_source(src).expect("a function");
        to_sexpr(&abstract_idents(ir))
    }

    #[test]
    fn one_pass_serves_both_languages() {
        // The SAME abstract_idents runs on Rust and Python IR: renamed params across
        // languages converge to the same canonical form — "one algorithm, all languages".
        let (r, _) = lower_rust_source("fn add(a: i32) { return a + 1; }").unwrap();
        let (p, _) = lower_python_source("def add(x):\n    return x + 1\n").unwrap();
        assert_eq!(to_sexpr(&abstract_idents(r)), to_sexpr(&abstract_idents(p)));
    }

    #[test]
    fn renamed_locals_and_bucketed_literals_converge() {
        // Two Type-2 clones (renamed locals, different literal) → one IR.
        let a = abstracted("fn f(a: i32) { return a + 1; }");
        let b = abstracted("fn g(b: i32) { return b + 2; }");
        assert_eq!(a, b, "renamed locals + bucketed literals must converge");
        assert_eq!(
            a,
            "(Unit (Var@param v0) (Block@body (Return (Binop@value (Var@left v0) (+@op) (Lit@right INT)))))"
        );
    }

    #[test]
    fn trailing_continue_is_stripped() {
        let strip = |src: &str| {
            let (ir, _) = lower_rust_source(src).unwrap();
            to_sexpr(&strip_dead(abstract_idents(ir)))
        };
        assert_eq!(
            strip("fn a() { for x in xs { f(x); continue; } }"),
            strip("fn b() { for x in xs { f(x); } }")
        );
    }

    #[test]
    fn commutative_operands_converge() {
        let canon = |src: &str| {
            let (ir, _) = lower_rust_source(src).unwrap();
            to_sexpr(&canonicalize_order(abstract_idents(ir)))
        };
        // `a + b` and `b + a` sort to the same canonical order.
        assert_eq!(
            canon("fn f() { return a + b; }"),
            canon("fn g() { return b + a; }")
        );
    }

    #[test]
    fn free_names_stay_external() {
        // a→v0, b→v1 (declared); called functions g/h stay external (by name).
        let s = abstracted("fn f(a: i32) { let b = g(a); return h(b); }");
        assert!(s.contains("(Var@param v0)"), "{s}");
        assert!(s.contains("(Var@target v1)"), "{s}");
        assert!(s.contains("(Var@callee g)"), "{s}");
        assert!(s.contains("(Var@callee h)"), "{s}");
    }
}
