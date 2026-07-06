//! Go frontend: tree-sitter `function_declaration` / `method_declaration` CST →
//! canonical IR. A *mapping* only — every synthesis and the recursion/loop algorithms
//! are shared in [`super`], so a Go `for … range` lowers to the **same** iterated
//! `Loop` a Rust/Python `for` does. Contrast the historical `src/lang/go.rs` (the full
//! per-grammar normalizer): this re-implements *no* algorithm.
//!
//! Go quirks handled here: block bodies wrap a `statement_list` (spliced away, D23);
//! `for` has four surface forms (infinite / condition-only / `range` / C-style
//! three-clause, the last hoisting its `init` to the enclosing block via
//! [`Frontend::expand_stmt`]); assignments wrap both sides in `expression_list`, and
//! `:=`/`var`/`const` *declare* (binding `@target`) where `=` *mutates* (`@place`).

use super::Lowering as L;
use super::{
    Frontend, block, break_guard, call_ext, eq_guard, lower_aug_assign, lower_body, lower_source,
    lower_unit, make_arm, make_branch, make_index, matches_guard, native, or_chain, push_else_arm,
};
use crate::ir::kind;
use crate::ir::transform::{TransformKind, TransformLog, Witness};
use crate::lang::Lang;
use crate::tree::{Bucket, Label, NormNode};
use tree_sitter::Node;

/// Lower a Go `function_declaration`/`method_declaration` CST node to canonical IR
/// (an enabled sink — the convenience for callers that want the event stream, e.g. tests;
/// the bulk path threads its own [`TransformLog::disabled`] via [`super::normalize`]).
pub fn lower_go(node: Node, src: &str) -> (NormNode, TransformLog) {
    let mut log = TransformLog::new();
    let tree = lower_unit(&Go, node, src, &mut log);
    (tree, log)
}

/// Convenience: parse Go `src` and lower its first `function_declaration`.
pub fn lower_go_source(src: &str) -> Option<(NormNode, TransformLog)> {
    let mut log = TransformLog::new();
    let tree = lower_source(&Go, Lang::Go, "function_declaration", src, &mut log)?;
    Some((tree, log))
}

pub(crate) struct Go;

fn span_of(node: Node) -> (u32, u32) {
    (node.start_byte() as u32, node.end_byte() as u32)
}

/// The data-driven Go dispatch table (D-SP4-2; `docs/sp4-grammar-lift-plan.md` §4), following the
/// Python increment-2 pattern (uniform arms as data; language-local lowerings as [`super::Lowering::Std`]
/// fn-pointers). Every **uniform** arm — a CST kind handled by a *shared* [`super`] helper — is data;
/// Go's **language-local** lowerings (`lower_for_stmt`, `lower_if_go`, `lower_switch`, `lower_return_go`,
/// `lower_inc_dec`, `lower_assignment_stmt`, and the `:=` adapter [`lower_short_var_decl`]) ride in the
/// table as `Std` fn-pointers rather than a hand-written residue list. The runtime dispatch reads this
/// exact table via [`super::lookup`] + [`super::dispatch`]; only the genuinely irreducible quirks the
/// table can't express — the inline `index_expression` construction and the single-element
/// `expression_list` unwrap guard — stay residue (see [`RESIDUE_KINDS`]).
pub(crate) const MAP: &[(&str, super::Lowering)] = &[
    ("function_declaration", L::Function),
    ("method_declaration", L::Function),
    ("block", L::Block),
    // Language-local lowerings — a shared node-set, a per-language shape — ride in the table as `Std`.
    ("for_statement", L::Std(lower_for_stmt)),
    ("if_statement", L::Std(lower_if_go)),
    ("expression_switch_statement", L::Std(lower_switch)),
    ("type_switch_statement", L::Std(lower_switch)),
    ("return_statement", L::Std(lower_return_go)),
    // `:=` declares (binding `@target`) — the shared `lower_assign_go` with `bind = true` pinned.
    ("short_var_declaration", L::Std(lower_short_var_decl)),
    ("assignment_statement", L::Std(lower_assignment_stmt)),
    ("inc_statement", L::Std(lower_inc_dec)),
    ("dec_statement", L::Std(lower_inc_dec)),
    ("expression_statement", L::Unwrap),
    ("break_statement", L::Leaf(kind::BREAK)),
    ("continue_statement", L::Leaf(kind::CONTINUE)),
    ("call_expression", L::Call("function", "arguments")),
    ("binary_expression", L::Binary("left", "operator", "right")),
    ("unary_expression", L::UnaryPositional),
    ("selector_expression", L::Field("operand", "field")),
    ("parenthesized_expression", L::DropParens),
    ("int_literal", L::Lit(Bucket::Int)),
    ("float_literal", L::Lit(Bucket::Float)),
    ("imaginary_literal", L::Lit(Bucket::Float)),
    ("interpreted_string_literal", L::Lit(Bucket::Str)),
    ("raw_string_literal", L::Lit(Bucket::Str)),
    ("rune_literal", L::Lit(Bucket::Char)),
    ("true", L::Lit(Bucket::Bool)),
    ("false", L::Lit(Bucket::Bool)),
    ("identifier", L::Var),
    // Field/type/package names can never be a declared local (§5.2.4 exception).
    ("field_identifier", L::ExtName),
    ("type_identifier", L::ExtName),
    ("package_identifier", L::ExtName),
    // No similarity signal — dropped (Type-1 convergence / spec §5.2.7), as Rust's comments do.
    ("comment", L::Drop),
];

