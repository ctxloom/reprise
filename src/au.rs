//! P7 verification: anti-unification (spec §5.6).
//!
//! The anti-unifier of two trees is their shared template; the substitutions
//! ARE the divergences, factored out as complete subtrees. Consistent
//! substitution pairs map to the SAME hole (Plotkin/Reynolds lgg), so
//! `f(a, a)` vs `f(b, b)` costs one hole, not two. List-valued nodes align by
//! graded (similarity-scored) Needleman-Wunsch/Smith-Waterman rather than
//! binary LCS: a clone whose every statement was lightly edited still aligns.

use crate::fingerprint::{HashMode, merkle, merkle_mode};
use crate::lang::LanguageProfile;
use crate::tree::{Label, NormNode};
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Debug, Clone)]
pub struct Hole {
    pub tokens_a: u32,
    pub tokens_b: u32,
    pub factorable: bool,
    /// False for zero-cost holes: consistent Local↔Local renames (alpha-
    /// renaming discovered post hoc) and synthetic loop-machinery gaps
    /// (`__has_next`/`__next` — canonicalization artifacts, not divergence).
    pub counted: bool,
}

#[derive(Debug, Clone)]
pub struct AuOutcome {
    pub template: NormNode,
    pub template_tokens: u32,
    pub holes: Vec<Hole>,
    pub divergence: f64,
    pub factorable: bool,
}

struct Ctx<'a> {
    profile: &'a dyn LanguageProfile,
    holes: HashMap<(u128, u128), (u32, Hole)>,
    next_hole: u32,
    /// Memo of input-subtree hashes keyed by node ADDRESS: exact merkle +
    /// sorted MaskedAll subtree-hash multiset (whose len is the D1 token
    /// count — one hash per node). Sound because both input trees are
    /// borrowed, hence immovable, for the whole `anti_unify` call. Before
    /// this cache, `au_list` recomputed `merkle`/`collect_hashes` from
    /// scratch for every DP CELL of the similarity matrix — the dominant
    /// cost of the near tier at 500k LOC (M4a perf pass; output-identical
    /// by construction).
    memo: HashMap<usize, (u128, Rc<Vec<u128>>)>,
}

impl Ctx<'_> {
    fn node_info(&mut self, node: &NormNode) -> (u128, Rc<Vec<u128>>) {
        let key = std::ptr::from_ref(node) as usize;
        if let Some((exact, vec)) = self.memo.get(&key) {
            return (*exact, Rc::clone(vec));
        }
        let exact = merkle(node);
        let mut hashes = Vec::new();
        collect_hashes(node, &mut hashes);
        hashes.sort_unstable();
        let vec = Rc::new(hashes);
        self.memo.insert(key, (exact, Rc::clone(&vec)));
        (exact, vec)
    }

    /// D1 token count via the memo (one MaskedAll hash per node).
    fn tokens(&mut self, node: &NormNode) -> u32 {
        self.node_info(node).1.len() as u32
    }

    fn is_machinery(&mut self, node: &NormNode) -> bool {
        self.tokens(node) <= 12 && is_synthetic(node)
    }
}

pub fn anti_unify(a: &NormNode, b: &NormNode, profile: &dyn LanguageProfile) -> AuOutcome {
    let mut ctx = Ctx {
        profile,
        holes: HashMap::new(),
        next_hole: 0,
        memo: HashMap::new(),
    };
    let template = au(a, b, &mut ctx);
    let template_tokens = template.token_count();
    let holes: Vec<Hole> = {
        let mut hs: Vec<(u32, Hole)> = ctx.holes.into_values().collect();
        hs.sort_by_key(|(id, _)| *id);
        hs.into_iter().map(|(_, h)| h).collect()
    };
    let sub_tokens: u32 = holes
        .iter()
        .filter(|h| h.counted)
        .map(|h| h.tokens_a + h.tokens_b)
        .sum();
    let divergence = if template_tokens == 0 {
        1.0
    } else {
        f64::from(sub_tokens) / (2.0 * f64::from(template_tokens))
    };
    let factorable = holes.iter().all(|h| h.factorable);
    let holes: Vec<Hole> = holes.into_iter().filter(|h| h.counted).collect();
    AuOutcome {
        template,
        template_tokens,
        holes,
        divergence,
        factorable,
    }
}

