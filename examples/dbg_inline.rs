//! TEMPORARY debug harness for M3a bring-up. Deleted before hand-off.
//! Usage: cargo run --example dbg_inline -- dir

use reprise::config::Config;
use reprise::tree::NormNode;

fn render(node: &NormNode, depth: usize, out: &mut String) {
    out.push_str(&"  ".repeat(depth));
    out.push_str(&format!(
        "{} field={:?} label={:?}\n",
        node.kind, node.field, node.label
    ));
    for c in &node.children {
        render(c, depth + 1, out);
    }
}

fn main() {
    let dir = std::env::args().nth(1).unwrap();
    let cfg = Config::default();
    let report = reprise::scan(std::path::Path::new(&dir), &cfg).unwrap();
    println!("stats: {:?}", report.stats);

    // Re-run the pipeline pieces to inspect variants.
    let files = reprise::walk::collect_files(std::path::Path::new(&dir), &cfg).unwrap();
    let label_interner = reprise::intern::LabelInterner::new();
    let mut units = Vec::new();
    let mut raws = Vec::new();
    for (path, lang) in &files {
        let src = std::fs::read_to_string(path).unwrap();
        let f =
            reprise::unit::extract_file_units_keep_raw(path, &src, *lang, &cfg, &label_interner);
        units.extend(f.units);
        raws.extend(f.raw_trees);
    }
    let table = reprise::inline::DefTable::build(&units, &raws, &cfg);
    for i in 0..units.len() {
        let exp = reprise::inline::expand_unit(i, &raws[i], &units, &table, &cfg);
        println!(
            "unit {i} {} tokens={} fp={:032x} inlined={} chain={:?}",
            units[i].name, units[i].token_count, units[i].fingerprint, exp.calls_inlined, exp.chain
        );
        if exp.calls_inlined > 0 {
            if let Some(v) = reprise::unit::finish_variant(
                i,
                &units[i],
                exp.tree.expect("calls_inlined > 0"),
                &cfg,
                &label_interner,
            ) {
                println!(
                    "  variant tokens={} fp={:032x}",
                    v.token_count, v.fingerprint
                );
                let mut s = String::new();
                render(v.tree.expect_resident(), 2, &mut s);
                println!("{s}");
            } else {
                println!("  variant == base (dropped)");
            }
        }
    }
    for (i, u) in units.iter().enumerate() {
        let mut s = String::new();
        render(u.tree.expect_resident(), 1, &mut s);
        println!("== plain unit {i} {} ==\n{s}", u.name);
    }
}
