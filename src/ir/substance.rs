//! Seam C (docs/native-analysis-overrides-plan.md §5; docs/substantiality-metric.md §0.3):
//! shape-keyed boilerplate substantiality discount.
//!
//! A per-language set of **canonical-shape recognizers** on the canonical IR marks
//! ubiquitous non-signal idioms (the Go/Python `if err != nil { return err }`
//! guard-clause family, trivial accessors) as **zero-substance in match-time
//! ranking** — as D30 dispatch and D14 machinery already contribute zero. This is
//! the recurrence-graded boilerplate the §0.3 shared-landmark-df measurement flagged
//! (boilerplate-vs-genuine AUC 0.905): the FP source that inflates a coincidental
//! group's consolidation value.
//!
//! **Immunity (the invariant this must never break):** the recognizers change
//! *ranking only* — never the fingerprint, the canonical tree, or the cache key.
//! They are keyed on canonical **shape** (per the promotion gate: shape, not a
//! native kind), so ONE recognizer serves every IR frontend; the discount is a
//! small, syntactic, same-language classification. Hash-neutral by construction.

use crate::ir::kind;
use crate::tree::{Label, NormNode};

/// Nullish sentinels across the IR frontends (lowercased): Go `nil`, Python
/// `None`, C-family `null`/`NULL`/`nullptr`, JS `undefined`. The label the
/// frontend lowered the sentinel to (Go `nil` → `NativeStmt "nil"`, Python
/// `None` → `NativeStmt "none"`).
const NULLISH: &[&str] = &["nil", "none", "null", "nullptr", "undefined"];

/// Comparison operators that form a nullish guard: `x == nil` / `x != nil` /
/// `x is None`. (`is not` lowers separately and is intentionally not covered.)
const GUARD_OPS: &[&str] = &["==", "!=", "is"];

/// "Nothing / error present → bail" predicate methods (Rust `Option`/`Result`),
/// the Call-shaped analogue of the Binop nullish guard. Conservative: excludes
/// `is_some`/`is_ok`/`is_empty`, whose early-return can be substantive control flow.
const NULLISH_METHODS: &[&str] = &["is_none", "is_err", "is_nil", "is_null"];

/// Total token mass of recognized boilerplate idioms within `tree` — the substance
/// to *discount* from the ranking value. A whole trivial accessor counts entirely;
/// otherwise, each nullish guard-clause subtree counts once (no double counting,
/// no recursion into a counted idiom).
pub fn boilerplate_mass(tree: &NormNode) -> u32 {
    if is_trivial_accessor_unit(tree) {
        return tree.token_count();
    }
    guard_mass(tree)
}

fn guard_mass(node: &NormNode) -> u32 {
    if is_nullish_guard_clause(node) {
        return node.token_count();
    }
    node.children.iter().map(guard_mass).sum()
}

// ---- guard-clause (err-check / nil-check) recognizer ----

/// A single-arm `if` whose guard is a nullish comparison / predicate and whose
/// body early-exits (`return`/`break`/`continue`) — the `if err != nil { return
/// err }` family. Shape-keyed, so it fires identically on Go, Python (`is None`),
/// and Rust (`.is_none()`). Multi-arm branches (match / if-else / ternary) are
/// excluded: a guard clause has exactly one arm and no alternative.
pub fn is_nullish_guard_clause(node: &NormNode) -> bool {
    if node.kind.as_ref() != kind::BRANCH {
        return false;
    }
    let arms: Vec<&NormNode> = node
        .children
        .iter()
        .filter(|c| c.kind.as_ref() == kind::ARM)
        .collect();
    if arms.len() != 1 {
        return false;
    }
    let arm = arms[0];
    let Some(guard) = child_by_field(arm, "guard") else {
        return false;
    };
    let Some(body) = child_by_field(arm, "body") else {
        return false;
    };
    is_nullish_test(guard) && is_early_exit_body(body)
}

/// A nullish comparison (`x == nil` / `x is None`) or a nullish predicate call
/// (`x.is_none()` / `x.is_err()`).
fn is_nullish_test(guard: &NormNode) -> bool {
    match guard.kind.as_ref() {
        kind::BINOP => {
            let op_ok =
                child_by_field(guard, "op").is_some_and(|op| GUARD_OPS.contains(&op.kind.as_ref()));
            let has_nullish = guard.children.iter().any(is_nullish_leaf);
            op_ok && has_nullish
        }
        kind::CALL => callee_method_name(guard)
            .is_some_and(|m| NULLISH_METHODS.contains(&m.to_ascii_lowercase().as_str())),
        _ => false,
    }
}

/// A short block that transfers control out — the guard-clause body. Bounded to a
/// few statements so a large substantive block guarded by a nil-check is not
/// discounted (only the terminal early exit matters).
fn is_early_exit_body(body: &NormNode) -> bool {
    if body.kind.as_ref() != kind::BLOCK {
        return false;
    }
    let stmts = &body.children;
    if stmts.is_empty() || stmts.len() > 3 {
        return false;
    }
    matches!(
        stmts.last().map(|s| s.kind.as_ref()),
        Some(kind::RETURN | kind::BREAK | kind::CONTINUE)
    )
}

// ---- trivial-accessor (getter) recognizer ----

