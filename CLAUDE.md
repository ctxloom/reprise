# Working on reprise

reprise is a buildless, tree-sitter-based code-clone detector. The design corpus lives in
`docs/` — start with `docs/PLAN.md` (the spec), `docs/SIMILARITY-IR.md` (the IR that unifies
the frontends), `DECISIONS.md` (the decision log), and `docs/universal-ir-trap.md` (the
failure modes this project is defined against). Build, test, and lint go through `just`
(`just test`, `just lint`, `just fmt`, `just bench-mutations`, `just bench-mutations-ir`) —
an `ltk` hook redirects raw `cargo`.

## Instrument first. Understand. Then change.

**No change to a mechanism lands on the strength of an argument about that mechanism.** Measure it,
understand what the measurement says, and only then decide what to build. This is the project's
first rule, and it is not ceremony — it is the rule the codebase keeps proving.

A partial list of things that were believed, written into plans, and turned out to be false —
each discovered only by measuring, and every one load-bearing at the time:

- *"The inliner's caps bound expansion."* They exempt the mutual-recursion SCC from **both** the
  depth and the size cap. One redis unit expanded to 25.1M nodes — 22× the whole corpus — and
  exhausted the machine's memory. The size cap was also the **precision** guard; skipping it
  manufactured clones out of a shared helper's mass.
- *"The landmark index is ~2.6 GB."* It is 8.49 GB. A whole `Vec<(u32,u32)>` — 2.15 GB — was in
  nobody's model.
- *"Extraction is a 22.5 GB hump, near-tier is another."* They were the same bytes, counted twice
  in two different sessions. Extraction's own peak is 12.54 GB; the peak is set in the near phase.
- *"The peak is 22.6 GB."* That is a **glibc dev-build artifact**. The shipped binary is
  musl + mimalloc and peaks at 15.8 GB. A whole optimization campaign was aimed at a build no user
  runs.
- *"The bag layer catches clone families landmark misses."* It has never proposed a single pair
  landmark did not. Zero yield, on every corpus, always on.
- *"`bench-retrieval` races real production code."* It **reimplements** the production helpers.

The pattern is always the same: a plausible model, written down once, never checked, then built
upon. **A number is either measured or it is labelled an estimate. Plans state which.** When a plan
asserts a row size, a ratio, or a bound, the first stage measures it — and the measurement may
delete the plan.

Corollaries that follow from the same rule:

- **Wall time on the dev box drifts ±50% batch-to-batch** (the same binary has measured 291s and
  443s). **Interleave every wall comparison against a control** (A/B/A/B within one batch). A
  non-interleaved read has already inverted two conclusions. Peak RSS is stable to <1%; wall is not.
  Prefer *within-run ratios*, which are immune.
- **`fs` and `drivers/` are different regimes, not different sizes of one** — the memory gate trips
  at one and not the other, so trees are freed in one and never in the other. Do not extrapolate
  between them.
- **Instrument so the next person cannot repeat this.** Where a mechanism's value is unmeasurable,
  the fix is a counter, not an argument. `verified_only_bag` did not exist, which is precisely why
  a zero-yield join ran on every scan for the life of the project.

## Cross-language: converge the *tooling*, best-effort the *behavior*

reprise supports many languages, and there are **two distinct axes** of cross-language "sharing."
Treat them very differently.

### 1. Shared tooling / implementation — converge aggressively where feasible

Prefer **one implementation that works across languages** over per-language reimplementations.
This is the thesis of the similarity-IR (SIMILARITY-IR §1, §6): frontends lower to a neutral IR
so the *algorithms* — loop/recursion lowering, canonicalization passes, the inliner, the
api-profile signature builder — are written **once**, not copied per language. When you find a
per-language copy of an *algorithm*, converge it onto the shared representation. The `Shapes`
enum (one canonical shim reused by the inline and api-profile tiers: `Historical(..)` delegates,
`Ir(Lang)` returns canonical constants) is the pattern — **share the mechanism, parameterize
only the few genuine per-language quirks.** Do this proactively; do not defer it silently.

### 2. Cross-language *behavioral* compatibility — a LIGHT, best-effort goal, never a requirement

