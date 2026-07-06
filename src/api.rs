//! P7b: api-profile tier (spec §5.7) — suspicion-only static birthmark.
//!
//! Signature per unit: the multiset of `(callee external name, control
//! context)` pairs, context = (loop_depth, in_branch, tail_position),
//! extracted from normalized trees. IDF weighting over the repo's callee
//! distribution makes ubiquitous calls contribute ~nothing; units with fewer
//! than `api_min_distinct_rare` rare callees emit no signature (pure
//! computation is invisible to this tier by design). Findings are pairs —
//! there is no AU template to merge groups around, and pairwise findings are
//! what §7.4(d) hand-labels — reported in their own section, never failing CI.

use crate::config::Config;
use crate::ir::kind;
use crate::lang::{Lang, LanguageProfile, child_field};
use crate::report::{Group, Tier};
use crate::tree::{Label, NormNode};
use crate::unit::{self, Unit};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Signature element: callee name + control context.
type Elem = (Box<str>, u8, bool, bool);

struct Signature {
    unit_idx: usize,
    elems: BTreeMap<Elem, u32>,
    rare_names: Vec<Box<str>>,
}

/// Owners cap when enumerating candidate pairs from one shared rare name.
const PAIR_OWNER_CAP: usize = 40;
/// Loop depth is capped for context stability.
const MAX_LOOP_DEPTH: u8 = 3;

pub fn find_api_groups(
    units: &[Unit],
    cfg: &Config,
    excluded_pairs: &HashSet<(usize, usize)>,
    signatures_emitted: &mut usize,
) -> Vec<Group> {
    if !cfg.api_profile.enabled {
        return Vec::new();
    }
    let mut groups = Vec::new();
    let mut langs: Vec<Lang> = units.iter().map(|u| u.lang).collect();
    langs.sort();
    langs.dedup();
    for lang in langs {
        groups.extend(api_groups_for_lang(
            units,
            lang,
            cfg,
            excluded_pairs,
            signatures_emitted,
        ));
    }
    groups
}

