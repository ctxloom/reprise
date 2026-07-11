//! Kotlin language profile (spec §3, M3c), grammar `tree-sitter-kotlin-ng`
//! (DECISIONS.md D23 records the crate choice). Node kinds and lowered shapes
//! taken from `examples/probe3.rs` output (DECISIONS.md D9).
//!
//! Kotlin quirks handled here: the function body is `function_body > block`
//! with no `body` field (navigated by kind); `break`/`continue`/`true` parse as
//! bare `identifier`s; navigation targets are relabeled External by a pre-pass
//! (no distinct field/kind distinguishes them positionally).

use super::{
    LanguageProfile, child_field, is_raw_ident, path_is_testy, raw_name, synth_call, synth_ident,
};
use crate::intern::{Field, Kind};
use crate::tree::{Bucket, Label, NormNode};
use std::path::Path;
use std::sync::LazyLock;

pub struct KotlinProfile;

// ---- interned grammar kind/field literals (interning-id-conversion WP) ----
//
// This is the historical, per-language profile — Kotlin's own grammar vocabulary
// (`tree-sitter-kotlin-ng` node/field names) is NOT the canonical IR vocabulary, so
// almost every literal below gets its own file-local `LazyLock<Kind>`/`LazyLock<Field>`,
// interned once ever, then compared as a bare id everywhere (mechanism rule 2). The
// two exceptions are `binary_fields`' `"left"`/`"right"` field names, which happen to
// be spelled identically to the canonical `crate::ir::field::id::LEFT`/`RIGHT` — those
// reuse the pre-registered consts directly (rule 1) rather than mint a redundant local
// static; `"operator"` is NOT the canonical `"op"`, so it gets its own static.
static FUNCTION_BODY: LazyLock<Kind> = LazyLock::new(|| Kind::intern("function_body"));
static IDENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("identifier"));
static FUNCTION_DECLARATION: LazyLock<Kind> =
    LazyLock::new(|| Kind::intern("function_declaration"));
static PARAMETER: LazyLock<Kind> = LazyLock::new(|| Kind::intern("parameter"));
static VARIABLE_DECLARATION: LazyLock<Kind> =
    LazyLock::new(|| Kind::intern("variable_declaration"));
static WHILE_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("while_statement"));
static DO_WHILE_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("do_while_statement"));
static FOR_STATEMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("for_statement"));
static BLOCK: LazyLock<Kind> = LazyLock::new(|| Kind::intern("block"));
static BINARY_EXPRESSION: LazyLock<Kind> = LazyLock::new(|| Kind::intern("binary_expression"));
static VALUE_ARGUMENTS: LazyLock<Kind> = LazyLock::new(|| Kind::intern("value_arguments"));
static VALUE_ARGUMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("value_argument"));
static FUNCTION_VALUE_PARAMETERS: LazyLock<Kind> =
    LazyLock::new(|| Kind::intern("function_value_parameters"));
static REPEAT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("REPEAT"));
static PROPERTY_DECLARATION: LazyLock<Kind> =
    LazyLock::new(|| Kind::intern("property_declaration"));
static ASSIGNMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("assignment"));
static IF_EXPRESSION: LazyLock<Kind> = LazyLock::new(|| Kind::intern("if_expression"));
static WHEN_EXPRESSION: LazyLock<Kind> = LazyLock::new(|| Kind::intern("when_expression"));
static RETURN_EXPRESSION: LazyLock<Kind> = LazyLock::new(|| Kind::intern("return_expression"));
static CALL_EXPRESSION: LazyLock<Kind> = LazyLock::new(|| Kind::intern("call_expression"));
static NAVIGATION_EXPRESSION: LazyLock<Kind> =
    LazyLock::new(|| Kind::intern("navigation_expression"));
