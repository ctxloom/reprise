//! Spec §5.2 property tests: normalization is deterministic and idempotent
//! (`normalize(normalize(t)) == normalize(t)`), and source spans survive.
//! Runs over the mutation-benchmark seeds plus inline snippets.

use reprise::config::Config;
use reprise::lang::Lang;
use reprise::normalize::{apply_passes, raw_units_from_source};
use std::fs;
use std::path::PathBuf;

fn seed_sources() -> Vec<(String, Lang)> {
    let mut out = Vec::new();
    for (dir, lang, ext) in [
        ("rust", Lang::Rust, "rs"),
        ("python", Lang::Python, "py"),
        ("typescript", Lang::TypeScript, "ts"),
        ("go", Lang::Go, "go"),
        ("kotlin", Lang::Kotlin, "kt"),
    ] {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("benches/mutations/seeds")
            .join(dir);
        for entry in fs::read_dir(&root).expect("seed dir") {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) == Some(ext) {
                out.push((fs::read_to_string(&path).unwrap(), lang));
            }
        }
    }
    assert!(out.len() >= 4, "expected at least 4 seeds");
    out
}

fn inline_sources() -> Vec<(String, Lang)> {
    vec![
        (
            "fn f(xs: &[i64]) -> i64 {\n    let mut acc = 0;\n    while acc < 10 {\n        acc += 1;\n    }\n    for x in xs {\n        acc += x;\n    }\n    acc\n}\n".to_string(),
            Lang::Rust,
        ),
        (
            "def f(xs):\n    acc = 0\n    while acc < 10:\n        acc += 1\n    for x in xs:\n        acc += x\n    return acc\n".to_string(),
            Lang::Python,
        ),
    ]
}

#[test]
fn normalization_is_idempotent() {
    let cfg = Config::default();
    for (src, lang) in seed_sources().into_iter().chain(inline_sources()) {
        for (name, raw) in raw_units_from_source(&src, lang) {
            let once = apply_passes(raw.clone(), lang, &cfg);
            let twice = apply_passes(once.clone(), lang, &cfg);
            assert_eq!(
                once, twice,
                "normalize not idempotent for unit `{name}` ({lang:?})"
            );
        }
    }
}

#[test]
fn normalization_is_deterministic() {
    let cfg = Config::default();
    for (src, lang) in seed_sources().into_iter().chain(inline_sources()) {
        let a: Vec<_> = raw_units_from_source(&src, lang)
            .into_iter()
            .map(|(_, t)| apply_passes(t, lang, &cfg))
            .collect();
        let b: Vec<_> = raw_units_from_source(&src, lang)
            .into_iter()
            .map(|(_, t)| apply_passes(t, lang, &cfg))
            .collect();
        assert_eq!(a, b, "normalize not deterministic ({lang:?})");
    }
}

#[test]
fn spans_survive_normalization() {
    // Spec §4 hard requirement: every normalized node maps back into the source.
    let cfg = Config::default();
    for (src, lang) in seed_sources().into_iter().chain(inline_sources()) {
        for (name, raw) in raw_units_from_source(&src, lang) {
            let normed = apply_passes(raw, lang, &cfg);
            let mut stack = vec![&normed];
            while let Some(node) = stack.pop() {
                let (start, end) = node.span;
                assert!(
                    (start as usize) <= src.len() && (end as usize) <= src.len() && start <= end,
                    "node span ({start},{end}) escapes source (len {}) in unit `{name}`",
                    src.len()
                );
                stack.extend(node.children.iter());
            }
        }
    }
}

#[test]
fn units_meet_phase1_floor() {
    // Guard for the benchmark itself: seeds must clear `min_unit_tokens`,
    // otherwise mutation recall silently tests nothing (DECISIONS.md D6).
    let cfg = Config::default();
    for (src, lang) in seed_sources() {
        let units = reprise::units_from_source(&src, lang, &cfg);
        assert_eq!(units.len(), 1);
        assert!(
            units[0].token_count >= cfg.thresholds.min_unit_tokens,
            "seed unit `{}` has {} tokens, below floor {}",
            units[0].name,
            units[0].token_count,
            cfg.thresholds.min_unit_tokens
        );
    }
}
