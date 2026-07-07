//! Rust frontend: tree-sitter `function_item` CST → canonical IR. A *mapping* only —
//! every synthesis and recursion helper is shared in [`super`]. Contrast the historical
//! `src/lang/rust.rs` (the full per-grammar normalizer): this is a small fraction of it.

use super::Lowering as L;
use super::{
    Frontend, bind_pattern_idents, block_wrap, case_guard, lower_assign, lower_source, lower_unit,
    make_arm, make_branch, make_index, make_lambda, make_multi_assign, native, push_else_arm,
};
use crate::ir::kind;
use crate::ir::transform::TransformLog;
use crate::lang::Lang;
use crate::tree::{Bucket, NormNode};
use tree_sitter::Node;

/// Lower a Rust `function_item` CST node to a canonical IR tree + its transform log
/// (an enabled sink — the convenience for callers that want the event stream, e.g. tests;
/// the bulk path threads its own [`TransformLog::disabled`] via [`super::normalize`]).
pub fn lower_rust(node: Node, src: &str) -> (NormNode, TransformLog) {
    let mut log = TransformLog::new();
    let tree = lower_unit(&Rust, node, src, &mut log);
    (tree, log)
}

/// Convenience: parse Rust `src` and lower its first `function_item`.
pub fn lower_rust_source(src: &str) -> Option<(NormNode, TransformLog)> {
    let mut log = TransformLog::new();
    let tree = lower_source(&Rust, Lang::Rust, "function_item", src, &mut log)?;
    Some((tree, log))
}

pub(crate) struct Rust;

/// The data-driven Rust dispatch table (D-SP4-1b; `docs/sp4-grammar-lift-plan.md` §4): every
/// **uniform** `lower_node` arm — a CST kind handled by a *shared* [`super`] helper — lives
/// here as data, so each referenced kind is enumerable (a MAP key) for the increment-5
/// conformance gate. The runtime dispatch reads this exact table via [`super::lookup`] +
/// [`super::dispatch`]; the irreducible Rust-local quirks the table can't express stay
/// hand-written residue (see [`RESIDUE_KINDS`]).
pub(crate) const MAP: &[(&str, super::Lowering)] = &[
    ("function_item", L::Function),
    ("block", L::Block),
    ("loop_expression", L::Loop),
    ("while_expression", L::While),
    ("for_expression", L::For("pattern", "value")),
    ("if_expression", L::If),
    ("unary_expression", L::UnaryPositional),
    ("return_expression", L::Return),
    ("call_expression", L::Call("function", "arguments")),
    ("field_expression", L::Field("value", "field")),
    ("binary_expression", L::Binary("left", "operator", "right")),
    (
        "compound_assignment_expr",
        L::AugAssign("left", "operator", "right", false),
    ),
    ("break_expression", L::Leaf(kind::BREAK)),
    ("continue_expression", L::Leaf(kind::CONTINUE)),
    ("integer_literal", L::Lit(Bucket::Int)),
    ("float_literal", L::Lit(Bucket::Float)),
    ("string_literal", L::Lit(Bucket::Str)),
    ("raw_string_literal", L::Lit(Bucket::Str)),
    ("char_literal", L::Lit(Bucket::Char)),
    ("boolean_literal", L::Lit(Bucket::Bool)),
    ("identifier", L::Var),
    // Field/type/primitive names are external by structure (§5.2.4 exception).
    ("field_identifier", L::ExtName),
    ("type_identifier", L::ExtName),
    ("primitive_type", L::ExtName),
    ("shorthand_field_identifier", L::ExtName),
    ("expression_statement", L::Unwrap),
    // Binding patterns that merely wrap a name (`mut x`, `&x`): unwrap to the name.
    ("mut_pattern", L::Unwrap),
    ("reference_pattern", L::Unwrap),
    ("parenthesized_expression", L::DropParens),
    // Comments, attributes, lifetimes and the bare `mut` specifier carry no similarity
    // signal — dropped (spec §5.2.1/§5.2.7; Type-1 convergence).
    ("line_comment", L::Drop),
    ("block_comment", L::Drop),
    ("attribute_item", L::Drop),
    ("inner_attribute_item", L::Drop),
    ("mutable_specifier", L::Drop),
    ("lifetime", L::Drop),
];