fn labels_equal(a: &NormNode, b: &NormNode) -> bool {
    a.label == b.label
}

/// Rust statements wrap in `expression_statement`; comparisons against the
/// loop core must look through the wrapper.
fn unwrap_stmt(node: &NormNode) -> &NormNode {
    if node.kind.as_ref() == "expression_statement" && node.children.len() == 1 {
        &node.children[0]
    } else {
        node
    }
}

fn au(a: &NormNode, b: &NormNode, ctx: &mut Ctx) -> NormNode {
    // REPEAT vs rolled loop core: align the template against the loop body —
    // the guard/bind machinery becomes holes (spec §5.3 canonicalization).
    let (ua, ub) = (unwrap_stmt(a), unwrap_stmt(b));
    if ua.kind.as_ref() == "REPEAT"
        && ctx.profile.is_loop_core(ub)
        && let Some(body) = crate::lang::child_field(ub, "body")
    {
        let children = au_list(&ua.children, &body.children, ctx);
        return NormNode::new("REPEAT", None, a.span, children);
    }
    if ub.kind.as_ref() == "REPEAT"
        && ctx.profile.is_loop_core(ua)
        && let Some(body) = crate::lang::child_field(ua, "body")
    {
        let children = au_list(&body.children, &ub.children, ctx);
        return NormNode::new("REPEAT", None, a.span, children);
    }

    if a.kind != b.kind || !labels_equal(a, b) {
        return hole(a, b, ctx);
    }
    // Binary-operator nodes: a differing operator makes the WHOLE expression
    // one factorable hole (an operator-token hole is a partial construct);
    // a matching operator aligns the flattened operand chains, because order
    // canonicalization may have sorted divergent operands differently.
    let is_operator_slot = |n: &NormNode| n.children.is_empty() && n.label.is_none(); // incl. word ops (and/or)
    if ctx.profile.binary_fields(&a.kind).is_some()
        && a.children.len() == 3
        && b.children.len() == 3
        && is_operator_slot(&a.children[1])
        && is_operator_slot(&b.children[1])
    {
        if a.children[1].kind != b.children[1].kind {
            return hole(a, b, ctx);
        }
        let mut ops_a = Vec::new();
        let mut ops_b = Vec::new();
        flatten_operands(a, &a.kind.clone(), &a.children[1].kind.clone(), &mut ops_a);
        flatten_operands(b, &b.kind.clone(), &b.children[1].kind.clone(), &mut ops_b);
        if ops_a.len() > 2 || ops_b.len() > 2 || ops_a.len() != ops_b.len() {
            let mut children = vec![a.children[1].clone()];
            children.extend(au_list(&ops_a, &ops_b, ctx));
            let mut node = NormNode::new(&a.kind, a.field.as_deref(), a.span, children);
            node.label = a.label.clone();
            return node;
        }
    }
    let children = if ctx.profile.is_list_kind(&a.kind) || a.children.len() != b.children.len() {
        au_list(&a.children, &b.children, ctx)
    } else {
        a.children
            .iter()
            .zip(&b.children)
            .map(|(ca, cb)| au(ca, cb, ctx))
            .collect()
    };
    let mut node = NormNode::new(&a.kind, a.field.as_deref(), a.span, children);
    node.label = a.label.clone();
    node
}