static INFIX_EXPRESSION: LazyLock<Kind> = LazyLock::new(|| Kind::intern("infix_expression"));
static INDEX_EXPRESSION: LazyLock<Kind> = LazyLock::new(|| Kind::intern("index_expression"));
static LAMBDA_LITERAL: LazyLock<Kind> = LazyLock::new(|| Kind::intern("lambda_literal"));
static ANONYMOUS_FUNCTION: LazyLock<Kind> = LazyLock::new(|| Kind::intern("anonymous_function"));

static CONDITION: LazyLock<Field> = LazyLock::new(|| Field::intern("condition"));
static OPERATOR: LazyLock<Field> = LazyLock::new(|| Field::intern("operator"));

impl LanguageProfile for KotlinProfile {
    fn is_function_like(&self, kind: &str) -> bool {
        kind == "function_declaration"
    }

    fn is_comment(&self, kind: &str) -> bool {
        matches!(kind, "line_comment" | "block_comment" | "comment")
    }

    fn strip_kind(&self, kind: &str) -> bool {
        // Annotations + visibility/other modifiers: noise for structural
        // matching (@Test is still read from the raw CST by unit_is_test).
        kind == "modifiers"
    }

    fn splice_kind(&self, kind: Kind) -> bool {
        // Hoist the function body's block out of its `function_body` wrapper so
        // it becomes a direct child of `function_declaration`.
        kind == *FUNCTION_BODY
    }

    fn is_identifier(&self, kind: &str) -> bool {
        kind == "identifier"
    }

    fn literal_bucket(&self, kind: &str) -> Option<Bucket> {
        match kind {
            "number_literal" | "integer_literal" | "hex_literal" | "bin_literal" => {
                Some(Bucket::Int)
            }
            "float_literal" | "real_literal" => Some(Bucket::Float),
            "string_literal" | "line_string_literal" => Some(Bucket::Str),
            "character_literal" => Some(Bucket::Char),
            "boolean_literal" => Some(Bucket::Bool),
            _ => None,
        }
    }

    fn unwrap_kind(&self, kind: &str) -> bool {
        kind == "parenthesized_expression"
    }

    fn keep_anon_parent(&self, parent_kind: &str) -> bool {
        matches!(
            parent_kind,
            "binary_expression" | "unary_expression" | "assignment"
        )
    }

    fn always_external(&self, _kind: Kind, _field: Option<Field>, _parent_kind: Kind) -> bool {
        // Navigation targets are handled by `externalize_nav` (no positional
        // field distinguishes them); nothing else is kind-based external.
        false
    }

    fn collect_declared(&self, root: &NormNode, out: &mut Vec<Box<str>>) {
        fn direct_idents(node: &NormNode, out: &mut Vec<Box<str>>) {
            for c in &node.children {
                if c.kind == *IDENT
                    && let Some(Label::Raw(text)) = &c.label
                {
                    out.push(text.clone());
                }
            }
        }
        fn walk(node: &NormNode, out: &mut Vec<Box<str>>) {
            if node.kind == *FUNCTION_DECLARATION {
                if let Some(name) = child_field(node, "name")
                    && let Some(Label::Raw(text)) = &name.label
                {
                    out.push(text.clone());
                }
            } else if node.kind == *PARAMETER {
                // `parameter { identifier name, user_type }` — the direct
                // identifier is the name; type identifiers are nested deeper.
                direct_idents(node, out);
            } else if node.kind == *VARIABLE_DECLARATION {
                // val/var/for-loop binder.
                direct_idents(node, out);
            }
            for child in &node.children {
                walk(child, out);
            }
        }
        walk(root, out);
    }

    fn lower_loops(
        &self,
        node: NormNode,
        label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
    ) -> NormNode {
        lower(externalize_nav(node, label_interner), label_interner)
    }

    fn lower_recursion(&self, root: NormNode) -> NormNode {
        lower_recursion(root)
    }

    fn rewrite_iteration(
        &self,
        root: NormNode,
        _label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
    ) -> NormNode {
        rewrite_iteration(root)
    }

