---
title: The Universal-IR Trap — why it failed before, how reprise is immune, and where it still isn't
status: research synthesis (2026-07-04). Companion to docs/SIMILARITY-IR.md §12 / §12.1,
  which distilled the same survey to one paragraph; this is the receipts + the point-by-point
  immunity argument the user asked for.
sessions:
  - fatal-main-storm
related:
  - docs/SIMILARITY-IR.md §2 (similarity-not-execution), §3 (scope), §12/§12.1 (prior art),
    principle 1/2/5, §10 (stop-at-the-tree), §15 (losslessness-is-relocation)
  - DECISIONS.md D44 (author-don't-borrow), D9/D24 (grammar-shape fragility), D11/D25/D-IR-4
    (node-addition discipline)
---

# The Universal-IR Trap

> **One-sentence thesis.** A shared cross-language representation is a 60-year graveyard because
> "universal" is a *semantic* claim — the same node must mean the same thing in every language —
> and that claim is false. reprise survives the same graveyard by never making the claim:
> **matching is same-language-only (spec §3)**, so its shared node-set is an *engineering*
> convenience (write one `lower_recursion`, not five), not a promise that Rust's `Loop` is
> comparable to Python's. They never meet. Everything below is why that one move is load-bearing,
> and where it does *not* save us.

This document exists because the same-death survey in `docs/SIMILARITY-IR.md` §12.1 asserts reprise
is "immune" in a single paragraph without the evidence. Here is the evidence, the failure modes it
generalizes to, and the honest residual exposure.

---

## 1. The recurring dream — and why it keeps returning

Since ~1958 the field has repeatedly proposed **one canonical, language- and machine-neutral
intermediate form**: lower every source language *into* it once, lower it *out* to every target
once, and the cost of building tools collapses from **N×M** (N languages × M machines/targets) to
**N+M**. The arithmetic is seductive and "obviously good," so the dream keeps returning under new
names — UNCOL, ANDF/TenDRA, JVM/CLR bytecode, "just use C," LLVM IR, WebAssembly, Babelfish UAST,
GitHub `semantic`. It keeps failing, or narrowing drastically, for one structural reason: **the
single middle form cannot be simultaneously general enough to preserve every language's meaning and
concrete enough to be useful — and it always sacrifices the source-level structure that made it
"universal" in the first place.**

reprise deliberately continues D44's line — *author* the substrate, don't *borrow* an execution IL —
but it is building a shared cross-language IR, which is exactly this graveyard. So the question "are
we about to die the same death?" is not rhetorical. It has a specific answer.

---

## 2. The graveyard

| Project | Era | What it tried to unify | How it actually died | Best primary source |
|---|---|---|---|---|
| **UNCOL** | 1958 | One IL between all languages and all machines | Never fully specified or implemented — "more a concept than a language" | Steel, *A First Version of UNCOL*, WJCC 1961 |
| **ANDF / TenDRA** | 1990s | Architecture-neutral *distribution* format (OSF) | Technically sound, never commercially released; died on vendor-coordination cost | Wikipedia (ANDF), citing InfoWorld |
| **LLVM IR** | 2003– | (Often *mistaken* for) a universal IR | By design **not** universal, **not** source-level, **not** portable — lossy target-lowering | Lattner PhD thesis 2005, §2.2–2.3 |
| **MLIR** | 2019– | The field's *concession*: stop unifying the IR | Reframes the goal — shared *infrastructure* + many **dialects**, explicitly not one IR | Lattner/Shpeisman MLIR keynote 2019 |
| **Babelfish (bblfsh)** | 2017–19 | Universal AST (UAST): per-language driver → ~118 shared "roles" | Shared vocabulary couldn't uniformly extract even a *function name*; native detail leaked or dropped; 0 languages ever reached "stable" | bblfsh/sdk issues #364, #361 |
| **GitHub `semantic`** | ~2015–21 | One typed AST + one analysis engine across languages | "Ultimately unityped"; hand-written per-language mapping rotted on every grammar bump; the shared-analysis payoff never shipped | ICFP 2022 experience report (arXiv:2206.09206) |

Two of these — **Babelfish** and **`semantic`** — are reprise's *exact* neighbours (cross-language
representation for code *analysis*, both built on the same substrate we use, tree-sitter). They are
the ones with candid maintainer post-mortems, so they carry most of the weight below. The classic
compiler-IL lineage (UNCOL → LLVM → MLIR) supplies the deep theory.