/// Tree-sitter kinds the Go frontend references but the shared [`MAP`] does not key on — registered as
/// data so the increment-5 conformance gate can enumerate EVERY kind the frontend references
/// (MAP keys ∪ `RESIDUE_KINDS`) and assert each still exists in the linked grammar. Per the generalized
/// rule (Python increment 2): every grammar-kind the frontend references that is NOT a MAP key —
/// residue-arm kinds ∪ the internal discriminants of the `Std` helpers, plus the grammar kinds the
/// non-`lower_node` frontend paths (`splice_kind`, `expand_stmt`) still couple to (the analog of Rust's
/// tail-return-only kinds `use_declaration`/`empty_statement`/`macro_definition`).
///
/// (The one referenced grammar kind deliberately left out is `parameter_declaration`, matched inside
/// [`Frontend::lower_params`] — the same param-shape gate gap Rust (`parameter`) and Python
/// (`typed_parameter`/`default_parameter`) leave open; tracked as cross-frontend deferred work.)
// Consumed by the increment-5 conformance gate (via `super::frontend_table`), which asserts every
// entry still exists in the linked grammar — so the increment-3 `allow(dead_code)` that scoped this
// to the plain library build is gone (parity with Rust's/Python's `RESIDUE_KINDS`).
pub(crate) const RESIDUE_KINDS: &[&str] = &[
    // Dispatched from `lower_node`'s residue arm (a table miss → an inline Go-local lowering).
    "index_expression", // xs[i] → Index (inline make_index)
    "expression_list",  // single-element unwrap guard; also lower_return_go's multi-value wrapper
    // Internal discriminants of the `Std` language-local helpers (not MAP keys, still grammar-coupled).
    "range_clause",    // lower_for_stmt: the `for … range` form
    "for_clause",      // lower_for_stmt / expand_stmt: the C-style three-clause form
    "expression_case", // lower_switch: a value/condition case arm
    "type_case",       // lower_switch: a `case T:` type-switch arm
    "default_case",    // lower_switch: the trivial-guard (else) arm
    "statement_list",  // splice_kind (block wrapper) / case_body (switch arm body)
    // Referenced only by `expand_stmt` (grouped `var (…)`/`const (…)` decls, spliced into the block).
    "var_declaration",
    "const_declaration",
    "var_spec",
    "const_spec",
];

impl Frontend for Go {
    fn lower_node(
        &self,
        node: Node,
        field: Option<&str>,
        src: &str,
        log: &mut TransformLog,
    ) -> Option<NormNode> {
        // The uniform arms AND Go's language-local lowerings are data (`MAP`): look the kind up,
        // dispatch it (shared helper or `Std` fn-ptr). A hit returning `None` is a deliberate drop
        // (`Lowering::Drop`, or an `Unwrap`/`DropParens` that found nothing) — NOT a miss, so it
        // never reaches residue.
        if let Some(entry) = super::lookup(MAP, node.kind()) {
            return super::dispatch(entry, self, node, field, src, log);
        }
        // Table miss → the Go-local residue (kinds registered in `RESIDUE_KINDS`): the two
        // irreducible inline quirks — `index_expression`'s field-based `Index` construction and the
        // single-element `expression_list` unwrap — else an unmodeled `native` leaf.
        let span = span_of(node);
        match node.kind() {
            "index_expression" => {
                let base = node
                    .child_by_field_name("operand")
                    .and_then(|n| self.lower_node(n, Some("base"), src, log));
                let idx = node
                    .child_by_field_name("index")
                    .and_then(|n| self.lower_node(n, Some("idx"), src, log));
                Some(make_index(base, idx, field, span))
            }
            // A grammar wrapper only: `assignment_statement`/`return` operands are wrapped
            // in an `expression_list`. A single-element list is transparent — unwrap it so
            // `lower_aug_assign` (B2) sees Go's `left`/`right` operand directly, at parity with
            // the single-expression `left`/`right` Rust/Python `op=` nodes carry. Multi-element
            // lists (tuple assignment) keep the native shape.
            "expression_list" if node.named_child_count() == 1 => node
                .named_child(0)
                .and_then(|inner| self.lower_node(inner, field, src, log)),
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
            if p.kind() != "parameter_declaration" {
                continue; // variadic / unnamed: skip (tail-recursion guard then bails)
            }
            let mut nc = p.walk();
            for name in p.children_by_field_name("name", &mut nc) {
                if let Some(v) = self.lower_node(name, Some("param"), src, log) {
                    out.push(v);
                }
            }
        }
    }