    fn normalize_loop_exit(&self, root: NormNode) -> NormNode {
        normalize_loop_exit(self, root)
    }

    fn commutative_ops(&self) -> &'static [&'static str] {
        &["+", "*", "==", "!=", "&&", "||"]
    }

    fn binary_fields(&self, kind: Kind) -> Option<(Option<Field>, Option<Field>, Option<Field>)> {
        if kind == *BINARY_EXPRESSION {
            Some((
                Some(crate::ir::field::id::LEFT),
                Some(*OPERATOR),
                Some(crate::ir::field::id::RIGHT),
            ))
        } else {
            None
        }
    }

    fn sortable_pair_kind(&self, _kind: Kind) -> Option<Field> {
        None
    }

    fn is_list_kind(&self, kind: Kind) -> bool {
        kind == *BLOCK
            || kind == *VALUE_ARGUMENTS
            || kind == *FUNCTION_VALUE_PARAMETERS
            || kind == *REPEAT
    }

    fn is_statement_kind(&self, kind: Kind) -> bool {
        kind == *PROPERTY_DECLARATION
            || kind == *ASSIGNMENT
            || kind == *FOR_STATEMENT
            || kind == *WHILE_STATEMENT
            || kind == *DO_WHILE_STATEMENT
            || kind == *IF_EXPRESSION
            || kind == *WHEN_EXPRESSION
            || kind == *RETURN_EXPRESSION
            || kind == *CALL_EXPRESSION
    }

    fn is_loop_core(&self, node: &NormNode) -> bool {
        node.kind == *WHILE_STATEMENT
            && node
                .children
                .iter()
                .any(|c| c.field == Some(*CONDITION) && is_true_ident(c))
    }

    fn is_continue_stmt(&self, node: &NormNode) -> bool {
        node.kind == *IDENT && ident_text_is(node, "continue")
    }

    fn unit_is_test(&self, node: tree_sitter::Node, src: &str, name: &str, path: &Path) -> bool {
        if name.starts_with("test") || path_is_testy(path) {
            return true;
        }
        // @Test / @ParameterizedTest live in the leading `modifiers` child.
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "modifiers" {
                let text = child.utf8_text(src.as_bytes()).unwrap_or_default();
                if text.contains("Test") {
                    return true;
                }
                break;
            }
        }
        false
    }

    // ---- Phase 3: inliner hooks (spec §5.4) ----

    fn call_kind(&self) -> Kind {
        *CALL_EXPRESSION
    }

    fn inline_params(&self, root: &NormNode) -> Option<Vec<Box<str>>> {
        simple_params(root)
    }

    fn return_value<'a>(&self, stmt: &'a NormNode) -> Option<&'a NormNode> {
        if stmt.kind == *RETURN_EXPRESSION && stmt.children.len() == 1 {
            return Some(&stmt.children[0]);
        }
        None
    }

    fn make_return(&self, mut value: NormNode) -> NormNode {
        let span = value.span;
        value.field = None;
        NormNode::with_kind(*RETURN_EXPRESSION, None, span, vec![value])
    }

    fn make_expr_stmt(&self, mut expr: NormNode) -> NormNode {
        // Kotlin statements are bare expressions (no wrapper node).
        expr.field = None;
        expr
    }

    fn make_expr_block(&self, _span: (u32, u32), _children: Vec<NormNode>) -> Option<NormNode> {
        None
    }
}

