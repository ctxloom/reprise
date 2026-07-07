//! Language frontends: tree-sitter CST → canonical IR ([`crate::ir`]).
//!
//! **Distinct from the historical `src/lang/`.** Those profiles lower to *per-grammar*
//! node kinds and each re-implements the full normalization (loop lowering, recursion,
//! identifier abstraction, …) — the parallel-implementation duplication the IR exists to
//! kill. A frontend here lowers *directly to the shared canonical IR*, so the
//! normalization **algorithms live once** ([`crate::ir::pass`]) and each language is a
//! thin CST-kind → canonical-kind *mapping* (see [`rust`], [`python`]). Only the mapping
//! and the parameter shape are per-language; recursion, node synthesis (break-guard,
//! arms), literal bucketing, and every pass are shared below.

pub mod go;
pub mod python;
pub mod rust;

pub use go::{lower_go, lower_go_source};
pub use python::{lower_python, lower_python_source};
pub use rust::{lower_rust, lower_rust_source};

use crate::ir::kind;
use crate::ir::transform::{TransformKind, TransformLog, Witness};
use crate::lang::Lang;
use crate::tree::{Bucket, Label, NormNode};
use tree_sitter::Node;

/// A per-language CST → canonical-IR mapping — the only per-language surface.
pub(crate) trait Frontend {
    /// Map one CST node to a canonical IR node (or `None` if dropped). **Implement this,
    /// but never call it directly** — every recursion goes through [`lower_node`](Frontend::lower_node),
    /// the shared wrapper that enforces the DoS depth guard.
    fn lower_node_inner(
        &self,
        node: Node,
        field: Option<&str>,
        src: &str,
        log: &mut TransformLog,
    ) -> Option<NormNode>;

    /// The recursion entry point every helper and frontend calls: a thin wrapper over
    /// [`lower_node_inner`](Frontend::lower_node_inner) that enforces the extraction depth
    /// guard (the untrusted-input DoS fix). tree-sitter's own parse is iterative and cannot
    /// overflow our stack, but this recursive descent of the CST can — a several-thousand-
    /// term left-assoc operator chain or deeply nested literals in a generated/minified file
    /// would exhaust a rayon worker's 2 MiB stack and SIGSEGV the whole scan. Past
    /// [`crate::normalize::MAX_EXTRACTION_DEPTH`] we stop descending and truncate the subtree
    /// (returning `None`, the same value a dropped node yields — callers already skip it),
    /// marking the unit truncated so it is flagged `parse_degraded`, never silently dropped
    /// (spec §5.1/§12). Every downstream walk (passes, fold, fingerprint) then operates on a
    /// tree whose depth is bounded by the cap, so none of them can overflow either.
    fn lower_node(
        &self,
        node: Node,
        field: Option<&str>,
        src: &str,
        log: &mut TransformLog,
    ) -> Option<NormNode> {
        if log.depth() >= crate::normalize::MAX_EXTRACTION_DEPTH {
            log.mark_truncated();
            return None;
        }
        log.enter();
        let result = self.lower_node_inner(node, field, src, log);
        log.leave();
        result
    }

    /// Push the unit's parameters as `Var@param` (binding shape differs per grammar).
    fn lower_params(
        &self,
        params: Node,
        src: &str,
        log: &mut TransformLog,
        out: &mut Vec<NormNode>,
    );

    /// A wrapper kind whose children are spliced directly into the parent statement
    /// list (Go's `statement_list` inside `block`), so blocks hold statements directly
    /// — the canonical shape every shared helper assumes. Mirrors the historical
    /// `LanguageProfile::splice_kind` (D23). Default: never splice.
    fn splice_kind(&self, _kind: &str) -> bool {
        false
    }

    /// A statement that lowers to *several* canonical statements spliced into the
    /// enclosing block — the one shape a single-node `lower_node` cannot express.
    /// The only user is Go's C-style `for init; cond; post {…}`, whose `init` must
    /// become a sibling *before* the `Loop` (mirrors the historical
    /// `expand_for_clauses`). Default: none.
    fn expand_stmt(
        &self,
        _node: Node,
        _src: &str,
        _log: &mut TransformLog,
    ) -> Option<Vec<NormNode>> {
        None
    }

    /// Lower the function body block, applying any language-specific return-position handling.
    /// Default: lower the `body` block as-is (Python/Go require an explicit `return`). The Rust
    /// frontend overrides this so the body's tail expression — Rust's implicit return — lowers
    /// to an explicit `Return`, converging `fn f() -> T { … expr }` with `{ … return expr; }`.
    fn lower_fn_body(
        &self,
        node: Node,
        span: (u32, u32),
        src: &str,
        log: &mut TransformLog,
    ) -> NormNode {
        node.child_by_field_name("body")
            .and_then(|b| self.lower_node(b, Some("body"), src, log))
            .unwrap_or_else(|| NormNode::new(kind::BLOCK, Some("body"), span, Vec::new()))
    }
}

// ---- data-driven dispatch table (D-SP4-1b; docs/sp4-grammar-lift-plan.md §4) ----

/// One entry of a frontend's `lower_node` dispatch table: **which shared lowering handles a
/// CST kind**, plus the per-language CST field names / literal bucket that lowering needs.
/// Each variant names one shared helper in this module; the [`dispatch`] runner turns an
/// entry back into that helper's call.
///
/// This is the SP4 grammar-lift's single source of truth. A frontend's
/// `const MAP: &[(&str, Lowering)]` makes every dispatched CST kind enumerable **as data**,
/// so a future conformance gate (increment 5) can read the *same* table the runtime dispatch
/// reads and assert every referenced kind still exists in the linked tree-sitter grammar —
/// turning today's silent grammar drift (a renamed kind falls through to `native()` with a
/// green build) into a loud, localized test failure. Because the table *is* the dispatch,
/// completeness is structural rather than a parallel hand-maintained list. The irreducible
/// per-language quirks the table cannot express stay hand-written residue, their kinds
/// registered separately but still as data.
///
/// Reused unchanged by every frontend — only a table's *contents* are per-language; the
/// vocabulary of shared lowerings is shared (Rust adopts it here; Python/Go in later
/// increments).
/// The one lowering-fn signature every shared [`super`] helper and the [`dispatch`] runner already
/// use — and the pointee of [`Lowering::Std`]. A language-local fn joins the table by matching it
/// (taking `&dyn Frontend` in place of its own concrete frontend); nothing else about it changes.
pub(crate) type LowerFn =
    fn(&dyn Frontend, Node, Option<&str>, (u32, u32), &str, &mut TransformLog) -> NormNode;

