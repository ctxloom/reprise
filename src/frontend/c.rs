//! C frontend: tree-sitter `function_definition` CST → canonical IR. Like Go, C is
//! C-like/imperative with no classes — the closest existing analog — but two real C quirks
//! have no counterpart in Rust/Go/Python and get dedicated handling here:
//!
//! 1. **Mandatory-paren conditions.** `if`/`while`/`do`/`switch` wrap their condition in a
//!    grammar-*required* `parenthesized_expression` (Go/Rust/Python conditions are bare). That
//!    wrapper is unwrapped directly ([`mandatory_cond_node`]) — no `ParenDrop` event, since it
//!    is not redundant/optional syntax; a genuinely redundant EXTRA layer (`if ((x))`) still
//!    records `ParenDrop` on the way through the ordinary `parenthesized_expression` MAP entry.
//! 2. **Optional braces.** An `if`/`while`/`do`/`for` body needn't be a `compound_statement`
//!    (`if (x) foo();` is legal) — Go/Rust/Python always require one. Every loop/if body is
//!    `block_wrap`-ed so the braced and unbraced spellings converge.
//!
//! **Preprocessor transparency (probe-load-bearing).** `preproc_if`/`preproc_ifdef` wrap
//! ordinary, fully-typed children (not opaque token soup) — recursing straight through is
//! *automatic* for unit discovery (`frontend::collect_ir_units` already walks every named child
//! unconditionally), and handled here for body lowering via [`Frontend::expand_stmt`]
//! ([`lower_preproc_children`]): both the taken and not-taken branch's statements splice into
//! the enclosing block, AS WRITTEN — never executing/emulating the preprocessor. Macro
//! *definitions* (`preproc_def`/`preproc_function_def`, their `preproc_arg` bodies) and
//! `preproc_include`/`preproc_call` stay opaque residue (see [`RESIDUE_KINDS`]).
//!
//! **No historical analog.** C has no `src/lang/c.rs` — CLAUDE.md retires that per-grammar
//! layer for new languages; this frontend is the entire C implementation.

use super::Lowering as L;
use super::{
    Frontend, block_wrap, break_guard, is_true_literal, lit, lower_assign, lower_aug_assign,
    lower_body, lower_source, lower_unary_positional, lower_unit, make_arm, make_index, native,
    push_else_arm, span_of, text,
};
use crate::ir::kind;
use crate::ir::transform::{TransformKind, TransformLog, Witness};
use crate::lang::Lang;
use crate::tree::{Bucket, Label, NormNode};
use tree_sitter::Node;

/// Lower a C `function_definition` CST node to a canonical IR tree + its transform log
/// (an enabled sink — the convenience for callers that want the event stream, e.g. tests;
/// the bulk path threads its own [`TransformLog::disabled`] via [`super::normalize`]).
pub fn lower_c(node: Node, src: &str) -> (NormNode, TransformLog) {
    let mut log = TransformLog::new();
    let tree = lower_unit(&C, node, src, &mut log);
    (tree, log)
}

/// Convenience: parse C `src` and lower its first `function_definition`.
pub fn lower_c_source(src: &str) -> Option<(NormNode, TransformLog)> {
    let mut log = TransformLog::new();
    let tree = lower_source(&C, Lang::C, "function_definition", src, &mut log)?;
    Some((tree, log))
}

pub(crate) struct C;

