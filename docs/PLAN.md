---
sessions:
  - tidal-moral-chive
---

# Implementation Plan: Semantic-ish Duplicate Detection for LLM-Generated Code

**Status:** Plan for implementation. Not started.
**Audience:** An implementing agent (Claude Code or similar) with full autonomy over code-level decisions but not architecture-level ones. Deviations from this plan should be recorded in a `DECISIONS.md` with rationale.
**Prepared:** 2026-07-01, from a design conversation. Context and rationale are included inline so this document stands alone.
**Rev 2 (same date):** Cross-language ambitions removed entirely. Matching is same-language only; normalization targets per-language native node kinds instead of a shared canonical vocabulary. Affected: §3, §5.2.2, §5.5, §10, §12.
**Rev 3 (same date):** Near-miss detection reframed around **anti-unification**: fingerprinting gains subtree-hash bags and hole-context hashes (§5.5); verification produces a shared template + substitutions instead of a bare similarity score (§5.6); ranking uses divergence ratio and hole shape (§6). Finding type changes from "these are similar" to "these are instances of this template, differing in these holes." Affected: §1, §5.5, §5.6, §6, §7, §9, §12, §13.
**Rev 4 (same date):** Loop handling changed from per-form canonical loop kinds to **decomposition into a minimal loop core** (unconditional loop + conditional break + hoisted init/step) — one lowering rule per form replaces N×M convergence decisions, and divergences localize into AU holes. Unrolling is explicitly documented as absent by design (folding §5.3 covers the unrolled direction). §11's mission sharpened: the IR tier targets the *structural-drift* band (recursion↔iteration via tail-call elimination, accumulator variants), not algorithm drift proper; the decompilation step is dropped as unnecessary. Affected: §5.2, §5.3, §11, §12.
**Rev 5 (same date):** **Recursion is a fourth source form for loop decomposition.** Linear/tail recursion lowers to the loop core at the AST level (self-call in tail position → parameter reassignment + continue), with an optional accumulator rule for near-tail linear recursion and a loop-exit normalization companion rule. This moves the recursion↔iteration flagship case from the deferred IR tier into Phase 2; §11 is demoted to residue insurance. Non-linear (tree) recursion remains permanently out of scope. Affected: §5.2, §7, §8, §11, §12.
**Rev 6 (same date):** Two extensions. (a) **Mutual recursion**: SCCs of the syntactic call graph are detected (Tarjan over the §5.4 definition table) and converged via *SCC-scoped inlining* — one expansion round within an SCC turns mutual recursion into self-recursion, feeding the Rev 5 lowering. (b) **API-profile tier** (`api-profile`, suspicion-only): static-birthmark-style matching on IDF-weighted (callee, control-context) multisets, targeting reimplemented-task duplication that survives structural rewriting. Explicitly weaker than Type-4 detection: blind to pure-computation functions, unverifiable by anti-unification, never fails CI. Affected: §5.2, §5.4, new §5.7, §6, §7, §8, §9, §12, §13.
**Rev 7 (same date):** Output-format conformance to prior-art standards (new §6.1): SARIF 2.1.0 mapping with duplicate groups as multi-location results, **structural/template hash as `partialFingerprints`** (stable alerts across cosmetic drift — the key decision), CPD-XML and jscpd-JSON compatibility for pipeline drop-in and §7.3 baseline diffing, and standard clone-metric definitions for CALIBRATION.md comparability. Affected: §6 (new §6.1), §7, §13.
**Rev 8 (same date):** Tool named **reprise** (was working name `dupfold`). Checked free on crates.io and PyPI; only collision is an unrelated npm package in a different ecosystem. Affected: §2, §9.
**Rev 9 (same date):** Review revision + audio-fingerprinting transfers. (a) **Window enumeration deleted** — replaced by generalized-suffix-array maximal-repeat discovery over the normalized token stream (exact sequence tier, linear-ish, no quadratic window sets). (b) **Baseline/suppression** (`reprise baseline`, `reprise:ignore` pragma) keyed on the Rev 7 structural fingerprints — without this the tool fails CI forever on any legacy repo. (c) **Drift tracking**: new top tier `inconsistent-update` fires when a diff modifies some but not all members of a known group (Juergens ICSE'09: 52% of clones changed inconsistently; ~half of unintentional inconsistencies are faults). (d) **Test-code policy** (separate section by default). (e) **Shazam transfers**: landmark-pair combinatorial hashing (§5.5, rival to hole hashes — loser is dropped), offset-histogram diagonal verification + region localization pre-AU (§5.6), Smith-Waterman graded-cost alignment replacing binary LCS at list nodes, greedy-string-tiling noted as the reorder option. (f) Version-keyed index caches. Affected: §2, §4, §5.1, §5.5, §5.6, §6, §7, §8, §9, §12, §13.

---

## 1. Problem statement and context

LLM coding assistants systematically duplicate code within a codebase. Empirical grounding: GitClear's 2025 analysis of 211M changed lines (2020–2024) found an 8x increase in duplicated code blocks and copy/pasted lines exceeding moved (refactored) lines for the first time; their duplication metric counted **Type-1 (near-literal) clone blocks**. The harder, more damaging residue reported by practitioners is **divergent duplication**: the LLM reimplements an existing helper (instead of calling it) with different identifiers, loop forms, or minor structural changes, and the copies then drift as features are added to one but not the other.

The clone-type taxonomy used throughout this document:

- **Type-1:** identical modulo whitespace/comments.
- **Type-2:** identical modulo identifier names and literal values.
- **Type-3:** near-miss — statements added/removed/reordered, loop form changed (`while`↔`for`), small structural edits.
- **Type-4:** semantically equivalent, syntactically unrelated (recursion↔iteration, different algorithm).

**Target:** Types 1–3 plus the "reimplemented helper with similar structure" subclass of Type-4, within a **single codebase**, at **repo-scan** and **PR-check** granularity. Full Type-4 is explicitly out of scope for this tool (see §10, Non-goals); a deeper IR-based tier for compilable languages is a possible future phase, sketched in §11 but **not to be built now**.

**Prior art the design draws on** (do not re-derive; references in §13):

- NiCad: normalization (pretty-printing, identifier abstraction, flexible transformation rules) on parse trees + longest-common-subsequence matching detects Type-3 well. This tool is essentially "NiCad's idea, rebuilt on tree-sitter, plus two novel-ish moves."
- Deckard: characteristic vectors over AST subtrees + locality-sensitive hashing for scalable near-miss detection.
- Ragkhitwetsagul & Krinke (IWSC 2017): compiler round-tripping as normalization recovers clones that defeat source-level detectors — motivates the (deferred) IR tier.
- Jia et al. (TOSEM 2022): function inlining breaks 1-to-1 function matching; "inlining simulation" (deliberately inlining before matching) is a studied mitigation — motivates the best-effort inliner (§5.4).
- Bulychev & Minea (2008, CloneDigger) and Li & Thompson (Wrangler, Erlang): **anti-unification** — the least general generalization (anti-unifier) of two trees is their shared "skeleton" with placeholders for divergent subtrees; the substitutions *are* the divergences, factored out as complete subtrees. Evans, Fraser & Ma (2007) is the parallel "structural abstraction" line (clones with arbitrary subtree changes). This is the formal core of the near-miss tier (§5.6): findings are templates + holes, not similarity percentages.

**The two design insights that differentiate this from off-the-shelf CPD/jscpd:**

1. **Detection tolerates unsoundness.** The output is a report, not a program transformation. Therefore the inliner may resolve calls heuristically, ignore side effects, and substitute arguments textually. A wrong inline yields at worst a noisy candidate pair, never a wrong program. This unlocks cross-function duplication detection (LLM reimplemented an existing helper inline) without real semantic analysis.
2. **Unrolling inverts at the AST level.** Instead of unrolling loops to match flattened copies (the IR-level move), detect runs of near-identical sibling subtrees and fold them into a canonical repeated form. The duplication is structurally visible in the AST before any optimizer obscures it.

---

## 2. Deliverable

A CLI tool named **`reprise`**, written in **Rust**, using the `tree-sitter` crate and per-language grammar crates. (A *reprise* is a theme that returns in altered form — a near-duplicate. Name checked free on crates.io and PyPI as of 2026-07-01; the only registry collision is an unrelated Babel hot-reload package on npm, a different ecosystem. The tool is Rust-first but not Rust-exclusive in scope, hence the cross-registry check.)

Three modes:

- `reprise scan <path>` — full-repo scan, emits a ranked report of duplicate groups.
- `reprise check <path> --base <git-ref>` — PR mode: only functions added/modified since `<git-ref>` are queried against the index of the whole repo. Exit code nonzero above a configurable severity threshold, for CI. Also emits `inconsistent-update` findings (§6) when the diff touches some but not all members of a known duplicate group.
- `reprise baseline <path>` — writes all current findings, keyed by their stable structural/template fingerprints (§6.1), to a checked-in `reprise-baseline.json`. Subsequent `check` runs fail only on findings **not in the baseline or worsened since it** (divergence ratio increased, member added). This is the legacy-repo adoption path: without it, the first run on any existing codebase fails CI permanently. An inline `reprise:ignore` comment pragma (on the unit's first line) suppresses a specific unit from all tiers; suppression counts appear in scan stats so they can't silently accumulate.
- Output formats: human-readable terminal (default), JSON (`--format json`), SARIF (`--format sarif`), CPD-XML (`--format cpd`), jscpd-JSON (`--format jscpd`) — see §6.1. Every finding must carry **original source coordinates** (file, line span) for all members of a duplicate group, the tier that produced it (§5), and a similarity score.

Configuration: a `reprise.toml` at repo root (all values have defaults; tool must run with zero config). Keys specified throughout this document are collected in §9.

---

## 3. Language support and priority order

Implement language support behind a trait (`LanguageProfile`) so languages are added by implementing one trait + a set of tree-sitter queries. Priority order:

1. **Rust** (dogfooding; the tool scans itself)
2. **Python**
3. **TypeScript** (including TSX)
4. **Go**
5. **Kotlin**

Phase gates (§8) require only Rust + Python for the first evaluation. **Matching is same-language only:** the pipeline runs per language partition end-to-end (multi-language repos are supported, but a Python unit is never compared against a Rust unit). This is a deliberate simplification — each `LanguageProfile` defines its own canonical forms and node-kind space with no obligation to align with any other language's. The `LanguageProfile` trait must provide:

- grammar handle (tree-sitter language)
- node-kind classification: which node kinds are *function-like units* (fn items, methods, closures above a size floor), *statement-like*, *identifier*, *literal*, *comment*, *import/use*
- desugaring rewrite set (§5.2, per-language)
- call-expression and function-definition queries for the inliner (§5.4)
- commutative-operator list (used syntactically; unsoundness accepted, see §5.2)

---

## 4. Architecture overview

Pipeline stages, each a pure function of the previous stage's output where feasible:

```
source files
  → [P1] parse (tree-sitter, error-tolerant)
  → [P2] unit extraction (function-like units + normalized token stream)
  → [P3] normalization (per-unit canonical tree)      §5.2
  → [P4] sibling-run folding (re-roll)                §5.3
  → [P5] best-effort inlining (expanded variants)     §5.4
  → [P6] fingerprinting (structural hashes, bags, landmark pairs, vectors)  §5.5
  → [P7] matching: sequence tier (suffix array) ∥ tree tier (hash buckets → histogram-verified candidates → AU)  §5.6
  → [P8] grouping, ranking, baseline/drift comparison, source-mapping, report  §6
```

Key data structure: every normalized node retains a back-pointer (byte span) into original source. Normalization must never lose source mapping — this is a hard requirement; findings that can't be mapped to source lines are useless.

Concurrency: per-file parse/normalize/fingerprint is embarrassingly parallel (rayon). Matching is a global phase. Target: full scan of a 500k-LOC repo in under 60 seconds on 8 cores, PR check under 5 seconds warm. These are targets, not hard gates; measure and record.

Persistence: an on-disk index (sled, or flat rkyv/bincode files keyed by content hash of each source file) so PR mode doesn't re-fingerprint unchanged files. Invalidate per-file by content hash. **Cache keys must additionally include the normalizer version and each grammar crate version** — a tool or grammar upgrade must invalidate the whole index, or stale fingerprints silently corrupt baseline and drift comparisons. Keep this simple; do not build a daemon. The baseline file (`reprise-baseline.json`, §2) and its per-group divergence snapshots are the drift-tracking state; they live in the repo, not the cache.

---

## 5. Stage specifications

### 5.1 Parsing and unit extraction (P1–P2)

- Parse every file matching the language profiles; respect `.gitignore` plus config `exclude` globs. Skip generated code by default via configurable path patterns (`**/generated/**`, `*.pb.go`, `*_pb2.py`, etc.) and a marker-comment check (`@generated`, `DO NOT EDIT`) in the first 5 lines.
- Tree-sitter is error-tolerant: files with ERROR nodes are still processed; units containing ERROR nodes are fingerprinted but flagged `parse_degraded: true` in findings. Do not silently drop them — LLM output under review is frequently mid-broken, and that's a primary use case.
- Extract as comparison units: (a) every function-like node; (b) the **normalized token stream** of each file, concatenated corpus-wide with unit separators, feeding the sequence tier (§5.6). **There is no window enumeration** — Rev 9 deleted it. Sub-function duplication is found two ways instead: exact contiguous runs by suffix-array maximal-repeat discovery over the token stream (complete and non-quadratic), and gapped/noisy regions by offset-histogram localization (§5.6). Inliner-produced region duplication (§5.4) is covered by the same two mechanisms applied to inlined variants' token streams.
- **Test-code policy:** units recognized as tests (via `LanguageProfile`: `#[test]`/`@Test`/pytest conventions, `test_`/`_test` naming, `tests/` paths) are legitimately repetitive (arrange-act-assert, table-driven cases) and would otherwise dominate the report. Policy per config `tests.mode`: `separate` (default — own report section, never fails CI), `exclude`, or `normal`. Test-vs-test findings follow the policy; test-vs-production findings are always reported normally (production code duplicated into a test fixture is a real finding).
- Apply a size floor **after normalization** (§5.2): units below `min_unit_tokens` (default 40 normalized tokens) are discarded, and sequence-tier repeats below `min_seq_tokens` (default 30) are not reported. Rationale: aggressive normalization makes all small functions (getters, trivial loops) converge — true-but-useless positives. This floor is the single most important precision knob; make it prominent in config and report which findings sit near it.

### 5.2 Normalization (P3)

Produce a canonical tree per unit. Ordered transformation list (apply in this order; order matters and should be covered by tests):

1. **Strip** comments, doc comments, attribute/decorator nodes that don't change behavior (configurable allowlist — e.g., keep `#[test]`/`@pytest.fixture` since they change unit semantics for grouping purposes).
2. **Loop decomposition** (before other desugaring): lower every loop form to a minimal per-language core —

   ```
   init*                    # hoisted before the loop
   loop:                    # unconditional loop primitive (Rust `loop`; Python/TS/Go/Kotlin: while-true)
       if !(cond): break    # break position preserved: pre-test vs post-test is real structure
       body
       step*
   ```

   One lowering rule per source form (`for`, `while`, `do`/`repeat`, `loop`+`break`, C-style three-clause, **and linear recursion — see below**) replaces pairwise convergence decisions between forms, and divergences between two loops localize exactly where anti-unification (§5.6) wants them: in the init/cond/step/body holes. Additions:
   - **Recursion lowering (linear/tail recursion is the fourth loop form):** a *self-call* — callee name matches the enclosing definition; detectable because lowering runs before identifier abstraction — in *tail position* (`return f(...)`, or `f(...)` as final block expression in expression-bodied languages) lowers to parameter reassignment + continue; base-case returns stay returns; the function's parameters become the loop-carried variables:

     ```
     def f(acc, xs):                         loop:
         if not xs: return acc        →          if not xs: return acc
         return f(acc + xs[0], xs[1:])           acc, xs = acc + xs[0], xs[1:]
     ```

     This is the AST analog of tail-call elimination, made viable by unsoundness tolerance: no safety proof required, a wrong lowering only produces a noisy candidate. **Optional second rule** (implement in Phase 2 if the first rule lands cleanly, else Phase 3): *near-tail linear recursion* — `return n * f(n-1)` and analogs with an associative operator — lowers via accumulator introduction (the AST analog of LLVM's tail-recursion-modulo-accumulation; associativity assumed, unsound, tolerated). The iterative counterpart an LLM writes almost always carries an explicit accumulator variable, so this rule targets exactly the converging shape. **Scope boundary, permanent:** non-linear recursion (≥2 self-calls — tree recursion, fib, quicksort, recursive DFS) does not lower; converging it with explicit-stack iteration is beyond even IR-level optimization and stays in the behavioral band, out of scope. Mutual recursion: skipped. **Provenance:** the components are classical (Scheme TCO, tail-recursion-modulo-cons) but their use as *clone-detection canonicalization* was not found published — same earn-its-keep treatment as §5.5.3, measured per §7.4.
   - **Loop-exit normalization (companion rule, required for exact convergence with lowered recursion):** when a loop is the final statement of a unit and a `break` is immediately followed by `return E`, pull the return into the loop at the break site. One canonical direction only; covered by the idempotence property test. Without this, lowered recursion (`return` inside loop) and lowered iteration (`break` + trailing `return`) converge only as near-misses; with it, clean cases converge exactly.
   - **Iteration-protocol rewrite:** index-over-collection-length with subscript access (`for i in range(len(xs)): … xs[i] …` and analogs) rewrites to the iterated form (`for x in xs`). Decomposition alone does not converge this split — it differs in *what* is iterated, not loop shape — and it is a signature LLM drift pattern, so it gets its own rule.
   - **`continue` note for the implementer:** in C-style `for`, `continue` executes the step; in the lowered form with a manual trailing step, it skips it. The lowering deliberately conflates these. Do not "fix" this — for detection it is correct and valuable: an LLM that rewrote a `for` as a `while` and introduced exactly this bug *should* converge with the original into one finding.
   - **Unrolling is absent by design** (this is a decision, not an omission): the unrolled→rolled direction is handled by folding (§5.3), which collapses N repeated copies into one template rather than multiplying one loop into N-copy variants; unrolling as a variant generator would require guessing unroll factors, amplify any body divergence by the copy count, and inflate the index for no coverage folding doesn't already provide.
3. **Desugar** the remainder per language via rewrite rules on the tree (implemented as tree-sitter query + tree rewrite). Minimum set per language, extend as calibration demands:
   - Rust: `if let`→`match` canonical form; method-call chains left as-is (do NOT attempt trait resolution); `?` left as-is.
   - Python: comprehension → lowered loop form (via step 2); `with` multi-item → nested; f-string → format-call canonical node.
   - TypeScript: arrow fn → function canonical node; optional-chain left as-is; async left as-is.
   - Go: all three `for` forms handled by step 2.
   - Kotlin: lambda-with-receiver left as-is; `when` → the language's canonical multi-branch form.
   - **Per-language canonical forms (no shared vocabulary):** matching is same-language only, so desugaring targets are defined per language and should reuse the grammar's *existing* node kinds wherever possible — the lowered loop core renders as native `loop`/`while True` syntax. Invent a synthetic node kind only when no native kind fits (likely just REPEAT from §5.3 and an expression-block node for §5.4). Rationale: rewriting into native kinds keeps rewrite rules small, makes normalized trees renderable as pseudo-source for report diffs, and avoids designing a lossy cross-language IL that nothing consumes.
4. **Identifier abstraction**: rename identifiers positionally *within each unit* — first distinct local → `v0`, second → `v1`, etc. **Exception:** calls to identifiers *defined outside the unit* (imports, other functions in the repo, stdlib) keep their resolved-or-literal name, because `sort()` vs `parse()` is a real semantic difference. Distinguish "local" vs "external" purely syntactically: declared in unit (params, let/var/def/assignments at first occurrence) → local; otherwise external. This is heuristic; accept the noise.
5. **Literal abstraction**: replace literals with typed buckets — `INT`, `FLOAT`, `STR`, `BOOL` — **except** literals in a small keep-list (0, 1, -1, "", common sentinel values) whose identity is often structural. Configurable.
6. **Order canonicalization**: sort order-insensitive constructs — import lists, struct/dict literal fields by key, chained `&&`/`||`/commutative-`+`/`*`/`|`/`&` operand lists by fingerprint of operand subtree. **Known unsoundness** (floats, operator overloading, short-circuit side effects): accepted, because output is a report. Do not add type analysis to fix this.
7. **Dead-syntax removal**: redundant parens, empty else, `pass`/`{}` bodies normalized.

Property tests (proptest): normalization is deterministic and idempotent (`normalize(normalize(t)) == normalize(t)`); source spans survive.

### 5.3 Sibling-run folding / re-roll (P4)

Detect runs of ≥ `fold_min_repeats` (default 3) consecutive sibling subtrees whose normalized fingerprints match **up to literal/identifier holes**: compute each sibling's fingerprint with literals and local identifiers masked; if a run matches under masking, replace the run with `REPEAT(n, body-template, hole-values)`.

Two payoffs, both must be implemented:
- **Canonicalization:** a hand-unrolled/LLM-flattened copy now matches the rolled original — after loop decomposition (§5.2.2), the rolled loop's body and the folded REPEAT's body-template converge on the same lowered form. Folding is the *only* mechanism for rolled↔unrolled convergence; there is deliberately no unrolling pass (§5.2.2).
- **Direct finding:** a REPEAT node above a size threshold is itself reportable as *internal duplication* ("this function contains 6 near-identical statement groups — extract a loop or helper"), tier-labeled `internal-repeat`. LLMs produce exactly this pattern.

Edge cases to handle: runs with a divergent final iteration (loop epilogue pattern — allow last element to mismatch and note it); nested runs (fold innermost first, fixpoint).

### 5.4 Best-effort inliner (P5)

Purpose: converge "caller uses existing helper" with "LLM reimplemented the helper's body inline in another function."

- Build a repo-wide **definition table**: (name, arity, language, param names, body subtree) for every function-like unit. Resolution of a call site: match by callee name + arity within the same language; prefer same-module, then same-crate/package, then repo-wide. If >`inline_max_candidates` (default 3) candidates, skip the call. Do **not** integrate SCIP/LSP/stack-graphs in this phase — the stack-graphs project is archived (Sep 2025) and per-language index integration is a Phase-4+ option only if calibration shows resolution ambiguity is a real precision problem. Record ambiguity-skip counts in scan stats so that decision is data-driven.
- Inline = substitute argument subtrees for parameter names in a copy of the callee's normalized body, splice into the caller at the call site (as an expression-block canonical node). Purely syntactic substitution; ignore side effects, evaluation order, aliasing — **by design** (see §1, insight 1).
- Policy knobs: inline only callees ≤ `inline_max_callee_tokens` (default 120 normalized tokens); recurse to depth `inline_max_depth` (default 2); never inline direct self-recursive calls (the Rev 5 lowering consumes those instead); never inline across languages.
- **Call graph and SCCs:** the definition table plus name+arity resolution yields a syntactic call graph; compute its strongly connected components (Tarjan). SCCs of size ≥2 are mutually recursive groups. **SCC-scoped inlining:** within an SCC, allow exactly one expansion round (inline each SCC partner once, stopping when a cycle would repeat a member) — this converts mutual recursion into direct self-recursion, which the recursion lowering (§5.2.2) then converts to the loop core. Net effect: a mutually-recursive state-machine pair can converge with a loop-plus-state-variable implementation. The merged SCC unit also enters the index, enabling *group-vs-group* findings (SCC A duplicates SCC B). Clean (tail-position-after-expansion) cases only; non-tail residue falls to the api-profile tier (§5.7). Provenance: this chain composes three already-specified mechanisms and is untested as a whole; each link is separately gated, and the composition gets a dedicated mutation class (§7.1).
- Each unit is fingerprinted **twice**: once un-inlined, once fully-inlined-per-policy. Both variants enter the index, tagged. A match involving an inlined variant is tier-labeled `inline-assisted` and the report must show the inline chain (which callees were expanded) so a human can judge it.
- Determinism requirement: inlining decisions must depend only on the definition table and policy, never on iteration order. Sort candidate sets.

### 5.5 Fingerprinting (P6)

Four representations per unit/window/variant. The design problem here is **hash amplification**: a single divergent leaf invalidates every ancestor Merkle hash up to the root, so exact hashing is maximally intolerant of small divergences. Layers 2–3 exist to make small divergences *indexable* rather than fatal.

1. **Whole-unit exact structural hash:** Merkle-style over the normalized tree (node kind + child hashes + kept names/literals). Equal hash ⇒ converged duplicate, tier `exact-normalized`. 128-bit non-cryptographic hash (xxh3-128).
2. **Subtree hash bag:** the multiset of Merkle hashes of all subtrees above a small size floor (`bag_min_subtree_tokens`, default 6). Near-duplicates share most of the bag; estimated Jaccard over bags (via MinHash) is the primary near-miss retrieval signal. Side benefit: for a candidate pair, the *unmatched* hashes localize the divergences before any pairwise work.
3. **Hole-context hashes:** for each internal node, additionally emit its hash with each child slot replaced by a HOLE marker (k=1; k=2 for high-arity list nodes only if calibration shows recall need). Two trees differing in exactly one subtree under a node still share that node's 1-hole hash — the hole hash is a *template fingerprint*, making factored-out divergence a first-class index key. Cost is a bounded constant-factor blowup (≤ n × arity extra hashes); report index size in scan stats.
4. **Landmark-pair hashes (Shazam transfer):** select *landmarks* — small subtrees that are both distinctive (high IDF over the repo's subtree-hash distribution; the IDF machinery is shared with §5.7) and stable under normalization — then emit combinatorial pair hashes `(anchor_hash, target_hash, structural_offset)` for each anchor and the landmarks in a bounded following region (offset = normalized-token distance, bucketed). Rationale, imported directly from audio fingerprinting: individually common landmarks become combinatorially rare in pairs, so pairs are highly discriminating while a local edit destroys only the pairs touching it — audio identification works from 1–2% surviving hash tokens. This is the principled attack on the convergent-trivia risk (§12): post-normalization small subtrees all look alike; pairs at specific offsets don't. Pair hashes also power the offset-histogram verification in §5.6.
   **Rivalry clause:** layers 3 and 4 are competing unpublished syntheses aimed at the same retrieval gap. Both run in Phase 2 under §7.4(b) measurement; whichever contributes less candidate recall per unit of index size is **dropped, not kept alongside** (record in DECISIONS.md). Do not ship both.
5. **Characteristic vector** (Deckard-style, retained as a cheap secondary signal): occurrence counts of the language's post-normalization node kinds plus structural features (depth, branch count, REPEAT count). **The index is partitioned by language** — vectors and hashes from different languages are never candidates; each partition's LSH parameters can be tuned independently.

### 5.6 Matching, anti-unification, and verification (P7)

**Sequence tier (exact contiguous, replaces windows):** build a generalized suffix array + LCP over the corpus-wide normalized token stream (per language partition) and read off **maximal repeated sequences** ≥ `min_seq_tokens`. This is the classic token-clone technique (Baker's `dup`; the CPD lineage): one linear-ish pass finds every exact duplicated run, maximally — no window enumeration, no subsumption cleanup within the tier. Maximal repeats spanning whole units merge with the tree tier's `exact-normalized` findings; sub-unit repeats are reported as region findings. At single-repo scale the suffix array rebuilds in seconds, so both scan and check modes rebuild from cached per-file token streams (winnowing, per Schleimer et al., is the incremental alternative with a formal guarantee — detection of any match ≥ w+k−1 tokens at fingerprint density within 33% of the proven lower bound; implementer may adopt it for PR mode if suffix-array rebuild proves slow, recording the choice; either way the guarantee framing — "every duplicated run ≥ t normalized tokens is found" — is a stated product property).

**Tree tier (near-miss):**
- Bucket whole-unit exact hashes → groups directly (tier `exact-normalized`).
- Candidate retrieval: bag-Jaccard (§5.5.2) above `candidate_sim` (default 0.70 estimated), OR shared template/pair fingerprints (§5.5.3/4, whichever survives the rivalry) covering ≥ `hole_hash_min_cover` (default 0.5) of the smaller unit. Union of both candidate sets goes to histogram verification.
- **Offset-histogram verification and localization (Shazam transfer, pre-AU):** for each candidate pair, every shared fingerprint votes `(Δposition)` — the difference of its normalized-token offsets in the two units. A genuine clone produces a dominant histogram bin (the "diagonal": many fingerprints agreeing on one alignment offset); coincidental collisions scatter. Reject candidates with no bin ≥ `histogram_min_votes` (default 5) — this is O(shared fingerprints), far cheaper than AU on a false candidate. The winning bin's fingerprint span also **localizes the matched region** (the diagonal's extent = region boundaries), which both handles partial-function clones without windows and tells AU where to align. Lineage: clone research visualized exactly these diagonals in Duploc-style dotplots; the histogram trick makes diagonal-finding a linear hash-vote instead of an O(n²) matrix scan.
- **Verification = anti-unification** on histogram-confirmed pairs (or confirmed regions), as before:
  - At **fixed-arity nodes**: classical positional anti-unification; mismatched kinds produce a hole.
  - At **list-valued nodes**: align children by **Smith-Waterman local alignment with graded substitution costs** — a child pair contributes credit proportional to its subtree similarity (bag-Jaccard of the two subtrees), rather than binary LCS match/no-match. This recovers the most LLM-typical drift shape: a clone where *every* statement was lightly edited aligns correctly under graded costs but as nothing under binary LCS (the cover-song lesson: align with tolerance after normalizing, don't demand exactness). Unmatched children become *list-holes* (gaps). O(nm) per pair, acceptable post-histogram.
  - **Reordering blind spot, narrowed:** greedy string tiling (Wise; the JPlag algorithm — from-memory citation, verify when implementing) matches tiles regardless of order and is the designated Phase-4 option for statement-block reordering, cheaper than GumTree; a *moved* subtree still appears as two holes until then. Count suspected-move patterns (matching deleted/inserted hole contents) in stats to justify or kill the spike.
- **Acceptance and scoring** replace the flat similarity threshold with two structural criteria:
  - **Divergence ratio:** `(|σ1| + |σ2|) / (2 * |T|)` in normalized tokens, must be ≤ `max_divergence` (default 0.15). Report `1 - ratio` as the similarity figure.
  - **Factorability:** classify each hole — an *expression hole* (complete expression subtree, free variables bound consistently in both contexts) or *statement hole* (complete statement/block) is **factorable** (mechanically consolidable: expression holes → parameters, statement holes → closures/callbacks/strategy). A hole that captures a partial construct, or many scattered tiny holes, is **non-factorable**. Pairs whose holes are all factorable and number ≤ `max_holes` (default 5) are tier `near-normalized`; pairs over budget or with non-factorable holes are demoted to tier `weak-similarity` (reported only with `--verbose`, never fails CI). This distinction — invisible to token-LCS scoring — is the primary precision mechanism of the near-miss tier.
- Anti-unification is linear on the aligned trees, so it is cheap post-retrieval; the retrieval and histogram layers carry the scalability burden.
- **Clone classes:** merge pairwise findings into groups and compute the n-way anti-unifier of the group (fold the pairwise AU over members) so the report shows one template per group, not per pair.
- Suppress subsumed findings: if two whole functions match, don't also report their internal regions; region matches strictly contained in function matches are dropped (interval containment per file pair). A unit never matches itself or its own inlined variant. Sequence-tier findings contained in tree-tier findings are likewise dropped.

### 5.7 API-profile tier (P7b, suspicion-only)

A static-birthmark-style signal targeting **reimplemented-task duplication that survives structural rewriting** — the practically common slice of Type-4, where two structurally unrelated implementations of the same task necessarily invoke the same external effects. Grounding: the software-birthmark literature (Tamada et al. onward) established that API-call profiles are functionality-bound and resilient to semantics-preserving rewrites, with frequency-based static birthmarks achieving high precision — **at whole-program granularity**. Function-level transfer is unvalidated; treat every default below as a calibration input, not a claim.

- **Signature** per unit: the multiset of `(callee, control_context)` pairs, where callee is any preserved non-local name (§5.2 identifier-abstraction exception) — both true externals and *internal repo helpers*, the latter being plausibly the strongest single signal for LLM duplication (two functions calling the same rare repo-local helpers) — and control_context is `(loop_depth, in_branch?, tail_position?)`. Context annotation captures most of the useful order information ("calls X inside a loop" ≠ "calls X once") at a fraction of sequence-matching cost.
- **Weighting:** IDF over the repo's callee distribution, so `len`/`push`/`format`-class ubiquitous calls contribute ~nothing. Units with fewer than `api_min_distinct_rare` (default 3) rare callees emit no signature at all — pure-computation functions are **invisible to this tier by design**; the canonical algorithmic Type-4 cases (sort vs. sort) are permanently out of its reach and out of the tool's scope.
- **Matching:** weighted-Jaccard/cosine over signatures within the language partition, threshold `api_profile_sim` (default 0.8, calibrate). Exclude pairs already found by stronger tiers.
- **Reporting:** tier `api-profile`, its own report section, never fails CI, no template (these pairs are structurally different by construction, so AU verification is impossible — the evidence is the shared rare-callee list with contexts, rendered as such). Ranked by IDF-weighted overlap mass.
- **Return-shape features** (return count, returns-in-loop, error-return pattern) fold in as minor signature dimensions; do not let them dominate the callee signal.

---

## 6. Reporting and ranking (P8)

Rank duplicate groups by **estimated consolidation value**: `(members - 1) * template_token_count * factorability_factor`, where `factorability_factor` is 1.0 if all holes are factorable, scaled down otherwise; boosted 1.5x if members span ≥2 directories (more likely genuinely missed reuse than deliberate locality), and boosted if any member was touched in the diff range (PR mode). Within equal value, lower divergence ratio ranks first. Tiers, in descending confidence/actionability: **`inconsistent-update`** (see below) > `exact-normalized` > `internal-repeat` > `near-normalized` > `inline-assisted` > `api-profile` (own section, suspicion-only, never fails CI) > `weak-similarity` (verbose only).

**`inconsistent-update` (drift tracking — the tool's most actionable finding).** In `check` mode, when the diff modifies some but not all members of a duplicate group known from the baseline state, emit an `inconsistent-update` finding: *"this change touches member 1 of a 3-member group; members 2 and 3 were not updated"*, listing the untouched members and the group template. This fires at exactly the moment the empirically established fault mechanism operates — Juergens et al. (ICSE 2009) found 52% of clones changed inconsistently, with roughly every second unintentional inconsistency constituting a fault — and prior art for the tracking mechanism exists (CloneTracker, Duala-Ekoko & Robillard ICSE 2007; the clone-genealogy line). Membership and template hashes come from the baseline file (§2); a group whose divergence ratio has *increased* since the baseline snapshot is additionally flagged as drifting, with the trend reported. Default `fail_on` includes this tier: it is the finding a reviewer most needs to see before merge. Baseline interaction: baselined groups are exempt from re-reporting as duplicates but are **still tracked** for inconsistent updates — accepting a duplicate's existence is not accepting its divergence.

Each finding: group id, tier, divergence ratio, members (file:line-span, function name, language), inline chain if any, `parse_degraded` flags — and for `near-normalized` groups, the **group template rendered as pseudo-source** with `⟨hole₁⟩…⟨holeₖ⟩` markers, plus a table of each member's substitution per hole. The template is the primary evidence artifact: it shows a reviewer exactly what's shared and exactly what differs, and factorable holes double as the refactoring recipe (expression holes → parameters, statement holes → closures). Rendering as pseudo-source is possible because normalization targets native node kinds (§5.2.2). Human-format output must lead with the top-N (default 20) and a one-line stats summary (units indexed, candidates, findings by tier, ambiguity-skips, suppression counts, hole/pair-hash index size).

### 6.1 Output-format conformance

No clone-detection-specific report *standard* exists; the prior art is one interchange standard (SARIF), two de facto CI formats, and a set of metric definitions. Conform to all three so the tool drops into existing pipelines and its numbers are comparable rather than bespoke.

**SARIF 2.1.0 (`--format sarif`)** — OASIS standard, GitHub code-scanning consumes a subset. Two clone-specific mapping decisions the default recipe gets wrong:

- *Duplicate group = one result at N locations, not N results.* Encode group members in `result.locations[]`; set the **primary** location to the diff-touched member (PR mode) or highest-ranked member (scan mode), and carry the remaining members as `relatedLocations[]` with integer `id`s. The `message` embeds the AU template with `[member](id)` references to those related locations — this matches GitHub's own multi-location result example and renders the template as the finding's explanation. Each hole's per-member substitution attaches to the corresponding related location.
- *Fingerprints must be structural, not line-based (the single highest-value reporting decision).* GitHub uses `partialFingerprints` to decide when two results are logically identical across runs; the default `primaryLocationLineHash` hashes source text, so any cosmetic edit to a member churns the alert (closes + reopens). Instead set `partialFingerprints` to the **tier-1 normalized structural hash** (exact tier) or the **AU template hash** (near-miss tier) — both already computed (§5.5–5.6). Result: a clone group stays the *same* alert as members drift through whitespace/rename/literal edits, exactly the drift this tool exists to track. Without this, expect the Trivy failure mode (N identical alerts for N references of the same finding on every rescan).
- *Do not overload `level`.* A clone is not a severity; keep the tier taxonomy (§6) in a `properties` bag and only coarse-map to `error`/`warning`/`note` for the CI gate (`fail_on`). Emit spec-valid general SARIF; the primary-location choice above is the GitHub-consumer optimization, not a spec requirement.

**CPD/PMD XML and jscpd JSON (`--format cpd`, `--format jscpd`)** — the lingua franca for duplication specifically (CPD's `<duplication lines= tokens=><file/><file/><codefragment/>` and jscpd's JSON + `--threshold`/max-clone exit-code convention). Two reasons to emit these: teams can drop the tool into an existing CPD/jscpd pipeline with no config change, and §7.3's baseline comparison becomes a mechanical diff instead of a manual reconciliation. These are incumbent-matching conventions, not standards; match them closely enough to be a drop-in.

**Metric definitions (for CALIBRATION.md and `scan --stats`)** — report the standard quantities with their literature definitions so results are comparable to industry tooling: precision / recall / F1 **per clone type** (Roy–Cordy taxonomy, §1); **duplication ratio** as both %duplicated-lines and %duplicated-tokens (token-based is more stable across formatting — report both, as CPD/jscpd do); clones per KLOC. Report the %-duplicated figure in the same shape GitClear used for the problem statement (copy/pasted-line percentage) so a team can place itself against that benchmark. Per §7, the mutation-benchmark and manual-sample numbers remain primary evidence; these ratios are context.

---

## 7. Evaluation and calibration (mandatory, not optional)

Precision claims must be measured, not asserted. Build this into the repo:

1. **Synthetic mutation benchmark** (`benches/mutations/`): a generator that takes seed functions from real code and applies scripted mutations — rename all identifiers, change literals, swap loop forms, reorder independent statements, unroll a loop 3x, inline a called helper, wrap in extra nesting, **lightly edit every statement (the Smith-Waterman class: binary LCS must fail it, graded alignment must pass it), rewrite an iterative seed as tail recursion, rewrite as near-tail (accumulator) recursion, rewrite as a mutually-recursive pair (tests the §5.4 SCC chain end-to-end), and — as designed-to-fail controls — rewrite as tree recursion (must NOT converge) and pair random unrelated functions of similar size (histogram verification must reject them)**. Each mutation class maps to the tier that should catch it. Recall per mutation class is the primary regression metric; wire into CI.
2. **Real-repo calibration:** run against 3+ real repos (the tool's own repo; at least one large OSS Rust repo, e.g. a medium crate workspace; one Python repo). Sample 50 findings stratified by tier, manually label true/false positive, record precision per tier in `CALIBRATION.md`. Tune `min_unit_tokens`, `report_sim`, inline policy against this. Repeat after any normalization change.
3. **Baseline comparison:** run `jscpd` and PMD CPD on the same repos; report the overlap and, specifically, the findings reprise makes that they miss (this is the tool's reason to exist — if the delta is empty, say so honestly in CALIBRATION.md and stop). Emit the `--format cpd`/`--format jscpd` output (§6.1) so this comparison is a mechanical diff of like-formatted results, not a hand reconciliation.
4. **The unmeasured variables from the design discussion:** (a) what fraction of near-duplicates converge to *exact* normalized equality vs. needing the anti-unification tier — validates or refutes normalization-first; (b) **the retrieval rivalry** (§5.5.3 vs §5.5.4): measure hole-context hashes and landmark-pair hashes against the bag-only baseline for candidate recall per unit of index size; the loser is dropped (both are unpublished synthesis); (c) the distribution of divergence ratios and hole counts on true vs. false positives — sets `max_divergence`/`max_holes` empirically rather than by the guessed defaults; (d) **api-profile function-level precision** (§5.7): the birthmark literature's numbers are whole-program; sample this tier's findings separately and expect to tighten `api_profile_sim`/`api_min_distinct_rare` substantially — if function-level precision is unusable even after tuning, drop the tier (record in DECISIONS.md); (e) **histogram verification effectiveness**: false-candidate rejection rate and the random-pair control class result — this gate is what keeps AU cheap, so measure it explicitly.

Ground-truth caution: do not use BigCloneBench for headline numbers (widely criticized label quality); the mutation benchmark + manual sampling is the evidence standard here.

---

## 8. Phasing and acceptance gates

**Phase 1 — Skeleton + exact tier (Rust, Python).**
P1–P3 (normalization without desugaring beyond loops), P6.1 exact hashes, P7 bucketing, minimal terminal report. Gate: on the mutation benchmark, 100% recall on Type-1/Type-2 mutation classes; runs on its own repo; scan of a 100k-LOC repo < 30s.

**Phase 2 — Sequence tier, near-miss tier, folding.**
Suffix-array sequence tier; full §5.2 desugaring for Rust/Python; P4 folding; P6.2–5 (hash bags, both rivalry layers, vectors) + LSH; P7 histogram verification + anti-unification with Smith-Waterman list alignment and factorability classification; subsumption; template rendering; ranking; JSON output; test-code policy. Gate: ≥80% recall on Type-3 mutation classes (loop-form swap, statement reorder, 3x unroll, single-subtree substitution, **light-edit-every-statement**); ≥70% recall on the tail-recursion class; 0% convergence on the tree-recursion and random-pair controls; manual precision sample ≥70% on real repos with `weak-similarity` demotion working; CALIBRATION.md exists including the §7.4 measurements and **the rivalry verdict** (§7.4(b)).

**Phase 3 — Inliner, baseline/drift, PR mode, remaining languages.**
P5 per §5.4 including the SCC chain; the api-profile tier (§5.7); **`reprise baseline` + `reprise:ignore` + baseline-aware `check`**; **`inconsistent-update` drift findings from baseline state**; `check --base` incremental mode with version-keyed on-disk index; TypeScript, Go, Kotlin profiles; SARIF. Gate: inline-mutation class recall ≥70%; mutual-recursion mutation class recall ≥50% (the chain is speculative; a low bar is honest); `inline-assisted` tier precision ≥50% on manual sample; api-profile tier gated only by §7.4(d) measurement existing, not by a number; a scripted end-to-end drift scenario (baseline → edit one member → `check` emits `inconsistent-update` naming the untouched members) passes; PR check < 5s warm on 500k-LOC repo.

**Phase 4 — Hardening.** Config polish, docs, generated-code detection tuning, performance, publish-ability. Optional spikes *only if calibration data justifies*: SCIP-based resolution for the inliner; embedding sweep for Type-4 residue; LLVM-IR deep tier (§11).

Each phase ends with updated CALIBRATION.md and DECISIONS.md. Do not proceed past a gate silently; if a gate fails, record why and either fix or renegotiate the gate in DECISIONS.md.

---

## 9. Configuration reference (defaults)

```toml
[scan]
exclude = []                  # globs, additive to .gitignore
generated_markers = ["@generated", "DO NOT EDIT"]

[thresholds]
min_unit_tokens        = 40    # post-normalization size floor
min_seq_tokens         = 30    # sequence-tier maximal-repeat floor
bag_min_subtree_tokens = 6     # subtree-hash bag floor
candidate_sim          = 0.70  # bag-Jaccard retrieval threshold
hole_hash_min_cover    = 0.5   # template/pair fingerprint retrieval threshold
histogram_min_votes    = 5     # offset-histogram diagonal acceptance
max_divergence         = 0.15  # AU acceptance: (|σ1|+|σ2|)/(2|T|)
max_holes              = 5     # AU acceptance: hole-count budget
fold_min_repeats       = 3

[tests]
mode = "separate"   # separate (own section, never fails CI) | exclude | normal

[baseline]
file = "reprise-baseline.json"
track_drift = true  # inconsistent-update findings + divergence trend on baselined groups

[inline]
enabled            = true
max_callee_tokens  = 120
max_depth          = 2
max_candidates     = 3

[api_profile]
enabled              = true
api_profile_sim      = 0.8   # weighted-Jaccard threshold; expect to tighten per §7.4(d)
api_min_distinct_rare = 3    # units below this emit no signature

[report]
top = 20
fail_on = "exact-normalized"  # PR mode: minimum tier that fails CI; "none" disables
sarif_fingerprint = "structural"  # "structural" (stable across cosmetic drift, recommended) | "line" (GitHub default)
```

---

## 10. Non-goals (do not build)

- Full Type-4 semantic clone detection (different algorithms, recursion↔iteration). Out of scope; the fuzzy tier will occasionally catch instances, fine, but no architecture for it.
- Program transformation / auto-refactoring. Report only.
- Soundness of the inliner or of commutative reordering. Explicitly waived (§1).
- Cross-language clone matching (Python impl vs Rust impl). Firmly out. Do not design normalization targets, node vocabularies, or index structures to preserve this option — same-language partitioning is a load-bearing simplification (§3, §5.2.2), not a temporary restriction.
- Type checking, name resolution beyond the syntactic definition table, LSP/SCIP integration (Phase-4 spike at most).
- A daemon/server/IDE plugin. CLI + CI only.

## 11. Deferred: LLVM-IR deep tier (sketch only — do not implement)

**Mission, stated precisely — and reduced by Rev 5:** the AST tier now handles linear-recursion↔iteration directly (§5.2.2), removing this tier's former flagship case. What remains is *residue insurance*: recursion/iteration pairs the syntactic rules miss (non-tail shapes beyond the accumulator rule's reach), arithmetic restructurings normalized by `reassociate`/`gvn`, and convergences that emerge only from optimization interactions (inlining enabling simplification enabling convergence). It does **not** converge algorithm drift proper (quicksort vs. mergesort survives every optimizer) — that band requires behavioral equivalence and stays out of scope entirely — and non-linear recursion vs. explicit-stack iteration is beyond this tier too. Evidence basis: compilation round-trips canonicalize modifications that defeat source-level detectors (Ragkhitwetsagul & Krinke 2017); their *decompilation* step existed only because NiCad consumes source — this design compares at canonicalized IR directly and skips that lossy stage. **The bar for building this tier is now correspondingly higher:** it requires Phase 1–3 calibration to show a material recall gap that is specifically in the residue classes above, in compiled-language code, at a frequency that justifies a second toolchain.

Pipeline sketch, for compilable subsets (Rust, C/C++; Go via gollvm or analogous gc-SSA treatment): per-function IR at -O0 → `mem2reg` → policy-forced inlining (fixed threshold, no cost model) → `tailcallelim` → `loop-simplify`/`loop-rotate`/`indvars` → full unroll of constant-trip loops ≤ fixed cap (unrolling is appropriate *here*, unlike the AST tier, because IR-level folding does not exist and constant-trip unrolling is deterministic) → `sroa`/`instcombine`/`reassociate`/`newgvn`/`simplifycfg`/`dce` → LLVM IR normalizer pass (`normalize`, from llvm-canon) → structural hash of region DAGs; survivors to MinHash; final tiny candidate set to pairwise equivalence checking (Alive2 or e-graph saturation). The key open question it answers is how often heuristic-free normalization actually converges structural-drift clones. Only pursue if Phase 1–3 calibration shows a material recall gap on structurally-rewritten duplicates in compiled-language code.

## 12. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Normalization too aggressive → precision collapse on small units | Size floor is post-normalization; calibrate per §7; report near-floor findings distinctly |
| Inliner noise swamps report | Separate tier, ranked below others, show inline chain, per-tier `fail_on` |
| Desugaring rules per language balloon | Prefer rewrites into native node kinds (§5.2.2); add rules only when a mutation-benchmark case or calibration FN demands; per-language rule count reported in scan stats |
| Tree-sitter grammar quirks (TSX, Kotlin) | Pin grammar crate versions; grammar-level test fixtures per language |
| Inline variants blow up index size | Variants capped at 2/unit; sequence tier operates on streams, not enumerated windows; measure index size in scan stats |
| Retrieval-layer rivalry (hole hashes vs landmark pairs) ships both and doubles index cost | §7.4(b) measurement with a mandatory drop-the-loser outcome; DECISIONS.md records the verdict |
| Baseline becomes a dumping ground (all findings baselined, tool neutered) | Suppression and baseline counts in every report header; drift tracking still active on baselined groups; `inconsistent-update` cannot be baselined away group-wide (only per-pragma) |
| Stale baseline/drift state after normalizer or grammar upgrade corrupts trend data | Version-keyed caches (§4); baseline entries carry the fingerprint-scheme version; mismatched versions trigger re-baseline prompt, never silent comparison |
| Smith-Waterman O(nm) alignment too slow on large candidate sets | Histogram verification gates AU (§5.6); measure rejected-candidate rate per §7.4(e); band-limited S-W (only cells near the histogram diagonal) as the optimization if needed |
| Hole-context hash layer (unpublished synthesis) adds cost without recall | Measured against bag-only baseline per §7.4(b); drop if it doesn't earn its keep |
| Moved subtrees inflate divergence ratio (AU sees delete+insert, not move) | Accept initially; count suspected-move patterns (matching deleted/inserted hole contents) in stats; GumTree-style move detection is a Phase-4 spike if material |
| n-way group anti-unifier degrades to trivial template as group grows | Compute group template but accept a member only if its pairwise divergence to the template stays ≤ `max_divergence`; split groups otherwise |
| Loop decomposition over-homogenizes small trees (everything-with-a-loop shares the lowered skeleton → coincidental convergence) | Size floor applies post-lowering; cond/step/body content still feeds all hashes; calibration §7 samples loop-heavy findings specifically |
| Recursion lowering misfires (non-self calls matched by name collision; accumulator rule applied to non-associative ops) | Tolerated by design for detection; tree-recursion control class in mutation benchmark guards the worst case; misfire rate observable via §7.2 precision sampling of recursion-converged findings |
| SCC chain (inline → self-recursion → lowering) fails silently at any link, producing garbage merged units | Dedicated end-to-end mutation class (§7.1); merged SCC units tagged in scan stats; low Phase-3 gate (50%) with honest renegotiation if unmet |
| api-profile tier drowns in function-level noise (birthmark evidence is program-level only) | IDF weighting + rare-callee floor; suspicion-only reporting, never fails CI; §7.4(d) measurement with an explicit drop-the-tier outcome |
| Subsumption bugs → duplicate findings of the same duplication | Property tests on interval containment; snapshot tests on report output |

## 13. References (for the implementer's background reading, not to be re-litigated)

- Roy & Cordy, *NICAD: Accurate detection of near-miss intentional clones using flexible pretty-printing and code normalization*, ICPC 2008.
- Plotkin, *A note on inductive generalization*, Machine Intelligence 5, 1970; Reynolds, *Transformational systems and the algebraic structure of atomic formulas*, Machine Intelligence 5, 1970. (Anti-unification origins.)
- Bulychev & Minea, *Duplicate code detection using anti-unification*, SYRCoSE 2008, and *An evaluation of duplicate code detection using anti-unification*, IWSC 2009. (CloneDigger; the direct ancestor of §5.6.)
- Evans, Fraser & Ma, *Clone detection via structural abstraction*, WCRE 2007. (Clones with arbitrary subtree changes; ancestor of the hole-hash idea in §5.5.3.)
- Li & Thompson, *Similar code detection and elimination for Erlang programs* (Wrangler), PADL 2010, and the incremental follow-up reporting clone classes with their anti-unifiers. (Model for group templates + refactoring integration.)
- Falleri et al., *Fine-grained and accurate source code differencing* (GumTree), ASE 2014. (Phase-4 move-detection option, §5.6.)
- Augsten, Böhlen & Gamper, *pq-grams* (approximate tree matching) — background for the hash-bag layer (optional reading).
- Tamada, Nakamura, Monden & Matsumoto, *Java Birthmarks — Detecting the Software Theft*, IEICE Trans. 2005 (static birthmarks: sequence-of-method-calls, used-classes); and the static API-call-frequency birthmark line (Choi et al., SAC 2013) plus authority-histogram refinements (frequency + call order + importance weighting). (Basis for §5.7; note all validated at whole-program granularity — the function-level transfer is this project's open question §7.4(d).)
- Jiang, Misherghi, Su, Glondu, *DECKARD: Scalable and accurate tree-based detection of code clones*, ICSE 2007.
- Ragkhitwetsagul & Krinke, *Using compilation/decompilation to enhance clone detection*, IWSC 2017. (Motivates §11.)
- Jia et al., *1-to-1 or 1-to-n? Investigating the effect of function inlining on binary similarity analysis*, TOSEM 2022. (Motivates §5.4; documents inlining-simulation strategies.)
- Jiang & Su, *Automatic mining of functionally equivalent code fragments via random testing*, ISSTA 2009. (Type-4 dynamic route; out of scope, cited for boundary-setting.)
- Tate, Stepp, Tatlock, Lerner, *Equality Saturation: A New Approach to Optimization*, POPL 2009; and the `egg` library. (Deferred equivalence-verification option in §11.)
- GitClear, *AI Copilot Code Quality 2025* (211M changed lines, 2020–2024). Problem-scale evidence; methodology counts Type-1 blocks.
- tree-sitter; tree-sitter-graph; github/stack-graphs (archived 2025-09-09 — reason SCIP/LSP is the only viable precise-resolution path, and why §5.4 avoids it).
- Svajlenko & Roy critiques of BigCloneBench label quality (evaluation caution in §7).
- OASIS, *Static Analysis Results Interchange Format (SARIF) Version 2.1.0*, and GitHub's code-scanning SARIF-support docs (multi-location results via `relatedLocations`, `partialFingerprints` for cross-run result identity). Basis for §6.1's SARIF mapping and the structural-fingerprint decision.
- PMD CPD report format and jscpd (duplication-report XML/JSON conventions and threshold exit codes). Incumbent CI formats matched in §6.1.
- GitClear, *AI Copilot Code Quality 2025* (cited above for problem scale) — also the source of the %-duplicated-lines metric shape reused in §6.1 for benchmark comparability.
- Baker, *On finding duplication and near-duplication in large software systems* (`dup`), WCRE 1995. (Suffix-structure maximal-repeat lineage behind the §5.6 sequence tier.)
- Schleimer, Wilkerson & Aiken, *Winnowing: Local Algorithms for Document Fingerprinting*, SIGMOD 2003 (MOSS). Guarantee: local algorithms detect any match ≥ w+k−1; winnowing density 2/(w+1), within 33% of the proven lower bound. (§5.6 incremental alternative and the product's guarantee framing.)
- Wang, *An Industrial-Strength Audio Search Algorithm*, ISMIR 2003 (Shazam). Combinatorially hashed constellation pairs (anchor + target + offset) and scoring by clusters of time-aligned hash tokens; identification from 1–2% surviving hashes. (Basis for §5.5.4 landmark pairs and §5.6 offset-histogram verification.)
- Ducasse, Rieger & Demeyer, *A language independent approach for detecting duplicated code* (Duploc), ICSM 1999. (Dotplot-diagonal ancestry of the histogram method — from-memory citation, verify when implementing.)
- Juergens, Deissenboeck, Hummel & Wagner, *Do Code Clones Matter?*, ICSE 2009. 52% of clones changed inconsistently; ~every second unintentional inconsistency a fault. (Empirical justification for the `inconsistent-update` tier.)
- Duala-Ekoko & Robillard, *Tracking code clones in evolving software* (CloneTracker), ICSE 2007; Kim, Sazawal, Notkin & Murphy, *An empirical study of code clone genealogies*, ESEC/FSE 2005. (Prior art for drift tracking, §6.)
- Wise, *String similarity via greedy string tiling* (YAP3/JPlag lineage). (Phase-4 reorder option, §5.6 — from-memory citation, verify when implementing.)
