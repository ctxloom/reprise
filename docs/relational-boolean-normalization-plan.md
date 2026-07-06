---
title: Relational/boolean canonicalization + desugar-consistency — implementation plan
status: plan — key decisions captured (2026-07-04); downstream of the transform-seam effort; not yet scheduled for implementation
sessions:
  - fatal-main-storm
related:
  - docs/SIMILARITY-IR.md §13 (decomposition ladder — rung 2 guard canonicalization overlaps Family C)
  - docs/SIMILARITY-IR.md §14 (Branch — the unified conditional Families A/C rewrite)
  - docs/SIMILARITY-IR.md §15.3 + docs/transform-seam.md (D-IR-12 detect-emit seam — sequencing decision below)
  - DECISIONS.md D-IR-4 (driving-case discipline), D-IR-9 (reversal/losslessness), D2 (positional/masked)
  - src/ir/pass.rs (where Families A/C land), src/frontend/mod.rs `normalize()` (the pipeline)
  - src/frontend/go.rs (Family B), src/ir/transform.rs (new TransformKinds)
---

# Relational/boolean canonicalization + desugar-consistency

## 1. Problem

Three convergence gaps sit in a soundness class reprise already accepts (the commutative-sort
class: exact modulo float/NaN/operator-overloading), and none are yet designed:

1. **Relational/boolean algebra is not normalized at all.** `a < b` ≢ `b > a`; `!(a < b)` ≢
   `a >= b`; `!(a && b)` ≢ `!a || !b`; `!!x` ≢ `x`. Worst offender: `break_guard`
   (`src/frontend/mod.rs:926`) synthesizes a raw `!(cond)`, so `while i < n {…}` lowers to a guard
   `!(i < n)` that **never converges** with a hand-rolled `loop { if i >= n { break } … }`
   (`i >= n`). The loop-unification win (principle 1) is already paid for but partly uncollected on
   the single most common loop shape.

2. **Desugaring is inconsistent across frontends.** `lower_aug_assign` desugars `a op= b → a = a op
   b` for Rust/Python (`AugAssign`), but Go `i++`/`i--` become a bare `Unop` (`go.rs:421`) and Go
   `+=`/`-=` stay `Native` (`go.rs:449`). So the loop-counter idiom — the commonest statement in
   imperative code — fails to converge across `i++` / `i += 1` / `i = i + 1` and across languages.

3. **Guard nesting is not canonicalized.** `if a { if b {…} }` ≢ `if a && b {…}`; a redundant
   `else` after a diverging arm (`if c { return x } else { y }`) keeps its nesting. Both are among
   the commonest hand-refactors (§13 rung 2 territory, never separately called out for the
   conjunction-merge).

The fix lives once on the IR (the whole point — `src/ir/pass.rs`), except Family B, which is
frontend lowering. This plan designs the passes, the pipeline placement, the driving cases, and the
gates — but **does not implement** until the sign-off decisions in §5 are settled.

## 2. Scope

**In scope — three families, sound end of the spectrum:**

- **Family A — Relational/boolean canonicalization** (new, language-agnostic pass(es)):
  - **A1 · Comparison orientation** — `a < b` ≡ `b > a`, `a <= b` ≡ `b >= a`. Canonicalize every
    ordered comparison to a chosen orientation (recommend: `<` / `<=`), flipping operands + operator
    together. The ordered-comparison analog of the existing commutative sort (which already handles
    the symmetric `==`/`!=`).
  - **A2 · Negation pushing** — `!(a && b)` → `!a || !b` (De Morgan), `!(a < b)` → `a >= b`
    (comparison inversion), `!!x` → `x` (double-negation). This is the pass that makes the
    `break_guard` `!cond` converge with a hand-written inverted comparison.

- **Family B — Desugar consistency** (frontend lowering, completes an existing family, no new
  TransformKind — reuses `AugAssign`):
  - **B1 · Increment/decrement** — Go `i++`/`i--` → the desugared `i = i + 1` / `i = i - 1` (the
    `lower_aug_assign` shape), not a `Unop`.
  - **B2 · Go compound assignment** — route Go `+=`/`-=`/… through `lower_aug_assign` (as Rust does)
    instead of `Native`.

