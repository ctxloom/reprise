//! Comparison units (spec §5.1): every function-like node, normalized,
//! folded (P4), and fingerprinted. Phase 3 adds inline-expanded variants
//! (spec §5.4): a unit fingerprints at most twice — plain plus one
//! fully-inlined variant (spec §12 cap) — and variants are tagged so the
//! matching tiers can label findings `inline-assisted`.

use crate::config::{Config, Normalizer};
use crate::fingerprint;
use crate::fold::{self, RepeatFinding};
use crate::lang::Lang;
use crate::normalize::{self, RawUnit};
use crate::tree::{Label, NormNode};
use std::path::{Path, PathBuf};

/// Tag on an inline-expanded variant unit (spec §5.4).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VariantTag {
    /// Index of the plain unit this variant derives from.
    pub base: usize,
    /// Callee names expanded, in expansion order (the inline chain).
    pub chain: Vec<String>,
    /// Exact fingerprints of the plain units whose bodies were inlined —
    /// the D3 tautology filter's minimum viable rule keys on these.
    pub expanded_fps: Vec<u128>,
    /// True when a mutual-recursion SCC partner was expanded (spec §5.4).
    pub scc: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Unit {
    pub file: PathBuf,
    pub lang: Lang,
    pub name: String,
    pub byte_span: (u32, u32),
    pub line_span: (u32, u32),
    pub token_count: u32,
    pub parse_degraded: bool,
    pub is_test: bool,
    /// `reprise:accept-drift` pragma (D41): one-sided edits to this unit
    /// report inconsistent-update as info instead of failing.
    pub accept_drift: bool,
    pub fingerprint: u128,
    pub tree: NormNode,
    /// Some for inline-expanded variants; None for plain units.
    pub variant: Option<VariantTag>,
}

/// Internal duplication inside one unit (spec §5.3 direct finding).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InternalRepeat {
    pub file: PathBuf,
    pub lang: Lang,
    pub unit_name: String,
    pub line_span: (u32, u32),
    pub count: u32,
    pub template_tokens: u32,
    /// Baseline identity (spec §2): (unit fingerprint, run template hash).
    pub unit_fp: u128,
    pub template_hash: u128,
}

/// One file's extraction, keeping the raw (pre-pass) trees alive for the
/// inliner's definition table (spec §5.4: the table is built from raw trees
/// so recursion lowering can fire on SCC-expanded variants).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct FileUnits {
    pub units: Vec<Unit>,
    pub repeats: Vec<InternalRepeat>,
    /// Raw trees, index-aligned with `units`.
    pub raw_trees: Vec<NormNode>,
    /// Units suppressed by the `reprise:ignore` pragma (spec §2, §12: the
    /// count must be visible so suppression can't silently accumulate).
    pub suppressed: usize,
}

/// Unit pragmas (spec §2 + D41): a comment containing a marker on the unit's
/// first line or the line above it. Substring match — see DECISIONS.md D21.
/// Single-line attributes / decorators / annotations between the pragma and
/// the `fn`/`def` line are skipped when scanning upward (multi-line attribute
/// arguments remain unhandled).
///
/// - `reprise:ignore` — suppress the unit from ALL tiers.
/// - `reprise:accept-drift` — the unit stays fully covered, but a one-sided
///   edit to it no longer FAILS inconsistent-update (reports as info): the
///   reviewed, source-located, D40-compatible way to accept divergence.
fn pragma_line(lines: &[&str], first_line: u32) -> Option<u32> {
    let has_pragma = |ln: u32| {
        ln >= 1
            && lines
                .get(ln as usize - 1)
                .is_some_and(|l| l.contains("reprise:ignore") || l.contains("reprise:accept-drift"))
    };
    if has_pragma(first_line) {
        return Some(first_line);
    }
    let mut ln = first_line.saturating_sub(1);
    while ln >= 1 {
        let text = lines
            .get(ln as usize - 1)
            .map(|l| l.trim_start())
            .unwrap_or("");
        // Rust `#[...]`; Python/TypeScript decorators and Kotlin annotations `@...`.
        if text.starts_with("#[") || text.starts_with('@') {
            ln -= 1;
            continue;
        }
        return has_pragma(ln).then_some(ln);
    }
    None
}

fn is_suppressed(lines: &[&str], first_line: u32) -> bool {
    pragma_line(lines, first_line)
        .is_some_and(|ln| lines[ln as usize - 1].contains("reprise:ignore"))
}

fn accepts_drift(lines: &[&str], first_line: u32) -> bool {
    pragma_line(lines, first_line)
        .is_some_and(|ln| lines[ln as usize - 1].contains("reprise:accept-drift"))
}