fn ident_text_is(node: &NormNode, text: &str) -> bool {
    match &node.label {
        Some(Label::Raw(t)) => t.as_ref() == text,
        // `External` carries only an `LSym` id, resolvable solely via the scan's
        // `LabelInterner` — not reachable here (this helper backs the trait's
        // `is_loop_core`/`is_continue_stmt`, whose signatures are fixed to
        // `&NormNode` only, so no interner can be threaded in). KNOWN LATENT GAP
        // (interning-id-conversion WP): callers reachable ONLY through the
        // `LanguageProfile` trait object (`au.rs`, `api.rs`, `normalize.rs::
        // remove_dead`, and this file's own `normalize_loop_exit`/`replace_breaks`)
        // therefore cannot recognize an ALREADY-lowered `true`/`break`/`continue`
        // marker once `abstract_idents` has turned its `Raw` label into `External`
        // (any Kotlin unit reaching these paths post-abstraction) — see
        // `ident_text_is_resolved` below for the fixed version used wherever a
        // `LabelInterner` IS reachable (`lower`, this file's iteration/nav helpers).
        Some(Label::External(_)) => false,
        _ => false,
    }
}

/// Like [`ident_text_is`], but resolves `Label::External` through `li` — use this
/// whenever a `LabelInterner` is reachable (unlike the trait-object-only call sites
/// `ident_text_is` still serves). Fixes a real idempotence bug (interning-id-
/// conversion WP): `lower_loops`'s own synthesized `true`/`continue` markers start
/// as `Label::Raw`, but a SECOND `apply_passes` pass (e.g. `tests/idempotence.rs`,
/// or any re-normalization of an already-normalized tree) sees them as `Label::
/// External` (abstract_idents already ran once) — `ident_text_is` alone would then
/// wrongly conclude "not yet lowered" and re-synthesize the break-guard, corrupting
/// the tree. This resolves through the CURRENT scan's interner instead, matching
/// the pre-conversion `Arc<str>`-handle design's self-resolving behavior exactly.
fn ident_text_is_resolved(node: &NormNode, text: &str, li: &crate::intern::LabelInterner) -> bool {
    match &node.label {
        Some(Label::Raw(t)) => t.as_ref() == text,
        Some(Label::External(sym)) => li.resolve(*sym) == text,
        _ => false,
    }
}

fn is_true_ident(node: &NormNode) -> bool {
    node.kind == *IDENT && ident_text_is(node, "true")
}

fn is_true_ident_resolved(node: &NormNode, li: &crate::intern::LabelInterner) -> bool {
    node.kind == *IDENT && ident_text_is_resolved(node, "true", li)
}

fn true_node(span: (u32, u32)) -> NormNode {
    synth_ident("true", Some("condition"), span)
}

fn eq_token(span: (u32, u32)) -> NormNode {
    NormNode::new("=", Some("operator"), span, Vec::new())
}

/// Relabel navigation targets (`.field`) as External — no field/kind positions
/// them, so a bare undeclared→External default would misfire when a target name
/// collides with a declared local.
fn externalize_nav(
    mut node: NormNode,
    label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
) -> NormNode {
    node.children = node
        .children
        .into_iter()
        .map(|c| externalize_nav(c, label_interner))
        .collect();
    if node.kind == *NAVIGATION_EXPRESSION
        && let Some(last) = node.children.last_mut()
        && last.kind == *IDENT
        && let Some(Label::Raw(t)) = &last.label
    {
        last.label = Some(Label::External(label_interner.intern(t)));
    }
    node
}

/// `if (!(cond)) { break }` in the manual shape (probe `cores`): Kotlin uses
/// `if_expression` with a fieldless `block` and a bare `identifier "break"`.
fn break_unless(mut cond: NormNode) -> NormNode {
    let span = cond.span;
    cond.field = Some("argument".into());
    let bang = NormNode::token("!", span);
    let negated = NormNode::new(
        "unary_expression",
        Some("condition"),
        span,
        vec![bang, cond],
    );
    let brk = synth_ident("break", None, span);
    let block = NormNode::new("block", None, span, vec![brk]);
    NormNode::new("if_expression", None, span, vec![negated, block])
}

fn call(
    name: &str,
    arg: NormNode,
    label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
) -> NormNode {
    synth_call(
        "call_expression",
        "value_arguments",
        name,
        None,
        arg,
        label_interner,
    )
}

