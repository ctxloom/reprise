//! Language support behind a trait (spec §3): a language is one
//! `LanguageProfile` impl plus grammar hooks. Matching is same-language only;
//! each profile owns its node-kind space with no cross-language obligations.

pub mod go;
pub mod kotlin;
pub mod python;
pub mod rust;
pub mod typescript;

use crate::tree::{Bucket, Label, NormNode};
use std::path::Path;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum Lang {
    Rust,
    Python,
    /// TypeScript (`.ts`) — uses the `tree-sitter-typescript` TS dialect.
    TypeScript,
    /// TSX (`.tsx`) — same grammar family, TSX dialect. A separate partition
    /// from `.ts` (same profile, distinct grammar object — DECISIONS.md D23).
    Tsx,
    Go,
    Kotlin,
}

impl Lang {
    pub fn from_path(path: &Path) -> Option<Lang> {
        match path.extension()?.to_str()? {
            "rs" => Some(Lang::Rust),
            "py" => Some(Lang::Python),
            "ts" => Some(Lang::TypeScript),
            "tsx" => Some(Lang::Tsx),
            "go" => Some(Lang::Go),
            "kt" | "kts" => Some(Lang::Kotlin),
            _ => None,
        }
    }

    pub fn ts_language(self) -> tree_sitter::Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
            Lang::Kotlin => tree_sitter_kotlin_ng::LANGUAGE.into(),
        }
    }

    pub fn profile(self) -> &'static dyn LanguageProfile {
        match self {
            Lang::Rust => &rust::RustProfile,
            Lang::Python => &python::PythonProfile,
            // Both TS dialects share one profile (identical node-kind space).
            Lang::TypeScript | Lang::Tsx => &typescript::TypeScriptProfile,
            Lang::Go => &go::GoProfile,
            Lang::Kotlin => &kotlin::KotlinProfile,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::Python => "python",
            Lang::TypeScript => "typescript",
            Lang::Tsx => "tsx",
            Lang::Go => "go",
            Lang::Kotlin => "kotlin",
        }
    }

    /// The canonical AND-operator spelling this language's source produces (Python `and`, every
    /// other language `&&`). The IR preserves the surface boolean token (see
    /// `LanguageProfile::commutative_ops` — Python has `and`/`or`, Rust/Go `&&`/`||`), so a
    /// synthesized conjunction (the C1 guard merge, `ir::pass::try_merge_branch`) must use the
    /// SAME token the language's hand-written `a && b` / `a and b` lowers to, or the
    /// "nesting ≡ conjunction" convergence never fires (askew-taps). Used when the merged subtree
    /// carries no boolean operator to sniff (a pure-atomic `if a: if b:`), so the token must come
    /// from the language rather than the tree.
    pub fn conjunction_token(self) -> &'static str {
        match self {
            Lang::Python => "and",
            _ => "&&",
        }
    }
}

pub trait LanguageProfile: Sync {
    /// Node kinds extracted as comparison units (spec §5.1).
    fn is_function_like(&self, kind: &str) -> bool;

