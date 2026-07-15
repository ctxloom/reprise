//! P5: best-effort inliner (spec §5.4).
//!
//! Purpose: converge "caller uses existing helper" with "LLM reimplemented the
//! helper's body inline in another function." Resolution is name+arity within
//! the language, preferring same-file, then same-directory, then repo-wide —
//! purely syntactic, unsound by design (spec §1 insight 1): a wrong inline
//! yields at worst a noisy candidate pair, never a wrong program.
//!
//! Everything here operates on RAW trees (pre-pass, `Label::Raw` identifiers):
//! the definition table stores raw bodies and splicing happens before
//! `apply_passes`, so recursion lowering fires on SCC-expanded units — the
//! whole point of the mutual-recursion chain (inline SCC partner once → direct
//! self-recursion → Rev 5 lowering → loop core).

use crate::config::Config;
use crate::intern::{Field, Kind, LSym, LabelInterner};
use crate::ir::kind;
use crate::lang::{Lang, LanguageProfile, child_field};
use crate::tree::{Label, NormNode};
use crate::unit::Unit;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static BLOCK_LOWER: LazyLock<Kind> = LazyLock::new(|| Kind::intern("block"));
/// Sentinel "no parent" kind for [`substitute`]'s root call — mirrors
/// `normalize.rs`'s `abstract_idents::walk`, whose root call passes the SAME `""`
/// sentinel (both feed `LanguageProfile::always_external`'s `parent_kind`, and
/// no real grammar/IR kind is ever the empty string, so it never collides).
static EMPTY_KIND: LazyLock<Kind> = LazyLock::new(|| Kind::intern(""));
static IDENTIFIER: LazyLock<Kind> = LazyLock::new(|| Kind::intern("identifier"));
static KEYWORD_ARGUMENT: LazyLock<Kind> = LazyLock::new(|| Kind::intern("keyword_argument"));
static EXPRESSION_STATEMENT: LazyLock<Kind> =
    LazyLock::new(|| Kind::intern("expression_statement"));
static MACRO_INVOCATION: LazyLock<Kind> = LazyLock::new(|| Kind::intern("macro_invocation"));
static PARAM_FIELD: LazyLock<Field> = LazyLock::new(|| Field::intern("param"));

/// The tree-shape primitives the inliner reads through, so one algorithm serves
/// both the historical per-grammar trees and the canonical IR (`normalizer = "ir"`).
/// The `Historical` arm reproduces the per-language [`LanguageProfile`] hooks
/// byte-for-byte; the `Ir` arm returns the canonical constants
/// (`docs/SIMILARITY-IR.md`), language-neutral except the expression-block
/// distinction (only Rust has one — DECISIONS.md D17).
#[derive(Clone, Copy)]
pub enum Shapes {
    Historical(&'static dyn LanguageProfile),
    Ir(Lang),
}

impl Shapes {
    /// Pick the shape family a unit was extracted with: IR when the active normalizer
    /// is `"ir"` and `lang` has a frontend, else the historical profile (which also
    /// covers the TS/Kotlin fallback under `normalizer = "ir"`, §9 P2).
    pub fn for_lang(lang: Lang, cfg: &Config) -> Shapes {
        // Single IR-eligibility predicate, shared with the api tier's selector
        // (`api::Shapes::for_lang`), so the two can't drift (they once re-spelled
        // this condition independently).
        if crate::unit::is_ir(lang, cfg) {
            Shapes::Ir(lang)
        } else {
            Shapes::Historical(lang.profile())
        }
    }

    fn call_kind(&self) -> Kind {
        match self {
            Shapes::Historical(p) => p.call_kind(),
            Shapes::Ir(_) => kind::id::CALL,
        }
    }

    fn block_kind(&self) -> Kind {
        match self {
            Shapes::Historical(_) => *BLOCK_LOWER,
            Shapes::Ir(_) => kind::id::BLOCK,
        }
    }

    pub(crate) fn params(&self, root: &NormNode) -> Option<Vec<Box<str>>> {
        match self {
            Shapes::Historical(p) => p.inline_params(root),
            // The unit's `Var@param` children carry the simple `Raw` names; a
            // non-simple param (should not occur post-lowering) bails, as historical.
            Shapes::Ir(_) => {
                let mut out = Vec::new();
                for c in &root.children {
                    if c.field != Some(*PARAM_FIELD) {
                        continue;
                    }
                    match &c.label {
                        Some(Label::Raw(t)) if c.kind == kind::id::VAR => out.push(t.clone()),
                        _ => return None,
                    }
                }
                Some(out)
            }
        }
    }

    /// `(callee name, positional arg subtrees)` for a plain-identifier call with
    /// positionally mappable arguments; None otherwise (methods, keyword args).
    /// `kw_arg` is the scan's interned id for the synthetic `keyword_argument`
    /// tag — interned ONCE per scan by the caller (rule 5), never per node here.
    fn call_parts<'a>(
        &self,
        node: &'a NormNode,
        kw_arg: LSym,
    ) -> Option<(Box<str>, Vec<&'a NormNode>)> {
        if node.kind != self.call_kind() {
            return None;
        }
        match self {
            Shapes::Historical(_) => {
                let f = child_field(node, "function")?;
                if f.kind != *IDENTIFIER {
                    return None;
                }
                let Some(Label::Raw(name)) = &f.label else {
                    return None;
                };
                let args = child_field(node, "arguments")?;
                if args.children.iter().any(|a| a.kind == *KEYWORD_ARGUMENT) {
                    return None;
                }
                Some((name.clone(), args.children.iter().collect()))
            }
            Shapes::Ir(_) => {
                let f = child_field(node, "callee")?;
                if f.kind != kind::id::VAR {
                    return None;
                }
                // A `Raw` callee is a plain call to a resolvable function; an `External`
                // callee (a method via `Field`, or a builtin) is never inlinable.
                let Some(Label::Raw(name)) = &f.label else {
                    return None;
                };
                let args: Vec<&NormNode> = node
                    .children
                    .iter()
                    .filter(|c| c.field == Some(crate::ir::field::id::ARG))
                    .collect();
                // Python keyword args lower to a `NativeStmt` tagged `keyword_argument`;
                // they are not positionally mappable (mirrors the historical guard).
                if args
                    .iter()
                    .any(|a| matches!(&a.label, Some(Label::External(t)) if *t == kw_arg))
                {
                    return None;
                }
                Some((name.clone(), args))
            }
        }
    }