/// Tree-sitter kinds the hand-written Rust residue references but the shared [`MAP`] does
/// not — registered as data so the increment-5 conformance gate can enumerate EVERY kind the
/// frontend references (MAP keys ∪ `RESIDUE_KINDS`) and assert each still exists in the linked
/// grammar. The residue is the irreducible per-language quirks the table can't express:
/// subject-folded `match` ([`lower_match`]), tuple-assign ([`lower_assignment_rust`]), the
/// `let` binding rule ([`lower_let_rust`]), and the non-uniform inline lowerings
/// (`index`/`reference`/`range`/`closure`) — plus the tail-return path ([`Rust::lower_fn_body`]
/// → [`lower_tail`]/[`return_tail`]) and the deeper discriminants those helpers test, and the
/// parameter kind [`Rust::lower_params`] matches on (another hand-written, non-`lower_node` path).
///
/// One residue reference is a *suffix* match, not a fixed kind: [`return_tail`] treats any
/// `*_item` node as a non-value. That is enumerated structurally by the gate, not as a string.
// Consumed by the increment-5 conformance gate (via `super::frontend_table`), which asserts every
// entry still exists in the linked grammar — so the increment-1 `allow(dead_code)` that scoped this
// to the plain library build is gone.
pub(crate) const RESIDUE_KINDS: &[&str] = &[
    // Dispatched from `lower_node`'s residue arm (a table miss → a Rust-local lowering).
    "match_expression",      // subject-fold → Branch (lower_match)
    "reference_expression",  // &x / &mut x → Unop (lower_reference)
    "range_expression",      // a..b / a..=b → Binop (lower_range)
    "closure_expression",    // |x| e → Lambda (lower_closure)
    "index_expression",      // xs[i] → Index (inline make_index)
    "let_declaration",       // let PAT = v (lower_let_rust)
    "assignment_expression", // x = v / (a, b) = (X, Y) (lower_assignment_rust)
    // Referenced only by the tail-return residue (return_tail's non-value guards).
    "use_declaration",
    "empty_statement",
    "macro_definition",
    // Internal discriminants of the residue helpers (still grammar-coupled strings).
    "match_arm",        // lower_match arm iteration
    "match_pattern",    // literal_pattern_value
    "negative_literal", // literal_pattern_value (a literal match arm)
    "tuple_expression", // lower_assignment_rust parallel-assign detection
    // Referenced by the hand-written `lower_params` (not `lower_node`): the parameter node kind it
    // matches on. Registered so a grammar rename of this kind trips the gate instead of silently
    // routing params to native(). (Its `pattern` field is already gated via the `For` entry.)
    "parameter", // lower_params: a `pat: T` function parameter
];

