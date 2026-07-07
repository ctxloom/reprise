//! reprise as an MCP tool surface (docs/SERVERS.md §5). This module is the
//! `rmcp`-independent core: each MCP tool is a thin wrapper over one of these
//! functions, which call the `reprise` lib and project its report model to
//! JSON (the same model `reprise scan --format json` emits). Keeping the reprise
//! integration separate from the transport/`rmcp` wiring lets it be tested
//! without standing up a server, and keeps the async runtime out of this layer.

use std::path::Path;

/// `reprise_scan` — full-repo scan. Returns the `ScanReport` as pretty JSON.
/// `path` is the scan root; config is loaded from `<path>/reprise.toml` if
/// present, else defaults (reprise runs with zero config).
pub fn scan_json(path: &str) -> anyhow::Result<String> {
    let root = Path::new(path);
    let cfg = reprise::Config::load(root)?;
    let report = reprise::scan(root, &cfg)?;
    Ok(serde_json::to_string_pretty(&report)?)
}

/// `reprise_check` — drift check against a base ref. When `base` is omitted it
/// is resolved by the shared policy (explicit → pinned `reprise.toml` ref →
/// merge-base with the default branch → `HEAD`; docs/SERVERS.md §5). Returns the
/// `CheckReport` as pretty JSON, including the `inconsistent-update` findings and
/// per-group divergence `trend`.
pub fn check_json(path: &str, base: Option<&str>) -> anyhow::Result<String> {
    let root = Path::new(path);
    let cfg = reprise::Config::load(root)?;
    let base = reprise_server_core::resolve_base(root, base, &cfg);
    let report = reprise::check::run(root, &cfg, &base, None)?;
    Ok(serde_json::to_string_pretty(&report)?)
}

/// Cap on returned matches — the most similar existing units, ranked.
const MAX_MATCHES: usize = 20;

#[derive(serde::Serialize)]
struct SimMatch {
    /// Repository-relative path of the existing unit.
    file: String,
    name: String,
    line_span: (u32, u32),
    /// "exact" (identical after rename/literal/loop-form normalization) or "near".
    kind: &'static str,
    /// `1 - divergence`.
    similarity: f64,
    divergence: f64,
    /// Number of divergence holes (0 for exact).
    holes: usize,
    /// Shared-skeleton template with `⟨h…⟩` holes — near matches only.
    #[serde(skip_serializing_if = "Option::is_none")]
    template: Option<String>,
}

