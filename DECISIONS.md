# DECISIONS.md

Deviations from and resolutions of ambiguities in `docs/PLAN.md` (the Rev 9 spec), per its
audience note. Entries D1–D8 were seeded during pre-implementation evaluation (2026-07-01),
before any code exists; they resolve under-specifications an implementer hits early, not
architecture changes. Later entries are appended as implementation proceeds.

---

## D1 — Definition of "normalized token"

**Problem:** Every threshold in §9 (`min_unit_tokens`, `min_seq_tokens`,
`bag_min_subtree_tokens`), the divergence ratio (§5.6), and the sequence tier's guarantee
framing denominate in "normalized tokens," but the spec never defines the unit.

**Decision:** One normalized token = one element of the pre-order serialization of the
normalized tree, where each node emits `(kind, label?)` — `label` present for kept external
names, kept literals, and literal buckets. A unit's token count is the length of this
serialization. The same serialization is the sequence tier's input (see D4), so the two
tiers count in the same unit. The definition is part of the fingerprint scheme and is
versioned with it (§4 cache keys, §12 stale-baseline risk).

## D2 — Identifier abstraction split: positional for the exact hash, masked for retrieval

**Problem:** Positional renaming (`v0`, `v1`, …) cascades: one inserted early declaration
renumbers every later local, which perturbs most subtree hashes and guts bag-Jaccard
retrieval (§5.5.2) — the near-miss tier's front door. This is a known weakness of
NiCad-style consistent renaming under statement insertion.

**Decision:** Positional names are used **only** for the whole-unit exact structural hash
(§5.5.1), where consistent-rename convergence is the point and structure is identical by
definition. All retrieval layers (subtree bags, hole-context hashes, landmark pairs,
characteristic vectors) mask every local to a single `LOCAL` marker. AU verification (§5.6)
sees the positional trees, so rename-consistency divergences still surface as holes there.
Effect on candidate recall is measured under §7.4(b) alongside the rivalry.

## D3 — Inline-variant tautology filter

**Problem:** An inlined variant of caller A textually contains callee B's body, so
"A-inlined matches B" is guaranteed by construction and worthless. §5.6's rule ("a unit
never matches itself or its own inlined variant") does not cover A-inlined vs B.

**Decision:** Suppress any finding in which the matched region of an inlined variant lies
entirely within spans produced by inline expansion, unless matched material extends beyond
those spans. Inline-expansion spans are recorded per variant at P5 for this purpose (they
are also needed for the §5.4 inline-chain display).

## D4 — Token stream provenance and sequence-tier mechanics

**Problem:** §5.1/§5.6 leave the stream underspecified: source of tokens, separators,
source mapping, and the fact that string-maximal repeats arrive in large overlapping
families.

**Decision:** (a) The stream is the D1 serialization of **normalized** trees — the sequence
tier and tree tier see the same canonical form, so a sequence-tier "exact run" means
exactly what the tree tier means by it. (b) Units are joined with per-unit unique sentinel
tokens (never equal to any real token) so no repeat crosses a unit boundary artificially.
(c) A parallel span array maps token index → original byte span (D1 nodes carry spans, §4
hard requirement). (d) Repeat extraction uses LCP-interval enumeration with a
supermaximal/local-maximality filter plus interval dedup per file pair — budgeted as a real
Phase-2 work item, not the "read off" the spec implies.

## D5 — Phase-1 normalization scope

**Problem:** §8 Phase 1 says "normalization without desugaring beyond loops," which names
§5.2 step 3 but leaves steps 6–7 ambiguous.

**Decision:** Phase 1 implements §5.2 steps 1 (strip), 2 (loop decomposition, excluding
recursion lowering which §8 gates in Phase 2), 4 (identifier abstraction), 5 (literal
abstraction), and 7 (dead-syntax removal). Step 3 (desugar) and step 6 (order
canonicalization — it wants subtree fingerprints anyway) move to Phase 2. The Phase-1 gate
(Type-1/Type-2 recall) exercises only steps 1, 4, 5; step 2 lands in Phase 1 because the
pass ordering (lowering before abstraction) is load-bearing and retrofitting it later would
churn every fingerprint.

## D6 — Mutation-benchmark hygiene

**Problem:** (a) The generator applies the same rewrite classes the normalizer inverts, so
class recall validates that rules exist, not that they match real LLM drift. (b) A naive
literal-mutation class that swaps `0`→`5` tests the keep-list (§5.2.5), not literal
abstraction.

**Decision:** (a) §7.2 manual sampling remains the counterweight and is non-negotiable; in
addition, real LLM-duplicated pairs found during dogfooding are accumulated into
`benches/wild/` as a second, non-synthetic recall set. (b) The literal-mutation class
mutates only non-keep-list literals to non-keep-list values; keep-list boundary behavior
gets its own tiny test, not a benchmark class.

## D7 — check-mode diff mapping and cross-tier subsumption

**Problem:** §2/§6 never say how a git diff maps to units; §5.6's subsumption rules don't
order `internal-repeat` vs sequence-tier findings on the same span.

**Decision:** (a) `check --base` parses `git diff -U0 <base>` hunk line ranges; a unit is
"touched" iff its line span intersects any hunk. (b) Subsumption ordering: within one span,
`internal-repeat` (a structural finding with a template) beats a sequence-tier repeat; the
existing containment rules otherwise apply unchanged.

## D8 — Cache storage: flat files, not sled

**Problem:** §4 offers "sled, or flat rkyv/bincode files."

**Decision:** Flat bincode files under `.reprise/cache/`, one per source file, named by
`xxh3(content ‖ normalizer_version ‖ grammar_versions)`. sled is effectively unmaintained
and a KV store buys nothing here; the spec's version-keying requirement is the filename.

---

## Environment facts recorded at evaluation time (2026-07-01)