fn api_groups_for_lang(
    units: &[Unit],
    lang: Lang,
    cfg: &Config,
    excluded_pairs: &HashSet<(usize, usize)>,
    signatures_emitted: &mut usize,
) -> Vec<Group> {
    let shapes = Shapes::for_lang(lang, cfg);
    // Eligible population: plain units at or above the size floor.
    let eligible: Vec<usize> = (0..units.len())
        .filter(|&i| {
            units[i].lang == lang
                && units[i].variant.is_none()
                && units[i].token_count >= cfg.min_unit_floor()
        })
        .collect();
    let n = eligible.len();
    if n < 2 {
        return Vec::new();
    }

    // Callee multisets (parallel) + document frequency over the population.
    use rayon::prelude::*;
    let raw: Vec<(usize, BTreeMap<Elem, u32>)> = eligible
        .par_iter()
        .map(|&i| {
            let mut elems = BTreeMap::new();
            let tree = &units[i].tree;
            // The unit body's final statement/expression is a tail position,
            // like returns: `return f(x)` and block-tail `f(x)` must agree.
            let tail_node = shapes.tail_node(tree);
            extract_calls(tree, shapes, Flags::default(), tail_node, &mut elems);
            (i, elems)
        })
        .collect();
    let mut df: HashMap<Box<str>, usize> = HashMap::new();
    for (_, elems) in &raw {
        let names: HashSet<&Box<str>> = elems.keys().map(|(name, ..)| name).collect();
        for name in names {
            *df.entry(name.clone()).or_insert(0) += 1;
        }
    }

    // Rare = document frequency at or below ~5% of units (floor 3): the same
    // rarity shape the landmark layer uses (§5.5.4 shares the IDF machinery).
    let rare_cap = 3.max(n / 20);
    let idf = |name: &str| -> f64 {
        let d = df.get(name).copied().unwrap_or(1).max(1);
        (1.0 + n as f64 / d as f64).ln()
    };

    let sigs: Vec<Signature> = raw
        .into_iter()
        .filter_map(|(unit_idx, elems)| {
            let mut rare_names: Vec<Box<str>> = elems
                .keys()
                .map(|(name, ..)| name.clone())
                .filter(|name| df[name] <= rare_cap)
                .collect();
            rare_names.sort();
            rare_names.dedup();
            if rare_names.len() < cfg.api_profile.api_min_distinct_rare {
                return None; // no signature (spec §5.7)
            }
            Some(Signature {
                unit_idx,
                elems,
                rare_names,
            })
        })
        .collect();
    *signatures_emitted += sigs.len();

    // Candidate pairs: signatures sharing at least one rare callee name.
    let mut by_rare: HashMap<&str, Vec<usize>> = HashMap::new();
    for (s, sig) in sigs.iter().enumerate() {
        for name in &sig.rare_names {
            by_rare.entry(name).or_default().push(s);
        }
    }
    let mut cand: Vec<(usize, usize)> = Vec::new();
    for owners in by_rare.values() {
        if owners.len() < 2 || owners.len() > PAIR_OWNER_CAP {
            continue;
        }
        for x in 0..owners.len() {
            for y in x + 1..owners.len() {
                cand.push((owners[x].min(owners[y]), owners[x].max(owners[y])));
            }
        }
    }
    cand.sort_unstable();
    cand.dedup();

    let mut groups = Vec::new();
    for (x, y) in cand {
        let (a, b) = (&sigs[x], &sigs[y]);
        let key = (a.unit_idx.min(b.unit_idx), a.unit_idx.max(b.unit_idx));
        if excluded_pairs.contains(&key) {
            continue; // already co-grouped by a stronger tier (spec §5.7)
        }
        let (ua, ub) = (&units[a.unit_idx], &units[b.unit_idx]);
        if ua.is_test && ub.is_test {
            continue; // arrange-act-assert noise; api section stays production
        }
        // Nested units (an inner fn and its parent) share callees by
        // construction — an artifact, not a finding (same rule as D10's
        // sequence-tier exclusion).
        if ua.file == ub.file && ua.byte_span.0 < ub.byte_span.1 && ub.byte_span.0 < ua.byte_span.1
        {
            continue;
        }
        let mut min_mass = 0.0f64;
        let mut max_mass = 0.0f64;
        let mut shared: Vec<(&Elem, u32)> = Vec::new();
        let keys: HashSet<&Elem> = a.elems.keys().chain(b.elems.keys()).collect();
        for elem in keys {
            let ca = a.elems.get(elem).copied().unwrap_or(0);
            let cb = b.elems.get(elem).copied().unwrap_or(0);
            let w = idf(&elem.0);
            min_mass += f64::from(ca.min(cb)) * w;
            max_mass += f64::from(ca.max(cb)) * w;
            if ca.min(cb) > 0 {
                shared.push((elem, ca.min(cb)));
            }
        }
        if max_mass <= 0.0 {
            continue;
        }
        let sim = min_mass / max_mass;
        if sim < cfg.api_profile.api_profile_sim {
            continue;
        }
        // Evidence: shared rare-callee list with contexts, IDF-heaviest first.
        shared.sort_by(|(ea, _), (eb, _)| {
            idf(&eb.0)
                .partial_cmp(&idf(&ea.0))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| ea.cmp(eb))
        });
        let mut evidence = String::from("shared calls:\n");
        for (elem, count) in &shared {
            evidence.push_str(&render_elem(elem, *count));
            evidence.push('\n');
        }
        let shared_rare = a
            .rare_names
            .iter()
            .filter(|name| b.rare_names.contains(name))
            .count() as u32;
        let mut fp_buf = Vec::with_capacity(32);
        let mut member_fps = [ua.fingerprint, ub.fingerprint];
        member_fps.sort_unstable();
        for fp in member_fps {
            fp_buf.extend_from_slice(&fp.to_le_bytes());
        }
        groups.push(Group {
            id: String::new(),
            tier: Tier::ApiProfile,
            // No structural template exists (spec §5.7); key on the member
            // pair's unit fingerprints. Never fails CI, so key churn under
            // edits is acceptable.
            fingerprint: crate::fingerprint::hex(xxhash_rust::xxh3::xxh3_128(&fp_buf)),
            token_count: shared_rare,
            value: min_mass, // IDF-weighted overlap mass (spec §5.7 ranking)
            note: None,
            divergence: 1.0 - sim,
            template: Some(evidence),
            inline_chain: None,
            members: vec![crate::group::member_of(ua), crate::group::member_of(ub)],
        });
    }
    groups
}

fn render_elem(elem: &Elem, count: u32) -> String {
    let (name, depth, branch, tail) = elem;
    let mut ctx_parts: Vec<String> = Vec::new();
    if *depth > 0 {
        ctx_parts.push(format!("loop×{depth}"));
    }
    if *branch {
        ctx_parts.push("branch".into());
    }
    if *tail {
        ctx_parts.push("tail".into());
    }
    if ctx_parts.is_empty() {
        ctx_parts.push("top".into());
    }
    let times = if count > 1 {
        format!(" ×{count}")
    } else {
        String::new()
    };
    format!("  {name}{times} ({})", ctx_parts.join(", "))
}

#[derive(Default, Clone, Copy)]
struct Flags {
    loop_depth: u8,
    in_branch: bool,
    tail: bool,
}

/// Branch-context kinds across the supported languages; post-normalization
/// both languages express branching through these.
fn is_branchy(kind: &str) -> bool {
    matches!(
        kind,
        "if_expression"
            | "if_statement"
            | "elif_clause"
            | "else_clause"
            | "match_expression"
            | "match_arm"
            | "match_statement"
            | "case_clause"
            | "try_statement"
            | "try_expression"
            | "except_clause"
            | "conditional_expression"
    )
}

fn is_return_kind(kind: &str) -> bool {
    matches!(kind, "return_expression" | "return_statement")
}