#[derive(Clone, Copy)]
pub(crate) enum Lowering {
    /// [`lower_function`] — a function/method unit.
    Function,
    /// [`block`] — a statement block.
    Block,
    /// [`lower_loop`] — an already-canonical infinite loop.
    Loop,
    /// [`lower_while`] — a `while` (break-guard synthesis).
    While,
    /// [`lower_if`] — an `if`/`else` conditional.
    If,
    /// [`lower_for`] with the `(pattern, iterable)` CST field names.
    For(&'static str, &'static str),
    /// [`lower_unary_positional`] — `<op> operand` with a positional operator token.
    UnaryPositional,
    /// [`lower_return`].
    Return,
    /// [`lower_call`] with the `(callee, arguments)` CST field names.
    Call(&'static str, &'static str),
    /// [`lower_field`] with the `(base, name)` CST field names.
    Field(&'static str, &'static str),
    /// [`lower_binary`] with the `(left, operator, right)` CST field names.
    Binary(&'static str, &'static str, &'static str),
    /// [`lower_aug_assign`] with `(left, operator, right, target_is_binding)`.
    AugAssign(&'static str, &'static str, &'static str, bool),
    /// [`lit`] into the given typed bucket.
    Lit(Bucket),
    /// [`leaf`] — a childless canonical node of the given `kind::*` (e.g. `Break`/`Continue`).
    Leaf(&'static str),
    /// [`var`] — a local read (`Raw`-labelled, later abstracted).
    Var,
    /// [`ext_name`] — a structurally-external name (field/type/package), never a local.
    ExtName,
    /// [`unwrap_stmt`] — splice through a wrapper to its first lowerable child (`Option`).
    Unwrap,
    /// [`drop_parens`] — drop redundant parentheses, recording `ParenDrop` (`Option`).
    DropParens,
    /// A **language-local** lowering fn riding *in* the table (D-SP4-2; `docs/sp4-grammar-lift-plan.md`).
    /// Increment 1 could only express a CST kind as data when a *shared* [`super`] helper handled it;
    /// a frontend's own quirk-lowering (Python's `lower_if_py`/`lower_match_py`/…) had to stay a
    /// hand-written residue arm. `Std` closes that gap: it carries a function pointer to the
    /// per-language fn, so the entry is still a MAP key (gate-enumerable) while the *behavior* is the
    /// frontend's. The pointee is the **exact** signature every shared helper and the [`dispatch`]
    /// runner already use — `fn(&dyn Frontend, node, field, span, src, log) -> NormNode` — so a
    /// language-local fn joins the table by taking `&dyn Frontend` (not its own concrete frontend)
    /// and nothing else changes. A `NormNode` (never `Option`) is returned: an `Std` entry always
    /// lowers to a node, so like the other node-returning variants its result is `Some`-wrapped by
    /// [`dispatch`]; irreducible `Option`-returning / inline quirks stay residue.
    Std(LowerFn),
    /// The node carries no similarity signal — dropped (`None`).
    Drop,
}

/// Look up a CST `kind` in a frontend's dispatch table. `Some` ⇒ the table handles this kind
/// (feed it to [`dispatch`]); `None` ⇒ a table *miss* the frontend resolves via its own
/// hand-written residue (or a `native` leaf). Kept distinct from [`dispatch`] precisely so a
/// table *hit* that legitimately drops the node ([`Lowering::Drop`] → `None`) is never
/// confused with a miss.
pub(crate) fn lookup<'a>(map: &'a [(&str, Lowering)], kind: &str) -> Option<&'a Lowering> {
    map.iter().find(|(k, _)| *k == kind).map(|(_, l)| l)
}

/// Run one dispatch-table entry: reconstruct the shared-helper call the [`Lowering`] names,
/// keyed off `node`'s span. A `None` result is a *lowered-away* node (a dropped/spliced
/// wrapper), distinct from a table miss (see [`lookup`]). The single dispatch runner every
/// frontend shares — the runtime half of the table that is the gate's source of truth.
pub(crate) fn dispatch(
    entry: &Lowering,
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    src: &str,
    log: &mut TransformLog,
) -> Option<NormNode> {
    let span = span_of(node);
    match *entry {
        Lowering::Function => Some(lower_function(fe, node, field, span, src, log)),
        Lowering::Block => Some(block(fe, node, field, span, src, log)),
        Lowering::Loop => Some(lower_loop(fe, node, field, span, src, log)),
        Lowering::While => Some(lower_while(fe, node, field, span, src, log)),
        Lowering::If => Some(lower_if(fe, node, field, span, src, log)),
        Lowering::For(pat, iter) => Some(lower_for(fe, node, field, span, (pat, iter), src, log)),
        Lowering::UnaryPositional => Some(lower_unary_positional(fe, node, field, span, src, log)),
        Lowering::Return => Some(lower_return(fe, node, field, span, src, log)),
        Lowering::Call(f, a) => Some(lower_call(fe, node, field, span, (f, a), src, log)),
        Lowering::Field(b, n) => Some(lower_field(fe, node, field, span, (b, n), src, log)),
        Lowering::Binary(l, o, r) => Some(lower_binary(fe, node, field, span, (l, o, r), src, log)),
        Lowering::AugAssign(l, o, r, bind) => Some(lower_aug_assign(
            fe,
            node,
            (l, o, r, bind),
            field,
            span,
            src,
            log,
        )),
        Lowering::Lit(bucket) => Some(lit(node, field, span, bucket, src, log)),
        Lowering::Leaf(k) => Some(leaf(k, field, span)),
        Lowering::Var => Some(var(node, field, span, src)),
        Lowering::ExtName => Some(ext_name(node, field, span, src)),
        Lowering::Unwrap => unwrap_stmt(fe, node, field, src, log),
        Lowering::DropParens => drop_parens(fe, node, field, span, src, log),
        // A language-local fn-ptr: call it with the same args every shared helper gets.
        Lowering::Std(f) => Some(f(fe, node, field, span, src, log)),
        Lowering::Drop => None,
    }
}

/// Lower one function CST node to canonical IR, recording every lowering-time
/// normalization into `log` (D-IR-12 construct-and-record). The caller owns the sink:
/// a [`TransformLog::disabled`] one makes recording a zero-cost no-op without changing
/// the tree (the bulk scan's read-model selector, `docs/transform-seam.md` §3).
pub(crate) fn lower_unit(
    fe: &dyn Frontend,
    node: Node,
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    fe.lower_node(node, None, src, log)
        .unwrap_or_else(|| NormNode::new(kind::UNIT, None, span_of(node), Vec::new()))
}

pub(crate) fn lower_source(
    fe: &dyn Frontend,
    lang: Lang,
    root_kind: &str,
    src: &str,
    log: &mut TransformLog,
) -> Option<NormNode> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang.ts_language()).ok()?;
    let tree = parser.parse(src, None)?;
    let func = find_kind(tree.root_node(), root_kind, 0)?;
    Some(lower_unit(fe, func, src, log))
}

/// The **IR normalizer plugin** (selected via `[normalize] normalizer = "ir"`):
/// lower a function CST node to canonical IR and run the language-agnostic passes,
/// producing the normalized tree the matching pipeline consumes (D-IR-1a).
///
/// The `log` sink is threaded through both lowering and the pass fold (D-IR-12 /
/// `docs/transform-seam.md` §3), so the caller picks the read model: the bulk scan
/// passes [`TransformLog::disabled`] (zero recording cost), while a reporter /
/// calibration consumer passes [`TransformLog::new`] to fold the complete event stream.
/// The log is hash-excluded, so the canonical tree is byte-identical either way.
pub fn normalize(lang: Lang, node: Node, src: &str, log: &mut TransformLog) -> NormNode {
    run_passes(lower_unit_for(lang, node, src, log), lang, log)
}

/// Lower one function CST node to canonical IR for `lang` (no passes yet) — the
/// pre-abstraction `Raw`-labelled tree. The inline phase (spec §5.4) consumes this
/// form so callee-body substitution keys on real names, exactly as the historical
/// path splices pre-`apply_passes` raw trees ([`crate::unit`]).
pub(crate) fn lower_unit_for(
    lang: Lang,
    node: Node,
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    match lang {
        Lang::Rust => lower_unit(&rust::Rust, node, src, log),
        Lang::Python => lower_unit(&python::Python, node, src, log),
        Lang::Go => lower_unit(&go::Go, node, src, log),
        // Remaining frontends (TS, Kotlin) fall back to an empty unit until built.
        _ => NormNode::new(kind::UNIT, None, span_of(node), Vec::new()),
    }
}

/// Run the language-agnostic canonicalization passes on a lowered IR tree — the
/// second half of [`normalize`], factored out so the inline phase can re-run it on a
/// spliced (still `Raw`) variant, converging it with the frontend's own canonical form.
pub fn run_passes(tree: NormNode, lang: Lang, log: &mut TransformLog) -> NormNode {
    // Every canonicalization pass runs through the D-IR-12 detect/emit seam: each detector
    // takes an immutable tree and cannot mutate, and `apply` is the sole mutator — it performs
    // the edits AND records their events. Order is load-bearing (§4.1): each detector runs on
    // the prior step's output so loci stay valid. Two placements are pinned:
    //   * Family C (`detect_guard_canonicalize`) runs BEFORE `abstract_idents` — it moves whole
    //     guard/body subtrees, so keeping its loci on `Raw` labels matches the other structural
    //     passes — and BEFORE Family A, whose A2 then normalizes negations inside the `a && b`
    //     guards C1 synthesizes.
    //   * Family A (`detect_boolean_normalize`) runs AFTER `abstract_idents` (label-independent,
    //     so `detect_comm_sort`'s abstracted-label sort key is unchanged) and BEFORE
    //     `detect_comm_sort` (A1 orients asymmetric comparisons and A2's De Morgan synthesizes
    //     fresh `&&`/`||` chains, both of which comm-sort then sorts).
    // `detect_comm_sort` must therefore stay post-abstraction and last-but-one.
    let iter = crate::ir::pass::detect_iter_protocol(&tree);
    let tree = crate::ir::edit::apply(tree, &iter, log);
    // The C-style counter loop (block-level `i=0` + `while i<len(coll)` + `i=i+1`) is the same
    // iteration as the range form — rewritten here to the identical canonical foreach shape, so
    // Go counter ≡ Go range ≡ Rust foreach ≡ Rust while-index. Disjoint from `detect_iter_protocol`
    // (a counter loop has no `__next` bind; a range loop has no init sibling), so it runs adjacent.
    let counter = crate::ir::pass::detect_counter_iter(&tree);
    let tree = crate::ir::edit::apply(tree, &counter, log);
    let loop_exit = crate::ir::pass::detect_loop_exit(&tree);
    let tree = crate::ir::edit::apply(tree, &loop_exit, log);
    let guard = crate::ir::pass::detect_guard_canonicalize(&tree, lang);
    let tree = crate::ir::edit::apply(tree, &guard, log);
    // Parallel multi-assign decomposition (§13, multi-assign rung only): `a, b = X, Y` and the
    // parallel Assign a tail-rec reassignment lowers to both reduce to a minimal single-assign
    // sequence here, so they converge with the equivalent adjacent single assigns. Runs BEFORE
    // `abstract_idents` so its synthetic cycle-breaking temps (`@target` binds) become positional
    // locals, and after the structural control-flow passes settle.
    let multi = crate::ir::pass::detect_multi_assign(&tree);
    let tree = crate::ir::edit::apply(tree, &multi, log);
    let abstract_idents = crate::ir::pass::detect_abstract_idents(&tree);
    let tree = crate::ir::edit::apply(tree, &abstract_idents, log);
    let boolean = crate::ir::pass::detect_boolean_normalize(&tree);
    let tree = crate::ir::edit::apply(tree, &boolean, log);
    let comm_sort = crate::ir::pass::detect_comm_sort(&tree);
    let tree = crate::ir::edit::apply(tree, &comm_sort, log);
    let dead = crate::ir::pass::detect_dead(&tree);
    crate::ir::edit::apply(tree, &dead, log)
}

/// One IR-normalized unit: metadata + the canonical tree, plus the pre-pass
/// lowered (`Raw`-labelled) tree the inline phase splices (spec §5.4).
pub struct IrUnit {
    pub name: String,
    pub byte_span: (u32, u32),
    pub line_span: (u32, u32),
    pub parse_degraded: bool,
    pub is_test: bool,
    pub tree: NormNode,
    /// Lowered, pre-abstraction form (the inliner's raw tree — see [`lower_for`]).
    pub raw: NormNode,
}