impl Frontend for Rust {
    fn lower_node_inner(
        &self,
        node: Node,
        field: Option<&str>,
        src: &str,
        log: &mut TransformLog,
    ) -> Option<NormNode> {
        // The uniform arms are data (`MAP`): look the kind up, dispatch to its shared helper.
        // A hit that returns `None` is a deliberately-dropped node (`Lowering::Drop`, or an
        // `Unwrap`/`DropParens` that found nothing) — NOT a miss, so it never reaches residue.
        if let Some(entry) = super::lookup(MAP, node.kind()) {
            return super::dispatch(entry, self, node, field, src, log);
        }
        // Table miss → the Rust-local residue (kinds registered in `RESIDUE_KINDS`): the
        // irreducible quirks the shared table can't express — subject-folded `match`,
        // tuple-`=`, the `let` binding rule — plus the non-uniform inline lowerings. Anything
        // else is an unmodeled `native` leaf.
        let span = (node.start_byte() as u32, node.end_byte() as u32);
        match node.kind() {
            "match_expression" => Some(lower_match(self, node, field, span, src, log, false)),
            "reference_expression" => Some(lower_reference(self, node, field, span, src, log)),
            "range_expression" => Some(lower_range(self, node, field, span, src, log)),
            "closure_expression" => Some(lower_closure(self, node, field, span, src, log)),
            "index_expression" => {
                let base = node
                    .named_child(0)
                    .and_then(|n| self.lower_node(n, Some("base"), src, log));
                let idx = node
                    .named_child(1)
                    .and_then(|n| self.lower_node(n, Some("idx"), src, log));
                Some(make_index(base, idx, field, span))
            }
            // `let x = …` binds; `x = …` (`assignment_expression`) mutates.
            "let_declaration" => Some(lower_let_rust(self, node, field, span, src, log)),
            "assignment_expression" => {
                Some(lower_assignment_rust(self, node, field, span, src, log))
            }
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

    /// A Rust function body's **tail expression** is an implicit `return` (`fn f() -> T { …
    /// expr }` ≡ `{ … return expr; }`). Lower the body block in return position so that tail
    /// expression becomes an explicit `Return` — the only override of the default (Python/Go
    /// require an explicit `return`, so they keep it). See [`lower_return_block`].
    fn lower_fn_body(
        &self,
        node: Node,
        span: (u32, u32),
        src: &str,
        log: &mut TransformLog,
    ) -> NormNode {
        match node.child_by_field_name("body") {
            Some(body) if body.kind() == "block" => lower_return_block(self, body, span, src, log),
            Some(body) => self
                .lower_node(body, Some("body"), src, log)
                .unwrap_or_else(|| NormNode::new(kind::BLOCK, Some("body"), span, Vec::new())),
            None => NormNode::new(kind::BLOCK, Some("body"), span, Vec::new()),
        }
    }
}

/// Lower a Rust `block` that sits in the function's **return position**: every statement
/// lowers normally, but the block's tail expression — its implicit return — becomes an
/// explicit `Return`, so `fn f() -> T { … expr }` converges with `{ … return expr; }`. Only
/// the tail child, identified from the CST (never a `;`-terminated statement, a `let x = { …
/// }` value block, or a nested item), is wrapped; the transform recurses through tail-position
/// `if`/`match` arms and nested `block`s ([`lower_tail`]).
fn lower_return_block(
    fe: &Rust,
    block: Node,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let mut cursor = block.walk();
    let named: Vec<Node> = block.named_children(&mut cursor).collect();
    let n = named.len();
    let mut children = Vec::new();
    for (i, c) in named.into_iter().enumerate() {
        if i + 1 == n
            && let Some(tail) = return_tail(c)
        {
            if let Some(node) = lower_tail(fe, tail, src, log) {
                children.push(node);
            }
        } else if let Some(node) = fe.lower_node(c, None, src, log) {
            children.push(node);
        }
    }
    NormNode::new(kind::BLOCK, Some("body"), span, children)
}

/// The tail (implicit-return) expression of a block, given its **last named child** — or
/// `None` when the block ends in a statement (no implicit return). A tail `if`/`match`/`block`
/// is wrapped by tree-sitter in `expression_statement` WITHOUT a trailing `;` (a `;` after a
/// block-expression becomes a sibling `empty_statement`, so a `;`-terminated one would not be
/// the last named child) — those unwrap to a tail; a `;`-terminated `expression_statement`, a
/// `let`, a nested item (`*_item`), or an attribute/comment is not a return value.
fn return_tail(child: Node) -> Option<Node> {
    let k = child.kind();
    match k {
        "expression_statement" => {
            let inner = child.named_child(0)?;
            matches!(inner.kind(), "if_expression" | "match_expression" | "block").then_some(inner)
        }
        "let_declaration" | "use_declaration" | "empty_statement" | "line_comment"
        | "block_comment" | "macro_definition" => None,
        _ if k.ends_with("_item") => None, // a nested item (fn/struct/impl/attribute), not a value
        _ => Some(child),
    }
}

/// Lower a Rust expression in the function's **return position** — its value is the function's
/// implicit return. A value expression is wrapped in `Return`; a tail `if`/`match` recurses so
/// each arm's own tail is wrapped (`{ if c { a } else { b } }` ≡ `{ if c { return a } else {
/// return b } }`); a nested `block` recurses into its tail. An `if` without `else` (unit-valued),
/// an explicit `return`/`break`/`continue`, or a loop is already control flow / not a value —
/// lowered as an ordinary statement, no synthetic `Return`.
fn lower_tail(fe: &Rust, node: Node, src: &str, log: &mut TransformLog) -> Option<NormNode> {
    let span = (node.start_byte() as u32, node.end_byte() as u32);
    match node.kind() {
        "block" => Some(lower_return_block(fe, node, span, src, log)),
        "if_expression" if node.child_by_field_name("alternative").is_some() => {
            Some(lower_if_tail(fe, node, span, src, log))
        }
        "match_expression" => Some(lower_match(fe, node, None, span, src, log, true)),
        "if_expression"
        | "return_expression"
        | "break_expression"
        | "continue_expression"
        | "loop_expression"
        | "while_expression"
        | "for_expression" => fe.lower_node(node, None, src, log),
        _ => {
            let v = fe.lower_node(node, Some("value"), src, log)?;
            Some(NormNode::new(kind::RETURN, None, span, vec![v]))
        }
    }
}

/// [`super::lower_if`] with each arm body lowered in **return position** ([`lower_tail`]), for a
/// tail `if/else` (or `else if` chain) whose value is the function's return. Only reached with an
/// `else` present (an else-less `if` is unit-valued — see [`lower_tail`]).
fn lower_if_tail(
    fe: &Rust,
    node: Node,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let guard = node
        .child_by_field_name("condition")
        .and_then(|c| fe.lower_node(c, Some("guard"), src, log));
    let body = node
        .child_by_field_name("consequence")
        .and_then(|c| lower_tail(fe, c, src, log))
        .map(|b| block_wrap(b, span));
    let mut arms = vec![make_arm(guard, body, span)];
    if let Some(alt) = node.child_by_field_name("alternative") {
        // `else_clause` wraps a `block` (plain `else`) or an `if_expression` (`else if`); both
        // lower in return position — a nested `if` recurses via `lower_tail`, spliced flat.
        let mut c = alt.walk();
        let else_body = alt
            .named_children(&mut c)
            .find_map(|n| lower_tail(fe, n, src, log));
        push_else_arm(&mut arms, else_body);
    }
    NormNode::new(kind::BRANCH, None, span, arms)
}

/// Lower a Rust `let PAT = value` binding. A destructuring pattern (`let (a, b) = …`) binds
/// every identifier it names, so those idents are declared locals — mark them `@target` so a
/// renamed capture abstracts, matching the tuple `for`-loop element bind that lets a tuple `for`
/// and a tuple index loop converge. A simple `let x` already gets `@target` from `lower_assign`.
fn lower_let_rust(
    fe: &Rust,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let mut assign = lower_assign(fe, node, field, span, ("pattern", "value", true), src, log);
    if let Some(target) = assign.children.first_mut()
        && target.field.as_deref() == Some("target")
        && target.kind.as_ref() != kind::VAR
    {
        bind_pattern_idents(target);
    }
    assign
}

/// Lower a Rust `assignment_expression`. A **parallel** tuple assignment (`(a, b) = (X, Y)`)
/// becomes a canonical multi-target `Assign` (byte-identical to Go's multi-`=`), which the shared
/// `detect_multi_assign` pass then sequentializes with minimal temps — so `(a, b) = (b, a % b)`
/// converges with the temped adjacent assigns and with tail-rec reassignment. A plain `x = e`
/// keeps the existing single mutation (`@place`) path.
fn lower_assignment_rust(
    fe: &Rust,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    if let (Some(left), Some(right)) = (
        node.child_by_field_name("left"),
        node.child_by_field_name("right"),
    ) && left.kind() == "tuple_expression"
        && right.kind() == "tuple_expression"
    {
        let mut lc = left.walk();
        let tgts: Vec<Node> = left.named_children(&mut lc).collect();
        let mut rc = right.walk();
        let vals: Vec<Node> = right.named_children(&mut rc).collect();
        if tgts.len() >= 2 && tgts.len() == vals.len() {
            // A Rust `=` is always a mutation of existing places → `@place` targets.
            let targets: Vec<NormNode> = tgts
                .iter()
                .filter_map(|t| fe.lower_node(*t, Some("place"), src, log))
                .collect();
            let values: Vec<NormNode> = vals
                .iter()
                .filter_map(|v| fe.lower_node(*v, Some("value"), src, log))
                .collect();
            if targets.len() == vals.len() && values.len() == vals.len() {
                return make_multi_assign(targets, values, field, span);
            }
        }
    }
    lower_assign(fe, node, field, span, ("left", "right", false), src, log)
}

/// `match x { pat => body, … }` → `Branch{ Arm[guard, body]… }` (§14) with the subject
/// folded into each guard: a **literal** arm `0 =>` becomes `x == 0` (so it converges with
/// `if x == 0`); a **pattern** arm `Some(y) =>` becomes `matches(x, Some(y))`; `_` is the
/// trivial-guard (else) arm. Captures become declared locals.
fn lower_match(
    fe: &Rust,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
    // `tail`: the match is in the function's return position, so each arm's value lowers in
    // return position (its tail becomes an explicit `Return` — see [`lower_tail`]).
    tail: bool,
) -> NormNode {
    let subject = node
        .child_by_field_name("value")
        .and_then(|v| fe.lower_node(v, None, src, log));
    let mut arms = Vec::new();
    if let Some(body) = node.child_by_field_name("body") {
        let mut c = body.walk();
        for arm in body.named_children(&mut c) {
            if arm.kind() != "match_arm" {
                continue;
            }
            let pat = arm.child_by_field_name("pattern");
            let arm_body = arm
                .child_by_field_name("value")
                .and_then(|v| {
                    if tail {
                        lower_tail(fe, v, src, log)
                    } else {
                        fe.lower_node(v, Some("body"), src, log)
                    }
                })
                .map(|b| block_wrap(b, span));
            let guard = pat.and_then(|p| {
                case_guard(
                    fe,
                    subject.as_ref(),
                    p,
                    is_wildcard(p, src),
                    literal_pattern_value(p),
                    span,
                    src,
                    log,
                )
            });
            arms.push(make_arm(guard, arm_body, span));
        }
    }
    make_branch(arms, field, span)
}

/// A `_` (catch-all) pattern → the trivial-guard arm.
fn is_wildcard(pat: Node, src: &str) -> bool {
    pat.utf8_text(src.as_bytes())
        .map(|t| t.trim() == "_")
        .unwrap_or(false)
}

/// The bare literal node of a literal `match_pattern` (`0`, `"s"`, `true`), else `None`
/// (a structural/binding pattern). A guard/or-pattern (`n if …`, `1 | 2`) is not a plain
/// literal → matched via `matches(…)`.
fn literal_pattern_value(pat: Node) -> Option<Node> {
    if pat.kind() != "match_pattern" || pat.named_child_count() != 1 {
        return None;
    }
    let inner = pat.named_child(0)?;
    matches!(
        inner.kind(),
        "integer_literal"
            | "float_literal"
            | "string_literal"
            | "raw_string_literal"
            | "char_literal"
            | "boolean_literal"
            | "negative_literal"
    )
    .then_some(inner)
}

/// `&x` / `&mut x` → `Unop{ &, x }` (the `mut` specifier is ignored — borrows are kept
/// as a distinct construct, matching the historical `reference_expression` node).
fn lower_reference(
    fe: &Rust,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let op = NormNode::new("&", Some("op"), span, Vec::new());
    let operand = node
        .child_by_field_name("value")
        .and_then(|n| fe.lower_node(n, Some("operand"), src, log));
    NormNode::new(
        kind::UNOP,
        field,
        span,
        std::iter::once(op).chain(operand).collect(),
    )
}

/// `a..b` / `a..=b` → `Binop{ a, .., b }` (positional; the `..`/`..=` token is the op).
/// A canonical range keeps `0..xs.len()` visible for the iteration-protocol rewrite.
fn lower_range(
    fe: &Rust,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let mut children = Vec::new();
    let mut first = true;
    let mut cursor = node.walk();
    for c in node.children(&mut cursor) {
        if c.is_named() {
            let f = if first { "left" } else { "right" };
            if let Some(n) = fe.lower_node(c, Some(f), src, log) {
                children.push(n);
                first = false;
            }
        } else {
            let op_span = (c.start_byte() as u32, c.end_byte() as u32);
            children.push(NormNode::new(
                c.utf8_text(src.as_bytes()).unwrap_or(".."),
                Some("op"),
                op_span,
                Vec::new(),
            ));
        }
    }
    NormNode::new(kind::BINOP, field, span, children)
}

/// `|x| body` → `Lambda{ x@param, body }` (§5 `Lambda`).
fn lower_closure(
    fe: &Rust,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let mut params = Vec::new();
    if let Some(pl) = node.child_by_field_name("parameters") {
        let mut c = pl.walk();
        for p in pl.named_children(&mut c) {
            if p.kind() == "identifier"
                && let Some(v) = fe.lower_node(p, Some("param"), src, log)
            {
                params.push(v);
            }
        }
    }
    let body = node
        .child_by_field_name("body")
        .and_then(|b| fe.lower_node(b, Some("body"), src, log));
    make_lambda(params, body, field, span)
}

#[cfg(test)]
mod tests {
    use super::lower_rust_source;
    use crate::ir::render::to_sexpr;
    use crate::ir::transform::{TransformKind, TransformLog, Witness};

    fn rust(src: &str) -> (crate::tree::NormNode, TransformLog) {
        lower_rust_source(src).expect("a rust function")
    }

    fn abs(src: &str) -> String {
        let (ir, _) = rust(src);
        to_sexpr(&crate::ir::abstract_idents(ir))
    }

    #[test]
    fn dispatch_table_is_a_consistent_single_source_of_truth() {
        // The table is the SP4 gate's source of truth (D-SP4-1b), so every referenced kind
        // must be data in EXACTLY one place: a unique MAP key OR a residue kind, never both.
        use std::collections::HashSet;
        let mut keys = HashSet::new();
        for (k, _) in super::MAP {
            assert!(keys.insert(*k), "duplicate MAP key: {k}");
        }
        let mut residue = HashSet::new();
        for k in super::RESIDUE_KINDS {
            assert!(residue.insert(*k), "duplicate residue kind: {k}");
            assert!(!keys.contains(k), "kind {k} is both a MAP key and residue");
        }
    }

    #[test]
    fn let_binds_but_assignment_mutates() {
        // `let x` declares (`@target` → local); `x = a` mutates an existing place.
        let s = abs("fn f(a: i32) { let mut x = a; x = a; }");
        assert!(
            s.contains("(Assign (Var@target v1)"),
            "let not a binding: {s}"
        );
        assert!(
            s.contains("(Assign (Var@place v1)"),
            "reassign not a place: {s}"
        );
    }

    #[test]
    fn mut_pattern_unwraps_to_the_plain_name() {
        // `let mut x = 0` — the `mut` specifier is stripped, leaving `x` as the target.
        let s = abs("fn f() { let mut x = 9; g(x); }");
        assert!(
            s.contains("(Assign (Var@target v0) (Lit@value INT))"),
            "{s}"
        );
        assert!(!s.contains("mutable_specifier"), "mut not stripped: {s}");
    }

    #[test]
    fn compound_assignment_desugars() {
        // `x += a` ≡ `x = x + a`.
        assert_eq!(
            abs("fn f(a: i32) { let mut x = a; x += a; }"),
            abs("fn g(a: i32) { let mut x = a; x = x + a; }")
        );
    }

    #[test]
    fn reference_and_closure_and_range_lower_to_canonical_nodes() {
        let s = abs("fn f(a: i32) { let r = &a; let c = |n| n + a; let z = 0..a; }");
        assert!(s.contains("(Unop@value (&@op)"), "reference: {s}");
        assert!(s.contains("(Lambda@value (Var@param"), "closure: {s}");
        assert!(
            s.contains("(Binop@value") && s.contains("(..@op)"),
            "range: {s}"
        );
        assert!(
            !s.contains("NativeStmt") && !s.contains("NativeExpr"),
            "{s}"
        );
    }

    fn canon(src: &str) -> String {
        let (ir, _) = rust(src);
        let tree = crate::ir::abstract_idents(ir);
        let edits = crate::ir::detect_comm_sort(&tree);
        to_sexpr(&crate::ir::apply(
            tree,
            &edits,
            &mut crate::ir::TransformLog::disabled(),
        ))
    }

    #[test]
    fn value_match_converges_with_the_equivalent_if_chain() {
        // §14: `match a { 0 => …, 1 => …, _ => … }` ≡ `if a==0 {…} else if a==1 {…} else {…}`.
        // The subject folds into each guard (`a == 0`) and the if-chain flattens — so they
        // are one and the same `Branch`. This is the convergence the subject-element blocked.
        assert_eq!(
            canon("fn f(a: i32) -> i32 { match a { 0 => g(), 1 => h(), _ => k() } }"),
            canon("fn f(a: i32) -> i32 { if a == 0 { g() } else if a == 1 { h() } else { k() } }")
        );
    }

    #[test]
    fn value_match_guard_is_a_subject_equality() {
        let s = abs("fn f(a: i32) -> i32 { match a { 0 => g(), _ => k() } }");
        assert!(
            !s.contains("subject"),
            "subject must fold into the guard: {s}"
        );
        // guard = `a == 0` → Binop{ Var@left, ==@op, Lit@right 0 }.
        assert!(
            s.contains("(Binop@guard (Var@left v0) (==@op) (Lit@right 0))"),
            "{s}"
        );
    }

    #[test]
    fn pattern_match_does_not_converge_with_an_equality_if_chain() {
        // A structural pattern is `matches(subj, pat)`, not an equality — must stay distinct.
        let pat = canon("fn f(a: i32) -> i32 { match a { Some(x) => x, _ => 0 } }");
        let iff = canon("fn f(a: i32) -> i32 { if a == 0 { 0 } else { 0 } }");
        assert_ne!(pat, iff);
        assert!(pat.contains("(Var@callee matches)"), "{pat}");
    }

    #[test]
    fn match_capture_rename_converges() {
        // Renamed captures must converge (the reason pattern binds are collected).
        assert_eq!(
            abs("fn f(a: i32) -> i32 { match a { Some(x) => x, _ => 0 } }"),
            abs("fn g(a: i32) -> i32 { match a { Some(y) => y, _ => 0 } }")
        );
    }

    #[test]
    fn attributes_are_stripped_from_the_body() {
        let s = abs("fn f() { #[allow(unused)] let x = 9; g(x); }");
        assert!(!s.contains("attribute_item"), "attribute not stripped: {s}");
    }

    #[test]
    fn tail_expression_is_a_return_but_a_value_block_is_not() {
        // Implicit-return modeling (§5.2): the function-body tail expression becomes an explicit
        // `Return` — but a `let x = { … }` VALUE block is not in return position, so its own tail
        // must stay a plain expression. Only the function's return position is wrapped (the
        // scoping guard: block-tail→Return must not over-reach into value blocks).
        let s = abs("fn f() -> i32 { let x = { h() }; g(x) }");
        assert!(
            s.contains("(Block@value (Call (Var@callee h)))"),
            "a `let`-value block tail was wrongly wrapped in Return: {s}"
        );
        assert!(
            s.contains("(Return (Call@value (Var@callee g) (Var@arg v0)))"),
            "the function tail expression was not wrapped in Return: {s}"
        );
        assert_eq!(
            s.matches("Return").count(),
            1,
            "expected exactly one Return (the function tail only): {s}"
        );
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
    fn tail_recursion_becomes_a_loop() {
        let (ir, log) = rust("fn go(n: i32) { if n == 0 { return; } go(n - 1); }");
        let s = to_sexpr(&ir);
        assert!(s.contains("(Loop"), "no loop: {s}");
        assert!(s.contains("Continue"), "no continue: {s}");
        assert!(
            log.events()
                .iter()
                .any(|e| e.kind == TransformKind::RecursionLower)
        );
    }

    #[test]
    fn tree_recursion_does_not_lower() {
        // Two self-calls, neither a tail site → must NOT become a loop.
        let (ir, log) = rust("fn fib(n: i32) { return fib(n - 1) + fib(n - 2); }");
        assert!(!to_sexpr(&ir).contains("(Loop"));
        assert!(
            !log.events()
                .iter()
                .any(|e| e.kind == TransformKind::RecursionLower)
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
        let (ir1, log1) = rust("fn f() { x(5); }");
        let (ir2, log2) = rust("fn g() { x(7); }");
        assert_eq!(to_sexpr(&ir1), to_sexpr(&ir2));
        let value = |log: &TransformLog| {
            log.events().iter().find_map(|e| match &e.witness {
                Witness::Literal(t) => Some(t.to_string()),
                _ => None,
            })
        };
        assert_eq!(value(&log1).as_deref(), Some("5"));
        assert_eq!(value(&log2).as_deref(), Some("7"));
    }
}