/// The data-driven C dispatch table (mirrors the Go/Python/Rust `MAP` pattern, D-SP4). Every
/// **uniform** arm — a CST kind handled by a *shared* [`super`] helper — is data; C's
/// **language-local** lowerings (`lower_function_c`, `lower_if_c`, `lower_while_c`,
/// `lower_do_c`, `lower_for_c`, `lower_assignment_c`, `lower_update_c`, `lower_ternary_c`,
/// `lower_number_c`) ride in the table as `Std` fn-pointers. Only the genuinely irreducible
/// quirk the table can't express — `subscript_expression`'s inline `Index` construction —
/// stays residue (see [`RESIDUE_KINDS`]).
///
/// Deliberately **out of MAP scope** for this WP (native residue, still walkable via
/// [`native`]'s recursive child-lowering — never a silent drop): `switch_statement`/
/// `case_statement` (C's real multi-case FALLTHROUGH has no counterpart in the shared
/// `Branch{Arm[guard,body]}` model every other frontend's switch/match uses — see the module
/// docs and the WP report for why forcing it in would distort semantics, D-SP4/CLAUDE.md's
/// promotion gate); `goto_statement`/`labeled_statement` (cheap, deliberately-unlowered
/// residue, rationed by the promotion gate); struct/union/enum/type declarations and
/// designated-initializer aggregates (`initializer_list`/`initializer_pair`/
/// `field_designator`/…, flagged by the probe for a *future* aggregate-promotion thread, not
/// "the imperative core" this WP scopes); `cast_expression`/`sizeof_expression`/
/// `offsetof_expression`/`alignof_expression` (no existing IR kind models these; Rust's
/// `as_expression` is native for the identical reason — not a C-specific gap); GNU inline asm
/// and C23 `[[attribute]]` syntax (both zero-observed in the probe's sampled directories).
pub(crate) const MAP: &[(&str, super::Lowering)] = &[
    ("function_definition", L::Std(lower_function_c)),
    ("compound_statement", L::Block),
    ("if_statement", L::Std(lower_if_c)),
    ("while_statement", L::Std(lower_while_c)),
    ("do_statement", L::Std(lower_do_c)),
    ("for_statement", L::Std(lower_for_c)),
    ("conditional_expression", L::Std(lower_ternary_c)),
    ("return_statement", L::Return),
    ("break_statement", L::Leaf(kind::BREAK)),
    ("continue_statement", L::Leaf(kind::CONTINUE)),
    ("expression_statement", L::Unwrap),
    ("call_expression", L::Call("function", "arguments")),
    ("field_expression", L::Field("argument", "field")),
    ("binary_expression", L::Binary("left", "operator", "right")),
    // C has ONE node kind for both plain `=` and every compound `op=` (unlike Rust, which has
    // separate `assignment_expression`/`compound_assignment_expr` kinds) — the Std adapter
    // reads the `operator` field text to pick the shared helper.
    ("assignment_expression", L::Std(lower_assignment_c)),
    // `i++`/`i--`/`++i`/`--i`: desugars to the shared `AugAssign` shape in STATEMENT position
    // (`i++;`), converging with `i += 1`/`i = i + 1` exactly like Go's `inc_statement` — else
    // (rare: used as a value, e.g. `x = i++`) stays a plain positional `Unop` (sound: C's
    // pre/post distinction has real value semantics the `Assign`-desugar can't express).
    ("update_expression", L::Std(lower_update_c)),
    ("unary_expression", L::UnaryPositional),
    // `*p` (deref) / `&x` (address-of): both prefix, positional — same shape as Rust's `&x`.
    ("pointer_expression", L::UnaryPositional),
    ("parenthesized_expression", L::DropParens),
    ("number_literal", L::Std(lower_number_c)),
    ("string_literal", L::Lit(Bucket::Str)),
    ("concatenated_string", L::Lit(Bucket::Str)),
    ("char_literal", L::Lit(Bucket::Char)),
    ("true", L::Lit(Bucket::Bool)),
    ("false", L::Lit(Bucket::Bool)),
    ("identifier", L::Var),
    // No similarity signal — dropped (Type-1 convergence / spec §5.2.7), as every other
    // frontend's comment kinds are.
    ("comment", L::Drop),
    // `null` is tree-sitter-c's OWN node kind for the literal spellings `NULL`/`nullptr` (not a
    // plain `identifier` — the grammar recognizes both textually as one keyword-literal rule).
    // Deliberately left OFF this table: it falls to the generic `native()` leaf, labelled by
    // `node.kind()` ("null") rather than the source spelling — so `NULL` and `nullptr` already
    // converge with each other with zero extra code, exactly mirroring how Python's bare `None`
    // (outside a `case` pattern) is handled today (`ir/substance.rs`'s doc comment: Python
    // `None` → `NativeStmt "none"`). No `Bucket::Null` variant is added (that touches the
    // shared, all-frontend `Bucket` enum for a single-language sentinel — the promotion gate's
    // "1 language → stays native" case).
];

/// Tree-sitter kinds the C frontend references but the shared [`MAP`] does not key on —
/// registered as data so the increment-5 conformance gate can enumerate EVERY kind the frontend
/// references (MAP keys ∪ `RESIDUE_KINDS`) and assert each still exists in the linked grammar
/// (parity with Rust/Go/Python's `RESIDUE_KINDS`).
///
/// Four groups:
///   * the one **residue-arm** kind — `subscript_expression`, whose inline `Index`
///     construction the table can't express (the analog of Go/Rust's residue
///     `index_expression`/Python's `subscript`);
///   * internal discriminants of the table-dispatched **language-local (`Std`)** helpers
///     (`declaration`, `function_declarator`) plus the parameter kind
///     [`C::lower_params`] matches on (`parameter_declaration`, parity with Go's identical
///     kind name — a naming coincidence, not shared implementation);
///   * the **preprocessor** kinds the probe ruled opaque-by-grammar-design (§5(B)): macro
///     *definitions* and their bodies, and plain directives — `preproc_def`,
///     `preproc_function_def`, `preproc_call`, `preproc_arg`, `preproc_params`,
///     `preproc_directive`, `preproc_defined`, `preproc_include`. `preproc_if`/`preproc_ifdef`
///     are the OPPOSITE call — transparent, not residue (see [`lower_preproc_children`]) — but
///     `expand_stmt` still string-matches them directly, so they are registered too (else a
///     grammar rename would silently kill the transparency with no gate catching it).
///     `preproc_elif`/`preproc_elifdef`/`preproc_else` are NOT listed: they are reached only
///     structurally, via the `alternative` field chain, never by a literal kind match.
///   * the two explicitly-decided-Native, high-frequency constructs the WP brief calls out by
///     name so they read as a deliberate choice, not an oversight: `goto_statement` (60.9% of
///     sampled `.c` files) / `labeled_statement`, and the one literal-syntax GNU attribute kind
///     observed in the sample, `attribute_specifier`. (The zero-observed asm family
///     (`gnu_asm_expression` & co, deferred — `arch/` wasn't sampled) and C23 `[[attribute]]`
///     bracket kinds are deliberately NOT pre-registered; they surface naturally in the
///     unmapped-kind advisory snapshot instead, exactly the mechanism that advisory exists for.)
pub(crate) const RESIDUE_KINDS: &[&str] = &[
    // Dispatched from `lower_node`'s residue arm (a table miss → an inline C-local lowering).
    "subscript_expression", // xs[i] → Index (inline make_index)
    // Internal discriminants of `expand_stmt`/`lower_params`/the `Std` helpers.
    "declaration", // expand_stmt: a local var/pointer/array declaration statement
    "function_declarator", // find_function_declarator: descends a (possibly pointer-wrapped) declarator
    "parameter_declaration", // lower_params: a named `T x` function parameter
    // Preprocessor: opaque-by-grammar-design residue (probe §5(B)) — macro definitions/bodies
    // and plain directives, never structurally parsed no matter how C-shaped their contents.
    "preproc_def",
    "preproc_function_def",
    "preproc_call",
    "preproc_arg",
    "preproc_params",
    "preproc_directive",
    "preproc_defined",
    "preproc_include",
    // Preprocessor: TRANSPARENT (recursed through, not opaque — probe §5(A)), but still
    // string-matched directly in `expand_stmt`, so registered for the same reason as above.
    "preproc_if",
    "preproc_ifdef",
    // Explicit, named Native residue (WP brief point 6) — cheap, deliberately unlowered.
    "goto_statement",
    "labeled_statement",
    "attribute_specifier",
];

