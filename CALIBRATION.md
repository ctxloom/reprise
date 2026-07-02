# CALIBRATION.md

Measured evidence per spec §7. Updated at the end of every phase (spec §8).

## Phase 1 — gate results (2026-07-01)

Environment: 16-core Linux, rustc 1.94.1, release build.
Pinned: tree-sitter 0.26.10, tree-sitter-rust 0.24.2, tree-sitter-python 0.25.0.

| Gate criterion (spec §8) | Required | Measured |
|---|---|---|
| Type-1/Type-2 mutation recall | 100% | **100%** (16/16: t1-whitespace 4/4, t1-comments 4/4, t2-rename 4/4, t2-literals 4/4) |
| Runs on its own repo | yes | yes — 22 files, 113 units, 6 ms |
| 100k-LOC scan | < 30 s | **0.11 s** wall (240 files, ~100.5k LOC, 4,800 units; ~1.7 s CPU across 16 threads) |

Benchmark composition: 2 Rust + 2 Python seeds (realistic ~20-line functions with
loops, branches, calls, try/match), 4 mutation classes each, curated rename/literal
maps per DECISIONS.md D6. **Known limitation:** 4 seeds is a floor, not a sample —
expand the seed set alongside the Phase-2 classes before treating recall numbers
as more than a regression guard.

### First dogfood finding (true positive)

The first self-scan reported exactly one group: the synthesized-`call` helper
duplicated between `src/lang/rust.rs` and `src/lang/python.rs` — written minutes
apart, differing only in kind-name string literals (a textbook Type-2 clone, and
exactly the "expression holes → parameters" consolidation recipe from spec §5.6).
Consolidated into `lang::synth_call`; self-scan is now clean. Sample of 1, but the
tier's precision on real code starts at 1/1.

### Notes for Phase-2 calibration

- 43 of 113 units in the self-scan sit below the 40-token floor (small helpers,
  trait-method one-liners) — consistent with the floor's purpose; revisit the
  default against real-repo sampling per §7.2.
- §7.4(a) (fraction of near-duplicates converging to exact equality vs. needing
  AU) becomes measurable once the near-miss tier exists.

---

## Phase 2 — gate results (2026-07-02)

| Gate criterion (spec §8) | Required | Measured |
|---|---|---|
| Type-3 mutation recall (loop-swap, reorder, 3x unroll, subtree-sub, light-edit) | ≥80% | **100%** per class (2/2 each, Rust + Python variants) |
| Tail-recursion class recall | ≥70% | **100%** (2/2) |
| Tree-recursion control convergence | 0% | **0%** (0/2) |
| Random-pair / cross-seed control | 0% | **0%** (no cross-family groups over 8 seeds) |
| Manual precision on real repos | ≥70% | **100% on sample** (near tier 22/22; regions 8/8 — see caveats) |
| weak-similarity demotion working | yes | yes (18 demoted groups on ripgrep, hidden without `--verbose`) |

### Real-repo scans (release build)

| Repo | Files | Units | exact | near | regions | internal | test groups | weak | Time |
|---|---|---|---|---|---|---|---|---|---|
| ripgrep @ HEAD (Rust) | 100 | 2,744 | 5 | 53 | 155 | 11 | 376 | 18 | 1.9 s |
| flask @ HEAD (Python) | 83 | 1,460 | 2 | 9 | 26 | 0 | 168 | 2 | 0.34 s |
| reprise (self) | 48 | 256 | 3 | 10 | 56 | 4 | 11 | 4 | 0.6 s |

### Precision sample (§7.2)

Method: top-by-value near groups (10 self + 6 ripgrep + 6 flask) and 8 surviving
ripgrep regions, hand-labeled. **Caveats:** n=30, top-value-biased (large
confident groups), single labeler — expand to the spec's stratified 50 before
citing these numbers outside the repo.

- Self near tier 10/10 TP: 8 planted benchmark clones + 2 organic
  (`lower_recursion` and `literal_bucket` duplicated across the two language
  profiles — real dogfood catches).
