---
title: Similarity IR — a purpose-built intermediate representation for reprise
status: authoritative design; P1 landed through increment 9 — Rust/Python/Go frontends
  + Branch switch/match↔if-chain convergence, IR-selectable via `normalizer = "ir"`;
  transform-seam refactor (D-IR-12, docs/transform-seam.md) and ⚠ nodes (ANF §13,
  try/catch D-IR-11) still pending (2026-07-04)
sessions:
  - meek-zany-rash
related:
  - DECISIONS.md D44 (execution-bytecode/LLVM rejected as substrate; author, don't borrow)
  - docs/PLAN.md §5.2 (current normalization pipeline this reframes)
  - DECISIONS.md D2 (positional vs masked identity), D9/D24 (shape-exact CST fragility)
  - DECISIONS.md D30 (dispatch-table suppression), D11/D25 (rule-addition discipline), D40 (git-ref baseline)
---

# Similarity IR

> **Reference convention:** bare **§X** = a section of *this* document; **spec §X** =
> docs/PLAN.md (the reprise Rev 9 spec); **D-IR-N** = a sign-off decision in this doc;
> bare **DN** = a DECISIONS.md entry.

## 1. Problem this solves

The five parallel language profiles (`src/lang/{rust,python,typescript,go,kotlin}.rs`)
duplicate *algorithms* — the `lower_recursion` / `rewrite_tail_sites` /
`reassign_stmts` family is ~95% identical across profiles (`src/lang/rust.rs:300`
≈ `src/lang/python.rs:330`), differing only in a handful of node-kind strings.
The root cause is structural: **normalization is written against each language's
tree-sitter CST kinds**, so every rewrite must be re-expressed per language.

D44 rejected borrowing an *execution* IL (JVM/CPython bytecode, LLVM/MLIR) as the
substrate — it forfeits the four properties reprise is defined by (buildless,
tree-structured evidence, toolchain-deterministic baseline, uniform matrix
coverage). The constructive path D44 pointed at is a **purpose-built IR, authored
not borrowed**. This document designs it.

This is a deliberate redesign, not an accretion: the current normalization is a
set of passes bolted onto a lightly-normalized CST. The IR is designed up front as
a *canonical form*, so the equivalences we want hold **by construction** instead of
being reconciled by later passes.

## 2. Core principle — an IR for similarity, not execution

An execution IR optimizes for unambiguous semantics + efficient dispatch. A
**similarity IR** optimizes for a different, in-tension pair:

- **Convergence** — equivalent-but-differently-written source maps to the *same*
  IR. This is what makes a clone detectable (recall).
- **Discrimination** — genuinely different logic maps to *different* IR. This is
  precision.

**The node-set granularity _is_ the similarity model.** Modeling `match` as a
generic `Branch` converges if-chains with match (recall up, precision maybe down);
keeping it distinct does not — Branch is since decided to unify (§14). Therefore the IR is **not designed in a vacuum — it
is calibrated** against the existing mutation-recall gate and the spec §7.2 precision
sample. Design a *minimal* IR, validate convergence empirically, and grow it only
when a benchmark FN/FP demands a distinction — the spec's §12 rule-addition
discipline (D11/D25), applied to the IR node-set itself.

## 3. Scope

**In scope (front half — the greenfield):**
- The IR node-set (a closed, language-neutral vocabulary).
- Per-language frontends: `tree-sitter CST → IR`, lowering **directly to canonical
  form** (no post-passes reconciling loop forms).

**Deliberately out of scope (preserved):**
- The matching/back half — `fingerprint`, `seq`, `tree`, `au`, `inline`,
  `group`, `report`. This is the hard-won differentiator (Shazam landmark
  constellations, suffix-array sequence tier, anti-unification templates) and it
  is **not where the duplication lives**. It stays.
- **tree-sitter as the sole frontend** (D44): buildless, error-tolerant, uniform
  across all 9 current+planned languages, Rust-embeddable. Unchanged.
- The calibration gates (`bench-mutations`, the wild corpus, the spec §7.2 method) as
  **fixed requirements**. Per the elegant-redo principle: keep the tests as the
  requirements, rebuild the implementation.
- Semantic/type resolution. Name-res and types belong to the *opt-in, requires-
  build deep-Type-4 tier* (D44 escape hatch), a separate effort — not this IR.

## 4. Design principles

1. **Canonical by construction, not lowered after the fact.** The break from
   NormNode. Today `while`/`for` exist as distinct nodes that passes reconcile,
   and the passes must byte-match tree-sitter's exact CST shape so hand-written and
   synthesized forms converge — the fragile part (D9/D24: any grammar bump shifts
   the shape). In the IR there is *one* `Loop`; every loop form + tail recursion is
   frontend-lowered directly into it. Convergence is structural, not a pass output.
   **This removes an entire class of grammar-upgrade fragility.**

2. **Small neutral node-set + a `Native` escape hatch.** ~15 core kinds. Any
   construct that has not *earned* a canonical form is `Native{lang_kind,
   children}`, matching only against the same `lang_kind`. This keeps the IR
   **complete** without modeling every language feature up front (mirrors Semgrep's
   generic-AST `OtherStmt`/`OtherExpr`). Kotlin `when` (D25 left native), TS type
   nodes, async — start as `Native`, graduate to a canonical node only when a
   benchmark case pulls them in. **"Neutral" means the passes don't branch on
   language — NOT that two languages' nodes are cross-comparable (they never meet;
   §12.1). The shared skeleton exists for algorithm reuse, not cross-language
   matching; keep it thin.**