- **Family C — Guard canonicalization** (new, language-agnostic pass(es); implements part of §13
  rung 2):
  - **C1 · Nested-if → conjunction** — `if a { if b {…} }` ≡ `if a && b {…}`, restricted to the
    single-arm, no-else nesting (sound only there).
  - **C2 · Redundant-else elimination** — `if c { <diverges> } else { Y }` ≡ `if c { <diverges> };
    Y`, where the then-arm ends in `Return`/`Break`/`Continue` (definitely diverges).

**Out of scope (deferred, named so the boundary is explicit):**

- Boolean-literal folding (`if c { true } else { false }` → `c`, `x == true` → `x`). Overlaps the
  §14 branch-hoist (D-IR designed). Fast-follow candidate, not this plan.
- The whole Tier-2 cross-idiom set (comprehension⟷loop, error-propagation, string-building,
  null-coalescing, method-chain⟷nested, resource-cleanup). Those are the §13-rung-3/4 frontier —
  precision-risky, `Native`-first, separate efforts.
- Constant folding, distributivity, strength reduction, statement reordering (needs a PDG — §10
  stops at the tree).

## 3. Why these are safe to converge (soundness ledger)

| Rewrite | Identity | Unsound only under |
|---|---|---|
| A1 comparison orient | `a < b ≡ b > a` | operator overloading with inconsistent `<`/`>` (pathological) |
| A2 De Morgan | `!(a&&b) ≡ !a\|\|!b` | none (boolean identity) |
| A2 comparison invert | `!(a<b) ≡ a>=b` | float/NaN (`!(a<b)` also true when NaN) — the accepted comm-sort class |
| A2 double-neg | `!!x ≡ x` | overloaded `!` (pathological) |
| B1/B2 desugar | `i++ ≡ i=i+1`, `a op= b ≡ a=a op b` | overloaded `++`/`op=` with side effects (pathological) |
| C1 conjunction | `if a{if b{X}} ≡ if a&&b{X}` | short-circuit side effects in `a` that `b` depends on — **but identical evaluation order**, so sound |
| C2 redundant else | `if c{ret}else{Y} ≡ if c{ret};Y` | none, given the then-arm provably diverges |

Every row is at or inside the unsoundness reprise already accepts for the report (D2 / §5.2.6). The
**binding constraint is precision, not soundness** (§13 risk-1): these are all convergence-increasing,
so each is gated on the §7.2 precision sample, not just recall.

## 4. Design

### 4.0 CQRS structure — mandatory (the §15.3 detect-emit seam, no infrastructure)