- crates.io: `tree-sitter` is at **0.26.x**; pin it and grammar crates at M0 (API churn
  across 0.20→0.26 is real). `cargo search reprise` shows no name collision, consistent
  with the spec's Rev 8 claim; re-verify at publish (Phase 4).
- Toolchain: rustc 1.94.1, `just` available; grammar crates need a C compiler (present in
  the dev container image).
- Pinned at M0: tree-sitter 0.26.10, tree-sitter-rust 0.24.2, tree-sitter-python 0.25.0.
  These version strings feed the D8 cache keys.

---

## D9 — Phase-1 implementation notes (2026-07-01)

- **Lowered forms mirror probed CST shapes exactly.** The loop core is synthesized to
  byte-match what tree-sitter produces for the manually written form (verified with
  `examples/probe.rs`, kept in-repo): Rust statements wrap in `expression_statement`;
  Python's manual `not (x)` puts the operand in the `argument` field after paren
  flattening. Any grammar upgrade can shift these shapes — the convergence tests
  (`tests/convergence.rs` while↔loop cases) are the regression guard, and the pinned
  grammar versions above are load-bearing.
- **Idempotence by label lifecycle**, not by re-checking: conversion emits transient
  `Raw`/`RawLit` labels; passes only consume transients, so a second application is
  identity. The one kind-based check: `while True` conditions are recognized as
  already-core by node kind (`true`), which also correctly treats hand-written
  `while True:` as the core form.
- **Anonymous-token policy**: operators are kept as their own node kind only under an
  explicit parent-kind allowlist (binary/unary/compound-assign/range for Rust;
  comparison/binary/boolean/unary/augmented for Python); all other anonymous tokens
  (punctuation, keywords) are implied by parent kinds and stripped. `not_operator` is
  deliberately outside the Python allowlist so synthesized and manual `not` converge.
- **Property tests are corpus-based for now** (seeds + inline snippets, normalize-twice
  equality), not generative proptest — revisit once the tree API stabilizes through M2;
  if generative testing still isn't warranted then, record why here.
- **Mutation-benchmark mutators live inside `tests/mutation_recall.rs`** rather than a
  reusable generator crate; extract when Phase-2 classes (loop-form swap, reorder,
  unroll, recursion rewrites) outgrow the file.

---

## D10 — Sequence-tier implementation notes (Phase 2, 2026-07-02)

Region findings carry tier **`exact-region`** (the spec's §6 list doesn't name the
sequence tier; ranked between `internal-repeat` and `near-normalized` in confidence).
Simplifications vs. §5.6, recorded per D4d: repeats come from adjacent-SA-pair scans
with left-maximality + per-pair containment dedup, not full LCP-interval supermaximal
enumeration (adequate at current scale; revisit if multi-way region groups are needed).
Same-unit repeats are skipped (folding owns internal duplication). **Nested units**
(an inner `fn` and its parent) serialize the same source text twice — regions between
overlapping same-file units are artifacts and are excluded (this removed 316 of 369
raw regions on ripgrep). Regions and internal-repeat findings follow the test-code
policy like any other group.

## D11 — Phase-2 desugaring subset

Implemented: loop decomposition (incl. Rev 5 recursion lowering with identity-pair
elision), loop-exit normalization, iteration-protocol rewrite (Python `range(len(x))`
form AND the while-index analogs in both languages), order canonicalization
(commutative chains sorted by (MaskedLocals, Exact) hash — masked-primary so local
index shifts don't reorder operands; dict/struct-field sorting), paren flattening,
dead-syntax (+ loop-tail `continue`). Deferred per the §12 rule-addition principle
(no benchmark class or calibration FN demanded them yet): `if let`→`match`,
comprehension→loop, `with`-multi→nested, f-string→format-call, fold epilogue
(divergent last iteration), the accumulator (near-tail) recursion rule. Each needs a
driving test case when added.

## D12 — Deckard characteristic vectors not built

Bag + landmark retrieval covers current scale; the vector/LSH layer (§5.5.5) would
add a third index with no demonstrated recall gap. Revisit if landmark candidate
volume (the §7.4b watch item) forces banding anyway.

## D13 — `max_divergence` = 0.18 (was spec-guessed 0.15)

Set empirically per §7.4(c): the honest whole-expression hole for `X` vs `X op Y`
substitution costs ~0.17 on realistic functions (score_rows case). Controls stay
comfortably rejected (unrelated pairs measure ≥0.27).

## D14 — Zero-cost holes in AU

Two hole classes count toward neither divergence nor the hole budget:
(a) **consistent Local↔Local leaf pairs** — alpha-renaming discovered post hoc, the
positional-cascade noise D2 predicted, not semantic divergence; (b) **synthetic
machinery gaps** — one-sided gaps consisting solely of lowering-generated guard/bind
statements (`__has_next`/`__next`, ≤12 tokens), which are canonicalization artifacts
of comparing folded REPEATs and differently-lowered loops. Both remain visible in
templates as holes; they just don't penalize. Without (a), positional index drift
alone blew every hole budget; without (b), rolled↔unrolled convergence paid for the
machinery the normalizer itself introduced.

## D15 — Subsumption refinements

Weak-similarity groups do NOT subsume sequence regions (verbose-only findings must
not eat harder evidence). AU alignment refuses to pair machinery statements with real
code (forces the free gap). Binary-operator AU: differing operators hole the whole
expression (an operator-token hole is a partial construct, §5.6); matching operators
align flattened operand chains (order canonicalization can sort divergent operands
differently on each side).

## D16 — Rivalry verdict (§5.5/§7.4b): landmark pairs KEPT, hole-context hashes DELETED

Measured 2026-07-02 (full table in CALIBRATION.md): landmark-only retrieval achieves
100% Phase-2 benchmark recall; hole-context-only misses light-edit and unroll; on
three real repos hole-context hashes contributed **zero** verified pairs that other
layers didn't already propose, at ~44% of the landmark layer's index cost. Layer code
removed per the spec's "dropped, not kept alongside." The `hole_hash_min_cover`
threshold key remains in §9/config as dead-but-accepted for config compatibility.

## D17 — M3a aggregation notes (2026-07-02)

(a) **Cross-granularity region dedup:** inline-variant token streams legitimately
feed the sequence tier (spec §5.1), but the same duplication can then surface once
from plain-unit streams and again from variant streams. Regions are now deduped by
file-pair line-span containment across ALL regions (largest covering run wins), not
just within one unit pair. (b) **api-profile coverage** at spec defaults is near-zero
at function granularity (see CALIBRATION §7.4d); tune-or-drop decision deferred to
Phase 4 as the spec provides. (c) M3a implementation detail worth its own line: the
definition table and inline splicing operate on RAW (pre-pass) trees so that the
existing recursion lowering fires on SCC-expanded units — this is what makes the
mutual-recursion chain converge, and it is why the inliner runs before P3 rather
than at the spec's nominal P5 slot in the pipeline diagram (detection-equivalent,
order recorded here per the §8 no-silent-gate rule).

