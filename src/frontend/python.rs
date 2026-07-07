//! Python frontend: tree-sitter `function_definition` CST → canonical IR. A *mapping*
//! only — every synthesis and recursion helper is shared in [`super`], identical to the
//! Rust frontend's. Contrast the historical `src/lang/python.rs` (the full per-grammar
//! normalizer): this is a small fraction of it, and it re-implements *nothing*.

use super::Lowering as L;
use super::{
    Frontend, bind_pattern_idents, case_guard, lower_assign, lower_source, lower_unary_field,
    lower_unit, make_arm, make_branch, make_index, make_lambda, make_multi_assign, native,
    push_else_arm,
};
use crate::ir::kind;
use crate::ir::transform::TransformLog;
use crate::lang::Lang;
use crate::tree::{Bucket, NormNode};
use tree_sitter::Node;

/// Lower a Python `function_definition` CST node to a canonical IR tree + transform log
/// (an enabled sink — the convenience for callers that want the event stream, e.g. tests;
/// the bulk path threads its own [`TransformLog::disabled`] via [`super::normalize`]).
pub fn lower_python(node: Node, src: &str) -> (NormNode, TransformLog) {
    let mut log = TransformLog::new();
    let tree = lower_unit(&Python, node, src, &mut log);
    (tree, log)
}

/// Convenience: parse Python `src` and lower its first `function_definition`.
pub fn lower_python_source(src: &str) -> Option<(NormNode, TransformLog)> {
    let mut log = TransformLog::new();
    let tree = lower_source(&Python, Lang::Python, "function_definition", src, &mut log)?;
    Some((tree, log))
}

pub(crate) struct Python;

/// The data-driven Python dispatch table (D-SP4-2; `docs/sp4-grammar-lift-plan.md` §4), extending
/// the Rust increment-1 pattern with the [`super::Lowering::Std`] variant. Every **uniform** arm — a
/// CST kind handled by a *shared* [`super`] helper — is data exactly as in Rust; Python's
/// **language-local** lowerings (`lower_if_py`, `lower_ternary`, `lower_match_py`, `lower_comparison`,
/// `lower_lambda`, `lower_assignment_py`, and the `not` → `!` adapter [`lower_not_py`]) ride in the
/// table as `Std` fn-pointers rather than a hand-written residue list. The runtime dispatch reads this
/// exact table via [`super::lookup`] + [`super::dispatch`]; only the genuinely irreducible quirk that
/// the table can't express — the inline `subscript` construction — stays residue (see [`RESIDUE_KINDS`]).
pub(crate) const MAP: &[(&str, super::Lowering)] = &[
    ("function_definition", L::Function),
    ("block", L::Block),
    ("while_statement", L::While),
    ("for_statement", L::For("left", "right")),
    // Language-local lowerings — a shared node-set, a per-language shape — ride in the table as `Std`.
    ("if_statement", L::Std(lower_if_py)),
    ("conditional_expression", L::Std(lower_ternary)),
    ("match_statement", L::Std(lower_match_py)),
    ("comparison_operator", L::Std(lower_comparison)),
    ("lambda", L::Std(lower_lambda)),
    ("assignment", L::Std(lower_assignment_py)),
    // `not x` → `!x` (the shared `lower_unary_field` with Python's `argument` field name pinned).
    ("not_operator", L::Std(lower_not_py)),
    ("unary_operator", L::UnaryPositional),
    ("boolean_operator", L::Binary("left", "operator", "right")),
    ("binary_operator", L::Binary("left", "operator", "right")),
    // `a op= b` desugars to `a = a op b` — definitionally self-referential (the lvalue is read then
    // written), so the target is a *mutation* (`@place`, `bind = false`), matching Go/Rust `op=`.
    (
        "augmented_assignment",
        L::AugAssign("left", "operator", "right", false),
    ),
    ("break_statement", L::Leaf(kind::BREAK)),
    ("continue_statement", L::Leaf(kind::CONTINUE)),
    ("expression_statement", L::Unwrap),
    ("call", L::Call("function", "arguments")),
    ("return_statement", L::Return),
    ("parenthesized_expression", L::DropParens),
    ("attribute", L::Field("object", "attribute")),
    ("integer", L::Lit(Bucket::Int)),
    ("float", L::Lit(Bucket::Float)),
    ("string", L::Lit(Bucket::Str)),
    ("concatenated_string", L::Lit(Bucket::Str)),
    ("true", L::Lit(Bucket::Bool)),
    ("false", L::Lit(Bucket::Bool)),
    ("identifier", L::Var),
    // No similarity signal — dropped (Type-1 convergence / spec §5.2.7); `pass`/`comment` reduce to
    // the shared `Drop` exactly as Rust's `line_comment`/`block_comment` do (one entry per kind).
    ("pass_statement", L::Drop),
    ("comment", L::Drop),
];

