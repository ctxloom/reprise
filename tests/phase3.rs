//! Phase-3 integration contract: best-effort inliner + SCC chain (spec §5.4)
//! and the api-profile tier (spec §5.7), all at `scan` level.

use reprise::config::Config;
use reprise::report::ScanReport;
use std::fs;
use tempfile::TempDir;

fn scan_snippets_cfg(ext: &str, sources: &[&str], cfg: &Config) -> ScanReport {
    let dir = TempDir::new().unwrap();
    for (i, src) in sources.iter().enumerate() {
        fs::write(dir.path().join(format!("m{i}.{ext}")), src).unwrap();
    }
    reprise::scan(dir.path(), cfg).unwrap()
}

fn scan_snippets(ext: &str, sources: &[&str]) -> ScanReport {
    scan_snippets_cfg(ext, sources, &Config::default())
}

fn ir_cfg() -> Config {
    let mut cfg = Config::default();
    cfg.normalize.normalizer = "ir".into();
    cfg
}

/// Scan under the IR normalizer (`[normalize] normalizer = "ir"`).
fn scan_snippets_ir(ext: &str, sources: &[&str]) -> ScanReport {
    scan_snippets_cfg(ext, sources, &ir_cfg())
}

/// Groups at the given tier joining files m0 and m1.
fn pair_groups<'a>(report: &'a ScanReport, tier: &str) -> Vec<&'a reprise::report::Group> {
    report
        .groups
        .iter()
        .filter(|g| g.tier.to_string() == tier)
        .filter(|g| {
            let fs: Vec<_> = g
                .members
                .iter()
                .map(|m| m.file.to_string_lossy().to_string())
                .collect();
            fs.iter().any(|f| f.contains("m0.")) && fs.iter().any(|f| f.contains("m1."))
        })
        .collect()
}

// ---------- inliner (spec §5.4) ----------

const RS_CALLER_AND_HELPER: &str = r#"
fn apply_discounts(prices: &mut Vec<i64>, cutoff: i64) -> i64 {
    let mut total = 0;
    for p in prices.iter_mut() {
        let v = adjust_price(*p, cutoff);
        *p = v;
        total += v;
    }
    if total > 900 {
        total -= 35;
    }
    total
}

fn adjust_price(price: i64, cutoff: i64) -> i64 {
    if price > cutoff {
        cutoff + (price - cutoff) / 4
    } else if price < 10 {
        price * 2 - 7
    } else {
        price + 4
    }
}
"#;

const RS_HAND_INLINED: &str = r#"
fn apply_discounts_flat(prices: &mut Vec<i64>, cutoff: i64) -> i64 {
    let mut total = 0;
    for p in prices.iter_mut() {
        let v = if *p > cutoff {
            cutoff + (*p - cutoff) / 4
        } else if *p < 10 {
            *p * 2 - 7
        } else {
            *p + 4
        };
        *p = v;
        total += v;
    }
    if total > 900 {
        total -= 35;
    }
    total
}
"#;

#[test]
fn inline_variant_converges_with_hand_inlined_copy() {
    let report = scan_snippets("rs", &[RS_CALLER_AND_HELPER, RS_HAND_INLINED]);
    assert!(
        report.stats.inline_variants >= 1,
        "no inline variant produced: {:?}",
        report.stats
    );
    let groups = pair_groups(&report, "inline-assisted");
    assert!(
        !groups.is_empty(),
        "no inline-assisted group joining caller and hand-inlined copy: {:#?}",
        report.groups
    );
    // The report must show the inline chain (spec §5.4).
    let chain = groups[0]
        .inline_chain
        .as_ref()
        .expect("inline-assisted group missing its inline chain");
    assert!(
        chain.iter().any(|c| c.contains("adjust_price")),
        "chain does not name the expanded callee: {chain:?}"
    );
}

#[test]
fn wrapper_variant_is_tautological_and_suppressed() {
    // D3 minimum rule: an inlined variant never groups with the callee it
    // inlined. A pure wrapper's variant IS the callee body — drop it.
    let src = r#"
fn wrap(x: i64, y: i64) -> i64 {
    combine(x, y)
}

fn combine(a: i64, b: i64) -> i64 {
    let mut acc = a * 3 + b;
    if acc > 100 {
        acc = acc - a;
    }
    while acc > 7 {
        acc = acc / 2 + b % 5;
    }
    acc + 1
}
"#;
    let report = scan_snippets("rs", &[src]);
    assert_eq!(
        report.stats.inline_variants, 0,
        "pure-wrapper variant should be dropped as tautological"
    );
    assert!(
        !report
            .groups
            .iter()
            .any(|g| g.tier.to_string() == "inline-assisted"),
        "tautological wrapper produced a finding: {:#?}",
        report.groups
    );
}