/// A unit whose whole body is `return <field|var|index>` (optionally `&`/`*`
/// wrapped) — a trivial getter. Recognized only at the unit-body level (a lone
/// field-return inside a larger function is not boilerplate). Note: such units are
/// below the reporting token floor, so this is dormant for group ranking at the
/// current floors — present for correctness and for the api-profile / lowered-floor
/// paths.
pub fn is_trivial_accessor_unit(unit: &NormNode) -> bool {
    if unit.kind.as_ref() != kind::UNIT {
        return false;
    }
    let Some(body) = unit
        .children
        .iter()
        .find(|c| c.kind.as_ref() == kind::BLOCK)
    else {
        return false;
    };
    if body.children.len() != 1 {
        return false;
    }
    let stmt = &body.children[0];
    if stmt.kind.as_ref() != kind::RETURN {
        return false;
    }
    // The returned expression (Return's sole child) is a simple accessor.
    stmt.children.iter().any(is_simple_accessor)
}

fn is_simple_accessor(n: &NormNode) -> bool {
    match n.kind.as_ref() {
        kind::VAR | kind::FIELD | kind::INDEX => true,
        kind::UNOP => n
            .children
            .iter()
            .any(|c| c.field.as_deref() != Some("op") && is_simple_accessor(c)),
        _ => false,
    }
}

// ---- shared helpers ----

fn child_by_field<'a>(node: &'a NormNode, field: &str) -> Option<&'a NormNode> {
    node.children
        .iter()
        .find(|c| c.field.as_deref() == Some(field))
}

fn label_str(n: &NormNode) -> Option<&str> {
    match &n.label {
        Some(Label::Raw(t) | Label::RawLit(t)) => Some(t.as_ref()),
        Some(Label::External(t) | Label::LitKept(t)) => Some(t.as_ref()),
        _ => None,
    }
}

fn is_nullish_leaf(n: &NormNode) -> bool {
    label_str(n).is_some_and(|s| NULLISH.contains(&s.to_ascii_lowercase().as_str()))
}

/// The method name of a call whose callee is `recv.method(...)` or bare `method(...)`.
fn callee_method_name(call: &NormNode) -> Option<&str> {
    let callee = child_by_field(call, "callee")?;
    match callee.kind.as_ref() {
        kind::FIELD => child_by_field(callee, "name").and_then(label_str),
        kind::VAR => label_str(callee),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::lang::Lang;
    use crate::unit;
    use std::path::PathBuf;

    fn tree(src: &str, lang: Lang, ext: &str) -> NormNode {
        let cfg = Config::default();
        let path = PathBuf::from(format!("t.{ext}"));
        let (units, _) = unit::extract_file_units(&path, src, lang, &cfg);
        units
            .into_iter()
            .next()
            .expect("one unit")
            .tree
            .into_resident()
    }

    #[test]
    fn go_err_check_is_boilerplate() {
        // `if err != nil { return err }` guard inside a larger function.
        let t = tree(
            "func f() error {\n  x, err := g()\n  if err != nil {\n    return err\n  }\n  return process(x)\n}\n",
            Lang::Go,
            "go",
        );
        let mass = boilerplate_mass(&t);
        assert!(mass > 0, "err-check guard recognized: {mass}");
        // Only the guard clause is boilerplate, not the whole function.
        assert!(
            mass < t.token_count(),
            "not the whole unit: {mass}/{}",
            t.token_count()
        );
    }

    #[test]
    fn go_nil_check_return_nil_is_boilerplate() {
        let t = tree(
            "func f(p *T) error {\n  if p == nil {\n    return nil\n  }\n  return p.Do()\n}\n",
            Lang::Go,
            "go",
        );
        assert!(boilerplate_mass(&t) > 0);
    }

    #[test]
    fn python_is_none_guard_is_boilerplate() {
        let t = tree(
            "def f(x):\n    if x is None:\n        return None\n    return x.do()\n",
            Lang::Python,
            "py",
        );
        assert!(boilerplate_mass(&t) > 0);
    }

    #[test]
    fn rust_is_none_guard_is_boilerplate() {
        let t = tree(
            "fn f(x: Option<i32>) -> i32 {\n  if x.is_none() {\n    return 0;\n  }\n  x.unwrap() + more()\n}\n",
            Lang::Rust,
            "rs",
        );
        assert!(boilerplate_mass(&t) > 0);
    }

    #[test]
    fn ordinary_relational_guard_is_not_boilerplate() {
        // The formats.rs FN_C shape: `if *points < cutoff { continue; }` — a real
        // guard but NOT nullish. Must NOT be discounted (protects genuine clones).
        let t = tree(
            "fn f(items: &[(String, i64)], cutoff: i64) -> i64 {\n  let mut acc = 0;\n  for (t, points) in items {\n    if *points < cutoff { continue; }\n    acc += points;\n  }\n  acc\n}\n",
            Lang::Rust,
            "rs",
        );
        assert_eq!(
            boilerplate_mass(&t),
            0,
            "relational guard is not nullish boilerplate"
        );
    }

    #[test]
    fn if_else_is_not_a_guard_clause() {
        // Two-arm branch (has an alternative) — not a guard clause even if it
        // early-returns, and the condition is relational anyway.
        let t = tree(
            "fn f(x: i32) -> i32 {\n  if x > 0 { return x; } else { return -x; }\n}\n",
            Lang::Rust,
            "rs",
        );
        assert_eq!(boilerplate_mass(&t), 0);
    }

    #[test]
    fn go_getter_is_trivial_accessor() {
        let t = tree(
            "func (s *S) Name() string {\n  return s.name\n}\n",
            Lang::Go,
            "go",
        );
        assert!(is_trivial_accessor_unit(&t));
        assert_eq!(boilerplate_mass(&t), t.token_count());
    }
}