/// Graded alignment of child lists (NW with gap penalty; matched pairs recurse).
fn au_list(xs: &[NormNode], ys: &[NormNode], ctx: &mut Ctx) -> Vec<NormNode> {
    const GAP: f64 = -0.30;
    const MATCH_BIAS: f64 = -0.35; // sim below this prefers gaps
    let n = xs.len();
    let m = ys.len();
    if n * m > 40_000 {
        // Degenerate size: single two-sided hole.
        let ra: Vec<&NormNode> = xs.iter().collect();
        let rb: Vec<&NormNode> = ys.iter().collect();
        return vec![run_hole(&ra, &rb, ctx)];
    }
    let sims: Vec<Vec<f64>> = xs
        .iter()
        .map(|x| ys.iter().map(|y| similarity(x, y, ctx)).collect())
        .collect();
    let mut dp = vec![vec![0.0f64; m + 1]; n + 1];
    for i in 1..=n {
        dp[i][0] = dp[i - 1][0] + GAP;
    }
    for j in 1..=m {
        dp[0][j] = dp[0][j - 1] + GAP;
    }
    for i in 1..=n {
        for j in 1..=m {
            let diag = dp[i - 1][j - 1] + sims[i - 1][j - 1] + MATCH_BIAS;
            dp[i][j] = diag.max(dp[i - 1][j] + GAP).max(dp[i][j - 1] + GAP);
        }
    }
    // Traceback into (Some(i), Some(j)) / one-sided ops.
    let mut ops: Vec<(Option<usize>, Option<usize>)> = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        if i > 0
            && j > 0
            && (dp[i][j] - (dp[i - 1][j - 1] + sims[i - 1][j - 1] + MATCH_BIAS)).abs() < 1e-9
        {
            ops.push((Some(i - 1), Some(j - 1)));
            i -= 1;
            j -= 1;
        } else if i > 0 && (dp[i][j] - (dp[i - 1][j] + GAP)).abs() < 1e-9 {
            ops.push((Some(i - 1), None));
            i -= 1;
        } else {
            ops.push((None, Some(j - 1)));
            j -= 1;
        }
    }
    ops.reverse();

    // Merge consecutive one-sided runs into single list-holes.
    fn flush<'a>(
        out: &mut Vec<NormNode>,
        gap_a: &mut Vec<&'a NormNode>,
        gap_b: &mut Vec<&'a NormNode>,
        ctx: &mut Ctx,
    ) {
        if !gap_a.is_empty() || !gap_b.is_empty() {
            out.push(run_hole(gap_a, gap_b, ctx));
            gap_a.clear();
            gap_b.clear();
        }
    }
    let mut out = Vec::new();
    let mut gap_a: Vec<&NormNode> = Vec::new();
    let mut gap_b: Vec<&NormNode> = Vec::new();
    for (oi, oj) in ops {
        match (oi, oj) {
            (Some(x), Some(y)) => {
                flush(&mut out, &mut gap_a, &mut gap_b, ctx);
                out.push(au(&xs[x], &ys[y], ctx));
            }
            (Some(x), None) => gap_a.push(&xs[x]),
            (None, Some(y)) => gap_b.push(&ys[y]),
            (None, None) => unreachable!(),
        }
    }
    flush(&mut out, &mut gap_a, &mut gap_b, ctx);
    out
}

/// Subtree similarity in [0,1]: exact-equal → 1; else Jaccard over all
/// MaskedAll subtree hashes (the graded S-W substitution cost, spec §5.6).
/// Hashes come from the per-call memo (`Ctx::node_info`) — computed once per
/// node, not once per DP cell.
fn similarity(a: &NormNode, b: &NormNode, ctx: &mut Ctx) -> f64 {
    // Synthetic lowering machinery (the small guard/bind statements) must GAP
    // against real code — gapped it costs nothing; paired it pollutes
    // divergence. Scoped to the machinery statements themselves: a whole loop
    // CONTAINS the markers but must still pair (e.g. with REPEAT).
    if ctx.is_machinery(a) != ctx.is_machinery(b) {
        return 0.0;
    }
    if a.kind != b.kind {
        // Folded REPEAT and the rolled loop core must PAIR in alignment so the
        // au() special case can compare template against loop body (§5.3).
        let (ua, ub) = (unwrap_stmt(a), unwrap_stmt(b));
        let repeat_loop = (ua.kind.as_ref() == "REPEAT" && ctx.profile.is_loop_core(ub))
            || (ub.kind.as_ref() == "REPEAT" && ctx.profile.is_loop_core(ua));
        return if repeat_loop { 0.6 } else { 0.0 };
    }
    let (ea, ha) = ctx.node_info(a);
    let (eb, hb) = ctx.node_info(b);
    if ea == eb {
        return 1.0;
    }
    let (mut i, mut j, mut shared) = (0usize, 0usize, 0usize);
    while i < ha.len() && j < hb.len() {
        match ha[i].cmp(&hb[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                shared += 1;
                i += 1;
                j += 1;
            }
        }
    }
    let union = ha.len() + hb.len() - shared;
    if union == 0 {
        1.0
    } else {
        shared as f64 / union as f64
    }
}