/// The CST kinds that seed an IR unit for `lang` (a language may declare functions
/// under more than one kind — Go's `function_declaration` + `method_declaration`).
/// Empty ⇒ no IR frontend yet (TS, Kotlin).
fn ir_root_kinds(lang: Lang) -> &'static [&'static str] {
    match lang {
        Lang::Rust => &["function_item"],
        Lang::Python => &["function_definition"],
        Lang::Go => &["function_declaration", "method_declaration"],
        _ => &[],
    }
}

/// Whether an IR frontend exists for `lang`. When `[normalize] normalizer = "ir"` is
/// selected but a language has no frontend yet (TS, Kotlin), extraction falls back to
/// the historical normalizer so those files still scan (a per-language capability gate,
/// not a global switch — the phased rollout, `docs/SIMILARITY-IR.md` §9 P2).
pub fn has_ir_frontend(lang: Lang) -> bool {
    !ir_root_kinds(lang).is_empty()
}

// ---- grammar-schema probe (D-SP4; docs/sp4-grammar-lift-plan.md §4, increment 4) ----

/// A readable, sorted dump of a grammar's reflection surface — the **named** node kinds and the
/// field names the *actually linked* tree-sitter [`tree_sitter::Language`] exposes, via the 0.26
/// reflection API (`node_kind_count`/`node_kind_for_id`/`node_kind_is_named`,
/// `field_count`/`field_name_for_id`). Snapshotted (increment 4) so a future grammar-crate bump
/// surfaces as a **reviewable diff** rather than a silent hash move — the cheap readable-schema
/// harness of `docs/SIMILARITY-IR.md` §12.1 applied to the grammar surface. It is additive tooling:
/// no canonical-form or parity claim. (It is also the ground truth the increment-5 conformance gate
/// checks the dispatch tables against — the inverse direction.)
///
/// Anonymous/auxiliary kinds are filtered out (`node_kind_is_named`) and field id 0 ("no field") is
/// skipped (field ids are `1..=field_count`); the result is sorted + de-duplicated for a stable,
/// order-independent snapshot. The named filter matches the gate's `id_for_node_kind(kind, named =
/// true)` resolution, so schema and gate agree on what "a kind exists" means.
// Registered-now/consumed-later: the snapshot test (increment 4) and the conformance gate
// (increment 5) are the only readers, both `#[cfg(test)]`, so this is dead in the plain library
// build — the same scoped `allow` the `RESIDUE_KINDS` tables carry.
#[allow(dead_code)]
pub(crate) fn grammar_schema(lang: Lang) -> String {
    use std::fmt::Write as _;
    let language = lang.ts_language();
    let mut kinds: Vec<&str> = (0..language.node_kind_count())
        .filter_map(|id| {
            let id = id as u16;
            language
                .node_kind_is_named(id)
                .then(|| language.node_kind_for_id(id))
                .flatten()
        })
        .collect();
    kinds.sort_unstable();
    kinds.dedup();
    let mut fields: Vec<&str> = (1..=language.field_count())
        .filter_map(|id| language.field_name_for_id(id as u16))
        .collect();
    fields.sort_unstable();
    fields.dedup();

    let mut out = String::new();
    let _ = writeln!(out, "grammar: {}", lang.name());
    let _ = writeln!(out, "named kinds ({}):", kinds.len());
    for k in &kinds {
        let _ = writeln!(out, "  {k}");
    }
    let _ = writeln!(out, "fields ({}):", fields.len());
    for f in &fields {
        let _ = writeln!(out, "  {f}");
    }
    out
}

/// A frontend's dispatch table paired with its residue-kind list — the two data structures the
/// conformance gate enumerates: `(MAP, RESIDUE_KINDS)`.
type FrontendTable = (&'static [(&'static str, Lowering)], &'static [&'static str]);

/// The `(dispatch table, residue-kinds)` pair for a `lang` that has an IR frontend — the **same**
/// [`MAP`](rust::MAP)/`RESIDUE_KINDS` the runtime dispatch reads, exposed so the increment-5
/// conformance gate checks against the one source of truth (structural completeness — Piece A of
/// `docs/sp4-grammar-lift-plan.md` §2). `None` for a language with no frontend yet (TS/Kotlin).
// Registered-now/consumed-later: the only reader is the `#[cfg(test)]` conformance gate below, so
// this is dead in the plain library build (same scoped `allow` as [`grammar_schema`]). Its being a
// non-test fn is what lets each frontend's `RESIDUE_KINDS` drop its own `allow` — they are now
// referenced here rather than only from a test.
#[allow(dead_code)]
pub(crate) fn frontend_table(lang: Lang) -> Option<FrontendTable> {
    match lang {
        Lang::Rust => Some((rust::MAP, rust::RESIDUE_KINDS)),
        Lang::Python => Some((python::MAP, python::RESIDUE_KINDS)),
        Lang::Go => Some((go::MAP, go::RESIDUE_KINDS)),
        _ => None,
    }
}

/// Walk `src`'s CST and IR-normalize every function-like unit — the IR-path analog of
/// `normalize::extract_raw_units`. Rust/Python/Go.
pub fn extract_ir_units(src: &str, lang: Lang, path: &std::path::Path) -> Vec<IrUnit> {
    let root_kinds = ir_root_kinds(lang);
    if root_kinds.is_empty() {
        return Vec::new();
    }
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&lang.ts_language()).is_err() {
        return Vec::new();
    }
    let Some(cst) = parser.parse(src, None) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    collect_ir_units(cst.root_node(), src, lang, root_kinds, path, &mut out, 0);
    out
}

/// `depth` bounds the unit-*finder*'s own CST descent — a second recursive walk, independent
/// of the lowering guard: it descends every named child hunting for nested units, so an
/// untrusted, deeply-nested file would overflow *here* (before or after lowering) too. Past
/// [`crate::normalize::MAX_EXTRACTION_DEPTH`] we stop searching deeper; a unit nested that far
/// is itself pathological, and units we already found lower under their own guard.
fn collect_ir_units(
    node: Node,
    src: &str,
    lang: Lang,
    root_kinds: &[&str],
    path: &std::path::Path,
    out: &mut Vec<IrUnit>,
    depth: u32,
) {
    if root_kinds.contains(&node.kind()) {
        // Bulk scan is the read-model selector's disabled sink (D-IR-12 /
        // `docs/transform-seam.md` §3): recording is a no-op, so the event stream costs
        // nothing and — being hash-excluded — cannot move the canonical tree. The one live
        // signal a disabled sink still carries is the depth guard's `truncated` flag: an
        // untrusted, over-deep unit is lowered up to `MAX_EXTRACTION_DEPTH` then truncated,
        // and the flag surfaces as `parse_degraded` (never a silent drop — spec §5.1/§12).
        let mut log = TransformLog::disabled();
        let raw = lower_unit_for(lang, node, src, &mut log);
        let tree = run_passes(raw.clone(), lang, &mut log);
        let name = node
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(src.as_bytes()).ok())
            .unwrap_or("<anon>")
            .to_string();
        let is_test = lang.profile().unit_is_test(node, src, &name, path);
        out.push(IrUnit {
            name,
            byte_span: (node.start_byte() as u32, node.end_byte() as u32),
            line_span: (
                node.start_position().row as u32 + 1,
                node.end_position().row as u32 + 1,
            ),
            parse_degraded: node.has_error() || log.truncated(),
            is_test,
            tree,
            raw,
        });
    }
    if depth >= crate::normalize::MAX_EXTRACTION_DEPTH {
        return;
    }
    let mut cursor = node.walk();
    for c in node.named_children(&mut cursor) {
        collect_ir_units(c, src, lang, root_kinds, path, out, depth + 1);
    }
}

/// The unit-*finder* for the test/convenience [`lower_source`] path. Like the production
/// finders ([`collect_ir_units`] / [`crate::normalize`]'s `collect_units`), this is a recursive
/// CST descent that would itself overflow the worker stack on a pathologically deep input, so it
/// stops searching past [`crate::normalize::MAX_EXTRACTION_DEPTH`] — the same cap the lowering
/// wrapper [`Frontend::lower_node`] enforces. Belt-and-suspenders: these helpers are off the
/// guarded production `scan` path, but a deep convenience call now returns `None` instead of
/// SIGSEGV. Normal (shallow) input is unaffected — the target is found far above the cap.
fn find_kind<'a>(node: Node<'a>, k: &str, depth: u32) -> Option<Node<'a>> {
    if node.kind() == k {
        return Some(node);
    }
    if depth >= crate::normalize::MAX_EXTRACTION_DEPTH {
        return None;
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find_map(|c| find_kind(c, k, depth + 1))
}

// ---- shared lowering helpers (language-agnostic; the algorithm, written once) ----

fn span_of(node: Node) -> (u32, u32) {
    (node.start_byte() as u32, node.end_byte() as u32)
}

fn text<'a>(node: Node, src: &'a str) -> &'a str {
    node.utf8_text(src.as_bytes()).unwrap_or_default()
}

pub(crate) fn leaf(k: &str, field: Option<&str>, span: (u32, u32)) -> NormNode {
    NormNode::new(k, field, span, Vec::new())
}

pub(crate) fn var(node: Node, field: Option<&str>, span: (u32, u32), src: &str) -> NormNode {
    NormNode::new(kind::VAR, field, span, Vec::new()).with_label(Label::Raw(text(node, src).into()))
}

