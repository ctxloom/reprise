//! Protocol-neutral helpers shared by reprise's LSP and MCP server surfaces
//! (docs/SERVERS.md). **Sync and tokio-free by design (D-SRV-2):** the async
//! runtime lives only in the `reprise-mcp` / `reprise-lsp` crates, never here
//! or in the core `reprise` lib/CLI. Everything here projects the existing
//! `reprise` report model — no new detection, no source-tree access.

pub mod diagnostics;

use reprise::lang::Lang;
use reprise::report::{Group, Member};
use std::path::Path;
use std::process::Command;

/// Map an MCP/LSP language argument to a `reprise` language.
///
/// Accepts both canonical names (`rust`, `python`, `typescript`, `tsx`, `go`,
/// `kotlin`, `c`) and the file extensions reprise keys on (`rs`, `py`, `ts`,
/// `tsx`, `go`, `kt`, `kts`, `c`, `h`), case-insensitively. Returns `None` for
/// anything reprise does not support so the caller can surface an explicit
/// unsupported-language error rather than guess. reprise's own
/// `Lang::from_path` is extension-only and takes a `Path`; this is the
/// string-facing door the servers need.
pub fn lang_from_str(s: &str) -> Option<Lang> {
    match s.trim().to_ascii_lowercase().as_str() {
        "rust" | "rs" => Some(Lang::Rust),
        "python" | "py" => Some(Lang::Python),
        "typescript" | "ts" => Some(Lang::TypeScript),
        "tsx" => Some(Lang::Tsx),
        "go" => Some(Lang::Go),
        "kotlin" | "kt" | "kts" => Some(Lang::Kotlin),
        // `.h` parses as C until C++ support exists (mirrors `Lang::from_path`, WP-K1a).
        "c" | "h" => Some(Lang::C),
        _ => None,
    }
}

/// A group's similarity — the reporter's derived `1 - divergence` (exact tiers
/// carry divergence 0, hence similarity 1.0). Both server projections display
/// it, so it is computed once here rather than reinvented per surface.
pub fn similarity(group: &Group) -> f64 {
    1.0 - group.divergence
}

/// Split a group into `(primary member, related members)` — the projection
/// every "one finding, N locations" surface needs: SARIF's `locations[0]` +
/// `relatedLocations[]`, and the LSP diagnostic + `relatedInformation[]` shape
/// (docs/SERVERS.md §4.2). `None` for a memberless group — not expected from a
/// real finding, but total by construction.
pub fn split_primary(group: &Group) -> Option<(&Member, &[Member])> {
    group.members.split_first()
}

/// How a server chooses `check`'s base ref when the caller did not pass one
/// (docs/SERVERS.md §5). Precedence, highest first:
///   1. an explicit base the caller supplies (e.g. an MCP `base` argument),
///   2. `[baseline] ref` pinned in `reprise.toml` (`config.baseline.pinned`),
///   3. the merge-base of `HEAD` with the repo's default branch — the
///      interactive PR-preview base,
///   4. `HEAD`, when git is unavailable or history is too shallow for a base.
///
/// Infallible: it always yields *some* ref, degrading to `HEAD` rather than
/// erroring, so a server never fails a request purely on baseline resolution.
pub fn resolve_base(root: &Path, explicit: Option<&str>, cfg: &reprise::Config) -> String {
    explicit_or_pinned(explicit, cfg)
        .unwrap_or_else(|| merge_base_with_default(root).unwrap_or_else(|| "HEAD".to_string()))
}

/// The git-free half of [`resolve_base`]: an explicit base, else the pinned
/// config ref. `None` when neither is set (blank/whitespace counts as unset),
/// leaving the caller to fall back to the git merge-base.
fn explicit_or_pinned(explicit: Option<&str>, cfg: &reprise::Config) -> Option<String> {
    if let Some(b) = explicit.map(str::trim).filter(|b| !b.is_empty()) {
        return Some(b.to_string());
    }
    cfg.baseline
        .pinned
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(String::from)
}