fn collect_hashes(node: &NormNode, out: &mut Vec<u128>) {
    out.push(merkle_mode(node, HashMode::MaskedAll));
    for child in &node.children {
        collect_hashes(child, out);
    }
}

/// Collect operands of a same-kind, same-operator chain (mirrors the
/// normalize-pass flatten; template-only shape, rebuilding is not needed).
fn flatten_operands(node: &NormNode, kind: &str, op: &str, out: &mut Vec<NormNode>) {
    if node.kind.as_ref() == kind
        && node.children.len() == 3
        && node.children[1].kind.as_ref() == op
    {
        flatten_operands(&node.children[0], kind, op, out);
        out.push(node.children[2].clone());
    } else {
        out.push(node.clone());
    }
}

/// See `Ctx::is_machinery`: a small statement that IS lowering machinery
/// (guard `if !__has_next(..)` / bind `x = __next(..)`), as opposed to real
/// code containing a lowered loop.
fn is_synthetic(node: &NormNode) -> bool {
    if matches!(&node.label, Some(Label::External(s)) if s.as_ref() == "__has_next" || s.as_ref() == "__next")
    {
        return true;
    }
    node.children.iter().any(is_synthetic)
}

fn is_op_token(node: &NormNode) -> bool {
    node.children.is_empty()
        && node.label.is_none()
        && !node.kind.chars().any(char::is_alphanumeric)
}

fn hole(a: &NormNode, b: &NormNode, ctx: &mut Ctx) -> NormNode {
    let key = (ctx.node_info(a).0, ctx.node_info(b).0);
    let id = match ctx.holes.get(&key) {
        Some((id, _)) => *id,
        None => {
            let id = ctx.next_hole;
            ctx.next_hole += 1;
            // Holes at bare operator tokens capture partial constructs (§5.6).
            let factorable = !(is_op_token(a) || is_op_token(b));
            // Consistent Local↔Local leaf pairs are alpha-renaming, not drift.
            let rename_only = a.children.is_empty()
                && b.children.is_empty()
                && matches!(&a.label, Some(Label::Local(_)))
                && matches!(&b.label, Some(Label::Local(_)));
            let (tokens_a, tokens_b) = (ctx.tokens(a), ctx.tokens(b));
            ctx.holes.insert(
                key,
                (
                    id,
                    Hole {
                        tokens_a,
                        tokens_b,
                        factorable,
                        counted: !rename_only,
                    },
                ),
            );
            id
        }
    };
    NormNode::new("HOLE", a.field.as_deref(), a.span, Vec::new()).with_label(Label::Local(id))
}