#[test]
fn ambiguous_call_sites_are_skipped_and_counted() {
    let def = |n: u32| {
        format!("fn helper_x(a: i64) -> i64 {{\n    let q = a * {n} + {n};\n    q - {n}\n}}\n")
    };
    let caller = "fn caller(v: i64) -> i64 {\n    let mut out = 0;\n    for i in 0..9 {\n        out += helper_x(v + i);\n    }\n    out\n}\n";
    let defs: Vec<String> = (0..4).map(def).collect();
    let mut sources: Vec<&str> = defs.iter().map(String::as_str).collect();
    sources.push(caller);
    let report = scan_snippets("rs", &sources);
    assert!(
        report.stats.ambiguity_skips >= 1,
        "4 same-name+arity candidates must be an ambiguity skip: {:?}",
        report.stats
    );
    assert_eq!(report.stats.inline_variants, 0);
}

#[test]
fn self_recursive_calls_are_never_inlined() {
    let src = r#"
fn countdown(n: i64, acc: i64) -> i64 {
    if n <= 0 {
        return acc;
    }
    countdown(n - 1, acc + n)
}
"#;
    let report = scan_snippets("rs", &[src]);
    assert_eq!(report.stats.inline_variants, 0);
    assert_eq!(report.stats.scc_units, 0, "size-1 SCC must not be tagged");
}

#[test]
fn oversized_callee_is_not_inlined() {
    let mut big = String::from("fn big_helper(a: i64) -> i64 {\n    let mut acc = a;\n");
    for i in 0..60 {
        big.push_str(&format!("    acc += a * {i} + {i};\n"));
    }
    big.push_str("    acc\n}\n");
    let caller = "fn caller(v: i64) -> i64 {\n    let mut t = 0;\n    for i in 0..5 {\n        t += big_helper(v + i);\n    }\n    t\n}\n";
    let report = scan_snippets("rs", &[&big, caller]);
    assert_eq!(
        report.stats.inline_variants, 0,
        "callee over max_callee_tokens must not be inlined"
    );
}

#[test]
fn python_multistatement_helper_at_expression_site_is_skipped() {
    // D17: Python has no native expression-block; such sites are skipped
    // rather than spliced as a synthetic node nothing else can converge with.
    let src = r#"
def caller(xs):
    out = []
    for x in xs:
        y = messy_helper(x)
        out.append(y * 2)
    return out


def messy_helper(v):
    t = 0
    for k in range(4):
        t = t + v * k
    return t
"#;
    let report = scan_snippets("py", &[src]);
    assert_eq!(report.stats.inline_variants, 0);
}

#[test]
fn mutual_recursion_pair_is_tagged_as_scc() {
    let src = r#"
fn even_steps(n: u64, acc: u64) -> u64 {
    if n == 0 {
        return acc;
    }
    odd_steps(n - 1, acc + 2)
}

fn odd_steps(n: u64, acc: u64) -> u64 {
    if n == 0 {
        return acc + 1;
    }
    even_steps(n - 1, acc * 2)
}
"#;
    let report = scan_snippets("rs", &[src]);
    assert_eq!(report.stats.scc_units, 2, "{:?}", report.stats);
    assert!(
        report.stats.inline_variants >= 2,
        "SCC members should produce merged variants: {:?}",
        report.stats
    );
}

#[test]
fn scan_with_variants_is_deterministic() {
    // Spec §5.4: inlining decisions depend only on the definition table and
    // policy, never on iteration order. Path-free group signatures must match.
    let sig = |r: &ScanReport| -> Vec<String> {
        r.groups
            .iter()
            .map(|g| {
                let members: Vec<_> = g
                    .members
                    .iter()
                    .map(|m| (m.name.clone(), m.line_span))
                    .collect();
                format!(
                    "{}|{}|{members:?}|{:?}",
                    g.tier, g.token_count, g.inline_chain
                )
            })
            .collect()
    };
    let report_a = scan_snippets("rs", &[RS_CALLER_AND_HELPER, RS_HAND_INLINED]);
    let report_b = scan_snippets("rs", &[RS_CALLER_AND_HELPER, RS_HAND_INLINED]);
    assert_eq!(sig(&report_a), sig(&report_b));
}