    fn splice_kind(&self, kind: &str) -> bool {
        kind == "statement_list"
    }

    fn expand_stmt(&self, node: Node, src: &str, log: &mut TransformLog) -> Option<Vec<NormNode>> {
        match node.kind() {
            // Grouped `var (…)` / `const (…)`: one Assign per spec, spliced in.
            "var_declaration" | "const_declaration" => {
                let mut out = Vec::new();
                let mut cursor = node.walk();
                for spec in node.named_children(&mut cursor) {
                    if matches!(spec.kind(), "var_spec" | "const_spec") {
                        out.push(lower_spec(self, spec, src, log));
                    }
                }
                Some(out)
            }
            // C-style `for init; cond; post {…}`: the `init` must become a sibling
            // *before* the `Loop` (mirrors the historical `expand_for_clauses`).
            "for_statement" => {
                let clause = node
                    .child_by_field_name("body")
                    .and_then(|_| find_named_clause(node))?;
                if clause.kind() != "for_clause" {
                    return None;
                }
                let init = clause.child_by_field_name("initializer")?;
                let span = span_of(node);
                let mut out = Vec::new();
                if let Some(n) = self.lower_node(init, None, src, log) {
                    out.push(n);
                }
                out.push(lower_for_stmt(self, node, None, span, src, log));
                Some(out)
            }
            _ => None,
        }
    }
}

/// The named child of a `for_statement` that is the loop *clause* (`for_clause` /
/// `range_clause` / a bare condition expression) — everything but the `body`.
fn find_named_clause(node: Node) -> Option<Node> {
    let body_id = node.child_by_field_name("body").map(|b| b.id());
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|c| Some(c.id()) != body_id)
}

/// Lower any `for` form to the canonical infinite `Loop` core (§5.2.2). The C-style
/// three-clause form's `init` is hoisted by [`Frontend::expand_stmt`]; here we build
/// the loop body (break-guard from the condition, `update` appended) for every form.
fn lower_for_stmt(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let mut body = lower_body(fe, node, span, src, log);
    let form = match find_named_clause(node) {
        None => "infinite",
        Some(c) => match c.kind() {
            "range_clause" => {
                lower_range_into(fe, c, &mut body, span, src, log);
                "range"
            }
            "for_clause" => {
                if let Some(update) = c.child_by_field_name("update")
                    && let Some(u) = fe.lower_node(update, None, src, log)
                {
                    body.children.push(u);
                }
                if let Some(cond) = c.child_by_field_name("condition")
                    && let Some(cn) = fe.lower_node(cond, None, src, log)
                {
                    body.children.insert(0, break_guard(cn, span));
                }
                "for"
            }
            // A bare expression clause is the condition of a `for cond {…}` (while-form).
            _ => {
                if let Some(cn) = fe.lower_node(c, None, src, log) {
                    body.children.insert(0, break_guard(cn, span));
                }
                "while"
            }
        },
    };
    if form != "infinite" {
        // `for {}` is already the core (no transform); the other forms fold to it.
        log.record(
            TransformKind::LoopLower,
            span,
            Witness::LoopForm(form.into()),
        );
    }
    NormNode::new(kind::LOOP, field, span, vec![body])
}

/// `for left := range xs { … }` → prepend `if !__has_next(xs) { break }` and
/// `left = __next(xs)` — the same iterated protocol Rust/Python `for` lowers to, so a
/// single-variable Go range converges with them.
fn lower_range_into(
    fe: &dyn Frontend,
    clause: Node,
    body: &mut NormNode,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) {
    let Some(iter) = clause
        .child_by_field_name("right")
        .and_then(|n| fe.lower_node(n, None, src, log))
    else {
        return;
    };
    let guard = break_guard(call_ext("__has_next", iter.clone(), span), span);
    body.children.insert(0, guard);
    if let Some(left) = clause.child_by_field_name("left") {
        let mut targets = lower_targets(fe, left, true, src, log);
        // Drop the blank identifier `_` — it binds nothing, so it carries no similarity signal.
        // The idiomatic element loop `for _, x := range xs` then collapses to the single-target
        // foreach shape, converging with `for x in xs` and with the counter-loop iteration
        // rewrite (Go counter ≡ Go range ≡ Rust foreach).
        targets.retain(|t| !is_blank_target(t));
        if !targets.is_empty() {
            let mut value = call_ext("__next", iter, span);
            value.field = Some("value".into());
            let mut children = targets;
            children.push(value);
            body.children
                .insert(1, NormNode::new(kind::ASSIGN, None, span, children));
        }
    }
}