/// The function body block, navigated by kind (`function_body` was spliced, so
/// the block is a direct child with no field).
fn body_block_idx(root: &NormNode) -> Option<usize> {
    root.children.iter().position(|c| c.kind == *BLOCK)
}

/// Lower while / do-while / for-in to the `while (true)` core (spec §5.2.2).
fn lower(
    mut node: NormNode,
    label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
) -> NormNode {
    node.children = node
        .children
        .into_iter()
        .map(|c| lower(c, label_interner))
        .collect();
    if node.kind == *WHILE_STATEMENT {
        // Already the core (`while (true)`) — don't re-lower. Uses the
        // interner-resolving check (a `LabelInterner` is available here): the
        // condition may be `Label::External` rather than `Raw` if this tree has
        // already been through one full `apply_passes` (idempotence — see
        // `ident_text_is_resolved`'s doc comment).
        if node
            .children
            .iter()
            .any(|c| c.field == Some(*CONDITION) && is_true_ident_resolved(c, label_interner))
        {
            return node;
        }
        let span = node.span;
        let field = node.field;
        let Some(cond) = node.take_field("condition") else {
            return node;
        };
        let Some(block_idx) = node.children.iter().position(|c| c.kind == *BLOCK) else {
            return node;
        };
        node.children[block_idx]
            .children
            .insert(0, break_unless(cond));
        let block = node.children.remove(block_idx);
        NormNode::with_kind(*WHILE_STATEMENT, field, span, vec![true_node(span), block])
    } else if node.kind == *DO_WHILE_STATEMENT {
        let span = node.span;
        let field = node.field;
        let Some(cond) = node.take_field("condition") else {
            return node;
        };
        let Some(block_idx) = node.children.iter().position(|c| c.kind == *BLOCK) else {
            return node;
        };
        node.children[block_idx].children.push(break_unless(cond)); // post-test
        let block = node.children.remove(block_idx);
        NormNode::with_kind(*WHILE_STATEMENT, field, span, vec![true_node(span), block])
    } else if node.kind == *FOR_STATEMENT {
        lower_for_in(node, label_interner)
    } else {
        node
    }
}

/// `for (x in xs) { body }` → `while (true) { if (!__has_next(xs)) break; x = __next(xs); body }`.
/// for_statement children (post-convert): variable_declaration, iterable, block.
fn lower_for_in(
    mut node: NormNode,
    label_interner: &std::sync::Arc<crate::intern::LabelInterner>,
) -> NormNode {
    let span = node.span;
    let field = node.field;
    let Some(block_idx) = node.children.iter().position(|c| c.kind == *BLOCK) else {
        return node;
    };
    let mut block = node.children.remove(block_idx);
    let Some(var_idx) = node
        .children
        .iter()
        .position(|c| c.kind == *VARIABLE_DECLARATION)
    else {
        return node;
    };
    let mut var = node.children.remove(var_idx);
    var.field = None;
    // The remaining non-block child is the iterable.
    let Some(iter_idx) = node.children.iter().position(|c| c.field.is_none()) else {
        return node;
    };
    let mut iterable = node.children.remove(iter_idx);
    iterable.field = None;
    let guard = break_unless(call("__has_next", iterable.clone(), label_interner));
    // Bind via `property_declaration` (a declaring form) so `collect_declared`
    // binds the loop variable as a local, like the natural `val x = …`.
    let next = call("__next", iterable, label_interner);
    let bind = NormNode::with_kind(*PROPERTY_DECLARATION, None, span, vec![var, next]);
    block.children.insert(0, guard);
    block.children.insert(1, bind);
    NormNode::with_kind(*WHILE_STATEMENT, field, span, vec![true_node(span), block])
}

// ---- iteration-protocol rewrite: `for (i in xs.indices)` / `0 until xs.size` ----