// ---------- inliner on the IR path (spec §5.4, normalizer = "ir") ----------

#[test]
fn inline_variant_converges_with_hand_inlined_copy_ir() {
    // The §7.2 recall-parity gap: the IR path emitted ZERO inline-assisted findings.
    // The same caller/hand-inlined pair must now converge under `normalizer = "ir"`.
    let report = scan_snippets_ir("rs", &[RS_CALLER_AND_HELPER, RS_HAND_INLINED]);
    assert!(
        report.stats.inline_variants >= 1,
        "no IR inline variant produced: {:?}",
        report.stats
    );
    let groups = pair_groups(&report, "inline-assisted");
    assert!(
        !groups.is_empty(),
        "no IR inline-assisted group joining caller and hand-inlined copy: {:#?}",
        report.groups
    );
    let chain = groups[0]
        .inline_chain
        .as_ref()
        .expect("inline-assisted group missing its inline chain");
    assert!(
        chain.iter().any(|c| c.contains("adjust_price")),
        "chain does not name the expanded callee: {chain:?}"
    );
}

/// A multi-statement helper bound into an expression position (`let v = helper(x)`) —
/// the IR expression-block splice must reshape the callee's trailing `Return` into the
/// block's tail value so it converges with the hand-inlined block.
const RS_EXTRACT_HELPER: &str = r#"
fn score_all(xs: &[i64]) -> i64 {
    let mut total = 0;
    for x in xs {
        let v = weigh(*x);
        total += v;
    }
    total
}

fn weigh(x: i64) -> i64 {
    let base = x * 3;
    let adj = base + 7;
    adj - x
}
"#;

const RS_EXTRACT_INLINED: &str = r#"
fn score_all_flat(xs: &[i64]) -> i64 {
    let mut total = 0;
    for x in xs {
        let v = {
            let base = *x * 3;
            let adj = base + 7;
            adj - *x
        };
        total += v;
    }
    total
}
"#;

#[test]
fn multistatement_expression_helper_converges_ir() {
    let report = scan_snippets_ir("rs", &[RS_EXTRACT_HELPER, RS_EXTRACT_INLINED]);
    assert!(
        report.stats.inline_variants >= 1,
        "no IR inline variant for the extract-helper case: {:?}",
        report.stats
    );
    assert!(
        !pair_groups(&report, "inline-assisted").is_empty(),
        "multi-statement expression-block inline did not converge on the IR path: {:#?}",
        report.groups
    );
}

#[test]
fn wrapper_variant_is_tautological_and_suppressed_ir() {
    // D3 tautology / wrapper guard must fire on the IR path too: a pure wrapper's
    // inlined variant IS the callee body, so it produces no finding.
    let src = r#"
fn wrap(x: i64, y: i64) -> i64 {
    combine(x, y)
}

fn combine(a: i64, b: i64) -> i64 {
    let mut acc = a * 3 + b;
    if acc > 100 {
        acc = acc - a;
    }
    while acc > 7 {
        acc = acc / 2 + b % 5;
    }
    acc + 1
}
"#;
    let report = scan_snippets_ir("rs", &[src]);
    assert_eq!(
        report.stats.inline_variants, 0,
        "IR pure-wrapper variant should be dropped as tautological: {:?}",
        report.stats
    );
    assert!(
        !report
            .groups
            .iter()
            .any(|g| g.tier.to_string() == "inline-assisted"),
        "IR tautological wrapper produced a finding: {:#?}",
        report.groups
    );
}

#[test]
fn python_multistatement_helper_at_expression_site_is_skipped_ir() {
    // D17 holds on the IR path: Python has no expression block, so a multi-statement
    // helper at an expression site is skipped rather than spliced.
    let src = r#"
def caller(xs):
    out = []
    for x in xs:
        y = messy_helper(x)
        out.append(y * 2)
    return out


def messy_helper(v):
    t = 0
    for k in range(4):
        t = t + v * k
    return t
"#;
    let report = scan_snippets_ir("py", &[src]);
    assert_eq!(report.stats.inline_variants, 0, "{:?}", report.stats);
}