    /// A named function binding extracted as a unit *in addition* to
    /// `is_function_like` nodes: languages where a callable is an expression
    /// bound to a name in a declaration rather than a declaration itself —
    /// TS `const f = (x) => …` / `const f = function () {…}`. Returns the
    /// bound name and the callable node (which is then converted as the unit
    /// root). Decl-only by construction: only recognize binding forms, so
    /// inline callbacks (`xs.map(x => …)`) are never extracted. Default: none
    /// (most languages declare functions directly, caught by is_function_like).
    fn binding_unit<'tree>(
        &self,
        _node: tree_sitter::Node<'tree>,
        _src: &str,
    ) -> Option<(String, tree_sitter::Node<'tree>)> {
        None
    }

    fn is_comment(&self, kind: &str) -> bool;

    /// Node kinds stripped wholesale during conversion (attributes, decorators,
    /// mutability specifiers, lifetimes — spec §5.2.1/§5.2.7).
    fn strip_kind(&self, kind: &str) -> bool;

    /// Identifier-carrying leaf kinds (become `Label::Raw`).
    fn is_identifier(&self, kind: &str) -> bool;

    /// Literal leaf kinds and their bucket (spec §5.2.5).
    fn literal_bucket(&self, kind: &str) -> Option<Bucket>;

    /// Wrapper kinds flattened away (redundant parens — spec §5.2.7).
    fn unwrap_kind(&self, kind: &str) -> bool;

    /// Kinds whose children are spliced directly into the parent during
    /// conversion, dropping the wrapper node (Go's `statement_list` inside
    /// `block`, so Go blocks hold statements directly like Rust/Python).
    /// Default: never splice.
    fn splice_kind(&self, _kind: &str) -> bool {
        false
    }

    /// Dispatch-arm kinds (match/case arms). Repeated dispatch arms still
    /// FOLD (the REPEAT canonicalization is load-bearing for cross-unit
    /// convergence) but do not emit `internal-repeat` findings: an enum→value
    /// table is idiomatic, not actionable duplication (D30; the M4a §7.2
    /// sample labeled every sampled match-arm repeat FP). Only Rust and
    /// Python implement this — the other profiles' switch bodies are not
    /// fold list kinds, so their arms can never fold in the first place.
    fn is_dispatch_arm(&self, _kind: &str) -> bool {
        false
    }

    /// Parents under which anonymous tokens are semantic (operators) and kept.
    fn keep_anon_parent(&self, parent_kind: &str) -> bool;

    /// Identifier positions that are always external regardless of the
    /// declared set (field names, attribute access, type names).
    fn always_external(&self, kind: &str, field: Option<&str>, parent_kind: &str) -> bool;

    /// Collect names syntactically declared within the unit (spec §5.2.4).
    /// Runs after loop lowering, on `Label::Raw` trees.
    fn collect_declared(&self, root: &NormNode, out: &mut Vec<Box<str>>);

    /// Loop decomposition into the per-language minimal core (spec §5.2.2).
    fn lower_loops(&self, node: NormNode) -> NormNode;

    // ---- Phase 2 ----

    /// Linear/tail recursion → loop core (spec §5.2.2 Rev 5). Runs first,
    /// while the unit's own name is still a `Raw` label.
    fn lower_recursion(&self, root: NormNode) -> NormNode;

    /// Iteration-protocol rewrite: index-over-length → iterated form (§5.2.2).
    /// Runs before loop lowering (needs intact `for` nodes).
    fn rewrite_iteration(&self, root: NormNode) -> NormNode;

    /// Loop-exit normalization: trailing `break`+`return E` → `return E` at the
    /// break site (§5.2.2 companion rule). Runs after loop lowering.
    fn normalize_loop_exit(&self, root: NormNode) -> NormNode;

    /// Commutative operator token kinds (spec §5.2.6; unsoundness accepted).
    fn commutative_ops(&self) -> &'static [&'static str];

    /// (left_field, op_field, right_field) for rebuildable binary kinds.
    fn binary_fields(
        &self,
        kind: &str,
    ) -> Option<(
        Option<&'static str>,
        Option<&'static str>,
        Option<&'static str>,
    )>;

    /// Kind whose children are sortable key/value pairs → the key field name.
    fn sortable_pair_kind(&self, kind: &str) -> Option<&'static str>;

    /// Kinds whose children form a variable-length list (AU aligns these with
    /// graded Smith-Waterman rather than positionally — spec §5.6).
    fn is_list_kind(&self, kind: &str) -> bool;

    /// Statement-like kinds (factorability classification — spec §5.6).
    fn is_statement_kind(&self, kind: &str) -> bool;

    /// Is this node the lowered loop core (`loop`/`while True`)?
    fn is_loop_core(&self, node: &NormNode) -> bool;

    /// Is this child a trailing `continue` (redundant at loop-body tail)?
    fn is_continue_stmt(&self, node: &NormNode) -> bool;

    /// Test-unit recognition (spec §5.1 test-code policy).
    fn unit_is_test(&self, node: tree_sitter::Node, src: &str, name: &str, path: &Path) -> bool;

    // ---- Phase 3: best-effort inliner (spec §5.4) ----

    /// The language's plain-call node kind (`call_expression` / `call`).
    fn call_kind(&self) -> &'static str;

    /// Simple-identifier parameter names of a raw unit, or None when patterns,
    /// defaults, or receivers make the unit non-inlinable by substitution.
    fn inline_params(&self, root: &NormNode) -> Option<Vec<Box<str>>>;

    /// If `stmt` is a value-returning `return <expr>` statement, the expr.
    fn return_value<'a>(&self, stmt: &'a NormNode) -> Option<&'a NormNode>;

    /// Statement-shaped `return <value>`.
    fn make_return(&self, value: NormNode) -> NormNode;

    /// Statement-shaped bare-expression statement.
    fn make_expr_stmt(&self, expr: NormNode) -> NormNode;

    /// Expression-position block for a multi-statement splice; None when the
    /// language has no native expression block (such sites are then skipped —
    /// DECISIONS.md D17).
    fn make_expr_block(&self, span: (u32, u32), children: Vec<NormNode>) -> Option<NormNode>;
}

