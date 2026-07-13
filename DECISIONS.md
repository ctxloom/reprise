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

> **Still not built; the premise narrowed.** The bag layer is since deleted (D48), so retrieval is
> landmark alone — a vector/LSH layer would be a *second* index, not a third. The decision is
> unchanged: no demonstrated recall gap.

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

> **Partially superseded by D48.** The hole-context verdict below stands. But this rivalry only asked
> which *challenger* to keep **beside** the bag-Jaccard layer — it never asked whether the **bag
> itself** earned its keep. It did not: measured later, its unique verified yield was **0** on every
> corpus, and it is now deleted. Retrieval is the landmark retriever **alone** — there is no candidate
> union. Do not read the "bag + landmark" configuration below as current.

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
  **(Closed by D43: const-bound arrows/function-expressions are now extracted.)**
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
(**Superseded by D42**: the static-link path is now musl-static via cargo-zigbuild;
the glibc build didn't run on older images. The license decision here still stands.)

## D35 — Repository host: github.com/ctxloom/reprise (2026-07-02)

User decision. Cargo.toml repository/homepage set; README clone URL set. Initial
push as a private repository (visibility is the owner's call to flip). Session/tool
state (.claude/, .ctxloom/, .mcp.json, .codex/) added to .gitignore before the first
commit so no local agent state ships.

## D36 — Report-quality feedback round (2026-07-02)

User feedback from a real scan (ctxloom/main): (a) cross-tier membership-subset
groups (an inline-assisted superset alongside its plain-tier subset) now dedupe —
survivor keeps the larger membership at its own tier, annotated when the dropped
subset carried a stronger tier (`group::dedupe_subset_groups`, unit-tested with the
reported shape); (b) the summary line reports `exact-region: N substantial of M`
using `report.micro_region_tokens` (default 60) so the headline reflects what the
ranking values instead of the micro-region long tail.

## D37 — Check-mode noise fixes from field feedback (2026-07-02)

Real-usage feedback (lefthook pre-commit wiring): (a) `check` now applies the
`report.micro_region_tokens` floor to exact-region emission — micro-regions (4-line
map-copy idioms shared across unrelated functions) stay in `scan` output but are
noise in exactly the mode an agent reads at commit time; this also covers the
reported prod-unit↔4-line-test-fake pairing, which entered as a micro region (a
4-line fake cannot clear the 40-token unit floor). (b) Units whose body is a single
top-level statement (pure delegation wrappers) get no inline variant: expanding one
folds a freshly-extracted helper back in, making reprise flag its own recommended
fix pattern. Real reimplemented-helper callers have surrounding code by definition.
(c) `check` without a baseline file prints an adoption hint (run + commit
`reprise baseline .`) since every finding otherwise reports as new.

**D37 addendum:** the thin-delegation rule relabeled wild fixture
`w8_ripgrep_convert` (M4a sample TP → by-design non-finding): `convert::usize`/`u64`
are single-statement wrappers around the already-extracted `str` helper — the same
shape as the field-reported fix-pattern noise. The fixture stays in the corpus as a
NEGATIVE (must-stay-silent) case, pinning the suppression. This is one hand-labeled
TP consciously traded for the remedy-pattern suppression; net field precision wins.

## D38 — Two-scan is check's default; baseline demoted to optional, transient-by-default curation (2026-07-02)

User decision ("I'm not a fan of storing derivative artifacts in VCS"; "yes, two
scan"; "though that file may live as a transient"). `check` without a baseline file
now synthesizes base state by scanning the base ref in a temp git worktree (reusing
the content-keyed cache via `cache.shared_root`, so unchanged files are warm hits)
and caches the synthesized state transiently under `.reprise/base-state/<sha>-<cfg>`.
Adoption therefore needs no artifact: pre-existing duplication is exempt, and
inconsistent-update/worsened findings fire from base state. The baseline file remains
as an opt-in curation layer — it uniquely provides a FIXED drift reference (cumulative
drift across PRs, vs two-scan's per-PR moving reference) and a reviewable/editable
acceptance set — and its default path moved to the untracked `.reprise/baseline.json`
(spec §9 said repo root; deviation recorded here). The repo's own committed
reprise-baseline.json was removed; reprise dogfoods two-scan mode in CI.

**D38 addendum / D39 — IU drift tracking is unit-granularity (+ span-precise regions).**
First CI run of two-scan mode failed on our own commit: 8 inconsistent-update
findings, all pairing the 450-line `scan()` with small functions — exact-region
entries whose members were mapped to their ENCLOSING UNIT spans, so any edit
anywhere in a large function "touched" every idiom-run it contains. Fix: IU tracks
unit-granularity tiers (exact/near/inline-assisted) with unit mapping; exact-region
entries fire only when the diff intersects the run's own span (span-precise);
internal-repeat never qualifies. Regression test: tail edit beside a shared run is
silent, in-run edit fires. Region IU reports but does NOT gate (informational):
the Juergens evidence is function-level clones, and a legitimate one-sided change
to a shared run has no acceptance path in artifact-free mode — unit-tier IU
remains the hard gate.

## D40 — The baseline is a git ref; the baseline file is gone (2026-07-02)

User decision: "baseline will never exist as a persistent, tracked file. they can
specify a git hash for a persistent baseline and rescan." The `reprise baseline`
subcommand and `[baseline] file` are removed. A persistent acceptance point is a
pinned ref — `[baseline] ref = "<sha|tag>"` (the default for `check` when `--base`
is omitted) — and base state is always (re)scanned from the ref via the D38
worktree path, with the transient `.reprise/base-state/` SHA-keyed cache. The
`Baseline` type survives solely as that cache's serialization; a scheme mismatch
is treated as a cache miss (rescan), no longer a user-facing re-baseline error.
What the file bought (D38) maps onto refs: fixed drift reference = pinned ref;
"moving the acceptance point" = an ordinary reviewable one-line pin bump. The one
capability with no ref equivalent — surgically un-accepting a single group while
keeping the rest — falls to `reprise:ignore` pragmas or an actual cleanup.

## D41 — Field-feedback round two (2026-07-02)

(a) **`reprise:accept-drift` pragma** — the missing per-finding IU exemption
("the big one" in the report). A file-based `reprise exempt` store would
reintroduce the derived artifact D40 killed; a source pragma is reviewable,
co-located, and survives refactors. Semantics: the unit stays covered by every
tier; IU findings whose TOUCHED members all carry the pragma report as info.
(b) **`git archive` fallback** for base-state extraction: `git worktree add`
failed in the field on a multi-worktree clone (".git/index: Not a directory",
unreproducible); archive reads only objects — no per-checkout admin state.
Caveat recorded: archive honors export-ignore, so the fallback path can omit
attribute-excluded files. (c) **Verbose source dumps**: `scan --verbose` and
`check --verbose` print each member's actual source (line-numbered, capped)
under its reference. (d) Report items 3 (check substantiality floor,
prod-vs-test-fake pairing) were already fixed in D37/D39 — the field binary
predated them; item 4a (`reprise baseline` arg errors) is moot post-D40.
(e) Follow-up recorded, not built: rare-token (IDF) substantiality floor so
ubiquitous-idiom runs (getFS/result/warnings-class) don't reach reports even
at length — needs calibration against the M4a sample method.

## D42 — Linux ships musl-static (supersedes D34's glibc static-pie); mimalloc on musl (2026-07-02)

> **The allocator half of this decision is SUPERSEDED by D49** (jemalloc on every target but
> Windows). The *finding* below stands unchanged — musl's default allocator is ~16x slower under the
> parallel scan — but the **musl-only gate** was wrong, and D49 explains why. The musl-static
> shipping decision (the other half of D42) still stands.

Supersedes the static-link half of D34. The shippable Linux binary is now
**musl-static via cargo-zigbuild**, not glibc `+crt-static`. Trigger: a glibc
binary built on a newer-glibc host (this dev host, trixie) links the host's
glibc symbol versions and **fails to run on an older image** (Debian bookworm) —
caught in the real agent-image build, whose gate now drops an incompatible
companion with a loud warning rather than failing the whole image. A fully
static musl binary has no libc dependency at all (verified: `ldd` → "not a
dynamic executable", no GLIBC symbols, plain SYSV ELF), so it runs on any Linux
regardless of glibc — kernel floor only.

D34's reason for avoiding musl ("this host has no musl C toolchain" for the
tree-sitter grammar C) no longer holds: **zig (via cargo-zigbuild) supplies the
musl C toolchain**, so `cargo zigbuild --target x86_64-unknown-linux-musl`
compiles the grammars and links static with no musl-tools install. `build-static`
/`install-static` and the release targets (`.goreleaser.yaml`,
release-completer, release-dryrun) switched from `*-linux-gnu` to `*-linux-musl`;
os/arch labels are unchanged (libc isn't in the name), so archive/cask names and
the tap URLs stay `reprise_<ver>_linux_<arch>`. macOS is not glibc — left as-is.

**Allocator, the catch:** musl's default allocator is pathological under
reprise's rayon-parallel, allocation-heavy scan — **~16x slower than glibc**
(77–80 s vs 4.7 s warm on the 500k perf corpus), which blows the ≤10 s gate
(D27/D32) by 8x. Fix: **mimalloc as `#[global_allocator]`, gated to
`cfg(target_env = "musl")`** (mirrors ripgrep's musl-jemalloc precedent). glibc
and macOS keep the system allocator (no dep, no change — mimalloc is absent from
their dep graph). Result: musl+mimalloc **3.4 s** warm — under the gate and
faster than the glibc build itself (a 23x swing from the one allocator line).

## D43 — TS `const f = () => …` extracted as a unit; Go/TS/Kotlin recall calibration (2026-07-03)

Closes the D25 gap "TS arrow/function assigned to a const is not extracted — is
invisible." A new `LanguageProfile::binding_unit` hook (default None) lets a
language extract a named callable bound in a declaration; TS implements it for a
`variable_declarator` whose value is an `arrow_function`/`function_expression`,
returning (name, callable). `collect_units` extracts it via the shared
`push_unit`. Decl-only by construction: inline callbacks (`xs.map(x => …)`) are
not variable_declarators, so they are never extracted (matches Rust/Python
decl-only extraction; verified by test). Extraction output changed for every TS
file, so **`cache::EXTRACTION_VERSION` bumped 2→3** (the D30 stale-cache lesson).

Scope caveat (v1): the unit root for a const-arrow is `arrow_function`, so it
converges with other const-arrows/function-expressions but NOT yet with a
top-level `function f(){…}` declaration (different root kind). Arrow↔declaration
cross-matching is a follow-up. Verified end-to-end: two duplicated const-arrow
functions now form an exact-normalized group (were invisible before).

**Calibration (audit follow-up).** The audit found Go/TS/Kotlin had NO recall
gate — Phase-2/3 mutation variants existed only for rust+python. Added seed1
`t3-loop-swap` and `t3-subtree-sub` variants for all three; both classes now
gate at 5/5 across five languages. Surfaced a real capability gap while doing it:
the `while (i < n) { x = a[i]; i++ }` manual-index loop form canonicalizes for
Rust but NOT for TS/Go/Kotlin (their `rewrite_iteration` handles the three-clause
`for`/`for i in indices` index forms, not while-with-manual-index). The
loop-swap variants use each language's handled index form; the while-index gap is
recorded here as a follow-up, not yet built. Wild-corpus pairs for TS/Kotlin
remain unbuilt — real-repo provenance labeling (the M4a method) is a separate
exercise and was deliberately not fabricated.

## D44 — Substrate stays source-via-tree-sitter + purpose-built IR; execution bytecode rejected (2026-07-03)

Design discussion, not a code change: whether to shrink the five parallel
`src/lang/*.rs` profiles by moving the normalization substrate off tree-sitter
**source** onto an existing lowered/desugared form — JVM bytecode (Java/Kotlin),
CPython bytecode (Python), CIL (C#) — read either directly or as textual
disassembly (`javap -c`, `python -m dis`) parsed by a tree-sitter grammar.
Recorded per the §8 no-silent-architecture rule and the D16 habit (weigh a rival
substrate on the merits, put the verdict on the record).

**Motivation acknowledged — the instinct is right about the smell.** The
normalization phase (§5.2: loop lowering, tail-recursion→loop, iteration-protocol
rewrite, positional identifier abstraction, literal bucketing) is a hand-grown IR,
and its per-language rewrites are exactly where the standing self-scan duplication
lives (the `lower_recursion`/`reassign_stmts` families, D43). Those rewrites
reimplement, per language, desugarings a compiler already performs: `lower_loops`
synthesizing `__has_next`/`__next` is literally CPython's `FOR_ITER`; positional
locals are literally bytecode slot numbers. "Stop hand-rolling an assembler and
borrow a real one" correctly names what reprise is accreting.

**Verdict: substrate stays.** Four properties reprise is *defined by* are each
broken by an execution-bytecode substrate:

| Property | Broken how |
|---|---|
| **Buildless / works on broken code** | Bytecode needs a successful compile of the whole project with resolved deps. The flagship `check` runs at PR time on frequently-non-compiling LLM-agent code and on arbitrary file subsets; tree-sitter parses one broken file in isolation. Product-defining — disqualifying on its own. |
| **Tree-structured evidence** | A method's bytecode is a flat instruction stream; control flow is jump labels, not nested subtrees. Subtree hashing, the offset-histogram diagonal (§5.6), and anti-unification templates all need structural nesting. Lowering collapses matching toward jscpd's linear-window regime — the thing reprise beats (§7.3) — and degrades the flagship source-shaped template into "a run of ops with holes." |
| **Toolchain-deterministic baseline** | Same source, different compiler/version/opt-level → different bytecode. CPython bytecode churns every minor release (3.11's specializing adaptive interpreter reworked it wholesale). A drift baseline (D40) that shifts when CI bumps the toolchain is dead. |
| **Uniform matrix coverage** | Only Python (CPython) and Kotlin (JVM) of the five current languages have a clean stable pseudo-assembly; Java is the only one of four planned. Rust→MIR(internal)/LLVM-IR(opt-soup), Go→SSA(internal), TS→V8(internal), C/C++→none structured. Forces a hybrid pipeline whose halves aren't comparable — more architecture, not less. |

**The "disassemble, then tree-sit it" variant does not rescue it.** Parsing
`javap`/`dis` text with a tree-sitter grammar recovers the *uniform frontend* —
but uniformity was never the problem (`NormNode` and the whole back half are
already language-agnostic; that is the existing win). The four properties above
are untouched by *how* the bytecode is parsed, and the CST of a disassembly is a
flat instruction listing — nesting is still jump labels, so the tree tier still
collapses. Recovering structured control flow means heuristic decompilation,
which rebuilds the source AST you started from, minus fidelity.

**Honest counterweight (why the idea keeps looking attractive).** For VM-family
languages the multiplier is real: Java/Kotlin/Scala/Groovy all disassemble to
JVM, so one grammar + one profile would cover the ecosystem; C#/F#/VB → one CIL.
That is genuine N→1 consolidation — but only purchasable at the cost of the four
properties, and same-language-only matching (§3) means cross-family unification
isn't even a goal reprise would spend them on.

**Direction if the profiles are reduced (a separate decision, not taken here):**
author the IR, don't borrow it. A purpose-built neutral normalization IR —
desugared, inspectable, textual for golden tests, tree-shaped exactly as clone
detection needs — *emitted from tree-sitter source* keeps every ergonomic (one
lowering target, controlled desugaring, one home for the rewrites) with none of
the four poisons, because it stays buildless, deterministic, and source-mapped.
§5.2 already half-specifies that instruction set. The smaller near-term step —
lifting the `lower_recursion`/`reassign_stmts` family out of the profiles into
shared generic functions parameterized by a per-language shape — is a
cross-profile consolidation and gets its own sign-off before it lands (it changes
topology across five deliberately-independent units).

**Escape hatch preserved.** Bytecode's one genuine fit in reprise is an *opt-in,
requires-build, deep-Type-4 tier* for JVM/CLR languages — semantic-clone
detection that sees through source-level obfuscation, added alongside the
source-first core, never replacing it. Bytecode-based clone/plagiarism detection
is an established technique for exactly that setting (compiled, whole-repo, fixed
toolchain); it is a different tool's default, not reprise's.

## D45 — Similarity IR: design promoted, P1 underway (2026-07-03)

D44 rejected borrowing an execution IL and pointed at a purpose-built IR "designed,
not merely chosen." That design is now written up as the authoritative
**`docs/SIMILARITY-IR.md`** (the D-IR-1…10 register lives there), and P1 implementation
is beginning on branch `similarity-ir`. What is decided vs. what P1 measures:

- **Scope.** Greenfield the *front half* — an IR node-set + per-language frontends
  lowering tree-sitter CST **directly to a canonical form**. The matching back half
  (`fingerprint`/`seq`/`tree`/`au`/`inline`) is preserved unchanged — it is not where
  the duplication lives. tree-sitter stays the sole frontend (buildless, D44).
- **The win.** The front-half algorithms currently copied across the five
  `src/lang/*.rs` profiles (loop lowering, recursion/tail, iteration rewrite, order
  canonicalization) move onto the IR, written once; the standing self-scan findings
  (`lower_recursion`/`reassign_stmts`) die by construction.
- **Resolved.** Node-set is a per-language shared *skeleton*, **not** a universal
  cross-language vocabulary — the trap that archived source{d} Babelfish and GitHub
  `semantic`; §3 same-language-only makes reprise immune (the shared vocabulary exists
  for algorithm reuse, never cross-language matching). `Branch` unified
  (if-chains/match/switch/ternary + value-producing branched-assignment; a dispatch
  table becomes a *recognized shape* of `Branch`, retiring the per-language
  `is_dispatch_arm` hook — D30 goes language-agnostic). Decomposition normalization via
  **ANF named intermediates** (not collapse-to-nested — the `(data,err)` multi-return
  case flipped it). The artifact is **lossless** (normalization *relocates* discriminators
  into per-transform witnesses, does not destroy them); transforms are reversible **for
  display only** (no compile-from-IR). The transform log is an append-only event stream
  held **in memory, never persisted** (excluded from the D19 cache) — the CQRS/ES *model*,
  not its infrastructure.
- **Measured in P1, not asserted (the D-IR register).** Container choice
  (same-`NormNode`-container vs typed enum), ANF naming granularity, e-graph threshold,
  fallthrough/arm-sort, and every ⚠ node-set boundary — all gated by `bench-mutations`
  + the §7.2 precision sample + `pair_ab` parity vs the current binary. The transform
  log doubles as the per-transform recall/precision **attribution harness**.
- **Rollout.** P1 = IR + Rust/Python frontends behind a flag, parity-gated; P2 =
  TS/Go/Kotlin; P3 = retire the `LanguageProfile` normalization hooks (bumps
  `FINGERPRINT_SCHEME`/`EXTRACTION_VERSION`, D19/D30).

## D46 — LSP & MCP server surfaces promoted in-scope; the "CLI + CI only" non-goal reversed (2026-07-03)

The spec (docs/PLAN.md §non-goals) said "a daemon/server/IDE plugin — CLI + CI only"
and "do not build a daemon." That is **reversed here**: two server surfaces move in-scope,
designed in the authoritative **`docs/SERVERS.md`** (the D-SRV-1…7 register lives there).
The reversal is deliberate, not accretion — the agent thesis pulls **MCP** in (reprise's
founding use case is agents reimplementing helpers; MCP lets the agent consult reprise
mid-write), and multi-editor reach + the live drift guardrail pull **LSP** in.

- **One substrate, two projections.** Neither server is a source of truth: both project the
  existing serde report model (`ScanReport`/`CheckReport`). The SARIF emitter already does
  the exact LSP projection (one finding → N `relatedLocations`, structural-hash
  `partialFingerprints`), so the model work is largely done. `scan()`/`check::run()` are
  already printing-free, exit-free lib calls — v1 needs near-zero core change.
- **Resolved (D-SRV register in docs/SERVERS.md).** Separate workspace crates
  (`reprise-mcp`/`reprise-lsp`/thin `reprise-server-core`); the core lib + static CLI stay
  **sync and tokio-free** (server deps `rmcp`/`tower-lsp` never enter the CLI binary). v1 is
  **stateless-per-request** — hold the warm D19 cache, re-run scan/check debounced (warm
  real-repo scans are sub-second, so a full re-scan is viable). LSP analyses on `didSave`.
  `find_similar` (MCP, on-thesis) is **append-and-rescan** over existing primitives
  (`units_from_source` + `au::anti_unify`), not new matching machinery. Build order: MCP and
  LSP **in parallel** after a shared-core M1.
- **Deferred, latency-gated (§6).** A held in-memory index + incremental single-file
  re-match — the only part that collides hard with the old non-goal — is built **only if a
  real repo trips a written latency trigger**. It also unlocks dirty-buffer
  live-as-you-type and the affordable live `inconsistent-update` guardrail (the standout LSP
  feature). Per the D-IR-4 driving-case discipline: no daemon-with-index until measurement
  demands it.
- **Relationship to D45 (the IR effort, same branch).** Orthogonal by construction. The
  servers depend on the **stable lib API** (`scan`/`check::run`/the report model) and the
  **preserved back half** (`au`/`group`/`fingerprint`) — both of which D45 §3 keeps
  unchanged. The IR rewrites the *front half* (`LanguageProfile` lowering) *behind* those
  API signatures, so server code is insulated. The one avoidable collision — factoring
  `scan()`'s extract half into a corpus-units accessor (M1) — is **deferred/coordinated with
  the IR work**; v1 `find_similar` uses `units_from_source` as-is instead. Server warm caches
  key on `FINGERPRINT_SCHEME`/`EXTRACTION_VERSION`, so the IR's scheme bump invalidates them
  automatically (D19) — no special transition handling. **Numbering note:** if the IR agent's
  next entry also claims D46, reconcile at commit (agents don't cut releases/commits here).

## D47 — Substantiality via landmark-density + IDF, not token size (novel direction; 2026-07-05)

The precision floors (`min_unit_tokens` 40, `min_seq_tokens` 30, `histogram_min_votes` 5,
`fold_min_repeats`) all gate on a **raw count** as a proxy for "enough distinctive structure to trust a
match." Two faults the D45 IR work made unavoidable: **(1) representation-dependent** — IR trees are ~18%
more compact, so every floor grew a per-normalizer twin (`min_unit_tokens_ir=33`, `histogram_min_votes_ir=4`,
each = original × 0.815); the knobs multiply with every representation (a third frontend / grammar bump
re-opens the calibration). **(2) crude** — raw count can't separate a substantive 25-token unit (w7's
chunked-I/O loop) from a trivial 25-token idiom (plumbing), forcing the Decision-4 (w7 / `min_seq_tokens`)
recall-vs-precision either-or that has no answer in the size domain.

**Decision (direction; not yet implemented).** Replace the size floors with a representation-invariant
substantiality score `f(distinct_landmark_count, idf_content)`, computed from quantities reprise **already
builds for retrieval** — the Shazam landmark constellation (§5.5.4; peaks = rare low-DF subtrees at
structural offsets, `matchtree.rs:143-176`) is a rarity-weighted structural-richness signal with
boilerplate excluded by construction. Because a unit's rare-peak count does not scale with raw token count,
`K` calibrates **once** for all normalizers (retiring the `_ir` twins), and it separates substantive-small
from trivial-small directly (recovers w7 while rejecting its ~134 plumbing look-alikes — the recall *and*
precision size floors make mutually exclusive). Subsumes the `elder-wow` IDF-substantiality item; the
landmark layer already shares the §5.7 IDF machinery (`api.rs:102`).

**Non-optional companion — the gate justifies itself.** Ships with diagnostics that report its **marginal
usefulness every scan**: units it decides differently than the token floor (recoveries / rejections vs the
size gate), the TP/FP split of that marginal set, and the score's TP-vs-FP separation — modeled on the
§7.4b retrieval-rivalry measurement (drop-the-loser), so a future representation change that quietly makes
it useless surfaces in the readout instead of hiding.

Full design + plan + caveats (corpus-relative rarity; match-time locus): **docs/substantiality-metric.md**.
Post-flip effort (the D45 default-normalizer flip lands first). Numbering: reconcile D46/D47 at commit.

**MEASURED NO-GO (2026-07-05, `fond-mute` measure-before-implement).** The score was prototyped against the
exact landmark/IDF machinery over realistic corpora on a 9-pass/9-fail labeled set. It **fails**:
**landmark_count is a size proxy** (r=0.994 with token_count → same AUC 0.654 as the baseline, same ~0.79
IR/historical shift → NOT representation-invariant), **structural IDF saturates** (nearly every ≥6-token
subtree is unique → idf≈max), and **the premise is empirically false** — plumbing runs have *as many* rare
peaks as (and IDF ≥) genuine small clones (`is_self_call` has `max_df=139` — it *contains* the corpus's
single commonest subtree). Root cause: substantive-vs-trivial-small is a **semantic/authorial** judgment,
not a structural-rarity property the landmark/IDF layer can observe. **DECISION: keep the per-normalizer
token floors (`min_unit_tokens_ir`, `histogram_min_votes_ir`) as-is; do NOT swap. w7 stays a Decision-4
hold-out; `elder-wow` is NOT subsumed.** A different axis (shared-fragment corpus-recurrence df, or callee
semantics) would be a new design. Full data: docs/substantiality-metric.md §0.

---

## D48 — The bag-Jaccard candidate layer is DELETED; retrieval is landmark alone (2026-07-11)

Near-tier retrieval was a **union of two candidate layers**: bag-Jaccard over the floor-6 subtree
hash set (`bag_set`, spec §5.5.2, thresholded by `candidate_sim`) and the landmark constellation
retriever (§5.5.4, the D16 rivalry winner). The union is gone. **The landmark retriever is the sole
source of near-tier candidates.**

**Measured, on every corpus (self/fs/net):**
- `candidates_total == candidates_landmark` **exactly** — the bag layer's candidate set was a strict
  *subset* of landmark's. It never proposed a pair landmark did not.
- `verified_only_bag = 0` — zero unique verified yield, everywhere.
- Disabling it left **every tier's group set byte-identical**. Output does not change.

The failure mode it nominally existed for — a clone family so large that its subtrees stop being rare,
starving landmark's rarity gate — **does not occur**. What the layer actually was: an unswitchable,
always-on, corpus-wide par-sort join over `bag_set` that contributed nothing, for the life of the
project.

**What survives.** `bag_set` itself — it is still a `UnitDigest` field and, by default, the
**document-frequency substrate** the landmark rarity gate is computed over (see
`retrieval.landmark_df_over_offsets`). **Only the JOIN dies.** So `bag_min_subtree_tokens` is still
live and still behavior-determining — it decides which subtrees are rarity-tested at all — it is just
no longer a *retrieval* threshold. `candidate_sim` is now read by nothing in the core; it survives
only because `reprise-mcp` reuses the value as an unrelated size-compatibility ratio (that coupling is
accidental and should get its own key).

**Consequences.**
- The §0.3 coverage gate is now **unconditional**. It was previously softened by the union: a pair the
  bag also proposed re-entered the candidate set independently and so survived a coverage drop, and
  that exemption was documented as the source of the gate's recall-safety. It was not — with every
  pair coverage-exposed, group sets are byte-identical.
- Stats `candidates_bag` and `verified_only_landmark` are **removed**: with one retriever,
  `candidates_total == candidates_landmark` and `verified_only_landmark == verified_pairs`, both by
  construction.
- The landmark layer's former hard-coded constants become config, all defaulting to the previous
  values so the default path stays byte-identical: `retrieval.landmark_fan_out` (3),
  `landmark_rare_floor` (3), `landmark_rare_divisor` (20), `landmark_rare_cap` (unset = derive), and
  `landmark_df_over_offsets` (false).

**Why this went unnoticed for so long — the lesson, and the invariant it buys.** The D16 rivalry
measured which *challenger* (hole-context vs landmark) to keep **beside** the bag. It never asked
whether the **incumbent** earned its own keep, and no counter existed that could have answered:
`verified_only_bag` did not exist. **INVARIANT, now enforced in `matchtree.rs`: any second candidate
layer added here MUST ship with a unique-yield counter.** A layer whose marginal contribution over the
layers it runs beside is not a live, per-scan number cannot be shown to earn its keep — and an
always-on join that yields nothing is indistinguishable from one that yields everything, until someone
counts.

Supersedes the retrieval-union half of D16 (its hole-context verdict stands). Spec §5.5.2/§5.6 updated
(PLAN.md Rev 10); CALIBRATION.md §7.4(b) annotated.

---

## D49 — One allocator: jemalloc on every target but Windows (supersedes D42's musl-only mimalloc gate; 2026-07-11)

D42 made mimalloc the `#[global_allocator]` **gated to `cfg(target_env = "musl")`**. glibc and macOS
kept the system allocator. D42's *finding* was right and is unchanged: musl's default allocator is
**~16x slower** than glibc under reprise's rayon-parallel scan (77 s vs 4.7 s on the 500k perf corpus).
**The gate was the mistake.**

**What the gate cost: a 6.8 GB dev/prod skew.** Dev, CI, and release-on-glibc all ran plain glibc
malloc and peaked at **22.7 GB** on `drivers/`; only the shipped musl binary got a fast allocator and
peaked at **15.8 GB**. Every Tier-2 memory budget was reasoned from a profile that **no shipped binary
had**. A whole optimization campaign was aimed at a build no user runs — this is the entry CLAUDE.md's
first rule cites.

**glibc has its own, independent allocator problem** (this is the second, separately-measured reason —
not a restatement of D42's): glibc malloc **cannot give back the trees the memory gate frees**.
Extraction packs ~12.7 GB of `NormNode` (64 B each) into rayon's per-thread arenas as *millions* of
small chunks; the gate spills and frees the trees, leaving ~8.7 GB of free chunks (18.3 M of them at
`drivers/` scale) scattered **mid-arena** — so almost no page is wholly free and `malloc_trim` reclaims
~nothing. The near tier then demands **large contiguous** Vecs (the landmark entries/events), which
exceed the mmap threshold and are served by **fresh mmap** rather than out of that 8.7 GB. The freed
memory is the wrong *shape* to satisfy the new demand, so both are resident at once. jemalloc's
arena/extent reclaim returns it: **peak RSS 22.70 → 15.37 GB (−7.33 GB, −32%) and wall −29%** at
`drivers/` scale, interleaved. Output identical.

**DECISION: jemalloc (`tikv-jemallocator`) as `#[global_allocator]` on every target except Windows.**
jemalloc measured strictly better than mimalloc on both `drivers/` and `fs/`, with no fs-scale
regression, so it replaces mimalloc *everywhere it can* rather than merely closing the musl gate.

**Windows keeps the system allocator — not by choice.** `tikv-jemalloc-sys` does not build for
`x86_64-pc-windows-gnu` (reprise's shipped Windows target): "could not find native static library
`jemalloc`", mirroring long-unresolved upstream Windows gaps in jemalloc itself (MSVC explicitly
unsupported; `-gnu` has an open, unmerged fix attempt). This is a hard build failure, not a judgment
call. Consequence to remember: **a Windows memory figure is not comparable to any other target's.**

**The general rule this buys:** *the binary you measure must be the binary you ship.* A per-target
allocator gate silently forks the memory profile, and the fork will be discovered — expensively — only
after budgets have been built on the wrong branch of it.

---

## D50 — The inliner's SCC bypass is rationed (`max_scc_depth`), with an aggregate node backstop (2026-07-11)

`resolve_policy` exempted mutual-recursion **SCC partners from BOTH `max_depth` and
`max_callee_tokens`**. The intent (spec §5.4, and the inline module's own doc) was always "inline each
SCC partner **once**" — but nothing enforced the *once*, so the exemption was effectively unbounded.

**What that cost.** One redis unit expanded to **25.1 M nodes — 22× the entire corpus** — and exhausted
the machine's memory. And the size cap was not only a cost cap: **`max_callee_tokens` is also the
precision guard.** Skipping it splices a large shared helper's mass into every caller and *manufactures
false clones out of the shared bulk* — observed concretely in btrfs, where `check_system_chunk` and
`btrfs_reserve_chunk_metadata` were reported as clones purely on the mass of the 74-line
`reserve_chunk_space` they both call. A cap that is bypassed for cost reasons silently takes a
precision guarantee with it.

**DECISION — ration the bypass, and backstop the unit:**
- **`inline.max_scc_depth` (default 1).** The SCC bypass still exists (an SCC partner *must* be able to
  bypass the caps or mutual recursion cannot be converted to self-recursion at all), but it is bounded:
  at most `max_scc_depth` SCC-partner splices may be on the stack. Default 1 = exactly the "once" the
  module always documented.
- **`inline.max_expansion_nodes` (default 250,000).** A pure aggregate per-unit backstop, charged
  across the whole expansion and checked **before** the SCC bypass (the SCC round does not get to
  bypass the backstop). It sits ~20× above the largest unit measured on any of eight corpora (redis,
  12,232 spliced nodes) and ~100× below the 25.1 M-node pathology it exists to stop. **Zero units
  truncate at any value in [25k, 500k]** — so the default is inert *by measurement*, not by hope.

**The budget is NOT fingerprint-determining, by policy.** Over budget, the unit is **skipped whole** —
it gets no inline variant at all, never a *partially* expanded one. This is the load-bearing design
choice: a partially-expanded unit would make the fingerprint a function of the budget (and thus of
config, and of expansion order), which would make the cache key and the output depend on a resource
limit. A whole-unit skip only ever *removes* an inline-assisted candidate; it can never alter one.

New stat **`inline_budget_skipped_units`** reports when the backstop fires, so it cannot fire silently
(it currently fires on nothing). Spec §5.4 updated (PLAN.md Rev 10).

---

## D51 — The landmark rarity gate is DELETED, not tuned (2026-07-12)

The landmark retriever (§5.5.4) gated which offset-sorted subtree peaks become constellation
anchors on a document-frequency cap: `rare_cap = landmark_rare_floor.max(n_units /
landmark_rare_divisor)` (historically `3.max(n/20)`), computed by default over `bag_set` rather
than the `offsets` inventory the gate actually filters (the E5b defect: every 3-to-5-token subtree
is absent from that `df` map, hits `unwrap_or(0)`, and is admitted as maximally rare regardless of
its true frequency).

**Measured, on real corpora (fs: 754,254 hashes; drivers/net):** the shipped gate is inert — it
rejects 3 of 754,254 hashes on fs and has never rejected a subtree of ≥7 tokens. The derived cap
sits 40–160× above the `df` distribution it gates (p99 of 3-token `df` is 46 on fs; the cap is
~1,968). Fixing the E5b defect so the gate binds on the inventory it filters does not help: at
every percentile tried, from the mildest (top 0.1%) up, it removes candidates and index rows but
**zero junk groups** — every group it costs is a real, tight clone family (median divergence
0.08–0.10), concentrated in units near the size floor (~46 median tokens: a unit that small has
few peaks to begin with, so culling its common ones drops it below `shared_landmarks_min` and it
stops being a candidate at all). `df` says nothing about whether the pair an anchor mints is real;
no threshold on it can separate signal from noise.

**DECISION: delete the gate outright, not tune it.** Every offset-sorted subtree peak is now
admitted into the constellation. Deleted: the `df` map construction in `Landmark::candidates` (a
754,254-entry `HashMap` per language, built solely to feed the gate), and the three config keys
that existed only to parameterize it (`landmark_rare_floor`, `landmark_rare_divisor`,
`landmark_rare_cap`, plus `landmark_df_over_offsets`, the E5b toggle). `landmark_fan_out` stays —
it is the real memory/recall lever, orthogonal to rarity.

**Measured effect of deletion, on real corpora:**

| corpus | groups | candidates | landmark index | peak RSS |
|---|---|---|---|---|
| fs (linux/fs) | 6,484 → 6,484 (1 group absorbed into a larger family, net 0) | +3.1% | +0.6% | flat to slightly lower |
| net (linux/drivers/net) | 28,743 → 28,747 (+4; every one of the 8 "lost" groups is a strict member-subset of a "gained" group — 100% absorption, zero true loss) | +3.4% | +0.7% | flat to slightly lower |

Peak RSS does not regress — if anything it trends slightly down, because the deleted `df` `HashMap`
had its own real cost that offset the modest growth in admitted candidates and index rows.

**The architectural point.** Flood control belongs at the **candidate** level, where pairwise
evidence exists to weigh a match — the §0.3 coverage-fraction gate already drops more candidates on
fs than survive it, at zero measured recall cost, because it weighs a shared constellation against
the size of the smaller unit. A rarity gate acts on **anchors**, before any pair exists to weigh:
it destroys evidence pre-emptively rather than adjudicating it. That is why one mechanism is free
and the other never was.

Spec §5.5.4/§9 updated (PLAN.md); `docs/substantiality-metric.md` and `CALIBRATION.md` annotated
where they described the gate as live.

## D52 — `check` reads the git object store: the INDEX, never the worktree (supersedes D41's base-worktree + archive fallback; 2026-07-13)

`reprise check` resolved both halves of its input from the **working tree**: the changed-unit set
came from `git diff -U0 <base>` (no `--cached`), and the file *content* came from a filesystem walk.
Base state was synthesized by materialising the base ref as a real checkout — `git worktree add`
**inside the scanned repo**, removed in `Drop`, with a `git archive | tar` fallback (D41(b)) for
clones where `worktree add` fails.

Both halves were wrong, and for the same reason: **the scanner could only read a filesystem**, so
`check` had to conjure a filesystem to read.

**Defect 1 — it judged code that was not being committed.** Reported from the field: a commit of
five staged files was failed by `inconsistent-update` findings in two files that were merely *dirty*
in the checkout, left there by a different concurrent session. On a shared or multi-agent working
tree — the workflow reprise is explicitly built for — the flagship pre-commit gate fails commits
over other people's work-in-progress. A gate that cries wolf is a gate people disable, and CLAUDE.md
tells agents never to bypass it.

**Defect 2 — a report-only tool wrote to the scanned repo.** `Drop` does not run on `SIGKILL`, and
an OOM-killed scan is exactly that; a killed `check` strands a worktree registration in the *user's*
`.git`. Not observed in the wild, but structural.

**The decision: make the content source an abstraction** (`src/source.rs`, `ContentSource`).
`scan` reads the live checkout (`FsSource`). `check` reads git objects (`GitSource`): the **index**
for the prospective commit, the **base ref** for base state — via `ls-files --stage` / `ls-tree -r`
plus one `cat-file --batch` pass. `git diff` gains `--cached`.

Consequences:
- **The bytes judged are the bytes committed.** An unstaged edit is invisible to `check` by
  construction, not by filtering. (Filtering findings to the staged *file list* while still reading
  worktree *content* was considered and rejected — it fingerprints bytes that are not being
  committed.)
- **No worktree is created in a user repo, ever.** The guarantee is structural rather than
  best-effort cleanup, so `Drop`-vs-`SIGKILL` stops mattering.
- **D41(b)'s archive fallback is deleted, not ported.** It existed solely to work around checkout
  layouts where `worktree add` fails; object reads do not care about checkout layout. Its
  shell-injection hardening (cute-coral) is preserved by construction: the base ref is resolved to a
  SHA via `rev-parse` and passed as a verbatim argv element, never through a shell.
- **Cache-neutral.** The D19 per-file cache is content-addressed, so a staged blob whose bytes equal
  the on-disk bytes is a cache HIT across sources (measured on ctxloom: 1031 hits, 0 misses).
- **`scan` is byte-identical** — verified on reprise itself and on ctxloom, every group, finding and
  stat, timings excluded.

**In CI this changes nothing:** a CI checkout has index == HEAD, so `--cached` against the merge-base
yields exactly the PR's changes. It changes local use: `check` now reports on what you have *staged*.
An unstaged edit is not part of the prospective commit and is not reported.

## D53 — `check` is a PR check: the CLI resolves the merge-base by default (2026-07-13)

`check` is specified as PR mode (PLAN.md §2; README: "this is what runs in CI on a PR"), and the
base-resolution policy for it already existed — `resolve_base`: explicit → pinned `[baseline] ref`
→ **merge-base with the default branch** → `HEAD`. But it lived in `reprise-server-core` and was
wired into the **MCP and LSP servers only**. The CLI required an explicit `--base` or a pinned ref
and **errored** otherwise. The flagship PR check had no PR base.

**What that cost.** The one deployment that mattered — a real pre-commit hook — could not use the
default, so it pinned `--base HEAD`. That silently turns a PR gate into a **per-commit** gate: a
duplicate introduced in branch commit 1 is no longer "touched" by commit 2, so it passes the local
hook and then fails in CI, which diffs against the merge-base. **The local gate and the PR gate
disagreed by construction.** With `--base HEAD`, the only thing separating "the commit" from "the
checkout" is the index — which is precisely the confusion D52 had to untangle.

**Decision.** `resolve_base` moves into the core (`reprise::baseline`), and the CLI uses it. Bare
`reprise check .` now answers the question the command is for. `reprise-server-core` re-exports it
rather than keeping a copy: **one resolution rule for the CLI and both servers**, because a base
that differs between a local run and CI turns a green hook into a red PR.

This also retires an axis-1 duplication: server-core carried its own `git()` helper whose doc
comment said it scrubbed `GIT_INDEX_FILE` "for the same reason as `reprise::check::git_cmd`". The
core helper is now the only one.

**Consequence for hooks.** A pre-commit hook should pass no base (or the merge-base) rather than
`--base HEAD`, so it previews exactly what CI will say. Once every copy of a duplicate lives on the
branch, the gate stays quiet; while the branch carries a finding, it keeps reporting — which is what
a drift gate is for. Base state is SHA-keyed and cached, so the extra scope costs one cached base
scan.