    /// The call node of a `f(...);` statement site (result discarded), else None.
    fn call_at_stmt<'a>(&self, child: &'a NormNode) -> Option<&'a NormNode> {
        match self {
            Shapes::Historical(_) => (child.kind == *EXPRESSION_STATEMENT
                && child.children.len() == 1)
                .then(|| &child.children[0]),
            // In the IR a bare expression IS the statement — a `Call` at block level.
            Shapes::Ir(_) => (child.kind == kind::id::CALL).then_some(child),
        }
    }

    fn return_value<'a>(&self, stmt: &'a NormNode) -> Option<&'a NormNode> {
        match self {
            Shapes::Historical(p) => p.return_value(stmt),
            Shapes::Ir(_) => (stmt.kind == kind::id::RETURN && stmt.children.len() == 1)
                .then(|| &stmt.children[0]),
        }
    }

    fn make_return(&self, value: NormNode) -> NormNode {
        match self {
            Shapes::Historical(p) => p.make_return(value),
            Shapes::Ir(_) => {
                let span = value.span;
                let mut v = value;
                v.field = Some(crate::ir::field::id::VALUE);
                NormNode::with_kind(kind::id::RETURN, None, span, vec![v])
            }
        }
    }

    fn make_expr_stmt(&self, expr: NormNode) -> NormNode {
        match self {
            Shapes::Historical(p) => p.make_expr_stmt(expr),
            Shapes::Ir(_) => {
                let mut e = expr;
                e.field = None; // the bare expression is the statement
                e
            }
        }
    }

    fn make_expr_block(&self, span: (u32, u32), children: Vec<NormNode>) -> Option<NormNode> {
        match self {
            Shapes::Historical(p) => p.make_expr_block(span, children),
            // Only Rust has an expression block (D17); Python/Go skip the site. The
            // caller (`ir_place_expr`) has already reshaped the block's tail value.
            Shapes::Ir(Lang::Rust) => {
                Some(NormNode::with_kind(kind::id::BLOCK, None, span, children))
            }
            Shapes::Ir(_) => None,
        }
    }

    /// A statement-shaped node (vs a bare trailing expression to be reshaped).
    fn is_statement_like(&self, node: &NormNode) -> bool {
        match self {
            Shapes::Historical(_) => is_statement_like(node),
            Shapes::Ir(_) => {
                node.kind == kind::id::ASSIGN
                    || node.kind == kind::id::RETURN
                    || node.kind == kind::id::BREAK
                    || node.kind == kind::id::CONTINUE
                    || node.kind == kind::id::LOOP
                    || node.kind == kind::id::BRANCH
                    || node.kind == kind::id::ITER
                    || node.kind == kind::id::NATIVE_STMT
            }
        }
    }

    /// The trailing value to reshape at a statement splice tail — a bare expression,
    /// or (IR only) a `Return`'s value; None for a real statement. Historical keeps its
    /// `pop_if(!is_statement_like)` semantics byte-for-byte.
    fn tail_value(&self, last: &NormNode) -> Option<NormNode> {
        match self {
            Shapes::Historical(_) => (!is_statement_like(last)).then(|| last.clone()),
            Shapes::Ir(_) => {
                if let Some(v) = self.return_value(last) {
                    Some(v.clone())
                } else if !self.is_statement_like(last) {
                    Some(last.clone())
                } else {
                    None
                }
            }
        }
    }

    fn is_identifier(&self, kind: Kind) -> bool {
        match self {
            Shapes::Historical(p) => p.is_identifier(kind.as_str()),
            Shapes::Ir(_) => kind == kind::id::VAR,
        }
    }

    fn always_external(&self, kind: Kind, field: Option<Field>, parent_kind: Kind) -> bool {
        match self {
            Shapes::Historical(p) => p.always_external(kind, field, parent_kind),
            // IR external names already carry `Label::External`, so a `Raw` match is
            // always a genuine local — nothing is structurally always-external here.
            Shapes::Ir(_) => false,
        }
    }
}

pub struct Def {
    pub unit_idx: usize,
    pub name: Box<str>,
    pub arity: usize,
    pub lang: Lang,
    pub file: PathBuf,
    pub params: Vec<Box<str>>,
    /// Root body's immediate child count (WP-D: from the projection, not a
    /// borrowed tree) — always `Some` in the projection for an admitted `Def`
    /// (`DefTable::build` only admits a unit when its body field resolved),
    /// unwrapped here. `try_expr_inline`'s `single` check reads this instead
    /// of `def.body.children.len()`.
    pub body_child_count: u32,
    /// Raw body token count — the `max_callee_tokens` basis (D18: measured on
    /// raw, because post-fold unit counts under-report the spliced mass).
    /// WP-D: read from the projection instead of computed from a borrowed body.
    pub body_tokens: u32,
}

/// Repo-wide definition table plus the syntactic call graph's SCCs (Tarjan).
pub struct DefTable {
    pub defs: Vec<Def>,
    /// (lang, arity) → name → defs; nested so lookups borrow the name
    /// (M3b: a Box<str> allocation per resolve dominated the inline phase
    /// at 500k LOC).
    by_key: HashMap<(Lang, usize), HashMap<Box<str>, Vec<usize>>>,
    def_of_unit: HashMap<usize, usize>,
    scc_of: Vec<usize>,
    scc_sizes: Vec<usize>,
}

enum Resolution {
    Hit(usize),
    Ambiguous,
    Miss,
}

impl DefTable {
    /// Build from the per-unit metadata projection (WP-D: no raw tree is
    /// walked or borrowed here at all — `call_sites` already carries every
    /// call site's (name, arity, span) and each unit's params/body shape,
    /// computed once at extraction). Index-aligned with `units`. Deterministic:
    /// `units` arrive in sorted file order, and candidate lists keep that
    /// order (spec §5.4 determinism requirement).
    pub fn build(
        units: &[Unit],
        call_sites: &[UnitCallSites],
        cfg: &Config,
        li: &LabelInterner,
    ) -> DefTable {
        let _ = li; // no longer needed to build the table (WP-D: calls are pre-collected)
        use rayon::prelude::*;
        // Parallel with order preserved (indexed collect): determinism holds.
        let defs: Vec<Def> = units
            .par_iter()
            .enumerate()
            .filter_map(|(i, unit)| {
                if unit.name == "<anon>" {
                    return None;
                }
                let cs = &call_sites[i];
                // Mirrors the old `shapes.params(&raw_trees[i])?; child_field(&raw_trees[i],
                // "body")?;` admission: both must resolve, or this unit is never a
                // resolution TARGET.
                let params = cs.params.clone()?;
                let body_child_count = cs.body_child_count?;
                Some(Def {
                    unit_idx: i,
                    name: unit.name.as_str().into(),
                    arity: params.len(),
                    lang: unit.lang,
                    file: unit.file.clone(),
                    params,
                    body_child_count,
                    body_tokens: cs.body_tokens,
                })
            })
            .collect();
        let mut by_key: HashMap<(Lang, usize), HashMap<Box<str>, Vec<usize>>> = HashMap::new();
        let mut def_of_unit = HashMap::new();
        for (d, def) in defs.iter().enumerate() {
            by_key
                .entry((def.lang, def.arity))
                .or_default()
                .entry(def.name.clone())
                .or_default()
                .push(d);
            def_of_unit.insert(def.unit_idx, d);
        }
        let mut table = DefTable {
            defs,
            by_key,
            def_of_unit,
            scc_of: Vec::new(),
            scc_sizes: Vec::new(),
        };
        // Syntactic call graph → Tarjan SCCs (spec §5.4). Parallel per def
        // (order-preserving collect keeps the §5.4 determinism requirement).
        // WP-D: no tree walk here at all — `call_sites[def.unit_idx].calls` was
        // collected once at extraction, by `collect_call_sites` (this pass used
        // to walk `def.body` here directly via the now-deleted `collect_calls`).
        let adj: Vec<Vec<usize>> = table
            .defs
            .par_iter()
            .map(|def| {
                let mut edges: Vec<usize> = call_sites[def.unit_idx]
                    .calls
                    .iter()
                    .filter_map(|(name, arity, _span)| {
                        match table.resolve(def.lang, name, *arity, &def.file, cfg) {
                            Resolution::Hit(d) => Some(d),
                            _ => None,
                        }
                    })
                    .collect();
                edges.sort_unstable();
                edges.dedup();
                edges
            })
            .collect();
        table.scc_of = tarjan(table.defs.len(), &adj);
        let mut sizes = vec![0usize; table.defs.len()];
        for &s in &table.scc_of {
            sizes[s] += 1;
        }
        table.scc_sizes = sizes;
        table
    }

