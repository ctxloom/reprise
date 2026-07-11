//! Debug: dump the D4 sequence-tier token stream of the first unit of each
//! given file and print their longest common contiguous run.
//! Usage: cargo run --example dbg_stream -- a.rs b.rs [unit_name_a unit_name_b]

use reprise::config::Config;
use reprise::lang::Lang;
use reprise::tree::{Label, NormNode};

fn serialize(node: &NormNode, out: &mut Vec<String>, li: &reprise::intern::LabelInterner) {
    // Resolved TEXT is what's compared across the two streams below (each built from
    // its own, independently-scoped `units_from_source` call) — safe: interning-id-
    // conversion WP's `LSym` ids are only meaningful within one interner, but the
    // RESOLVED strings baked in here are ordinary content, comparable regardless of
    // which interner produced them.
    let label = match &node.label {
        None => String::new(),
        Some(Label::External(s)) => format!("E{}", li.resolve(*s)),
        Some(Label::Local(_)) => "L".to_string(),
        Some(Label::LitKept(s)) => format!("K{}", li.resolve(*s)),
        Some(Label::LitBucket(b)) => format!("B{}", b.name()),
        Some(Label::Raw(s)) | Some(Label::RawLit(s)) => format!("R{s}"),
    };
    out.push(format!(
        "{}/{}/{}",
        node.kind,
        node.field.map(|f| f.as_str()).unwrap_or(""),
        label
    ));
    for child in &node.children {
        serialize(child, out, li);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cfg = Config::default();
    let mut streams = Vec::new();
    for (i, path) in args.iter().take(2).enumerate() {
        let src = std::fs::read_to_string(path).unwrap();
        let lang = Lang::from_path(std::path::Path::new(path)).unwrap();
        let label_interner = reprise::intern::LabelInterner::new();
        let units =
            reprise::unit::units_from_source_with_interner(&src, lang, &cfg, &label_interner);
        let unit = match args.get(2 + i) {
            Some(name) => units.iter().find(|u| &u.name == name).unwrap(),
            None => &units[0],
        };
        let mut toks = Vec::new();
        serialize(unit.tree.expect_resident(), &mut toks, &label_interner);
        eprintln!("{}: unit {} with {} tokens", path, unit.name, toks.len());
        streams.push(toks);
    }
    // Longest common contiguous run (quadratic, debug only).
    let (a, b) = (&streams[0], &streams[1]);
    let mut best = (0usize, 0usize, 0usize);
    let mut dp = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        let mut prev = 0usize;
        for j in 1..=b.len() {
            let cur = dp[j];
            dp[j] = if a[i - 1] == b[j - 1] { prev + 1 } else { 0 };
            if dp[j] > best.0 {
                best = (dp[j], i, j);
            }
            prev = cur;
        }
    }
    let (len, ai, _bj) = best;
    eprintln!("longest common run: {len} tokens");
    for t in &a[ai - len..ai] {
        println!("  {t}");
    }
}