**These passes must be CQRS, `again` without the ES infrastructure** (§15.1 "adopt the model, not the
machinery"; D-IR-10 in-memory-only). Concretely, the §15.3 / D-IR-12 detect-emit seam **is** the
CQRS realization and is a hard requirement here, not the deferred option my earlier D-IR-15(a) lean
assumed:

- **Command side (write model) — `Edit`.** A pass emits described mutations; it does **not** perform
  them. A detector is `fn detect_x(tree: &NormNode) -> Vec<Edit>` — an *immutable* borrow in, `Edit`s
  out. With no `&mut`, it **cannot mutate**: "orient a comparison without recording it" does not
  compile. This is the structural guarantee, enforced by the type system (§15.3's bar).
- **Command handler — the single `apply`.** `apply(tree, &[Edit], &mut TransformLog) -> NormNode` is
  the **sole** front-half canonicalization mutator; it performs each edit **and** records its event
  atomically. Transform-without-event is unrepresentable.
- **Query side (read models) — projections.** Every view is a fold over `(genesis tree, event
  stream)`: the aggressive canonical tree (matching form) is `apply`'s output; reversal /
  display-unlowering / match-explanation are inverse-folds materialized only when a consumer asks.
- **No infrastructure (the `again`).** In bulk scan the only projection is the aggressive tree, so the
  log is **disabled** (`TransformLog::disabled()` — `record()` early-returns, allocates nothing;
  D-IR-10). No event store, no persistence, no async projectors — the stream is a deterministic,
  re-derivable function of source, reconstructed on demand by re-normalizing the touched unit.

This means the plan **depends on the `ir::edit` seam** (`src/ir/edit.rs`, `Edit` + `apply` +
`TransformLog::disabled()`) already existing — so this work is **sequenced after the transform-seam
effort** (`docs/transform-seam.md` / §15.3), which builds that seam and converts the existing five
passes. **This plan does not build the seam** (user, 2026-07-04); it adds four `Edit` variants +
detectors on top of it. Family B is exempt from the dependency: it is *lowering* (construct-and-record
— builds genesis, records inline), which §15.3 keeps as construct-and-record, not detect-emit, so it
may land independently of the seam. The compile-time guarantee is scoped to the canonicalization
passes (A/C), exactly per §15.3.

### 4.1 Pipeline placement (`src/frontend/mod.rs` `normalize()`)

Current order:
```
rewrite_iteration → normalize_loop_exit → abstract_idents → canonicalize_order → strip_dead
```
Proposed order, in the `apply(tree, detect_*(&tree), log)` fold form the seam mandates (new steps
bracketed; existing five shown in their current shape until the seam converts them per
`docs/transform-seam.md`):
```
tree = rewrite_iteration(tree); tree = normalize_loop_exit(tree);
tree = apply(tree, &detect_guard_canonicalize(&tree), log);  # C1,C2 — restructure Branches; may create `&&`
tree = abstract_idents(tree);
tree = apply(tree, &detect_boolean_normalize(&tree), log);   # A1,A2 — orient/invert; label-independent; precedes comm-sort
tree = canonicalize_order(tree);                             # sees De-Morgan'd `||`/`&&` chains + oriented cmps
tree = strip_dead(tree);
```
(The transform-seam effort precedes this plan, so the existing five are **already** detect-emit —
`canonicalize_order` is `apply(tree, &detect_comm_sort(&tree), log)` by the time A/C land. A/C slot in
as more `apply` steps; no later conversion is owed on them.)

Load-bearing ordering facts:
- **A before `canonicalize_order`.** A2's De Morgan turns `!(a && b)` into a fresh `||` chain that
  comm-sort must then sort; A1's orientation settles asymmetric operands before symmetric ones sort
  around them. A is label-independent (operates on operators/shape), so its position relative to
  `abstract_idents` is free — placed after, so comm-sort's abstracted-label sort key is unchanged.
- **C before A.** C1 synthesizes `a && b` guards from already-lowered `a`/`b`; A then normalizes any
  negations inside them. C before `abstract_idents` too (it moves whole guard/body subtrees; doing
  it pre-abstraction keeps loci on `Raw` labels, matching the other structural passes).
- **B is not here** — it is frontend lowering (§4.3), upstream of the whole pipeline.

Confluence: A1 (asymmetric ops) and comm-sort (symmetric ops) touch disjoint operator sets — no
competing directions, so the ordered pipeline stays confluent (no e-graph needed, D-IR-7 untouched).

### 4.2 Families A & C — detectors in `src/ir/pass.rs`, edits in `src/ir/edit.rs`

Each is a read-only detector `fn detect_*(tree: &NormNode) -> Vec<Edit>`; the mutation lives in
`apply`'s per-`Edit` arm, which records the event. New `Edit` variants (loci are **byte spans, not
child indices** — §15.3 gotcha; a removal shifts siblings) and their recorded `TransformKind` +
`Witness`:

- **A1 · orient** — `detect_boolean_normalize` finds a `Binop` whose op ∈ {`>`,`>=`} (non-canonical
  orientation). → `Edit::CmpOrient{ locus }`. `apply` swaps `left`/`right` and flips the op to
  `<`/`<=`. Records `TransformKind::CmpOrient`, witness `Witness::Order(vec![1,0])` (the swap, so
  reversal restores the original orientation).
- **A2 · not-push** — same detector finds a `Unop{!, x}` reducible by: comparison inversion (`x` a
  comparison `Binop` → invert op, drop `!`: `<`↔`>=`, `>`↔`<=`, `==`↔`!=`), De Morgan (`x` a
  `Binop{&&|||}` → flip op, negate both operands, recurse), or double-negation (`x` a `Unop{!,y}` →
  `y`). → `Edit::NotPush{ locus }`. Records `TransformKind::NotPush`, witness `Witness::None`
  (bijective given the locus — reversal re-wraps the `!`). **The headline interaction:** the
  `break_guard`-synthesized `!(i < n)` becomes `i >= n`, converging `while` with hand-rolled
  `loop`+`break`.
- **C1 · conjunction-merge** — `detect_guard_canonicalize` finds a one-`Arm` `Branch` (guard
  g_outer, body a `Block` whose sole child is a one-arm no-else `Branch`, guard g_inner). →
  `Edit::GuardMerge{ locus }`. `apply` folds to one arm, guard `Binop{g_outer, &&, g_inner}`. Records
  `TransformKind::GuardMerge`, witness = the merged nesting depth (so reversal re-nests).
- **C2 · redundant-else** — same detector finds a `Branch` `[Arm(g, body_diverges),
  Arm(trivial-guard, else_body)]` with `body_diverges` ending in `Return`/`Break`/`Continue`. →
  `Edit::DropElse{ locus }`. `apply` drops the else arm and splices `else_body` as siblings after the
  `Branch` in the enclosing `Block` (a block-level rebuild, like `normalize_loop_exit` — the detector
  keys the enclosing block's locus). Records `TransformKind::DeadElse`, witness = the hoisted else
  body (so reversal re-wraps it as the else arm).

`diverges(body)`: a shared pure helper — last child is `Return`/`Break`/`Continue`, or a `Branch` all
of whose arms diverge. Read-only (detector-side), reused by C2 and future §13-rung-2 work.

Because detectors are pure and one `apply` records, each of A1/A2/C1/C2 satisfies the CQRS write-side
guarantee **by construction** — there is no code path that mutates the tree for these transforms
without appending to the stream.

### 4.3 Family B — frontend lowering

Pure `src/frontend/go.rs` changes (Rust/Python already correct; TS/Kotlin get the same when built):
- `lower_inc_dec` (`go.rs:421`) → build `Assign{ place, Binop{ read, +/- , Lit 1 } }` via the same
  synthesis `lower_aug_assign` uses, recording `AugAssign`. `i++`→`i = i + 1`, keep `1` (keep-list).
- `lower_assignment_stmt` (`go.rs:449`) → route the compound-operator branch to `lower_aug_assign`
  (drop the `=` off the op token) instead of `native(...)`.

Result: Go `for i := 0; i < n; i++`, `i += 1`, and `i = i + 1` all converge, and converge with the
Rust/Python equivalents.

### 4.4 Cache + versioning

Families A/B/C all change the canonical tree, so bump `kind::SCHEME_VERSION` (2 → 3) once at the end
of the batch (D19/D30 — a warm cache must never serve an old canonical form). New `TransformKind`
variants are hash-excluded, so they do **not** independently force a bump.

## 5. Decisions for sign-off

- **D-IR-13 · Adopt the relational/boolean family + orientation convention.** (a) Adopt Families A+C
  as a new §13-style family in `docs/SIMILARITY-IR.md`, canonical comparison orientation = `<`/`<=`
  (operands flipped to make the smaller-relation the spelling). (b) Adopt but pick the other
  orientation (`>`/`>=`). (c) Reject / defer. → **Recommendation: (a).** Orientation is arbitrary but
  must be fixed; `<`/`<=` matches how ranges/loops read.

- **D-IR-14 · Desugar-consistency scope.** Family B routes Go inc/dec + compound-assign through the
  existing `AugAssign` path. (a) Do B now (cheapest, lowest-risk win, no new node/kind). (b) Fold B
  into the eventual TS/Kotlin frontend work instead. → **Recommendation: (a) now** — it is pure recall
  gain at near-zero precision risk and closes an existing inconsistency.

- **D-IR-15 · Pass authoring shape — RESOLVED (user, 2026-07-04): CQRS detect-emit, no
  infrastructure.** New lossy passes (A1 orient, A2 invert) *destroy a discriminator* (original
  orientation / negation form) — exactly the D-IR-9 break the §15.3 seam prevents — so they **must**
  be authored as the CQRS write-side (detectors emit `Edit`s; the single `apply` mutates + records),
  not the legacy `fn(NormNode)->NormNode` shape. Consequence: this plan builds the `ir::edit` seam
  skeleton (Increment 0, §6) as a prerequisite and authors A/C detectors in the target shape from the
  start. "No infrastructure" (§15.1 / D-IR-10): in-memory, disabled-in-bulk, re-derivable — no store,
  no persistence, no async projectors. (My earlier lean toward legacy-shape-with-dual-write is
  withdrawn; the CQRS structural guarantee — silent mutation won't compile — is worth the seam
  dependency, and it means A/C owe no later conversion.)

- **D-IR-15b · Seam scope — RESOLVED (user, 2026-07-04): this plan builds none of the seam.** The
  `ir::edit` seam is delivered by the separate transform-seam effort (`docs/transform-seam.md`), which
  is **sequenced before** this work. This plan is downstream: it adds its four `Edit` variants +
  detectors to the seam that already exists. Family B (lowering) has no seam dependency and may land
  independently.

- **D-IR-16 · Phasing / go-no-go.** Land B → A → C as independent, separately-gated increments (§6),
  each abortable if the §7.2 precision sample regresses. → **Recommendation: adopt** (matches the
  P1-increment cadence and D-IR-4 driving-case discipline).

## 6. Phased increments (each parity/precision-gated)

Mirrors the existing `P1 increment N` commit cadence. **Not** parity-neutral (unlike the seam
refactor) — these *intend* to move the canonical tree toward more convergence, so the gate is the §8
recall-parity + §7.2 precision gate, not a byte-identical-tree invariant.

- **Prerequisite (not built here): the transform-seam effort.** `docs/transform-seam.md` (§15.3 /
  D-IR-12) lands first, creating `src/ir/edit.rs` (`Edit`, `apply`, `TransformLog::disabled()`) and
  converting the existing five passes to detect-emit. This plan's A/C increments **add** their four
  `Edit` variants + detectors to that seam; they do not create it (D-IR-15b).
- **Increment B (Family B — desugar consistency).** `go.rs` inc/dec + compound-assign → `AugAssign`
  (construct-and-record, not detect-emit — it is *lowering*, §15.3). Driving cases: Go `i++` ≡ `i +=
  1` ≡ `i = i + 1`, and Go `for`-counter ≡ Rust/Python. Lowest risk, and — being lowering — has **no
  seam dependency**, so it may land before the transform-seam effort. Bump SCHEME_VERSION here or defer
  to the batch's last increment.
- **Increment A1 (comparison orientation).** `detect_boolean_normalize` (orient case) + `Edit::CmpOrient`
  + `apply` arm. Driving case: `a < b` ≡ `b > a`; near-miss `a < b` ≢ `a <= b` stays distinct. Gate.
- **Increment A2 (negation pushing).** Same detector (not-push cases) + `Edit::NotPush` + `apply`
  arm. Driving case: `while i < n {…}` ≡ `loop { if i >= n { break } … }` (**the headline** — the
  `break_guard` convergence); `!(a && b)` ≡ `!a || !b`; `!!x` ≡ `x`. Gate — watch precision on the
  wild corpus (De Morgan can converge unrelated boolean soup).
- **Increment C1 (conjunction-merge).** `detect_guard_canonicalize` (merge case) + `Edit::GuardMerge`
  + `apply` arm. Driving case: `if a { if b {…} }` ≡ `if a && b {…}`. Gate.
- **Increment C2 (redundant-else).** Same detector (redundant-else case) + `Edit::DropElse` + `apply`
  arm + `diverges` helper. Driving case: `if c { return x } else { y }` ≡ `if c { return x }; y`. Gate.

Each increment: add the `Edit` variant + `apply` arm + detector case → fixture tests (the driving
cases, in the style of `while_and_loop_converge_with_differing_logs`) → run the gates → land or abort.

## 7. Validation method (per increment)

0. **CQRS structural guarantee (a type fact).** Assert every A/C detector is `fn(&NormNode) ->
   Vec<Edit>` — no detector takes `&mut`, so silent mutation won't compile. `apply` is the sole
   mutator and always records. This *is* §15.3's bar, checked at compile time, not by review.
1. **Driving-case fixtures (D-IR-4).** Hand-authored convergence tests proving the intended pair now
   converges — and, for discrimination, a near-miss that must **stay distinct** (e.g. A2 must not
   converge `a < b` with `a <= b`).
2. **Recall gate.** `just bench-mutations` — no regression; ideally the new convergences surface.
3. **Precision gate (binding).** `just scan-self` + the wild corpus + a §7.2-style spot check — **no
   new false positives**. This is where an increment gets aborted.
4. **Reversal round-trip = the query-side projection (free D-IR-9 harness).** Under a recording
   (enabled) log, the inverse-fold `N⁻¹(N(x)) ≈ x` for the new transform — catches a wrong/missing
   witness (e.g. A1's `Order` witness must restore orientation). This exercises the CQRS read-side:
   the display/explanation projection is a fold over the same event stream `apply` wrote.
5. **Determinism.** Same source → same `TransformLog::to_text()` (existing snapshot style).
6. **Cross-language convergence.** Where applicable, assert Rust ≡ Python ≡ Go for the idiom (the
   `go_single_range_converges_with_rust_and_python_for` pattern).

## 8. Risks

- **Precision drift from A2 De Morgan** — boolean expressions are high-frequency boilerplate;
  over-converging them risks D30-class false positives. Mitigation: the §7.2 gate per increment;
  abort A2 if it regresses; De Morgan is the increment most likely to be dialed back.
- **C2 needs enclosing-block context** — implement as a block-level fold (like `normalize_loop_exit`),
  not a node map, or the else-hoist has nowhere to splice.
- **Pipeline-order fragility** — A must precede comm-sort, C must precede A. Encode the order in
  `normalize()` with a comment; a golden test on a combined fixture (`!(a > b) && c`) pins confluence.
- **Sequencing dependency** — A/C cannot land until the transform-seam effort (`docs/transform-seam.md`)
  establishes `src/ir/edit.rs`. Mitigation: Family B (lowering) has no such dependency and can proceed
  first; A/C queue behind the seam. This plan is explicitly downstream (D-IR-15b).
- **`apply` correctness under multiple edits per pass** — a detector may emit several `Edit`s over
  one tree; `apply` must key by span/locus and rebuild once (never sequential index mutation — §15.3
  gotcha), or a `DropElse` shifts the loci of later edits. Mitigation: collect-then-rebuild; a golden
  test with two edits in one pass.

## 9. Doc + decision updates (part of the work)

- `docs/SIMILARITY-IR.md`: new family section (§16 or a §13 sibling) describing Families A/C as
  CQRS detect-emit passes built on the §15.3 seam, plus a note completing the desugar family (B);
  register D-IR-13…16 (D-IR-15 / D-IR-15b RESOLVED). Cross-reference §15.3 — Families A/C are
  **downstream** of the transform-seam effort, which lands the seam and converts the existing five first.
- `DECISIONS.md`: entries for the accepted decisions.
- `src/ir/edit.rs` (created by the transform-seam effort, **not** this plan): add the four A/C `Edit`
  variants (`CmpOrient`/`NotPush`/`GuardMerge`/`DropElse`) + their `apply` arms.
- `src/ir/transform.rs`: `CmpOrient`, `NotPush`, `GuardMerge`, `DeadElse` variants + `to_text` slugs.