/// `reprise_find_similar` — does `snippet` (a complete function/method) duplicate
/// something already in the repo at `path`? Normalizes the snippet to a unit and
/// matches it one-vs-many against every same-language unit: exact by structural
/// fingerprint (Type-1/2 — after rename/literal/loop-form normalization), else
/// anti-unified under reprise's clone thresholds (Type-3). Returns ranked matches
/// as JSON. Report-only — the agent decides whether to call the existing helper.
pub fn find_similar_json(path: &str, snippet: &str, lang: &str) -> anyhow::Result<String> {
    let lang = reprise_server_core::lang_from_str(lang)
        .ok_or_else(|| anyhow::anyhow!("unsupported language: {lang:?}"))?;
    let root = Path::new(path);
    let cfg = reprise::Config::load(root)?;
    let floor = cfg.min_unit_floor();

    // Candidate unit from the in-memory snippet (no file on disk). The largest
    // function-like unit is the candidate when the snippet holds several.
    let candidate = reprise::units_from_source(snippet, lang, &cfg)
        .into_iter()
        .max_by_key(|u| u.token_count)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no function-like unit found in the snippet for language {}",
                lang.name()
            )
        })?;

    // Same-language corpus, assembled from public API (walk + per-file extract)
    // rather than by factoring scan() — the corpus accessor is deferred to avoid
    // colliding with the concurrent IR rework (task inner-envy). No cache reuse
    // here; the held index (docs/SERVERS.md §6, M5) is the answer if this is slow.
    let ir = cfg.normalize.normalizer == reprise::config::Normalizer::Ir
        && reprise::frontend::has_ir_frontend(lang);
    // The historical `LanguageProfile` is anti_unify's structural oracle ONLY on the historical
    // path; the IR path answers those predicates from canonical-IR kinds. Bind a profile solely
    // when `!ir`, so the IR case never touches `LanguageProfile` (the profile call vanishes for
    // it); a `historical`/TS/Kotlin scan still supplies it.
    let profile = (!ir).then(|| lang.profile());
    let mut matches: Vec<SimMatch> = Vec::new();
    for (file, flang) in reprise::walk::collect_files(root, &cfg)? {
        if flang != lang {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&file) else {
            continue;
        };
        let (units, _) = reprise::unit::extract_file_units(&file, &src, flang, &cfg);
        for u in units {
            if u.token_count < floor {
                continue; // below reprise's index floor — never a reportable clone
            }
            if let Some(m) = compare(&candidate, &u, profile, ir, &cfg, root) {
                matches.push(m);
            }
        }
    }

    // Exact matches (divergence 0) first, then ascending divergence.
    matches.sort_by(|a, b| {
        a.divergence
            .partial_cmp(&b.divergence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    matches.truncate(MAX_MATCHES);

    let result = serde_json::json!({
        "candidate": {
            "lang": lang.name(),
            "name": candidate.name,
            "token_count": candidate.token_count,
        },
        "match_count": matches.len(),
        "matches": matches,
    });
    Ok(serde_json::to_string_pretty(&result)?)
}

/// One-vs-one verdict: exact structural duplicate, else anti-unify and accept
/// under reprise's near-clone thresholds. `None` when the pair is not a clone.
fn compare(
    cand: &reprise::Unit,
    u: &reprise::Unit,
    // The historical `LanguageProfile`, needed ONLY on the historical path (`ir == false`);
    // `None` on the IR path, which anti_unify answers from canonical-IR kinds.
    profile: Option<&dyn reprise::lang::LanguageProfile>,
    ir: bool,
    cfg: &reprise::Config,
    root: &Path,
) -> Option<SimMatch> {
    let rel = u
        .file
        .strip_prefix(root)
        .unwrap_or(&u.file)
        .display()
        .to_string();

    // Exact after normalization (renames, literals, loop form folded away).
    if u.fingerprint == cand.fingerprint {
        return Some(SimMatch {
            file: rel,
            name: u.name.clone(),
            line_span: u.line_span,
            kind: "exact",
            similarity: 1.0,
            divergence: 0.0,
            holes: 0,
            template: None,
        });
    }

    // Cheap size prefilter before the expensive anti-unification: a near clone
    // (divergence ≤ max_divergence) cannot have wildly different token counts.
    if !size_compatible(
        cand.token_count,
        u.token_count,
        cfg.thresholds.candidate_sim,
    ) {
        return None;
    }

    let outcome = reprise::au::anti_unify(&cand.tree, &u.tree, profile, ir);
    if outcome.divergence <= cfg.thresholds.max_divergence
        && outcome.holes.len() as u32 <= cfg.thresholds.max_holes
    {
        Some(SimMatch {
            file: rel,
            name: u.name.clone(),
            line_span: u.line_span,
            kind: "near",
            similarity: 1.0 - outcome.divergence,
            divergence: outcome.divergence,
            holes: outcome.holes.len(),
            template: Some(reprise::au::render_template(&outcome.template)),
        })
    } else {
        None
    }
}

/// Token-count ratio prefilter: skip pairs too different in size to be a near
/// clone. Uses reprise's own candidate-similarity threshold as the band.
fn size_compatible(a: u32, b: u32, candidate_sim: f64) -> bool {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    hi != 0 && f64::from(lo) / f64::from(hi) >= candidate_sim
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A comfortably-over-the-token-floor Python unit, parameterized by one
    /// attribute name. The attribute is `External` (not masked), so changing it
    /// yields a *near* clone (one hole), while changing names/params/locals stays
    /// exact — the two behaviours this module's matching must distinguish.
    fn dup_py(attr: &str) -> String {
        let src = "def report(records):
    lines = []
    total = 0
    count = len(records)
    for record in records:
        if not record.ATTR:
            continue
        total += 1
        label = record.name
        lines.append(label)
    lines.append(str(total))
    lines.append(str(count))
    return sep.join(lines)
";
        src.replace("ATTR", attr)
    }

    fn write(path: std::path::PathBuf, content: &str) {
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn scan_json_projects_the_report_model() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path().join("a.py"), &dup_py("enabled"));
        write(dir.path().join("b.py"), &dup_py("active"));

        let out = scan_json(dir.path().to_str().unwrap()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        // Prove the report model reaches JSON intact (the projection contract).
        // Whether a *specific* group is found is the §8 substrate-parity golden
        // test's job, not this scaffold unit test — assert the shape, not calibration.
        assert!(
            v.get("groups").and_then(|g| g.as_array()).is_some(),
            "report exposes a groups array: {out}"
        );
        assert!(v.get("stats").is_some(), "report exposes stats");
    }

    #[test]
    fn find_similar_flags_a_reimplemented_helper() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path().join("existing.py"), &dup_py("enabled"));
        // A reimplementation: renamed function, differing attribute. reprise masks
        // the name but keeps the attribute External, so this is a near clone of the
        // existing helper — exactly the "reimplemented instead of called" case.
        let snippet = dup_py("active").replace("def report(", "def summarize(");

        let out = find_similar_json(dir.path().to_str().unwrap(), &snippet, "python").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(
            v["match_count"].as_u64().unwrap() >= 1,
            "found the existing helper: {out}"
        );
        let m = &v["matches"][0];
        assert_eq!(m["name"], "report", "names the existing unit: {out}");
        assert!(
            m["similarity"].as_f64().unwrap() > 0.8,
            "high similarity: {out}"
        );
    }
}