/// An identifier that is `External` **by structure** (a field/type/package name — it can
/// never be a declared local), built directly so identifier abstraction leaves it alone.
/// Mirrors the historical `always_external` hook (spec §5.2.4 exception).
pub(crate) fn ext_name(node: Node, field: Option<&str>, span: (u32, u32), src: &str) -> NormNode {
    NormNode::new(kind::VAR, field, span, Vec::new())
        .with_label(Label::External(text(node, src).into()))
}

/// Literals whose identity is *structural* and must not bucket (spec §5.2.5): `0`/`1`/
/// `-1` are the loop/index/step constants a clone genuinely shares, and `""` the empty
/// string. The historical default `[normalize] literal_keep`; config-override plumbing
/// into the frontend is a later increment (matches `Config::default`).
const LIT_KEEP: &[&str] = &["0", "1", "-1", ""];

/// Literal → typed bucket (`Label::LitBucket`), **unless** it is on the keep-list
/// (`Label::LitKept`, kept verbatim — spec §5.2.5 / design principle 3 `Lit(bucket,
/// keep?)`). A bucketed literal records its value as a witness (§15) so two clones that
/// differ only in a constant converge, yet the divergence stays recoverable; a kept
/// literal is already exact, so nothing is normalized away and no witness is recorded.
pub(crate) fn lit(
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    bucket: Bucket,
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let raw = text(node, src);
    // Compare on content: string/char by inner text (quote- and prefix-stripped),
    // numerics by their trimmed form — mirrors the historical `abstract_literals` key.
    let key = match bucket {
        Bucket::Str | Bucket::Char => lit_inner_text(raw),
        _ => raw.trim().to_string(),
    };
    let label = if LIT_KEEP.contains(&key.as_str()) {
        Label::LitKept(key.into())
    } else {
        log.record(TransformKind::LitBucket, span, Witness::Literal(raw.into()));
        Label::LitBucket(bucket)
    };
    NormNode::new(kind::LIT, field, span, Vec::new()).with_label(label)
}

/// Strip prefix letters (`r`/`b`/`f`/`u`) and symmetric quotes to get literal content
/// for keep-list comparison (ported from the historical `normalize::inner_text`).
fn lit_inner_text(text: &str) -> String {
    let stripped = text.trim_start_matches(|c: char| c.is_ascii_alphabetic() || c == '#');
    let stripped = stripped.trim_end_matches('#');
    for quote in ["\"\"\"", "'''", "\"", "'"] {
        if stripped.len() >= 2 * quote.len()
            && stripped.starts_with(quote)
            && stripped.ends_with(quote)
        {
            return stripped[quote.len()..stripped.len() - quote.len()].to_string();
        }
    }
    stripped.to_string()
}

/// Unmodeled construct → opaque, resolution-free escape hatch (§12.1); the tree-sitter
/// kind is the discriminating tag, children still lowered.
pub(crate) fn native(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    NormNode::new(
        kind::NATIVE_STMT,
        field,
        span,
        lower_stmts(fe, node, src, log),
    )
    .with_label(Label::External(node.kind().into()))
}

pub(crate) fn block(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    NormNode::new(kind::BLOCK, field, span, lower_stmts(fe, node, src, log))
}

fn lower_stmts(fe: &dyn Frontend, node: Node, src: &str, log: &mut TransformLog) -> Vec<NormNode> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for c in node.named_children(&mut cursor) {
        if fe.splice_kind(c.kind()) {
            // Hoist the wrapper's children into this list (Go `statement_list`).
            out.extend(lower_stmts(fe, c, src, log));
        } else if let Some(stmts) = fe.expand_stmt(c, src, log) {
            // One statement lowering to several (Go C-style `for`: init + Loop).
            out.extend(stmts);
        } else if let Some(n) = fe.lower_node(c, None, src, log) {
            out.push(n);
        }
    }
    out
}

pub(crate) fn lower_body(
    fe: &dyn Frontend,
    node: Node,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    node.child_by_field_name("body")
        .and_then(|b| fe.lower_node(b, Some("body"), src, log))
        .unwrap_or_else(|| NormNode::new(kind::BLOCK, Some("body"), span, Vec::new()))
}

