---
title: Closing SP4 — the grammar-coupled frontend lift (default plan)
status: IMPLEMENTED — Inc 1-6 landed on similarity-ir (session stark-mixed-front); approved
  (2026-07-05). The residual exposure the universal-IR survey flagged as "the caveat to watch"
  (docs/universal-ir-trap.md §5.1, §6) is now GATED — data-driven dispatch tables + a conformance
  gate (full grammar-generation still future). Sign-off RESOLVED (user, 2026-07-05):
  D-SP4-1 data-driven table; D-SP4-2 refactor-first; D-SP4-3 caveat 4 only. Increments re-sequenced.
sessions:
  - fatal-main-storm
related:
  - docs/universal-ir-trap.md §5.1 (SP4 mitigated-not-eliminated), §6 (the takeaway: "the one
    place the graveyard can still reach us")
  - docs/SIMILARITY-IR.md §12.1 (generate the lift from the grammar — structural imperative),
    §12 (tree-sitter-queries bullet; CodeQL dbscheme bullet), principle 1, §8 (parity gate)
  - DECISIONS.md D9/D24 (grammar-shape fragility), D19 (grammar-version-pinned cache keys),
    D-IR-4 (node-addition discipline)
  - docs/transform-seam.md (the parity-neutral, per-step landing pattern this reuses)
---

# Closing SP4 — the grammar-coupled frontend lift

> **Thesis.** SP4 is the only sticking point in the universal-IR graveyard still in range of
> reprise (trap §6). Its teeth is *silent* hash drift: a grammar bump renames a node-kind, the
> hand-written `match` arm stops matching, the node falls through `_ => native(...)` to a
> **different-but-still-valid** canonical tree, the `u128` moves, clones stop converging — **no
> error, green build.** This plan does not make the lift write itself (impossible — see §Risks).
> It converts that silent drift into a **loud, localized, test-time failure**: SP4 goes from
> *mitigated* (hope a convergence fixture catches it) to *gated* (a bump can't move a hash without
> tripping a named assertion). That conversion is the realistic, honest closure.

## 1. What SP4 is, concretely, in this tree

Two string families couple `src/frontend/{rust,python,go}.rs` to a *specific pinned grammar
version*, with no structural detection of drift:

1. **Dispatch kinds** — the `match node.kind()` arms: `"while_expression"`, `"if_expression"`,
   `"call_expression"`, … (`rust.rs:45`). ~40 per frontend.
2. **Field names** — the tuples threaded into shared helpers: `("pattern","value")` for a `for`,
   `("left","operator","right")` for a binop, `child_by_field_name("condition")` inside
   `lower_while` (`frontend/mod.rs`). ~25 per frontend.

The failure mode is the interaction with the catch-all `_ => native(node, …)` (`rust.rs:152`):

- **Renamed kind** → old arm dead → node routes to `native()` → a *valid* `NativeStmt` tree that
  is **not** the canonical form it used to be. Hash moves. No panic.
- **Renamed field** → `child_by_field_name` returns `None` → the guard/operand silently drops out
  of the lowered node. Hash moves. No panic.

Today's only guards (trap §5.1): pinned grammar crate versions (D9/D19) — which merely make drift
*happen on a bump you chose*, not *loud when it happens* — and `tests/convergence.rs`, which
catches drift **only if a fixture happens to exercise the broken construct**. That "if a fixture
happens to" is the hole.

`tests/convergence.rs` and pinned versions are a *mitigation*. The bar this plan sets (SIMILARITY-IR
§12.1): **a grammar bump cannot silently move a hash — it either stays green or fails loudly and
points at the exact broken reference.**

## 2. The fix — enumerate the coupling, then gate it against the linked grammar

Two coupled pieces. Piece B is the teeth; Piece A is what makes B *complete and trustworthy*.

**Piece A — make every referenced (kind, field) string enumerable.** You cannot write a complete
gate over strings buried in arbitrary `match`/`if` control flow — you can't prove you listed them
all. So the referenced set must be reachable *as data*. This is the "declarative/centralize" step
(§12's "the tabular half of the frontend could be data"); it also delivers a DRY win, but
*completeness of the gate* is the load-bearing reason, not tidiness.

**Piece B — the conformance gate (the SP4 closure).** For each `Lang`, load the pinned grammar via
`lang.ts_language()` and, using the tree-sitter 0.26 `Language` reflection API
(`id_for_node_kind`, `field_id_for_name`, `node_kind_count`/`node_kind_for_id`,
`field_count`/`field_name_for_id`), assert **every enumerated (kind, field) exists in the grammar**.
Absent → the test fails with a precise message:

> `rust frontend references node-kind "while_expression", absent from the linked tree-sitter-rust
> (0.24.2) — grammar drift; the lowering for this construct is dead and now routes to native().`

Plus the **inverse advisory**: enumerate named grammar kinds the frontend neither maps nor
deliberately routes to `native()`, snapshot that set, so a bump that *adds/renames* kinds produces a
reviewable diff — a bump prompts a look instead of silently changing behaviour. (This is the cheap
SCIP-style readable-snapshot harness from §12.1, applied to the grammar surface.)

**Why the runtime API, not `node-types.json`:** the reflection API validates against the *actually
linked* grammar object, is always present (no build-script file wrangling), and `id_for_node_kind →
0` / `field_id_for_name → None` is exactly the "does this string still exist" predicate SP4 needs.
Limitation noted: field existence is grammar-*global*, not scoped per node-type (node-types.json
would scope it); global existence is sufficient to catch renames/removals, which is the SP4 hole.

## 3. Decisions for sign-off

These change an interface seam (the `Frontend` dispatch shape) and topology (a shared mapping
representation across three deliberately-parallel frontends), so they are surfaced explicitly, not
folded into steps (per the decision-gating discipline).

### D-SP4-1 · How declarative is Piece A?
- **(a) Registry alongside the `match` (cheapest).** Keep the hand-written dispatch; add a
  per-frontend `const REFERENCED: &[(kind, &[field])]` the gate checks. *Risk:* the registry is a
  **parallel** list — a new `match` arm with a forgotten registry entry drifts silently; completeness
  is by discipline, not structure.
- **(b) Data-driven dispatch (RECOMMENDED).** Replace the `match` with a table
  `const MAP: &[(kind, Lowering)]` (`Lowering` = an enum/fn-ptr picking the shared `lower_*` helper +
  its field args). The dispatch **is** the table, the gate reads the same table the runtime uses →
  completeness is **structural** (one source of truth). Irreducible quirks (Rust tail-return,
  tuple-assign, `match` subject-fold) stay a small explicit hand-written residue, itself
  gate-registered. Parity-neutral refactor.
- **(c) tree-sitter `.scm` queries / TSG DSL (the §12.1 "purest").** Express the mapping as query
  files. *Rejected for now:* tree-sitter queries *select* nodes but cannot *lower* them — they can't
  synthesize a break-guard, fold a subject into each guard, wrap a tail in `Return`, or do the
  conditional tuple-assign detection. They'd cover only trivial 1:1 kinds and leave the structural
  majority in Rust anyway → a hybrid split across two representations that makes drift-tracking
  *harder*. This is why §12 rated it a "modest DRY win," not the fix. Revisit only if the tabular
  fraction ever dominates.

→ **Recommendation: (b).** It is the honest realization of "tabular half is data, structural half is
Rust," and gives ~all the SP4 closure at a fraction of (c)'s machinery.
**RESOLVED (user, 2026-07-05): (b)** — data-driven dispatch table; the table is the single source of
truth the gate reads, so completeness is structural.

### D-SP4-2 · Gate enforcement & sequencing
- **(a) Test-only, gate-first.** Land Piece B as a `just test` assertion (+ CI) over the current
  hand-written frontends now (partial registry, D-SP4-1a interim), refactor to the table (D-SP4-1b)
  language-by-language afterward. Lowest risk; teeth land immediately; interleaves with the in-flight
  parity work (`boned-jam`, `tidy-grime`) exactly like the transform-seam conversions.
- **(b) Refactor-first.** Do D-SP4-1b table conversion first, derive the gate from it. Cleaner end
  state sooner, but a bigger single change against still-moving frontends.
- **(c) build-time.** A `build.rs`/codegen validation step instead of a test. More plumbing; a test is
  enough to make drift loud. Defer.

→ **Recommendation: (a) gate-first, then table.** De-risks against live frontend churn and gives the
unbuilt TS/Kotlin frontends (SIMILARITY-IR §9 P2) the gate + table as their *template* — born gated.
**RESOLVED (user, 2026-07-05): (b) refactor-first** — build the table per language first, derive the
gate from it (the tables are a prerequisite for a structurally-complete gate anyway). The per-language
conversions stay parity-neutral, so they still interleave with the in-flight parity work; the SP4
closure (the gate) lands after the three tables. §4 re-sequenced accordingly.

### D-SP4-3 · Scope of §5's other residuals (fold in, or separate?)
Trap §5 lists three more policy-not-structure caveats. Two are cheap structural guardrails in the
same spirit:
- **(3) `Native` inertness** — a test that a `Native*` node's label is exactly the source kind and
  its children are only lowered children (no resolver call, no cross-language map).
- **(4) node-set discipline** — a test that every emitted kind ∈ `kind::ALL` (or an operator token /
  `Native` label), catching an ad-hoc raw-CST-kind leak structurally.

→ **Recommendation: include (4) as an optional final increment (cheap, high-fit); leave (3) as a
doc-comment invariant + a light test; leave caveat (2) — intra-language over-convergence — to the
separate `fond-mute` substantiality-metric effort, where it already lives.**
**RESOLVED (user, 2026-07-05): caveat 4 only** — the emitted-kinds-∈-`kind::ALL` test as the optional
final increment; caveat 3 stays a doc-comment invariant + a light test; caveat 2 stays with `fond-mute`.

## 4. Increments (refactor-first per D-SP4-2; each parity-gated; the transform-seam pattern)

The load-bearing invariant, as in transform-seam: the mapping representation is **hash-neutral**, so
every conversion is a **pure refactor** — canonical tree, every `u128`, every finding byte-identical.
Gate each step on `pair_ab` showing **zero** canonical-tree diff (Rust/Python/Go; mutation-recall +
wild corpus). `SCHEME_VERSION` does **not** move (no canonical-form change). The three tables come
*before* the gate: the table is the gate's single source of truth, so a structurally-complete gate
needs them first.

1. **Table skeleton + convert the Rust dispatch (D-SP4-1b), parity-gated.** Introduce
   `const MAP: &[(kind, Lowering)]` (`Lowering` = an enum/fn-ptr selecting the shared `lower_*` helper
   + its field args) and replace `rust.rs`'s `match node.kind()` with a table lookup. The irreducible
   quirks (tail-return, tuple-assign, `match` subject-fold) move to a small explicit hand-written
   residue with its own registered strings. Establishes the pattern the other two follow.
2. **Convert the Python dispatch to the table, parity-gated.**
3. **Convert the Go dispatch to the table, parity-gated.** (All three tables now carry the complete
   referenced (kind, field) set — completeness is structural, not a parallel list.)
4. **Grammar-schema probe + snapshot.** `grammar_schema(lang) -> { named_kinds, fields }` via the
   `Language` API (`node_kind_count`/`node_kind_for_id`, `field_count`/`field_name_for_id`); snapshot
   per grammar (readable, sorted) so any future bump is a reviewable diff.
5. **Conformance gate (Piece B — the SP4 closure), derived from the tables.** Assert every table
   (kind, field) exists in the linked grammar (`id_for_node_kind`/`field_id_for_name`), failing with a
   named, localized message; add the unmapped-kinds advisory (grammar kinds neither in a table nor
   deliberately `native()`-routed). Prove teeth: a planted bogus reference makes it red; a simulated
   rename makes the snapshot diff.
6. **Version-pin + docs.** Bind the gate to the grammar crate versions; document the **bump ritual**
   (bump crate → run gate → green, or an actionable diff → only then bump `SCHEME_VERSION` if the
   canonical form *legitimately* moved). Update SIMILARITY-IR §12.1 and universal-ir-trap §5.1/§6 from
   "mitigated / not yet built" to "gated."
7. **(optional) §5 guardrail (caveat 4).** A test that every emitted kind ∈ `kind::ALL` (or an operator
   token / `Native` label), catching an ad-hoc raw-CST-kind leak structurally. (Caveat 3 stays a
   doc-comment invariant + a light test; caveat 2 stays with `fond-mute`.)

## 5. Validation

- **Parity (non-negotiable).** `pair_ab` zero canonical-tree diff across every conversion — the pure-
  refactor invariant. The gate/table refactor must not move a single hash.
- **Teeth.** A test asserting the gate *fails* on a planted bogus kind/field and *passes* clean; the
  schema snapshot diffs on a simulated rename.
- **Determinism.** Same source → same canonical tree and same schema snapshot; `SCHEME_VERSION`
  unchanged.
- `just test` (full suite incl. `tests/convergence.rs`), `clippy -D warnings`, `fmt` clean.

## 6. Risks & honest caveats

1. **This closes SP4, it does not eliminate it.** The human still authors the mapping *semantics*
   ("`while_expression` is a Loop"); no tool can generate that, and queries can't generate the
   structural lowering (why D-SP4-1c is rejected). What changes is drift becomes **loud, localized,
   test-time** instead of **silent, diffuse, discovered-as-a-mystery-recall-regression**. That is the
   real, bounded win — matching the doc's own framing (§6: the fix is "named … but not yet built";
   this builds the enforceable half of it). Do not oversell it as "the lift now writes itself."
2. **Grammar-API scoping.** Field existence is grammar-global, not per-node-type; kind enumeration
   includes anonymous/aux kinds (filter to named). Global existence still catches every rename/removal
   — the SP4 hole — but won't catch "field valid on the grammar but no longer on *this* node-type."
   Acceptable; note it, and let the parity gate + convergence net cover the residue.
3. **Sequencing friction** with `boned-jam`/`tidy-grime` (parity + default flip). Under the resolved
   refactor-first order the table conversions land against still-moving frontends — mitigated because
   each conversion is per-language and parity-neutral (byte-identical canonical tree), so they
   interleave the same way the transform-seam pass conversions do; the gate itself lands last.
4. **Payoff timing.** The biggest dividend is on the *unbuilt* TS/Kotlin frontends (P2): building them
   into the table + gate from day one is far cheaper than retrofitting. This argues for doing the core
   (Inc 0–1, and the table pattern) *before* P2, not after the flip.

## 7. Out of scope

- The default flip / parity work (`boned-jam`, `tidy-grime`) — a different axis; this rides alongside.
- Auto-generating mapping *semantics* or a query-lowering engine (D-SP4-1c) — deferred.
- The substantiality/precision residual (trap §5.2) — owned by `fond-mute`.
- Any change to the matching back half, or to `SCHEME_VERSION`/canonical form (pure refactor).
