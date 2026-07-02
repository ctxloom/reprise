//! P1 file discovery: .gitignore-aware walking plus config excludes and
//! generated-path globs (spec §5.1).

use crate::config::Config;
use crate::lang::Lang;
use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;
use std::path::{Path, PathBuf};

pub fn collect_files(root: &Path, cfg: &Config) -> anyhow::Result<Vec<(PathBuf, Lang)>> {
    // A missing root must be a hard error (exit 2), not a silent empty scan —
    // a typo'd CI path would otherwise report a clean green run.
    if !root.exists() {
        anyhow::bail!("scan root does not exist: {}", root.display());
    }
    let mut overrides = OverrideBuilder::new(root);
    // The D8 cache lives under the scan root; never scan it (it's a hidden
    // dir, which the walker already skips — this is belt-and-braces).
    overrides.add("!.reprise/**")?;
    for glob in cfg.scan.exclude.iter().chain(&cfg.scan.generated_paths) {
        overrides.add(&format!("!{glob}"))?;
    }
    let overrides = overrides.build()?;

    let mut files = Vec::new();
    for entry in WalkBuilder::new(root).overrides(overrides).build() {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if let Some(lang) = Lang::from_path(entry.path()) {
            files.push((entry.into_path(), lang));
        }
    }
    files.sort();
    Ok(files)
}

/// A first line this long with no earlier newline is a minified/bundled file
/// signature (M4a): no human-written source in the supported languages keeps
/// a 512-char first line, while minifiers emit exactly that.
const MINIFIED_FIRST_LINE_LEN: usize = 512;

/// Marker-comment check in the first 5 lines (spec §5.1), plus first-line
/// signatures (M4a): minified/bundled single-line files.
pub fn is_generated(src: &str, cfg: &Config) -> bool {
    if src
        .lines()
        .next()
        .is_some_and(|l| l.len() >= MINIFIED_FIRST_LINE_LEN)
    {
        return true;
    }
    src.lines()
        .take(5)
        .any(|line| cfg.scan.generated_markers.iter().any(|m| line.contains(m)))
}