    /// Units belonging to a mutual-recursion SCC of size ≥2 (scan stat).
    pub fn scc_unit_count(&self) -> usize {
        (0..self.defs.len())
            .filter(|&d| self.scc_sizes[self.scc_of[d]] >= 2)
            .count()
    }

    /// Name+arity resolution with same-file → same-dir → repo-wide preference;
    /// more than `max_candidates` after preference filtering is an ambiguity.
    fn resolve(
        &self,
        lang: Lang,
        name: &str,
        arity: usize,
        from_file: &Path,
        cfg: &Config,
    ) -> Resolution {
        let Some(all) = self.by_key.get(&(lang, arity)).and_then(|m| m.get(name)) else {
            return Resolution::Miss;
        };
        let tiered = |pred: &dyn Fn(&Def) -> bool| -> Vec<usize> {
            all.iter()
                .copied()
                .filter(|&d| pred(&self.defs[d]))
                .collect()
        };
        let same_file = tiered(&|def| def.file == from_file);
        let pool = if !same_file.is_empty() {
            same_file
        } else {
            let dir = from_file.parent();
            let same_dir = tiered(&|def| def.file.parent() == dir);
            if !same_dir.is_empty() {
                same_dir
            } else {
                all.clone()
            }
        };
        if pool.len() > cfg.inline.max_candidates {
            return Resolution::Ambiguous;
        }
        // ≤ max_candidates: take the first in deterministic (file, span) order;
        // candidate lists preserve the sorted unit order.
        Resolution::Hit(pool[0])
    }
}

/// Extraction-time projection of one unit's raw tree (WP-D, raw-trees
/// elimination): everything `DefTable::build`'s adjacency pass and
/// `expand_unit`'s pre-walk resolvability check read, with ZERO raw-tree bytes
/// retained. Built once per unit, at extraction (`corpus_units_from`), from the
/// SAME `Shapes` methods the inliner already uses — never re-spelled.
/// Index-aligned with `units`/`raw_trees` (one entry per unit, same alignment
/// `raw_trees` had — including non-plain/anon units, which `DefTable::build`
/// still filters by name exactly as before).
#[derive(Debug, Clone)]
pub struct UnitCallSites {
    /// Root body's immediate child count — `None` when this unit's root has NO
    /// "body" field at all (NOT the same as a body with 0 children): measured
    /// against the real grammars, Kotlin's tree-sitter grammar has no `body`
    /// FIELD whatsoever (the body is a same-kind child, located structurally —
    /// `src/lang/kotlin.rs`'s own doc comment), so `child_field(unit, "body")`
    /// is `None` for EVERY Kotlin unit; historical-mode (non-default) Go can
    /// also produce a body-less declaration (an assembly-linked/`go:linkname`
    /// func) since, unlike the IR path, the historical `normalize::convert`
    /// does not synthesize a placeholder body. Collapsing this to a bare `u32`
    /// (0 for both "no field" and "empty body") would flip `expand_unit`'s
    /// `is_some_and(|b| b.children.len() <= 1)` thin-delegation check from
    /// "never fires" to "always fires" for every Kotlin unit — silently
    /// disabling the inliner for that language. Two independent readers:
    /// `expand_unit`'s thin-delegation fast skip (`Some(n) if n <= 1` → no
    /// variant, D37) when THIS unit is a caller, and `try_expr_inline`'s
    /// `single` check (`def.body.children.len() == 1`, i.e. `Some(1)` here)
    /// when THIS unit is spliced as a callee — always `Some` in that reader,
    /// since `DefTable::build` only admits a unit as a `Def` when its body
    /// field resolved.
    pub body_child_count: Option<u32>,
    /// D18 basis (`max_callee_tokens`); moved from "computed in DefTable::build
    /// from a borrowed body" to "computed once at extraction."
    pub body_tokens: u32,
    /// Every positionally-mappable call site anywhere in the body subtree
    /// (same reach as [`collect_call_sites`]): callee name, arg count, byte
    /// span. A call node
    /// that `Shapes::call_parts` would reject (method call, kw-arg,
    /// non-identifier callee) is OMITTED — it can never resolve either way, so
    /// it carries no information the pre-walk decision needs.
    pub calls: Vec<(Box<str>, usize, (u32, u32))>,
    /// `Shapes::params(&unit_tree)` — `None` when this unit cannot be a
    /// resolution TARGET (non-simple params). `DefTable::build` only admits a
    /// unit into `defs` when this is `Some`.
    pub params: Option<Vec<Box<str>>>,
}

/// Collects every call site in a subtree as `(name, arity, span)` (WP-D): the
/// span is needed because `resolve_policy`'s ambiguity tracking
/// (`ctx.ambiguous_sites.insert(node.span)`) must be reproducible from the
/// projection alone, so the projection-based precheck can build the same
/// `ambiguous_sites` set the tree-walking oracle does.
pub(crate) fn collect_call_sites(
    node: &NormNode,
    shapes: Shapes,
    out: &mut Vec<(Box<str>, usize, (u32, u32))>,
    kw_arg: LSym,
) {
    if let Some((name, args)) = shapes.call_parts(node, kw_arg) {
        out.push((name, args.len(), node.span));
    }
    for child in &node.children {
        collect_call_sites(child, shapes, out, kw_arg);
    }
}