#[test]
fn mutual_recursion_pair_is_tagged_as_scc_ir() {
    let src = r#"
fn even_steps(n: u64, acc: u64) -> u64 {
    if n == 0 {
        return acc;
    }
    odd_steps(n - 1, acc + 2)
}

fn odd_steps(n: u64, acc: u64) -> u64 {
    if n == 0 {
        return acc + 1;
    }
    even_steps(n - 1, acc * 2)
}
"#;
    let report = scan_snippets_ir("rs", &[src]);
    assert_eq!(report.stats.scc_units, 2, "{:?}", report.stats);
    assert!(
        report.stats.inline_variants >= 2,
        "IR SCC members should produce merged variants: {:?}",
        report.stats
    );
}

#[test]
fn self_recursive_calls_are_never_inlined_ir() {
    let src = r#"
fn countdown(n: i64, acc: i64) -> i64 {
    if n <= 0 {
        return acc;
    }
    countdown(n - 1, acc + n)
}
"#;
    let report = scan_snippets_ir("rs", &[src]);
    assert_eq!(report.stats.inline_variants, 0, "{:?}", report.stats);
    assert_eq!(report.stats.scc_units, 0, "size-1 SCC must not be tagged");
}

// ---------- api-profile tier (spec §5.7) ----------

const RS_API_A: &str = r#"
fn sync_widgets(ids: &[u64]) -> u64 {
    let mut ok = 0;
    for id in ids {
        let w = open_widget(*id);
        if w > 10 {
            flush_widget(w);
            ok += 1;
        }
    }
    finalize_registry(ok)
}
"#;

const RS_API_B: &str = r#"
fn audit_widget_batch(batch: &[(u64, u64)], limit: u64) -> u64 {
    let mut score = 0;
    let mut penalties = 0;
    for pair in batch {
        let scaled = pair.0 * 3 + pair.1 % 97;
        let w = open_widget(scaled);
        if w > limit {
            flush_widget(w);
            score += w * 2 + penalties;
        } else {
            penalties += 3;
        }
    }
    finalize_registry(score + penalties * 5)
}
"#;

#[test]
fn api_profile_pairs_structurally_different_reimplementations() {
    let report = scan_snippets("rs", &[RS_API_A, RS_API_B]);
    assert_eq!(report.stats.api_signatures, 2, "{:?}", report.stats);
    // Must NOT be a structural match — that would invalidate the corpus.
    assert!(
        pair_groups(&report, "near-normalized").is_empty()
            && pair_groups(&report, "exact-normalized").is_empty(),
        "corpus is supposed to be structurally divergent"
    );
    assert_eq!(report.api_groups.len(), 1, "{:#?}", report.api_groups);
    let g = &report.api_groups[0];
    assert_eq!(g.tier.to_string(), "api-profile");
    assert_eq!(g.members.len(), 2);
    let evidence = g.template.as_ref().expect("api finding needs evidence");
    for name in ["open_widget", "flush_widget", "finalize_registry"] {
        assert!(
            evidence.contains(name),
            "evidence missing {name}: {evidence}"
        );
    }
    // Context annotations render as text.
    assert!(
        evidence.contains("loop"),
        "no context rendering: {evidence}"
    );
}

#[test]
fn api_profile_pairs_structurally_different_reimplementations_ir() {
    // Same corpus on the IR normalizer: `extract_calls` must read `Call`/`External`/
    // canonical control-context off the lowered tree and emit the same *kind* of
    // signature (rare-callee multiset + context). The `for`-loop's tail
    // `finalize_registry(..)` fuses into the loop's exit arm as a `Return`, so the
    // three rare callees still surface, both units still pair, and context renders.
    let report = scan_snippets_ir("rs", &[RS_API_A, RS_API_B]);
    assert_eq!(report.stats.api_signatures, 2, "{:?}", report.stats);
    assert!(
        pair_groups(&report, "near-normalized").is_empty()
            && pair_groups(&report, "exact-normalized").is_empty(),
        "corpus is supposed to be structurally divergent"
    );
    assert_eq!(report.api_groups.len(), 1, "{:#?}", report.api_groups);
    let g = &report.api_groups[0];
    assert_eq!(g.tier.to_string(), "api-profile");
    assert_eq!(g.members.len(), 2);
    let evidence = g.template.as_ref().expect("api finding needs evidence");
    for name in ["open_widget", "flush_widget", "finalize_registry"] {
        assert!(
            evidence.contains(name),
            "evidence missing {name}: {evidence}"
        );
    }
    assert!(
        evidence.contains("loop"),
        "no context rendering: {evidence}"
    );
}