3. **Two-layer identity, designed in.** reprise retrofitted this (D2): positional
   locals for the exact hash, masked for retrieval. Bake it into the node type:
   every node is `(structural_kind, optional_label)`; locals are positional by
   construction, free names/fields/types are `External(name)`, literals are
   `Lit(bucket, keep?)`. The masked/positional split becomes an IR invariant, not
   a bolted-on pass.

4. **Spans are a type-level invariant.** No IR node can be constructed without a
   source span. This preserves "findings map to real lines" and the pseudo-source
   template render (spec §4 / D1). The full provenance model — `Source` vs `Derived`
   spans, hash-excluded — is §15.

5. **The IR is a pure function of `(source bytes, grammar version, IR-scheme
   version)`.** No toolchain input, no host-ordering dependence. This is the
   determinism LLVM/bytecode structurally cannot offer, and it is what keeps the
   drift baseline (D40) stable across a base scan and a PR scan.

6. **A textual rendering is a _view_, not the substrate.** Build the IR directly;
   *print* it as an s-expression for golden tests and debugging. (This is
   essentially the `template:` block reprise already emits — the readable
   "assembly" form, without re-parsing it back in.)

## 5. Starter node-set

Contentious kinds are flagged ⚠ — these are calibration decisions (decide by
benchmark), not design decisions.

```
Unit{params, body}     Block{stmts}       Loop{body}        Assign{targets, value}
Return{val?}           Break              Continue          Call{callee, args}
Binop{op, lhs, rhs}    Unop{op, operand}  Index{base, idx}  Field{base, name}
Lambda{params, body}   Var{Local(pos) | External(name)}     Lit{bucket, keep?}
Native{lang_kind, children}              ← escape hatch; matches same lang_kind only

Branch{arms:[(guard, body)]}      DECIDED (§14): ONE ordered first-match conditional
                                  subsuming if-chains / match / when / switch / ternary
⚠ Iter{...}                       model the iteration protocol as explicit nodes
                                  (today: synthetic __has_next / __next)
(DispatchTable — dropped)         RESOLVED (§14): not a node, a recognized *shape* of
                                  Branch (all arms literal-eq guard + trivial body);
                                  keeps D30 language-agnostic
⚠ Try/catch                       Branch-family (§14 / D-IR-11): catch-list = ordered
                                  typed arms + bind; `Native`-first until structure settled
```

The ⚠ nodes are exactly where convergence vs discrimination is decided. Resolve
each by measurement, not taste.

## 6. Frontend contract

Each language frontend is `fn lower(cst: tree_sitter::Node, src: &str) -> Ir`.
Per-language code then splits cleanly:

**De-duplicated (the win — front-half algorithms currently copied across profiles):**
loop lowering, recursion/tail detection, iteration rewrite, and order canonicalization
move onto the IR, written once. The `lower_recursion`/`reassign_stmts` family stops
being per-language because it operates on `Ir::Loop`/`Ir::Call`, not
`"call_expression"` vs `"call"`. **This is the duplication the effort exists to
kill, gone by construction.** (The back-half algorithms — anti-unification,
fingerprinting — are *already* language-agnostic on NormNode; §3 (Scope) keeps them
unchanged, and they are not part of this win.)

**Stays per-language (irreducible):** the classification tables (`literal_bucket`,
`is_function_like` — data, not duplication) and the CST→IR *mapping* + genuine
semantic quirks (Rust block-tail return vs Python explicit return; Go multi-return;
`Native` graduations). Smaller, and containing **no duplicated algorithms**.

Honest accounting: per-language code does not vanish — it shrinks to "mapping +
quirks." The win is eliminating **algorithm** duplication.

## 7. Decisions for sign-off

These change topology (a shared abstraction across five deliberately-independent
profiles) and an interface seam (a frontend contract replacing the
`LanguageProfile` normalization hooks). Surfaced explicitly, not folded into steps.

- **D-IR-1 · Container.**
  **(a) Designed contents, same container** — keep the `NormNode` shape
  (`kind + label + span + children`) but define a closed neutral kind vocabulary
  and lower directly into it, retiring per-language post-passes. Back half
  untouched (it already matches on `kind`). ~80% of the value at ~20% of the risk.
  **(b) Typed IR** — replace stringly-typed `kind: Box<str>` with `enum Ir {
  Loop{..}, Branch{..}, … }`. Exhaustiveness-checked, self-documenting, but
  rewrites the back half. → **Recommendation: (a).**
  **RESOLVED (2026-07-04): (a)** — same container; the IR path feeds
  `fingerprint`/`seq`/`tree`/`au` unchanged (canonical `NormNode` matches on `kind`).

- **D-IR-2 · Back half.** Keep `fingerprint/seq/tree/au/inline` unchanged (it is
  not where the duplication is). → **Recommendation: keep, out of scope.**

- **D-IR-3 · Trait migration.** Does the IR *replace* the `LanguageProfile`
  normalization hooks outright, or *coexist* behind a flag during migration
  (parity-gated switchover)? → **Recommendation: coexist behind a build flag until
  parity is proven on all languages, then retire the hooks.**
  **RESOLVED (2026-07-04, user): coexist via a non-boolean, plugin-extensible
  `[normalize] normalizer` selector (NOT a build flag) — the IR is one plugin and more
  may register later. Wired at `unit.rs`'s extraction seam (`normalizer = "ir"` →
  `frontend::extract_ir_units`); default stays `"historical"`.**

- **D-IR-4 · Node-set growth discipline.** Start minimal; a new canonical node
  (any ⚠) requires a driving benchmark case, per spec §12 (D11/D25). → **Recommendation:
  adopt the driving-case rule explicitly.**