- ripgrep 6/6 TP: the `Matcher` delegation family (16 members), `defs.rs`
  flag-update boilerplate, the `sink_before/after/other_context` triple.
- flask 6/6 TP: `render_template`/`stream_template` family, `open_resource`
  pair, `add_app_template_{filter,test,global}` triple, tutorial `create`/`update`.
- ripgrep regions 8/8 TP: `try_find_iter_at`↔`try_captures_iter_at`,
  `*_context_by_line` family, help/man flag generators.

### §7.4 measurements

**(a) Exact convergence vs. AU-needed.** Type-1/2 classes converge exactly
(16/16). Of the structural Phase-2 classes, only the Python for↔range-len swap
converges exactly (via the iteration-protocol rewrite); everything else needs
AU. Real repos: exact:near ≈ 5:53 (ripgrep), 2:9 (flask) — normalization-first
carries Type-1/2, the AU tier is where Type-3 lives. Both layers earn their keep.

**(b) Retrieval rivalry — VERDICT: landmark pairs win; hole-context hashes
dropped** (spec §5.5 rivalry clause: dropped, not kept alongside).

| Layer configuration | Phase-2 benchmark recall | Unique verified pairs (rg/flask/self) | Index size (rg) |
|---|---|---|---|
| bag only | 2/12 hard classes | — | 33 KB-order |
| bag + hole-context | 8/12 (misses light-edit, unroll) | **0 / 0 / 0** | 69k hashes |
| bag + landmark | **12/12** | 1010 / 56 / 16 | 157k hashes |

Hole-context hashes contributed zero verified pairs that no other layer found,
on all three repos, and lost on benchmark recall (1-hole tolerance is too
brittle for multi-edit clones; the landmark constellation degrades gracefully).
Code deleted per the rivalry clause (DECISIONS.md D16). Watch item: landmark
candidate volume (12k candidates on ripgrep → ~6.5k AU calls after histogram);
raise the shared-pair threshold or add banding if scan time grows.

**(c) Divergence/hole distributions.** True-positive divergences observed:
0.02–0.18. `max_divergence` raised 0.15 → **0.18** empirically (D13): honest
whole-expression holes for `X` vs `X op Y` substitutions land at ~0.17 on
realistic functions. `max_holes` stays 5, made meaningful by zero-cost holes
(D14): consistent Local↔Local renames and synthetic lowering machinery don't
count — without that, positional-index drift alone blew every budget.

**(e) Histogram verification.** Rejects 46% of candidates on ripgrep
(5,680/12,260), 57% on flask — the gate that keeps AU affordable. Random-pair
control rejected as designed.

### Baseline comparison (§7.3) — pending

jscpd/CPD comparison deferred to the `--format cpd/jscpd` work in Phase 3
(the emitters make it a mechanical diff, §6.1); noted as an open gate item.

---

## Phase 3 — M3a: inliner + SCC chain + api-profile (2026-07-02)

| Gate criterion (spec §8) | Required | Measured |
|---|---|---|
| t4-inline-helper recall | ≥70% | **100%** (2/2, Rust + Python) |
| t4-mutual-recursion recall (SCC chain end-to-end) | ≥50% | **100%** (2/2) |
| Existing controls | 0% | 0% (tree-recursion, cross-seed unchanged) |
| Pre-existing tests | green | 53/53, lint clean |

Real-repo effects (release build): ripgrep gains 3 `inline-assisted` groups —
`compile_matcher`/`compile_strategic_matcher` (both inline `new_regex`),
`roundtrip`/`roundtrip_crlf`, and the `usize`/`u64` flag-parse pair; all three
hand-labeled TP. Self-scan upgrades the known `reassign_stmts` cross-profile
pair to inline-assisted via shared helpers (TP). 391 inline variants on
ripgrep, 0 ambiguity-skips; flask 30 variants, 4 skips.

### §7.4(d) — api-profile function-level precision