pub fn extract_file_units_keep_raw(path: &Path, src: &str, lang: Lang, cfg: &Config) -> FileUnits {
    // Normalizer selector (non-boolean, plugin-extensible — D-IR-3): the "ir" plugin
    // lowers directly to the canonical IR (D-IR-1a) and skips the historical passes.
    // A language without an IR frontend yet (TS, Kotlin) falls back to the historical
    // normalizer so those files still scan (per-language capability gate, §9 P2).
    if cfg.normalize.normalizer == Normalizer::Ir && crate::frontend::has_ir_frontend(lang) {
        return extract_ir_file_units(path, src, lang, cfg);
    }
    let mut out = FileUnits {
        units: Vec::new(),
        repeats: Vec::new(),
        raw_trees: Vec::new(),
        suppressed: 0,
    };
    let lines: Vec<&str> = src.lines().collect();
    for mut raw in normalize::extract_raw_units(src, lang, path) {
        if is_suppressed(&lines, raw.line_span.0) {
            out.suppressed += 1;
            continue;
        }
        let accept_drift = accepts_drift(&lines, raw.line_span.0);
        let raw_tree =
            std::mem::replace(&mut raw.tree, NormNode::new("", None, (0, 0), Vec::new()));
        let (tree, found) = pass_and_fold(raw_tree.clone(), lang, cfg);
        let unit = unit_from_tree(path, lang, &raw, tree, accept_drift);
        for f in found {
            // Report a run only when the duplicated mass clears the sequence floor.
            if f.template_tokens * f.count >= cfg.thresholds.min_seq_tokens {
                out.repeats.push(InternalRepeat {
                    file: path.to_path_buf(),
                    lang,
                    unit_name: raw.name.clone(),
                    line_span: (
                        byte_to_line(src, f.byte_span.0),
                        byte_to_line(src, f.byte_span.1),
                    ),
                    count: f.count,
                    template_tokens: f.template_tokens,
                    unit_fp: unit.fingerprint,
                    template_hash: f.template_hash,
                });
            }
        }
        out.units.push(unit);
        out.raw_trees.push(raw_tree);
    }
    out
}

/// The IR-normalizer extraction path (D-IR-1a): each function is lowered directly to
/// canonical IR, sibling-run folded (spec §5.3 / D30, on the canonical kinds via
/// [`crate::ir::FoldRules`]), fingerprinted, and flowed through the same matching back
/// half as the historical path. (Inline variants remain deferred — back-half, §3.)
fn extract_ir_file_units(path: &Path, src: &str, lang: Lang, cfg: &Config) -> FileUnits {
    let mut out = FileUnits {
        units: Vec::new(),
        repeats: Vec::new(),
        raw_trees: Vec::new(),
        suppressed: 0,
    };
    let lines: Vec<&str> = src.lines().collect();
    for u in crate::frontend::extract_ir_units(src, lang, path) {
        if is_suppressed(&lines, u.line_span.0) {
            out.suppressed += 1;
            continue;
        }
        let accept_drift = accepts_drift(&lines, u.line_span.0);
        // Fold repeat runs (mutates the tree → must precede the fingerprint, like the
        // historical path) and collect the internal-repeat findings.
        let mut found = Vec::new();
        let tree = fold::fold_repeats_with(
            u.tree,
            &crate::ir::FoldRules,
            cfg.thresholds.fold_min_repeats as usize,
            &mut found,
        );
        let fingerprint = fingerprint::merkle(&tree);
        let token_count = tree.token_count();
        for f in found {
            // Report a run only when the duplicated mass clears the sequence floor.
            if f.template_tokens * f.count >= cfg.thresholds.min_seq_tokens {
                out.repeats.push(InternalRepeat {
                    file: path.to_path_buf(),
                    lang,
                    unit_name: u.name.clone(),
                    line_span: (
                        byte_to_line(src, f.byte_span.0),
                        byte_to_line(src, f.byte_span.1),
                    ),
                    count: f.count,
                    template_tokens: f.template_tokens,
                    unit_fp: fingerprint,
                    template_hash: f.template_hash,
                });
            }
        }
        // Retain the pre-abstraction lowered tree (not the canonical `tree`): the
        // inline phase (spec §5.4) splices callee bodies by name and re-runs the
        // passes on the result, exactly as the historical path keeps pre-`apply_passes`
        // raw trees. (Historical raw trees hold the same pre-normalization form.)
        out.raw_trees.push(u.raw);
        out.units.push(Unit {
            file: path.to_path_buf(),
            lang,
            name: u.name,
            byte_span: u.byte_span,
            line_span: u.line_span,
            token_count,
            parse_degraded: u.parse_degraded,
            is_test: u.is_test,
            accept_drift,
            fingerprint,
            tree,
            variant: None,
        });
    }
    out
}

