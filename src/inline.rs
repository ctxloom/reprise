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
use crate::ir::kind;
use crate::lang::{Lang, LanguageProfile, child_field};
use crate::tree::{Label, NormNode};
use crate::unit::Unit;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

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

    fn call_kind(&self) -> &'static str {
        match self {
            Shapes::Historical(p) => p.call_kind(),
            Shapes::Ir(_) => kind::CALL,
        }
    }

    fn block_kind(&self) -> &'static str {
        match self {
            Shapes::Historical(_) => "block",
            Shapes::Ir(_) => kind::BLOCK,
        }
    }

    fn params(&self, root: &NormNode) -> Option<Vec<Box<str>>> {
        match self {
            Shapes::Historical(p) => p.inline_params(root),
            // The unit's `Var@param` children carry the simple `Raw` names; a
            // non-simple param (should not occur post-lowering) bails, as historical.
            Shapes::Ir(_) => {
                let mut out = Vec::new();
                for c in &root.children {
                    if c.field.as_deref() != Some("param") {
                        continue;
                    }
                    match &c.label {
                        Some(Label::Raw(t)) if c.kind.as_ref() == kind::VAR => out.push(t.clone()),
                        _ => return None,
                    }
                }
                Some(out)
            }
        }
    }

    /// `(callee name, positional arg subtrees)` for a plain-identifier call with
    /// positionally mappable arguments; None otherwise (methods, keyword args).
    fn call_parts<'a>(&self, node: &'a NormNode) -> Option<(Box<str>, Vec<&'a NormNode>)> {
        if node.kind.as_ref() != self.call_kind() {
            return None;
        }
        match self {
            Shapes::Historical(_) => {
                let f = child_field(node, "function")?;
                if f.kind.as_ref() != "identifier" {
                    return None;
                }
                let Some(Label::Raw(name)) = &f.label else {
                    return None;
                };
                let args = child_field(node, "arguments")?;
                if args
                    .children
                    .iter()
                    .any(|a| a.kind.as_ref() == "keyword_argument")
                {
                    return None;
                }
                Some((name.clone(), args.children.iter().collect()))
            }
            Shapes::Ir(_) => {
                let f = child_field(node, "callee")?;
                if f.kind.as_ref() != kind::VAR {
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
                    .filter(|c| c.field.as_deref() == Some("arg"))
                    .collect();
                // Python keyword args lower to a `NativeStmt` tagged `keyword_argument`;
                // they are not positionally mappable (mirrors the historical guard).
                if args.iter().any(|a| {
                    matches!(&a.label, Some(Label::External(t)) if t.as_ref() == "keyword_argument")
                }) {
                    return None;
                }
                Some((name.clone(), args))
            }
        }
    }

    /// The call node of a `f(...);` statement site (result discarded), else None.
    fn call_at_stmt<'a>(&self, child: &'a NormNode) -> Option<&'a NormNode> {
        match self {
            Shapes::Historical(_) => (child.kind.as_ref() == "expression_statement"
                && child.children.len() == 1)
                .then(|| &child.children[0]),
            // In the IR a bare expression IS the statement — a `Call` at block level.
            Shapes::Ir(_) => (child.kind.as_ref() == kind::CALL).then_some(child),
        }
    }

    fn return_value<'a>(&self, stmt: &'a NormNode) -> Option<&'a NormNode> {
        match self {
            Shapes::Historical(p) => p.return_value(stmt),
            Shapes::Ir(_) => (stmt.kind.as_ref() == kind::RETURN && stmt.children.len() == 1)
                .then(|| &stmt.children[0]),
        }
    }

    fn make_return(&self, value: NormNode) -> NormNode {
        match self {
            Shapes::Historical(p) => p.make_return(value),
            Shapes::Ir(_) => {
                let span = value.span;
                let mut v = value;
                v.field = Some("value".into());
                NormNode::new(kind::RETURN, None, span, vec![v])
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
            Shapes::Ir(Lang::Rust) => Some(NormNode::new(kind::BLOCK, None, span, children)),
            Shapes::Ir(_) => None,
        }
    }

    /// A statement-shaped node (vs a bare trailing expression to be reshaped).
    fn is_statement_like(&self, node: &NormNode) -> bool {
        match self {
            Shapes::Historical(_) => is_statement_like(node),
            Shapes::Ir(_) => matches!(
                node.kind.as_ref(),
                kind::ASSIGN
                    | kind::RETURN
                    | kind::BREAK
                    | kind::CONTINUE
                    | kind::LOOP
                    | kind::BRANCH
                    | kind::ITER
                    | kind::NATIVE_STMT
            ),
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

    fn is_identifier(&self, kind: &str) -> bool {
        match self {
            Shapes::Historical(p) => p.is_identifier(kind),
            Shapes::Ir(_) => kind == crate::ir::kind::VAR,
        }
    }

    fn always_external(&self, kind: &str, field: Option<&str>, parent_kind: &str) -> bool {
        match self {
            Shapes::Historical(p) => p.always_external(kind, field, parent_kind),
            // IR external names already carry `Label::External`, so a `Raw` match is
            // always a genuine local — nothing is structurally always-external here.
            Shapes::Ir(_) => false,
        }
    }
}

pub struct Def<'t> {
    pub unit_idx: usize,
    pub name: Box<str>,
    pub arity: usize,
    pub lang: Lang,
    pub file: PathBuf,
    pub params: Vec<Box<str>>,
    /// Raw (pre-pass) body subtree, borrowed from the caller's raw trees
    /// (M3b: cloning every body cost ~0.4s at 500k LOC for a table that
    /// mostly resolves misses).
    pub body: &'t NormNode,
    /// Raw body token count — the `max_callee_tokens` basis (D18: measured on
    /// raw, because post-fold unit counts under-report the spliced mass).
    pub body_tokens: u32,
}

/// Repo-wide definition table plus the syntactic call graph's SCCs (Tarjan).
pub struct DefTable<'t> {
    pub defs: Vec<Def<'t>>,
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

impl<'t> DefTable<'t> {
    /// Build from raw trees (index-aligned with `units`). Deterministic:
    /// `units` arrive in sorted file order, and candidate lists keep that
    /// order (spec §5.4 determinism requirement).
    pub fn build(units: &[Unit], raw_trees: &'t [NormNode], cfg: &Config) -> DefTable<'t> {
        use rayon::prelude::*;
        // Parallel with order preserved (indexed collect): determinism holds.
        let defs: Vec<Def<'t>> = units
            .par_iter()
            .enumerate()
            .filter_map(|(i, unit)| {
                if unit.name == "<anon>" {
                    return None;
                }
                let shapes = Shapes::for_lang(unit.lang, cfg);
                let params = shapes.params(&raw_trees[i])?;
                let body = child_field(&raw_trees[i], "body")?;
                Some(Def {
                    unit_idx: i,
                    name: unit.name.as_str().into(),
                    arity: params.len(),
                    lang: unit.lang,
                    file: unit.file.clone(),
                    params,
                    body_tokens: body.token_count(),
                    body,
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
        let adj: Vec<Vec<usize>> = table
            .defs
            .par_iter()
            .map(|def| {
                let mut calls = Vec::new();
                collect_calls(def.body, Shapes::for_lang(def.lang, cfg), &mut calls);
                let mut edges: Vec<usize> = calls
                    .iter()
                    .filter_map(|(name, arity)| {
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

fn collect_calls(node: &NormNode, shapes: Shapes, out: &mut Vec<(Box<str>, usize)>) {
    if let Some((name, args)) = shapes.call_parts(node) {
        out.push((name, args.len()));
    }
    for child in &node.children {
        collect_calls(child, shapes, out);
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
    pub tree: NormNode,
    /// Callee names expanded (deduped, expansion order) — the inline chain.
    pub chain: Vec<String>,
    /// Unit indices of the inlined callees (D3 tautology filter keys).
    pub expanded_units: Vec<usize>,
    pub scc: bool,
    pub calls_inlined: u32,
    pub ambiguity_skips: u32,
}

impl Expansion {
    /// No-op expansion (unit skipped: thin delegation, D37).
    fn skipped(raw: &NormNode) -> Expansion {
        Expansion {
            tree: raw.clone(),
            chain: Vec::new(),
            expanded_units: Vec::new(),
            scc: false,
            calls_inlined: 0,
            ambiguity_skips: 0,
        }
    }
}

struct Ctx<'a> {
    table: &'a DefTable<'a>,
    cfg: &'a Config,
    shapes: Shapes,
    lang: Lang,
    file: &'a Path,
    unit_idx: usize,
    root_name: Box<str>,
    /// Def indices in the root unit's SCC (empty unless SCC size ≥2). These
    /// bypass the size cap: the SCC round must complete to reach
    /// self-recursion (spec §5.4).
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
}

/// Fully-inline-per-policy expansion of one unit's raw tree (spec §5.4).
pub fn expand_unit(
    unit_idx: usize,
    raw: &NormNode,
    units: &[Unit],
    table: &DefTable,
    cfg: &Config,
) -> Expansion {
    let lang = units[unit_idx].lang;
    let shapes = Shapes::for_lang(lang, cfg);
    // Pure delegation bodies (a single top-level statement, i.e. a thin
    // wrapper around one call) get no inline variant: expanding one folds the
    // helper back in, so freshly-extracted helpers' wrappers would re-match
    // each other — reprise flagging its own recommended fix (user feedback,
    // D37). Real reimplemented-helper cases have surrounding code.
    if crate::lang::child_field(raw, "body").is_some_and(|b| b.children.len() <= 1) {
        return Expansion::skipped(raw);
    }
    let scc_partners: HashSet<usize> = match table.def_of_unit.get(&unit_idx) {
        Some(&d) if table.scc_sizes[table.scc_of[d]] >= 2 => (0..table.defs.len())
            .filter(|&e| e != d && table.scc_of[e] == table.scc_of[d])
            .collect(),
        _ => HashSet::new(),
    };
    let mut ctx = Ctx {
        table,
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
    };
    let tree = walk(raw.clone(), &mut ctx);
    Expansion {
        tree,
        chain: ctx.chain,
        expanded_units: ctx.expanded_units,
        scc: ctx.scc_hit,
        calls_inlined: ctx.calls_inlined,
        ambiguity_skips: ctx.ambiguous_sites.len() as u32,
    }
}

fn walk(mut node: NormNode, ctx: &mut Ctx) -> NormNode {
    if node.kind.as_ref() == ctx.shapes.block_kind() {
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
    if node.kind.as_ref() == ctx.shapes.call_kind() {
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
        && child.kind.as_ref() == ctx.shapes.call_kind()
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
    let single = def.body.children.len() == 1;
    if !single && !ctx.has_expr_block {
        return node; // D17: no synthetic expression block
    }
    let field = node.field.clone();
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
        let inner = if only.kind.as_ref() == "expression_statement" && only.children.len() == 1 {
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
    field: Option<Box<str>>,
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
    let k = node.kind.as_ref();
    if k == kind::RETURN && node.children.len() == 1 {
        let field = node.field.clone();
        let mut v = node.children.remove(0);
        v.field = field;
        *node = v;
    } else if k == kind::BRANCH {
        for arm in &mut node.children {
            if let Some(body) = arm
                .children
                .iter_mut()
                .find(|c| c.field.as_deref() == Some("body"))
                && body.kind.as_ref() == kind::BLOCK
            {
                de_return_tail(&mut body.children);
            }
        }
    } else if k == kind::BLOCK {
        de_return_tail(&mut node.children);
    }
}

/// Resolve a call node against the table and the §5.4 policy knobs. Counts
/// ambiguity skips; enforces the self-recursion and cycle guards, depth, and
/// the callee size cap (bypassed for SCC partners — the SCC round).
fn resolve_policy(node: &NormNode, ctx: &mut Ctx) -> Option<(usize, Vec<NormNode>)> {
    let (name, args) = ctx.shapes.call_parts(node)?;
    if name == ctx.root_name {
        return None; // never inline direct self-recursion (Rev 5 owns it)
    }
    let def_idx = match ctx
        .table
        .resolve(ctx.lang, &name, args.len(), ctx.file, ctx.cfg)
    {
        Resolution::Hit(d) => d,
        Resolution::Ambiguous => {
            ctx.ambiguous_sites.insert(node.span);
            return None;
        }
        Resolution::Miss => return None,
    };
    let def = &ctx.table.defs[def_idx];
    if def.unit_idx == ctx.unit_idx || ctx.stack.contains(&def_idx) {
        return None;
    }
    if !ctx.scc_partners.contains(&def_idx) {
        if ctx.stack.len() >= ctx.cfg.inline.max_depth as usize {
            return None;
        }
        if def.body_tokens > ctx.cfg.inline.max_callee_tokens {
            return None;
        }
    }
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
    let body = substitute(def.body.clone(), &map, ctx.shapes, "");
    let name = def.name.to_string();
    let unit_idx = def.unit_idx;
    let is_partner = ctx.scc_partners.contains(&def_idx);
    ctx.stack.push(def_idx);
    let body = walk(body, ctx);
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
fn is_statement_like(node: &NormNode) -> bool {
    let kind = node.kind.as_ref();
    kind.ends_with("_statement")
        || kind.ends_with("_declaration")
        || kind.ends_with("_item")
        || kind == "expression_statement"
        || kind == "macro_invocation"
}

fn substitute(
    mut node: NormNode,
    map: &HashMap<&str, &NormNode>,
    shapes: Shapes,
    parent_kind: &str,
) -> NormNode {
    if shapes.is_identifier(&node.kind)
        && !shapes.always_external(&node.kind, node.field.as_deref(), parent_kind)
        && let Some(Label::Raw(text)) = &node.label
        && let Some(rep) = map.get(text.as_ref())
    {
        let mut r = (*rep).clone();
        r.field = node.field.clone();
        return r;
    }
    let kind = node.kind.clone();
    node.children = node
        .children
        .into_iter()
        .map(|c| substitute(c, map, shapes, &kind))
        .collect();
    node
}