- **D-IR-5 · Frontend.** tree-sitter remains the sole parse source (D44). Reach
  for a native Rust parser (`swc`/`oxc` for TS, `syn` for Rust) only if a specific
  CST proves to be the bottleneck. → **Recommendation: tree-sitter only for now.**

> **Full D-IR register:** D-IR-1…5 above; **D-IR-6/7** (ANF naming granularity;
> e-graph threshold) in §13; **D-IR-8/11** (fallthrough & arm-sort; try/catch structure)
> in §14; **D-IR-9** (reversal strength — RESOLVED), **D-IR-10** (log lifecycle — RESOLVED),
> and **D-IR-12** (transform-seam enforcement — leaning detect-emit) in §15.
> Open items are P1-gated (§8).

## 8. Validation method

1. Design a minimal IR (§5, ⚠ nodes started as `Native` or their simplest form).
2. Build frontends for **Rust + Python first** (richest benchmark coverage).
3. Run `bench-mutations` + the wild corpus + a spec §7.2-style spot check.
4. Adjust node granularity where convergence/discrimination misses.
5. **Recall-parity gate vs the current binary** (the D27/D32 method,
   `benches/compare/pair_ab.py`): zero lost reportable member-pairs on serde/ripgrep
   before switchover. **Parity = at least as good** (no recall regression), NOT
   byte-identical findings (user, 2026-07-03): the IR legitimately finds *more* where
   it fixes historical grammar-shape fragility (principle 1), and such gains pass the
   gate — only regressions fail it.
6. Only then add the remaining three frontends, each re-passing the gates.

## 9. Phased rollout

- **P0 — Design** (this doc): node-set + contract + sign-off decisions.
- **P1 — IR + Rust/Python frontends** behind a flag; parity-gated (§8.5). Node-set
  ⚠ decisions get *answered by measurement* here.
- **P2 — Remaining frontends** (TS, Go, Kotlin), each gated.
- **P3 — Retire** the `LanguageProfile` normalization hooks once all languages hit
  parity. Bump `cache::FINGERPRINT_SCHEME`/`EXTRACTION_VERSION` (D19/D30) — the IR
  changes the canonical form, so every cache key must move.

## 10. Non-goals

- Semantic/type resolution or true behavioral Type-4 (that is the separate opt-in,
  requires-build tier — D44 escape hatch).
- Cross-language matching (same-language-only stays, spec §3).
- Rewriting the matching engine.
- Any change to the buildless / error-tolerant / single-static-binary profile.

## 11. Relationship to the spec

Reframes docs/PLAN.md §5.2 from "a pipeline of normalization passes over a CST"
to "frontends lower directly to a canonical IR." The desugaring *inventory* (§5.2.2
loop lowering, recursion, iteration protocol; §5.2.4 identifier abstraction;
§5.2.5 literal buckets; §5.2.6 order canonicalization) is preserved as
*requirements*; only where that work lives changes. Continues DECISIONS.md D44:
the substrate is now being **designed**, not merely chosen.

## 12. Prior art — lessons

> Survey of multi-language IR tooling landed (2026-07-03); distilled in §12.1.
> Headline: same-language-only (spec §3) makes reprise **immune** to the universal-node-set
> trap that archived Babelfish and GitHub `semantic`.

**The load-bearing fork: normalize-in-IR vs equivalence-at-match-time.** Semgrep's
`AST_generic` is the closest shipping artifact to this IR (per-language parser →
one generic AST → one engine, proven at 30+ languages) — but it deliberately
*barely normalizes*: it keeps `While`/`For`/`Foreach`/`DoWhile` as distinct
constructors and handles equivalences (`x += 1` ≡ `x = x + 1`, assoc/comm,
deep-ellipsis) as **match-time equivalence rules**, because its contract is
"user patterns look like source." reprise's workload is the opposite — **all-pairs
bulk similarity needs a canonical form for hashable equality**, which you cannot
get by expanding equivalences per pair at match time. So: take Semgrep's node-set
*coverage and structure* as the reference, but normalize far *past* where Semgrep
stops. This is why §2's aggressive-convergence bias is correct rather than reckless
— reprise is the one workload where normalize-into-canonical-IR is *required*, and
it can afford it precisely because it exposes no pattern language to users.

Concrete borrowings:

- **Semgrep — category-specific escape hatches, not one `Native`.** `AST_generic`
  has `OtherStmt`/`OtherExpr`/`OtherPat`/`OtherType`, each with a *tag* enum for the
  specific construct so two different "other"s never spuriously match. → Refine §5's
  single `Native` into per-category hatches carrying a discriminating `lang_kind`.
- **Semgrep — resolution is a separate layer.** Name/scope resolution is a distinct
  `Naming_AST` pass *over* the generic AST, not baked in. → Keep resolution out of
  the core IR; it belongs to the opt-in Type-4 tier only.
- **Semgrep — the cautionary tale.** "Generic" drifted toward a *union of every
  language's features* — maintenance burden + spurious-match risk. Same-language-only
  (spec §3) shields reprise from the match risk; D-IR-4's driving-case rule shields the
  burden. → Aim *smaller* than Semgrep, on purpose; push variance to the hatches.
- **GumTree — the `(type, label)` minimal tree** is exactly the `(kind, label) +
  span + children` shape, and validates it for both matching and template/diff work.
  Its other lesson — matching needs *stable node identity across edits* — is what the
  Shazam landmark layer already supplies.
- **NiCad — normalization as a knob; consistent-vs-blind rename = D2.** Its thesis
  (normalize + flexible pretty-print, configurable aggressiveness) is reprise's, and
  its consistent/blind-rename split is D2's positional/masked split arrived at
  independently. → Treat each ⚠ node's normalization as a calibrated dial.