pub fn extract_file_units(
    path: &Path,
    src: &str,
    lang: Lang,
    cfg: &Config,
) -> (Vec<Unit>, Vec<InternalRepeat>) {
    let f = extract_file_units_keep_raw(path, src, lang, cfg);
    (f.units, f.repeats)
}

fn pass_and_fold(tree: NormNode, lang: Lang, cfg: &Config) -> (NormNode, Vec<RepeatFinding>) {
    let tree = normalize::apply_passes(tree, lang, cfg);
    let mut found: Vec<RepeatFinding> = Vec::new();
    let tree = fold::fold_repeats(
        tree,
        lang.profile(),
        cfg.thresholds.fold_min_repeats as usize,
        &mut found,
    );
    (tree, found)
}

/// Whether extraction for `lang` used the IR normalizer (else the historical path,
/// including the TS/Kotlin fallback under `normalizer = "ir"`, §9 P2).
pub(crate) fn is_ir(lang: Lang, cfg: &Config) -> bool {
    cfg.normalize.normalizer == Normalizer::Ir && crate::frontend::has_ir_frontend(lang)
}

/// The IR analog of [`pass_and_fold`] for a spliced inline variant: re-run recursion
/// lowering (an inlined mutual-recursion partner is now direct self-recursion — spec
/// §5.4), then the canonical IR passes and the IR sibling-run fold.
fn ir_pass_and_fold(
    expanded: NormNode,
    root_name: &str,
    lang: Lang,
    cfg: &Config,
) -> (NormNode, Vec<RepeatFinding>) {
    let expanded = ir_relower_recursion(expanded, root_name);
    let tree = crate::frontend::run_passes(
        expanded,
        lang,
        &mut crate::ir::transform::TransformLog::disabled(),
    );
    let mut found: Vec<RepeatFinding> = Vec::new();
    let tree = fold::fold_repeats_with(
        tree,
        &crate::ir::FoldRules,
        cfg.thresholds.fold_min_repeats as usize,
        &mut found,
    );
    (tree, found)
}

/// Re-run tail-recursion lowering on a spliced IR unit (mirrors the historical
/// `apply_passes` recursion pass). The frontend runs it inside `lower_function`, so a
/// variant that became self-recursive by inlining its SCC partner needs it re-applied
/// on the lowered body before the passes. Needs the unit's own name + simple param
/// names — both still present as `Raw` labels on the lowered tree.
fn ir_relower_recursion(mut unit: NormNode, name: &str) -> NormNode {
    let mut params: Vec<Box<str>> = Vec::new();
    for c in &unit.children {
        if c.field.as_deref() == Some("param") {
            match &c.label {
                Some(Label::Raw(t)) => params.push(t.clone()),
                _ => return unit, // a non-simple param: the frontend would not lower either
            }
        }
    }
    let Some(idx) = unit
        .children
        .iter()
        .position(|c| c.field.as_deref() == Some("body"))
    else {
        return unit;
    };
    let body = unit.children.remove(idx);
    let body = crate::ir::pass::lower_tail_recursion(
        name,
        &params,
        body,
        &mut crate::ir::transform::TransformLog::disabled(),
    );
    unit.children.insert(idx, body);
    unit
}

fn unit_from_tree(
    path: &Path,
    lang: Lang,
    raw: &RawUnit,
    tree: NormNode,
    accept_drift: bool,
) -> Unit {
    Unit {
        file: path.to_path_buf(),
        lang,
        name: raw.name.clone(),
        byte_span: raw.byte_span,
        line_span: raw.line_span,
        token_count: tree.token_count(),
        parse_degraded: raw.parse_degraded,
        is_test: raw.is_test,
        accept_drift,
        fingerprint: fingerprint::merkle(&tree),
        tree,
        variant: None,
    }
}

/// Run the inline-expanded raw tree through the same passes as the plain unit
/// and fingerprint it as a tagged variant (spec §5.4: "each unit fingerprints
/// twice"). Returns None when the variant converged back onto its own plain
/// form, or onto a callee it inlined (D3 tautology: a pure wrapper's variant
/// IS the callee).
pub fn finish_variant(
    base_idx: usize,
    base: &Unit,
    expanded: NormNode,
    cfg: &Config,
) -> Option<Unit> {
    // IR-path variants re-run the canonical IR passes (`frontend::run_passes`) rather
    // than the historical `apply_passes`, so a spliced variant converges with the
    // frontend's own canonical form for the equivalent hand-inlined function.
    let (tree, _found) = if is_ir(base.lang, cfg) {
        ir_pass_and_fold(expanded, &base.name, base.lang, cfg)
    } else {
        pass_and_fold(expanded, base.lang, cfg)
    };
    let fingerprint = fingerprint::merkle(&tree);
    if fingerprint == base.fingerprint {
        return None;
    }
    Some(Unit {
        file: base.file.clone(),
        lang: base.lang,
        name: base.name.clone(),
        byte_span: base.byte_span,
        line_span: base.line_span,
        token_count: tree.token_count(),
        parse_degraded: base.parse_degraded,
        is_test: base.is_test,
        accept_drift: base.accept_drift,
        fingerprint,
        tree,
        variant: Some(VariantTag {
            base: base_idx,
            chain: Vec::new(),
            expanded_fps: Vec::new(),
            scc: false,
        }),
    })
}