/// Iterative Tarjan; returns SCC id per node.
fn tarjan(n: usize, adj: &[Vec<usize>]) -> Vec<usize> {
    let mut index = vec![usize::MAX; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut scc = vec![0usize; n];
    let mut next_index = 0usize;
    let mut scc_count = 0usize;
    for start in 0..n {
        if index[start] != usize::MAX {
            continue;
        }
        let mut work: Vec<(usize, usize)> = vec![(start, 0)];
        while let Some(&(v, child_pos)) = work.last() {
            if child_pos == 0 {
                index[v] = next_index;
                low[v] = next_index;
                next_index += 1;
                stack.push(v);
                on_stack[v] = true;
            }
            if child_pos < adj[v].len() {
                work.last_mut().unwrap().1 += 1;
                let w = adj[v][child_pos];
                if index[w] == usize::MAX {
                    work.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
            } else {
                work.pop();
                if let Some(&(u, _)) = work.last() {
                    low[u] = low[u].min(low[v]);
                }
                if low[v] == index[v] {
                    loop {
                        let w = stack.pop().unwrap();
                        on_stack[w] = false;
                        scc[w] = scc_count;
                        if w == v {
                            break;
                        }
                    }
                    scc_count += 1;
                }
            }
        }
    }
    scc
}

pub struct Expansion {
    /// The spliced tree — `None` iff `calls_inlined == 0` (thin-delegation skip,
    /// or a real walk that resolved and inlined nothing). The raw-tree clone
    /// this requires is expensive (M3c: ~55% of units inline zero calls and
    /// produce no variant, so `lib.rs` discards `tree` unread whenever
    /// `calls_inlined == 0` — see `any_resolvable_call`, which proves that case
    /// cheaply, read-only, before paying for the clone).
    pub tree: Option<NormNode>,
    /// Callee names expanded (deduped, expansion order) — the inline chain.
    pub chain: Vec<String>,
    /// Unit indices of the inlined callees (D3 tautology filter keys).
    pub expanded_units: Vec<usize>,
    pub scc: bool,
    pub calls_inlined: u32,
    pub ambiguity_skips: u32,
    /// This unit got no variant because its expansion exceeded
    /// `inline.max_expansion_nodes` — NOT because it had nothing to inline. The two
    /// are otherwise identical (`tree: None`, `calls_inlined: 0`), and conflating
    /// them would make the backstop a silent recall loss; `scan()` surfaces this as
    /// `Stats::inline_budget_skipped_units`.
    pub budget_skipped: bool,
}

impl Expansion {
    /// No-op expansion (unit skipped: thin delegation, D37, or no resolvable call).
    fn skipped(ambiguity_skips: u32) -> Expansion {
        Expansion {
            tree: None,
            chain: Vec::new(),
            expanded_units: Vec::new(),
            scc: false,
            calls_inlined: 0,
            ambiguity_skips,
            budget_skipped: false,
        }
    }

    /// The unit's expansion blew the aggregate node budget: no variant (policy
    /// `skip` — never a truncated tree), and it says so.
    fn budget_skipped(ambiguity_skips: u32) -> Expansion {
        Expansion {
            budget_skipped: true,
            ..Expansion::skipped(ambiguity_skips)
        }
    }
}

struct Ctx<'a> {
    table: &'a DefTable,
    /// WP-D transitional scaffold (step 2 only, deleted in step 4 when the
    /// raw-tree memo replaces it): `splice_body` needs a callee's body
    /// subtree, and `Def` no longer carries one — index-aligned with `units`,
    /// same slice `expand_unit`'s `raw` parameter is drawn from.
    raw_trees: &'a [NormNode],
    cfg: &'a Config,
    shapes: Shapes,
    lang: Lang,
    file: &'a Path,
    unit_idx: usize,
    root_name: Box<str>,
    /// Def indices in the root unit's SCC (empty unless SCC size ≥2). A partner may
    /// relax the depth/size caps to let the SCC round reach self-recursion (spec §5.4),
    /// but only within `max_scc_depth` — see `resolve_policy`'s bypass site for why the
    /// relaxation must stay rationed.
    scc_partners: HashSet<usize>,
    has_expr_block: bool,
    /// Defs currently being expanded — the cycle stop ("stop when a cycle
    /// would repeat a member").
    stack: Vec<usize>,
    chain: Vec<String>,
    expanded_units: Vec<usize>,
    scc_hit: bool,
    calls_inlined: u32,
    /// Distinct ambiguous call sites (by span): a site rejected at the
    /// statement level is revisited at expression level and must count once.
    ambiguous_sites: HashSet<(u32, u32)>,
    /// SCC-partner splices currently on the stack (the `max_scc_depth` basis).
    scc_splices: u32,
    /// Spliced nodes charged against `max_expansion_nodes` so far.
    spliced_nodes: u64,
    /// Set when a splice was refused for budget; the unit gets no variant.
    over_budget: bool,
    /// The scan's interned id for the synthetic `keyword_argument` tag
    /// (interning-id-conversion WP) — computed ONCE at `Ctx` construction via the
    /// per-scan `LabelInterner` (rule 5), so the IR path's `keyword_argument`
    /// check in `Shapes::call_parts` compares bare `LSym == LSym`, never resolving
    /// per node on the `resolve_policy` hot path.
    kw_arg_sym: LSym,
}

/// Fully-inline-per-policy expansion of one unit's raw tree (spec §5.4).
// WP-D transitional: `raw_trees` (step 2's scaffold) drops out in step 4 when
// the memo replaces it, bringing this back under clippy's default arg count.
#[allow(clippy::too_many_arguments)]
pub fn expand_unit(
    unit_idx: usize,
    raw: &NormNode,
    // WP-D transitional scaffold (step 2 only; see `Ctx::raw_trees`) — deleted
    // in step 4 when `splice_body` fetches a callee's body through the memo
    // instead.
    raw_trees: &[NormNode],
    call_sites: &[UnitCallSites],
    units: &[Unit],
    table: &DefTable,
    cfg: &Config,
    li: &LabelInterner,
) -> Expansion {
    let lang = units[unit_idx].lang;
    let shapes = Shapes::for_lang(lang, cfg);
    // Pure delegation bodies (a single top-level statement, i.e. a thin
    // wrapper around one call) get no inline variant: expanding one folds the
    // helper back in, so freshly-extracted helpers' wrappers would re-match
    // each other — reprise flagging its own recommended fix (user feedback,
    // D37). Real reimplemented-helper cases have surrounding code. WP-D:
    // projection-based — `None` (no "body" field at all, e.g. every Kotlin
    // unit) must NOT trigger this, only `Some(n) if n <= 1` — see
    // `UnitCallSites::body_child_count`'s doc comment.
    if call_sites[unit_idx]
        .body_child_count
        .is_some_and(|n| n <= 1)
    {
        return Expansion::skipped(0);
    }
    let scc_partners: HashSet<usize> = match table.def_of_unit.get(&unit_idx) {
        Some(&d) if table.scc_sizes[table.scc_of[d]] >= 2 => (0..table.defs.len())
            .filter(|&e| e != d && table.scc_of[e] == table.scc_of[d])
            .collect(),
        _ => HashSet::new(),
    };
    let mut ctx = Ctx {
        table,
        raw_trees,
        cfg,
        shapes,
        lang,
        file: &units[unit_idx].file,
        unit_idx,
        root_name: units[unit_idx].name.as_str().into(),
        scc_partners,
        has_expr_block: shapes.make_expr_block((0, 0), Vec::new()).is_some(),
        stack: Vec::new(),
        chain: Vec::new(),
        expanded_units: Vec::new(),
        scc_hit: false,
        calls_inlined: 0,
        ambiguous_sites: HashSet::new(),
        scc_splices: 0,
        spliced_nodes: 0,
        over_budget: false,
        kw_arg_sym: li.intern("keyword_argument"),
    };
    // M3c: `raw.clone()` used to run unconditionally here, even though ~55% of
    // units inline zero calls and produce no variant (`lib.rs` discards `tree`
    // whenever `calls_inlined == 0` — the exact anti-pattern `DefTable::Def.body`
    // was already fixed for, above). WP-D: the precheck is now projection-based
    // (`any_resolvable_call_projected`) — it decides this over
    // `call_sites[unit_idx].calls` without touching a single tree node at all
    // (previously a read-only tree descent, `any_resolvable_call`, still kept
    // below as the differential-test oracle): if it proves no call resolves,
    // the real `walk` below is guaranteed to splice nothing, so both the
    // precheck AND the clone it guards are skipped outright.
    if !any_resolvable_call_projected(unit_idx, call_sites, &mut ctx) {
        // Mirrors `resolve_given`'s policy, so a budget of 0 refuses every call
        // here and no call ever "resolves" — the unit would exit through this
        // cheap path. That is still a BUDGET skip, not a benign one, and must
        // report itself as such. (For any budget ≥ 1 this cannot trigger: no
        // splice has happened yet during the read-only precheck, so
        // `spliced_nodes` is still 0 and the check `B <= 0` is false.)
        if ctx.over_budget {
            return Expansion::budget_skipped(ctx.ambiguous_sites.len() as u32);
        }
        return Expansion::skipped(ctx.ambiguous_sites.len() as u32);
    }
    let tree = walk(raw.clone(), &mut ctx);
    // Policy `skip` (design doc §4): over the aggregate expansion-node budget, the
    // unit gets NO variant at all — never a truncated one. This is what keeps
    // `max_expansion_nodes` out of the byte-identity surface: it only ever selects
    // a variant's presence/absence, never alters the content of one that IS emitted.
    if ctx.over_budget {
        return Expansion::budget_skipped(ctx.ambiguous_sites.len() as u32);
    }
    Expansion {
        tree: Some(tree),
        chain: ctx.chain,
        expanded_units: ctx.expanded_units,
        scc: ctx.scc_hit,
        calls_inlined: ctx.calls_inlined,
        ambiguity_skips: ctx.ambiguous_sites.len() as u32,
        budget_skipped: false,
    }
}

/// Projection-based precheck (WP-D): decides the SAME boolean as
/// `any_resolvable_call` (below, kept as this function's differential-test
/// oracle — see `inline_pushdown_precheck_matches_tree_walk` in the test
/// module) purely over `call_sites[unit_idx].calls`, touching zero raw-tree
/// bytes. Shares `resolve_given` with `resolve_policy` (the real walk's
/// per-call decision), so the two can never independently drift on the
/// POLICY itself — only (if at all) on whether `collect_call_sites`'s
/// traversal reach matches `any_resolvable_call`'s, which is exactly what the
/// differential test checks. Same early-exit-leaves-a-harmless-partial-
/// `ambiguous_sites` argument as `any_resolvable_call` applies here too: the
/// real `walk`, run only when this returns `true`, independently re-derives
/// the complete set from its own full traversal.
fn any_resolvable_call_projected(
    unit_idx: usize,
    call_sites: &[UnitCallSites],
    ctx: &mut Ctx,
) -> bool {
    call_sites[unit_idx]
        .calls
        .iter()
        .any(|&(ref name, arity, span)| resolve_given(name, arity, span, ctx).is_some())
}

/// Read-only descent proving whether `node`'s subtree contains at least one call
/// `resolve_policy` would accept — i.e. whether the real `walk` below could
/// possibly splice anything. Visits every node (same reach as `walk`, mirrors
/// `collect_call_sites`'s traversal), so when no call resolves the search exhausts the
/// whole tree and `ctx.ambiguous_sites` comes out complete — the zero-clone path
/// above reports `ambiguity_skips` straight from it. When a call DOES resolve, the
/// search returns early without visiting the rest; the caller then commits to the
/// clone + real `walk`, which independently re-derives the full `ambiguous_sites`
/// (and every other `ctx` field) from its own complete traversal, so the partial
/// set left behind by an early exit is a harmless subset (`HashSet` insertion is
/// idempotent) — no reset needed between the two.
///
/// WP-D: no longer called by production (`expand_unit` now calls
/// `any_resolvable_call_projected`) — kept ONLY as the differential-test
/// oracle until `inline_expansion_resident_vs_memo_is_byte_identical` (step 4)
/// has run green, per the plan's own discipline (don't delete the oracle
/// before the test that needs it exists). Deleted in step 7.
#[cfg_attr(not(test), allow(dead_code))]
fn any_resolvable_call(node: &NormNode, ctx: &mut Ctx) -> bool {
    resolve_policy(node, ctx).is_some() || node.children.iter().any(|c| any_resolvable_call(c, ctx))
}

fn walk(mut node: NormNode, ctx: &mut Ctx) -> NormNode {
    if node.kind == ctx.shapes.block_kind() {
        let children = std::mem::take(&mut node.children);
        let n = children.len();
        let mut out = Vec::with_capacity(n);
        for (i, child) in children.into_iter().enumerate() {
            match try_statement_site(child, i + 1 == n, ctx) {
                Ok(stmts) => out.extend(stmts),
                Err(child) => out.push(walk(child, ctx)),
            }
        }
        node.children = out;
        return node;
    }
    node.children = node.children.into_iter().map(|c| walk(c, ctx)).collect();
    if node.kind == ctx.shapes.call_kind() {
        return try_expr_inline(node, ctx);
    }
    node
}

/// How the callee body's trailing bare expression (Rust) is re-shaped at a
/// statement splice site.
enum TailMode {
    /// Splicing replaced `return f(...)`: the trailing value is returned.
    Return,
    /// Splicing replaced `f(...);`: the trailing value is discarded.
    Discard,
    /// Splicing replaced a block-tail `f(...)`: the value stays the block tail.
    Keep,
}

/// Statement-level splice sites: `return f(...)`, `f(...);`, and (Rust)
/// block-tail `f(...)`. Returns the replacement statements, or gives the
/// child back untouched.
fn try_statement_site(
    child: NormNode,
    is_last: bool,
    ctx: &mut Ctx,
) -> Result<Vec<NormNode>, NormNode> {
    // `return f(...)` — splice the body; the callee's returns become the
    // caller's returns (exact for a tail call, tolerated elsewhere).
    if let Some(call) = ctx.shapes.return_value(&child)
        && let Some((def_idx, args)) = resolve_policy(call, ctx)
    {
        let body = splice_body(def_idx, args, ctx);
        return Ok(finish_tail(body, TailMode::Return, ctx));
    }
    // `f(...);` — result unused.
    if let Some(call) = ctx.shapes.call_at_stmt(&child)
        && let Some((def_idx, args)) = resolve_policy(call, ctx)
    {
        let body = splice_body(def_idx, args, ctx);
        return Ok(finish_tail(body, TailMode::Discard, ctx));
    }
    // Rust block-tail `f(...)` — the call's value is the block's value; the
    // body's statements splice flat and its trailing expression becomes the
    // new block tail (what an LLM writes when inlining by hand).
    if is_last
        && child.kind == ctx.shapes.call_kind()
        && let Some((def_idx, args)) = resolve_policy(&child, ctx)
    {
        let body = splice_body(def_idx, args, ctx);
        return Ok(finish_tail(body, TailMode::Keep, ctx));
    }
    Err(child)
}

/// Expression-position call: substitute a single-expression body directly;
/// wrap multi-statement bodies in the language's expression block (Rust) or
/// skip the site (Python — D17).
fn try_expr_inline(node: NormNode, ctx: &mut Ctx) -> NormNode {
    let Some((def_idx, args)) = resolve_policy(&node, ctx) else {
        return node;
    };
    let def = &ctx.table.defs[def_idx];
    let single = def.body_child_count == 1;
    if !single && !ctx.has_expr_block {
        return node; // D17: no synthetic expression block
    }
    let field = node.field;
    let span = node.span;
    let stmts = splice_body(def_idx, args, ctx);
    if let Shapes::Ir(_) = ctx.shapes {
        return ir_place_expr(stmts, field, span, node, ctx);
    }
    if stmts.len() == 1 {
        let only = &stmts[0];
        // `return X` one-liner (Python idiom) → X.
        if let Some(v) = ctx.shapes.return_value(only) {
            let mut v = v.clone();
            v.field = field;
            return v;
        }
        // Bare trailing expression (Rust expression body) → the expression.
        // Block-ending expressions (`if`/`match`/`loop`) parse wrapped in
        // `expression_statement` even in tail position; unwrap them.
        let inner = if only.kind == *EXPRESSION_STATEMENT && only.children.len() == 1 {
            &only.children[0]
        } else {
            only
        };
        if !ctx.shapes.is_statement_like(inner) {
            let mut v = inner.clone();
            v.field = field;
            return v;
        }
    }
    match ctx.shapes.make_expr_block(span, stmts) {
        Some(mut block) => {
            block.field = field;
            block
        }
        // A single-statement callee body can still expand to several statements
        // through nested inlining (`splice_body` runs `walk`); with no
        // expression block to land them in, leave the call un-inlined.
        None => node,
    }
}

/// Place a spliced IR callee body at an expression site. The body was lowered in
/// *return position* (the Rust frontend pushes tail `Return`s into branch arms /
/// nested blocks), so de-return the tail first ([`de_return_tail`]) to match the
/// hand-written expression form: a single value substitutes in directly (`if`/`match`
/// are expressions in the IR, so no `Block` wrapper), and a multi-statement body lands
/// in an expression block (Rust only — Python/Go leave the call, D17).
fn ir_place_expr(
    mut stmts: Vec<NormNode>,
    field: Option<crate::intern::Field>,
    span: (u32, u32),
    node: NormNode,
    ctx: &Ctx,
) -> NormNode {
    de_return_tail(&mut stmts);
    if stmts.len() == 1 {
        let mut v = stmts.pop().unwrap();
        v.field = field;
        return v;
    }
    match ctx.shapes.make_expr_block(span, stmts) {
        Some(mut block) => {
            block.field = field;
            block
        }
        None => node,
    }
}

/// Reverse the Rust frontend's return-position lowering on a spliced body's TAIL, so an
/// inlined value-returning function matches the hand-written expression form: a tail
/// `Return v` becomes bare `v`, and a tail `Branch`/`Block` recurses into its own tails
/// (`if c { return a } else { return b }` → `if c { a } else { b }`). Statement splice
/// sites keep their `Return`s (a `return f(x)` site wants them), so this runs only for
/// the expression path.
fn de_return_tail(stmts: &mut [NormNode]) {
    if let Some(last) = stmts.last_mut() {
        de_return_node(last);
    }
}

fn de_return_node(node: &mut NormNode) {
    let k = node.kind;
    if k == kind::id::RETURN && node.children.len() == 1 {
        let field = node.field;
        let mut v = node.children.remove(0);
        v.field = field;
        *node = v;
    } else if k == kind::id::BRANCH {
        for arm in &mut node.children {
            if let Some(body) = arm
                .children
                .iter_mut()
                .find(|c| c.field == Some(crate::ir::field::id::BODY))
                && body.kind == kind::id::BLOCK
            {
                de_return_tail(&mut body.children);
            }
        }
    } else if k == kind::id::BLOCK {
        de_return_tail(&mut node.children);
    }
}

/// The tree-free half of `resolve_policy` (WP-D): every §5.4 policy knob over
/// a call's already-extracted `(name, arity, span)` plus `Ctx`'s own scalar
/// state — no `NormNode` in sight. This is the escalation-#1 sufficiency
/// argument made real: `resolve_policy` (below) and the projection-based
/// `any_resolvable_call_projected` both call THIS, so the decision itself
/// cannot drift between the tree-walking and projection-based paths — only
/// (if at all) the reach of the calls fed into it could, which is exactly
/// what `collect_call_sites` vs `any_resolvable_call`'s traversal proves
/// equal (differential test). Counts ambiguity skips; enforces the
/// self-recursion and cycle guards, the aggregate expansion budget, depth,
/// and the callee size cap — the last two relaxed for an SCC partner only
/// within `max_scc_depth` (see the bypass site below).
fn resolve_given(name: &str, arity: usize, span: (u32, u32), ctx: &mut Ctx) -> Option<usize> {
    if name == ctx.root_name.as_ref() {
        return None; // never inline direct self-recursion (Rev 5 owns it)
    }
    let def_idx = match ctx.table.resolve(ctx.lang, name, arity, ctx.file, ctx.cfg) {
        Resolution::Hit(d) => d,
        Resolution::Ambiguous => {
            ctx.ambiguous_sites.insert(span);
            return None;
        }
        Resolution::Miss => return None,
    };
    let def = &ctx.table.defs[def_idx];
    if def.unit_idx == ctx.unit_idx || ctx.stack.contains(&def_idx) {
        return None;
    }
    // Aggregate per-unit expansion budget (backstop, checked BEFORE the SCC bypass —
    // the SCC round does not get to bypass it): over budget, the unit gets no variant
    // at all (policy skip, never a truncated one — `expand_unit` reads `over_budget`
    // after the walk and discards everything).
    if u64::from(ctx.cfg.inline.max_expansion_nodes) <= ctx.spliced_nodes {
        ctx.over_budget = true;
        return None;
    }
    // The SCC round is BOUNDED by the caps, never exempt from them.
    //
    // `max_callee_tokens` is a PRECISION guard, not merely a size/perf knob: splice a large
    // shared helper into two thin sibling wrappers and their bodies become mostly the HELPER's
    // mass, so they "match" each other on code that exists exactly once — a clone report
    // pointing at nothing duplicated. The size cap is what keeps such a helper out.
    //
    // And an SCC partner exempted from `max_depth` nests without bound: the cycle stop only
    // forbids REPEATING a member, so a chain of distinct partners descends as deep as the SCC
    // is large, at any body size.
    //
    // Hence the bypass is rationed by `max_scc_depth` — enough to turn mutual recursion into
    // direct self-recursion (the round's whole purpose) and no further. Never widen this to an
    // unconditional exemption.
    //
    // SCC partners bypass `max_depth`/`max_callee_tokens` ONLY while strictly fewer
    // than `max_scc_depth` partner splices are already on the stack — "inline SCC
    // partner once" (module doc), not as deep as the SCC has distinct members.
    let scc_bypass =
        ctx.scc_partners.contains(&def_idx) && ctx.scc_splices < ctx.cfg.inline.max_scc_depth;
    if !scc_bypass {
        if ctx.stack.len() >= ctx.cfg.inline.max_depth as usize {
            return None;
        }
        if def.body_tokens > ctx.cfg.inline.max_callee_tokens {
            return None;
        }
    }
    Some(def_idx)
}

/// Resolve a call node against the table and the §5.4 policy knobs, extracting
/// its positional arg subtrees (needed only at splice time — the tree-free
/// decision itself is `resolve_given`, shared with the projection-based
/// precheck).
fn resolve_policy(node: &NormNode, ctx: &mut Ctx) -> Option<(usize, Vec<NormNode>)> {
    let (name, args) = ctx.shapes.call_parts(node, ctx.kw_arg_sym)?;
    let def_idx = resolve_given(&name, args.len(), node.span, ctx)?;
    Some((def_idx, args.into_iter().cloned().collect()))
}

/// Substitute argument subtrees for parameter names in a copy of the callee
/// body, then recursively expand the copy (depth+1 via the stack).
fn splice_body(def_idx: usize, args: Vec<NormNode>, ctx: &mut Ctx) -> Vec<NormNode> {
    let def = &ctx.table.defs[def_idx];
    debug_assert_eq!(def.params.len(), args.len());
    let map: HashMap<&str, &NormNode> = def
        .params
        .iter()
        .map(|p| p.as_ref())
        .zip(args.iter())
        .collect();
    // WP-D transitional scaffold (step 2 only; see `Ctx::raw_trees`'s doc
    // comment) — `Def` no longer carries a borrowed body, so re-derive it from
    // the still-resident raw trees. `expect`: a `Def` is only ever admitted by
    // `DefTable::build` when its body field resolved (`UnitCallSites::
    // body_child_count` was `Some`), so this always finds one.
    let raw_body = child_field(&ctx.raw_trees[def.unit_idx], "body").expect(
        "a Def always has a body field — DefTable::build only admits units where \
         the projection's body_child_count resolved",
    );
    let body = substitute(raw_body.clone(), &map, ctx.shapes, *EMPTY_KIND);
    // Charge the budget after substitution (so duplicated argument subtrees are
    // counted) but before recursing, so a nested splice's own budget check sees
    // this splice's cost. Overshoot note: the check in `resolve_policy` runs
    // BEFORE a splice, so the final admitted splice may push the total past the
    // budget by its own size — bounded, measured at 0.07% on a 500k-node probe;
    // not worth a param-use-table prediction to close (see design doc §6.2).
    ctx.spliced_nodes += u64::from(body.token_count());
    let name = def.name.to_string();
    let unit_idx = def.unit_idx;
    let is_partner = ctx.scc_partners.contains(&def_idx);
    ctx.stack.push(def_idx);
    if is_partner {
        ctx.scc_splices += 1;
    }
    let body = walk(body, ctx);
    if is_partner {
        ctx.scc_splices -= 1;
    }
    ctx.stack.pop();
    if !ctx.chain.contains(&name) {
        ctx.chain.push(name);
    }
    if !ctx.expanded_units.contains(&unit_idx) {
        ctx.expanded_units.push(unit_idx);
    }
    ctx.calls_inlined += 1;
    ctx.scc_hit |= is_partner;
    body.children
}

/// Re-shape a spliced body's trailing value per site kind. The trailing value is a
/// bare expression (historical) or a `Return`'s value (IR, whose value-returning
/// bodies end in an explicit `Return`) — see [`Shapes::tail_value`].
fn finish_tail(mut stmts: Vec<NormNode>, mode: TailMode, ctx: &Ctx) -> Vec<NormNode> {
    if let Some(last) = stmts.last().and_then(|l| ctx.shapes.tail_value(l)) {
        stmts.pop();
        stmts.push(match mode {
            TailMode::Return => ctx.shapes.make_return(last),
            TailMode::Discard => ctx.shapes.make_expr_stmt(last),
            TailMode::Keep => last,
        });
    }
    stmts
}

/// Statement-shaped nodes (vs a bare trailing expression). The suffix checks
/// cover kinds outside the profile's factorability list (items, declarations).
/// Resolves to the actual grammar string (`.as_str()`) — a suffix check
/// (`ends_with`) cannot be expressed as id equality, and this runs once per
/// splice-tail decision (bounded by inline expansion sites), never on the
/// `anti_unify` per-node-pair hot path.
fn is_statement_like(node: &NormNode) -> bool {
    let kind = node.kind.as_str();
    kind.ends_with("_statement")
        || kind.ends_with("_declaration")
        || kind.ends_with("_item")
        || node.kind == *EXPRESSION_STATEMENT
        || node.kind == *MACRO_INVOCATION
}

fn substitute(
    mut node: NormNode,
    map: &HashMap<&str, &NormNode>,
    shapes: Shapes,
    parent_kind: Kind,
) -> NormNode {
    if shapes.is_identifier(node.kind)
        && !shapes.always_external(node.kind, node.field, parent_kind)
        && let Some(Label::Raw(text)) = &node.label
        && let Some(rep) = map.get(text.as_ref())
    {
        let mut r = (*rep).clone();
        r.field = node.field;
        return r;
    }
    let kind = node.kind;
    node.children = node
        .children
        .into_iter()
        .map(|c| substitute(c, map, shapes, kind))
        .collect();
    node
}

#[cfg(test)]
mod wp_d_precheck_tests {
    //! WP-D (raw-trees elimination) step 3's mandatory, red-first differential
    //! test: `any_resolvable_call_projected` must decide EXACTLY what
    //! `any_resolvable_call` (the tree-walking oracle) decides — same bool,
    //! same `ambiguous_sites` — for every unit, or the memo (which relies on
    //! the precheck needing no raw tree) is unsound (escalation #1).

    use super::*;
    use crate::config::Config;

    /// Builds the SAME `Ctx` `expand_unit` would for `unit_idx`, so the test
    /// exercises the precheck under realistic state (SCC partners, root name,
    /// budget) rather than a stubbed one — the exact scalar state
    /// `resolve_given` reads besides the call's own `(name, arity, span)`.
    fn fresh_ctx<'a>(
        unit_idx: usize,
        units: &'a [Unit],
        table: &'a DefTable,
        cfg: &'a Config,
        li: &LabelInterner,
        raw_trees: &'a [NormNode],
    ) -> Ctx<'a> {
        let lang = units[unit_idx].lang;
        let shapes = Shapes::for_lang(lang, cfg);
        let scc_partners: HashSet<usize> = match table.def_of_unit.get(&unit_idx) {
            Some(&d) if table.scc_sizes[table.scc_of[d]] >= 2 => (0..table.defs.len())
                .filter(|&e| e != d && table.scc_of[e] == table.scc_of[d])
                .collect(),
            _ => HashSet::new(),
        };
        Ctx {
            table,
            raw_trees,
            cfg,
            shapes,
            lang,
            file: &units[unit_idx].file,
            unit_idx,
            root_name: units[unit_idx].name.as_str().into(),
            scc_partners,
            has_expr_block: shapes.make_expr_block((0, 0), Vec::new()).is_some(),
            stack: Vec::new(),
            chain: Vec::new(),
            expanded_units: Vec::new(),
            scc_hit: false,
            calls_inlined: 0,
            ambiguous_sites: HashSet::new(),
            scc_splices: 0,
            spliced_nodes: 0,
            over_budget: false,
            kw_arg_sym: li.intern("keyword_argument"),
        }
    }

    fn assert_precheck_matches_oracle(
        units: &[Unit],
        raw_trees: &[NormNode],
        call_sites: &[UnitCallSites],
        table: &DefTable,
        cfg: &Config,
        li: &LabelInterner,
        label: &str,
    ) {
        for i in 0..units.len() {
            let mut ctx_a = fresh_ctx(i, units, table, cfg, li, raw_trees);
            let oracle = any_resolvable_call(&raw_trees[i], &mut ctx_a);

            let mut ctx_b = fresh_ctx(i, units, table, cfg, li, raw_trees);
            let projected = any_resolvable_call_projected(i, call_sites, &mut ctx_b);

            assert_eq!(
                oracle, projected,
                "{label}: unit {i} ({}) — bool diverged: oracle={oracle} projected={projected}",
                units[i].name
            );
            assert_eq!(
                ctx_a.ambiguous_sites, ctx_b.ambiguous_sites,
                "{label}: unit {i} ({}) — ambiguous_sites diverged",
                units[i].name
            );
            // over_budget is set by resolve_given identically on both paths —
            // part of the same "no divergence anywhere in the decision" claim.
            assert_eq!(
                ctx_a.over_budget, ctx_b.over_budget,
                "{label}: unit {i} ({}) — over_budget diverged",
                units[i].name
            );
        }
    }

    #[test]
    fn inline_pushdown_precheck_matches_tree_walk_wild_corpus() {
        let mut cfg = Config::default();
        cfg.cache.enabled = false; // don't litter fixture dirs with .reprise/
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/wild");
        let corpus = crate::corpus_units(&root, &cfg).expect("wild corpus scans");
        assert!(!corpus.units.is_empty());
        let table = DefTable::build(
            &corpus.units,
            &corpus.call_sites,
            &cfg,
            &corpus.label_interner,
        );
        assert_precheck_matches_oracle(
            &corpus.units,
            &corpus.raw_trees,
            &corpus.call_sites,
            &table,
            &cfg,
            &corpus.label_interner,
            "benches/wild",
        );
    }

    /// A tighter budget/depth than the wild corpus is likely to trip on its
    /// own — exercises `resolve_given`'s budget/depth/SCC-bypass branches,
    /// which the default-config wild-corpus pass may not reach at all.
    #[test]
    fn inline_pushdown_precheck_matches_tree_walk_tight_budget() {
        let mut cfg = Config::default();
        cfg.cache.enabled = false;
        cfg.inline.max_expansion_nodes = 5;
        cfg.inline.max_depth = 1;
        cfg.inline.max_callee_tokens = 20;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/wild");
        let corpus = crate::corpus_units(&root, &cfg).expect("wild corpus scans");
        let table = DefTable::build(
            &corpus.units,
            &corpus.call_sites,
            &cfg,
            &corpus.label_interner,
        );
        assert_precheck_matches_oracle(
            &corpus.units,
            &corpus.raw_trees,
            &corpus.call_sites,
            &table,
            &cfg,
            &corpus.label_interner,
            "benches/wild (tight budget)",
        );
    }

    /// Method calls, keyword-arg calls, and nested calls — the exact shapes
    /// `Shapes::call_parts` filters, which is where `collect_call_sites`'s
    /// reach could plausibly diverge from `any_resolvable_call`'s tree walk
    /// if the filter were ever re-spelled instead of shared.
    #[test]
    fn inline_pushdown_precheck_matches_tree_walk_synthetic_fixture() {
        let mut cfg = Config::default();
        cfg.cache.enabled = false;
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("fixture.py"),
            r#"
def helper(a, b):
    return a + b

def obj_method_call(x):
    return x.method(1, 2)

def kwarg_call(x):
    return helper(a=x, b=1)

def nested(x):
    return helper(helper(x, 1), 2)

def ambiguous_target(a):
    return a

def ambiguous_target(a, b):
    return a + b
"#,
        )
        .unwrap();
        let corpus = crate::corpus_units(dir.path(), &cfg).expect("fixture scans");
        cfg.inline.max_candidates = 0; // force every same-arity multi-def name ambiguous
        let table = DefTable::build(
            &corpus.units,
            &corpus.call_sites,
            &cfg,
            &corpus.label_interner,
        );
        assert_precheck_matches_oracle(
            &corpus.units,
            &corpus.raw_trees,
            &corpus.call_sites,
            &table,
            &cfg,
            &corpus.label_interner,
            "synthetic fixture",
        );
    }
}
