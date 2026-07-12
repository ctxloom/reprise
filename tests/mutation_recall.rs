//! Synthetic mutation benchmark (spec §7.1) — Phase-1 classes only.
//!
//! Phase-1 gate (spec §8): **100% recall** on Type-1/Type-2 mutation classes.
//! Each seed in `benches/mutations/seeds/` is paired with a sidecar TOML giving
//! curated rename/literal maps (DECISIONS.md D6: maps are hand-written per seed,
//! independent of the tool's own classifier, and touch no keep-list literals).
//!
//! Classes:
//!   t1-whitespace  — blank lines / trailing spaces (indentation-safe for Python)
//!   t1-comments    — full-line + end-of-line comment noise
//!   t2-rename      — word-boundary rename of all unit-local identifiers
//!   t2-literals    — non-keep-list literal value changes

use reprise::config::{Config, Normalizer};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

#[derive(serde::Deserialize)]
struct SeedMeta {
    #[serde(default)]
    rename: BTreeMap<String, String>,
    #[serde(default)]
    literals: BTreeMap<String, String>,
}

struct Seed {
    name: String,
    ext: &'static str,
    source: String,
    meta: SeedMeta,
}

/// Whether this bench run is pinned to the historical normalizer (`just bench-mutations-historical`,
/// `REPRISE_NORMALIZER=historical`) — mirrors [`bench_config`]'s own env read. C has no historical
/// `LanguageProfile` (`Lang::has_historical_profile` is false — CLAUDE.md retires the per-grammar
/// historical layer for new languages): `corpus_units`/`scan` refuse `normalizer = "historical"`
/// outright for any C file (a clean `anyhow` error, by design — see `src/lib.rs`/`src/unit.rs`).
/// [`load_seeds`] uses this to leave C OUT of the seed set on the historical run, rather than let
/// every `converges()` call on a C seed error out — the bench harness handling C's no-historical-mode
/// policy cleanly, per the WP-K1b brief, instead of the historical gate simply failing on C input.
fn normalizer_is_historical() -> bool {
    std::env::var("REPRISE_NORMALIZER").as_deref() == Ok("historical")
}

fn load_seeds() -> Vec<Seed> {
    let mut seeds = Vec::new();
    let mut dirs = vec![
        ("rust", "rs"),
        ("python", "py"),
        ("typescript", "ts"),
        ("go", "go"),
        ("kotlin", "kt"),
    ];
    // C is IR-frontend-only (WP-K1b) — excluded from the historical run (see
    // `normalizer_is_historical`'s doc comment), included on every other run (the default IR
    // path `just bench-mutations`/`bench-mutations-ir`).
    if !normalizer_is_historical() {
        dirs.push(("c", "c"));
    }
    for (dir, ext) in dirs {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("benches/mutations/seeds")
            .join(dir);
        let mut paths: Vec<_> = fs::read_dir(&root)
            .expect("seed dir")
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(ext))
            .collect();
        paths.sort();
        for path in paths {
            let meta_path = path.with_extension("toml");
            let meta: SeedMeta =
                toml::from_str(&fs::read_to_string(&meta_path).expect("seed meta")).unwrap();
            seeds.push(Seed {
                name: format!("{dir}/{}", path.file_stem().unwrap().to_str().unwrap()),
                ext,
                source: fs::read_to_string(&path).unwrap(),
                meta,
            });
        }
    }
    assert!(seeds.len() >= 4, "need at least 2 seeds per language");
    seeds
}