/// The tree-shape primitives `extract_calls` reads through, so one extraction serves
/// both the historical per-grammar trees and the canonical IR (`normalizer = "ir"`),
/// mirroring [`crate::inline::Shapes`]. The `Historical` arm reproduces the per-language
/// [`LanguageProfile`] hooks byte-for-byte; the `Ir` arm returns the canonical constants
/// (`docs/SIMILARITY-IR.md`) — the call/loop/branch/return kinds, and the `callee` field.
/// The discriminating signal (rare-callee names) is identical either way: a free callee
/// lowers to `Call{ callee: Var@callee External(name) }`, and the IR keeps external names
/// verbatim (only bound locals are abstracted), so [`callee_name`]'s `External` walk reads
/// the same function names off both forms.
#[derive(Clone, Copy)]
enum Shapes {
    Historical(&'static dyn LanguageProfile),
    Ir,
}

impl Shapes {
    /// Pick the shape family a unit was extracted with — IR when the active normalizer is
    /// `"ir"` and `lang` has a frontend, else the historical profile (also the TS/Kotlin
    /// fallback under `normalizer = "ir"`, §9 P2) — mirroring the inline tier's selector so
    /// mixed scans resolve per-unit.
    fn for_lang(lang: Lang, cfg: &Config) -> Shapes {
        if unit::is_ir(lang, cfg) {
            Shapes::Ir
        } else {
            Shapes::Historical(lang.profile())
        }
    }

    fn call_kind(&self) -> &'static str {
        match self {
            Shapes::Historical(p) => p.call_kind(),
            Shapes::Ir => kind::CALL,
        }
    }

    /// The callee subtree of a call: historical `function` field, IR `callee` field.
    fn callee<'a>(&self, node: &'a NormNode) -> Option<&'a NormNode> {
        match self {
            Shapes::Historical(_) => child_field(node, "function"),
            Shapes::Ir => child_field(node, "callee"),
        }
    }

    fn is_loop(&self, node: &NormNode) -> bool {
        match self {
            Shapes::Historical(p) => p.is_loop_core(node),
            Shapes::Ir => node.kind.as_ref() == kind::LOOP,
        }
    }

    fn is_branch(&self, node: &NormNode) -> bool {
        match self {
            Shapes::Historical(_) => is_branchy(&node.kind),
            Shapes::Ir => node.kind.as_ref() == kind::BRANCH,
        }
    }

    fn is_return(&self, node: &NormNode) -> bool {
        match self {
            Shapes::Historical(_) => is_return_kind(&node.kind),
            Shapes::Ir => node.kind.as_ref() == kind::RETURN,
        }
    }

    /// The block-tail node whose call shares tail context with `return`s. On the historical
    /// trees this is the body's last child (a bare block-tail `f(x)`). On the IR the frontend
    /// has already pushed every tail expression into an explicit `Return` (return-position
    /// lowering), so `is_return` alone carries tail context; the pointer heuristic is then not
    /// only redundant but wrong — the last body node is often a trailing `Loop` (a `for`'s tail
    /// return fuses into its exit arm), which would leak `tail` onto the whole loop body.
    fn tail_node<'a>(&self, tree: &'a NormNode) -> Option<&'a NormNode> {
        match self {
            Shapes::Historical(_) => child_field(tree, "body").and_then(|b| b.children.last()),
            Shapes::Ir => None,
        }
    }
}

fn extract_calls(
    node: &NormNode,
    shapes: Shapes,
    flags: Flags,
    tail_node: Option<&NormNode>,
    out: &mut BTreeMap<Elem, u32>,
) {
    let mut here = flags;
    if shapes.is_loop(node) {
        here.loop_depth = (here.loop_depth + 1).min(MAX_LOOP_DEPTH);
    }
    if shapes.is_branch(node) {
        here.in_branch = true;
    }
    if shapes.is_return(node) || tail_node.is_some_and(|t| std::ptr::eq(t, node)) {
        here.tail = true;
    }
    if node.kind.as_ref() == shapes.call_kind()
        && let Some(f) = shapes.callee(node)
        && let Some(name) = callee_name(f)
    {
        *out.entry((name, here.loop_depth, here.in_branch, here.tail))
            .or_insert(0) += 1;
    }
    for child in &node.children {
        extract_calls(child, shapes, here, tail_node, out);
    }
}

/// Rightmost preserved External name inside a call's function expression:
/// `f` → f, `x.method` → method, `mod::helper` → helper. Locals (closures)
/// yield nothing; the loop-lowering synthetics are excluded.
fn callee_name(function: &NormNode) -> Option<Box<str>> {
    let mut found: Option<Box<str>> = None;
    fn walk(node: &NormNode, found: &mut Option<Box<str>>) {
        if let Some(Label::External(name)) = &node.label {
            *found = Some(name.clone());
        }
        for child in &node.children {
            walk(child, found);
        }
    }
    walk(function, &mut found);
    match found {
        Some(name) if name.as_ref() == "__has_next" || name.as_ref() == "__next" => None,
        other => other,
    }
}