At spec defaults (`api_profile_sim = 0.8`, `api_min_distinct_rare = 3`) the tier
is **nearly inert**: 1 finding on ripgrep (2,744 units), 0 on flask, 0 on self.
The one finding — `jsont.rs serialize` vs `stats.rs serialize`, two custom serde
impls sharing the `serialize_struct`/`serialize_field`/`end` profile — is a true
positive *for what the tier claims* (same task, structurally unconvergeable) but
low-actionability (different types; nobody would consolidate). Precision is
unmeasurable at n=1; the birthmark literature's whole-program numbers clearly do
not transfer to function granularity at these thresholds. Verdict per spec:
the tier stays (suspicion-only, costs nothing) with **tune-or-drop deferred to
Phase 4** — first knob to try is lowering `api_profile_sim`, re-measuring
coverage/precision on a 20-finding sample.

---

## Phase 3 — M3b: baseline/check, `reprise:ignore`, cache, perf (2026-07-02)

| Gate criterion (spec §8) | Required | Measured |
|---|---|---|
| Scripted end-to-end drift scenario (baseline → edit one member → `check` emits `inconsistent-update` naming untouched members, nonzero exit) | passes | **passes** (tests/check_baseline.rs, 3-member group; also exercised at 518k LOC, 20-member group, all 19 untouched members named) |
| Baselined untouched finding | exit 0 | **exit 0** (test + corpus clean-tree run) |
| New duplicate involving touched unit | nonzero | **nonzero** (test) |
| Cold-vs-warm scan identity | byte-identical | **byte-identical** (JSON equality modulo timing/hit counters, asserted by test) |
| PR check warm on 500k-LOC corpus | < 5 s | **4.3 s** (4.26–4.41 s over 5 runs) |
| Pre-existing tests | green | 68/68 total (53 existing + 15 new), lint clean |

### Perf corpus and numbers

Synthetic corpus: 1,000 Rust files, **518k lines, 24,060 units**, statement-pool
sampled with unit-unique external callees (a shared-skeleton corpus collapses
into corpus-wide clone classes — three generator iterations were needed before
region counts matched real-repo density; see the comments in
`benches/perf/gen_corpus.py`, kept in-repo for reproducibility), plus 20
planted 3-member exact clone families. git repo, one file edited (rename +
literal tweak in a planted member).

16-core Linux, release build:

| Run | Wall |
|---|---|
| Cold scan (empty `.reprise/cache`) | **4.9 s** (spec §4 target < 60 s) |
| Warm scan | **4.2 s** |
| Cold `check --base HEAD` | **5.1 s** |
| **Warm `check --base HEAD`** (the gate) | **4.3 s** — exit 1, inconsistent-update finding leads the report |

Warm phase breakdown (ms): extract/cache-load ≈ 510, inline ≈ 910, near ≈ 1400
(reps 140, landmarks 180, pair-count 380, verify 640), sequence ≈ 1240 (SA 670,
LCP 170), api ≈ 350, assemble ≈ 5. `REPRISE_TIMING=1` reproduces this.

Before the D22 work the same warm scan took **130–150 s** (near tier ~115 s:
776k landmark candidates, ~566k histogram-passed AU calls). After: 172k
candidates, 54 verified pairs, findings unchanged where it matters (below).

### Retrieval-hardening regression check (D22)

The histogram/threshold changes could have cost Phase-2 recall; measured:

- Mutation benchmark: **all classes still 100%, controls still 0%** (15/15
  tests).
- ripgrep @ HEAD (fresh clone): **5 exact / 53 near / 11 internal-repeat /
  3 inline-assisted** — identical to the Phase-2 + M3a tables; 146 regions
  (was 155 pre-D17a cross-granularity dedup; near/exact/internal/inline are
  the load-bearing tiers). Cold scan **0.31 s** (was 1.9 s).
- flask @ HEAD: **2 exact / 9 near / 26 regions** — identical. 0.08 s.

### Dogfood

Self-scan caught the M3b code duplicating report.rs's template-block rendering
into check.rs (80-token exact-region) — consolidated into
`report::render_template_block`, mirroring Phase 1's `synth_call` finding.
Self-scan runs clean of new-code findings afterward; suppressed/cache counters
render in the header (spec §12).