#[test]
fn api_profile_requires_min_distinct_rare_callees_ir() {
    // Below `api_min_distinct_rare` (two distinct callees) → no signature on IR either.
    let a = r#"
fn poll_one(ids: &[u64]) -> u64 {
    let mut n = 0;
    for id in ids {
        let w = open_widget(*id);
        if w > 4 {
            n += w * 3;
        }
    }
    finalize_registry(n)
}
"#;
    let b = r#"
fn poll_two(ids: &[u64], cap: u64) -> u64 {
    let mut n = 7;
    for id in ids {
        let w = open_widget(id + 1);
        if w < cap {
            n += w + 2;
        } else {
            n -= 1;
        }
    }
    finalize_registry(n * 2)
}
"#;
    let report = scan_snippets_ir("rs", &[a, b]);
    assert_eq!(report.stats.api_signatures, 0);
    assert!(report.api_groups.is_empty(), "{:#?}", report.api_groups);
}

#[test]
fn api_profile_requires_min_distinct_rare_callees() {
    // Only two distinct callees → below api_min_distinct_rare (3) → no signature.
    let a = r#"
fn poll_one(ids: &[u64]) -> u64 {
    let mut n = 0;
    for id in ids {
        let w = open_widget(*id);
        if w > 4 {
            n += w * 3;
        }
    }
    finalize_registry(n)
}
"#;
    let b = r#"
fn poll_two(ids: &[u64], cap: u64) -> u64 {
    let mut n = 7;
    for id in ids {
        let w = open_widget(id + 1);
        if w < cap {
            n += w + 2;
        } else {
            n -= 1;
        }
    }
    finalize_registry(n * 2)
}
"#;
    let report = scan_snippets("rs", &[a, b]);
    assert_eq!(report.stats.api_signatures, 0);
    assert!(report.api_groups.is_empty(), "{:#?}", report.api_groups);
}

#[test]
fn api_profile_excludes_pairs_already_grouped_by_stronger_tiers() {
    // Near-identical pair sharing rare callees: near tier owns it.
    let a = r#"
fn ship_orders(orders: &[u64]) -> u64 {
    let mut done = 0;
    for o in orders {
        let t = load_manifest(*o);
        if t > 5 {
            seal_crate(t);
            done += 1;
        }
    }
    notify_dock(done)
}
"#;
    let b = r#"
fn ship_parcels(parcels: &[u64]) -> u64 {
    let mut count = 0;
    for p in parcels {
        let m = load_manifest(*p);
        if m > 9 {
            seal_crate(m);
            count += 1;
        }
    }
    notify_dock(count)
}
"#;
    let report = scan_snippets("rs", &[a, b]);
    assert!(
        !pair_groups(&report, "exact-normalized").is_empty()
            || !pair_groups(&report, "near-normalized").is_empty(),
        "expected a structural group: {:#?}",
        report.groups
    );
    assert!(
        report.api_groups.is_empty(),
        "api tier must exclude pairs co-grouped by stronger tiers: {:#?}",
        report.api_groups
    );
}

#[test]
fn api_profile_can_be_disabled() {
    let mut cfg = Config::default();
    cfg.api_profile.enabled = false;
    let report = scan_snippets_cfg("rs", &[RS_API_A, RS_API_B], &cfg);
    assert!(report.api_groups.is_empty());
    assert_eq!(report.stats.api_signatures, 0);
}

#[test]
fn inliner_can_be_disabled() {
    let mut cfg = Config::default();
    cfg.inline.enabled = false;
    let report = scan_snippets_cfg("rs", &[RS_CALLER_AND_HELPER, RS_HAND_INLINED], &cfg);
    assert_eq!(report.stats.inline_variants, 0);
    assert!(
        !report
            .groups
            .iter()
            .any(|g| g.tier.to_string() == "inline-assisted")
    );
}