pub(crate) fn lower_function(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let mut children = Vec::new();
    if let Some(params) = node.child_by_field_name("parameters") {
        fe.lower_params(params, src, log, &mut children);
    }
    let mut body = fe.lower_fn_body(node, span, src, log);
    // Tail-recursion lowering needs the function name + simple param names, both
    // available here from the CST; the shared algorithm lives in `crate::ir::pass`.
    if let Some(name) = node.child_by_field_name("name").map(|n| text(n, src)) {
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

pub(crate) fn lower_call(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    fields: (&str, &str),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let (fn_f, args_f) = fields;
    let mut children = Vec::new();
    if let Some(c) = node
        .child_by_field_name(fn_f)
        .and_then(|n| fe.lower_node(n, Some("callee"), src, log))
    {
        children.push(c);
    }
    if let Some(args) = node.child_by_field_name(args_f) {
        let mut cursor = args.walk();
        for a in args.named_children(&mut cursor) {
            if let Some(c) = fe.lower_node(a, Some("arg"), src, log) {
                children.push(c);
            }
        }
    }
    NormNode::new(kind::CALL, field, span, children)
}

pub(crate) fn lower_return(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let mut cursor = node.walk();
    let value = node
        .named_children(&mut cursor)
        .find_map(|c| fe.lower_node(c, Some("value"), src, log));
    NormNode::new(kind::RETURN, field, span, value.into_iter().collect())
}

pub(crate) fn drop_parens(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> Option<NormNode> {
    log.record(TransformKind::ParenDrop, span, Witness::None);
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find_map(|c| fe.lower_node(c, field, src, log))
}

pub(crate) fn lower_binary(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    fields: (&str, &str, &str),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let (l_f, op_f, r_f) = fields;
    let mut children = Vec::new();
    if let Some(l) = node
        .child_by_field_name(l_f)
        .and_then(|n| fe.lower_node(n, Some("left"), src, log))
    {
        children.push(l);
    }
    if let Some(op) = node.child_by_field_name(op_f) {
        children.push(NormNode::new(
            text(op, src),
            Some("op"),
            span_of(op),
            Vec::new(),
        ));
    }
    if let Some(r) = node
        .child_by_field_name(r_f)
        .and_then(|n| fe.lower_node(n, Some("right"), src, log))
    {
        children.push(r);
    }
    NormNode::new(kind::BINOP, field, span, children)
}

/// `target = value` → `Assign`. `fields = (target_cst_field, value_cst_field, bind)`:
/// `bind` distinguishes a *declaration* (`@target`, becomes a positional local) from a
/// *mutation* (`@place`, left untouched by abstraction) — the difference between `let x
/// = …` / Python `x = …` (bindings) and Rust's `x = …` `assignment_expression`.
pub(crate) fn lower_assign(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    fields: (&str, &str, bool),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let (target_f, value_f, bind) = fields;
    let target_field = if bind { "target" } else { "place" };
    let mut children = Vec::new();
    if let Some(t) = node
        .child_by_field_name(target_f)
        .and_then(|n| fe.lower_node(n, Some(target_field), src, log))
    {
        children.push(t);
    }
    if let Some(v) = node
        .child_by_field_name(value_f)
        .and_then(|n| fe.lower_node(n, Some("value"), src, log))
    {
        children.push(v);
    }
    NormNode::new(kind::ASSIGN, field, span, children)
}

/// Augmented assignment `a op= b` → the desugared `Assign{ a, Binop{ a op b } }`
/// (spec §5.2.3 desugaring; a sound total identity). `bind` picks the outer lvalue's
/// field — `@target` where the language treats `op=` as a declaration site (Python's
/// `collect_declared`), `@place` for a pure mutation. The compound operator token
/// (`+=`) drops its trailing `=` to become the binop operator (`+`).
pub(crate) fn lower_aug_assign(
    fe: &dyn Frontend,
    node: Node,
    // (left_field, op_field, right_field, target_is_binding)
    fields: (&str, &str, &str, bool),
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let (lf, opf, rf, bind) = fields;
    let target_field = if bind { "target" } else { "place" };
    let mut children = Vec::new();
    // The lvalue appears twice: as the assignment target and as the binop's left operand
    // (the read). Lowering it twice is idempotent (no side effects in a lowering).
    let target = node
        .child_by_field_name(lf)
        .and_then(|n| fe.lower_node(n, Some(target_field), src, log));
    let read = node
        .child_by_field_name(lf)
        .and_then(|n| fe.lower_node(n, Some("left"), src, log));
    let op = node.child_by_field_name(opf).map(|o| {
        let text = self::text(o, src).trim_end_matches('=');
        NormNode::new(text, Some("op"), span_of(o), Vec::new())
    });
    let right = node
        .child_by_field_name(rf)
        .and_then(|n| fe.lower_node(n, Some("right"), src, log));
    let binop = NormNode::new(
        kind::BINOP,
        Some("value"),
        span,
        read.into_iter().chain(op).chain(right).collect(),
    );
    if let Some(t) = target {
        children.push(t);
    }
    children.push(binop);
    log.record(TransformKind::AugAssign, span, Witness::None);
    NormNode::new(kind::ASSIGN, field, span, children)
}

/// `Branch{ arm* }` — the unified conditional (§14): if-chains, `match`, `switch` and
/// ternary all lower to this one kind. There is **no** separate subject element — a
/// `match`/`switch` subject is folded *into* each arm's guard ([`eq_guard`] /
/// [`matches_guard`]), which is what makes `switch x { case 1: … }` converge with the
/// equivalent `if x == 1 { … }` (a real refactor pair). The dispatch-table false positive
/// (idiomatic enum→value tables) is handled by D30 fold-but-don't-report, not by keeping
/// the shapes structurally distinct.
pub(crate) fn make_branch(arms: Vec<NormNode>, field: Option<&str>, span: (u32, u32)) -> NormNode {
    NormNode::new(kind::BRANCH, field, span, arms)
}

/// A value-case guard: `subject == value` — byte-identical to how an `if subject == value`
/// condition lowers, so a value `switch`/`match` arm converges with the if-chain arm.
pub(crate) fn eq_guard(subject: &NormNode, value: NormNode, span: (u32, u32)) -> NormNode {
    let mut left = subject.clone();
    left.field = Some("left".into());
    let op = NormNode::new("==", Some("op"), span, Vec::new());
    let mut right = value;
    right.field = Some("right".into());
    NormNode::new(kind::BINOP, Some("guard"), span, vec![left, op, right])
}

/// A pattern-case guard: `matches(subject, pat)` — a synthesized region-test (§14 pattern
/// arms). It keeps the subject (two matches on different subjects stay distinct) and the
/// pattern's captures (declared locals), and — being a `matches` call, not an equality —
/// correctly does **not** converge with an equality if-chain.
pub(crate) fn matches_guard(subject: &NormNode, mut pat: NormNode, span: (u32, u32)) -> NormNode {
    let callee = NormNode::new(kind::VAR, Some("callee"), span, Vec::new())
        .with_label(Label::External("matches".into()));
    let mut subj = subject.clone();
    subj.field = Some("arg".into());
    // Keep a bare-capture pattern's own `@target` (a binding); else it rides as `@arg`.
    if pat.field.as_deref() != Some("target") {
        pat.field = Some("arg".into());
    }
    NormNode::new(kind::CALL, Some("guard"), span, vec![callee, subj, pat])
}

/// Combine value-case guards with `||` (`case 1, 2:` ≡ `x==1 || x==2`), left-associative.
/// One guard passes through; none yields `None` (an else/default arm).
pub(crate) fn or_chain(guards: Vec<NormNode>, span: (u32, u32)) -> Option<NormNode> {
    let mut it = guards.into_iter();
    let mut acc = it.next()?;
    acc.field = Some("guard".into());
    for mut g in it {
        acc.field = Some("left".into());
        g.field = Some("right".into());
        let op = NormNode::new("||", Some("op"), span, Vec::new());
        acc = NormNode::new(kind::BINOP, Some("guard"), span, vec![acc, op, g]);
    }
    Some(acc)
}

/// A `match`/`case` arm guard — single-sourced across frontends (§14). `_` (wildcard) → `None`
/// (the else arm); a **literal** pattern → `subject == literal` (converges with an if-chain);
/// anything else → `matches(subject, pattern)`. The only genuine per-language quirks — the
/// wildcard test and the literal-pattern grammar kinds — are computed by each frontend and passed
/// in as `is_wildcard` / `literal`; the 3-arm orchestration lives here once.
#[allow(clippy::too_many_arguments)]
pub(crate) fn case_guard(
    fe: &dyn Frontend,
    subject: Option<&NormNode>,
    pat: Node,
    is_wildcard: bool,
    literal: Option<Node>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> Option<NormNode> {
    if is_wildcard {
        return None;
    }
    match (subject, literal) {
        (Some(subj), Some(lit)) => Some(eq_guard(subj, fe.lower_node(lit, None, src, log)?, span)),
        (Some(subj), None) => Some(matches_guard(subj, lower_pattern(fe, pat, src, log)?, span)),
        (None, _) => lower_pattern(fe, pat, src, log),
    }
}

/// Lower a `match`/`case` pattern into an arm guard, marking its bound identifiers as
/// declared locals (`@target`) so a renamed capture (`Some(x)` vs `Some(y)`) still
/// converges — mirrors the historical `collect_pattern_idents` (which likewise skips
/// `type`-annotation subtrees, and never captures the `External` type/variant names).
pub(crate) fn lower_pattern(
    fe: &dyn Frontend,
    node: Node,
    src: &str,
    log: &mut TransformLog,
) -> Option<NormNode> {
    let mut g = fe.lower_node(node, Some("guard"), src, log)?;
    bind_pattern_idents(&mut g);
    Some(g)
}

pub(crate) fn bind_pattern_idents(node: &mut NormNode) {
    if node.field.as_deref() == Some("type") {
        return; // a type annotation inside the pattern is not a binding
    }
    if node.kind.as_ref() == kind::VAR && matches!(node.label, Some(Label::Raw(_))) {
        node.field = Some("target".into());
    }
    for c in &mut node.children {
        bind_pattern_idents(c);
    }
}

/// `Lambda{ params…, body }` from already-lowered `@param` vars and a body node.
pub(crate) fn make_lambda(
    params: Vec<NormNode>,
    body: Option<NormNode>,
    field: Option<&str>,
    span: (u32, u32),
) -> NormNode {
    NormNode::new(
        kind::LAMBDA,
        field,
        span,
        params.into_iter().chain(body).collect(),
    )
}

pub(crate) fn unwrap_stmt(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    src: &str,
    log: &mut TransformLog,
) -> Option<NormNode> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find_map(|c| fe.lower_node(c, field, src, log))
}

pub(crate) fn lower_loop(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    // Already the canonical core — no transform recorded.
    NormNode::new(
        kind::LOOP,
        field,
        span,
        vec![lower_body(fe, node, span, src, log)],
    )
}

pub(crate) fn lower_while(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    // `while cond { body }` → `Loop { break-guard; body }`. Lossy: record LoopLower with
    // the source form so it reverses for display and the log diff explains the match.
    // `while true` / `while True` is already the infinite-loop core — no break-guard, so it
    // converges with `loop {}` and with an explicit `while True: if !c { break }` form.
    let cond_node = node.child_by_field_name("condition");
    let mut body = lower_body(fe, node, span, src, log);
    if cond_node.is_some_and(|c| is_true_literal(c, src)) {
        return NormNode::new(kind::LOOP, field, span, vec![body]);
    }
    if let Some(cond) = cond_node.and_then(|c| fe.lower_node(c, None, src, log)) {
        body.children.insert(0, break_guard(cond, span));
    }
    log.record(
        TransformKind::LoopLower,
        span,
        Witness::LoopForm("while".into()),
    );
    NormNode::new(kind::LOOP, field, span, vec![body])
}

/// The condition is the boolean-true keyword (`true` / Python `True`) — a `while` over it
/// (or a Go `for true {}`) is the infinite-loop core, so no break-guard is synthesized.
pub(crate) fn is_true_literal(node: Node, src: &str) -> bool {
    matches!(text(node, src).trim(), "true" | "True")
}

pub(crate) fn lower_for(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    fields: (&str, &str),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    // `for pat in iter { body }` → the canonical iterated loop (§5.2.2):
    //   Loop { if !__has_next(iter) { break }; pat = __next(iter); body }
    // The synthesized has_next/next protocol is identical across languages, so a Rust
    // `for` and a Python `for` converge to the same canonical form.
    let (pat_f, iter_f) = fields;
    let mut body = lower_body(fe, node, span, src, log);
    let iter = node
        .child_by_field_name(iter_f)
        .and_then(|n| fe.lower_node(n, None, src, log));
    let pat = node
        .child_by_field_name(pat_f)
        .and_then(|n| fe.lower_node(n, Some("target"), src, log))
        .map(|mut p| {
            // A destructuring loop pattern (`for (k, v) in …`) is a binding site: every
            // identifier it binds is a declared local, so renamed captures converge.
            bind_pattern_idents(&mut p);
            p
        });
    if let (Some(iter), Some(pat)) = (iter, pat) {
        let guard = break_guard(call_ext("__has_next", iter.clone(), span), span);
        if pat.kind.as_ref() == kind::VAR {
            // Simple element var (`for x in xs`): bind it directly to `__next`.
            let bind = make_assign(pat, call_ext("__next", iter, span), span);
            body.children.insert(0, bind);
        } else {
            // Destructuring pattern (`for (k, v) in items`): bind the element to a synthetic
            // temp first, then destructure the temp — `__e = __next(coll); (k, v) = __e`. This
            // is the SAME two-step shape an index/counter loop reaches after its element bind
            // `i = __next(coll)` + a source-level element destructure `(k, v) = coll[i]` rewrites
            // to `(k, v) = i` — so a tuple `for` and a tuple index loop converge (span-unique
            // temp name so nested destructuring loops stay distinct after abstraction).
            let temp: Box<str> = format!("__e{}", span.0).into();
            let elem = NormNode::new(kind::VAR, Some("target"), span, Vec::new())
                .with_label(Label::Raw(temp.clone()));
            let elem_read =
                NormNode::new(kind::VAR, None, span, Vec::new()).with_label(Label::Raw(temp));
            let destructure = make_assign(pat, elem_read, span);
            let bind = make_assign(elem, call_ext("__next", iter, span), span);
            body.children.insert(0, destructure);
            body.children.insert(0, bind);
        }
        body.children.insert(0, guard);
    }
    log.record(
        TransformKind::LoopLower,
        span,
        Witness::LoopForm("for".into()),
    );
    NormNode::new(kind::LOOP, field, span, vec![body])
}

/// Synthesize `name(arg)` with an `External` callee (the `__has_next`/`__next` protocol).
pub(crate) fn call_ext(name: &str, arg: NormNode, span: (u32, u32)) -> NormNode {
    let callee = NormNode::new(kind::VAR, Some("callee"), span, Vec::new())
        .with_label(Label::External(name.into()));
    let mut a = arg;
    a.field = Some("arg".into());
    NormNode::new(kind::CALL, None, span, vec![callee, a])
}

/// `target = value` where `target` already carries its `@target` field.
pub(crate) fn make_assign(target: NormNode, value: NormNode, span: (u32, u32)) -> NormNode {
    let mut v = value;
    v.field = Some("value".into());
    NormNode::new(kind::ASSIGN, None, span, vec![target, v])
}

/// A parallel multi-target `Assign{ target*, value* }` (all targets, then all values) — the
/// canonical shape a parallel/tuple assignment (`a, b = X, Y`) lowers to (byte-identical to Go's
/// multi-`=`), which the shared [`crate::ir::pass::detect_multi_assign`] pass then sequentializes
/// into single assigns with minimal temps. `targets` already carry `@place`/`@target`, `values`
/// `@value`.
pub(crate) fn make_multi_assign(
    targets: Vec<NormNode>,
    values: Vec<NormNode>,
    field: Option<&str>,
    span: (u32, u32),
) -> NormNode {
    NormNode::new(
        kind::ASSIGN,
        field,
        span,
        targets.into_iter().chain(values).collect(),
    )
}

/// Field access `base.name` → `Field{ base, name }`. The field name is **always
/// External** by structure (it is never a local), so the historical `always_external`
/// hook dissolves: abstraction leaves it alone.
pub(crate) fn lower_field(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    fields: (&str, &str),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let (base_f, name_f) = fields;
    let mut children = Vec::new();
    if let Some(b) = node
        .child_by_field_name(base_f)
        .and_then(|n| fe.lower_node(n, Some("base"), src, log))
    {
        children.push(b);
    }
    if let Some(name) = node.child_by_field_name(name_f) {
        children.push(
            NormNode::new(kind::VAR, Some("name"), span_of(name), Vec::new())
                .with_label(Label::External(text(name, src).into())),
        );
    }
    NormNode::new(kind::FIELD, field, span, children)
}

/// Index/subscript `base[idx]` → `Index{ base, idx }`. Base/idx are extracted per
/// language (Rust `index_expression` is positional; Python `subscript` is field-based).
pub(crate) fn make_index(
    base: Option<NormNode>,
    idx: Option<NormNode>,
    field: Option<&str>,
    span: (u32, u32),
) -> NormNode {
    NormNode::new(
        kind::INDEX,
        field,
        span,
        base.into_iter().chain(idx).collect(),
    )
}

pub(crate) fn lower_if(
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
        // The `else`/`else if` payload (Rust wraps it in an `else_clause`).
        let mut cursor = alt.walk();
        let else_body = alt
            .named_children(&mut cursor)
            .find_map(|c| fe.lower_node(c, Some("body"), src, log));
        push_else_arm(&mut arms, else_body);
    }
    NormNode::new(kind::BRANCH, field, span, arms)
}

/// Wrap a bare arm body in a `Block` so a `match`/`case` arm whose body is a single
/// expression (`0 => g()`) matches the `Block` an `if` consequence (`if … { g() }`) lowers
/// to — the last piece that lets value-`match` arms converge with if-chain arms.
pub(crate) fn block_wrap(mut body: NormNode, span: (u32, u32)) -> NormNode {
    if body.kind.as_ref() == kind::BLOCK {
        return body;
    }
    body.field = None;
    NormNode::new(kind::BLOCK, Some("body"), span, vec![body])
}

/// Attach an `else`/`else if` payload to a `Branch`'s arm list. An `else if` lowers to a
/// nested `Branch` — its arms are **spliced flat** (§14: one ordered first-match
/// conditional, not a nest), which is what lets an `if/else-if` chain converge with the
/// equivalent flat `match`/`switch`. A plain `else` block becomes the trivial-guard arm.
pub(crate) fn push_else_arm(arms: &mut Vec<NormNode>, else_body: Option<NormNode>) {
    match else_body {
        Some(b) if b.kind.as_ref() == kind::BRANCH => arms.extend(b.children),
        Some(b) => {
            let span = b.span;
            arms.push(make_arm(None, Some(b), span));
        }
        None => {}
    }
}

/// Rust-style unary: `<op-token> <operand>` (positional). Operator text kept as written.
pub(crate) fn lower_unary_positional(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let mut op = None;
    let mut operand = None;
    let mut cursor = node.walk();
    for c in node.children(&mut cursor) {
        if c.is_named() {
            operand = fe.lower_node(c, Some("operand"), src, log);
        } else if op.is_none() {
            op = Some(NormNode::new(
                text(c, src),
                Some("op"),
                span_of(c),
                Vec::new(),
            ));
        }
    }
    NormNode::new(
        kind::UNOP,
        field,
        span,
        op.into_iter().chain(operand).collect(),
    )
}

/// Python-style `not x`: canonicalize the negation operator to `!` so it converges with
/// the break-guard synthesis and with Rust's `!x`.
pub(crate) fn lower_unary_field(
    fe: &dyn Frontend,
    node: Node,
    field: Option<&str>,
    span: (u32, u32),
    arg_f: &str,
    src: &str,
    log: &mut TransformLog,
) -> NormNode {
    let operand = node
        .child_by_field_name(arg_f)
        .and_then(|n| fe.lower_node(n, Some("operand"), src, log));
    let op = NormNode::new("!", Some("op"), span, Vec::new());
    NormNode::new(
        kind::UNOP,
        field,
        span,
        std::iter::once(op).chain(operand).collect(),
    )
}

// ---- pure canonical-node synthesis (no frontend) ----

/// `Arm[ guard?, body? ]` — a member of a canonical `Branch` (§14).
pub(crate) fn make_arm(
    guard: Option<NormNode>,
    body: Option<NormNode>,
    span: (u32, u32),
) -> NormNode {
    NormNode::new(
        kind::ARM,
        Some("arm"),
        span,
        guard.into_iter().chain(body).collect(),
    )
}

/// The canonical break-guard `Branch[ Arm[ !cond → { Break } ] ]` — built to be
/// byte-identical to a hand-written `if !cond { break }`, which is what makes `while`
/// converge with an explicit `loop` (and across languages: same synthesis for both).
pub(crate) fn break_guard(cond: NormNode, span: (u32, u32)) -> NormNode {
    let mut operand = cond;
    operand.field = Some("operand".into());
    let bang = NormNode::new("!", Some("op"), span, Vec::new());
    let guard = NormNode::new(kind::UNOP, Some("guard"), span, vec![bang, operand]);
    let brk = NormNode::new(kind::BREAK, None, span, Vec::new());
    let body = NormNode::new(kind::BLOCK, Some("body"), span, vec![brk]);
    NormNode::new(
        kind::BRANCH,
        None,
        span,
        vec![make_arm(Some(guard), Some(body), span)],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run the full IR pipeline (lowering + all passes) on `src`'s first unit, threading
    /// `log` as the sink, and return the canonical s-expression.
    fn pipeline(src: &str, lang: Lang, log: &mut TransformLog) -> String {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&lang.ts_language()).unwrap();
        let cst = parser.parse(src, None).unwrap();
        let root_kind = ir_root_kinds(lang)[0];
        let func = find_kind(cst.root_node(), root_kind, 0).expect("a unit");
        crate::ir::to_sexpr(&normalize(lang, func, src, log))
    }

    /// The full-pipeline canonical form of `src`'s first Rust unit (disabled sink).
    fn canon(src: &str) -> String {
        pipeline(src, Lang::Rust, &mut TransformLog::disabled())
    }

    #[test]
    fn a1_orients_comparisons_and_keeps_near_misses_distinct() {
        // A1 driving case: `a < b` ≡ `b > a` (orient `>` → `<`, swapping operands).
        assert_eq!(
            canon("fn f() -> bool { a < b }"),
            canon("fn g() -> bool { b > a }"),
            "`a < b` and `b > a` must converge",
        );
        // …and `a <= b` ≡ `b >= a`.
        assert_eq!(
            canon("fn f() -> bool { a <= b }"),
            canon("fn g() -> bool { b >= a }"),
            "`a <= b` and `b >= a` must converge",
        );
        // Near-miss (discrimination): strict vs. non-strict must STAY DISTINCT.
        assert_ne!(
            canon("fn f() -> bool { a < b }"),
            canon("fn g() -> bool { a <= b }"),
            "`a < b` must not converge with `a <= b`",
        );
    }

    #[test]
    fn a2_while_converges_with_hand_rolled_inverted_break() {
        // A2 HEADLINE (the break_guard convergence): `while i < n { … }` lowers to a guard
        // `!(i < n)`, which A2 inverts to `i >= n` and A1 then orients to `n <= i` — the SAME
        // spelling a hand-rolled `loop { if i >= n { break } … }` reaches. The two forms, long
        // divergent on the commonest loop shape, now converge (fixpoint of A2→A1).
        assert_eq!(
            canon("fn w(n: i32) { let mut i = 0; while i < n { s(i); i += 1; } }"),
            canon("fn l(n: i32) { let mut i = 0; loop { if i >= n { break; } s(i); i += 1; } }"),
            "`while i < n` must converge with the hand-rolled inverted-break loop",
        );
    }

    #[test]
    fn a2_pushes_de_morgan_double_neg_and_comparison_inversion() {
        // De Morgan: `!(a && b)` ≡ `!a || !b`.
        assert_eq!(
            canon("fn f() -> bool { !(a && b) }"),
            canon("fn g() -> bool { !a || !b }"),
            "De Morgan must converge",
        );
        // Double negation: `!!x` ≡ `x`.
        assert_eq!(
            canon("fn f() -> bool { !!x }"),
            canon("fn g() -> bool { x }"),
            "double negation must cancel",
        );
        // Comparison inversion (then A1 orient): `!(a < b)` ≡ `b <= a`.
        assert_eq!(
            canon("fn f() -> bool { !(a < b) }"),
            canon("fn g() -> bool { b <= a }"),
            "`!(a < b)` must converge with `b <= a`",
        );
    }

    #[test]
    fn a2_keeps_inequivalent_boolean_forms_distinct() {
        // Precision: `!(a && b)` = `!a || !b` must NOT converge with `!(a || b)` = `!a && !b`.
        assert_ne!(
            canon("fn f() -> bool { !(a && b) }"),
            canon("fn g() -> bool { !(a || b) }"),
            "`!(a && b)` must not converge with `!(a || b)`",
        );
        // A negated comparison is not its unnegated self.
        assert_ne!(
            canon("fn f() -> bool { !(a < b) }"),
            canon("fn g() -> bool { a < b }"),
            "`!(a < b)` must not converge with `a < b`",
        );
    }

    #[test]
    fn c1_merges_nested_ifs_and_keeps_else_bearing_nests_distinct() {
        // C1 driving case: `if a { if b { X } }` ≡ `if a && b { X }`.
        assert_eq!(
            canon("fn f() { if a { if b { g(); } } }"),
            canon("fn h() { if a && b { g(); } }"),
            "nested single-arm ifs must converge with the conjunction",
        );
        // Near-miss (discrimination): an inner `else` blocks the merge (sound only for no-else),
        // so it must STAY DISTINCT from the merged conjunction.
        assert_ne!(
            canon("fn f() { if a && b { g(); } }"),
            canon("fn h() { if a { if b { g(); } else { k(); } } }"),
            "an else-bearing nest must not merge",
        );
        // Near-miss: an extra statement beside the inner `if` also blocks the merge.
        assert_ne!(
            canon("fn f() { if a && b { g(); } }"),
            canon("fn h() { if a { s(); if b { g(); } } }"),
            "an inner `if` with a sibling statement must not merge",
        );
    }

    #[test]
    fn c2_drops_redundant_else_and_keeps_non_diverging_ones() {
        // C2 driving case: `if c { return x } else { g() }` ≡ `if c { return x }; g()` — the
        // else is redundant once the then-arm provably diverges, so its body hoists to siblings.
        assert_eq!(
            canon("fn f(c: bool, x: i32) -> i32 { if c { return x; } else { g(); } h() }"),
            canon("fn k(c: bool, x: i32) -> i32 { if c { return x; } g(); h() }"),
            "a redundant else after a return must hoist",
        );
        // Near-miss (discrimination): a NON-diverging then-arm keeps its else (dropping it would
        // change control flow), so it must STAY DISTINCT from the hoisted form.
        assert_ne!(
            canon("fn f(c: bool) { if c { s(); } else { g(); } h() }"),
            canon("fn k(c: bool) { if c { s(); } g(); h() }"),
            "a non-diverging then-arm must not have its else dropped",
        );
    }

    #[test]
    fn confluence_not_gt_and_is_order_stable() {
        // Golden confluence (§8 pipeline-order fragility): `!(a > b) && c` exercises A2
        // (not-push: invert `>` → `<=`, dropping the `!`), A1 (the `<=` is already canonical),
        // and comm-sort (sorts the `&&` chain). The pinned order — A after abstraction, then
        // comm-sort — is confluent, so every equivalent spelling reaches ONE canonical form.
        let golden = canon("fn f() -> bool { !(a > b) && c }");
        // not-push + orient interplay: converges with the hand-written normalized comparison.
        assert_eq!(
            golden,
            canon("fn g() -> bool { a <= b && c }"),
            "A: not-push+orient",
        );
        // comm-sort interplay: the `&&` operands sort regardless of source order.
        assert_eq!(
            golden,
            canon("fn h() -> bool { c && !(a > b) }"),
            "comm-sort order",
        );
        // Structure: the `!` and `>` are gone, the canonical comparison is `<=`, the chain `&&`.
        assert!(
            !golden.contains("(Unop") && !golden.contains("(>@op)"),
            "negation/`>` not normalized away: {golden}",
        );
        assert!(
            golden.contains("(<=@op)") && golden.contains("(&&@op)"),
            "expected a `<=` inside a sorted `&&` chain: {golden}",
        );
    }

    const CASES: &[(Lang, &str)] = &[
        (
            Lang::Rust,
            "fn f(a: i32, xs: &[i32]) -> i32 { let mut s = 0; for i in 0..xs.len() { s += xs[i]; } return s + a; }",
        ),
        (Lang::Rust, "fn w() { while c() { s(); continue; } }"),
        (Lang::Python, "def f(a, b):\n    a += b\n    return b + a\n"),
        (
            Lang::Python,
            "def f(xs):\n    for i in range(len(xs)):\n        g(xs[i])\n",
        ),
        (
            Lang::Go,
            "func f(n int) int {\n\tfor i := 0; i < n; i++ {\n\t\ts()\n\t}\n\treturn (n)\n}\n",
        ),
        // The counter-loop iteration rewrite (block-level splice) under both sinks.
        (
            Lang::Go,
            "func total(xs []int) int {\n\tacc := 0\n\tfor i := 0; i < len(xs); i++ {\n\t\tacc += xs[i]\n\t}\n\treturn acc\n}\n",
        ),
        (
            Lang::Go,
            "func gcd(a int, b int) int {\n\tif b == 0 {\n\t\treturn a\n\t}\n\treturn gcd(b, a)\n}\n",
        ),
        // Exercises Families A + C under both sinks: A1/A2 (`while a > n` → guard `!(a>n)` →
        // `n >= a` → `a <= n`... normalized), C1 (nested `if`), C2 (redundant else after return).
        (
            Lang::Rust,
            "fn f(n: i32) -> i32 { while a > n { if b { if c { g(); } } } if d { return 1; } else { h(); } 0 }",
        ),
    ];

    #[test]
    fn normalize_tree_is_invariant_to_the_sink() {
        // Parity gate (D-IR-12 / `docs/transform-seam.md` §3): the canonical tree out of
        // the pipeline is byte-identical whether the sink records (enabled) or is the bulk
        // path's null sink (disabled). The log is hash-excluded and recording never feeds
        // back into tree construction, so completing the event stream cannot move the tree.
        for &(lang, src) in CASES {
            let enabled = pipeline(src, lang, &mut TransformLog::new());
            let disabled = pipeline(src, lang, &mut TransformLog::disabled());
            assert_eq!(enabled, disabled, "sink moved the tree for {lang:?}: {src}");
        }
    }

    #[test]
    fn disabled_sink_drops_events_the_recording_sink_keeps() {
        // The two sinks are the read-model selector: enabled folds the complete stream
        // (lowering + passes), disabled records nothing at all (the bulk path's zero cost).
        let mut on = TransformLog::new();
        let _ = pipeline(CASES[0].1, Lang::Rust, &mut on);
        assert!(!on.is_empty(), "recording sink captured no events");
        let mut off = TransformLog::disabled();
        let _ = pipeline(CASES[0].1, Lang::Rust, &mut off);
        assert!(off.is_empty(), "disabled sink recorded events");
    }
}

#[cfg(test)]
mod grammar_schema_tests {
    use super::grammar_schema;
    use crate::lang::Lang;

    /// The grammars that have an IR frontend (the increment-5 gate's scope). TS/Kotlin have no
    /// frontend yet (`docs/SIMILARITY-IR.md` §9 P2), so their grammar surface is not snapshotted
    /// until their frontends (and tables) exist.
    const SNAPSHOTTED: &[(Lang, &str)] = &[
        (Lang::Rust, "grammar-rust.snap"),
        (Lang::Python, "grammar-python.snap"),
        (Lang::Go, "grammar-go.snap"),
    ];

    /// Snapshot each frontend grammar's named-kind + field surface (increment 4). A grammar-crate
    /// bump that renames/adds/removes a kind or field then produces a **reviewable diff** here
    /// instead of silently moving a hash. Regenerate deliberately, after *reviewing* a bump's diff,
    /// with `UPDATE_GRAMMAR_SNAPSHOTS=1 cargo test` (the bump ritual — increment 6). This is
    /// additive tooling: it makes no canonical-form/parity claim, it only watches the grammar edge.
    #[test]
    fn grammar_schema_matches_snapshot() {
        for &(lang, file) in SNAPSHOTTED {
            let actual = grammar_schema(lang);
            let path = format!(
                "{}/src/frontend/snapshots/{}",
                env!("CARGO_MANIFEST_DIR"),
                file
            );
            if std::env::var_os("UPDATE_GRAMMAR_SNAPSHOTS").is_some() {
                std::fs::write(&path, &actual).expect("write grammar snapshot");
                continue;
            }
            let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!(
                    "missing grammar snapshot {path}: {e}; \
                     regenerate with UPDATE_GRAMMAR_SNAPSHOTS=1"
                )
            });
            assert_eq!(
                actual, expected,
                "grammar schema for {lang:?} drifted from {file}; if a grammar bump is intended, \
                 review the diff then regenerate with UPDATE_GRAMMAR_SNAPSHOTS=1"
            );
        }
    }
}