- **Deckard — a closed vocabulary pays a second dividend.** Characteristic-vector
  retrieval (skipped in D12) only works over a *fixed, small* node-type set, so
  principle-2's closed kind-set keeps the D12 revisit (vector/LSH retrieval if
  landmark volume forces banding) available for free.
- **CodeQL — write the node-set down as a schema.** Its per-language `dbscheme` is a
  formal, versioned AST schema extractors must conform to and tests check against. →
  Specify the IR node-set as a written, versioned schema (not just Rust types) so
  five frontends have one conformance target.
- **A-normal form / MIR — a candidate canonicalization.** ANF let-binds every
  intermediate value, which would converge `f(g(x))` with `y = g(x); f(y)` (the
  extract-a-temp refactor — a real clone pattern). → Flagged as a **P1 ⚠ benchmark
  case** (§8): adopt only if it earns recall without wrecking precision. §13 adopts
  ANF as the decomposition family's chosen direction.
- **tree-sitter queries (`.scm`) — the tabular half of the frontend could be data.**
  Classification (which nodes are functions/literals/comments) could be declarative
  queries rather than hand-walked Rust; structural lowering (§6) cannot. → Consider
  for the mapping half of the frontend contract; a modest DRY win per language.

### 12.1 Survey findings (2026-07-03)

Full per-system synthesis is in the session transcript; the design-changing distillation:

- **One correction — the node-set is per-language, not a universal vocabulary.**
  Babelfish's UAST and GitHub `semantic`'s open-union core died the same death: a
  "universal" cross-language node vocabulary collapses to a trivial common
  denominator while everything language-specific leaks back to per-language handling —
  pure maintenance debt. reprise is **immune** because matching is same-language-only
  (spec §3), so the shared vocabulary exists **only to make the algorithms language-agnostic
  (one `lower_recursion`), NOT to claim Rust's `Loop` is comparable to Python's**
  (they never meet). Reframe principle 2: "neutral" = *the passes don't branch on
  language*, not *the nodes are cross-comparable*. Keep the shared skeleton thin; push
  variance to per-language lowering + the escape hatch. (Babelfish, GitHub `semantic`,
  Rascal/M3's "language-specific first, thin shared layer second".)
- **Generate the CST→IR lift from the grammar — do not hand-write it.** `semantic`'s
  fatal cost was hand-written per-language "assignment" passes ("to recover richer
  structure you essentially have to parse the parse tree"), brittle across grammar
  bumps. This **elevates §12's tree-sitter-queries idea from "modest DRY win" to
  structural imperative**: derive the mapping declaratively (queries / a TSG-style
  construction DSL), version-pinned, so a grammar bump can't silently move a hash.
  (GitHub `semantic` ✗, stack-graphs ✓.)
- **Spans are a decoupled side-channel, excluded from the identity hash; keep two
  views.** srcML wraps (not replaces) source for byte-exact provenance; M3 decouples
  location as URI values. → nodes carry spans but the structural hash is span-free
  (reprise already does this, D1); keep a byte-anchored *surface* view alongside the
  span-stripped *hashable* view (surface feeds the template). See §15.
- **The escape hatch must stay inert — an opaque node, never a resolver call.** Infer
  only ingests via the compiler; Fraunhofer cpg's LLVM-IR fallback is explicitly *not*
  forgiving and needs compiled IR. → `Native` is a closed, opaque, resolution-free node
  (matches same `lang_kind` only); never enrich it.
- **Closed versioned enum + snapshot-testable serialization = the determinism
  harness.** Open unions (`semantic`) and open extensibility (Kythe) drift. SCIP beat
  LSIF mainly on readable IDs enabling cheap snapshot testing. → principle 6's textual
  view becomes the **primary drift/regression harness** — a readable, content-addressable
  dump, snapshot-tested per fixture. This is concretely how §8's "evaluate" gate is
  mechanized.
- **Two independent hash channels: structural shape vs leaf tokens.** CCFinder's
  hand-tuned per-language `$p`/namespace/template stripping is the leaky-maintenance
  anti-pattern; code2vec/SourcererCC validate keeping *shape* separate from *endpoint
  tokens*. → confirms D2's positional/masked split; keep it as two toggleable channels.
- **New watch-item — large atoms need a sub-leaf channel.** difftastic's real pain was
  big string literals/identifiers being single atoms when users want word-level
  matching. reprise will hit this with large literals → plan a secondary intra-leaf
  channel (or a length cap) before it bites.
- **Validations (now evidence-backed):** stop at the tree — no CFG/PDG, where every
  graph-IR (CPG/Joern) became build-coupled + non-deterministic (§10 holds); a cheap
  candidate-pruning index before exact pairwise scoring is mandatory (SourcererCC's
  inverted index = the landmark constellation); an AST alone can be the whole IR
  (Truffle); per-unit purity — zero cross-file state feeding the hash — buys determinism
  + incrementality together (stack-graphs).

## 13. Normalization families — the decomposition ladder (design commitment, 2026-07-03)

reprise's normalization is a set of *canonical-form families*. Two already ship: the
**control-flow family** (loop-form unification + recursion⟷loop — the latter already a
functional-decomposition canonicalization) and the **order family** (commutative /
key sorting, D11). Accepted here: a third — the **functional-decomposition family**,
canonicalizing how a computation is split into sub-expressions, temporaries, and
helper calls. Its call-level member already exists as the Phase-3 inliner
(`inline.rs`); the IR reframing may fold it in. Add the rest as a ranked ladder,
sound→unsound, each gated by a driving case (D-IR-4) AND the spec §7.2 precision sample
(risk 1) — i.e. every rung is a **hypothesis evaluated in P1 (§8)**, not a commitment:

1. **Intermediate-variable normalization (ANF).** Name intermediates uniformly rather
   than inlining them. *Total* (no single-use/side-effect guard, no skipped cases) and
   confluent — satisfies risk-2's monotone-direction requirement cleanly. Handles
   multi-return/tuple binds natively (below). High value: extract-/inline-variable is
   among the commonest refactors and the commonest way an agent "rewrites differently."
2. **Return/guard canonicalization** (guard-clause ⟷ nested-if; early-return). Partly
   present (`normalize_loop_exit`).
3. **Error-propagation canonicalization** ⚠ (Go `if err != nil { return err }`, Rust
   `?`, explicit `match` on `Result` — one early-return-on-error idiom, wild surface
   variance). Big real-world prize (Go especially) but a D30-class precision trap:
   err-checks are ubiquitous boilerplate, so canonicalize the *form* to converge the
   surrounding logic but **discount the boilerplate itself as a finding** (as dispatch
   tables D30 / machinery holes D14 are). Own ⚠.
4. **Method-chain ⟷ nested-call, then combinator ⟷ loop** — the frontier;
   unsound-ish, precision-risky; `Native`-first, graduate only with driving cases.

**Direction — named intermediates (ANF), NOT collapse-to-nested.** The `(data, err)`
multi-return case is the clarifier. Collapse (inline single-use temps) *refuses to
fire* on a tuple bind — one call feeds two bindings, so inlining would double-call —
so it is inert on exactly Go's most common shape. ANF is strictly more canonical: it
is total (no soundness guard), treats the tuple bind as *already canonical*
(`let (t0,t1)=f()`, positional names) instead of a skipped case, is the
authored/source-mapped form of the SSA "name every intermediate" instinct that made
bytecode feel canonical (the good half, minus D44's build/CFG/determinism costs), and
its generated temps are just more positional locals — which identifier abstraction
(D2) already handles.

**Not a binary — a granularity dial (D-IR-6).** The real choice is *which* intermediates
get named. Name at **call-result and destructure boundaries** (where `(data,err)` lives
and extract-temp refactors happen): clear yes. Name **pure operator subtrees** (`a+b` →
`let t=a+b`): probably no — that shreds the nesting the tree tier (subtree hashing,
landmarks, AU) feeds on. Default: name effectful/call/bind boundaries, leave pure
expression trees nested. P1 measures.

**Risk 1 — precision is the binding constraint.** Decomposition canonicalization
*increases convergence*; precision is reprise's softer number (spec §7.2). Every rung
validates against the precision sample, not just mutation recall.

**Risk 2 — the tree→sequence shift.** Naming intermediates flattens nested expressions
into `let`-sequences, moving load off the tree tier onto the sequence tier. May *help*
near-miss alignment (extract-temp becomes a sequence insertion — Smith-Waterman's
strength, D14) or *dilute* discrimination (functions become look-alike flat let-lists).
Genuinely uncertain → the primary P1 measurement. Companion cost: the flagship template
must **re-nest ANF for display** (match flat, render nested — the matching form and the
display form need not be the same; mechanism in §15).

**Risk 3 — order-sensitivity / confluence.** A fixed pass order can fail to converge
equivalent inputs a different order would. ANF's totality/confluence largely defuses
this for the decomposition family; competing-direction rules elsewhere remain the
signal that equality saturation (e-graphs / egg) starts to earn its keep (D-IR-7).

### Open sub-decisions (sign-off)
- **D-IR-6 · ANF naming granularity.** Name intermediates at which boundaries? Leaning
  call-result + destructure (yes), pure-operator subtrees (no). P1 measures.
  (Supersedes the earlier collapse-to-nested lean — the tuple case flipped it.)
- **D-IR-7 · e-graph threshold.** Stay with the ordered pipeline until a needed
  canonicalization requires competing directions; revisit egg only then. Recorded so
  the decision has a trigger, not a vibe.

## 14. Branch — the unified conditional (design commitment, 2026-07-03)

One canonical `Branch { arms: [(guard, body)] }`, ordered, first-match-wins, subsuming
if / else-if chains, `match`/`when`/`switch`, and ternary. Resolves the §5 ⚠ Branch
(unify: yes) and ⚠ DispatchTable (below).

- **`else`/`default`** = a final arm with a trivial (always-true) guard.
- **Ternary** `a ? b : c` = a 2-arm Branch.
- **Value-producing / branched-assignment (§14 core).** Branch is usable in expression
  position, and the canonical rule hoists a common arm-tail effect out: when *every*
  arm's tail is the same operation on the same target — `lv = e` or `return e` — hoist
  it and bind/return once. So `x = c ? a : b` ≡ `if c { x=a } else { x=b }` ≡
  `x = if c {a} else {b}` ≡ Python `x = a if c else b` ≡ Kotlin `x = if(c) a else b` /
  `when` all converge to `x = Branch{…}`; the return sibling (`if c { return a } else
  { return b }` → `return Branch{…}`) is the same rule with `return` hoisted (ties to
  §13 rung 2 / `normalize_loop_exit`). Hoist-out is ANF-consistent (one binding site).
  Requires all arms end in the *same* target op; partial → no hoist. Evaluation-gated
  (§8).
- **Pattern arms lower to guard + binds.** `match subj { Pat => body }` →
  `(matches(subj,Pat), [binds(Pat,subj); body])`: the pattern test folds into the guard,
  its destructuring becomes binding statements (ANF-named per §13) prefixing the body.
  Honest note: "one branch type" does **not** make patterns free — it *relocates* pattern
  handling into each frontend's lowering (§6), since patterns are the most
  language-divergent surface (Rust patterns, Python structural patterns, Kotlin `when`,
  Go type switches, TS discriminated unions).
- **Value arms fold the subject into the guard — the mechanism that converges switch/match
  with if-chains (landed, IR-selectable via `normalizer = "ir"`, 2026-07-04).** A
  subject-as-separate-element form was tried first and *blocked* this convergence (a bare
  `if subj == 1` has no subject element to line up against a switch's), so the subject
  folds into each guard instead: a value arm `case 1:` → `subj == 1` — **byte-identical to
  an `if subj == 1` condition** — a multi-value arm → `subj == a || subj == b`, and a
  pattern arm stays `matches(subj, Pat)` (deliberately *not* an equality, so genuine
  pattern-matches stay distinct from value dispatch). If-chains flatten to match:
  `else if`/`elif` splice their arms into one ordered Branch rather than nesting, and Rust
  `match` bodies are block-wrapped. **Verified across Rust/Python/Go:** a value switch/match
  now converges with the equivalent if-chain in all three; pattern-matches stay distinct;
  the dispatch-table FP is left to D30 fold-but-don't-report, as the design intends. (Bears
  on **D-IR-11**: it argues against carrying `match`'s subject as a separate Branch element,
  at least in the matching projection.)
- **Fallthrough switches** (JS/TS/C without `break`) don't fit an arm list; lower the
  break-terminated case (the common one), leave true fallthrough `Native`-first.
- **D30 goes language-agnostic (resolves ⚠ DispatchTable).** A dispatch table is not a
  separate node — it's a *recognized shape* of Branch (every arm a literal-equality
  guard + trivial body). D30's fold-but-don't-report suppression checks that shape on
  canonical Branch, retiring the per-language `is_dispatch_arm` hook.
- **Try/catch is a Branch-family construct (user, 2026-07-03; D-IR-11).** A catch-clause
  list is an *ordered, first-match, type-guarded arm list* — exactly the Branch shape:
  `catch (E e) { … }` lowers to an arm `(catches(E), [bind e; handler])`, reusing the
  pattern-arm lowering (the exception type is the guard, the caught variable is the
  pattern bind); a bare `catch` / `except:` is the trivial-guard else arm. Converges
  Java/Python/Kotlin/TS/C++/C# try/catch. Honest wrinkles: (a) the guard is a
  *region-throw*, not a point-test — the "subject" is the *effect* of running the
  protected block, not an expression; (b) `finally` has no arm analog (a trailing
  unconditional block — the loose end); (c) we model the **syntactic shape** (protected
  region + ordered typed handlers + finally), NOT the exceptional control-flow graph —
  §10 stops at the tree. Cross-connection: Go has no try/catch — its error handling is
  the `if err != nil` idiom in the §13 rung-3 error-propagation family; whether the two
  *cross-converge* is a separate, precision-risky Type-4 question, deferred.
  **`Native`-first until D-IR-11 settles the structure** (escape-hatch discipline),
  graduating with a driving case.

**Arm ordering.** Order is semantically load-bearing (first-match-wins), so a general
Branch is *ordered* — arms cannot be sorted (unlike commutative operands). Exception:
arms with provably-disjoint guards are order-insensitive and *may* be sorted for
convergence (`x==1/x==2/x==3` should converge regardless of order). Disjointness is
cheap only for the literal-equality case — also the D30 shape — so sort arms iff
literal-disjoint, keep source order otherwise.

**Risk.** Unification increases convergence → spec §7.2 gate. Lower-risk than §13's frontier:
its main false-positive source (idiomatic dispatch tables) is already suppressed by
D30, which this makes cleaner, not looser.

### Open sub-decision (sign-off)
- **D-IR-8 · Fallthrough & arm-sort.** (a) Fallthrough switches `Native`-first vs
  lower-by-body-duplication → leaning `Native`-first. (b) Sort literal-disjoint dispatch
  arms vs keep source order → leaning sort-when-disjoint (matches D11 + the D30 shape).
  P1 decides.
- **D-IR-11 · Try/catch structure.** A dedicated `Try` node in the Branch family vs.
  `Branch` gaining an optional protected-region + `finally` element (the latter also
  subsumes `match`'s explicit subject). Catch-arms reuse the pattern-arm lowering either
  way. `Native`-first until settled; P1 + a driving case decide.

## 15. Provenance & reversible transforms (design commitment, 2026-07-03)

Two coupled requirements: **every IR node carries source-mapping data**, and **every
transform is recorded canonically so it can be reversed.** They serve one end —
matching runs on the aggressively-normalized IR, but all *evidence* (templates,
divergence holes, findings) must render back in the user's own source. Extends
principle 4 and the §12.1 spans-as-side-channel lesson.

**Per-node provenance — never absent:**
- `Source(span)` — a real byte span (nodes mirroring source).
- `Derived(transform_id, spans[], role)` — a synthesized node (ANF temp, lowered loop
  core, hoisted branch value), tagged with the transform that created it and the *set*
  of source spans it descends from (a hoisted `x = Branch{…}` descends from every arm's
  assignment — hence a set, not one span).

Every node thus maps to real lines directly or transitively; "findings map to real
lines" survives arbitrarily aggressive normalization. **Provenance is hash-excluded**
(like spans): two clones that used different source forms must still collide.

**Reversible transforms — witness model.** A normalization is `N: Source→IR` plus a
**reversal witness** `W` with `N⁻¹(IR, W) → Source`:
- *Bijective* (paren drop, commutative sort) → `W` empty/trivial.
- *Lossy* (loop unification `for`/`while`/`foreach`→`Loop`; ANF naming; branch hoist) →
  `W` records exactly the discriminator normalized away ("this Loop was `for x in xs`";
  "this temp inlines subexpr at span S").

The per-unit **transform log** is an ordered, **canonical** (deterministic: same source
→ same log) list of `(kind, locus, witness)` over a **closed, versioned transform enum**
(LoopLower, RecursionLower, AnfName, BranchHoist, CommSort, ParenDrop, LitBucket, …), schema'd like
the node-set (§12.1). Reverse = replay inverses in reverse order.

**The log is also explanatory (bonus).** Two members of a clone group can carry
*different* logs — one a `for`, one a `while`; one a ternary, one an if/else. The **diff
of their logs is a source-level explanation of the convergence** and localizes AU holes
back to real syntax. Provenance + log together are the mechanism that lets the template
**un-lower for display** (the §13 "match flat, render nested" step) instead of showing
ANF soup.

**Losslessness is relocation, not retention effort (user, 2026-07-03).** "Normalize
aggressively" and "don't lose data" are not in tension: normalization does not *destroy*
the `for`-vs-`while` discriminator, it *moves* it out of the hashable tree into the
transform witness. The **matching projection** is lossy by design (that is how clones
converge); the **artifact** (canonical tree + transform log + provenance) is **lossless**.
Reversal yields **display/explanation only — never runnable code** (no compile-from-IR,
no emission). We reverse to *show*, not to *run*.

### D-IR-9 · Reversal strength — RESOLVED
Not (a)-vs-(b): **retain full completeness (the artifact loses no data — every lossy
transform stores its witness), but the only *consumer* is display/explanation.** No
un-normalized code emission, no compile-from-IR. The retained witnesses still buy the
free correctness harness `N⁻¹(N(x)) ≡ x` (bijective) / `≈ x` (lossy) — the "evaluate"
gate applied to the normalizer.

### 15.1 The event-sourced framing (model, not stack)
The transform log is an **append-only, ordered, deterministic event log**: `source` is
genesis, each transform an immutable event (kind + locus + witness), and every view is a
**projection** (fold) — the span-stripped structural hash, the token channel, the
source-faithful display template, the format emitters (SARIF/JSON/CPD, which already are
exactly this). Reversal-for-display = replay / inverse-fold. This is CQRS/ES, and it is
the right conceptual spine: **append-only ES makes "don't lose data" a property
guaranteed by construction**, and normalization becomes *appending* transform-events, not
destructively editing information away.

**Adopt the model, not the machinery (pushback, per request):**
- **Source is the source of truth, not the event log.** Transforms are a deterministic,
  *re-derivable* function of the source — unlike real ES, where events are irreplaceable
  ground truth. So "persist events forever, never lose them" is unnecessary: always
  re-normalizable. It is a derivation *trace*, not an event *stream* — the sharpest
  divergence from ES.
- **In-process, one-shot, microseconds.** ES's apparatus (event store, aggregates, async
  projections, eventual consistency, sagas, durable persistence) solves
  durability/concurrency/distribution problems reprise does not have; importing it is
  ceremony without payoff.
- **When the stack would earn its keep (trigger, cf. D-IR-7):** if reprise ever wants
  **incremental within-unit re-normalization** (edit one statement → replay only the
  affected transform-events into updated projections, not recompute the function), ES's
  replay/incremental-projection model pays off for real. Until then: model yes,
  infrastructure no.

### 15.2 Un-applying transforms, and the event stream's standalone value

**Reversal = un-apply (replay inverses) — one caveat.** To reverse, replay the transform
events in reverse order applying each inverse. The caveat is *why the witness exists*: a
**lossy (many-to-one) transform has no functional inverse from its kind alone** —
"un-apply LoopLower" is ambiguous (`for`? `while`? `foreach`?). So the event must carry
its discriminator *as payload*. The witness is therefore **not a separate mechanism — it
IS the event's payload**, exactly the ES rule that an event carries enough to be replayed.
"Un-apply the transform" and "apply the inverse with the witness" are one operation. Even
bijective transforms carry a minimal payload (paren-drop records *where*; commutative-sort
records the original order) — "trivially invertible" still means "the event records the
removed bit."

Refinements:
- **Global/positional transforms don't un-apply purely locally.** ANF temp numbering and
  identifier abstraction are positional (the D2 cascade — one insert shifts the rest).
  Reverse-order replay still works, but those events must capture positional context, not
  just a local edit.
- **For display, per-node provenance is often cheaper than replay.** Each node carries its
  source span(s); rendering the original at those spans + marking synthetics is
  source-faithful without replaying anything. Use inverse-replay only where structure must
  be *rebuilt* (re-nest ANF, un-hoist a Branch) that spans alone don't convey. Two
  mechanisms; pick the cheaper per need.

**The event stream is valuable beyond reversal.** Kept as a first-class part of the
artifact, at analysis time it buys:
- **Match explanation.** The *diff of two members' logs* is a source-level account of why
  they converged ("one a `for`, one a `while`; one used `1`, one `2`") — a narrated
  derivation, not a bare similarity %. **TODO (factoring):** divergent `LitBucket`
  witnesses are *factorable* — lift the differing constant to a parameter and rewrite
  the members as calls passing it (currying / partial application). That is the README's
  expression-hole→parameter refactoring recipe, with the witness supplying the values;
  it belongs in the report (back half), not P1.
- **Normalizer debugging + per-transform calibration attribution (high-value).** A recall
  miss = diff the two logs to find where normalization diverged; a precision miss = find
  which transform collapsed an unrelated pair. Attributes each FP/FN to a *specific*
  transform and measures each transform's **marginal recall/precision** — directly the P1
  "evaluate" gate (§8) and the D-IR-4 driving-case discipline, mechanized.
- **Tunable aggressiveness as replay-depth.** Fewer transforms → *conservative* projection
  (precision); more → *aggressive* (recall). Realizes §12.1's "expose multiple aggressiveness
  levels, don't bake one" as a **projection choice, not a rebuild** (CQRS multi-read-model
  applied to normalization strength).
- **Incremental re-normalization** — the ES-stack trigger (§15.1), if it lands.

**D-IR-10 · Log lifecycle — RESOLVED: in-memory only, never persisted (user, 2026-07-03).**
The log is a deterministic function of source (§15.1), so it is valuable *and cheap*: held
**in memory for the current run and never written to disk** — explicitly **excluded from the
D19 per-file cache** (a warm cache hit restores the normalized tree, not the log). When a
consumer needs it — the reporter for match-explanation / template un-lowering, or the P1
calibration harness — the touched units are **re-normalized with logging on** (few units,
cheap). Persisting an event log *is* the CQRS/ES infrastructure we decline (§15.1); not
persisting it is that same decision from the other side.

### 15.3 The transform seam — the event log made structural (design commitment, 2026-07-04)

§15/§15.1 make the event stream the **computational spine**, but the P1 implementation followed
the model for *lowering* and abandoned it for *canonicalization*. `frontend::normalize` applies the
five shared passes as legacy `NormNode → NormNode` mutations with the log inert — `let tree =
rewrite_iteration(tree); normalize_loop_exit; abstract_idents; canonicalize_order; strip_dead;` —
then returns the lowering-only log. That chain **is** the pre-IR `apply_passes` pipeline in IR
clothes. The cost: five of eleven `TransformKind`s are never recorded (`IterProtocol`/`LoopExit`/
`CommSort`/`DeadStrip` + `abstract_idents`' name witnesses), and `canonicalize_order` sorts
operands **without recording the order** — a live D-IR-9 losslessness break (the discriminator is
*destroyed*, not relocated to a witness). Recording is optional because the passes are written in
the legacy shape, so it was skipped.

**The bar: a transform must be impossible without its event** — enforced by the type system, not
review. The realization: **passes become pure detectors; one applier is the sole mutator.**
- A pass is `fn detect(tree: &NormNode) -> Vec<Edit>` — an *immutable* tree in, `Edit`s out. With
  no `&mut`, it **cannot mutate**; "sort without recording" does not compile.
- `apply<S: EventSink>(tree, edits, &mut S) -> NormNode` is the only front-half canonicalization
  mutator, and it performs each edit **and** records its event atomically. Transform-without-event
  is unrepresentable.
- Every view is a **projection** over `(genesis, stream)`: the aggressive canonical tree (the
  matching form) is `apply`'s output; the display template, match-explanation, reversal, and
  calibration attribution are folds / inverse-folds. §15.1, made real.

**The sink is the read-model selector — this is how D-IR-10 discharges the always-on cost.** In
bulk scan the only projection materialized is the aggressive tree, so it passes a **`NullSink`**
(`record` is a no-op; generic `apply` monomorphizes it away) — zero recording cost, none of the
current build-then-discard waste. A consumer that needs the stream (reporter, calibration)
re-normalizes the handful of *touched* units with a recording sink (D-IR-10: the stream is a
deterministic, re-derivable function of source). Losslessness is *on demand*, never *always
materialized*. Witnesses stay cheap by default (indices / spans / source slices), owned data
materialized only at record time, so a disabled sink costs nothing.

**Lowering stays construct-and-record** — it *builds* the genesis tree rather than transforming a
prior one, so dual-write is inherent there. The compile-time guarantee is scoped to
canonicalization (the broken part); lowering is instead held to per-frontend **completeness**
(every normalization it performs emits its event — Go, the newest frontend, currently records the
least).

**Migration is parity-neutral.** The log and witnesses are **hash-excluded** (§15 / §12.1), so
completing the stream *cannot* change the canonical tree — every `u128` fingerprint and every
finding stays byte-identical. The gate is therefore a pure refactor invariant: `pair_ab` shows
**zero** canonical-tree diff (Rust/Python/Go, mutation-recall + wild corpus) before and after; the
sole observable delta is the stream going partial → complete. Each pass converts independently
under that invariant, so it interleaves safely with the live P1 frontend work, and the free
`N⁻¹(N(x)) ≈ x` reversal round-trip (D-IR-9) becomes checkable the moment a pass is wired.

**D-IR-12 · Enforcement level.** (a) a shared `apply` helper (discipline-enforced); (b) encapsulated
mutation (privacy); (c) **detect-emit — passes are `&NormNode -> Vec<Edit>`, one `apply`
mutates + records.** (a)/(b) leave silent mutation representable; only (c) makes it a compile error
and yields the §15.2 replay-depth read-models for free. → **Leaning (c) for the five
canonicalization passes; lowering stays construct-and-record.** P1.

**Open sub-decision (sign-off) — genesis purity.** Lowering currently interleaves normalization
(literal bucketing, paren-drop) into the genesis and records those events. A *pure* faithful genesis
— all normalization as post-lowering events — would enable replay-depth *below* what lowering bakes
in, but no consumer needs sub-lowering aggressiveness today. → **Leaning interleaved-now; pure
genesis gated on a replay-depth read-model earning it** (the §15.2 trigger).

**Implementation hand-off.** The concrete module shape (`ir::edit`), the per-pass detect→apply
recipe, the parity-gated landing order (`canonicalize_order` first — the live data loss), and the
gotchas are in [`docs/transform-seam.md`](transform-seam.md).