impl Frontend for C {
    fn lower_node_inner(
        &self,
        node: Node,
        field: Option<&str>,
        src: &str,
        log: &mut TransformLog,
    ) -> Option<NormNode> {
        // The uniform arms AND C's language-local lowerings are data (`MAP`): look the kind
        // up, dispatch it (shared helper or `Std` fn-ptr). A hit returning `None` is a
        // deliberate drop (`Lowering::Drop`, or `DropParens` that found nothing) — NOT a miss.
        if let Some(entry) = super::lookup(MAP, node.kind()) {
            return super::dispatch(entry, self, node, field, src, log);
        }
        // Table miss → the C-local residue: the one irreducible inline quirk —
        // `subscript_expression`'s field-based `Index` construction — else an unmodeled
        // `native` leaf (covers `switch_statement`/`case_statement`, `goto_statement`/
        // `labeled_statement`, struct/union/enum/type declarations, designated-initializer
        // aggregates, casts, `sizeof`/`offsetof`/`alignof`, `null`, asm, attributes, and every
        // opaque preprocessor kind — see the `MAP`/`RESIDUE_KINDS` doc comments).
        let span = span_of(node);
        match node.kind() {
            "subscript_expression" => {
                let base = node
                    .child_by_field_name("argument")
                    .and_then(|n| self.lower_node(n, Some("base"), src, log));
                let idx = node
                    .child_by_field_name("index")
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
            if p.kind() != "parameter_declaration" {
                continue; // `variadic_parameter` (`...`) or an unnamed `void`: skip
            }
            let Some(declarator) = p.child_by_field_name("declarator") else {
                continue; // an abstract (unnamed) parameter, e.g. a prototype's bare `int`
            };
            if let Some(name) = declarator_name(declarator)
                && let Some(v) = self.lower_node(name, Some("param"), src, log)
            {
                out.push(v);
            }
        }
    }

    /// `expand_stmt` carries three C-local statement expansions (checked before ordinary
    /// `lower_node` dispatch by the shared `lower_stmts`):
    ///   * `declaration` — a local `T x = v, *y, z[8];` splices one `Assign` per declarator
    ///     into the enclosing block (mirrors Go's grouped `var (...)`), one with no value at
    ///     all for a bare (uninitialized) declarator — exactly Go's bare `var x int` shape.
    ///   * `for_statement` — a C-style `for (init; cond; update)`'s `init` hoists to a sibling
    ///     BEFORE the `Loop` (mirrors Go's C-style-for `expand_for_clauses` handling).
    ///   * `preproc_if` / `preproc_ifdef` — preprocessor-conditional TRANSPARENCY (probe §5(A),
    ///     load-bearing): splice both branches' statements into the enclosing block, as
    ///     written (see [`lower_preproc_children`]).
    fn expand_stmt(&self, node: Node, src: &str, log: &mut TransformLog) -> Option<Vec<NormNode>> {
        match node.kind() {
            "declaration" => Some(lower_declaration(self, node, src, log)),
            "preproc_if" | "preproc_ifdef" => Some(lower_preproc_children(self, node, src, log)),
            "for_statement" => {
                let init = node.child_by_field_name("initializer")?;
                let span = span_of(node);
                let mut out = Vec::new();
                if init.kind() == "declaration" {
                    out.extend(lower_declaration(self, init, src, log));
                } else if let Some(n) = self.lower_node(init, None, src, log) {
                    out.push(n);
                }
                out.push(lower_for_c(self, node, None, span, src, log));
                Some(out)
            }
            _ => None,
        }
    }

    /// `function_definition` has no `name` field of its own (see the trait doc comment) — find
    /// the nested `function_declarator` (through any pointer wrapping) and its bound identifier,
    /// the same two-step descent [`lower_function_c`] uses for the tail-recursion name.
    fn unit_name(&self, node: Node, src: &str) -> Option<String> {
        let fn_declarator = node
            .child_by_field_name("declarator")
            .and_then(find_function_declarator)?;
        let name = fn_declarator
            .child_by_field_name("declarator")
            .and_then(declarator_name)?;
        Some(text(name, src).to_string())
    }

    /// Mirrors the probe's §7 KUnit recommendation exactly: the dominant, unambiguous
    /// `*_test.c`/`*-test.c` filename convention (512 repo-wide hits) OR a testy path
    /// component (parity with every other language's `path_is_testy` convention), OR — as a
    /// secondary/confirming signal for a file that matches neither — a `kunit_test_suite(`/
    /// `kunit_test_suite_with_init(` registration call anywhere in the file (a cheap, grep-safe
    /// substring check; KUnit's registration API name is stable and unlikely to collide).
    fn unit_is_test(&self, _node: Node, src: &str, _name: &str, path: &std::path::Path) -> bool {
        let fname = path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or_default();
        if fname.ends_with("_test.c")
            || fname.ends_with("-test.c")
            || crate::lang::path_is_testy(path)
        {
            return true;
        }
        src.contains("kunit_test_suite(") || src.contains("kunit_test_suite_with_init(")
    }
}

/// Strip a declarator down to its bound `identifier`, descending through any wrapper that
/// carries a `declarator` field (`pointer_declarator` `*p`, `array_declarator` `a[8]`,
/// `function_declarator` (a function-pointer declarator's own name), `parenthesized_declarator`
/// `(*fp)(int)`) until reaching the bare name. Iterative (not recursive) with a generous,
/// bounded step cap — declarator nesting is human-authored (pointer/array levels), never an
/// attacker-controlled expression-depth vector, but the cap keeps this defensive regardless.
/// `None` for an abstract declarator with no name at all (a bare type in a prototype/cast).
fn declarator_name(node: Node) -> Option<Node> {
    let mut cur = node;
    for _ in 0..64 {
        if cur.kind() == "identifier" {
            return Some(cur);
        }
        cur = cur.child_by_field_name("declarator")?;
    }
    None
}

/// Descend through pointer/parenthesized declarator wrappers to find the innermost
/// `function_declarator` — needed because a pointer-returning function's declarator is wrapped
/// (`void *foo(int x)` → `pointer_declarator{ function_declarator{ … } }`), so
/// `function_definition`'s own `declarator` field is not always the `function_declarator`
/// directly. Iterative, same bounded-step shape as [`declarator_name`].
fn find_function_declarator(node: Node) -> Option<Node> {
    let mut cur = node;
    for _ in 0..64 {
        if cur.kind() == "function_declarator" {
            return Some(cur);
        }
        cur = cur.child_by_field_name("declarator")?;
    }
    None
}

/// `function_definition` → `Unit` (mirrors the shared [`super::lower_function`], adapted: C's
/// `function_definition` has no `parameters`/`name` field of its own — both live on the nested
/// `function_declarator`, found via [`find_function_declarator`] since a pointer-returning
/// function's declarator wraps it). The body/tail-recursion handling is otherwise identical to
/// the shared helper.
fn lower_function_c(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let fn_declarator = node
        .child_by_field_name("declarator")
        .and_then(find_function_declarator);
    let mut children = Vec::new();
    if let Some(params) = fn_declarator.and_then(|fd| fd.child_by_field_name("parameters")) {
        fe.lower_params(params, src, log, &mut children);
    }
    let mut body = fe.lower_fn_body(node, span, src, log);
    let name = fn_declarator
        .and_then(|fd| fd.child_by_field_name("declarator"))
        .and_then(declarator_name)
        .map(|n| text(n, src));
    if let Some(name) = name {
        let param_names: Vec<Box<str>> = children
            .iter()
            .filter_map(|p| match &p.label {
                Some(Label::Raw(t)) => Some(t.clone()),
                _ => None,
            })
            .collect();
        if param_names.len() == children.len() {
            body = crate::ir::pass::lower_tail_recursion(name, &param_names, body, log);
        }
    }
    children.push(body);
    NormNode::new(kind::UNIT, field, span, children)
}

/// A `declaration` statement (`int a, *b = v, c[8];`) → one `Assign` per comma-separated
/// declarator, spliced into the enclosing block (mirrors Go's grouped `var (...)`
/// `expand_for_clauses`-adjacent handling in shape). Each declarator's bound name is `@target`
/// (a fresh local, per C's `int x;`/`int x = v;` declaration syntax — never ambiguous with a
/// mutation the way Python's bare `=` is, since C mutations use the separate
/// `assignment_expression` grammar rule). A bare (uninitialized) declarator emits `Assign` with
/// ONLY a target child, no value — byte-identical in shape to Go's bare `var x int`.
fn lower_declaration(
    fe: &dyn Frontend,
    node: Node,
    src: &str,
    log: &mut TransformLog,
) -> Vec<NormNode> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for d in node.children_by_field_name("declarator", &mut cursor) {
        out.push(lower_declarator(fe, d, src, log));
    }
    out
}