## D18 — M3a inline-tier refinements (recorded late; code referenced D18 already)

Two M3a decisions whose code comments cite D18; the entry itself was omitted at
the time and is recorded now, unchanged in substance. (a) **Base-equal
suppression:** an inline pair whose BASES are exact-equal is tautological — the
plain exact tier owns that pair, or the size floor deliberately excluded it, and
equal bases inlining equal callees match by construction (extends the D3 filter;
see `matchtree::tautological`, `group::build_inline_exact_groups`). (b)
**`max_callee_tokens` measures the raw body**, not the post-fold unit: folding
under-reports the mass a splice actually inserts (see `inline::Def`).

## D19 — Cache implementation (D8 concretized, 2026-07-02)

Per-file cache value = the whole `unit::FileUnits` (units incl. normalized
trees, internal repeats, raw trees for the inliner's definition table,
suppressed-unit count), serialized with **bincode 1.3** via serde derives on
`NormNode`/`Label`/`Bucket`/`Lang`/`Unit`. Key = xxh3-128 of (root-relative
path ‖ file content ‖ `FINGERPRINT_SCHEME` ‖ `CARGO_PKG_VERSION` ‖ grammar
version list ‖ extraction-relevant config: `literal_keep`, `fold_min_repeats`,
`min_seq_tokens`). Notes: (a) grammar versions are a **hardcoded constant list**
(`cache::GRAMMAR_VERSIONS`) adjacent to the Cargo.toml pins — Cargo exposes no
dependency versions at build time; the list must move with any grammar bump
(the D9 pins are load-bearing anyway). (b) The relative path is in the key
because `is_test` and repeat coordinates depend on it; a moved repo still hits
(paths re-anchored on load). (c) Writes are best-effort (read-only roots
degrade to cold scans) and write-then-rename (no torn reads). (d) The cache
dir writes its own `.reprise/.gitignore` containing `*` — scanned repos need
no .gitignore edit; reprise's own repo also ignores `.reprise/` explicitly.
(e) Config knob `[cache] enabled` (default true) exists for perf comparison
and forcing cold runs. Cold-vs-warm byte-identity is asserted by test.

## D20 — Baseline and check semantics (spec §2/§6 concretized, 2026-07-02)

(a) The baseline records **all four report sections** (main/test/api/weak,
tagged), satisfying §2's "all current findings"; `check` gates on `main` only —
the other sections never fail CI by construction, and inconsistent-update
tracking also reads `main` only (test-code drift never gates, per the §5.1
test policy). (b) **Member identity** between a baseline snapshot and current
state: same root-relative file AND (same non-anon name OR overlapping line
span) — names are stable across line drift, spans across renames; the
fingerprint key is the group identity (§6.1). A fingerprint match additionally
requires ≥1 member match, guarding against key collisions between disjoint
groups. (c) **Worsened** = a member with no baseline counterpart (member
added), or divergence increased beyond `DIVERGENCE_EPS = 0.01`. (d)
**Inconsistent-update findings are computed from the baseline state + diff**,
not from current groups: a group whose members drifted past `max_divergence`
(and thus vanished from the current scan) still fires — that is the fault
window §6 exists for. Baseline members map to current units via (b); an
unmapped member in an edited/deleted file counts as touched. (e) When an
inconsistent-update finding already reports a group's drift, the same group is
not re-emitted as `worsened` unless a member was also added; untouched
baselined groups with divergence growth are emitted as non-failing `drifting`
notes with the trend. (f) Exit codes: 0 clean, 1 findings at/above `fail_on`
(CLI `--fail-on` overrides `[report] fail_on`; `none` disables). The §6
confidence order for the gate is inconsistent-update > exact-normalized >
internal-repeat > exact-region (D10) > near-normalized > inline-assisted;
api-profile and weak-similarity never fail.

## D21 — `reprise:ignore` pragma semantics (spec §2 concretized)

Suppression is a **substring check** for `reprise:ignore` on the unit's first
line or the line immediately above (spec wording), without verifying the match
sits inside a comment token — a string literal containing the marker on a
unit's first line would false-suppress; accepted as negligible against the
cost of per-language comment parsing at this stage. "Line immediately above"
is literal: Rust attributes / Python decorators between the pragma and the
`fn`/`def` line defeat it (the pragma must sit directly on/above the
definition line). A suppressed unit is removed from the entire pipeline —
all tiers, the sequence-tier stream, AND the inliner's definition table (the
pragma means "leave this unit alone"; callers lose inline-assisted convergence
through suppressed helpers, a deliberate simplification). Counts appear in
scan stats and the terminal header (§12).