Making a clone in one language *match* a clone in another is **not required, and must never be
forced.** Forcing a universal cross-language vocabulary is the major trap this project is defined
against (`docs/universal-ir-trap.md`; SIMILARITY-IR §12.1): it collapses to a trivial common
denominator while everything language-specific leaks back to per-language handling — pure
maintenance debt, plus spurious-match risk. Matching stays **same-language** (spec §3), and the
shared IR skeleton exists for **algorithm reuse (axis 1), not to claim two languages' constructs
are cross-comparable** (SIMILARITY-IR §2, §12.1) — keep it thin.

So: make **light efforts** toward cross-language behavioral convergence where it falls out
**cleanly and cheaply** — e.g. modeling Go/Rust/Python `i = i + 1` identically (the self-reference
`@place` rule: an assignment whose target appears in its own value is provably a mutation, no
scope analysis needed) — a small, sound, syntactic win, so we took it. But when convergence would
need **disproportionate effort, scope/type analysis, or forcing unlike constructs together** (the
opt-in, requires-build Type-4 tier — D44's escape hatch), **stop and leave it.** Don't force it.

### Rule of thumb

Aggressively DRY the **tooling**; treat cross-language **behavioral** convergence as a
nice-to-have pursued only when it's clean. Never trade the maintenance/precision cost of forced
universality for it. When a cross-language convergence is *tempting but costly*, that is a signal
to stop — not to build a universal abstraction.

## Promoting a construct into the shared IR — the gate, and the TDD discipline

Adding a construct to the canonical node-set, or lowering two languages' constructs to one node, is
an axis-2 (behavioral-convergence) decision. It is **desirable — factor out as much as is
*feasible*** — but "feasible" has a test, and it is the exact test the dead universal-IR projects
skipped (`docs/universal-ir-trap.md`).

**The gate — cross-language recurrence × exact-equality.**
- **1 language → stays in that language.** Native + a back-half override for its shape. A
  single-language construct earns no cross-frontend reuse from a shared node; promoting it is pure
  vocabulary-growth cost (the SP2 trap).
- **2 languages → probable promotion, but only if the two are *exactly* equal.** Two earns reuse
  but has no slack for a semantic mismatch — where SP3 bites (surface-alike, meaning-different).
- **3+ languages → indicative** (confidence rises with count), still equality-gated.

Orthogonal and always fine: a construct that reduces to a node **we already have** *lowers into it*
with no new kind (Python comprehension → `Loop`; Elvis → `Branch`). New kinds are rationed by the
gate; *using* existing kinds is not.

**Exact-equality is proven by a test corpus, never eyeballed.** Do not assert "≈" from taste. The rule:
1. **Capture real-world examples** of the construct in each candidate language, from actual code, into
   the corpus — unit invariants alongside `tests/convergence.rs`/`tests/lang_*.rs`, broader samples in
   `benches/wild`/`tests/wild.rs`. Synthetic one-liners are not enough; the invariants live in real variance.
2. **Write the invariants as tests first (TDD), red before green.** A *promotion*'s invariants:
   equivalent spellings **converge** (equal fingerprint) AND genuinely-different code stays **distinct**
   (precision). A *language override*'s invariants: the shape properties it must preserve.
3. **Then evaluate.** If the corpus tests hold across the candidates, "≈" is real — promote. If a
   language's version diverges under real inputs, it is "≠" — keep it per-language/Native. **The tests,
   not the argument, decide.**

Default when unproven: **stay Native / per-language.** (Example: try/catch recurs in ~6 languages and
*looks* promotable, but catch-binding semantics differ — so we keep it **by-language until a real
corpus proves the invariants.**)

**Do not execute the toolchain to handle a native construct.** Compile-time metaprogramming — the
C/C++ preprocessor, Rust `macro_rules!`/proc-macros, C++ templates — stays **Native, unexpanded**.
Running an expander needs build context (include paths, `-D` config, *which* preprocessor) and is
non-deterministic across environments; it breaks the buildless, toolchain-deterministic core (D44;
SIMILARITY-IR principle 5) and explodes `#include` into every file. Deep expansion-aware analysis, if
ever wanted, is the **opt-in requires-build tier** (D44's escape hatch, PLAN.md §11), never the default.
("C and Rust both have a preprocessor" is an SP3 lure — textual `#define` ≠ hygienic AST macros; do not
lump them. Even C vs C++ preprocessing must be *proven* equal by corpus before sharing a node.)