/// One declarator (`init_declarator` or a bare name/pointer/array declarator) → `Assign{
/// name@target, value@value? }`. [`declarator_name`] handles both shapes uniformly — an
/// `init_declarator` itself carries a `declarator` field, so the same descent finds the bound
/// name whether or not there is an initializer.
fn lower_declarator(fe: &dyn Frontend, d: Node, src: &str, log: &mut TransformLog) -> NormNode {
    let span = span_of(d);
    let target = declarator_name(d).and_then(|n| fe.lower_node(n, Some("target"), src, log));
    let value = d
        .child_by_field_name("value")
        .and_then(|v| fe.lower_node(v, Some("value"), src, log));
    NormNode::new(
        kind::ASSIGN,
        None,
        span,
        target.into_iter().chain(value).collect(),
    )
}

/// Preprocessor-conditional TRANSPARENCY (probe §5(A), load-bearing): `#if`/`#ifdef`/`#elif`/
/// `#else` wrap ordinary, fully-typed children — 38.4% of the sampled kernel corpus's
/// conditional blocks directly wrap a `function_definition`. Flatten `node`'s real children
/// (skipping the `name`/`condition` field — the macro-name/expression being tested, not code)
/// into the enclosing statement list, recursing into a nested conditional's own children the
/// same way, then recurse into the `alternative` chain (`preproc_elif`/`preproc_elifdef`/
/// `preproc_else`) so BOTH the taken and not-taken branch's statements splice in, AS WRITTEN —
/// never executing/emulating the preprocessor (CLAUDE.md's preprocessor rule). Structural, not
/// kind-matched: `alternative`'s payload is handled by field shape, not by name, since only
/// `preproc_if`/`preproc_ifdef` can ever appear as a bare statement-list child in the first
/// place (`preproc_elif`/`preproc_elifdef`/`preproc_else` are reachable ONLY via this
/// `alternative` field chain per the grammar, never directly).
fn lower_preproc_children(
    fe: &dyn Frontend,
    node: Node,
    src: &str,
    log: &mut TransformLog,
) -> Vec<NormNode> {
    let skip_ids: Vec<usize> = ["name", "condition", "alternative"]
        .iter()
        .filter_map(|f| node.child_by_field_name(f))
        .map(|n| n.id())
        .collect();
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for c in node.named_children(&mut cursor) {
        if skip_ids.contains(&c.id()) {
            continue; // the macro name/condition (not code), or `alternative` (handled below)
        }
        if let Some(stmts) = fe.expand_stmt(c, src, log) {
            out.extend(stmts);
        } else if let Some(n) = fe.lower_node(c, None, src, log) {
            out.push(n);
        }
    }
    if let Some(alt) = node.child_by_field_name("alternative") {
        out.extend(lower_preproc_children(fe, alt, src, log));
    }
    out
}