/// The merge-base commit between `HEAD` and the repo's default branch. `None`
/// if git is absent, `root` is not a repo, or no base is found — [`resolve_base`]
/// then falls back to `HEAD`.
fn merge_base_with_default(root: &Path) -> Option<String> {
    let default = default_branch(root)?;
    let sha = git(root, &["merge-base", "HEAD", &default])?;
    let sha = sha.trim();
    (!sha.is_empty()).then(|| sha.to_string())
}

/// The repo's default branch ref: the remote's advertised default
/// (`origin/HEAD` → e.g. `origin/main`), else the first common branch that
/// actually exists.
fn default_branch(root: &Path) -> Option<String> {
    if let Some(sym) = git(
        root,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) {
        let name = sym.trim();
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    ["origin/main", "origin/master", "main", "master"]
        .into_iter()
        .find(|cand| git(root, &["rev-parse", "--verify", "--quiet", cand]).is_some())
        .map(String::from)
}

/// Run a git subcommand in `root`, returning trimmed-nothing stdout on a clean
/// exit and `None` otherwise. Never panics: a missing git binary is just `None`.
///
/// Scrubs GIT_INDEX_FILE/GIT_DIR/GIT_WORK_TREE for the same reason as
/// `reprise::check::git_cmd`: an inherited value redirects this child at the
/// CALLER's repo state (e.g. a pre-commit hook's index) instead of `root`.
fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use reprise::report::{Group, Member, Tier};

    fn member(name: &str) -> Member {
        Member {
            file: std::path::PathBuf::from(name),
            lang: "rust".into(),
            name: name.into(),
            line_span: (1, 2),
            parse_degraded: false,
        }
    }

    fn group_with(divergence: f64, members: Vec<Member>) -> Group {
        Group {
            id: "g".into(),
            tier: Tier::NearNormalized,
            fingerprint: "0".into(),
            token_count: 10,
            value: 1.0,
            note: None,
            divergence,
            template: None,
            inline_chain: None,
            members,
        }
    }

    #[test]
    fn lang_from_str_accepts_names_and_extensions() {
        assert_eq!(lang_from_str("rust"), Some(Lang::Rust));
        assert_eq!(lang_from_str("rs"), Some(Lang::Rust));
        assert_eq!(lang_from_str("PYTHON"), Some(Lang::Python));
        assert_eq!(lang_from_str(" ts "), Some(Lang::TypeScript));
        assert_eq!(lang_from_str("tsx"), Some(Lang::Tsx));
        assert_eq!(lang_from_str("go"), Some(Lang::Go));
        assert_eq!(lang_from_str("kt"), Some(Lang::Kotlin));
        assert_eq!(lang_from_str("kotlin"), Some(Lang::Kotlin));
        assert_eq!(lang_from_str("c"), Some(Lang::C));
        assert_eq!(lang_from_str("h"), Some(Lang::C));
        assert_eq!(lang_from_str("java"), None);
        assert_eq!(lang_from_str(""), None);
    }

    #[test]
    fn similarity_is_one_minus_divergence() {
        assert_eq!(similarity(&group_with(0.0, vec![])), 1.0);
        assert!((similarity(&group_with(0.25, vec![])) - 0.75).abs() < 1e-9);
    }

    #[test]
    fn split_primary_separates_first_from_rest() {
        let g = group_with(0.1, vec![member("a.rs"), member("b.rs"), member("c.rs")]);
        let (primary, related) = split_primary(&g).unwrap();
        assert_eq!(primary.name, "a.rs");
        assert_eq!(related.len(), 2);
        assert!(split_primary(&group_with(0.1, vec![])).is_none());
    }

    #[test]
    fn base_precedence_is_explicit_then_pinned_then_none() {
        let mut cfg = reprise::Config::default();
        cfg.baseline.pinned = Some("v1.4.0".into());
        assert_eq!(
            explicit_or_pinned(Some("origin/main"), &cfg).as_deref(),
            Some("origin/main")
        );
        // Blank explicit falls through to the pinned ref.
        assert_eq!(
            explicit_or_pinned(Some("  "), &cfg).as_deref(),
            Some("v1.4.0")
        );
        assert_eq!(explicit_or_pinned(None, &cfg).as_deref(), Some("v1.4.0"));
        assert_eq!(explicit_or_pinned(None, &reprise::Config::default()), None);
    }
}