## D22 — M3b performance work: retrieval hardening + parallelism (2026-07-02)

The Phase-2 watch item (landmark candidate volume) fired at 500k LOC: warm
scans took 130–150 s, ~90% in the near tier — 776k landmark candidates of
which ~566k passed the offset histogram and paid full AU. Changes, in
principle order:

- **Histogram hardening** (the §5.6 gate, semantics change): each shared hash
  now votes at most once per Δ-bin (multiplicity of a repeated small idiom no
  longer fakes a diagonal), and the acceptance threshold scales with unit
  size — `max(histogram_min_votes, min_inventory/8)` — because a pair that
  can survive `max_divergence` must share a material fraction of the smaller
  unit's subtree inventory; 5 absolute votes on a 250-token unit is 2%
  coverage, admitting pairs AU is guaranteed to reject. (Wang's original
  scores by aligned-cluster size for the same reason.)
- **Landmark candidate floor** raised from ≥2 to ≥4 shared pair hashes
  (`SHARED_LANDMARKS_MIN`) — true clones share constellations.
- **Windowed pair-event enumeration** for hashes with >8 owners (window 3,
  owner order = unit order): grouping is transitive via union-find, so
  adjacent-owner chains connect large clone families without O(owners²)
  events per hash. Non-adjacent members of a big family may lose their direct
  pairwise finding but stay in the same group.
- **Mechanical parallelism/allocation work** (no semantics change): sort-based
  pair counting (was BTreeMap per event), sorted-vec offset inventories merged
  linearly in the histogram (was HashMap probes), parallel rep building /
  landmark generation / corpus serialization (dense token ids by sorted-hash
  rank replace the sequential interner) / DefTable construction and call-graph
  edges / api-profile extraction, suffix-array prefix-doubling over
  materialized u128-packed keys with parallel sorts, borrowed (not cloned)
  callee bodies in `Def`, borrow-keyed inline resolution, file-pair-bucketed
  region dedup. `REPRISE_TIMING=1` prints per-stage timings; `phase_ms` is in
  scan stats.

**Recall evidence:** mutation benchmark unchanged (all classes 100%/0%
controls); ripgrep re-scan matches Phase-2/M3a exactly (5 exact / 53 near /
11 internal / 3 inline-assisted), flask matches exactly (2 / 9 / 26 regions);
ripgrep cold scan 1.9 s → 0.31 s. Numbers in CALIBRATION.md M3b.

## D23 — M3c language additions: grammar crates, versions, TS/TSX partitioning (2026-07-02)

Phase-3 milestone M3c adds **TypeScript (incl. TSX), Go, and Kotlin** profiles
(spec §3 priority order 3–5).

- **Grammar crates pinned** (added to `cache::GRAMMAR_VERSIONS` per D19, and the
  Cargo.toml pins): `tree-sitter-typescript 0.23.2` (exposes both
  `LANGUAGE_TYPESCRIPT` and `LANGUAGE_TSX`), `tree-sitter-go 0.25.0`,
  `tree-sitter-kotlin-ng 1.1.0`. All resolve against the pinned
  `tree-sitter 0.26`.
- **Kotlin crate choice — `tree-sitter-kotlin-ng` 1.1.0 (chosen) over
  `tree-sitter-kotlin` 0.3.8.** Spec §12 flags Kotlin grammar quality. The
  `-ng` fork (tree-sitter-grammars org) parsed every M3c fixture with **zero
  ERROR nodes** and exposes clean fields (`name`, `condition`, binary
  `left`/`operator`/`right`). It is actively maintained against current
  tree-sitter. Recorded per the working agreement to evaluate and justify.
