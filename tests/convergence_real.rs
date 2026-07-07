//! Real-corpus backing for two promoted IR rungs.
//!
//! CLAUDE.md's promotion gate requires that a construct lifted into the shared IR prove its
//! cross-language exact-equality invariants on REAL harvested code, not synthetic one-liners.
//! Two rungs in `convergence.rs` previously rested only on synthetic snippets:
//!   * the self-referential `@place` mutation (`i += 1` ≡ `i = i + 1`), `convergence.rs` ~:334;
//!   * the parallel multi-assign decomposition, `convergence.rs` ~:392.
//!
//! This suite re-asserts their promotion invariants over small self-contained functions harvested
//! from `benches/corpus` (ripgrep/serde = Rust, flask/click = Python, gin = Go). Each fixture
//! records its provenance (corpus file:line) inline. Matching is same-language (spec §3), so a
//! convergence invariant compares a REAL function to the same function with the one construct
//! respelled (hand-written twin), and a precision invariant keeps genuinely-different code apart.

use reprise::lang::Lang;

mod common;
use common::fp_ir;

// ============================================================================================
// Rung 1 — self-referential `@place` mutation: `x <op>= e` ≡ `x = x <op> e`
// (companion to convergence.rs::self_referential_assignment_converges_cross_language_ir ~:341)
// ============================================================================================

// ---- Python: click `_truncate_visible` — visible/i compound-increment counters ----
// Provenance: benches/corpus/click/src/click/_textwrap.py:11-35 (`visible += 1`, `i += 1`).
const PY_TRUNCATE_AUG: &str = r#"
def _truncate_visible(text, n):
    if n <= 0:
        return ""
    visible = 0
    i = 0
    cut = 0
    end = len(text)
    while i < end:
        m = _ansi_re.match(text, i)
        if m is not None:
            i = m.end()
            continue
        visible += 1
        i += 1
        cut = i
        if visible >= n:
            break
    return text[:cut]
"#;
// Hand-written twin: the two self-referential compound-assigns respelled explicitly.
const PY_TRUNCATE_EXPLICIT: &str = r#"
def _truncate_visible(text, n):
    if n <= 0:
        return ""
    visible = 0
    i = 0
    cut = 0
    end = len(text)
    while i < end:
        m = _ansi_re.match(text, i)
        if m is not None:
            i = m.end()
            continue
        visible = visible + 1
        i = i + 1
        cut = i
        if visible >= n:
            break
    return text[:cut]
"#;

#[test]
fn real_python_self_ref_counters_converge_ir() {
    // The real `visible += 1` / `i += 1` accumulators must fingerprint-equal their explicit
    // `visible = visible + 1` / `i = i + 1` respelling — the synthetic @place invariant, on
    // real click code.
    assert_eq!(
        fp_ir(PY_TRUNCATE_AUG, Lang::Python),
        fp_ir(PY_TRUNCATE_EXPLICIT, Lang::Python),
        "click _truncate_visible: `+= 1` counters must converge with explicit `= x + 1`",
    );
}

// ---- Rust: ripgrep `DecimalFormatter::new` — `i -= 1`, `n /= 10` ----
// Provenance: benches/corpus/ripgrep/crates/printer/src/util.rs:425-439 (self-contained; the
// `Self::MAX_U64_LEN` const is inlined to 20 so the fn stands alone).
const RS_DECIMAL_AUG: &str = r#"
fn new(mut n: u64) -> [u8; 20] {
    let mut buf = [0; 20];
    let mut i = buf.len();
    loop {
        i -= 1;
        let digit = u8::try_from(n % 10).unwrap();
        n /= 10;
        buf[i] = b'0' + digit;
        if n == 0 {
            break;
        }
    }
    buf
}
"#;
const RS_DECIMAL_EXPLICIT: &str = r#"
fn new(mut n: u64) -> [u8; 20] {
    let mut buf = [0; 20];
    let mut i = buf.len();
    loop {
        i = i - 1;
        let digit = u8::try_from(n % 10).unwrap();
        n = n / 10;
        buf[i] = b'0' + digit;
        if n == 0 {
            break;
        }
    }
    buf
}
"#;

// ---- Rust: ripgrep `find_cap_ref` — `i += 1`, `cap_end += 1` ----
// Provenance: benches/corpus/ripgrep/crates/matcher/src/interpolate.rs:97-134.
const RS_CAPREF_AUG: &str = r#"
fn find_cap_ref(replacement: &[u8]) -> Option<CaptureRef<'_>> {
    let mut i = 0;
    if replacement.len() <= 1 || replacement[0] != b'$' {
        return None;
    }
    let mut brace = false;
    i += 1;
    if replacement[i] == b'{' {
        brace = true;
        i += 1;
    }
    let mut cap_end = i;
    while replacement.get(cap_end).map_or(false, is_valid_cap_letter) {
        cap_end += 1;
    }
    if cap_end == i {
        return None;
    }
    let cap = std::str::from_utf8(&replacement[i..cap_end]).expect("valid UTF-8 capture name");
    Some(CaptureRef { cap, end: cap_end })
}
"#;
const RS_CAPREF_EXPLICIT: &str = r#"
fn find_cap_ref(replacement: &[u8]) -> Option<CaptureRef<'_>> {
    let mut i = 0;
    if replacement.len() <= 1 || replacement[0] != b'$' {
        return None;
    }
    let mut brace = false;
    i = i + 1;
    if replacement[i] == b'{' {
        brace = true;
        i = i + 1;
    }
    let mut cap_end = i;
    while replacement.get(cap_end).map_or(false, is_valid_cap_letter) {
        cap_end = cap_end + 1;
    }
    if cap_end == i {
        return None;
    }
    let cap = std::str::from_utf8(&replacement[i..cap_end]).expect("valid UTF-8 capture name");
    Some(CaptureRef { cap, end: cap_end })
}
"#;