/// The blank identifier `_` (a `Var` labelled `_`) — a range target that binds nothing.
fn is_blank_target(node: &NormNode) -> bool {
    node.kind.as_ref() == kind::VAR
        && matches!(&node.label, Some(Label::Raw(t)) if t.as_ref() == "_")
}

/// Go `switch`/`type switch` → `Branch{ x@subject, Arm[case, body]… }` (§14). An
/// `expression_case`'s value(s) are the guard, a `type_case`'s type the guard, and
/// `default_case` the trivial-guard (else) arm. Each case body is the `statement_list`
/// lowered to a `Block`. (The type-switch `alias` bind is not yet modelled.)
fn lower_switch(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let subject = node
        .child_by_field_name("value")
        .and_then(|v| fe.lower_node(v, None, src, log));
    let mut arms = Vec::new();
    let mut c = node.walk();
    for case in node.named_children(&mut c) {
        let (guard, body) = match case.kind() {
            // `case v1, v2:` → `subj==v1 || subj==v2` (converges with the equivalent if);
            // a value-less `switch { case cond: }` uses the condition as the guard directly.
            "expression_case" => (
                case.child_by_field_name("value")
                    .and_then(|vl| case_value_guard(fe, subject.as_ref(), vl, span, src, log)),
                case_body(fe, case, span, src, log),
            ),
            // `switch x.(type) { case T: }` is a region-test, not an equality → `matches`.
            "type_case" => (
                case.child_by_field_name("type").and_then(|t| {
                    let ty = fe.lower_node(t, None, src, log)?;
                    Some(match subject.as_ref() {
                        Some(subj) => matches_guard(subj, ty, span),
                        None => ty,
                    })
                }),
                case_body(fe, case, span, src, log),
            ),
            "default_case" => (None, case_body(fe, case, span, src, log)),
            _ => continue,
        };
        arms.push(make_arm(guard, body, span));
    }
    make_branch(arms, field, span)
}