#[cfg(test)]
mod conformance_gate_tests {
    //! The SP4 closure (increment 5): assert every construct each frontend's table references still
    //! exists in the *actually linked* tree-sitter grammar, turning a silent grammar-drift hash move
    //! (a renamed kind falls through to `native()` with a green build) into a **loud, localized**
    //! test failure that names the dead construct. Derived from the dispatch tables (Piece B over
    //! Piece A — `docs/sp4-grammar-lift-plan.md` §2), so the gate and runtime share one source of
    //! truth. Plus the inverse advisory snapshot: the named grammar kinds no table models, so a bump
    //! that adds/renames a kind is a reviewable diff.

    use super::{Lowering, frontend_table};
    use crate::lang::Lang;
    use tree_sitter::Language;

    /// The languages with an IR frontend + table (the gate's scope). TS/Kotlin have no frontend yet.
    const GATED: &[Lang] = &[Lang::Rust, Lang::Python, Lang::Go];

    /// The per-language CST **field** names a table entry threads into its shared helper — the field
    /// half of the (kind, field) coupling. Only the field-parameterized `Lowering` variants carry
    /// any; the rest use positional children or the shared helper's fixed field set. (`Leaf`/`Lit`
    /// carry a *canonical* kind/bucket, NOT a grammar field — deliberately not collected.)
    fn entry_fields(l: &Lowering, out: &mut Vec<&'static str>) {
        match *l {
            Lowering::For(pat, iter) => out.extend([pat, iter]),
            Lowering::Call(callee, args) => out.extend([callee, args]),
            Lowering::Field(base, name) => out.extend([base, name]),
            Lowering::Binary(l, o, r) => out.extend([l, o, r]),
            Lowering::AugAssign(l, o, r, _) => out.extend([l, o, r]),
            _ => {}
        }
    }