#[test]
fn real_rust_self_ref_mutations_converge_ir() {
    // `-=` / `/=` (DecimalFormatter) and `+=` (find_cap_ref) must each converge with their
    // explicit self-referential respelling.
    assert_eq!(
        fp_ir(RS_DECIMAL_AUG, Lang::Rust),
        fp_ir(RS_DECIMAL_EXPLICIT, Lang::Rust),
        "DecimalFormatter::new: `i -= 1` / `n /= 10` must converge with explicit forms",
    );
    assert_eq!(
        fp_ir(RS_CAPREF_AUG, Lang::Rust),
        fp_ir(RS_CAPREF_EXPLICIT, Lang::Rust),
        "find_cap_ref: `i += 1` / `cap_end += 1` must converge with explicit `= x + 1`",
    );
}

// ---- Go: gin `iterate` accumulate — local var, single-term RHS (`path += root.path`) ----
// Provenance: benches/corpus/gin/gin.go:397-398 (the `iterate` path accumulator).
const GO_ACCUM_AUG: &str = "package main\nfunc iterate(path string, root *node) string {\n\tpath += root.path\n\treturn path\n}\n";
const GO_ACCUM_EXPLICIT: &str = "package main\nfunc iterate(path string, root *node) string {\n\tpath = path + root.path\n\treturn path\n}\n";

// ---- Go: gin `responseWriter.Write` — member-target self-ref (`w.size += n`) ----
// Provenance: benches/corpus/gin/response_writer.go:84-89.
const GO_FIELD_AUG: &str = "package main\nfunc (w *responseWriter) Write(data []byte) (n int, err error) {\n\tw.WriteHeaderNow()\n\tn, err = w.ResponseWriter.Write(data)\n\tw.size += n\n\treturn\n}\n";
const GO_FIELD_EXPLICIT: &str = "package main\nfunc (w *responseWriter) Write(data []byte) (n int, err error) {\n\tw.WriteHeaderNow()\n\tn, err = w.ResponseWriter.Write(data)\n\tw.size = w.size + n\n\treturn\n}\n";

#[test]
fn real_go_self_ref_local_accumulator_converges_ir() {
    assert_eq!(
        fp_ir(GO_ACCUM_AUG, Lang::Go),
        fp_ir(GO_ACCUM_EXPLICIT, Lang::Go),
        "gin iterate: local `path += root.path` must converge with explicit `path = path + ...`",
    );
}

#[test]
fn real_go_self_ref_member_target_converges_ir() {
    // A member-access target (`w.size += n`) is still a self-referential mutation: `w.size`
    // appears in its own value, so both spellings must take @place and converge.
    assert_eq!(
        fp_ir(GO_FIELD_AUG, Lang::Go),
        fp_ir(GO_FIELD_EXPLICIT, Lang::Go),
        "gin responseWriter.Write: `w.size += n` must converge with `w.size = w.size + n`",
    );
}

// ---- Precision: an accumulator (`x += e`, @place) must NOT collapse with a plain
// per-iteration re-assign (`x = e`, @target) — the self-referential read is load-bearing.
// (companion to convergence.rs::fresh_binding_stays_distinct_from_mutation_ir ~:379)
const GO_REASSIGN: &str = "package main\nfunc iterate(path string, root *node) string {\n\tpath = root.path\n\treturn path\n}\n";

#[test]
fn real_self_ref_accumulator_stays_distinct_from_plain_reassign_ir() {
    assert_ne!(
        fp_ir(GO_ACCUM_AUG, Lang::Go),
        fp_ir(GO_REASSIGN, Lang::Go),
        "an accumulator `path += root.path` must not collide with a fresh re-assign `path = root.path`",
    );
}