### 2.1 The two neighbours, in one paragraph each

**Babelfish / UAST (source{d}).** One containerized parser *driver* per language piped its native
AST to a Go normalizer that annotated it into a Universal AST — a language-agnostic tree tagged from
a fixed vocabulary of ~118 semantic **roles** (`Identifier`, `If`, `Function`, …). The point was
"code as data": ML and feature-extraction across languages from one representation. It reached ~9
beta-quality drivers and a v2 "Semantic UAST" redesign before source{d} (the company) wound down in
late 2019. The proximate cause of death was the startup, but the *design* was already visibly stuck
(§3).

**GitHub `semantic`.** A Haskell library that composed each language's AST from reusable
cross-language syntax pieces (Wouter Swierstra's *Data types à la carte* open-union encoding), with a
hand-written per-language **"assignment"** pass mapping tree-sitter parse trees onto those typed
ASTs, and an abstract-interpretation evaluator meant to be *one* analysis engine reused across all
languages (the N+M dream, made real). It shipped GitHub's diff summaries and first "jump to
definition." It is now archived; the maintainers replaced its every core choice (§3, §4).

---

## 3. The sticking points

Eight recurring failure modes. The first three are the *"universal" collapse* proper; #4–#5 are the
*maintenance treadmill*; #6–#8 are the *it-never-paid-off / it-doesn't-scale* modes.

### SP1 — The high-vs-low pincer (the original "UNCOL problem")
A universal middle form is pulled in two incompatible directions. Pitched **high** (preserving
source semantics), it can't be lowered efficiently to divergent targets. Pitched **low** (a
"universal assembler"), lowering to any real machine reintroduces the same inefficiency you were
escaping — *and* throws away source structure. Lattner states LLVM's position outright: it "is **not**
intended to be a universal compiler IR … does not represent high-level language features directly …
nor does it capture machine-dependent features" (2005 thesis §2.2). Lowering to LLVM IR "eliminates
… source-level exception semantics … and a tremendous amount of other source-level detail" (§2.3) —
so "language-specific optimizations must be performed in the front-end." The universal form is
*structurally* forced to be lossy about exactly the source facts an analyzer wants.

