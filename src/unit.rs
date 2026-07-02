//! Comparison units (spec §5.1): every function-like node, normalized,
//! folded (P4), and fingerprinted. Phase 3 adds inline-expanded variants
//! (spec §5.4): a unit fingerprints at most twice — plain plus one
//! fully-inlined variant (spec §12 cap) — and variants are tagged so the
//! matching tiers can label findings `inline-assisted`.

use crate::config::Config;
use crate::fingerprint;
use crate::fold::{self, RepeatFinding};
use crate::lang::Lang;
use crate::normalize::{self, RawUnit};
use crate::tree::NormNode;
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
    let (tree, _found) = pass_and_fold(expanded, base.lang, cfg);
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