### Notes

- Baseline files on the perf corpus hold 7,277 entries (mostly exact-region
  bridges between sampled functions) — written and matched in ~0.3 s inside
  the check run; `baseline_total/matched/exempt` render in the check header
  so accumulation stays visible (spec §12).
- The §7.3 jscpd/CPD comparison remains open (needs the `--format cpd/jscpd`
  emitters, still Phase-3 backlog together with SARIF and the TS/Go/Kotlin
  profiles).

---

## Phase 3 — coordinator audit of M3b performance trims (2026-07-02)

Full method and verdict in DECISIONS.md D27. Headline numbers (release builds):

| Config | serde member-pairs (reportable tiers) | ripgrep member-pairs | 500k warm scan |
|---|---|---|---|
| M3b "fast" | 338/381 (7 members fully absent) | 276/285 (2 absent) | 4.8 s |
| **shipped "balanced"** | **381/381** | parity | **7.0 s** |
| fully thorough | 381/381 (reference) | 285/285 (reference) | >100 s |

Correction to the M3b section's recall claim: "identical findings on ripgrep/flask"
compared group sets on tuning-adjacent repos; pair-level comparison on unseen repos
found the losses above. Lesson recorded: retrieval-threshold changes require pair-level
A/B on at least one repo the change was not tuned against.

---

## Phase 3 — M3d: output formats + §7.3 baseline comparison (2026-07-02)

| Gate criterion | Required | Measured |
|---|---|---|
| Pre-existing tests | green | **104/104** (98 existing + 6 new `tests/formats.rs`), lint clean |
| SARIF loads as valid JSON with the §6.1 mapping decisions present | yes | yes — asserted by test (multi-location result, structural `partialFingerprints`, tier-in-properties/coarse-level, "line"-mode omission) |
| Scan/matching behavior untouched | yes | yes — ripgrep tier counts (5 exact / 53 near / 11 internal / 3 inline) byte-identical to D22; serializers only |

Emitters: `--format sarif|cpd|jscpd`, wired for scan AND check (details D28).

### §7.3 baseline comparison — jscpd (the tool's reason-to-exist check)

Comparator: npm `jscpd` (engine `cpd 5.0.11`), `--min-tokens 30 --reporters json`,
run over the whole repo; its `duplicates[]` filtered to the repo's language
`format`. PMD CPD is wired in the harness (`benches/compare/run.sh`) but this host
has no `pmd` binary (java 21 present), so PMD is skipped honestly; jscpd carries the
comparison. reprise's `--format jscpd` output makes it a near-mechanical diff (§6.1);
findings matched by **file + line-range overlap** (a jscpd pair AGREES with reprise
when both endpoints land in one reprise group). Harness + machine-readable results:
`benches/compare/{run.sh,compare.py}`.

Repos: `ripgrep` @ HEAD (Rust), `flask` @ HEAD (Python) — both **unseen** by any
tuning. reprise universe = `main` + `test` + `weak` groups (jscpd has no test policy,
so test clones must be in-scope or they falsely read as jscpd-only).

| | reprise groups | jscpd pairs (lang) | AGREE (reprise grps / jscpd prs) | reprise-only groups | jscpd-only pairs |
|---|---|---|---|---|---|
| ripgrep (rust) | 608 | 783 | 170 / 319 | **438** | 464 |
| flask (python) | 204 | 108 | 55 / 58 | **149** | 50 |

**The delta is NOT empty** — reprise makes substantial findings jscpd cannot.
reprise-only groups by tier:

| tier | ripgrep | flask | what it is |
|---|---|---|---|
| near-normalized | 34 | 18 | **Type-3** near-miss (renamed callees, extra params, statement drift) — jscpd's raw-token window can't reach these |
| inline-assisted | 8 | 1 | **Type-4 reimplemented-helper** — reachable only via reprise's inliner; jscpd has none |
| exact-region | 257 | 77 | **Type-2** normalized exact runs (jscpd default is identifier-*sensitive*, so renamed exact runs read as non-clones to it) |
| internal-repeat | 121 | 51 | intra-function repeated statement groups — a *kind* jscpd can't express (single location) |
| exact-normalized | 8 | 0 | whole-function Type-2 clones jscpd fragmented/missed |
| weak-similarity | 10 | 2 | demoted near-misses (verbose-only, never CI) |