/// Tree-sitter kinds the Python frontend references but the shared [`MAP`] does not key on —
/// registered as data so the increment-5 conformance gate can enumerate EVERY kind the frontend
/// references (MAP keys ∪ `RESIDUE_KINDS`) and assert each still exists in the linked grammar.
///
/// Three sources, all grammar-coupled strings the gate must still see:
///   * the one **residue-arm** kind — `subscript`, whose inline `Index` construction the table
///     can't express (the analog of Rust's residue `index_expression`);
///   * the internal discriminants of the table-dispatched **language-local (`Std`)** helpers.
///     This is the schema increment 2 introduces: an `Std` entry's *own* kind is a MAP key, but the
///     grammar kinds it matches on internally (a `case_clause`, an `elif_clause`) are not — so they
///     are enumerated here, generalizing Rust's "residue helpers' internal discriminants" to
///     "residue arm ∪ Std helpers". (Go increment 3 inherits this rule.); and
///   * the parameter kinds the hand-written [`Python::lower_params`] matches on — another
///     non-`lower_node` path the gate must see (parity with Rust's/Go's param registration).
// Consumed by the increment-5 conformance gate (via `super::frontend_table`), which asserts every
// entry still exists in the linked grammar — so the increment-2 `allow(dead_code)` that scoped this
// to the plain library build is gone (parity with Rust's `RESIDUE_KINDS`).
pub(crate) const RESIDUE_KINDS: &[&str] = &[
    // Dispatched from `lower_node`'s residue arm (a table miss → an inline Python-local lowering).
    "subscript", // xs[i] → Index (inline make_index)
    // Internal discriminants of the `Std` language-local helpers (not MAP keys, still grammar-coupled).
    "elif_clause",     // if_chain: an `elif` vs the trailing `else`
    "case_clause",     // lower_match_py: `match`/`case` arm iteration
    "case_pattern",    // lower_match_py / literal_pattern_value
    "none",            // literal_pattern_value: a `case None:` literal arm
    "pattern_list",    // tuple_elements: a parallel-assign target list
    "tuple_pattern",   // tuple_elements: a destructuring target
    "expression_list", // tuple_elements: a parallel-assign value list
    "tuple",           // tuple_elements: a parenthesized value tuple
    // Referenced by the hand-written `lower_params` (not `lower_node`): the parameter node kinds it
    // matches on. Registered so a grammar rename of a param kind trips the gate instead of silently
    // routing params to native(). (A bare `identifier` param is already gated via the MAP `Var` key.)
    "typed_parameter",   // lower_params: a `x: T` parameter
    "default_parameter", // lower_params: a `x = v` parameter
];

