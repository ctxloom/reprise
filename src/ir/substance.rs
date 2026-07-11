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

use crate::intern::{Field, Kind, LabelInterner};
use crate::ir::field;
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
///
/// Takes the scan's `LabelInterner` (interning-id-conversion WP): the nullish-guard
/// recognizer resolves `Label::External`/`LitKept` text (nullish sentinels, predicate
/// method names) via [`label_str`], which needs the SAME interner that built `tree`
/// (interning preamble rule 5 — a fresh interner has no entries for `tree`'s ids).
pub fn boilerplate_mass(tree: &NormNode, li: &LabelInterner) -> u32 {
    if is_trivial_accessor_unit(tree) {
        return tree.token_count();
    }
    guard_mass(tree, li)
}

fn guard_mass(node: &NormNode, li: &LabelInterner) -> u32 {
    if is_nullish_guard_clause(node, li) {
        return node.token_count();
    }
    node.children.iter().map(|c| guard_mass(c, li)).sum()
}

// ---- guard-clause (err-check / nil-check) recognizer ----

/// A single-arm `if` whose guard is a nullish comparison / predicate and whose
/// body early-exits (`return`/`break`/`continue`) — the `if err != nil { return
/// err }` family. Shape-keyed, so it fires identically on Go, Python (`is None`),
/// and Rust (`.is_none()`). Multi-arm branches (match / if-else / ternary) are
/// excluded: a guard clause has exactly one arm and no alternative.
pub fn is_nullish_guard_clause(node: &NormNode, li: &LabelInterner) -> bool {
    if node.kind != kind::id::BRANCH {
        return false;
    }
    let arms: Vec<&NormNode> = node
        .children
        .iter()
        .filter(|c| c.kind == kind::id::ARM)
        .collect();
    if arms.len() != 1 {
        return false;
    }
    let arm = arms[0];
    let Some(guard) = child_by_field(arm, field::id::GUARD) else {
        return false;
    };
    let Some(body) = child_by_field(arm, field::id::BODY) else {
        return false;
    };
    is_nullish_test(guard, li) && is_early_exit_body(body)
}

/// A nullish comparison (`x == nil` / `x is None`) or a nullish predicate call
/// (`x.is_none()` / `x.is_err()`).
fn is_nullish_test(guard: &NormNode, li: &LabelInterner) -> bool {
    static GUARD_OP_KINDS: std::sync::LazyLock<Vec<Kind>> =
        std::sync::LazyLock::new(|| GUARD_OPS.iter().map(|s| Kind::intern(s)).collect());
    if guard.kind == kind::id::BINOP {
        let op_ok = child_by_field(guard, field::id::OP)
            .is_some_and(|op| GUARD_OP_KINDS.contains(&op.kind));
        let has_nullish = guard.children.iter().any(|c| is_nullish_leaf(c, li));
        op_ok && has_nullish
    } else if guard.kind == kind::id::CALL {
        callee_method_name(guard, li)
            .is_some_and(|m| NULLISH_METHODS.contains(&m.to_ascii_lowercase().as_str()))
    } else {
        false
    }
}

/// A short block that transfers control out — the guard-clause body. Bounded to a
/// few statements so a large substantive block guarded by a nil-check is not
/// discounted (only the terminal early exit matters).
fn is_early_exit_body(body: &NormNode) -> bool {
    if body.kind != kind::id::BLOCK {
        return false;
    }
    let stmts = &body.children;
    if stmts.is_empty() || stmts.len() > 3 {
        return false;
    }
    stmts.last().is_some_and(|s| {
        s.kind == kind::id::RETURN || s.kind == kind::id::BREAK || s.kind == kind::id::CONTINUE
    })
}

// ---- trivial-accessor (getter) recognizer ----

/// A unit whose whole body is `return <field|var|index>` (optionally `&`/`*`
/// wrapped) — a trivial getter. Recognized only at the unit-body level (a lone
/// field-return inside a larger function is not boilerplate). Note: such units are
/// below the reporting token floor, so this is dormant for group ranking at the
/// current floors — present for correctness and for the api-profile / lowered-floor
/// paths.
pub fn is_trivial_accessor_unit(unit: &NormNode) -> bool {
    if unit.kind != kind::id::UNIT {
        return false;
    }
    let Some(body) = unit.children.iter().find(|c| c.kind == kind::id::BLOCK) else {
        return false;
    };
    if body.children.len() != 1 {
        return false;
    }
    let stmt = &body.children[0];
    if stmt.kind != kind::id::RETURN {
        return false;
    }
    // The returned expression (Return's sole child) is a simple accessor.
    stmt.children.iter().any(is_simple_accessor)
}

fn is_simple_accessor(n: &NormNode) -> bool {
    if n.kind == kind::id::VAR || n.kind == kind::id::FIELD || n.kind == kind::id::INDEX {
        true
    } else if n.kind == kind::id::UNOP {
        n.children
            .iter()
            .any(|c| c.field != Some(field::id::OP) && is_simple_accessor(c))
    } else {
        false
    }
}

// ---- shared helpers ----

fn child_by_field(node: &NormNode, field: Field) -> Option<&NormNode> {
    node.children.iter().find(|c| c.field == Some(field))
}