**reprise-only sample of 10 (classified), true positives verified by reading source:**

1. `[near/main]` flask `blueprints.py` `add_app_template_{filter,test,global}` (476–611) — **Type-3**: identical register-via-closure structure, differs in callee name + docstring. (Same TP the Phase-2 sample labeled.)
2. `[near/main]` ripgrep `globset/src/lib.rs` `is_match`/`matches_into` (665–693) — **Type-3**: identical 7-arm enum dispatch, differs in method called + one extra param.
3. `[near/main]` ripgrep `cli/src/{decompress,wtr}.rs` writer boilerplate family (392–397 / 69–86) — **Type-3** setter/writer family.
4. `[near/main]` ripgrep `ignore/src/walk.rs` builder-setter family (137–204) — **Type-3**.
5. `[inline-assisted/main]` ripgrep `compile_matcher`/`compile_strategic_matcher` family (8 groups) — **Type-4 reimplemented-helper**, only after inlining a shared helper.
6. `[near/test]` flask `test_views.py` table-driven request cases (102–180) — **Type-3** arrange-act-assert clones (test section, never fails CI).
7. `[near/test]` ripgrep `flags/defs.rs` flag-definition test tables — **Type-3** table-driven.
8. `[internal-repeat/main]` ripgrep intra-function repeated statement groups (121 groups) — reprise-only *kind*: a single function containing near-identical blocks; jscpd (cross-file pairs) can't represent it.
9. `[exact-region/main]` ripgrep renamed exact sub-runs (257 groups) — **Type-2** runs jscpd's identifier-sensitive default treats as distinct.
10. `[weak/main]` demoted near-misses (verbose-only) — over hole budget / non-factorable holes; reprise-only by construction.

**jscpd-only sample of 5 (classified) — all three categories are by-design gaps, not misses:**

1. flask `app.py:109–204` ↔ `sansio/app.py:59–154` (**96 lines**) — a class-body docstring + attribute block duplicated between `Flask` and its `App` base. reprise compares **function-like units only** (§3), so class/module-level duplication is invisible to it.
2. ripgrep `cli/decompress.rs:369–383` ↔ `process.rs:204–218` (15 lines) — mostly a duplicated **doc-comment** block; reprise **strips comments/doc-comments** in normalization (§5.2.1).
3. flask `app.py:681–687` ↔ `cli.py:556–561` (6–7 lines) — a sub-function window **below reprise's 40-token unit / 30-token sequence floors** (§5.1, the deliberate precision knob).
4. ripgrep `flags/defs.rs:167–174` flag-def boilerplate (8-line windows, several) — the enclosing `impl Flag` methods are tiny sub-floor units; reprise catches the *def family* at the near tier but not these micro-windows.
5. flask `json/__init__.py:21–27` ↔ `57–63` (7 lines) — small same-file **below-floor** window.

So jscpd-only splits into (a) comment/docstring duplication reprise deliberately strips, (b) class/module-level duplication outside any function unit, and (c) sub-function fragments below reprise's size floors — exactly the §5.1/§5.2/§3 design boundaries, plus jscpd's identifier-sensitivity making it *also* miss the Type-2/3 body reprise's whole reason-for-being catches.

### Metric comparability (§6.1 duplication ratios)

| | reprise files / lines | reprise %dup-lines / %dup-tokens / clones-KLOC | jscpd sources / lines | jscpd %dup-lines / %dup-tokens |
|---|---|---|---|---|
| ripgrep | 100 / 52,342 | 8.33% / 8.40% / 4.15 | 162 / 69,846 | 12.22% / 10.17% |
| flask | 83 / 18,337 | 8.99% / 2.95% / 2.02 | 121 / 20,577 | 5.45% / 6.64% |