### SP2 — Lowest-common-denominator vs. union-of-all-features
Make the shared vocabulary **narrow** enough to be genuinely common and it's too coarse — native
detail *leaks or is dropped*. bblfsh/sdk **#361**: "native objects that have a semantic mapping have
more properties than the ones we have in our schema … the latest SDK update produces an error when
you silently drop any field"; the alternatives were to drop information, add an `extra` field that
"turns the semantic objects into 'schema-less'," or hand-write per-driver transforms — "Very
cumbersome to write and maintain, ugly, fails the DRY principle." Make it **rich** enough to be
useful and it fills with language-specific special cases and stops being uniform; `semantic`'s
maintainers call the à-la-carte core "**ultimately unityped** … our approach discarded this
information … many runtime type checks, which obviated many of the gains from choosing a strongly
typed implementation language" (ICFP §6.3). Macrakis framed the same double bind for "just use C as
the IL": underspecified ("no way of specifying integer or floating-point precision") *or*
overspecified ("parameter passage is always by value … precludes the installer from choosing an
implementation strategy"). **There is no width that is both common and faithful.**

### SP3 — The semantic gap: identical syntax, divergent meaning
Even when two languages *look* the same, a shared node silently conflates them. `semantic`'s
canonical example: Python, Java, and Ruby all write `super()`, so it "should" be one shared node —
but Ruby's zero-arg "zsuper" implicitly forwards arguments, forcing a Ruby-specific type; the
verdict is "this problem domain is sufficiently complicated to, at times, **actively resist
abstraction**" (ICFP §6.3). Babelfish's v1 spec admits the same at the tree level: a typical `if`
gets an `If` role plus "**either** a `Statement` role for some languages (e.g. Go, Java) **or** an
`Expression` role for others (e.g. Scala)" — so a cross-language query over roles conflates
statement-`if` with expression-`if`. And bblfsh **#364**, the single most damning artifact, is filed
against the v2 layer *built specifically to normalize structure*: extracting a function's name needs
one XPath for Go/JS/Python/Ruby and a *different* one for Java/PHP — "**Therefore there is no common
way to get the function names.**" The universal layer couldn't uniformly do the most basic thing.

### SP4 — The assignment tax + grammar-churn brittleness
tree-sitter gives only a generic labeled tree, so recovering usable structure means a *second*,
hand-written grammar per language: "To recover a richer structure … you essentially have to **parse
the parse tree** (which is what we do with assignment)" (`semantic`'s own `why-tree-sitter.md`). That
mapping is welded to each grammar and rots on every bump, *with no compile-time safety*: "The system
was brittle. Each language's Assignment code was tightly coupled to the language's tree-sitter
grammar … it could break at runtime if we changed the structure of the grammar, without any
compile-time error … our system had inadvertently become **incentivized against iterative
improvement**" (CodeGen blog, 2020). The ICFP report (§6.3) calls assignment "a **reliable source of
bugs**" that "held us back from upgrading language grammars regularly." Babelfish had the same shape
— annotation rules pinned to native node names (`AnnotateType("IfStmt", …)`) — kept in lockstep with
a centrally-versioned role enum.

### SP5 — The m×n maintenance treadmill and no path to community ownership
Two grammars per language, per-language drivers, a monolithic core in one exotic language. Babelfish
after ~2.5 years and VC funding: **9 beta, 1 alpha, 0 stable** drivers. `semantic`: "A two-step
parsing process required writing two separate language-specific grammars by hand … time-consuming,
engineering-intensive, error-prone, and tedious … leveraging community support for adding languages
has been difficult because … it was backed by such a grueling process" (CodeGen). Decisively, the
whole edifice couldn't be *owned* by the people who care about each language: code navigation "was
later replaced by a domain-specific language for querying syntax trees … we needed to enable external
contributors to maintain their own code navigation rules without having to write Haskell code …
**Our production systems now invoke the Tree-sitter code directly, rather than being mediated by
Semantic**" (ICFP §7).

### SP6 — The shared-analysis payoff never materialized — it collapsed back to per-language
The entire justification for one AST is *one analysis reused across all languages* (turn m×n into
m+n). It was too heavy to ship. `semantic`: "At one time, we planned on implementing code navigation
using abstract interpretation, but in the end that implementation was pursued in a Rust service using
a notion of **stack graphs** … which allowed us to express scope-aware code navigation without the
time and effort associated with developing full abstract interpretation for all targeted languages"
(ICFP §4.4/§6.4). The universal semantic engine lost to a narrower, per-language, declarative
mechanism.

### SP7 — Even a *technically sound* universal IR dies on ecosystem-coordination cost
ANDF/TenDRA was genuinely well-engineered and was still "never widely adopted … never released
commercially by OSF or any of its members," partly because "having multiple installation systems
would complicate software support." A universal *interchange* form needs every vendor to ship and
validate a back-end; that coordination cost kills adoption independent of the technical merits.

### SP8 — Whole-program semantic models don't scale; file-incremental determinism wins
`semantic`'s whole-program typed-AST + abstract-interpretation posture never centered incrementality;
its successor's central contribution is that "at index time … we look at each file **completely in
isolation**," each file becoming a disjoint subgraph (stack-graphs paper, arXiv:2211.01224). At
"thousands of new code snapshots each minute," reanalyzing the world is quadratic and fatal. The
lesson MLIR states from the compiler side — "Duplication of infrastructure at all levels" — is the
same disease: the win is *not* a shared canonical representation, it's shared infrastructure over
per-unit, deterministically-derivable pieces.

**Root cause, all eight in one line:** *no single fixed representation is at once general (lossless
across languages' semantics) and useful (concrete enough to analyze), so every attempt sacrifices the
source structure that justified it — and then falls back, expensively and per-language, to recover
it.* The field's convergent answer (tree-sitter, LLVM front-ends, MLIR dialects, stack-graphs) is to
**stop unifying meaning**: unify *shape* (syntax) or *infrastructure* (tooling), keep the semantic
part per-language and declarative.

---

## 4. Why reprise is immune — sticking point by sticking point

The load-bearing fact is one design decision, from which most immunities descend:

> **Matching is same-language-only (spec §3).** A Python unit is never compared against a Rust one.
> The shared node-set therefore exists *only to make the algorithms language-agnostic* (one
> `lower_recursion` that operates on `Ir::Loop`, not on `"call_expression"` vs `"call"`), **NOT** to
> claim Rust's `Loop` and Python's `Loop` are the same thing. In the matching engine, they never
> meet (SIMILARITY-IR.md §12.1, principle 2).

This converts "universal IR" from a *semantic* claim (impossible, SP1–SP3) into an *engineering*
convenience (cheap). Every dead project needed cross-language node equivalence to be *true*; reprise
needs it to be *never asked*.

| Sticking point | Fatal to | reprise's neutralizer | Where it's decided |
|---|---|---|---|
| **SP1** high-vs-low pincer | UNCOL, LLVM-as-universal | Not on the axis: the IR is for **similarity, not execution/codegen**. It is *allowed* to be lossy toward source; the discriminator is *relocated* into a hash-excluded transform witness, not preserved in the matching form. | SIMILARITY-IR.md §2, §15 |
| **SP2** LCD vs union | Babelfish, `semantic` | Same-language-only removes the "must be cross-comparable" requirement → no LCD collapse. Variance is **pushed out** to a `Native{lang_kind}` escape hatch (matches same `lang_kind` only), not forced into the vocabulary. New canonical nodes require a **driving benchmark case** (D-IR-4 / D11/D25) → no drift-to-union. "Aim smaller than Semgrep, on purpose." | spec §3; principle 2; §12; D-IR-4 |
| **SP3** semantic gap | Babelfish (#364), `semantic` (zsuper) | Structurally impossible to conflate: Rust's `Loop` and Python's `Loop` **never meet**. The shared *name* buys algorithm reuse; it is never a cross-language identity claim. | §12.1; principle 2 |
| **SP4** assignment tax + grammar churn | `semantic` (decisive), Babelfish | **Partly** neutralized: lowering goes *directly* to canonical form, so canonicalization passes don't byte-match CST shapes — "removes an entire class of grammar-upgrade fragility" (principle 1). Intended full fix: **generate the CST→IR lift from the grammar**, version-pinned (§12.1). ✓ *See §5 — now **gated** (SP4 Inc 1-6): a rename is a loud test failure, not silent drift; the lift is still hand-written (full grammar-generation not yet built).* | principle 1; §6; §12.1 |
| **SP5** m×n treadmill / ownership | Babelfish, `semantic` | The shared skeleton kills **algorithm** duplication (the real m×n cost: one `lower_recursion`, not five). reprise is a **single static binary**, not a platform soliciting community language drivers — the "contributors must learn Haskell" ownership wall doesn't exist. | §6 |
| **SP6** shared-analysis never ships | `semantic` (its whole thesis) | reprise **never attempts** whole-program semantic analysis. It **stops at the tree** — no CFG/PDG/abstract-interpretation (§10). The "analysis" is structural hashing + anti-unification, already language-agnostic on `NormNode` (the back half, unchanged, §3). There is no m+n evaluator to fail to ship. | §10; §3; §12.1 |
| **SP7** ecosystem-coordination death | ANDF | N/A: the IR is **internal**, never an interchange format. Zero external adoption surface — nobody else has to ship a back-end. | §3 (scope) |
| **SP8** whole-program doesn't scale | `semantic` | The IR is a **pure function of `(source bytes, grammar version, IR-scheme version)`** with **zero cross-file state** feeding the hash (principle 5) — file-incremental determinism by construction, the exact property stack-graphs won on. Already realized: the D19 per-file content-keyed cache. | principle 5; §12.1; D19 |

The pattern: reprise doesn't *out-engineer* the universal-IR trap, it **refuses the bet that defines
it**. Same-language-only means the vocabulary is never a semantic contract (kills SP1–SP3);
stop-at-the-tree means there's no shared semantic engine to collapse (SP6); an internal, per-file,
deterministic IR means no interchange coordination and no whole-program blowup (SP7–SP8). What's left
— the algorithm-dedup win — is genuine but modest, and honestly the *only* thing the shared skeleton
is for.

---

## 5. Where reprise is **not** immune (the residual exposure)

The immunity is real but not total. Four honest caveats, most important first.

**(1) SP4 is now gated, not eliminated — the frontend lift is still hand-written, but drift can no longer be silent.**
Principle 1 moves fragility *off the canonicalization passes* (they no longer byte-match CST shapes),
which is a real class of grammar-upgrade breakage removed. But the CST→IR **mapping itself** still
reads tree-sitter node kinds by hand (`src/frontend/*.rs`; §6 lists it as the irreducible per-language
part), and DECISIONS.md **D9** says it plainly: "Any grammar upgrade can shift these shapes." The
declarative, grammar-*generated* lift that would fully *eliminate* SP4 — the exact fix `semantic`'s
CodeGen and GitHub's stack-graphs both landed — is stated as a **structural imperative** in §12.1 and
remains **not yet built**. But the *silent*-drift teeth are now **gated** (SP4 Inc 1-6, `similarity-ir`):
the hand-written lift is a data-driven dispatch table whose every referenced (kind, field) a conformance
gate asserts still exists in the linked grammar — so a renamed node-kind becomes a loud, localized test
failure naming the dead construct, not a silently-different hash. reprise still carries a *smaller*
version of the brittleness (the mapping is hand-authored, living at the frontend seam), but a grammar
bump can no longer move a hash without tripping a named assertion. **Full grammar-generation remains the
eventual elimination; the gate is the realistic closure, now landed.**

**(2) Same-language-only shields cross-language spurious matches, not intra-language over-convergence.**
reprise's whole strategy is to normalize *past* where Semgrep stops (the load-bearing fork, §12) to
get a hashable canonical form. Aggressive convergence raises recall but pressures **precision** — and
precision is reprise's softer number (spec §7.2). SP3's "identical syntax, divergent meaning" can't
strike *across* languages here, but an over-eager canonical node can still merge genuinely-different
logic *within* one language. The mitigation is empirical, not structural: the ⚠ nodes are decided by
benchmark, every new node needs a driving case (D-IR-4), and every convergence rung validates against
the §7.2 precision sample. That's discipline, not a guarantee.

**(3) The `Native` hatch must stay inert.** Its whole value is being an opaque, resolution-free node
that matches same `lang_kind` only (§12.1). The moment it grows toward *enrichment* — a resolver call,
a cross-language mapping — it reintroduces SP2/SP4 (the schema-leak and the maintenance treadmill).
This is a standing rule the code must not violate, not a property the type system enforces today.

**(4) The node-set discipline is policy, not structure.** D-IR-4's "a new canonical node requires a
driving benchmark case" is the dam against SP2's drift-to-union. Nothing *structurally* prevents the
vocabulary from bloating toward a union-of-all-features if the discipline slips — it would just be a
slower treadmill in the same direction. And per §6's honest accounting, **per-language code does not
vanish**; it shrinks to "mapping + quirks." reprise's own self-scan still reports the five parallel
`src/lang/*.rs` profiles as its single largest duplication finding — accepted, deliberate, gated — but
a standing reminder that reprise escapes per-language *algorithm* work, not per-language work.

---

## 6. The takeaway

Every project in §2 died because it needed a shared representation to **mean the same thing in two
languages**, and that is false in general (SP1–SP3), expensive to fake (SP4–SP5), and doesn't pay off
when faked (SP6–SP8). reprise's escape is not cleverness — it's declining the premise: **same-language-only
(spec §3) means the shared node-set is never a cross-language semantic claim, only a way to write each
algorithm once; stop-at-the-tree (§10) means there is no universal semantic engine to collapse; and a
per-file, deterministic, internal IR (principle 5) means no interchange coordination (SP7) and no
whole-program blowup (SP8).**

The one place the graveyard could still reach us is SP4 — the hand-written frontend lift — and its
*silent*-drift teeth are now **gated** (a conformance gate turns a grammar-kind rename into a loud test
failure, SP4 Inc 1-6); grammar-*generated*, version-pinned lowering remains named but unbuilt as the
eventual full elimination. Everything else is structurally out of range **because reprise never tried
to be universal in the first place.**

---

## Sources

**GitHub `semantic`** (primary, high confidence — maintainer-authored):
- ICFP 2022 experience report, Thomson/Rix/Wu/Schrijvers — arXiv:2206.09206 (§4.4, §6.3, §6.4, §7).
- CodeGen post, Ayman Nadeem, 2020 — github.blog/engineering/architecture-optimization/codegen-semantics-improved-language-support-system/.
- `why-tree-sitter.md` — github.com/github/semantic/blob/main/docs/why-tree-sitter.md.
- Successor: "Introducing stack graphs," Creager, 2021 — github.blog/open-source/introducing-stack-graphs/; paper arXiv:2211.01224.
- Repo status: github.com/github/semantic (archived; last activity ~2025-04).

**Babelfish / bblfsh** (primary, high confidence — maintainer-authored issues, re-verified this session):
- bblfsh/sdk **#364** "Semantic function structure is inconsistent across languages" — "there is no common way to get the function names."
- bblfsh/sdk **#361** native-props-exceed-schema — "Very cumbersome to write and maintain, ugly, fails the DRY principle."
- UAST v1 spec (statement-vs-expression `if`; "not the responsibility … to provide a language independent tree structure") and `adding-uast-annotations.md` (role gatekeeping); `languages.md` (9 beta / 1 alpha / 0 stable) — github.com/bblfsh/documentation.

**Classic compiler-IL lineage** (primary/authoritative):
- UNCOL: Steel, *A First Version of UNCOL*, WJCC 1961; Conway, *Proposal for an UNCOL*, CACM 1958; Wikipedia (UNCOL).
- Macrakis, *From UNCOL to ANDF* (OSF, 1993) — tendra.org/Macrakis93-uncol.pdf (under/over-specification; "premature implementation decisions"). ⚠ *OSF advocacy paper: trust its problem analysis, not its (falsified) prediction that ANDF would succeed.*
- LLVM: Lattner PhD thesis 2005, §2.2–2.3 — llvm.org/pubs/2005-05-04-LattnerPHDThesis.pdf.
- MLIR: Shpeisman/Lattner keynote 2019 — llvm.org/devmtg/2019-04/slides/Keynote-ShpeismanLattner-MLIR.pdf.
- GCC GENERIC/GIMPLE language-dependent-trees hook — gcc.gnu.org/onlinedocs/gccint/.
- ANDF non-adoption — Wikipedia (Architecture Neutral Distribution Format), citing InfoWorld.

**Folklore / weak — flagged, used only as articulation, not authority:**
- "LLVM IR is not portable / not stable / not well-specified" — HN item 20026179 (community consensus, not a Lattner statement; it *is* consistent with the thesis position above).
- No sourced Steve Yegge / Steve Johnson UNCOL quote could be verified — **excluded** (the real "Johnson" thread is S.C. Johnson's Portable C Compiler, the "use C as the IL" pragmatic alternative, not a skeptic quote).
- A blog "The Semantic Impedance Mismatch" with a tidy "200+ types → 47 universal categories" figure surfaced in search, does **not** resolve to a real source, and was a hallucinated citation — **excluded, do not cite.**