/// One-sided (or two-sided) gap run → a single hole.
fn run_hole(gap_a: &[&NormNode], gap_b: &[&NormNode], ctx: &mut Ctx) -> NormNode {
    let mut hash_side = |items: &[&NormNode]| -> u128 {
        let mut buf = Vec::new();
        for it in items {
            buf.extend_from_slice(&ctx.node_info(it).0.to_le_bytes());
        }
        xxhash_rust::xxh3::xxh3_128(&buf)
    };
    let key = (hash_side(gap_a), hash_side(gap_b));
    let span = gap_a
        .first()
        .or_else(|| gap_b.first())
        .map(|n| n.span)
        .unwrap_or((0, 0));
    let id = match ctx.holes.get(&key) {
        Some((id, _)) => *id,
        None => {
            let id = ctx.next_hole;
            ctx.next_hole += 1;
            // One-sided gaps made only of synthetic lowering machinery
            // (guard/bind around __has_next/__next) are canonicalization
            // artifacts of the REPEAT↔loop comparison, not real divergence.
            let synthetic = (gap_a.is_empty() && gap_b.iter().all(|n| ctx.is_machinery(n)))
                || (gap_b.is_empty() && gap_a.iter().all(|n| ctx.is_machinery(n)));
            let tokens_a = gap_a.iter().map(|n| ctx.tokens(n)).sum::<u32>();
            let tokens_b = gap_b.iter().map(|n| ctx.tokens(n)).sum::<u32>();
            ctx.holes.insert(
                key,
                (
                    id,
                    Hole {
                        tokens_a,
                        tokens_b,
                        // Gap runs are whole child-list elements: statements or
                        // list items — mechanically consolidable.
                        factorable: true,
                        counted: !synthetic,
                    },
                ),
            );
            id
        }
    };
    NormNode::new("HOLE", None, span, Vec::new()).with_label(Label::Local(id))
}

// ---------- template rendering (pseudo-source, spec §6) ----------

/// Compact pseudo-source rendering of a template. Not a pretty-printer:
/// structure + labels + ⟨hole⟩ markers, readable enough to judge a finding.
pub fn render_template(node: &NormNode) -> String {
    let mut out = String::new();
    render(node, 0, &mut out);
    out
}

fn render(node: &NormNode, indent: usize, out: &mut String) {
    match node.kind.as_ref() {
        "HOLE" => {
            let id = match &node.label {
                Some(Label::Local(i)) => *i + 1,
                _ => 0,
            };
            out.push_str(&format!("⟨h{id}⟩"));
            return;
        }
        "REPEAT" => {
            out.push_str("repeat× {");
            for child in &node.children {
                newline(indent + 1, out);
                render(child, indent + 1, out);
            }
            newline(indent, out);
            out.push('}');
            return;
        }
        "block" => {
            out.push('{');
            for child in &node.children {
                newline(indent + 1, out);
                render(child, indent + 1, out);
            }
            newline(indent, out);
            out.push('}');
            return;
        }
        _ => {}
    }
    if let Some(label) = &node.label {
        let text = match label {
            Label::External(s) => s.to_string(),
            Label::Local(i) => format!("v{i}"),
            Label::LitKept(s) => s.to_string(),
            Label::LitBucket(b) => b.name().to_string(),
            Label::Raw(s) | Label::RawLit(s) => s.to_string(),
        };
        out.push_str(&text);
        return;
    }
    if node.children.is_empty() {
        out.push_str(&node.kind);
        return;
    }
    if let Some(prefix) = kind_keyword(&node.kind) {
        out.push_str(prefix);
        out.push(' ');
    }
    let parenthesized = matches!(
        node.kind.as_ref(),
        "arguments" | "argument_list" | "parameters"
    );
    if parenthesized {
        out.push('(');
    }
    let mut first = true;
    for child in &node.children {
        if !first {
            out.push_str(if parenthesized { ", " } else { " " });
        }
        first = false;
        render(child, indent, out);
    }
    if parenthesized {
        out.push(')');
    }
}

fn newline(indent: usize, out: &mut String) {
    out.push('\n');
    for _ in 0..indent {
        out.push_str("  ");
    }
}

fn kind_keyword(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "if_statement" | "if_expression" => "if",
        "while_statement" => "while",
        "loop_expression" => "loop",
        "return_statement" | "return_expression" => "return",
        "break_statement" | "break_expression" => "break",
        "continue_statement" | "continue_expression" => "continue",
        "else_clause" => "else",
        "elif_clause" => "elif",
        "match_expression" => "match",
        "try_statement" => "try",
        "not_operator" => "not",
        "let_declaration" => "let",
        "function_item" | "function_definition" => "fn",
        _ => return None,
    })
}
