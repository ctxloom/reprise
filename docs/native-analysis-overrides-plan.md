---
title: Per-language native overrides for the back-half analysis — a driving-case survey
status: survey + seam sketch (2026-07-05). Grounds the "analysis systems may need per-lang
  overrides for native things" plumbing in concrete cases across all existing + planned
  languages, before any seam is built (D-IR-4 driving-case discipline). Decisions open.
sessions:
  - fatal-main-storm
related:
  - docs/SIMILARITY-IR.md §5 (node-set + Native hatch), §13 (decomposition ladder / graduation),
    §14 (Branch unification), principle 2 ("neutral" = passes don't branch on language)
  - docs/universal-ir-trap.md §4 (SP2/SP3 immunity), §5.3 (the escape hatch must stay inert)
  - DECISIONS.md D14 (zero-cost machinery holes), D30 (dispatch fold-but-don't-report),
    D25 (Kotlin `when` left Native), D-IR-4 (node/override needs a driving case)
  - src/au.rs (the canonical-vs-Native fall-through seam), src/fold.rs (FoldRules), src/lang/mod.rs
  - tasks: elder-wow (IDF substantiality — Go-idiom FP class), fond-mute (substantiality metric)
---

# Per-language native overrides — a driving-case survey

> **Why this exists.** The back half (fingerprint / seq / tree / AU / fold / group / report) is
> meant to be **generic on the canonical `NormNode`** and we intend to keep it that way "fairly
> hard." But some constructs stay `Native{lang_kind}` (they haven't earned a canonical node), and a
> few per-language *idioms* are ubiquitous boilerplate — and for those the generic analysis is
> silently wrong. This surveys **all** existing + planned languages to find those cases *first*, so
> the override plumbing is built to real driving cases, not speculation. **Headline (substantiated
> model, 2026-07-05): promotion into the canonical vocabulary is *desirable and a lot of constructs
> earn it* — because a promotion here is a within-language convergence claim (same-language-only, trap
> §4), never the cross-language semantic-identity claim that killed Babelfish/`semantic` (SP3).
> Convergence *is* recall, so we promote aggressively wherever real spelling-variance exists (the loop
> / conditional / decomposition / order / error-propagation families — spanning every language). What
> must NOT promote is a construct with *no equivalent alternate spelling* (JSX, aggregate literals,
> preprocessor): promoting it grows the vocabulary (SP2 cost) for zero convergence — those need their
> *shape* classified (an override), not a node. The decision is need-driven (§4.0): three independent
> needs → four seams. Two earlier drafts over-swung (promote-everything, then promote-nothing); §4.0 is
> the substantiated middle.**

## 1. The mechanism — where a native thing gets mishandled

The back-half analyses answer three structural predicates on *canonical* kinds and **fall through to
the per-grammar profile for everything else** (`src/au.rs:82-116`, `src/fold.rs:18-28`). For a
`Native*` node that fall-through currently yields the generic default, which is often wrong:

| Analysis system | Predicate it needs | Generic default on a `Native` node | Failure when the native thing is… |
|---|---|---|---|
| **AU near-miss** (au.rs) | `is_list_kind` → Smith-Waterman vs **positional** align | positional | …a variable-length **list** (an insert/delete mis-holes the whole tail) |
| **Fold / re-roll** (fold.rs) | `is_list_kind` → which siblings re-roll | never folds | …a **repeated** list (misses the re-roll canonicalization) |
| **Fold / report** (fold.rs) | `is_dispatch_arm` → D30 fold-but-don't-report | reports | …a **dispatch** table (idiomatic → false positive) |
| **Substantiality** (group/api/report) | idiom discount (cf. D30, D14) | counts toward substance | …ubiquitous **boilerplate** (length-only FP; elder-wow) |
| **Order canon / AU binop** | `commutative_ops` (exists) | fixed op set | …an **overloaded** operator (comm-sort unsound in a new way) |

Three exits for each case, and telling them apart is the survey's real job:
- **GRADUATE** — add a canonical node/shape via a driving case (§13/§14, D-IR-4). Best when the
  construct is **cross-language common** — the shared algorithm then converges it for free.
- **OVERRIDE** — keep it `Native`, hand the back half a per-language **classification tag**
  (list? sortable-pair? dispatch? boilerplate?). Best when the construct is **language-idiosyncratic**.
- **ACCEPT** — the generic default is fine (a wrapper whose children are already lowered; a type node
  we ignore by non-goal).

**The immunity guardrail (non-negotiable, trap §5.3 / §4).** An override is **DATA** (a declarative
classification consumed by one generic algorithm), **never** a per-language algorithm, **never** a
resolver call or cross-language mapping. Same-language-only already means two languages' natives never
meet; the tag only tells the *one* generic algorithm the *shape* of a native node. Every entry is
gated by a driving benchmark case (D-IR-4). This is why the override plumbing does **not** reopen
SP2/SP4.

## 2. The recurring categories (derived from §3)

1. **List-shaped native** — children are a variable-length sequence. (→ AU Smith-Waterman + fold)
2. **Sortable-pair native** — children are order-insensitive key/value pairs. (→ order canon + AU)
3. **Dispatch-shaped native** — ordered first-match arms. (→ D30 suppress + arm alignment)
4. **Boilerplate idiom** — ubiquitous non-signal *shape* (often **canonical**, not Native). (→ discount)
5. **Unsound-canon guard** — a generic canonicalization is unsafe for this language's operator set.

## 3. Per-language survey

Existing frontends (Rust/Python/Go) cite what falls to `native()` today; planned ones (TS/Kotlin,
then Java/C#/C/C++) reason from the steady-state node-set (§5) + idioms. `G/O/A` = graduate / override / accept.

### 3.1 Rust (frontend built)
| Construct | Stays | Category | G/O/A | Note |
|---|---|---|---|---|
| `macro_invocation` (`vec![]`, `println!`, `assert_eq!`) | Native | 1 (args a list) | **O** list | commonest Rust native; args mis-align positionally |
| `array_expression` / `tuple_expression` (literal) | Native | 1 | **O** list | |
| `struct_expression` `Foo{a:1,b:2}` | Native | 2 | **O** sortable-pair(key=`field`) | field inits order-insensitive |
| `try_expression` (`x?`) | Native | 4 | **G** (err-prop, §13 r3) | chain-of-`?`; canonicalize form, discount boilerplate |
| `await`/`async`/`unsafe` blocks | Native wrapper | — | **A** (unwrap) | children already lowered; wrapper is a machinery token |

**Rust verdict:** mostly **list** + **sortable-pair** overrides. Low idiom-boilerplate.

### 3.2 Python (frontend built)
| Construct | Stays | Category | G/O/A | Note |
|---|---|---|---|---|
| `list/set/dict/generator` **comprehension** | Native | 5 | **G** → Loop/Iter | *the* Python case; LLM swaps comprehension↔for-append constantly (§13 r4) |
| `dictionary`/`set`/`list`/`tuple` literal | Native | 1/2 | **O** list / sortable-pair(dict) | |
| `try_statement` (except/finally) | Native | 3/5 | **G** → Branch-family (D-IR-11) | except-clauses = ordered typed arms |
| `with_statement` | Native wrapper | 4 | **A**/strip | resource idiom; body lowered |
| `assert`/`raise`/`global`/`del`/`yield` | Native | 4 | **A** | `assert` mildly boilerplate |
| f-string (`string` w/ interpolation) | Lit(Str) | — | **watch** | embedded exprs collapse into one atom (§12.1 large-atom) |

**Python verdict:** **comprehension graduation** is the big win; dict/list overrides; try/except graduation.

### 3.3 TypeScript / TSX (no frontend yet — P2)
| Construct | Category | G/O/A | Note |
|---|---|---|---|
| **JSX** (`jsx_element`, children + `jsx_attribute`) | 1 + 2 | **O** list(children) + sortable-pair(attrs) | *the* TS case; components duplicated constantly |
| `object` literal `{a:1,b:2}` | 2 | **O** sortable-pair | |
| `array` literal | 1 | **O** list | |
| `switch` (fallthrough) | 3 | **O** dispatch (§14 fallthrough Native-first) | |
| type layer (`interface`/`type`/generics/`as`) | — | **A** (non-goal: types) | NativeType, ignored |
| `enum` | 3/2 | **O** dispatch/value-table | |
| decorators | — | strip | |

**TS verdict:** **JSX list+sortable** dominates; object-literal sortable; fallthrough dispatch.

### 3.4 Go (frontend built)
| Construct | Stays | Category | G/O/A | Note |
|---|---|---|---|---|
| **`if err != nil { return err }`** | *canonical* (Branch+Return) | 4 | **G/discount** (§13 r3) | ubiquitous boilerplate → length-only FP (elder-wow). **Keyed by canonical shape, not a native kind** |
| `composite_literal` `T{…}` / `[]T{…}` / `map[…]{…}` | Native | 1/2 | **O** list / sortable-pair | very common |
| `select_statement` | Native | 3 | **O** dispatch | channel-op arms |
| `type_switch_statement` | canonical (handled) | 3 | verify D30 suppression | |
| `defer`/`go` statement | Native wrapper | 4 | **A**/strip | idiom/machinery token |
| `goto`+labels, `iota` const block | Native | 3 | **O** dispatch(value-table) | rare |

**Go verdict:** **err-check boilerplate discount** (shape-keyed) + **composite-literal list/sortable**;
select dispatch. The err-check case proves the boilerplate slot must key on **shape**, not only native kind.

### 3.5 Kotlin (no frontend yet — P2; D25 left `when` Native)
| Construct | Category | G/O/A | Note |
|---|---|---|---|
| **`when`** (subject + subjectless) | 3/5 | **G** → Branch (§14) | subsumes if-chain within-lang; the clean graduation |
| Elvis `a ?: b`, safe-call `?.`, `!!` | 5 | **G** → Branch/guarded-Call | Elvis = 2-arm branch |
| `try`/catch/finally | 3/5 | **G** → Branch-family (D-IR-11) | |
| trailing-lambda `xs.map { it*2 }` (`it`) | 5 | **G** (combinator↔loop) / **A** | `it` = synthetic param (frontend quirk, not a back-half override) |
| coroutines (`suspend`/`launch{}`), data/sealed class, `object` | — | **A** | wrappers/decls |
| string template | Lit | — | watch (large-atom) |

**Kotlin verdict:** **`when` graduation** is the headline; Elvis/try-catch graduate; combinator frontier.

### 3.6 Java (planned — gaunt-think)
| Construct | Category | G/O/A | Note |
|---|---|---|---|
| `try`/catch/finally | 3/5 | **G** → Branch-family (D-IR-11) | ordered typed catch arms |
| checked-exception try/catch wrapping | 4 | **discount** | log-and-rethrow boilerplate |
| Streams `xs.stream().filter().map().collect()` | 5 | **G** (combinator↔loop, §13 r4) / **A** (method-chain) | precision-risky graduation |
| `switch` (classic fallthrough + arrow/pattern) | 3 | **O** dispatch / **G** Branch | two shapes |
| anonymous class / lambda | — | Lambda + native | |
| annotations, generics/wildcards | — | strip / **A** | |

### 3.7 C# (planned)
| Construct | Category | G/O/A | Note |
|---|---|---|---|
| **LINQ query** `from x in xs where p select f` | 5 | **G** → Loop/Iter | comprehension-in-disguise, *the* C# case (mirrors Python) |
| fluent LINQ `.Where().Select()` | 5 | **G** (combinator↔loop) / **A** | |
| properties `{get;set;}` / auto-props | 4 | **discount** | getter/setter boilerplate |
| `switch` expr + patterns / classic switch | 3 | **G** Branch / **O** dispatch | |
| `yield return`, `async`/`await` | 5 | **G** (generator↔loop) / **A** | |
| object/collection initializers `new T{A=1}` / `{1,2}` | 2/1 | **O** sortable-pair / list | |
| `using` stmt, attributes, `?.`/`??` | — | **A** / strip | |

### 3.8 C (planned)
| Construct | Category | G/O/A | Note |
|---|---|---|---|
| **preprocessor** (`#define` fn-macros, `#include`, `#ifdef`) | 1 | **O** list(macro args) / **A** | native layer; conditional-compile is structural noise — **watch** |
| **`goto cleanup;` idiom** (`if(err) goto cleanup;` runs) | 4 | **discount** (shape-keyed, like Go err-check) | C's error idiom |
| `switch` fallthrough | 3 | **O** dispatch | |
| designated init `{.a=1}` / array init `{1,2,3}` | 2 / 1 | **O** sortable-pair / list | |
| struct/union/enum, fn pointers, varargs, bitfields | — | **A** / native | |

### 3.9 C++ (planned — the hard one)
| Construct | Category | G/O/A | Note |
|---|---|---|---|
| **operator overloading** (`+` may be non-commutative: matrix, string) | 5 | **O** — **narrow `commutative_ops`** | *the* distinctive C++ case; widens §5.2.6 unsoundness. Uses the **existing** `commutative_ops()` seam |
| templates / SFINAE / `constexpr` metaprogramming | — | native / **A** | structurally wild; keep opaque |
| try/catch, `std::ranges`/`std::transform` | 5 | **G** Branch-family / combinator↔loop | as Java/C# |
| initializer lists `{1,2,3}`, uniform init | 1/2 | **O** list / sortable-pair | |
| references/`auto`/`move`/lambdas/namespaces | — | **A** | (+ all of C §3.8) |

## 4. Synthesis

### 4.0 The promotion gate — cross-language recurrence × exact-equality (the substantiated rule)
Promote **as much as is feasible** — but "feasible" has a precise test, and it is the exact check
Babelfish/`semantic` never made. A construct earns a place in the shared canonical vocabulary by **how
many languages it recurs in, gated by exact structural-semantic equality**:

| Recurrence | Rule | Why |
|---|---|---|
| **1 language** | **Stays in that language** — Native + an override for its shape. No shared node. | Earns *no cross-frontend algorithm reuse* from a shared node; promoting it is pure SP2 vocabulary-growth cost. |
| **2 languages** | **Probable promotion — but only if the two are *exactly* equal.** Else stay Native. | Two earns reuse, but has no slack to absorb a semantic mismatch — where SP3 (`super()`, stmt-vs-expr `if`) bites. |
| **3+ languages** | **Indicative** (confidence rises with count) — still gated on exact-equality. | Broad recurrence is strong evidence of a genuine universal shape; the equality gate stays mandatory. |

**Exact-equality is the SP3 defense, made operational:** the canonical node's shape carries the *same*
meaning in every source language, so lowering to it converges genuinely-equivalent code and merges
nothing different. `if` is exactly equal across 9; **resource cleanup (`with`/`using`/`defer`/RAII) is
NOT** (block-end vs function-end vs scope-end timing) — so despite recurring in ~5 languages it does
**not** promote. That refusal *is* the immunity working: recurrence alone is the trap, recurrence
**× exact-equality** is the escape.

**Orthogonal, always-on: lower into an *existing* node.** A construct that reduces to a node already in
the vocabulary (Python comprehension → the 9-language `Loop`; Elvis → `Branch`) lowers into it with **no
new kind** — how within-language spelling-variants converge, always desirable regardless of the
construct's own recurrence. New kinds are rationed by the table; *using* the kinds we have is not.

Underneath the gate sit three **needs** (they pick *which seam*, once the gate decides *whether a node*):
**N1 convergence** (recall — the reason to promote or lower-into), **N2 shape-correct analysis** (a
Native construct's shape is mishandled but it has no variant to converge with → override tag), **N3
boilerplate discount** (ubiquitous low-signal shape → substantiality).

### 4.1 The construct recurrence audit (the substantiation)
Across {Rust, Python, TS/TSX, Go, Kotlin, Java, C#, C, C++}. "≈" = exactly-equal enough to promote; "≠"
= recurs but fails the equality gate (SP3 guard fires). **Every `≈` here is a *hypothesis to be proven
by a real-world test corpus (TDD), not a decision* (CLAUDE.md "Promoting a construct…"): capture
examples → write invariant tests red-first → promote only if they hold; unproven defaults to
Native/per-language.** Every promotion is also D-IR-4 + §7.2 gated.

| Construct | Recurs | Equal? | Decision |
|---|---|---|---|
| loop (while/for/foreach/recursion) | 9 | ≈ | **Loop** (done) — comprehension/generator *lower into* it |
| conditional / switch / match / `when` / ternary (break-terminated) | 9 | ≈ | **Branch** (done, §14); true **fallthrough** ≠ → stays Native |
| call / binop / unop / assign / return / field / index / lambda | 7–9 | ≈ | done (core vocabulary) |
| **aggregate literal — sequence** (`[..]`, `vec!`, composite, init-list) | 9 | ≈ | **PROMOTE → `Seq` node** (user-confirmed; corpus/TDD-gated; retires override slot 1) |
| **aggregate literal — keyed** (struct/object/dict/map/designated-init) | 9 | ≈ | **PROMOTE → `Record` node**, key-sorted (user-confirmed; corpus/TDD-gated; retires slot 2) |
| try / catch / finally | 6 | **?≈** | **BY-LANGUAGE until a corpus proves it** (user, 2026-07-05): catch-binding semantics differ (Py `except E as e` / Java multi-catch / C++ by-value-or-ref) → Native until TDD confirms; D-IR-11 stays open |
| **null-safety Elvis `?:`/`??`**, safe-call `?.` | 3 | ≈ | **lower → Branch** (2-arm) / guarded Call |
| error-propagation *form* (`?` / err-check / goto-cleanup) | 3 | ≠ (surfaces differ) | **cautious form-canon** (§13 r3, gated) **+ N3 discount** |
| resource cleanup (`with`/`using`/`defer`/RAII) | ~5 | **≠** | **do NOT promote** — SP3 guard; Native wrapper / strip |
| method-chain combinators (streams/LINQ-fluent/iterators/ranges) | 6 | ≈ *as Call chains* | already **Call** (generic); combinator↔loop deferred (§13 r4) |
| **JSX** | 1 (TS) | — | Native — or lower → Call+Record+Seq (its own compile target) |
| channels / `select` / goroutines | 1 (Go) | — | **stays Go** — Native + dispatch override |
| preprocessor / macros (C `#define`, C++, Rust `macro_rules!`/proc-macro) | C/C++ ~2; Rust **≠** | ?≈ / ≠ | **Native, unexpanded, per-language** (user, 2026-07-05). Do **not** execute the toolchain (D44/buildless; CLAUDE.md); Rust hygienic-AST macros ≠ C textual `#define` (SP3 lure); even C≈C++ is corpus/TDD-gated. Expansion-aware analysis → opt-in build tier (§11) only |
| operator overloading (`+` maybe non-commutative) | 1 (C++) | — | **stays C++** — narrows the existing `commutative_ops` (guard) |
| decorators / annotations | 5 | — | stripped (non-signal), not promoted |

**What the audit changes vs the earlier drafts:** *more* promotes than the skeptical pass (aggregates,
try/catch, Elvis clear the gate), **and** the override residue **shrinks** — promoting sequence/keyed
aggregates to `Seq`/`Record` retires override slots 1–2, leaving a small genuine Native residue:
single-language dispatch (Go `select`), the C++ commutativity guard, boilerplate discount, and JSX
(Native-or-lowered). And the equality gate *stops* the seductive over-promotions (resource cleanup,
error-prop surface) that recurrence alone would have waved through. That is "promote as much as feasible,
and no more."

### 4.2 The override vocabulary — the shrunken residue
After §4.1 promotes sequence/keyed aggregates to `Seq`/`Record` (retiring slots 1–2), the live override
residue is small. The full classification vocabulary, for reference (slots 1–2 apply only to aggregates
*kept* Native by choice, e.g. JSX) — **five per-language classifications**, four keyed by native
`lang_kind` and one by canonical *shape*:

1. `native_list_kinds(lang) : Set<kind>` — Smith-Waterman + fold. *(slot exists as canonical
   `is_list_kind`; extend to native kinds)*
2. `native_sortable_pairs(lang) : Map<kind, key_field>` — order canon + AU. *(extends
   `sortable_pair_kind`)*
3. `native_dispatch_kinds(lang) : Set<kind>` — D30 suppress + first-match arms. *(the language-
   agnostic replacement for the retired `is_dispatch_arm`, now Native-scoped)*
4. `idiom_boilerplate(lang) : Set<Shape>` — substantiality discount; **shape-keyed** (Go err-check,
   C goto-cleanup, getters), so it also matches *canonical* subtrees, not only Native kinds. Ties
   into elder-wow / fond-mute.
5. `commutative_ops(lang)` — **already exists**; C++ narrows it. No new machinery, just an entry.

Slots 1–3 are pure "what shape is this opaque native node"; slot 4 is the only one that inspects
canonical structure; slot 5 is an existing seam. Nothing here resolves, enriches, or crosses
languages — it is classification data feeding the *same* generic algorithm (trap §5.3 satisfied).

## 5. The four seams (concrete)

Running the promotion gate reshaped the plumbing: it is **not** one "NativePolicy with five slots" —
it is four distinct seams, because slots 1–2 became a *node-set* promotion (Seam A), leaving a small
native-tag seam (B), a substantiality seam (C), and one language-parameterized pass (D). Each below has
its signature, its threading points in the real code, and its immunity note.

### Seam A — node-set: `Seq` + `Record` (N1, user-confirmed; retires override slots 1–2)
Two new canonical kinds: `Seq` (order-significant element list — `[..]`, `vec!`, positional
composite/init-list) and `Record` (key/value entries, **key-sorted** — struct/object/dict/map/designated-init).
- *`src/ir/kind.rs`*: add `SEQ`/`RECORD` to the vocabulary + `ALL`; **bump `SCHEME_VERSION` 3→4** and
  `EXTRACTION_VERSION` (the canonical form moves — D19/D30 cache-key discipline).
- *Entry shape (sub-decision):* a `Record` entry is `[@key, @value]`; reuse `Assign` or a minimal `Pair`
  — decide at the first frontend (*lean:* a dedicated lightweight entry — a record field is neither a
  binding nor a mutation, so overloading `Assign` would blur it).
- *`src/frontend/*`*: aggregates lower via shared `make_seq(elems)` / `make_record(entries)` (new
  helpers in `frontend/mod.rs`): Rust `array/tuple/vec! → Seq`, `struct_expression → Record`; Python
  `list/set/tuple → Seq`, `dictionary → Record`; Go `composite_literal → Seq|Record`. (TS/Kotlin/… as built.)
- *`src/ir/pass.rs`*: new `detect_record_sort` (mirrors `detect_comm_sort`) emits `RecordSort{ locus,
  order }` — **reversible**, original order is the witness (D-IR-9); threaded into `run_passes` beside
  `detect_comm_sort`. `Seq` is **not** sorted.
- *Back half — the retirement:* add `SEQ`/`RECORD` as list-kinds in `au.rs::Ctx::is_list_kind` (IR
  branch, `src/au.rs:92`) and the `fold.rs` `FoldRules` IR impl. Aggregates now Smith-Waterman-align and
  fold **generically — no per-language list/sortable tag** (slots 1–2 gone).
- *Immunity:* +2 kinds, both at recurrence 9 × exact-equal — rationed by the gate, not drift.

### Seam B — native shape tag (N2; the shrunken residue)
After A, the residue is chiefly **dispatch-shape** (Go `select`, a fallthrough switch kept Native, JSX
if opaque). D-NAT-1 recommendation: **carry-as-node-tag** (keeps the back half language-blind).
- `NormNode` gains a **hash-excluded** `native_shape: NativeShape` (`Opaque | List | Dispatch`), set
  only on `Native*` nodes (like provenance — hash-excluded, so it cannot move a fingerprint).
- Set at the `native()` call site (`frontend/mod.rs:369`) from a tiny per-frontend `lang_kind → shape`
  classifier — local to the frontend that already holds `lang_kind`; **no cross-language map**.
- Read in the back-half native fall-through: `au.rs::is_list_kind` → `shape == List`; `fold.rs`
  D30-suppression → `shape == Dispatch`. The algorithm reads a tag; it never calls a per-lang function.
- *Immunity:* a syntactic, same-language, hash-excluded *classification* — never a resolver or
  cross-language identity. The hatch stays inert in the sense that matters (§5.3): it resolves nothing.

### Seam C — boilerplate discount (N3; shape-keyed substantiality)
Go `err != nil`, C `goto cleanup`, getters — a *canonical shape* that is ubiquitous non-signal. Not a
node, not a native tag: a substantiality **discount**, owned by the `fond-mute` metric.
- A per-language set of **canonical-shape recognizers** (e.g. "a `Branch` with one arm `guard = x !=
  <nullish>` / body = `Return x`") marks a matched subtree **zero-substance** — as D30 dispatch and D14
  machinery already contribute zero.
- Threads where substantiality is computed: today `group.rs::consolidation_value` (`src/group.rs:145`)
  + the token-floor gates; under `fond-mute`, a term in `f(distinct_landmark_count, idf_content)`.
  Subsumes `elder-wow` (the IDF idiom-FP class).
- *Immunity:* a small per-lang shape-predicate that changes *ranking*, not the canonical form — no
  vocabulary growth, no fingerprint move.

### Seam D — per-language commutative set (N1 precision guard; the C++ case)
Comm-sort commutativity is a **global const** today (`COMMUTATIVE`, `src/ir/pass.rs:103`) — fine for
Rust/Python/Go. C++ operator overloading makes `+`/`*` possibly non-commutative, so the set becomes a
**per-language input**:
- `detect_comm_sort(node, commutative: &[&str])` (+ `is_commutative_chain`); `run_passes` threads the
  per-lang set. Default = today's const; **C++ narrows it** (drop `+`/`*`, keep always-commutative
  logical/bitwise).
- *Immunity:* per-lang **data** fed to the *same* algorithm — the one place a pass becomes
  language-parameterized, still data-not-branch. Gate on a C++ driving case; canonical tree moves for C++ only.

### Landing order & cache
1. **Seam A** — biggest payoff (the confirmed promotion + retires the largest override need). Moves the
   canonical form → `SCHEME_VERSION`/`EXTRACTION_VERSION` bump; corpus/TDD-gate the convergence (§4.1) first.
2. **Seam D** — small, isolated; lands with the C++ frontend (`gaunt-think`).
3. **Seam B** — driven by the first Native dispatch case that needs it (Go `select` / TS switch).
4. **Seam C** — rides `fond-mute`; substantiality, post-flip.

Seams **B and C are hash-neutral** (B a hash-excluded tag; C match-time ranking) → no version bump. **A**
is the only canonical-form move; **D** moves it for C++ only.

## 6. Decisions for sign-off

- **D-NAT-1 · Seam shape.** (a) node-tags / (b) policy-lookup / (c) hybrid (§5). *Lean:* (c) —
  node-tags for slots 1–3 (keeps the hot path language-blind, your stated goal), a small policy object
  for slot 4 + slot 5. Confirm the inertness trade you prefer.
- **D-NAT-2 · Promotion gate (the substantiated rule, §4.0).** Promote by **cross-language recurrence
  × exact-equality**: 1 language → stays Native; 2 → probable, only if exactly equal; 3+ → indicative,
  still equality-gated. Prefer **lower-into-existing** over a new node; reserve new kinds for recurrence
  ≥2–3 + exact-equality (aggregates → `Seq`/`Record`, try/catch → `Try`). Override tags are for the
  low-recurrence Native residue only. *Lean:* adopt as stated — it is the operational SP3 defense.
- **D-NAT-3 · Timing.** Build the seam now (identity default, zero behavior change) and populate
  per-language entries **only** on a driving benchmark case (D-IR-4); or defer the seam until the first
  case forces it. *Lean:* build the seam now (it's the plumbing you asked for, and it unblocks TS/JSX +
  Kotlin `when` cleanly at P2), populate lazily.
- **D-NAT-4 · Relationship to SP4/parity.** Separate axis from the SP4 grammar-lift (jumpy-keep) and
  the default flip (boned-jam). *Lean:* separate; but the D-NAT-1(c) node-tags ride the same frontend
  seam SP4's table touches, so sequence them adjacent.

## 7. Immunity accounting (why this is safe)

Slots 1–5 are declarative classification **data**, consumed by unchanged generic algorithms; no
per-language code path, no resolution, no cross-language node identity (same-language-only stands).
The escape hatch stays opaque (a shape tag is not a resolver). Each entry needs a driving case
(D-IR-4). Net: the plumbing lives **inside** the universal-IR immunity (trap §4). The vocabulary stays honest
because new kinds are rationed by **recurrence × exact-equality** (§4.0) — the exact test the neighbours
skipped — while *within-language* variance converges by lowering into existing kinds, and the
low-recurrence residue stays Native behind declarative override *data*. Promotion is maximized *and*
disciplined; neither the vocabulary nor the algorithms branch on language.
