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
    fn free_names_stay_external() {
        // a→v0, b→v1 (declared); called functions g/h stay external (by name).
        let s = abstracted("fn f(a: i32) { let b = g(a); return h(b); }");
        assert!(s.contains("(Var@param v0)"), "{s}");
        assert!(s.contains("(Var@target v1)"), "{s}");
        assert!(s.contains("(Var@callee g)"), "{s}");
        assert!(s.contains("(Var@callee h)"), "{s}");
    }
}
