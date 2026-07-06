# Working on reprise

reprise is a buildless, tree-sitter-based code-clone detector. The design corpus lives in
`docs/` — start with `docs/PLAN.md` (the spec), `docs/SIMILARITY-IR.md` (the IR that unifies
the frontends), `DECISIONS.md` (the decision log), and `docs/universal-ir-trap.md` (the
failure modes this project is defined against). Build, test, and lint go through `just`
(`just test`, `just lint`, `just fmt`, `just bench-mutations`, `just bench-mutations-ir`) —
an `ltk` hook redirects raw `cargo`.

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