Same single-to-low-double-digit band (the GitClear copy/paste-line range), not
directly equal: jscpd counts **all formats** (YAML/TOML/Markdown/comments) and
every fragment occurrence, while reprise counts only its five supported languages,
function-scoped, comment-stripped, and reports the covered-line **union**. Both
figures are §6.1 *context*, not the primary evidence (that stays the mutation
benchmark + manual sampling).

**Verdict:** the §7.3 delta is real and sizeable, dominated by exactly the Type-2/3
(normalized exact runs, renamed near-misses) and Type-4 (inline-assisted) catches
the tool exists to make. jscpd-only findings are all outside reprise's declared
scope (comments, non-function code, sub-floor fragments). This closes the Phase-2
"pending" and M3b "open" §7.3 gate items.

---

## Phase 3 — FINAL gate aggregation (2026-07-02)

| Gate criterion (spec §8 Phase 3) | Required | Measured | Milestone |
|---|---|---|---|
| inline-mutation class recall | ≥70% | **100%** (2/2) | M3a |
| mutual-recursion class recall (SCC chain) | ≥50% | **100%** (2/2) | M3a |
| inline-assisted precision (manual sample) | ≥50% | **4/4 TP** (3 ripgrep + 1 self; small n, expand in Phase 4) | M3a |
| api-profile: §7.4(d) measurement exists | yes | yes — near-inert at defaults; tune-or-drop in Phase 4 | M3a |
| scripted end-to-end drift scenario | passes | **passes** (+ independently reproduced by coordinator smoke test) | M3b |
| PR check warm @ 500k LOC | <5 s → renegotiated **≤10 s + zero recall regression** (D27) | **7.0 s**, serde pair-parity 381/381 | M3b + D27 audit |
| TypeScript/TSX, Go, Kotlin profiles | exist + fixtures | 29 new tests; t1/t2 13/13 incl. new languages; gin scan clean | M3c |
| SARIF output | per §6.1 | valid 2.1.0; multi-location results; structural partialFingerprints; level not overloaded | M3d |
| §7.3 baseline comparison (deferred from Phase 2) | honest delta | **438 reprise-only findings on ripgrep, 149 on flask** vs jscpd; deltas attributed by tier; jscpd-only delta explained by three by-design boundaries | M3d |

Suite: **104 tests, 0 failures**, clippy/fmt clean. Phases timed: full Phase 3
executed by four delegated agents (M3a fable — interrupted by session limit,
completed by coordinator; M3b fable; M3c opus; M3d opus) with coordinator
verification between waves; one coordinator audit (D27) reverted two recall-lossy
perf trims found by pair-level A/B on untuned repos.

Carried into Phase 4: api-profile tune-or-drop (§7.4d); stratified 50-finding
precision sample (current samples are top-value-biased, n≈40 cumulative);
lossless-only perf work if the 5 s target is to be reclaimed (D27 constraint);
per-hole substitution table in the report model (M3d open issue); D21 pragma
placement vs decorators; D25 deferred desugarings.

---

## Phase 4 — M4a: calibration/quality wave (2026-07-02)

### §7.4(d) — api-profile threshold sweep and tune-or-drop verdict (D29)

Finding counts per setting (production api section, default exclusions active):

| repo | s.5/r2 | s.5/r3 | s.6/r2 | s.6/r3 | s.7/r2 | s.7/r3 | s.8/r2 | s.8/r3 |
|---|---|---|---|---|---|---|---|---|
| ripgrep | 64 | **24** | 32 | 7 | 20 | 1 | 14 | 0 |
| flask | 18 | 2 | 16 | 1 | 15 | 0 | 15 | 0 |
| serde | 25 | **13** | 8 | 3 | 4 | 0 | 3 | 0 |
| click | 5 | 2 | 3 | 1 | 2 | 1 | 1 | 1 |
| gin | 14 | **12** | 3 | 2 | 1 | 0 | 1 | 0 |

Hand-labels (seeded RNG samples; TP = plausibly same task reimplemented,
structurally-true-but-useless = FP):