/// C wraps every `if`/`while`/`do`/`switch` condition in a MANDATORY `parenthesized_expression`
/// (unlike Go/Rust/Python's bare condition) — this is the grammar-required wrapper, not
/// redundant/optional syntax, so its removal records no `ParenDrop` (a genuinely redundant
/// EXTRA layer, `if ((x))`, still records one on the way through the ordinary
/// `parenthesized_expression` MAP entry). Returns the raw (not-yet-lowered) inner node so a
/// caller can test it (`c_is_infinite_cond`) before deciding how to lower it.
fn mandatory_cond_node(node: Node) -> Option<Node> {
    let wrapper = node.child_by_field_name("condition")?;
    let mut cursor = wrapper.walk();
    wrapper.named_children(&mut cursor).next()
}

/// The condition is a literal always-true value — `true`/`True` (shared [`is_true_literal`],
/// stdbool/C23) OR the bare numeric literal `1` (idiomatic C's `while (1) { … }`/`for (;1;)`
/// infinite-loop spelling, with no `true` keyword available pre-C23). The `1` check is
/// deliberately C-LOCAL (not folded into the shared `is_true_literal`, which stays untouched so
/// Go/Rust/Python's canonical output cannot move) — a `while (1)` then converges with `for (;;)`
/// exactly as a Go `for true {}` converges with `for {}`.
fn c_is_infinite_cond(node: Node, src: &str) -> bool {
    is_true_literal(node, src) || text(node, src).trim() == "1"
}

/// A loop/if body that may be a bare, unbraced single statement (`while (x) foo();` is legal
/// C — Go/Rust/Python always require a block). `block_wrap`s it so the braced and unbraced
/// spellings converge (`while (x) { foo(); }` ≡ `while (x) foo();`).
fn lower_loop_body(
    fe: &dyn Frontend,
    node: Node,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    block_wrap(lower_body(fe, node, span, src, log), span)
}

/// C `if`/`else`/`else if` → the unified `Branch` (§14). `consequence`/`alternative` are C's own
/// field names (matching Rust/Go's shared shape); `alternative` wraps its payload in an
/// `else_clause` (an `if_statement` for `else if`, any `statement` for a plain/unbraced `else`) —
/// its bare, unfielded single child IS the payload. An `else if`'s lowered form (a `Branch`) is
/// passed through un-wrapped so `push_else_arm` splices it flat; anything else is `block_wrap`-ed
/// (the unbraced-else convergence case).
fn lower_if_c(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let guard = mandatory_cond_node(node).and_then(|c| fe.lower_node(c, Some("guard"), src, log));
    let body = node
        .child_by_field_name("consequence")
        .and_then(|c| fe.lower_node(c, Some("body"), src, log))
        .map(|b| block_wrap(b, span));
    let mut arms = vec![make_arm(guard, body, span)];
    if let Some(alt) = node.child_by_field_name("alternative") {
        let mut cursor = alt.walk();
        let else_body = alt
            .named_children(&mut cursor)
            .find_map(|c| fe.lower_node(c, Some("body"), src, log))
            .map(|b| {
                if b.kind == kind::id::BRANCH {
                    b // an `else if`: pass through so `push_else_arm` splices it flat
                } else {
                    block_wrap(b, span)
                }
            });
        push_else_arm(&mut arms, else_body);
    }
    NormNode::new(kind::BRANCH, field, span, arms)
}