// ============================================================================================
// Rung 2 — parallel multi-assign decomposition — REAL-CODE GAP (finding, see suite report)
//
// The decomposition (src/ir/pass.rs::decompose_parallel_assign) fires ONLY on a canonical
// multi-target `Assign`: N≥2 simple distinct target vars with N SEPARATE values, produced by
// Python `a, b = X, Y`, Go `a, b = X, Y`, or Rust REASSIGNMENT `(a, b) = (X, Y)`. Grepping the
// whole corpus (gin, flask, click, ripgrep, serde, incl. tests) finds ZERO of these decomposing
// forms: no swaps `a, b = b, a`, no N=N parallel (re)assignments in ANY language, no gcd/tail-rec.
//
// Every real multi-target assignment in the corpus is instead a form the rung DELIBERATELY leaves
// intact:
//   * tuple-unpack-from-a-single-call `a, b = f()` / `a, b := f()` / `let (a, b) = f()` — a count
//     mismatch (N targets, 1 value): ANF territory, explicitly out of scope (pass.rs:1466); and
//   * a Rust destructuring `let (a, b) = (x, y)` BINDING — lowered by `lower_let_rust`
//     (src/frontend/rust.rs:318) to a SINGLE tuple-pattern `Assign`, not a multi-target one, so it
//     does NOT decompose.
//
// So the rung's synthetic convergence invariants (convergence.rs ~:400-450) CANNOT be reproduced
// on real corpus code. The closest real form is the Rust independent tuple-`let` below; it is a
// destructuring bind, distinct from the decomposing REASSIGNMENT the synthetic test uses, and it
// does NOT converge with the adjacent single `let`s. The aspirational convergence assertion is
// kept verbatim but `#[ignore]`d as a documented gap (per the TDD discipline: do not weaken a
// real-code assertion to force it green). A future frontend change that decomposes tuple-`let`s
// would flip this test — at which point drop the `#[ignore]`.
// ============================================================================================

// ---- Rust: ripgrep independent tuple-let with per-slot arithmetic ----
// Provenance: benches/corpus/ripgrep/crates/printer/src/standard.rs:722
//   `let (s, e) = (m.start() - range.start, m.end() - range.start);`
const RS_TUPLE_PARALLEL: &str = r#"
fn record(m: &Match, range: &Range, matches: &mut Vec<Match>) {
    let (s, e) = (m.start() - range.start, m.end() - range.start);
    matches.push(Match::new(s, e));
}
"#;
const RS_TUPLE_ADJACENT: &str = r#"
fn record(m: &Match, range: &Range, matches: &mut Vec<Match>) {
    let s = m.start() - range.start;
    let e = m.end() - range.start;
    matches.push(Match::new(s, e));
}
"#;

// ---- Rust: ripgrep independent tuple-let, method-call values ----
// Provenance: benches/corpus/ripgrep/crates/ignore/src/types.rs:421
//   `let (key, glob) = (name.to_string(), glob.to_string());`
const RS_TUPLE2_PARALLEL: &str = r#"
fn add(name: &str, glob: &str) -> String {
    let (key, glob) = (name.to_string(), glob.to_string());
    format!("{key}:{glob}")
}
"#;
const RS_TUPLE2_ADJACENT: &str = r#"
fn add(name: &str, glob: &str) -> String {
    let key = name.to_string();
    let glob = glob.to_string();
    format!("{key}:{glob}")
}
"#;

#[test]
#[ignore = "FINDING: corpus has no decomposing multi-assign; the real Rust `let (a,b)=(x,y)` is a \
            destructuring BIND, not the tuple REASSIGNMENT the rung decomposes — so it does NOT \
            converge with the adjacent `let`s. Aspirational assertion kept, parked as a gap."]
fn real_rust_independent_tuple_let_decomposes_to_adjacent_lets_ir() {
    // Would hold IF a tuple-`let` binding decomposed like the tuple REASSIGNMENT the synthetic
    // `independent_multi_assign_converges_with_two_adjacent_assigns_ir` covers. It currently does
    // not (see the module header): both assertions FAIL as of this writing.
    assert_eq!(
        fp_ir(RS_TUPLE_PARALLEL, Lang::Rust),
        fp_ir(RS_TUPLE_ADJACENT, Lang::Rust),
        "ripgrep `let (s, e) = (..., ...)` must equal the two adjacent `let`s",
    );
    assert_eq!(
        fp_ir(RS_TUPLE2_PARALLEL, Lang::Rust),
        fp_ir(RS_TUPLE2_ADJACENT, Lang::Rust),
        "ripgrep `let (key, glob) = (..., ...)` must equal the two adjacent `let`s",
    );
}

// ---- Precision at the decomposed-sequence level (the form decomposition WOULD emit): two single
// `let`s that read different sources must stay distinct. This DOES hold on real-derived code and
// mirrors convergence.rs::independent_decomposition_does_not_collide_with_unrelated_pair_ir ~:465.
const RS_TUPLE_UNRELATED: &str = r#"
fn record(m: &Match, range: &Range, matches: &mut Vec<Match>) {
    let s = m.end() - range.start;
    let e = m.start() - range.start;
    matches.push(Match::new(s, e));
}
"#;

#[test]
fn real_adjacent_lets_with_distinct_reads_stay_distinct_ir() {
    // `s = start; e = end` must NOT collide with the swapped-source `s = end; e = start`.
    assert_ne!(
        fp_ir(RS_TUPLE_ADJACENT, Lang::Rust),
        fp_ir(RS_TUPLE_UNRELATED, Lang::Rust),
        "two single `let`s reading different sources must stay distinct",
    );
}