/// Shared helper: synthesize a call node with an External callee, using the
/// language's native call/argument-list kinds. (Consolidated from per-language
/// copies — reprise's own first self-scan finding.)
pub(crate) fn synth_call(
    call_kind: &str,
    args_kind: &str,
    name: &str,
    field: Option<&str>,
    arg: NormNode,
) -> NormNode {
    let span = arg.span;
    let callee = NormNode::new("identifier", Some("function"), span, Vec::new())
        .with_label(crate::tree::Label::External(name.into()));
    let args = NormNode::new(args_kind, Some("arguments"), span, vec![arg]);
    NormNode::new(call_kind, field, span, vec![callee, args])
}

/// Shared helper: synthesize a `Raw` identifier leaf (consolidated from
/// per-language copies — another self-scan finding).
pub(crate) fn synth_ident(name: &str, field: Option<&str>, span: (u32, u32)) -> NormNode {
    NormNode::new("identifier", field, span, Vec::new()).with_label(Label::Raw(name.into()))
}

/// Shared helper: collect `Raw` identifier texts in a pattern-like subtree,
/// skipping type annotations (subtrees in a `type` field).
pub(crate) fn collect_pattern_idents(node: &NormNode, out: &mut Vec<Box<str>>) {
    if node.field.as_deref() == Some("type") {
        return;
    }
    if let Some(crate::tree::Label::Raw(text)) = &node.label
        && node.kind.as_ref() == "identifier"
    {
        out.push(text.clone());
    }
    for child in &node.children {
        collect_pattern_idents(child, out);
    }
}

pub(crate) fn child_field<'a>(node: &'a NormNode, field: &str) -> Option<&'a NormNode> {
    node.children
        .iter()
        .find(|c| c.field.as_deref() == Some(field))
}

/// The unit's own name while still un-abstracted; None once passes ran.
pub(crate) fn raw_name(root: &NormNode) -> Option<Box<str>> {
    match &child_field(root, "name")?.label {
        Some(Label::Raw(text)) => Some(text.clone()),
        _ => None,
    }
}

pub(crate) fn is_raw_ident(node: &NormNode, text: &str) -> bool {
    node.kind.as_ref() == "identifier"
        && matches!(&node.label, Some(Label::Raw(t)) if t.as_ref() == text)
}

/// Count calls to `name` anywhere in the subtree (self-call census for the
/// linear-recursion guard: tree recursion must NOT lower).
pub(crate) fn count_self_calls(node: &NormNode, call_kind: &str, name: &str) -> u32 {
    let mut n = 0;
    if node.kind.as_ref() == call_kind
        && let Some(f) = child_field(node, "function")
        && is_raw_ident(f, name)
    {
        n += 1;
    }
    node.children
        .iter()
        .map(|c| count_self_calls(c, call_kind, name))
        .sum::<u32>()
        + n
}

/// Path-based test heuristics shared by all languages.
pub(crate) fn path_is_testy(path: &Path) -> bool {
    path.components().any(|c| {
        matches!(
            c.as_os_str().to_str(),
            Some("tests") | Some("test") | Some("testing")
        )
    })
}