fn rewrite_iteration(mut node: NormNode) -> NormNode {
    node.children = node.children.into_iter().map(rewrite_iteration).collect();
    if node.kind != *FOR_STATEMENT {
        return node;
    }
    let Some((ivar, coll)) = index_for_match(&node) else {
        return node;
    };
    let span = node.span;
    let Some(block_idx) = node.children.iter().position(|c| c.kind == *BLOCK) else {
        return node;
    };
    let mut block = node.children.remove(block_idx);
    block = replace_index(block, &ivar, &coll);
    // Rebuild `for (ivar in coll) { block }` — lower_for_in handles the rest.
    let var = NormNode::with_kind(
        *VARIABLE_DECLARATION,
        None,
        span,
        vec![synth_ident(&ivar, None, span)],
    );
    let iterable = synth_ident(&coll, None, span);
    NormNode::with_kind(*FOR_STATEMENT, node.field, span, vec![var, iterable, block])
}

/// Matches `for (i in coll.indices)` or `for (i in 0 until coll.size)` whose
/// body uses `i` only as `coll[i]`. Returns (ivar, coll).
fn index_for_match(node: &NormNode) -> Option<(Box<str>, Box<str>)> {
    let var = node
        .children
        .iter()
        .find(|c| c.kind == *VARIABLE_DECLARATION)?;
    let ivar = match var.children.first().map(|c| (&c.kind, &c.label)) {
        Some((k, Some(Label::Raw(t)))) if *k == *IDENT => t.clone(),
        _ => return None,
    };
    // The iterable is the fieldless, non-block, non-variable child.
    let iterable = node
        .children
        .iter()
        .find(|c| c.field.is_none() && c.kind != *BLOCK && c.kind != *VARIABLE_DECLARATION)?;
    let coll = iterable_collection(iterable)?;
    let block = node.children.iter().find(|c| c.kind == *BLOCK)?;
    if !index_uses_only(block, &ivar, &coll) {
        return None;
    }
    Some((ivar, coll))
}

/// `coll.indices` or `0 until coll.size` → Some(coll).
fn iterable_collection(node: &NormNode) -> Option<Box<str>> {
    if node.kind == *NAVIGATION_EXPRESSION {
        // coll.indices
        let base = navigation_base(node, "indices")?;
        return Some(base);
    }
    if node.kind == *INFIX_EXPRESSION && node.children.len() == 3 {
        // 0 until coll.size
        if !matches!(&node.children[0].label, Some(Label::RawLit(t)) if t.as_ref() == "0") {
            return None;
        }
        if !ident_raw_or_ext(&node.children[1], "until") {
            return None;
        }
        if node.children[2].kind == *NAVIGATION_EXPRESSION {
            return navigation_base(&node.children[2], "size");
        }
    }
    None
}

/// `base.<suffix>` where base is a plain identifier → Some(base text).
fn navigation_base(nav: &NormNode, suffix: &str) -> Option<Box<str>> {
    if nav.children.len() != 2 {
        return None;
    }
    if !ident_raw_or_ext(&nav.children[1], suffix) {
        return None;
    }
    match (&nav.children[0].kind, &nav.children[0].label) {
        (k, Some(Label::Raw(t))) if *k == *IDENT => Some(t.clone()),
        _ => None,
    }
}

fn ident_raw_or_ext(node: &NormNode, text: &str) -> bool {
    node.kind == *IDENT && ident_text_is(node, text)
}

fn is_target_index(node: &NormNode, ivar: &str, coll: &str) -> bool {
    node.kind == *INDEX_EXPRESSION
        && node.children.len() == 2
        && is_raw_ident(&node.children[0], coll)
        && is_raw_ident(&node.children[1], ivar)
}

fn index_uses_only(node: &NormNode, ivar: &str, coll: &str) -> bool {
    if is_target_index(node, ivar, coll) {
        return true;
    }
    if is_raw_ident(node, ivar) {
        return false;
    }
    node.children.iter().all(|c| index_uses_only(c, ivar, coll))
}