/// Resolve a node's label to text, whatever payload it carries: `Raw`/`RawLit` are
/// already `Box<str>`; `External`/`LitKept` are `LSym`s resolved through `li` — the SAME
/// interner that built `n` (interning preamble rule 5). Owned (not borrowed): an
/// `External`/`LitKept` resolve hands back a fresh `Arc<str>`, so there is no borrowed
/// `&str` to return uniformly across all four variants: callers immediately lowercase
/// the result anyway, so the allocation is not extra work.
fn label_str(n: &NormNode, li: &LabelInterner) -> Option<String> {
    match &n.label {
        Some(Label::Raw(t) | Label::RawLit(t)) => Some(t.to_string()),
        Some(Label::External(t) | Label::LitKept(t)) => Some(li.resolve(*t).to_string()),
        _ => None,
    }
}

fn is_nullish_leaf(n: &NormNode, li: &LabelInterner) -> bool {
    label_str(n, li).is_some_and(|s| NULLISH.contains(&s.to_ascii_lowercase().as_str()))
}

/// The method name of a call whose callee is `recv.method(...)` or bare `method(...)`.
fn callee_method_name(call: &NormNode, li: &LabelInterner) -> Option<String> {
    let callee = child_by_field(call, field::id::CALLEE)?;
    if callee.kind == kind::id::FIELD {
        child_by_field(callee, field::id::NAME).and_then(|n| label_str(n, li))
    } else if callee.kind == kind::id::VAR {
        label_str(callee, li)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::lang::Lang;
    use crate::unit;
    use std::path::PathBuf;

    /// Builds the tree AND returns the `LabelInterner` that built it — `boilerplate_mass`
    /// needs to resolve this SAME interner's ids (interning preamble rule 5); a fresh,
    /// unrelated interner would panic on `resolve` (or worse, silently resolve the wrong
    /// string) for any `Label::External`/`LitKept` the tree carries. Goes through
    /// `extract_file_units_keep_raw` (not the convenience `extract_file_units`, whose
    /// throwaway internal interner is discarded before returning) so the caller can keep
    /// the interner alive alongside the tree.
    fn tree(src: &str, lang: Lang, ext: &str) -> (NormNode, std::sync::Arc<LabelInterner>) {
        let cfg = Config::default();
        let path = PathBuf::from(format!("t.{ext}"));
        let li = LabelInterner::new();
        let fu = unit::extract_file_units_keep_raw(&path, src, lang, &cfg, &li);
        let t = fu
            .units
            .into_iter()
            .next()
            .expect("one unit")
            .tree
            .into_resident();
        (t, li)
    }

    #[test]
    fn go_err_check_is_boilerplate() {
        // `if err != nil { return err }` guard inside a larger function.
        let (t, li) = tree(
            "func f() error {\n  x, err := g()\n  if err != nil {\n    return err\n  }\n  return process(x)\n}\n",
            Lang::Go,
            "go",
        );
        let mass = boilerplate_mass(&t, &li);
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
        let (t, li) = tree(
            "func f(p *T) error {\n  if p == nil {\n    return nil\n  }\n  return p.Do()\n}\n",
            Lang::Go,
            "go",
        );
        assert!(boilerplate_mass(&t, &li) > 0);
    }

    #[test]
    fn python_is_none_guard_is_boilerplate() {
        let (t, li) = tree(
            "def f(x):\n    if x is None:\n        return None\n    return x.do()\n",
            Lang::Python,
            "py",
        );
        assert!(boilerplate_mass(&t, &li) > 0);
    }

    #[test]
    fn rust_is_none_guard_is_boilerplate() {
        let (t, li) = tree(
            "fn f(x: Option<i32>) -> i32 {\n  if x.is_none() {\n    return 0;\n  }\n  x.unwrap() + more()\n}\n",
            Lang::Rust,
            "rs",
        );
        assert!(boilerplate_mass(&t, &li) > 0);
    }

    #[test]
    fn ordinary_relational_guard_is_not_boilerplate() {
        // The formats.rs FN_C shape: `if *points < cutoff { continue; }` — a real
        // guard but NOT nullish. Must NOT be discounted (protects genuine clones).
        let (t, li) = tree(
            "fn f(items: &[(String, i64)], cutoff: i64) -> i64 {\n  let mut acc = 0;\n  for (t, points) in items {\n    if *points < cutoff { continue; }\n    acc += points;\n  }\n  acc\n}\n",
            Lang::Rust,
            "rs",
        );
        assert_eq!(
            boilerplate_mass(&t, &li),
            0,
            "relational guard is not nullish boilerplate"
        );
    }

    #[test]
    fn if_else_is_not_a_guard_clause() {
        // Two-arm branch (has an alternative) — not a guard clause even if it
        // early-returns, and the condition is relational anyway.
        let (t, li) = tree(
            "fn f(x: i32) -> i32 {\n  if x > 0 { return x; } else { return -x; }\n}\n",
            Lang::Rust,
            "rs",
        );
        assert_eq!(boilerplate_mass(&t, &li), 0);
    }

    #[test]
    fn go_getter_is_trivial_accessor() {
        let (t, li) = tree(
            "func (s *S) Name() string {\n  return s.name\n}\n",
            Lang::Go,
            "go",
        );
        assert!(is_trivial_accessor_unit(&t));
        assert_eq!(boilerplate_mass(&t, &li), t.token_count());
    }
}