- **`.ts` and `.tsx` are separate `Lang` variants sharing one
  `TypeScriptProfile`** but routed to distinct grammar objects
  (`LANGUAGE_TYPESCRIPT` / `LANGUAGE_TSX`) as the scope instructs ("route .tsx
  to the TSX language object"). Consequence: `.ts` and `.tsx` units land in
  **separate language partitions** and never cross-group. Same-language-only
  matching (§3) makes this sound; cross-dialect `.ts`↔`.tsx` grouping is a
  deferred nicety, not a correctness gap.
- **New trait hook `splice_kind`** (default `false`): kinds whose children are
  hoisted into the parent during conversion. Go returns it for
  `statement_list` (Go wraps every `block` body in a `statement_list`); Kotlin
  for `function_body` (the body block sits under a `function_body` wrapper).
  This makes Go/Kotlin blocks hold statements directly, matching the
  Rust/Python shape every downstream pass assumes. Rust/Python use the default.

## D24 — M3c per-language normalization quirks (2026-07-02)

Probed shapes (`examples/probe3.rs`, D9) drove these; the notable non-obvious ones:

- **Go loop core = infinite `for {}`** (no `while`/`loop` keyword). All three
  `for` forms lower to it: condition-only lowers like `while`; three-clause
  hoists init + appends step at the **block level** (a single-node rewrite
  can't emit init+loop, so `expand_for_clauses` runs on statement lists); range
  lowers with `__has_next`/`__next`. The range/for-in binding uses the
  language's **declaring** form (`:=` short_var_declaration) so
  `collect_declared` binds the loop var as a local — a plain `=` assignment
  left it External and broke rename/index convergence.
- **Kotlin quirks:** `break`/`continue`/`true` parse as bare `identifier`s
  (not keyword nodes), so `is_loop_core`/`is_continue_stmt` match identifier
  text (`Raw`/`External` "true"/"continue"); the loop core is
  `while (true)`. Function body is `function_body > block` with **no `body`
  field**, navigated by kind. **Navigation targets** (`x.field`) carry no
  positional field/kind that `always_external` can key on, so a pre-pass
  (`externalize_nav`) relabels them External before identifier abstraction. The
  for-in binding uses `property_declaration` (the declaring form) for the same
  collect-declared reason as Go.
- **TypeScript:** type-level nodes (`type_annotation`, `type_parameters`,
  `type_arguments`, `decorator`, `accessibility_modifier`) are stripped
  wholesale — behaviorally neutral for clone detection, like Rust lifetimes,
  and they otherwise inflate/drift the tree. `number` is the sole numeric kind
  (int and float alike) → `Bucket::Int`. Loop core is `while (true)`; the
  already-`true` guard (mirrored from Python) prevents re-lowering a manual
  core.

## D25 — M3c desugaring subset and deferrals (spec §5.2.3, §12 rule-addition rule)

Implemented per language: loop decomposition (while / do-while / for-of/for-in /
three-clause / Go-range), loop-exit normalization, tail-recursion lowering,
iteration-protocol rewrite (Go `for i:=0;i<len(xs);i++` → range — the canonical
Go pattern; TS `for(let i=0;i<xs.length;i++)` → for-of; Kotlin `for(i in
xs.indices)` and `for(i in 0 until xs.size)` → for-in), commutative-operand and
key sorting, paren/dead-syntax removal. **TS arrow → function-expression
desugaring is implemented** (parenthesized-param arrows; expression bodies wrap
as `{ return … }`), so `(x)=>x+1` converges with `function(x){return x+1}`.

Deferred (no driving benchmark/calibration case yet — the §12 principle):

- **TS for-of vs for-in are not distinguished.** The `of`/`in` operator is an
  anon token that convert() drops (for_in_statement is not a keep-anon parent),
  so both lower alike. A for-in (object keys) can converge with a for-of
  (values); accepted as a minor false-positive risk under the unsoundness
  tolerance.
- **TS arrow/function assigned to a const is not extracted as a unit** (only
  `function_declaration`/`method_definition` are units, mirroring
  Rust/Python's decl-only extraction). `export const f = (x)=>…` is invisible.
  The scope explicitly permits deferring this; recorded as a gap.
- **Kotlin `when` is left as its native `when_expression`** (spec instruction).
- **Kotlin multi-param tail recursion emits sequential single assignments**
  (Kotlin has no simultaneous multi-assign). Unsound for coupled updates but
  tolerated for detection; single-changed-param recursion (the common case,
  incl. identity-dropped partners) converges cleanly.
- **Kotlin is not an inliner definition source** (no `body`-field access path in
  `inline.rs`); Kotlin units still inline *callers* nowhere and are matched by
  every other tier. Near-tail (accumulator) recursion remains deferred from D11
  for all languages.

## D26 — Inliner: unreachable multi-statement splice → graceful skip (2026-07-02)

`inline::try_expr_inline` pre-checked `def.body.children.len()==1` to guarantee
an expression-position splice stays single-statement, then `unreachable!()`d
otherwise. But `splice_body` runs nested inlining (`walk`), so a one-statement
callee body (`return g(...)`) can expand to several statements; with no
expression block (Go/Python `make_expr_block == None`) this hit the
`unreachable!` and **panicked on the gin corpus**. A latent bug (Python could
trip it too) exposed by Go. Fix: the `None` arm now returns the call
un-inlined. No behavior change for cases that previously succeeded; only
formerly-panicking sites now skip. Rust/Python inline tests unchanged.

## D27 — Coordinator audit of the D22 perf trims: two reverted, gate renegotiated (2026-07-02)

The D22 recall evidence ("findings identical on ripgrep/flask") was too coarse — those
were the repos the tuning was validated against, and group-set comparison hides member
loss. A pair-level A/B on repos the tuning never saw (serde, click) showed the fast
configuration losing **43/381 reportable member-pairs on serde (11%), 7 members
vanishing entirely** — including textbook true positives (`visit_bytes`/`visit_byte_buf`
family, `deserialize_bool/map/seq`, `visit_str`×6). Bisection attributed the loss:

| Trim | serde pairs lost | 500k warm cost of reverting |
|---|---|---|
| histogram hardening (distinct votes + size-scaled threshold) | **0/43** | reverting → >100 s (this WAS the perf fix) |
| `SHARED_LANDMARKS_MIN` 2→4 | 15/43 | ~negligible |
| owner windowing (window 3, >8 owners) | 30/43 | ~2 s |

**Verdict:** keep the histogram hardening (zero recall cost, the entire AU-flood fix);
revert the landmark floor to 2 and disable owner windowing by default. Both are now
config keys (`[retrieval] shared_landmarks_min = 2`, `owner_pair_window = 0` meaning
unlimited — df_cap=50 already bounds the per-hash quadratic), so tuning constants can
no longer hide behind rebuilds. Verified after the change: serde 381/381 pairs (parity
with fully-thorough), all 98 tests green, mutation gates unchanged.

**Gate renegotiation (per spec §8's explicit mechanism and §4's "targets, not hard
gates"):** warm 500k-LOC check is now ~7.0 s against the §8 "<5 s" line. The lossy
configuration met 5 s by dropping 11% of true pairs on unseen repos; for a tool whose
flagship finding is the PR-time `inconsistent-update`, recall dominates 2 s of CI
latency. Phase-3 perf gate restated: **≤10 s warm at 500k LOC, with zero recall
regression vs. the thorough configuration** (both measured). Further speed must come
from mechanically-lossless work (banding, AU memoization), not retrieval trims.

## D28 — M3d output formats + §7.3 baseline comparison (spec §6.1/§7.3, 2026-07-02)

Standard-format emitters (`--format sarif|cpd|jscpd`) for scan AND check, plus
the deferred prior-art comparison. Pure serializers — no matching/grouping/
ranking touched; scan tier counts on ripgrep/flask are byte-for-byte the D22
figures (5 exact / 53 near / 11 internal / 3 inline). New module `src/formats/`
(`sarif.rs`, `cpd.rs`, `jscpd.rs`, shared `mod.rs`); config key
`report.sarif_fingerprint` ("structural" default | "line"); Stats gains the §6.1
duplication metrics (`total_lines`, `total_tokens`, `duplicated_lines[_pct]`,
`duplicated_tokens[_pct]`, `clones_per_kloc`).

**SARIF mapping (the three §6.1 decisions, implemented exactly):**
- (1) **Group = one result at N locations.** Primary `locations[0]` = highest-
  ranked member (scan: `members[0]`) or the diff-touched member (check: first
  member matching `CheckFinding.touched`); remaining members are
  `relatedLocations[]` with integer `id`s, referenced from `message.text` as
  `[member](id)` per GitHub's multi-location convention; the AU template is
  embedded in the message.
- (2) **`partialFingerprints` = structural.** Key `reprise/structuralFingerprint/v1`
  = `Group.fingerprint` (the D20 stable hash — exact/AU-template/region/internal
  as applicable). `report.sarif_fingerprint = "line"` **omits** our key entirely
  so GitHub falls back to `primaryLocationLineHash` (asserted by test).
- (3) **`level` not overloaded.** Tier → `properties.tier` (+ `divergence`,
  `value`, `tokenCount`, `section`, inline chain); `level` is only the CI-gate
  coarse map: api-profile/weak → `note`, tier at/above `fail_on` → `error`, else
  `warning`. Spec-valid skeleton: `$schema`, `version` "2.1.0",
  `runs[0].tool.driver{name:"reprise", version: CARGO_PKG_VERSION, rules[]}` one
  rule per tier with `shortDescription`; region via `physicalLocation` +
  relative `artifactLocation.uri` + `region.startLine/endLine`.

**Section policy (all three formats).** SARIF emits `main` (full), `test` + `api`
(note-level, `properties.section` marker), and `weak` **only under `--verbose`**
(picked: weak is verbose-only elsewhere too). CPD/jscpd emit `main` + `test`
(+`weak` verbose); they **exclude `api`** (structurally different — no shared
fragment to slice) and **exclude single-member `internal-repeat`** (CPD/jscpd
need ≥2 locations).

**Duplication ratios.** Line ratio = union of covered source lines over `main`
groups ÷ total scanned lines (∈[0,100], the GitClear copy/paste-line shape).
Token ratio = `Σ token_count·(members−1)` over `main` groups ÷ `total_tokens`
(D1 normalized tokens — removable-copy mass, more stable across formatting).
Both are §6.1 "context, not primary evidence." jscpd's own `statistics.total`
block is recomputed independently from the emitted `duplicates[]` for faithful
drop-in.

**jscpd pairwise expansion.** An N-member group → N−1 `duplicates[]` entries
(first member vs. each other), because jscpd models a clone as a
`firstFile`/`secondFile` pair. Fragment/`lines`/`tokens` come from the group's
first member / `token_count`.

**Tests** (`tests/formats.rs`, 6 tests): structural assertions, **not** golden
files (golden files churn on every ranking/token tweak and add no coverage the
structural checks miss). SARIF/jscpd parsed as JSON via serde_json (existing
dep); CPD well-formedness hand-checked (balanced tags/CDATA, attribute presence)
rather than adding a quick-xml dev-dep.

**§7.3 comparison harness** lives in `benches/compare/` (`run.sh` + `compare.py`)
so it re-runs later on any host: it checks `npx`/`java` availability, runs jscpd
(and PMD CPD iff a `pmd` binary is present), scans with reprise, and diffs by
file + line-range overlap. On THIS host node 22 + java 21 are present but no
`pmd` binary — so jscpd (npm `jscpd`, engine `cpd 5.0.11`) is the comparator;
PMD is wired but skipped with an honest note. Numbers in CALIBRATION §7.3.
Comparator note: the npm `jscpd` now ships a Rust engine reporting as `cpd`; it
emits standard jscpd JSON, so the mechanical diff holds. jscpd's `duplicates[]`
are filtered to the target language `format` (it also clones YAML/TOML/comments,
which reprise's per-language function-scoped model does not cover).

## D29 — api-profile §7.4(d) verdict: KEPT, retuned `api_profile_sim` 0.8 → 0.5 (M4a, 2026-07-02)

The mandated tune-or-drop measurement (spec §7.4d; deferred from M3a where the
tier was near-inert: 1 finding on ripgrep, 0 elsewhere). Threshold sweep over
sim ∈ {0.5, 0.6, 0.7, 0.8} × rare ∈ {2, 3} on ripgrep/flask/serde/click/gin
(full table in CALIBRATION.md Phase 4), then 20-finding hand-labels of the two
candidate settings, TP = plausibly the same task reimplemented; structurally-
true-but-useless labeled FP (the tier is suspicion-only and must earn reviewer
attention).

- **rare=2 settings fail.** Dropping the rare floor to 2 creates volume
  (s0.7/r2: 42 findings) but the findings are boilerplate-family bridges: pairs
  of tiny functions sharing 2 generic-ish calls, mostly BETWEEN near-tier
  groups the structural tiers already report (ripgrep `update` families, flask
  `record_once` one-liners). Hand-label: **3/20 TP (15%)**; at the sim=0.8
  cutoff within the same labels, 3/15 (20%).
- **s0.5/r3 passes, barely.** The rare≥3 signature floor carries precision;
  sim=0.5 restores volume (ripgrep 24 / serde 13 / gin 12 — ≥10 per large
  repo; flask 2 / click 2). Hand-label of 20 (seeded, cross-repo): **11/20 TP
  (55%)** — genuine catches include serde's cross-crate `end()` copy family,
  serde_derive ser.rs scaffolding, gin's Run*/LoadHTML* families (the repeated
  proxy-warning block is a textbook inconsistent-update hazard), click's
  stream-getter pair, ripgrep help generators. FP mode: hand-written
  serde-impl families where field lists are the content (jsont.rs), enum
  parse tables, tutorial view-pattern echoes.

**Verdict per the decision rule** (drop only if NO setting reaches ≥50%
useful-precision with ≥10 findings per large repo): the tier stays; defaults
become `api_profile_sim = 0.5`, `api_min_distinct_rare = 3`. Honest caveats
recorded: 55% at n=20 is a narrow pass; per-repo precision skews (serde/gin
high, ripgrep low — the jsont serialize family dominates its FPs); the spec's
"expect to tighten" prediction inverted — the rare floor, not sim, is the
precision knob at function granularity. Open issue: pairwise family inflation
(6 profile-similar functions → up to 15 findings); a family-grouping pass
would improve reviewer economics but changes report shape — deferred until
the tier proves it earns attention at all.

## D30 — internal-repeat: dispatch-arm tables fold but are not findings (M4a, 2026-07-02)

The §7.2 stratified sample measured internal-repeat at **2/5 TP (40%)** — below
the 50% tier bar, mandating a mitigation. All three FPs were the same shape:
match/case dispatch tables (enum→literal `fmt` arms, completion-string tables)
where the "repeated template" is the idiomatic language construct for a lookup
table and no reviewer action exists. The two TPs were statement-run repeats
(detect-feature/push sequences) — loop-able real duplication.

Mitigation: new profile hook `is_dispatch_arm` (Rust `match_arm`, Python
`case_clause`; other languages' switch bodies are not fold list kinds, so
their arms never fold and need no hook). `fold_repeats` still FOLDS dispatch
runs — the REPEAT canonicalization is load-bearing for cross-unit convergence
— but emits no report-level finding when every template node is a dispatch
arm. Effect: ripgrep internal-repeat 11 → 5, serde 6 → 4; every dropped
finding hand-verified as a match-arm table; all other tiers byte-identical.
Post-mitigation, both sampled survivors are TPs.

**Cache-key gap found while validating:** extraction outputs changed but every
cache-key component stayed put, so warm scans served pre-D30 findings. New
`cache::EXTRACTION_VERSION` constant in the D19 key, bumped on any change to
`FileUnits` content for identical input that doesn't already bump
`FINGERPRINT_SCHEME`. A baselined match-arm finding simply goes unmatched
after upgrade (no CI effect; check gates only on diff-touched main findings).

## D31 — M4a correctness batch + wild corpus (2026-07-02)

- **D21 pragma refinement:** `is_suppressed` now scans upward past contiguous
  single-line attributes/decorators/annotations (trimmed `#[`- or `@`-prefixed
  lines), so `// reprise:ignore` above `#[inline]` above `fn` suppresses.
  Multi-line attribute arguments (`@deco(\n…)`) remain unhandled — recorded
  limitation, same substring semantics otherwise.
- **Generated-code first-line signatures (spec §5.1):** (a) a first line ≥512
  chars marks the file generated (minified/bundled signature — no supported
  language keeps such a line in human source); (b) default `generated_markers`
  gains `"Code generated by"` (the Go generator banner prefix; many emitters
  omit "DO NOT EDIT"). Lockfiles need no signature: no lockfile extension maps
  to a scanned language, and yarn-style banners already contain covered
  markers.
- **`fail_on` validated at config load** (`Config::validate`): a typo in
  `[report] fail_on` now fails `Config::load` with the full valid-tier list
  instead of surfacing only when `check` runs in CI.
- **`--top 0` shows all groups** (terminal renderer; help text updated).
- **`benches/wild/` created (closes the D6 mandate):** 9 fixtures = verbatim
  source of hand-labeled TP findings from the M4a precision sample, covering
  all five reportable tiers across all five languages' repos (provenance
  table in benches/wild/README.md). `tests/wild.rs` asserts each converges at
  its labeled tier and that no fixture exists without a test row.

## D32 — M4a lossless perf pass: AU memoization + histogram early-exit (2026-07-02)

Profile (warm 500k scan, this host, back-to-back): 10.2 s total; near-verify
3.23 s of which histogram-only accounts for 0.51 s — anti-unification
dominated, and inspection showed `au_list` recomputing `merkle` +
`collect_hashes` (full subtree hashing) for **every DP cell** of the graded
Smith-Waterman similarity matrix.

Changes (both provably output-identical):
1. **Per-call AU memo** (`Ctx::node_info`): exact merkle + sorted MaskedAll
   subtree-hash multiset per input node, keyed by node address (input trees
   are borrowed/immovable for the call). Hole token counts and machinery
   checks read the same memo (multiset length IS the D1 token count).
2. **Histogram early exits:** return true the moment `best ≥ needed`; return
   false when `best + min(remaining_a, remaining_b) < needed` (each remaining
   shared hash adds ≤1 vote per bin under the D22 distinct-vote rule).

Result: warm 500k scan 10.2 → **8.1 s** (near-verify 3.23 → 1.14 s); warm
`check --base HEAD` **8.1 s** against the D27 ≤10 s gate. **Parity evidence
(the D27 method, `benches/compare/pair_ab.py`):** serde 1526/1526 and ripgrep
2155/2155 reportable member-pairs identical vs. the pre-change binary, zero
lost, zero demoted; 117/117 tests green.

**The original 5 s line is NOT reachable losslessly:** zeroing the entire
remaining near tier would still leave ~5.4 s (sequence 2.6 s — SA
construction is already early-terminating prefix-doubling with parallel
sorts; inline 1.0 s and extract 0.8 s — both already parallel). Banded S-W
was considered and skipped: verify is no longer the bottleneck and banding is
not provably lossless (alignments leaving the band change divergence).
Reaching 5 s needs algorithmic swaps (SA-IS; incremental def tables), out of
scope for a polish pass under the D27 constraint. Gate standing: ≤10 s with
zero recall regression — met with margin.

## D33 — M4b publish preparation: license placeholder, name-check, metadata, exit codes (2026-07-02)

Phase-4 milestone M4b is docs/polish/publish **preparation only** — no
`cargo publish`, no git tags, no push. Decisions made while making the crate
publish-ready:

- **License is UNCONFIRMED — COORDINATOR MUST ESCALATE.** The Cargo.toml carries
  `license = "MIT OR Apache-2.0"` as the **Rust-ecosystem default PLACEHOLDER
  only**. This is a significant decision the user must approve before any publish.
  No `LICENSE`/`LICENSE-MIT`/`LICENSE-APACHE` files were added (adding them would
  imply a confirmed choice). **Blocker for publish:** confirm the license with the
  user, then set it and add matching LICENSE files. A `# PLACEHOLDER` comment sits
  directly above the key in Cargo.toml.

- **crates.io name-check (re-verified per D-eval note and spec Rev 8).**
  `cargo search reprise` on 2026-07-02 returns **no crate named `reprise`** — the
  three results (`constitution`, `embassy-bmp280`, `kalman-fixed-agnostic`) are
  fuzzy matches, none is the name. The name is **still free**, consistent with the
  spec's Rev 8 claim and the M0 environment note. (Not reserved/held — first
  publish claims it.)

- **Publish metadata added:** `description`; `keywords = [clone-detection,
  duplicate-code, tree-sitter, code-quality, static-analysis]`; `categories =
  [development-tools, command-line-utilities]`; `readme = "README.md"`.
  `repository`/`homepage` are `https://github.com/OWNER/reprise` **placeholders**
  with a `TODO(publish)` comment — real URLs are a pre-publish step (no repo host
  is decided).

- **`exclude` decisions (verified with `cargo package --list` — local only, no
  publish).** Excluded: `benches/**` (planted-clone calibration corpus + wild net
  + perf/compare harnesses — not shippable crate content), `examples/**` (internal
  debug probes: `probe.rs`, `dbg_*.rs`, `debug_near.rs` — they use the crate for
  debugging, they are not user-facing usage examples), `reprise-baseline.json` and
  `.reprise/**` (dogfood/cache state, meaningless downstream; `.reprise/` is also
  auto-ignored by cargo). **`docs/PLAN.md` stays IN** — it is the authoritative
  spec and the README links it. Result: **package drops from 120 → 53 files**, no
  benches/examples/baseline/.reprise, `docs/PLAN.md` retained. Note: shipped
  `tests/` reference `benches/` fixtures only at **runtime** (via
  `CARGO_MANIFEST_DIR`), never `include_str!`, so the crate still **compiles**
  without benches/ (verification build is unaffected); downstream simply can't run
  reprise's own fixture tests, which is expected.

- **Exit-code split implemented (spec §8 / D20f: "implement the 2 distinction if
  trivial").** It was trivial and is now done: `main` maps any `anyhow` error to
  **exit 2** (usage/runtime error), reserving **exit 1** for genuine `check`
  findings at/above `fail_on`, with **0** clean. clap's own argument errors already
  exit 2, so the three codes never collide. Previously bad-`--format`/bad-git-ref
  errors shared exit 1 with real findings — now disambiguated. Verified:
  bad-format/missing-arg/bad-git-ref → 2; check-with-findings → 1; clean → 0. The
  `check --help` epilogue documents the three codes.

- **CLI help polish:** every `<PATH>` arg now has help text; flag help strings name
  their config-file equivalents (`--top` → `[report] top`, `--fail-on` → `[report]
  fail_on`, etc.); `--version` verified (`reprise 0.1.0`, from `CARGO_PKG_VERSION`).

- **Dogfood baseline written:** `reprise baseline .` at repo root produced
  `reprise-baseline.json` (305 findings: 233 main / 60 test / 6 api / 6 weak),
  capturing the repo's known duplication — chiefly the five deliberate parallel
  `src/lang/*.rs` profiles. **`check . --base HEAD` does NOT work yet:** the repo
  has a `.git` but **no commits** (`git rev-parse HEAD` → "Needed a single
  revision"), so there is no `HEAD` to diff against. Per the milestone's
  do-not-commit rule, this is left as-is and documented in the README development
  section; `check` will work once the repo has its first commit.

## D34 — License confirmed: BSD-3-Clause; static-link install targets (2026-07-02)

User-approved license: **BSD-3-Clause** (supersedes the D33 placeholder). `LICENSE`
file added (copyright holder inferred from the user's address as "Ben Abbitt" —
correct the name in LICENSE if wrong); `Cargo.toml` license key set; README updated.

`just install` (cargo install --path) and `just install-static`/`build-static` added.
Static linking uses `-C target-feature=+crt-static` on `x86_64-unknown-linux-gnu`,
producing a static-pie binary verified to run the self-scan — chosen over the musl
target because the tree-sitter grammar crates compile C and this host has no musl C
toolchain; the musl route (`rustup target add x86_64-unknown-linux-musl` + musl-tools)
is documented in the justfile comment as the alternative where available. Note the
standard glibc/crt-static caveat (NSS/dlopen unavailable) — irrelevant to this CLI.

## D35 — Repository host: github.com/ctxloom/reprise (2026-07-02)

User decision. Cargo.toml repository/homepage set; README clone URL set. Initial
push as a private repository (visibility is the owner's call to flip). Session/tool
state (.claude/, .ctxloom/, .mcp.json, .codex/) added to .gitignore before the first
commit so no local agent state ships.