fn replace_index(mut node: NormNode, ivar: &str, coll: &str) -> NormNode {
    if is_target_index(&node, ivar, coll) {
        let field = node.field.map(|f| f.as_str());
        return synth_ident(ivar, field, node.span);
    }
    node.children = node
        .children
        .into_iter()
        .map(|c| replace_index(c, ivar, coll))
        .collect();
    node
}

// ---- recursion lowering (spec §5.2.2 Rev 5) ----

fn lower_recursion(root: NormNode) -> NormNode {
    let Some(name) = raw_name(&root) else {
        return root;
    };
    let Some(params) = simple_params(&root) else {
        return root;
    };
    if params.is_empty() {
        return root;
    }
    let Some(body_idx) = body_block_idx(&root) else {
        return root;
    };
    let total = count_self_calls(&root.children[body_idx], &name);
    if total == 0 {
        return root;
    }
    let mut root = root;
    let mut replaced = 0u32;
    let new_body = rewrite_tail_sites(
        root.children[body_idx].clone(),
        &name,
        &params,
        &mut replaced,
    );
    if replaced != total {
        return root;
    }
    let span = new_body.span;
    let loop_node = NormNode::with_kind(
        *WHILE_STATEMENT,
        None,
        span,
        vec![true_node(span), new_body],
    );
    // Function body is a `block` holding the single while-core statement.
    let wrapper = NormNode::with_kind(*BLOCK, None, span, vec![loop_node]);
    root.children[body_idx] = wrapper;
    root
}

fn simple_params(root: &NormNode) -> Option<Vec<Box<str>>> {
    let params = root
        .children
        .iter()
        .find(|c| c.kind == *FUNCTION_VALUE_PARAMETERS)?;
    let mut out = Vec::new();
    for p in &params.children {
        if p.kind != *PARAMETER {
            return None;
        }
        // First identifier child is the name (type identifiers are nested).
        match p.children.first().map(|c| (&c.kind, &c.label)) {
            Some((k, Some(Label::Raw(t)))) if *k == *IDENT => out.push(t.clone()),
            _ => return None,
        }
    }
    Some(out)
}

/// Kotlin call callee is `children[0]` (no `function` field).
fn kt_is_self_call(node: &NormNode, name: &str) -> bool {
    node.kind == *CALL_EXPRESSION && node.children.first().is_some_and(|f| is_raw_ident(f, name))
}

fn count_self_calls(node: &NormNode, name: &str) -> u32 {
    let here = u32::from(kt_is_self_call(node, name));
    here + node
        .children
        .iter()
        .map(|c| count_self_calls(c, name))
        .sum::<u32>()
}

/// Nested callable kinds — a `return <self-call>` inside a nested local `fun`,
/// a lambda, or an anonymous function is the INNER callable's tail position, not
/// the outer unit's. Descending would emit `continue` outside any loop and
/// reassign the outer params, while the local `count_self_calls` census still
/// counts the nested call (`replaced == total`) so the bad lowering would
/// commit. Stopping descent leaves the nested call uncounted → `replaced !=
/// total` → the unit safely bails out of recursion lowering.
fn is_nested_callable(kind: Kind) -> bool {
    kind == *FUNCTION_DECLARATION || kind == *LAMBDA_LITERAL || kind == *ANONYMOUS_FUNCTION
}