/// A Go `case` value list → the arm guard. With a switch subject, each value `v` becomes
/// `subject == v`, OR-joined for a multi-value case; without one (`switch { case c: }`),
/// each value is already a boolean condition and passes through.
fn case_value_guard(
    fe: &dyn Frontend,
    subject: Option<&NormNode>,
    list: Node,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> Option<NormNode> {
    let mut c = list.walk();
    let guards: Vec<NormNode> = list
        .named_children(&mut c)
        .filter_map(|v| {
            let value = fe.lower_node(v, None, src, log)?;
            Some(match subject {
                Some(subj) => eq_guard(subj, value, span),
                None => value,
            })
        })
        .collect();
    or_chain(guards, span)
}

/// A case body (its `statement_list`) → a `Block`.
fn case_body(
    fe: &dyn Frontend,
    case: Node,
    _span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> Option<NormNode> {
    let mut c = case.walk();
    let sl = case
        .named_children(&mut c)
        .find(|n| n.kind() == "statement_list")?;
    Some(block(fe, sl, Some("body"), span_of(sl), src, log))
}

/// Go `if`/`else if`/`else` → the unified `Branch` (§14). Go's `alternative` field is
/// the payload *directly* (`if_statement` for else-if, `block` for else) — no
/// `else_clause` wrapper — so lowering it recursively nests an else-if as the else arm's
/// body, matching the shared `lower_if`'s shape for Rust/Python.
fn lower_if_go(
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
        // `else if` (an `if_statement`) lowers to a nested `Branch`, spliced flat.
        let else_body = fe.lower_node(alt, Some("body"), src, log);
        push_else_arm(&mut arms, else_body);
    }
    NormNode::new(kind::BRANCH, field, span, arms)
}

/// `return e1, e2` → `Return{ e1@value, e2@value }`; a bare `return` → `Return{}`. The
/// `expression_list` wrapper is unwrapped so `return f(x)` is `Return{ Call }` — the
/// single-child shape tail-recursion lowering keys on.
fn lower_return_go(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let mut children = Vec::new();
    let mut cursor = node.walk();
    if let Some(list) = node
        .named_children(&mut cursor)
        .find(|c| c.kind() == "expression_list")
    {
        let mut lc = list.walk();
        for v in list.named_children(&mut lc) {
            if let Some(n) = fe.lower_node(v, Some("value"), src, log) {
                children.push(n);
            }
        }
    }
    NormNode::new(kind::RETURN, field, span, children)
}

/// `i++` / `i--` desugars to the [`lower_aug_assign`] shape `i = i ± 1` (Family B / D-IR-14):
/// `Assign{ i@place, Binop@value{ i@left, ±@op, 1@right } }`, recording `AugAssign`. There is
/// no `+= 1` CST node to route through the shared helper (the `1` is synthetic), so the same
/// synthesis is reproduced here. Desugaring — rather than a bare `Unop` — converges Go's
/// loop-counter idiom across `i++` / `i += 1` / `i = i + 1` and with the Rust/Python
/// equivalents. The literal `1` is a kept structural constant (spec §5.2.5), never bucketed.
fn lower_inc_dec(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let op_text = if node.kind() == "inc_statement" {
        "+"
    } else {
        "-"
    };
    let lvalue = node.named_child(0);
    // The lvalue appears twice: the assignment target (`@place`, a mutation) and the binop's
    // left operand (`@left`, the read). Lowering it twice is idempotent (§ `lower_aug_assign`).
    let place = lvalue.and_then(|n| fe.lower_node(n, Some("place"), src, log));
    let read = lvalue.and_then(|n| fe.lower_node(n, Some("left"), src, log));
    let op = NormNode::new(op_text, Some("op"), span, Vec::new());
    let one = NormNode::new(kind::LIT, Some("right"), span, Vec::new())
        .with_label(Label::LitKept("1".into()));
    let binop = NormNode::new(
        kind::BINOP,
        Some("value"),
        span,
        read.into_iter().chain([op, one]).collect(),
    );
    log.record(TransformKind::AugAssign, span, Witness::None);
    NormNode::new(
        kind::ASSIGN,
        field,
        span,
        place.into_iter().chain(std::iter::once(binop)).collect(),
    )
}

/// `assignment_statement`: plain `=` maps to a canonical `Assign` (a *mutation* — `@place`,
/// never a binding). Compound `+=`/`-=`/… desugars via the shared [`lower_aug_assign`]
/// (Family B / D-IR-14): `a op= b` → `a = a op b`, dropping the `=` off the operator token
/// to get the binary op — the same path Rust's `compound_assignment_expr` takes, so `i += 1`
/// converges with `i = i + 1` and Go's `i++`.
fn lower_assignment_stmt(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let is_plain = node
        .child_by_field_name("operator")
        .map(|o| span_text(o, src) == "=")
        .unwrap_or(false);
    if is_plain {
        lower_assign_go(fe, node, field, span, false, src, log)
    } else {
        lower_aug_assign(
            fe,
            node,
            ("left", "operator", "right", false),
            field,
            span,
            src,
            log,
        )
    }
}

/// `:=` short variable declaration → a *binding* [`lower_assign_go`] (`@target`), the shared helper
/// with `bind = true` pinned. A thin per-language adapter letting it ride in the table as an `Std`
/// entry (the analog of Python's `lower_not_py`), rather than a hand-written residue arm.
fn lower_short_var_decl(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    lower_assign_go(fe, node, field, span, true, src, log)
}

/// Build `Assign` from a `left`/`right` pair of `expression_list`s. `bind` picks the
/// target field: `@target` (a declaration — becomes a positional local) for `:=`/`var`/
/// `const`, `@place` (a mutation — untouched by abstraction) for `=`.
fn lower_assign_go(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    bind: bool,
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let mut children = Vec::new();
    if let Some(left) = node.child_by_field_name("left") {
        children.extend(lower_targets(fe, left, bind, src, log));
    }
    if let Some(right) = node.child_by_field_name("right") {
        children.extend(lower_values(fe, right, src, log));
    }
    NormNode::new(kind::ASSIGN, field, span, children)
}

/// A `var_spec`/`const_spec` → `Assign{ name@target…, value@value… }`.
fn lower_spec(fe: &dyn Frontend, spec: Node, src: &str, log: &mut TransformLog) -> NormNode {
    let span = span_of(spec);
    let mut children = Vec::new();
    let mut nc = spec.walk();
    for name in spec.children_by_field_name("name", &mut nc) {
        if let Some(n) = fe.lower_node(name, Some("target"), src, log) {
            children.push(n);
        }
    }
    if let Some(value) = spec.child_by_field_name("value") {
        children.extend(lower_values(fe, value, src, log));
    }
    NormNode::new(kind::ASSIGN, None, span, children)
}

/// Lower the elements of an `expression_list` of assignment targets. `bind` targets take
/// `@target` (declared locals); mutation lvalues take `@place`.
fn lower_targets(
    fe: &dyn Frontend,
    list: Node,
    bind: bool,
    src: &str,
    log: &mut TransformLog,
) -> Vec<NormNode> {
    let tf = if bind { "target" } else { "place" };
    let mut out = Vec::new();
    let mut cursor = list.walk();
    for t in list.named_children(&mut cursor) {
        if let Some(n) = fe.lower_node(t, Some(tf), src, log) {
            out.push(n);
        }
    }
    out
}

/// Lower the elements of an `expression_list` of assigned values (each `@value`).
fn lower_values(fe: &dyn Frontend, list: Node, src: &str, log: &mut TransformLog) -> Vec<NormNode> {
    let mut out = Vec::new();
    let mut cursor = list.walk();
    for v in list.named_children(&mut cursor) {
        if let Some(n) = fe.lower_node(v, Some("value"), src, log) {
            out.push(n);
        }
    }
    out
}

fn span_text<'a>(node: Node, src: &'a str) -> &'a str {
    node.utf8_text(src.as_bytes()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::lower_go_source;
    use crate::frontend::{lower_python_source, lower_rust_source};
    use crate::ir::render::to_sexpr;
    use crate::ir::transform::{TransformKind, TransformLog, Witness};

    fn go(src: &str) -> (crate::tree::NormNode, TransformLog) {
        lower_go_source(src).expect("a go function")
    }

    #[test]
    fn dispatch_table_is_a_consistent_single_source_of_truth() {
        // The table is the SP4 gate's source of truth (D-SP4-2), so every referenced kind must be
        // data in EXACTLY one place: a unique MAP key OR a residue kind, never both.
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
    fn go_records_paren_drop() {
        // Completeness (D-IR-12 construct-and-record): dropping redundant `( … )` is a
        // normalization, so it emits a ParenDrop event on an enabled sink — Go reaches the
        // shared `drop_parens`, at parity with Rust/Python.
        let (_, log) = go("func f() {\n\tx := (a + b)\n\tg(x)\n}\n");
        assert!(
            log.events()
                .iter()
                .any(|e| e.kind == TransformKind::ParenDrop),
            "paren-drop not recorded: {}",
            log.to_text()
        );
    }

    #[test]
    fn go_bucketed_literal_records_its_value_witness() {
        // A bucketed literal is lossy — its value rides in the witness so the divergence
        // stays recoverable (parity with Rust's witness coverage). `9` is off the keep-list.
        let (_, log) = go("func f() {\n\tg(9)\n}\n");
        let val = log.events().iter().find_map(|e| match &e.witness {
            Witness::Literal(t) => Some(t.to_string()),
            _ => None,
        });
        assert_eq!(val.as_deref(), Some("9"), "lit-bucket value not in witness");
    }

    #[test]
    fn go_multi_return_flattens_without_an_event() {
        // `return a, b` unwraps the `expression_list` to `Return{ a@value, b@value }` — a
        // FAITHFUL structural flatten (both values kept, order kept, nothing relocated to a
        // witness), so no transform is recorded. It is not a lossy normalization, and there
        // is no `TransformKind` for it (adding one would be new vocabulary — out of scope).
        let (ir, log) = go("func f() (int, int) {\n\treturn a, b\n}\n");
        assert_eq!(
            to_sexpr(&ir),
            "(Unit (Block@body (Return (Var@value a) (Var@value b))))"
        );
        assert!(
            log.is_empty(),
            "multi-return must record nothing: {}",
            log.to_text()
        );
    }

    #[test]
    fn go_lowers_the_core_subset() {
        let (ir, _log) = go("func add(a int) int {\n\treturn a + 5\n}\n");
        assert_eq!(
            to_sexpr(&ir),
            "(Unit (Var@param a) (Block@body (Return (Binop@value (Var@left a) (+@op) (Lit@right INT)))))"
        );
    }

    #[test]
    fn go_short_var_decl_is_a_binding_assign() {
        // `:=` declares — target becomes a positional local under abstraction.
        let (ir, _) = go("func f() {\n\tx := 5\n\tg(x)\n}\n");
        let s = to_sexpr(&crate::ir::abstract_idents(ir));
        assert!(
            s.contains("(Assign (Var@target v0) (Lit@value INT))"),
            "{s}"
        );
        assert!(s.contains("(Var@arg v0)"), "{s}");
    }

    #[test]
    fn keep_list_literals_stay_exact_while_others_bucket() {
        // `0`/`1`/`-1`/`""` are structural (spec §5.2.5) — kept verbatim, so a clone
        // that uses `1` does NOT converge with one that uses `9`.
        let (one, _) = go("func f() {\n\tg(1)\n}\n");
        let (nine, _) = go("func f() {\n\tg(9)\n}\n");
        let (one2, _) = go("func h() {\n\tg(1)\n}\n");
        let sx = |ir| to_sexpr(&crate::ir::abstract_idents(ir));
        assert!(sx(one.clone()).contains("(Lit@arg 1)"), "1 not kept");
        assert!(sx(nine).contains("(Lit@arg INT)"), "9 not bucketed");
        assert_eq!(sx(one), sx(one2), "two `1` calls must converge");
    }

    #[test]
    fn go_condition_for_and_while_form_records_looplower() {
        let (ir, log) = go("func w() {\n\tfor c() {\n\t\ts()\n\t}\n}\n");
        let s = to_sexpr(&ir);
        assert!(s.contains("(Loop"), "{s}");
        assert!(s.contains("(Break)"), "no break-guard: {s}");
        assert!(
            log.events()
                .iter()
                .any(|e| e.kind == TransformKind::LoopLower)
        );
    }

    #[test]
    fn go_infinite_for_is_the_bare_loop_core() {
        let (ir, log) = go("func l() {\n\tfor {\n\t\tbreak\n\t}\n}\n");
        let s = to_sexpr(&ir);
        assert_eq!(s, "(Unit (Block@body (Loop (Block@body (Break)))))");
        // Already canonical — no LoopLower recorded.
        assert!(
            !log.events()
                .iter()
                .any(|e| e.kind == TransformKind::LoopLower)
        );
    }

    #[test]
    fn go_three_clause_for_hoists_init_and_appends_desugared_update() {
        // `for i := 0; i < n; i++ { s() }` → `i = 0`; `Loop{ break-guard; s(); i = i + 1 }`.
        // Family B (D-IR-14): the `i++` update desugars to the `AugAssign` shape `i = i + 1`,
        // NOT a bare `Unop`, so the loop-counter idiom converges with `i += 1` / `i = i + 1`.
        let (ir, _) = go("func f(n int) {\n\tfor i := 0; i < n; i++ {\n\t\ts()\n\t}\n}\n");
        let s = to_sexpr(&ir);
        // init hoisted as a sibling before the Loop
        assert!(s.contains("(Assign (Var@target"), "no hoisted init: {s}");
        assert!(s.contains("(Loop"), "{s}");
        // the desugared update: `i = i + 1`, no `Unop`.
        assert!(
            s.contains("(Assign (Var@place i) (Binop@value (Var@left i) (+@op) (Lit@right 1)))"),
            "update not desugared to `i = i + 1`: {s}"
        );
        assert!(!s.contains("Unop (++"), "stale `++` Unop update: {s}");
    }

    #[test]
    fn go_single_range_converges_with_rust_and_python_for() {
        // The shared iterated-loop synthesis makes a single-variable Go `range`
        // structurally identical to a Rust/Python `for`.
        let abs = |ir| to_sexpr(&crate::ir::abstract_idents(ir));
        let (g, _) = go("func f() {\n\tfor v := range xs {\n\t\tg(v)\n\t}\n}\n");
        let (r, _) = lower_rust_source("fn f() { for v in xs { g(v); } }").unwrap();
        let (p, _) = lower_python_source("def f():\n    for v in xs:\n        g(v)\n").unwrap();
        let (gs, rs, ps) = (abs(g), abs(r), abs(p));
        assert_eq!(gs, rs, "go vs rust");
        assert_eq!(gs, ps, "go vs python");
        assert!(gs.contains("__has_next") && gs.contains("__next"), "{gs}");
    }

    #[test]
    fn go_inc_compound_and_explicit_assign_all_converge() {
        // Family B (D-IR-14): `i++`, `i += 1`, and `i = i + 1` all desugar to the same
        // `Assign{ i@place, Binop{ i, +, 1 } }` — the loop-counter idiom converges across
        // all three spellings (sexpr AND fingerprint equality).
        let sx = |src: &str| {
            let (ir, _) = go(src);
            to_sexpr(&ir)
        };
        let fp = |src: &str| {
            let (ir, _) = go(src);
            crate::fingerprint::merkle(&crate::ir::abstract_idents(ir))
        };
        let inc = "func f() {\n\ti++\n}\n";
        let compound = "func f() {\n\ti += 1\n}\n";
        let explicit = "func f() {\n\ti = i + 1\n}\n";
        let want = "(Unit (Block@body (Assign (Var@place i) (Binop@value (Var@left i) (+@op) (Lit@right 1)))))";
        assert_eq!(sx(inc), want, "i++ desugar shape");
        assert_eq!(sx(inc), sx(compound), "i++ vs i += 1");
        assert_eq!(sx(inc), sx(explicit), "i++ vs i = i + 1");
        assert_eq!(fp(inc), fp(compound), "fingerprint i++ vs i += 1");
        assert_eq!(fp(inc), fp(explicit), "fingerprint i++ vs i = i + 1");
        // Discrimination: the operator is preserved, so `++`/`+=` stay distinct from `--`/`-=`.
        assert_ne!(
            sx(inc),
            sx("func f() {\n\ti--\n}\n"),
            "i++ must not equal i--"
        );
        assert_ne!(
            sx(compound),
            sx("func f() {\n\ti -= 1\n}\n"),
            "i += 1 must not equal i -= 1"
        );
    }

    #[test]
    fn go_inc_and_compound_record_aug_assign() {
        // Both desugar paths route the `AugAssign` event onto an enabled log (B routes
        // through / mirrors `lower_aug_assign`).
        for src in ["func f() {\n\ti++\n}\n", "func f() {\n\ti += 1\n}\n"] {
            let (_, log) = go(src);
            assert!(
                log.events()
                    .iter()
                    .any(|e| e.kind == TransformKind::AugAssign),
                "no AugAssign recorded for {src:?}: {}",
                log.to_text()
            );
        }
    }

    #[test]
    fn go_counter_for_converges_with_rust_while() {
        // Family B's cross-language headline: a Go C-style counter loop lowers byte-identically
        // to the Rust `while`-counter equivalent — the `i++` update now matches Rust's `i += 1`
        // (both a `@place` mutation), so the whole loop converges.
        let abs = |ir| to_sexpr(&crate::ir::abstract_idents(ir));
        let (g, _) = go("func f(n int) {\n\tfor i := 0; i < n; i++ {\n\t\ts()\n\t}\n}\n");
        let (r, _) =
            lower_rust_source("fn f(n: i32) { let mut i = 0; while i < n { s(); i += 1; } }")
                .unwrap();
        assert_eq!(abs(g), abs(r), "go counter-for vs rust while-counter");
    }

    #[test]
    fn go_counter_update_matches_python_binding_model() {
        // Python's aug-assign is definitionally self-referential (`i += 1` desugars to
        // `i = i + 1`), so its target is a mutation (`@place`) — the SAME model Go/Rust use
        // for a re-assignment. Go's `i += 1` and Python's `i += 1` now agree completely:
        // same desugared arithmetic AND the same `@place` target field.
        let (g, _) = go("func f() {\n\ti += 1\n}\n");
        let (p, _) = lower_python_source("def f():\n    i += 1\n").unwrap();
        let (gs, ps) = (to_sexpr(&g), to_sexpr(&p));
        // Both desugar to `Assign{ i@place, Binop{ i, +, 1 } }` — byte-identical.
        let want = "(Assign (Var@place i) (Binop@value (Var@left i) (+@op) (Lit@right 1)))";
        assert!(gs.contains(want), "go aug-assign: {gs}");
        assert!(ps.contains(want), "python aug-assign: {ps}");
    }

    #[test]
    fn go_if_else_if_else_is_one_flat_branch_chain() {
        let (ir, _) = go(
            "func f(a int) int {\n\tif a > 0 {\n\t\treturn a\n\t} else if a < 0 {\n\t\treturn a\n\t} else {\n\t\treturn a\n\t}\n}\n",
        );
        let s = to_sexpr(&ir);
        // §14: one flat ordered Branch (else-if spliced in, not nested), 3 arms.
        assert_eq!(s.matches("Branch").count(), 1, "{s}");
        assert_eq!(s.matches("Arm").count(), 3, "{s}");
    }

    #[test]
    fn go_switch_converges_with_the_equivalent_if_chain() {
        // §14: `switch a { case 0: …; case 1: …; default: … }` ≡ the if-chain of `a == n`.
        let canon = |src: &str| {
            let (ir, _) = go(src);
            let tree = crate::ir::abstract_idents(ir);
            let edits = crate::ir::detect_comm_sort(&tree);
            to_sexpr(&crate::ir::apply(
                tree,
                &edits,
                &mut crate::ir::TransformLog::disabled(),
            ))
        };
        assert_eq!(
            canon(
                "func f(a int) int {\n\tswitch a {\n\tcase 0:\n\t\treturn g()\n\tcase 1:\n\t\treturn h()\n\tdefault:\n\t\treturn k()\n\t}\n}\n"
            ),
            canon(
                "func f(a int) int {\n\tif a == 0 {\n\t\treturn g()\n\t} else if a == 1 {\n\t\treturn h()\n\t} else {\n\t\treturn k()\n\t}\n}\n"
            ),
        );
    }

    #[test]
    fn go_selector_is_field_with_external_name() {
        let (ir, _) = go("func f() {\n\treturn obj.Field\n}\n");
        let s = to_sexpr(&crate::ir::abstract_idents(ir));
        assert!(s.contains("(Field"), "{s}");
        assert!(
            s.contains("(Var@name Field)"),
            "field name not external: {s}"
        );
    }

    #[test]
    fn go_tail_recursion_becomes_a_loop() {
        let (ir, log) = go(
            "func gcd(a int, b int) int {\n\tif b == 0 {\n\t\treturn a\n\t}\n\treturn gcd(b, a)\n}\n",
        );
        let s = to_sexpr(&ir);
        assert!(s.contains("(Loop"), "no loop: {s}");
        assert!(s.contains("(Continue)"), "no continue: {s}");
        assert!(
            log.events()
                .iter()
                .any(|e| e.kind == TransformKind::RecursionLower)
        );
    }
}