/// C `while (cond) body` → `Loop{ break-guard; body }` (§5.2.2), the shared break-guard
/// synthesis Go/Rust/Python's `while` also reaches (mirrors [`super::lower_while`], adapted for
/// C's mandatory-paren condition and optional-brace body — see [`mandatory_cond_node`] /
/// [`lower_loop_body`]). `while (1)`/`while (true)` is already the infinite-loop core.
fn lower_while_c(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let mut body = lower_loop_body(fe, node, span, src, log);
    if let Some(cond_node) = mandatory_cond_node(node)
        && !c_is_infinite_cond(cond_node, src)
    {
        if let Some(cn) = fe.lower_node(cond_node, None, src, log) {
            body.children.insert(0, break_guard(cn, span));
        }
        log.record(
            TransformKind::LoopLower,
            span,
            Witness::LoopForm("while".into()),
        );
    }
    NormNode::new(kind::LOOP, field, span, vec![body])
}

/// C `do body while (cond);` → `Loop{ body; break-guard }` — the SAME `Loop`+break-guard core
/// as `while`, with the guard placed AFTER the body instead of before: sound and exact, since a
/// do-while unconditionally runs the body once, then repeats while `cond` holds — precisely
/// what a trailing `if !cond { break }` models. No existing reprise frontend has a do-while (Go/
/// Rust/Python have none), so this is genuinely new per-language shape — but no new IR
/// vocabulary: only `Loop`/`Branch`/`Break`, already shared.
fn lower_do_c(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let mut body = lower_loop_body(fe, node, span, src, log);
    if let Some(cond_node) = mandatory_cond_node(node)
        && !c_is_infinite_cond(cond_node, src)
    {
        if let Some(cn) = fe.lower_node(cond_node, None, src, log) {
            body.children.push(break_guard(cn, span));
        }
        log.record(
            TransformKind::LoopLower,
            span,
            Witness::LoopForm("do-while".into()),
        );
    }
    NormNode::new(kind::LOOP, field, span, vec![body])
}

/// C `for (init; cond; update) body` → `Loop{ break-guard; body; update }` (§5.2.2). Unlike
/// Go, C's `for_statement` already exposes `initializer`/`condition`/`update` as direct,
/// un-nested fields (no `for_clause`/`range_clause` wrapper to discriminate) — `init` hoists to
/// a sibling via [`Frontend::expand_stmt`] (mirrors Go's C-style-for handling exactly); this fn
/// builds the loop body itself, called either from `expand_stmt` (when there IS an initializer
/// to hoist) or directly via ordinary `MAP` dispatch (no initializer, e.g. `for (; i<n; i++)`).
fn lower_for_c(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let mut body = lower_loop_body(fe, node, span, src, log);
    if let Some(update) = node.child_by_field_name("update") {
        // The update clause's value is always discarded (like Go's `for_clause.update`), so
        // `i++`/`i--` there always desugars — matching Go's C-style-for update handling exactly,
        // regardless of the general statement-position-only rule `lower_update_c` applies.
        let lowered = if update.kind() == "update_expression" {
            Some(desugar_update(fe, update, None, span_of(update), src, log))
        } else {
            fe.lower_node(update, None, src, log)
        };
        if let Some(u) = lowered {
            body.children.push(u);
        }
    }
    match node.child_by_field_name("condition") {
        Some(cond) if !c_is_infinite_cond(cond, src) => {
            if let Some(cn) = fe.lower_node(cond, None, src, log) {
                body.children.insert(0, break_guard(cn, span));
            }
            log.record(
                TransformKind::LoopLower,
                span,
                Witness::LoopForm("for".into()),
            );
        }
        // No condition (`for (;;)`) or a literal always-true one: already the infinite-loop
        // core, no break-guard, no event — mirrors Go's `for {}`/`for true {}`.
        _ => {}
    }
    NormNode::new(kind::LOOP, field, span, vec![body])
}