    /// Every grammar-coupled string a frontend references *as data*: the node kinds (MAP keys ∪
    /// `RESIDUE_KINDS`) and the table-embedded field names.
    fn referenced(lang: Lang) -> (Vec<&'static str>, Vec<&'static str>) {
        let (map, residue) = frontend_table(lang).expect("a gated language has a table");
        let mut kinds: Vec<&str> = map.iter().map(|(k, _)| *k).collect();
        kinds.extend(residue.iter().copied());
        let mut fields = Vec::new();
        for (_, l) in map {
            entry_fields(l, &mut fields);
        }
        (kinds, fields)
    }

    /// The dead references: node kinds that no longer resolve (`id_for_node_kind → 0`, the bare-`u16`
    /// miss sentinel) and field names that no longer resolve (`field_id_for_name → None`). Empty ⇒
    /// conformant. Shared by the real gate and the teeth test (which feeds it a bogus list). The
    /// `named = true` resolution mirrors `grammar_schema`'s `node_kind_is_named` filter, so "exists"
    /// means the same thing in both directions.
    fn dead_references(language: &Language, kinds: &[&str], fields: &[&str]) -> Vec<String> {
        let mut dead = Vec::new();
        for &k in kinds {
            if language.id_for_node_kind(k, true) == 0 {
                dead.push(format!("node-kind {k:?}"));
            }
        }
        for &f in fields {
            if language.field_id_for_name(f).is_none() {
                dead.push(format!("field {f:?}"));
            }
        }
        dead
    }