fn rewrite_tail_sites(
    mut node: NormNode,
    name: &str,
    params: &[Box<str>],
    replaced: &mut u32,
) -> NormNode {
    if is_nested_callable(node.kind) {
        return node; // a self-call inside a nested closure is not a tail site
    }
    if node.kind == *BLOCK {
        let mut out = Vec::with_capacity(node.children.len());
        for child in node.children {
            // `return f(...)`: return_expression { call_expression }
            let is_return_site = child.kind == *RETURN_EXPRESSION
                && child.children.len() == 1
                && kt_is_self_call(&child.children[0], name);
            if is_return_site {
                let call = child.children.into_iter().next().unwrap();
                out.extend(reassign_stmts(call, params, replaced));
            } else {
                out.push(rewrite_tail_sites(child, name, params, replaced));
            }
        }
        node.children = out;
        node
    } else {
        node.children = node
            .children
            .into_iter()
            .map(|c| rewrite_tail_sites(c, name, params, replaced))
            .collect();
        node
    }
}

/// `p_i = a_i` (one assignment per changed param — Kotlin has no simultaneous
/// multi-assignment; the sequential form is unsound for coupled updates but
/// tolerated for detection, DECISIONS.md D23) then `continue`.
fn reassign_stmts(call: NormNode, params: &[Box<str>], replaced: &mut u32) -> Vec<NormNode> {
    let span = call.span;
    let args: Vec<NormNode> = call
        .children
        .iter()
        .find(|c| c.kind == *VALUE_ARGUMENTS)
        .map(|a| a.children.iter().map(unwrap_arg).collect())
        .unwrap_or_default();
    if args.len() != params.len() {
        *replaced += 1;
        return vec![continue_ident(span)];
    }
    *replaced += 1;
    let mut out = Vec::new();
    for (i, arg) in args.into_iter().enumerate() {
        if is_raw_ident(&arg, &params[i]) {
            continue; // identity: skip
        }
        let mut arg = arg;
        arg.field = Some("right".into());
        let left = synth_ident(&params[i], Some("left"), span);
        out.push(NormNode::with_kind(
            *ASSIGNMENT,
            None,
            span,
            vec![left, eq_token(span), arg],
        ));
    }
    out.push(continue_ident(span));
    out
}

/// `value_argument { <expr> }` → the inner expr (call args are wrapped).
fn unwrap_arg(node: &NormNode) -> NormNode {
    if node.kind == *VALUE_ARGUMENT && node.children.len() == 1 {
        node.children[0].clone()
    } else {
        node.clone()
    }
}

fn continue_ident(span: (u32, u32)) -> NormNode {
    synth_ident("continue", None, span)
}

// ---- loop-exit normalization ----

fn normalize_loop_exit(profile: &KotlinProfile, mut root: NormNode) -> NormNode {
    let Some(body_idx) = body_block_idx(&root) else {
        return root;
    };
    let body = &mut root.children[body_idx];
    let n = body.children.len();
    if n < 2 {
        return root;
    }
    let ret_value: Option<NormNode> = {
        let last = &body.children[n - 1];
        if last.kind == *RETURN_EXPRESSION && last.children.len() == 1 {
            Some(last.children[0].clone())
        } else {
            None
        }
    };
    let Some(value) = ret_value else {
        return root;
    };
    let loop_stmt = &mut body.children[n - 2];
    if !profile.is_loop_core(loop_stmt) {
        return root;
    }
    let mut replaced = 0u32;
    replace_breaks(profile, loop_stmt, &value, &mut replaced, true);
    if replaced > 0 {
        body.children.truncate(n - 1);
    }
    root
}

fn replace_breaks(
    profile: &KotlinProfile,
    node: &mut NormNode,
    value: &NormNode,
    replaced: &mut u32,
    top: bool,
) {
    if !top && profile.is_loop_core(node) {
        return;
    }
    let mut i = 0;
    while i < node.children.len() {
        if node.children[i].kind == *IDENT && ident_text_is(&node.children[i], "break") {
            let span = node.children[i].span;
            let mut v = value.clone();
            v.field = None;
            node.children[i] = NormNode::with_kind(*RETURN_EXPRESSION, None, span, vec![v]);
            *replaced += 1;
        } else {
            replace_breaks(profile, &mut node.children[i], value, replaced, false);
        }
        i += 1;
    }
}