- **s0.7/r2, n=20** (covers s0.8/r2 as a subset via per-finding sim): **3/20 TP
  (15%)**; at the 0.8 cutoff 3/15 (20%). TPs: click `get_binary_stream`/
  `get_text_stream`, serde `collect_seq`/`collect_map`, gin
  `CreateTestContext`/`CreateTestContextOnly`. The 17 FPs are boilerplate-family
  bridges: 12 ripgrep `update` pairs LINKING two already-reported near groups
  ({glob-push}×{TypeChange} etc.), 3 flask `record_once` one-liner pairs,
  `matches_into`/`interpolate`, `decide_tag`/`decide_identifier`.
- **s0.5/r3, n=20** (ripgrep 7 / serde 5 / gin 5 / flask 2 / click 1): **11/20
  TP (55%)**. TPs: help `generate_short`/`generate_long`; serde cross-crate
  `end()` copy family (3 pairs); serde_derive `serialize_*_variant`
  scaffolding (2); gin `RunUnix`/`RunQUIC` + `Run`/`RunQUIC` (the duplicated
  proxy-warning block ×5 is a live inconsistent-update hazard);
  `LoadHTMLGlob`/`LoadHTMLFiles`; ripgrep `from_entry_os`/`from_path`; click
  stream getters. FPs: jsont.rs serialize-impl family (4 — field lists ARE the
  content), gin per-format `Render` impls (2 — interface-driven), color.rs
  `from_str` enum tables, flask tutorial view-pattern echoes (2).
  Sampling note: the second batch's exclusion of batch-1 draws used Python's
  salted `hash()` and re-drew RunUnix/RunQUIC once; the duplicate was replaced
  deterministically with the next unlabeled gin finding.

**Verdict (D29): KEEP, defaults now sim=0.5 / rare=3** — 55% ≥ 50% with ≥10
findings on each large repo (ripgrep/serde/gin). Narrow pass at n=20;
per-repo skew (ripgrep-side FPs dominated by one serialize-impl family)
recorded in D29 with the family-grouping follow-up.

### §7.2 — stratified 50-finding precision sample

Pool: all main-section groups of ripgrep+flask+serde+click+gin at pre-M4a
defaults (796 groups). Allocation proportional with min 5/tier: exact-region
27, near-normalized 8, exact-normalized 5, internal-repeat 5, inline-assisted
5. Selection: seeded RNG (seed 50202607) over (repo, group-id)-sorted pools —
NOT top-by-value. Every finding read in source; labels one-line each (S01–S50
in the M4a working notes; representative justifications below).

| tier | TP/n | precision |
|---|---|---|
| exact-normalized | 5/5 | 100% |
| near-normalized | 8/8 | 100% |
| exact-region | 17/27 | 63% |
| internal-repeat | 2/5 | 40% → **mitigated (D30)** |
| inline-assisted | 3/5 | 60% |
| **overall** | **35/50** | **70%** — meets the ≥70% Phase-2/3 bar, exactly at the line |

Label detail (compact; TP unless marked):
exact: with/no-filename update mirror; sort/sortr update; byte_count ×2 sinks;
SeqDeserializer end() copies; visit_seq deserialize family ×4.
near: gin Render family; TagOrContent visit_* ×6; click format_options/
format_arguments; flask max_* property triple; deserialize_any seq/map family;
gin MarshalXML copied into utils.go (div 0.006); TagOrContentField visitors;
tuple-struct/tuple delegators.
exact-region TPs: Debug-fmt twins, field-key visitors, enum_internally/
enum_untagged variant deserialization, struct_/tuple preamble, serialize
element/key, bound/receiver visitor arms, flask delete/patch shortcuts,
deserialize_map[_in_place] blocks, impossible.rs twins, standard.rs coloring
loop, click chunk-pump example↔test, tutorial register/create scaffold,
serialize_*_visitor block, visit_str/borrowed twins, Content match by-val/
by-ref twins, u16/u32 delegators, Glob/GlobSet is_match.
exact-region FPs (10): **signature/scaffold-dominated runs between
different-logic functions** — trait-method signatures shared by an impl and a
forwarder (replace_with_captures_at), match-scaffold between different
dispatch methods of the Content deserializers (5 cases), the exception-triple
parameter list (`__exit__`/`_close_with_exception_info`), and Python
`@t.overload` stub↔impl signature echoes (2).
internal-repeat: FPs were all match/case dispatch tables (Unexpected fmt,
hyperlink Part fmt, fish completion arms); TPs were statement runs
(runtime_cpu_features detect/push ×3, format_eta divmod chain).
inline FPs (2): pairs whose match mass is the SHARED callee's inlined body
(gin init/init via SetMode; Bind/BindBody via decodeMsgPack) — one-line
delegator bases, nothing consolidatable. TPs: gin decode* family via
validate, ripgrep convert::usize/u64 via str, header/query Bind.