impl Frontend for Python {
    fn lower_node_inner(
        &self,
        node: Node,
        field: Option<&str>,
        src: &str,
        log: &mut TransformLog,
    ) -> Option<NormNode> {
        // The uniform arms AND Python's language-local lowerings are data (`MAP`): look the kind up,
        // dispatch it (shared helper or `Std` fn-ptr). A hit returning `None` is a deliberate drop
        // (`Lowering::Drop`, or an `Unwrap`/`DropParens` that found nothing) — NOT a miss, so it
        // never reaches residue.
        if let Some(entry) = super::lookup(MAP, node.kind()) {
            return super::dispatch(entry, self, node, field, src, log);
        }
        // Table miss → the Python-local residue (kinds registered in `RESIDUE_KINDS`): the one
        // irreducible inline quirk — `subscript`'s field-based `Index` construction — else an
        // unmodeled `native` leaf.
        let span = (node.start_byte() as u32, node.end_byte() as u32);
        match node.kind() {
            "subscript" => {
                let base = node
                    .child_by_field_name("value")
                    .and_then(|n| self.lower_node(n, Some("base"), src, log));
                let idx = node
                    .child_by_field_name("subscript")
                    .and_then(|n| self.lower_node(n, Some("idx"), src, log));
                Some(make_index(base, idx, field, span))
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

/// Lower a Python `assignment`. A **parallel** multi-target assignment (`a, b = X, Y`) becomes a
/// canonical multi-target `Assign` (byte-identical to Go's multi-`=`), which the shared
/// [`super::make_multi_assign`] → `detect_multi_assign` pass sequentializes with minimal temps —
/// so `a, b = b, a%b` converges with the temped adjacent assigns and with tail-rec reassignment.
/// A single-target assignment keeps the existing binding-vs-mutation rule; a destructure
/// (`a, b = f()`, unequal target/value counts — ANF territory, out of scope) falls through to the
/// single path (its tuple pattern stays `native`).
fn lower_assignment_py(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    // Purely syntactic identity rule (no scope analysis): a target read in its own RHS is
    // provably a *mutation* of an already-bound variable (`i = i + 1`, the swap `a, b = b, a`),
    // so it takes `@place` — matching Go/Rust `=`. A non-self-referential target (`x = 5`) stays
    // `@target` (a fresh binding — undecidable as a mutation without scope analysis).
    let bind = !assignment_is_self_referential(node, src);
    if let (Some(left), Some(right)) = (
        node.child_by_field_name("left"),
        node.child_by_field_name("right"),
    ) && let Some(tgts) = tuple_elements(left)
        && tgts.len() >= 2
    {
        let tf = if bind { "target" } else { "place" };
        if let Some(vals) = tuple_elements(right)
            && vals.len() == tgts.len()
        {
            // Parallel multi-target assignment (`a, b = X, Y`): a canonical multi-target `Assign`
            // the shared decomposition sequentializes with minimal temps.
            let targets: Vec<NormNode> = tgts
                .iter()
                .filter_map(|t| fe.lower_node(*t, Some(tf), src, log))
                .collect();
            let values: Vec<NormNode> = vals
                .iter()
                .filter_map(|v| fe.lower_node(*v, Some("value"), src, log))
                .collect();
            if targets.len() == vals.len() && values.len() == vals.len() {
                return make_multi_assign(targets, values, field, span);
            }
        } else if let Some(value) = fe.lower_node(right, Some("value"), src, log) {
            // Destructuring unpack (`label, score = entries[i]`): one RHS value, a tuple target.
            // Mark the pattern's bound idents as declared locals (`@target`) so a renamed capture
            // abstracts — matching the tuple `for`-loop element bind exactly. (Its counterpart in
            // a `for label, score in entries` loop is the destructure `super::lower_for` synthesizes.)
            if let Some(mut target) = fe.lower_node(left, Some(tf), src, log) {
                bind_pattern_idents(&mut target);
                return NormNode::new(kind::ASSIGN, field, span, vec![target, value]);
            }
        }
    }
    lower_assign(fe, node, field, span, ("left", "right", bind), src, log)
}

/// The element nodes of a Python tuple target/value list — a `pattern_list`/`tuple_pattern`
/// (targets) or `expression_list`/`tuple` (values). `None` for a single (non-list) operand.
fn tuple_elements(node: Node) -> Option<Vec<Node>> {
    if !matches!(
        node.kind(),
        "pattern_list" | "tuple_pattern" | "expression_list" | "tuple"
    ) {
        return None;
    }
    let mut c = node.walk();
    Some(node.named_children(&mut c).collect())
}

/// Whether an `assignment`'s target variable(s) appear free in its RHS value — a purely
/// syntactic test proving the assignment is a *mutation* of an already-bound variable
/// (`i = i + 1`, `total = total + x`, the swap `a, b = b, a`), not a fresh declaration.
/// Rationale: if the target is read on the right it must already be bound, and Python has
/// function-level scope, so the read and the write name the same variable — hence `@place`
/// (like Go/Rust `=`), not `@target`. This is the ONLY per-target signal Python's grammar
/// gives (unlike `let`/`:=`); a non-self-referential `x = 5` stays `@target`.
///
/// `bind` is a single flag for the whole `Assign`, so this is decided over ALL targets:
/// any target read in the RHS makes the assignment a mutation. Single-target counters/
/// accumulators (the convergence case) and the swap `a, b = b, a` (both read) map cleanly;
/// a mixed `a, b = a, 2` biases to `@place` (the FP-safe direction — a spurious `@place`
/// merely leaves a `Var` `External`, never over-converges). Tuple targets themselves lower
/// through `native` (unmodeled), so only the outer field label is affected there anyway.
///
/// Names bound *inside* a RHS lambda are excluded (the lambda subtree is opaque), so
/// `x = f(lambda x: x)` — where the RHS `x` is a shadowed lambda parameter, not the outer
/// `x` — stays a fresh binding. This is the conservative direction: an unrecognized
/// self-reference just keeps `@target`, never over-converges.
fn assignment_is_self_referential(node: Node, src: &str) -> bool {
    let (Some(left), Some(right)) = (
        node.child_by_field_name("left"),
        node.child_by_field_name("right"),
    ) else {
        return false;
    };
    let mut targets = Vec::new();
    collect_target_idents(left, src, &mut targets);
    targets
        .iter()
        .any(|name| rhs_mentions_name(right, name, src))
}

/// Collect every identifier name in a target subtree (`i`; the tuple `a, b`; `x.y`; `x[i]`).
fn collect_target_idents<'a>(node: Node, src: &'a str, out: &mut Vec<&'a str>) {
    if node.kind() == "identifier" {
        out.push(node.utf8_text(src.as_bytes()).unwrap_or_default());
        return;
    }
    let mut c = node.walk();
    for child in node.named_children(&mut c) {
        collect_target_idents(child, src, out);
    }
}

/// Whether `name` occurs as an identifier anywhere in the RHS value expression, skipping
/// `lambda` subtrees (a name bound as a lambda parameter is not the outer target).
fn rhs_mentions_name(node: Node, name: &str, src: &str) -> bool {
    if node.kind() == "lambda" {
        return false; // opaque: a lambda-bound name is not the outer variable
    }
    if node.kind() == "identifier" {
        return node.utf8_text(src.as_bytes()) == Ok(name);
    }
    let mut c = node.walk();
    node.named_children(&mut c)
        .any(|child| rhs_mentions_name(child, name, src))
}

/// Python `if / elif / else` → the unified `Branch` (§14). Python attaches `elif` as an
/// `elif_clause` and `else` as an `else_clause`, both under repeated `alternative` fields
/// (not the single `else_clause`-wrapper the shared `lower_if` assumes). Each `elif`'s
/// arms are spliced flat by `push_else_arm`, so the whole chain is one ordered `Branch`
/// that converges with the equivalent `match`.
fn lower_if_py(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let mut ac = node.walk();
    let alts: Vec<Node> = node
        .children_by_field_name("alternative", &mut ac)
        .collect();
    let cond = node.child_by_field_name("condition");
    let cons = node.child_by_field_name("consequence");
    if_chain(fe, cond, cons, &alts, 0, field, span, src, log)
}

#[allow(clippy::too_many_arguments)]
fn if_chain(
    fe: &dyn Frontend,
    cond: Option<Node>,
    cons: Option<Node>,
    alts: &[Node],
    idx: usize,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let guard = cond.and_then(|c| fe.lower_node(c, Some("guard"), src, log));
    let body = cons.and_then(|c| fe.lower_node(c, Some("body"), src, log));
    let mut arms = vec![make_arm(guard, body, span)];
    if let Some(alt) = alts.get(idx) {
        let else_body = match alt.kind() {
            // `elif` → a nested `Branch`, spliced flat by `push_else_arm`.
            "elif_clause" => Some(if_chain(
                fe,
                alt.child_by_field_name("condition"),
                alt.child_by_field_name("consequence"),
                alts,
                idx + 1,
                Some("body"),
                span,
                src,
                log,
            )),
            // `else_clause { body }` → the trivial-guard final arm.
            _ => alt
                .child_by_field_name("body")
                .and_then(|b| fe.lower_node(b, Some("body"), src, log)),
        };
        push_else_arm(&mut arms, else_body);
    }
    NormNode::new(kind::BRANCH, field, span, arms)
}

/// `match subj: case p: …` → `Branch{ subj@subject, Arm[p, body]… }` (§14). `case _:` is
/// the trivial-guard arm; captures in a `case_pattern` become declared locals.
fn lower_match_py(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let subject = node
        .child_by_field_name("subject")
        .and_then(|s| fe.lower_node(s, None, src, log));
    let mut arms = Vec::new();
    if let Some(body) = node.child_by_field_name("body") {
        let mut c = body.walk();
        for case in body.named_children(&mut c) {
            if case.kind() != "case_clause" {
                continue;
            }
            let mut pc = case.walk();
            let pat = case
                .named_children(&mut pc)
                .find(|n| n.kind() == "case_pattern");
            let cons = case
                .child_by_field_name("consequence")
                .and_then(|b| fe.lower_node(b, Some("body"), src, log));
            let guard = pat.and_then(|p| {
                let is_wildcard = p.utf8_text(src.as_bytes()).map(str::trim) == Ok("_");
                case_guard(
                    fe,
                    subject.as_ref(),
                    p,
                    is_wildcard,
                    literal_pattern_value(p),
                    span,
                    src,
                    log,
                )
            });
            arms.push(make_arm(guard, cons, span));
        }
    }
    make_branch(arms, field, span)
}

/// The bare literal node of a literal `case_pattern` (`case 1:` / `case "s":`), else `None`
/// (a class/capture pattern → matched via `matches(…)`).
fn literal_pattern_value(pat: Node) -> Option<Node> {
    if pat.kind() != "case_pattern" || pat.named_child_count() != 1 {
        return None;
    }
    let inner = pat.named_child(0)?;
    matches!(
        inner.kind(),
        "integer" | "float" | "string" | "concatenated_string" | "true" | "false" | "none"
    )
    .then_some(inner)
}

/// `a if c else b` → a 2-arm `Branch` (§14). `conditional_expression` is positional:
/// named children are `[value_if_true, condition, value_if_false]`.
fn lower_ternary(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let mut c = node.walk();
    let parts: Vec<Node> = node.named_children(&mut c).collect();
    let then_body = parts
        .first()
        .and_then(|n| fe.lower_node(*n, Some("body"), src, log));
    let guard = parts
        .get(1)
        .and_then(|n| fe.lower_node(*n, Some("guard"), src, log));
    let else_body = parts
        .get(2)
        .and_then(|n| fe.lower_node(*n, Some("body"), src, log));
    let arms = vec![
        make_arm(guard, then_body, span),
        make_arm(None, else_body, span),
    ];
    NormNode::new(kind::BRANCH, field, span, arms)
}

/// Python chained comparison `a < b <= c` → left-associative `Binop` nesting
/// (`Binop{ Binop{a < b} <= c }`), so each is a canonical 3-child `Binop` the order
/// pass can act on and a simple `a == b` converges with the other languages' binaries.
fn lower_comparison(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let mut oc = node.walk();
    let ops: Vec<Node> = node.children_by_field_name("operators", &mut oc).collect();
    let mut nc = node.walk();
    let operands: Vec<Node> = node.named_children(&mut nc).collect();
    let Some(first) = operands
        .first()
        .and_then(|n| fe.lower_node(*n, Some("left"), src, log))
    else {
        return native(fe, node, field, span, src, log);
    };
    let mut acc = first;
    for (i, op) in ops.iter().enumerate() {
        acc.field = Some("left".into());
        let op_span = (op.start_byte() as u32, op.end_byte() as u32);
        let op_node = NormNode::new(
            op.utf8_text(src.as_bytes()).unwrap_or_default(),
            Some("op"),
            op_span,
            Vec::new(),
        );
        let right = operands
            .get(i + 1)
            .and_then(|n| fe.lower_node(*n, Some("right"), src, log));
        acc = NormNode::new(
            kind::BINOP,
            None,
            span,
            std::iter::once(acc)
                .chain(std::iter::once(op_node))
                .chain(right)
                .collect(),
        );
    }
    acc.field = field.map(Into::into);
    acc
}

/// `lambda x: e` → `Lambda{ x@param, e@body }` (§5 `Lambda`).
fn lower_lambda(
    fe: &dyn Frontend,
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

/// Python `not x` → `Unop{ !, x }`: the shared [`lower_unary_field`] with Python's `argument` field
/// name pinned, so `not` canonicalizes to `!` and converges with Rust's `!x` and the break-guard
/// synthesis. A thin per-language adapter (the one Python quirk — the field name) around the shared
/// helper, letting it ride in the table as an `Std` entry instead of a residue arm.
fn lower_not_py(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    lower_unary_field(fe, node, field, span, "argument", src, log)
}

#[cfg(test)]
mod tests {
    use super::lower_python_source;
    use crate::frontend::lower_rust_source;
    use crate::ir::render::to_sexpr;
    use crate::ir::transform::TransformKind;

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

    fn abs(src: &str) -> String {
        let (ir, _) = lower_python_source(src).unwrap();
        to_sexpr(&crate::ir::abstract_idents(ir))
    }

    #[test]
    fn boolean_and_comparison_ops_lower_to_binops() {
        // `a and b` → Binop; `a < b` → Binop; both with a real op token.
        let s = abs("def f(a, b):\n    return a < b and a\n");
        assert!(s.contains("(Binop@") && s.contains("(and@op)"), "{s}");
        assert!(s.contains("(<@op)"), "{s}");
        assert!(
            !s.contains("NativeStmt") && !s.contains("NativeExpr"),
            "{s}"
        );
    }

    #[test]
    fn chained_comparison_nests_left_associatively() {
        let s = abs("def f(a, b, c):\n    return a < b <= c\n");
        // Binop{ Binop{a < b} <= c }
        assert!(s.contains("(<@op)") && s.contains("(<=@op)"), "{s}");
        assert_eq!(s.matches("Binop").count(), 2, "{s}");
    }

    #[test]
    fn unary_minus_lowers_to_unop() {
        let s = abs("def f(a):\n    return -a\n");
        assert!(s.contains("(Unop@value (-@op)"), "{s}");
    }

    #[test]
    fn augmented_assignment_desugars_to_binop_assign() {
        // `a += b` ≡ `a = a + b`: same canonical IR, and `+=` recorded as a transform.
        let plus_eq = "def f(a, b):\n    a += b\n    return a\n";
        let explicit = "def g(a, b):\n    a = a + b\n    return a\n";
        assert_eq!(abs(plus_eq), abs(explicit), "aug-assign must desugar");
        let (_, log) = lower_python_source(plus_eq).unwrap();
        assert!(
            log.events()
                .iter()
                .any(|e| e.kind == TransformKind::AugAssign)
        );
        // `+=` and `-=` stay distinct (op preserved in the binop).
        assert_ne!(
            abs(plus_eq),
            abs("def h(a, b):\n    a -= b\n    return a\n")
        );
    }

    #[test]
    fn self_referential_assign_is_a_place_mutation() {
        // `i = i + 1`: `i` is read in its own RHS → provably a mutation → `@place`
        // (converges with `i += 1` and with Go/Rust `i = i + 1`).
        let s = abs("def f(i):\n    i = i + 1\n    return i\n");
        assert!(
            s.contains("(Assign (Var@place v0) (Binop@value (Var@left v0) (+@op) (Lit@right 1)))"),
            "self-referential assign must be @place: {s}"
        );
        assert!(!s.contains("Var@target"), "{s}");
        // Explicit `i = i + 1` and the aug-assign `i += 1` converge byte-for-byte.
        assert_eq!(
            abs("def f(i):\n    i = i + 1\n    return i\n"),
            abs("def f(i):\n    i += 1\n    return i\n"),
            "explicit self-assign must converge with aug-assign",
        );
    }

    #[test]
    fn tuple_assignment_lowers_to_a_multi_target_assign_not_native() {
        // The `native` gap fix: `a, b = b, a % b` now lowers to a canonical multi-target `Assign`
        // (self-referential → `@place` mutations), NOT a `NativeStmt` tuple pattern — so it flows
        // through the shared parallel-assign decomposition (byte-identical to Go's multi-`=`).
        let (ir, _) =
            lower_python_source("def f(a, b):\n    a, b = b, a % b\n    return a\n").unwrap();
        let s = to_sexpr(&ir);
        assert!(!s.contains("Native"), "tuple assign still native: {s}");
        assert!(
            s.contains("(Assign (Var@place a) (Var@place b) (Var@value b) (Binop@value (Var@left a) (%@op) (Var@right b)))"),
            "not a canonical multi-target Assign: {s}"
        );
    }

    #[test]
    fn fresh_binding_stays_a_target() {
        // `x = 5`: `x` is NOT read in the RHS → a genuine fresh binding stays `@target`
        // (undecidable as a mutation without scope analysis; the near-miss must not flip).
        let s = abs("def f():\n    x = 5\n    return x\n");
        assert!(s.contains("(Assign (Var@target v0)"), "fresh binding: {s}");
        assert!(!s.contains("Var@place"), "{s}");
        // `x = y` (RHS names a different variable) also stays a fresh binding.
        let s2 = abs("def f(y):\n    x = y\n    return x\n");
        assert!(s2.contains("(Assign (Var@target"), "{s2}");
        assert!(!s2.contains("Var@place"), "{s2}");
    }

    #[test]
    fn lambda_shadowed_target_is_not_self_referential() {
        // Edge case: the RHS `x` is a shadowed lambda parameter, not the outer `x`, so the
        // assignment stays a fresh binding (`@target`) — the lambda subtree is opaque.
        let s = abs("def f(g):\n    x = g(lambda x: x)\n    return x\n");
        assert!(
            s.contains("(Assign (Var@target"),
            "lambda-shadowed target must stay @target: {s}"
        );
        assert!(!s.contains("Var@place"), "{s}");
    }

    #[test]
    fn if_elif_else_is_one_flat_branch_chain() {
        let s = abs(
            "def f(a, b):\n    if a:\n        return a\n    elif b:\n        return b\n    else:\n        return a\n",
        );
        // §14: one flat ordered Branch (elif spliced in, not nested), 3 arms.
        assert_eq!(s.matches("Branch").count(), 1, "{s}");
        assert_eq!(s.matches("Arm").count(), 3, "{s}");
        assert!(!s.contains("Native"), "{s}");
    }

    #[test]
    fn value_match_converges_with_the_equivalent_if_chain() {
        // §14: `match a: case 1/case 2/case _` ≡ `if a==1 / elif a==2 / else`.
        let canon = |src: &str| {
            let (ir, _) = lower_python_source(src).unwrap();
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
                "def f(a):\n    match a:\n        case 1:\n            return g()\n        case 2:\n            return h()\n        case _:\n            return k()\n"
            ),
            canon(
                "def f(a):\n    if a == 1:\n        return g()\n    elif a == 2:\n        return h()\n    else:\n        return k()\n"
            ),
        );
    }

    #[test]
    fn ternary_is_a_two_arm_branch() {
        // `a if c else b`: guard c, then-arm a, else-arm b.
        let s = abs("def f(a, b, c):\n    return a if c else b\n");
        assert!(s.contains("(Branch@value"), "{s}");
        assert_eq!(s.matches("Arm").count(), 2, "{s}");
    }

    #[test]
    fn lambda_lowers_to_a_lambda_node() {
        let s = abs("def f():\n    return lambda x: x + x\n");
        assert!(s.contains("(Lambda@value (Var@param v0)"), "{s}");
    }

    #[test]
    fn python_records_paren_drop() {
        // Completeness (D-IR-12 construct-and-record): dropping redundant parens is a
        // normalization, so it emits a ParenDrop event on an enabled sink (parity with Rust).
        let (_, log) = lower_python_source("def f(a):\n    return (a)\n").unwrap();
        assert!(
            log.events()
                .iter()
                .any(|e| e.kind == TransformKind::ParenDrop),
            "paren-drop not recorded: {}",
            log.to_text()
        );
    }

    #[test]
    fn python_lowers_the_core_subset() {
        let (ir, _log) = lower_python_source("def add(a):\n    return a + 5\n").unwrap();
        assert_eq!(
            to_sexpr(&ir),
            "(Unit (Var@param a) (Block@body (Return (Binop@value (Var@left a) (+@op) (Lit@right INT)))))"
        );
    }

    #[test]
    fn field_access_converges_and_names_stay_external() {
        let abs = |ir| to_sexpr(&crate::ir::abstract_idents(ir));
        let (r, _) = lower_rust_source("fn f(b: i32) { return a.b; }").unwrap();
        let (p, _) = lower_python_source("def f(b):\n    return a.b\n").unwrap();
        let (rs, ps) = (abs(r), abs(p));
        assert_eq!(rs, ps);
        // field name `b` stays External even though a local `b` (the param) exists.
        assert!(rs.contains("(Var@name b)"), "{rs}");
    }

    #[test]
    fn index_access_converges() {
        let abs = |ir| to_sexpr(&crate::ir::abstract_idents(ir));
        let (r, _) = lower_rust_source("fn f() { return xs[i]; }").unwrap();
        let (p, _) = lower_python_source("def f():\n    return xs[i]\n").unwrap();
        assert_eq!(abs(r), abs(p));
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