    /// The named grammar kinds the linked `Language` exposes (the same set `grammar_schema` lists).
    fn named_kinds(language: &Language) -> Vec<&'static str> {
        let mut v: Vec<&str> = (0..language.node_kind_count())
            .filter_map(|id| {
                let id = id as u16;
                language
                    .node_kind_is_named(id)
                    .then(|| language.node_kind_for_id(id))
                    .flatten()
            })
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// THE GATE. Every kind + field each frontend's table references must still exist in its linked
    /// grammar; an absence is grammar drift — the construct's lowering is dead and now routes to
    /// `native()`, a silent hash move — so the gate fails loudly, naming the dead construct.
    #[test]
    fn every_referenced_construct_exists_in_the_linked_grammar() {
        for &lang in GATED {
            let language = lang.ts_language();
            let (kinds, fields) = referenced(lang);
            let dead = dead_references(&language, &kinds, &fields);
            assert!(
                dead.is_empty(),
                "{lang:?} frontend references {} construct(s) absent from the linked tree-sitter \
                 grammar — grammar drift; each lowering is dead and now routes to native(): {dead:?}",
                dead.len(),
            );
        }
    }

    /// The one residue reference that is a *suffix* match, not a literal kind: Rust's `return_tail`
    /// treats any `*_item` node as a non-value (a nested item, not a tail expression). The gate can't
    /// enumerate `_item` as a MAP/residue string, so it is checked structurally here — the Rust
    /// grammar must still expose at least one named `*_item` kind, else the guard is dead.
    #[test]
    fn rust_item_suffix_guard_is_live() {
        let language = Lang::Rust.ts_language();
        assert!(
            named_kinds(&language).iter().any(|k| k.ends_with("_item")),
            "Rust grammar exposes no `*_item` named kind — `return_tail`'s suffix guard is dead",
        );
    }

    /// TEETH. Prove the gate is not vacuous: a deliberately-bogus kind and field are reported as
    /// dead, while the real tables report nothing (the same check the real gate runs).
    #[test]
    fn gate_has_teeth_on_a_bogus_reference() {
        let language = Lang::Rust.ts_language();
        let dead = dead_references(
            &language,
            &["while_expression", "definitely_not_a_kind_42"],
            &["condition", "definitely_not_a_field_42"],
        );
        assert!(
            dead.contains(&"node-kind \"definitely_not_a_kind_42\"".to_string()),
            "gate failed to flag a bogus node-kind: {dead:?}",
        );
        assert!(
            dead.contains(&"field \"definitely_not_a_field_42\"".to_string()),
            "gate failed to flag a bogus field: {dead:?}",
        );
        assert_eq!(
            dead.len(),
            2,
            "gate flagged a real construct as dead (false positive): {dead:?}",
        );
    }

    /// PARAM COVERAGE. Each frontend lowers parameters in a hand-written `lower_params` (not the
    /// `lower_node` table), so its parameter CST kinds were invisible to `referenced()` and could
    /// silently rot to `native()` on a grammar rename. Registering them in `RESIDUE_KINDS` closes
    /// that: this test proves (a) every param kind is now in the `referenced()` set — so the forward
    /// gate iterates over it — and (b) each resolves in the linked grammar today. Together with the
    /// teeth test (`dead_references` flags a non-resolving kind), a grammar rename/removal of a param
    /// kind now trips `every_referenced_construct_exists_in_the_linked_grammar` instead of routing
    /// params to `native()` unseen.
    #[test]
    fn param_kinds_are_gated() {
        // The exact kinds each frontend's `lower_params` matches on.
        let cases: &[(Lang, &[&str])] = &[
            (Lang::Rust, &["parameter"]),
            (Lang::Python, &["typed_parameter", "default_parameter"]),
            (Lang::Go, &["parameter_declaration"]),
        ];
        for &(lang, param_kinds) in cases {
            let (kinds, _) = referenced(lang);
            let referenced: std::collections::HashSet<&str> = kinds.into_iter().collect();
            let language = lang.ts_language();
            for &pk in param_kinds {
                assert!(
                    referenced.contains(pk),
                    "{lang:?} param kind {pk:?} is ungated: absent from referenced() (MAP ∪ \
                     RESIDUE_KINDS) — the forward gate can't see it, so a grammar rename would \
                     silently route params to native()",
                );
                assert_ne!(
                    language.id_for_node_kind(pk, true),
                    0,
                    "{lang:?} param kind {pk:?} no longer resolves in the linked grammar — \
                     `lower_params` is dead; the forward gate should now be flagging this",
                );
            }
        }
    }

    /// INVERSE ADVISORY. The named grammar kinds no table models (grammar named kinds − MAP keys −
    /// `RESIDUE_KINDS`) — the set that falls through to `native()`. Snapshotted so a grammar bump
    /// that adds or renames a kind produces a reviewable diff here (a new construct to consider
    /// modeling), complementing the forward gate (which catches removals/renames of *referenced*
    /// kinds). Regenerate a reviewed bump with `UPDATE_GRAMMAR_SNAPSHOTS=1 cargo test`.
    #[test]
    fn unmapped_kinds_advisory_matches_snapshot() {
        for &lang in GATED {
            let language = lang.ts_language();
            let (kinds, _) = referenced(lang);
            let referenced: std::collections::HashSet<&str> = kinds.into_iter().collect();
            let unmapped: Vec<&str> = named_kinds(&language)
                .into_iter()
                .filter(|k| !referenced.contains(k))
                .collect();

            let mut actual = String::new();
            actual.push_str(&format!("grammar: {} (unmapped → native)\n", lang.name()));
            actual.push_str(&format!("unmapped named kinds ({}):\n", unmapped.len()));
            for k in &unmapped {
                actual.push_str("  ");
                actual.push_str(k);
                actual.push('\n');
            }

            let file = format!("unmapped-{}.snap", lang.name());
            let path = format!(
                "{}/src/frontend/snapshots/{}",
                env!("CARGO_MANIFEST_DIR"),
                file
            );
            if std::env::var_os("UPDATE_GRAMMAR_SNAPSHOTS").is_some() {
                std::fs::write(&path, &actual).expect("write unmapped snapshot");
                continue;
            }
            let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!("missing unmapped snapshot {path}: {e}; regenerate with UPDATE_GRAMMAR_SNAPSHOTS=1")
            });
            assert_eq!(
                actual, expected,
                "unmapped-kinds advisory for {lang:?} drifted; a grammar bump may have added/renamed \
                 a kind — review, then regenerate with UPDATE_GRAMMAR_SNAPSHOTS=1"
            );
        }
    }
}