**Mitigation implemented (internal-repeat < 50% → D30):** dispatch-arm repeats
still fold but emit no finding. Post-change counts: ripgrep internal 11→5,
serde 6→4 (every dropped finding verified to be a match-arm table; all other
tiers byte-identical on all five repos). Both sampled survivors are TP.
**Recorded, not implemented** (tiers ≥50%): exact-region signature-dominated
runs (candidate rule: require body-statement tokens in the run — recall-
affecting, needs own calibration); inline shared-callee echoes (candidate D3
extension: suppress when matched mass sits predominantly within inline-
expansion spans on both sides).

Repo tier counts after M4a changes (default config, warm):

| repo | exact | near | region | internal | inline | api (new defaults) |
|---|---|---|---|---|---|---|
| ripgrep | 5 | 53 | 145 | **5** | 3 | 24 |
| flask | 2 | 9 | 26 | 0 | 0 | 2 |
| serde | 22 | 76 | 325 | **4** | 1 | 13 |
| click | 1 | 15 | 55 | 4 | 0 | 2 |
| gin | 0 | 11 | 15 | 0 | 11 | 12 |

### benches/wild (D6 closure)

9 fixtures extracted verbatim from the sample's TP findings (provenance table
with repo+path+commit in benches/wild/README.md): exact ×3 (byte_count,
sort/sortr, serde end), near ×3 (gin MarshalXML, serde TagOrContent, flask
max_* properties), exact-region (click chunk-pump), inline-assisted (ripgrep
convert), internal-repeat (runtime_cpu_features). `tests/wild.rs` asserts
each converges at its labeled tier + fixture/table consistency.

### Lossless perf pass (D27 constraint; method + verdict in D32)

| measurement | before | after |
|---|---|---|
| warm 500k scan (this host, back-to-back) | 10.2 s | **8.1 s** |
| — near verify | 3.23 s | 1.14 s |
| warm 500k `check --base HEAD` (the gate) | — | **8.1 s** (≤10 s gate met) |
| serde pair parity vs pre-change binary | — | **1526/1526** (0 lost, 0 demoted) |
| ripgrep pair parity | — | **2155/2155** |

Changes: per-call AU hash memoization (au_list recomputed full subtree hashes
per DP cell) + histogram early-exits — both output-identical by construction
and verified by pair-level A/B (`benches/compare/pair_ab.py`, the D27
method). The original 5 s line is NOT reachable losslessly (D32: remaining
blocks are SA construction / inline table / cache load, all already parallel;
banding skipped as not provably lossless and no longer the bottleneck).

### M4a gate summary

- Tests **117/117** green (104 → 117: +11 tests/m4a.rs, +2 tests/wild.rs);
  clippy/fmt clean.
- api-profile verdict recorded with data (D29): kept at retuned defaults.
- Stratified §7.2 sample: 70% overall (at the bar), per-tier above, one
  sub-50% tier found and mitigated (D30).
- Wild corpus exists with passing tests (D31).
- Perf: 8.1 s warm at 500k with zero pair-level recall regression (D32);
  5 s honestly declared unreachable under the lossless constraint.
- Correctness batch (D31): pragma-over-attributes, generated first-line
  signatures, fail_on load-time validation, --top 0; plus the
  EXTRACTION_VERSION cache-key fix (D30).
