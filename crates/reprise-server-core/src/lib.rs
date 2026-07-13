//! Protocol-neutral helpers shared by reprise's LSP and MCP server surfaces
//! (docs/SERVERS.md). **Sync and tokio-free by design (D-SRV-2):** the async
//! runtime lives only in the `reprise-mcp` / `reprise-lsp` crates, never here
//! or in the core `reprise` lib/CLI. Everything here projects the existing
//! `reprise` report model — no new detection, no source-tree access.

pub mod diagnostics;

use reprise::lang::Lang;
use reprise::report::{Group, Member};

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

/// `check`'s base-ref resolution — explicit → pinned `[baseline] ref` →
/// merge-base with the default branch (the PR base) → `HEAD`.
///
/// Re-exported from the core (`reprise::baseline::resolve_base`), NOT
/// reimplemented: the CLI and both servers must resolve the base identically, or
/// a local pre-commit run and CI would gate different diffs (docs/SERVERS.md §5,
/// DECISIONS.md D52).
pub use reprise::baseline::resolve_base;

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
}