/// `assignment_expression`: C has ONE node kind for both plain `=` (a pure mutation — `@place`;
/// C declarations are a SEPARATE grammar rule, `declaration`, so `=` here is never a fresh
/// binding) and every compound `op=` (unlike Rust, which has separate node kinds for the two).
/// Reads the `operator` field text to route to the shared [`lower_assign`] or
/// [`lower_aug_assign`] — the same shape Go's `lower_assignment_stmt` picks between.
fn lower_assignment_c(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let is_plain = node
        .child_by_field_name("operator")
        .map(|o| text(o, src) == "=")
        .unwrap_or(false);
    if is_plain {
        lower_assign(fe, node, field, span, ("left", "right", false), src, log)
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

/// `i++`/`i--`/`++i`/`--i`: in STATEMENT position (`i++;`, the direct child of an
/// `expression_statement` — its value is always discarded there) desugars via
/// [`desugar_update`] to the SAME `Assign{ i@place, Binop{ i, ±, 1 } }` shape `i += 1`/
/// `i = i + 1` reach (Family B / D-IR-14), converging all three spellings exactly like Go's
/// `inc_statement`/`dec_statement`. Elsewhere (rarer: used as a value, e.g. `x = i++`) stays a
/// plain positional `Unop` — sound, since prefix vs. postfix carry different VALUES there and
/// the `Assign`-desugar has no value to offer (a deliberate, documented precision limit: the
/// plain-`Unop` form does not distinguish `i++` from `++i` — both lower to `Unop{ "++", i }` —
/// an accepted narrow residue for the rare expression-position case, not a soundness gap).
fn lower_update_c(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let is_stmt = node
        .parent()
        .is_some_and(|p| p.kind() == "expression_statement");
    if is_stmt {
        desugar_update(fe, node, field, span, src, log)
    } else {
        lower_unary_positional(fe, node, field, span, src, log)
    }
}

/// The actual `i++`/`i--` → `Assign{ i@place, Binop@value{ i@left, ±@op, 1@right(kept) } }`
/// synthesis — byte-identical in shape to Go's `lower_inc_dec` (Family B / D-IR-14). The
/// lvalue is lowered TWICE (target/place — the write, and left — the read); idempotent, no
/// side effects in a lowering. `1` is the kept structural constant (spec §5.2.5), never
/// bucketed, matching Go's synthesized update literal exactly.
fn desugar_update(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let op_text = if node
        .child_by_field_name("operator")
        .map(|o| text(o, src) == "++")
        .unwrap_or(true)
    {
        "+"
    } else {
        "-"
    };
    let lvalue = node.child_by_field_name("argument");
    let place = lvalue.and_then(|n| fe.lower_node(n, Some("place"), src, log));
    let read = lvalue.and_then(|n| fe.lower_node(n, Some("left"), src, log));
    let op = NormNode::new(op_text, Some("op"), span, Vec::new());
    let one = NormNode::new(kind::LIT, Some("right"), span, Vec::new())
        .with_label(Label::LitKept(log.label_interner().intern("1")));
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

/// `cond ? then : else` → a 2-arm `Branch` (§14), the same shape Python's ternary reaches.
/// `condition`/`consequence`/`alternative` are C's own field names (matching `lower_if_c`'s
/// shape, unlike Python's positional `conditional_expression`). GNU's Elvis extension
/// `cond ?: else` (an omitted `consequence`) reuses the condition itself as the then-value —
/// the CLAUDE.md-sanctioned "Elvis → Branch" lowering (a structural approximation: the real
/// GNU semantics avoid re-evaluating `cond`, but for similarity purposes "the then-value IS the
/// condition" is the exact, sound reading).
fn lower_ternary_c(
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
    let then_body = match node.child_by_field_name("consequence") {
        Some(c) => fe.lower_node(c, Some("body"), src, log),
        None => node
            .child_by_field_name("condition")
            .and_then(|c| fe.lower_node(c, Some("body"), src, log)),
    };
    let else_body = node
        .child_by_field_name("alternative")
        .and_then(|c| fe.lower_node(c, Some("body"), src, log));
    let arms = vec![
        make_arm(guard, then_body, span),
        make_arm(None, else_body, span),
    ];
    NormNode::new(kind::BRANCH, field, span, arms)
}

/// tree-sitter-c uses ONE node kind, `number_literal`, for every numeric literal (unlike
/// Rust/Go/Python, which split `integer_literal`/`float_literal` at the grammar level) — bucket
/// choice needs a light text sniff: a `0x`/`0X` prefix is an integer (hex floats `0x1p0` are
/// vanishingly rare in kernel C; treated as `Int`), else a `.`/`e`/`E` marks a float, else an
/// integer. A small, sound, syntactic heuristic — not scope/type analysis.
fn lower_number_c(
    _fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    lit(node, field, span, number_bucket(text(node, src)), src, log)
}

fn number_bucket(t: &str) -> Bucket {
    let t = t.trim();
    if t.len() > 1 && (t.starts_with("0x") || t.starts_with("0X")) {
        return Bucket::Int;
    }
    if t.contains('.') || t.contains('e') || t.contains('E') {
        Bucket::Float
    } else {
        Bucket::Int
    }
}

#[cfg(test)]
mod tests {
    use super::lower_c_source;
    use crate::ir::render::to_sexpr;
    use crate::ir::transform::TransformKind;

    #[test]
    fn dispatch_table_is_a_consistent_single_source_of_truth() {
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
        let (ir, log) = lower_c_source(src).expect("a c function");
        to_sexpr(
            &crate::ir::abstract_idents(ir, log.label_interner()),
            log.label_interner(),
        )
    }

    fn fp(src: &str) -> u128 {
        let (ir, log) = lower_c_source(src).expect("a c function");
        crate::fingerprint::merkle(
            &crate::ir::abstract_idents(ir, log.label_interner()),
            log.label_interner(),
        )
    }

    #[test]
    fn self_ref_mutation_converges_across_all_three_c_spellings() {
        // The task's headline gate: `i += 1`, `i = i + 1`, and `i++;` must all converge
        // (fingerprint AND canonical sexpr), matching Go/Rust/Python's `@place` rule exactly.
        let plus_eq = "void f(int i) { i += 1; }";
        let explicit = "void f(int i) { i = i + 1; }";
        let incr = "void f(int i) { i++; }";
        assert_eq!(abs(plus_eq), abs(explicit), "i += 1 vs i = i + 1");
        assert_eq!(abs(plus_eq), abs(incr), "i += 1 vs i++");
        assert_eq!(fp(plus_eq), fp(explicit));
        assert_eq!(fp(plus_eq), fp(incr));
        assert!(
            abs(explicit).contains(
                "(Assign (Var@place v0) (Binop@value (Var@left v0) (+@op) (Lit@right 1)))"
            ),
            "{}",
            abs(explicit)
        );
    }

    #[test]
    fn genuinely_different_functions_stay_distinct() {
        let a = "int f(int a, int b) { return a + b; }";
        let b = "int f(int a, int b) { return a * b - 1; }";
        assert_ne!(fp(a), fp(b));
    }

    #[test]
    fn declaration_without_initializer_is_a_bare_target() {
        // `int x;` → `Assign{ x@target }`, no value — same shape as Go's bare `var x int`.
        let s = abs("void f(void) { int x; x = 5; g(x); }");
        assert!(s.contains("(Assign (Var@target v0))"), "{s}");
    }

    #[test]
    fn compound_literal_for_loop_converges_with_while() {
        let for_form = abs("void f(int n) { for (int i = 0; i < n; i++) { s(i); } }");
        let while_form = abs("void f(int n) { int i = 0; while (i < n) { s(i); i += 1; } }");
        assert_eq!(for_form, while_form);
    }

    #[test]
    fn while_1_and_for_ever_are_both_the_infinite_core() {
        let a = abs("void f(void) { while (1) { s(); } }");
        let b = abs("void f(void) { for (;;) { s(); } }");
        assert_eq!(a, b);
        assert!(!a.contains("Branch"), "no break-guard expected: {a}");
    }

    #[test]
    fn unbraced_and_braced_bodies_converge() {
        let unbraced = abs("void f(int x) { if (x) foo(); }");
        let braced = abs("void f(int x) { if (x) { foo(); } }");
        assert_eq!(unbraced, braced);
    }

    #[test]
    fn do_while_places_the_guard_after_the_body() {
        let (ir, log) = lower_c_source("void f(int n) { do { s(); } while (n < 10); }").unwrap();
        let s = to_sexpr(&ir, log.label_interner());
        assert!(s.contains("(Loop"), "{s}");
        assert!(
            log.events()
                .iter()
                .any(|e| e.kind == TransformKind::LoopLower)
        );
        // The break-guard (a Branch) must come AFTER the call, not before.
        let call_pos = s.find("(Call").unwrap();
        let branch_pos = s.find("(Branch").unwrap();
        assert!(branch_pos > call_pos, "guard must follow the body: {s}");
    }

    #[test]
    fn function_inside_ifdef_is_extracted() {
        let src = "#ifdef CONFIG_FOO\nstatic int helper(int x) {\n    return x + 1;\n}\n#endif\n";
        let label_interner = crate::intern::LabelInterner::new();
        let units = crate::frontend::extract_ir_units(
            src,
            crate::lang::Lang::C,
            std::path::Path::new("t.c"),
            &label_interner,
        );
        assert_eq!(units.len(), 1, "expected exactly one unit");
        assert_eq!(units[0].name, "helper");
    }

    #[test]
    fn both_ifdef_branches_splice_into_the_enclosing_block() {
        // Probe §5(A): a preproc conditional INSIDE a function body lowers its children in
        // place, BOTH branches, as written — never emulating the preprocessor.
        let src = "void f(void) {\n#ifdef DEBUG\n    dbg();\n#else\n    release();\n#endif\n}\n";
        let (ir, log) = lower_c_source(src).unwrap();
        let s = to_sexpr(&ir, log.label_interner());
        assert!(s.contains("dbg"), "{s}");
        assert!(s.contains("release"), "{s}");
    }

    #[test]
    fn error_bearing_file_still_yields_a_well_typed_unit() {
        // Mirrors the probe's dominant ERROR-residue shape: an attribute-macro token
        // (`__init`) between the type and declarator becomes an isolated ERROR node, but the
        // function's declarator and body parse fine on either side — extraction must not gate
        // on file-level cleanliness.
        let src = "static void __init foo(void) {\n    bar();\n}\n";
        let label_interner = crate::intern::LabelInterner::new();
        let units = crate::frontend::extract_ir_units(
            src,
            crate::lang::Lang::C,
            std::path::Path::new("t.c"),
            &label_interner,
        );
        assert_eq!(
            units.len(),
            1,
            "unit extraction must not gate on ERROR ancestry"
        );
        assert_eq!(units[0].name, "foo");
        assert!(
            units[0].parse_degraded,
            "the ERROR ancestry must flag parse_degraded"
        );
    }

    #[test]
    fn c_unit_is_test_matches_kunit_conventions() {
        use crate::frontend::Frontend;
        use crate::lang::Lang;
        use std::path::Path;
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&Lang::C.ts_language()).unwrap();
        let src = "void f(void) {}\n";
        let tree = parser.parse(src, None).unwrap();
        let node = tree
            .root_node()
            .named_child(0)
            .expect("a function_definition");

        assert!(super::C.unit_is_test(node, src, "f", Path::new("foo_test.c")));
        assert!(super::C.unit_is_test(node, src, "f", Path::new("foo-test.c")));
        assert!(super::C.unit_is_test(node, src, "f", Path::new("tests/foo.c")));
        assert!(!super::C.unit_is_test(node, src, "f", Path::new("foo.c")));

        let kunit_src = "void f(void) {}\nkunit_test_suite(my_suite);\n";
        assert!(super::C.unit_is_test(node, kunit_src, "f", Path::new("foo.c")));
    }
}