impl Unit {
    /// Human-readable inline chain: "caller ⇐ callee1, callee2" (spec §5.4:
    /// the report must show which callees were expanded).
    pub fn chain_line(&self) -> Option<String> {
        let tag = self.variant.as_ref()?;
        let scc = if tag.scc { " (scc)" } else { "" };
        Some(format!("{} ⇐ {}{}", self.name, tag.chain.join(", "), scc))
    }
}

/// The plain unit an index reports as: itself, or a variant's base.
pub fn base_of(units: &[Unit], i: usize) -> usize {
    units[i].variant.as_ref().map(|t| t.base).unwrap_or(i)
}

/// In-memory extraction (tests, future stdin mode). Plain units only.
pub fn units_from_source(src: &str, lang: Lang, cfg: &Config) -> Vec<Unit> {
    extract_file_units(Path::new("memory.in"), src, lang, cfg).0
}

pub fn byte_to_line(src: &str, byte: u32) -> u32 {
    let byte = (byte as usize).min(src.len());
    src.as_bytes()[..byte]
        .iter()
        .filter(|&&b| b == b'\n')
        .count() as u32
        + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ir_cfg() -> Config {
        let mut cfg = Config::default();
        cfg.normalize.normalizer = Normalizer::Ir;
        cfg
    }

    #[test]
    fn ir_normalizer_is_the_default() {
        // The canonical IR is the default since the §8 switchover: each function lowers to the
        // `Unit` canonical root (the `Unit` kind is IR-only).
        let cfg = Config::default();
        assert_eq!(cfg.normalize.normalizer, Normalizer::Ir);
        let units = units_from_source("fn add(a: i32) -> i32 { return a + 1; }", Lang::Rust, &cfg);
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].tree.kind.as_ref(), "Unit");
    }

    #[test]
    fn historical_normalizer_is_still_selectable() {
        // The per-grammar historical normalizer stays fully supported: selecting it keeps the
        // per-grammar tree (the `Unit` canonical root is IR-only), unchanged from before the flip.
        let mut cfg = Config::default();
        cfg.normalize.normalizer = Normalizer::Historical;
        let units = units_from_source("fn add(a: i32) -> i32 { return a + 1; }", Lang::Rust, &cfg);
        assert_eq!(units.len(), 1);
        assert_ne!(units[0].tree.kind.as_ref(), "Unit");
    }

    #[test]
    fn ir_normalizer_selector_produces_canonical_units() {
        // `normalizer = "ir"` lowers to the canonical `Unit` root (not `function_item`).
        let units = units_from_source(
            "fn add(a: i32) -> i32 { return a + 1; }",
            Lang::Rust,
            &ir_cfg(),
        );
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].tree.kind.as_ref(), "Unit");
    }

    #[test]
    fn ir_path_folds_internal_repeats() {
        fn has_repeat(n: &NormNode) -> bool {
            n.kind.as_ref() == "REPEAT" || n.children.iter().any(has_repeat)
        }
        let cfg = ir_cfg();
        // A run of ≥3 identical statements folds to a REPEAT (proves fold is wired on the
        // IR path) — the convergence that makes rolled duplication detectable.
        let folded = "fn f(a: i32) { g(a); g(a); g(a); g(a); }";
        let fu =
            extract_file_units_keep_raw(std::path::Path::new("m.rs"), folded, Lang::Rust, &cfg);
        assert!(has_repeat(&fu.units[0].tree), "run did not fold to REPEAT");

        // A run of substantial statements also clears the reporting floor → a finding.
        let big = "fn f(a: i32, xs: &[i32]) { let t = a + xs[a] * 7 + xs[a]; let t = a + xs[a] * 7 + xs[a]; let t = a + xs[a] * 7 + xs[a]; let t = a + xs[a] * 7 + xs[a]; }";
        let fb = extract_file_units_keep_raw(std::path::Path::new("m.rs"), big, Lang::Rust, &cfg);
        assert!(
            !fb.repeats.is_empty(),
            "no internal-repeat finding above the floor"
        );
    }
}