// ---------- mutators ----------

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Word-boundary replacement without the regex crate. Applies all rules in a
/// single left-to-right pass (longest key wins), so outputs are never re-matched.
fn replace_words(src: &str, map: &BTreeMap<String, String>) -> String {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort_by_key(|k| std::cmp::Reverse(k.len()));
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    'outer: while i < bytes.len() {
        let prev_ok = i == 0 || !is_word(bytes[i - 1] as char);
        if prev_ok {
            for key in &keys {
                let k = key.as_bytes();
                if bytes[i..].starts_with(k) {
                    let next = i + k.len();
                    let next_ok = next >= bytes.len() || !is_word(bytes[next] as char);
                    if next_ok {
                        out.push_str(&map[*key]);
                        i = next;
                        continue 'outer;
                    }
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Plain (non-word-boundary) ordered substring replacement for literals.
fn replace_literals(src: &str, map: &BTreeMap<String, String>) -> String {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort_by_key(|k| std::cmp::Reverse(k.len()));
    let mut out = src.to_string();
    for key in keys {
        out = out.replace(key.as_str(), &map[key]);
    }
    out
}

fn mutate_whitespace(src: &str, ext: &str) -> String {
    let mut out = String::new();
    for line in src.lines() {
        out.push_str(line);
        if !line.trim().is_empty() {
            out.push_str("   "); // trailing spaces
        }
        out.push('\n');
        out.push('\n'); // blank line between every line (indentation-safe)
    }
    if ext == "rs" {
        // Whitespace is fully insignificant in Rust: also reindent everything.
        out = out
            .lines()
            .map(|l| {
                if l.trim().is_empty() {
                    String::new()
                } else {
                    format!("  {l}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        out.push('\n');
    }
    out
}

fn mutate_comments(src: &str, ext: &str) -> String {
    // `//` for the C-family grammars (rs/ts/go/kt); `#` only for Python.
    let marker = if ext == "py" {
        "# mutation noise"
    } else {
        "// mutation noise"
    };
    let mut out = String::new();
    for line in src.lines() {
        // Full-line comment before each nonempty line, at matching indentation.
        if !line.trim().is_empty() {
            let indent: String = line.chars().take_while(|c| *c == ' ').collect();
            out.push_str(&indent);
            out.push_str(marker);
            out.push('\n');
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

// ---------- harness ----------

/// Base config for a bench run. `REPRISE_NORMALIZER=ir` (or any known selector)
/// runs the ENTIRE bench on that normalizer path — the standing IR-recall gate
/// (`just bench-mutations-ir`). Unset keeps the historical default, so a plain
/// `just bench-mutations` is byte-identical to before. This exists because the
/// bench formerly hard-coded `Config::default()` (historical), so IR-path
/// mutation recall was never measured (the §8 flip masking).
fn bench_config() -> Config {
    let mut cfg = Config::default();
    if let Ok(n) = std::env::var("REPRISE_NORMALIZER") {
        cfg.normalize.normalizer = n
            .parse::<Normalizer>()
            .expect("REPRISE_NORMALIZER must be one of: ir, historical");
    }
    // Landmark rarity / fan-out overrides, so the oracle can gate a `retrieval.landmark_*`
    // sweep (the bench builds its Config in-process and never reads a `reprise.toml`).
    //
    // ⚠ READ THIS BEFORE USING THE ORACLE AS A RARITY GATE. `converges()` scans a temp dir
    // holding exactly TWO files, so a bench corpus has a handful of units: `rare_cap` pins to
    // its FLOOR (`3.max(n/20)` = 3) and the largest document frequency any subtree can reach
    // is 2. Every subtree is therefore "rare" no matter what, and the rarity gate CANNOT BIND.
    // `landmark_rare_divisor` and `landmark_df_over_offsets` are consequently INERT here — the
    // oracle is structurally incapable of pricing them, and a green run says nothing about
    // their corpus-scale recall cost. Rarity only bites at scale; measure it on fs/net against
    // the verify-on-union ACCEPT set. (`landmark_fan_out` and a fixed `landmark_rare_cap` <= 2
    // DO bind here.) Measured in the rarity-sweep WP (2026-07); see CALIBRATION.md / DECISIONS.md when landed.
    if let Ok(v) = std::env::var("REPRISE_LM_RARE_DIVISOR") {
        cfg.retrieval.landmark_rare_divisor = v.parse().expect("REPRISE_LM_RARE_DIVISOR: u32");
    }
    if let Ok(v) = std::env::var("REPRISE_LM_RARE_FLOOR") {
        cfg.retrieval.landmark_rare_floor = v.parse().expect("REPRISE_LM_RARE_FLOOR: u32");
    }
    if let Ok(v) = std::env::var("REPRISE_LM_RARE_CAP") {
        cfg.retrieval.landmark_rare_cap = Some(v.parse().expect("REPRISE_LM_RARE_CAP: u32"));
    }
    if let Ok(v) = std::env::var("REPRISE_LM_FAN_OUT") {
        cfg.retrieval.landmark_fan_out = v.parse().expect("REPRISE_LM_FAN_OUT: usize");
    }
    if std::env::var_os("REPRISE_LM_DF_FIX").is_some() {
        cfg.retrieval.landmark_df_over_offsets = true;
    }
    cfg
}

/// Returns true iff scanning {seed, mutant} yields a reportable group
/// (exact-normalized or near-normalized; weak-similarity does not count as
/// recall) whose members span both files.
fn converges(seed: &Seed, mutant_source: &str) -> bool {
    let dir = TempDir::new().unwrap();
    let seed_path = dir.path().join(format!("seed.{}", seed.ext));
    let mutant_path = dir.path().join(format!("mutant.{}", seed.ext));
    fs::write(&seed_path, &seed.source).unwrap();
    fs::write(&mutant_path, mutant_source).unwrap();
    let report = reprise::scan(dir.path(), &bench_config()).unwrap();
    report
        .groups
        .iter()
        .filter(|g| {
            matches!(
                g.tier.to_string().as_str(),
                "exact-normalized" | "near-normalized"
            )
        })
        .any(|g| {
            let files: Vec<_> = g.members.iter().map(|m| &m.file).collect();
            files
                .iter()
                .any(|f| f.ends_with(seed_path.file_name().unwrap()))
                && files
                    .iter()
                    .any(|f| f.ends_with(mutant_path.file_name().unwrap()))
        })
}

/// Curated structural variants: benches/mutations/variants/<lang>/<seed>.<class>.<ext>
/// Companion corpus files (`<seed>.<class>.helper.<ext>`) are not variants;
/// `load_companions` picks them up per class.
fn load_variants(seed: &Seed) -> Vec<(String, String)> {
    let (dir, stem) = seed.name.split_once('/').unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("benches/mutations/variants")
        .join(dir);
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(&root) {
        let mut paths: Vec<_> = entries.map(|e| e.unwrap().path()).collect();
        paths.sort();
        for path in paths {
            let fname = path.file_name().unwrap().to_str().unwrap();
            let prefix = format!("{stem}.");
            let suffix = format!(".{}", seed.ext);
            if let Some(rest) = fname.strip_prefix(&prefix)
                && let Some(class) = rest.strip_suffix(&suffix)
                && !class.ends_with(".helper")
            {
                out.push((class.to_string(), fs::read_to_string(&path).unwrap()));
            }
        }
    }
    out
}

/// Extra corpus files a class ships alongside seed+variant (e.g. the helper
/// definition the t4-inline-helper class needs present for the inliner).
fn load_companions(seed: &Seed, class: &str) -> Vec<String> {
    let (dir, stem) = seed.name.split_once('/').unwrap();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("benches/mutations/variants")
        .join(dir)
        .join(format!("{stem}.{class}.helper.{}", seed.ext));
    fs::read_to_string(&path).into_iter().collect()
}

#[test]
fn phase1_mutation_recall_is_100_percent() {
    let seeds = load_seeds();
    let mut results: BTreeMap<&str, (u32, u32)> = BTreeMap::new();
    let mut failures = Vec::new();

    for seed in &seeds {
        let classes: Vec<(&str, String)> = vec![
            ("t1-whitespace", mutate_whitespace(&seed.source, seed.ext)),
            ("t1-comments", mutate_comments(&seed.source, seed.ext)),
            ("t2-rename", replace_words(&seed.source, &seed.meta.rename)),
            (
                "t2-literals",
                replace_literals(&seed.source, &seed.meta.literals),
            ),
        ];
        for (class, mutant) in classes {
            assert_ne!(
                mutant, seed.source,
                "mutator `{class}` produced identity on {}",
                seed.name
            );
            let entry = results.entry(class).or_insert((0, 0));
            entry.1 += 1;
            if converges(seed, &mutant) {
                entry.0 += 1;
            } else {
                failures.push(format!("{class} on {}", seed.name));
            }
        }
    }

    println!("\nmutation recall (Phase 1):");
    for (class, (hit, total)) in &results {
        println!("  {class:<16} {hit}/{total}");
    }
    assert!(
        failures.is_empty(),
        "Phase-1 gate requires 100% recall; missed:\n  {}",
        failures.join("\n  ")
    );
}

/// Phase-2 gate (spec §8): ≥80% recall on Type-3 classes, ≥70% on
/// tail-recursion, and **0%** on the designed-to-fail controls.
#[test]
fn phase2_mutation_recall_meets_gate() {
    let seeds = load_seeds();
    let mut results: BTreeMap<String, (u32, u32)> = BTreeMap::new();

    for seed in &seeds {
        for (class, variant) in load_variants(seed) {
            if class.starts_with("t4-") {
                continue; // Phase-3 classes need companion corpus files
            }
            assert_ne!(
                variant, seed.source,
                "variant `{class}` identical on {}",
                seed.name
            );
            let entry = results.entry(class).or_insert((0, 0));
            entry.1 += 1;
            if converges(seed, &variant) {
                entry.0 += 1;
            }
        }
    }

    println!("\nmutation recall (Phase 2):");
    for (class, (hit, total)) in &results {
        println!("  {class:<22} {hit}/{total}");
    }

    let recall = |class: &str| -> f64 {
        let (hit, total) = results
            .get(class)
            .unwrap_or_else(|| panic!("no variants for class {class}"));
        f64::from(*hit) / f64::from(*total)
    };
    for t3 in [
        "t3-loop-swap",
        "t3-reorder",
        "t3-subtree-sub",
        "t3-light-edit",
        "t3-unroll",
    ] {
        assert!(recall(t3) >= 0.8, "{t3} recall {} < 0.8", recall(t3));
    }
    assert!(
        recall("t2r-tail-recursion") >= 0.7,
        "tail-recursion recall {} < 0.7",
        recall("t2r-tail-recursion")
    );
    assert!(
        recall("ctl-tree-recursion") == 0.0,
        "tree-recursion control converged — recursion lowering is misfiring"
    );
}

/// Like `converges`, but with companion files in the corpus and accepting the
/// `inline-assisted` tier (spec §5.4): the Phase-3 classes converge through
/// the inliner, which is a weaker-but-counted tier per §6.
fn converges_inline(seed: &Seed, variant_source: &str, companions: &[String]) -> bool {
    let dir = TempDir::new().unwrap();
    let seed_path = dir.path().join(format!("seed.{}", seed.ext));
    let variant_path = dir.path().join(format!("mutant.{}", seed.ext));
    fs::write(&seed_path, &seed.source).unwrap();
    fs::write(&variant_path, variant_source).unwrap();
    for (i, src) in companions.iter().enumerate() {
        fs::write(dir.path().join(format!("helper{i}.{}", seed.ext)), src).unwrap();
    }
    let report = reprise::scan(dir.path(), &bench_config()).unwrap();
    report
        .groups
        .iter()
        .filter(|g| {
            matches!(
                g.tier.to_string().as_str(),
                "exact-normalized" | "near-normalized" | "inline-assisted"
            )
        })
        .any(|g| {
            let files: Vec<_> = g.members.iter().map(|m| &m.file).collect();
            files
                .iter()
                .any(|f| f.ends_with(seed_path.file_name().unwrap()))
                && files
                    .iter()
                    .any(|f| f.ends_with(variant_path.file_name().unwrap()))
        })
}

/// Phase-3 gate (spec §8): inline-helper class recall ≥70%, mutual-recursion
/// class recall ≥50% (the SCC chain is speculative; the low bar is honest).
#[test]
fn phase3_mutation_recall_meets_gate() {
    let seeds = load_seeds();
    let mut results: BTreeMap<String, (u32, u32)> = BTreeMap::new();

    for seed in &seeds {
        for (class, variant) in load_variants(seed) {
            if !class.starts_with("t4-") {
                continue;
            }
            assert_ne!(
                variant, seed.source,
                "variant `{class}` identical on {}",
                seed.name
            );
            let companions = load_companions(seed, &class);
            let entry = results.entry(class.clone()).or_insert((0, 0));
            entry.1 += 1;
            if converges_inline(seed, &variant, &companions) {
                entry.0 += 1;
            }
        }
    }

    println!("\nmutation recall (Phase 3):");
    for (class, (hit, total)) in &results {
        println!("  {class:<22} {hit}/{total}");
    }

    let recall = |class: &str| -> f64 {
        let (hit, total) = results
            .get(class)
            .unwrap_or_else(|| panic!("no variants for class {class}"));
        f64::from(*hit) / f64::from(*total)
    };
    assert!(
        recall("t4-inline-helper") >= 0.7,
        "inline-helper recall {} < 0.7",
        recall("t4-inline-helper")
    );
    assert!(
        recall("t4-mutual-recursion") >= 0.5,
        "mutual-recursion recall {} < 0.5 — the SCC chain failed its gate",
        recall("t4-mutual-recursion")
    );
}

/// Random-pair control (spec §7.1): scanning ALL seeds together must produce
/// no reportable group that mixes different seed families.
#[test]
fn cross_seed_control_produces_no_groups() {
    let seeds = load_seeds();
    let dir = TempDir::new().unwrap();
    for seed in &seeds {
        let fname = format!("{}.{}", seed.name.replace('/', "_"), seed.ext);
        fs::write(dir.path().join(fname), &seed.source).unwrap();
    }
    let report = reprise::scan(dir.path(), &bench_config()).unwrap();
    let mixed: Vec<_> = report
        .groups
        .iter()
        .filter(|g| {
            let stems: Vec<_> = g
                .members
                .iter()
                .map(|m| m.file.file_stem().unwrap().to_str().unwrap().to_string())
                .collect();
            stems.iter().any(|s| s != &stems[0])
        })
        .collect();
    assert!(
        mixed.is_empty(),
        "cross-seed groups (false positives): {mixed:#?}"
    );
}
