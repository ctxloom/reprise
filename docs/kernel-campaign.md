---
title: Linux-kernel campaign — grammar probe, scale, findings legitimacy, preprocessor tiers (2026-07-10)
status: campaign complete, five reports banked. Grammar-probe rulings are already load-bearing in the
  shipped C frontend (commit e787158): preproc-conditional transparency, ERROR-tolerant extraction,
  switch/goto kept Native for v1, and the designated-initializer count staged for the future aggregate
  thread. Full-TU macro expansion is REJECTED (data in §5). Trigger-once macro materialization is
  mechanically proven but its guardrail-acceptance round is an honest 0/10 — round 2 (statement-expression
  normalization) is not yet built. Follow-ups filed, not resolved (§6).
sessions:
  - stark-mixed-front
related:
  - docs/PLAN.md §Concurrency (line 104 — the 500k-LOC/60s, <5s-warm-PR-check targets measured here)
  - docs/SIMILARITY-IR.md (the IR the C frontend lowers to; §12.1's promotion gate governs whether any
    finding here — e.g. guard-inversion — earns a canonical-IR change)
  - docs/universal-ir-trap.md (cross-language behavioral convergence stays out of scope; C is
    same-language matching)
  - DECISIONS.md D44 (the "opt-in, requires-build, deep-Type-4 tier" escape hatch full-TU expansion
    would have needed, and that §5 recommends against building)
  - source reports (outside the repo): /home/babbitt/.ctxloom/sessions/stark-mixed-front/artifacts/
    {c-probe,wp-k2,cocci-validate,guardrail,expansion-probe}-report.txt
---

# Linux-kernel campaign

Five measurement reports, one real corpus (Linux v7.1, commit 8cd9520d35a6c38db6567e97dd93b1f11f185dc6,
local shallow clone), run against reprise's C frontend as shipped at commit e787158 (IR-first, no
historical profile). This document synthesizes them; it states no number that isn't in one of the five
reports. Where a report's own number carries an explicit caveat (extrapolation, sample bias, unverified
hypothesis), that caveat is carried over here too.

## 1. Purpose & scope

The campaign was scoped against three success criteria, in order of how much confidence they can bear:

1. **Works at scale.** Does `reprise scan` survive a real, large, C corpus — not toy fixtures — and where
   does it stop working?
2. **Corpus banked.** Does the campaign leave behind kernel-harvested invariants, mutation-bench C
   classes, and wild fixtures the C frontend can be regression-tested against going forward?
3. **Findings legitimacy.** When reprise reports a clone in mature, heavily-reviewed kernel code, is the
   finding actually something a maintainer would act on — or noise?

Expectations for (3) were deliberately set low going in: the kernel is ~35 years of code review by a
famously duplication-averse maintainer culture. Finding large volumes of actionable internal duplication
was never the bar; the bar was whether the *few* findings that do surface hold up under manual
inspection, and whether reprise's own precision machinery (floors, divergence rejection) behaves
sensibly on this scale of input.

A fourth thread ran alongside the above and motivates most of §5's work: an **agent-guardrail**
question — when an LLM coding agent is asked to reimplement something instead of calling a macro it
either doesn't see or doesn't bother reading, does reprise catch the reimplementation as a near-duplicate
of the macro's real body? This is a distinct question from "does reprise find kernel bugs"; it's "can
reprise backstop an agent that rolls its own `round_up()`."

## 2. Grammar probe → frontend decisions

`c-probe-report.txt` sampled 392 files (235 `.c` / 157 `.h`, 59.9%/40.1% split) across kernel/, fs/,
drivers/{gpu,net,media,clk,usb,scsi,input,pinctrl}, net/, mm/, lib/, include/linux/ — a stratified,
deterministic (every-Nth-file) sample, not random — and parsed all 392 with tree-sitter-c 0.24.2 against
reprise's exact tree-sitter 0.26 / tree-sitter-language 0.1.7 ABI line. Four rulings came directly out of
this probe and are now load-bearing in the shipped frontend:

- **Preprocessor conditionals are transparent, never expanded.** `#if`/`#ifdef`/`#elif`/`#else` wrap
  *normally-typed* children — of 916 `preproc_if`+`preproc_ifdef` nodes in the sample, **352 (38.4%)**
  directly contain a `function_definition` descendant. The frontend recurses through these wrapper kinds
  into their children rather than treating a conditional block as one opaque unit; both branches of an
  `#ifdef`/`#else` pair parse as independent, fully-structured functions. Macro *definition* bodies
  (`preproc_def`/`preproc_function_def`) are the opposite case — always one opaque `preproc_arg` string,
  by grammar design — and stay Native/residue.
- **ERROR-tolerant extraction, not clean-parse gating.** **40.1%** of sampled files (157/392) had zero
  ERROR nodes; the probe's structural finding is that ERROR nodes in the rest are almost always small,
  localized islands (an attribute-macro token like `__init` isolated in its own ERROR node between
  well-typed siblings) — so unit harvesting must **not** gate on `root.has_error() == false`. One
  outlier, `kernel/sched/core.c`, collapses its entire `translation_unit` into one top-level ERROR node
  yet still yields 452 correctly-typed `function_definition` descendants; excluding that file, whole-
  sample bytes-under-ERROR drops from 4.89% to **1.31%** — per-node extraction survives even that file.
- **switch/goto kept Native for v1.** The probe's own recommendation, on frequency grounds, was the
  opposite: `goto_statement` appears in 60.9% of sampled `.c` files (2,699 occurrences) and
  `switch_statement` in 40.0% (512 occurrences, ~5.8 `case` arms each) — both "far too common to leave
  as Native/unlowered" per the probe's MAP-scope recommendation. The shipped frontend kept both Native
  for v1 anyway (scope/time trade-off, not a disagreement with the data); switch lowering is filed as a
  follow-up (§6), driven by a concrete case the Coccinelle validation surfaced (§4).
- **Designated initializers staged for the future aggregate thread.** 5,661 `initializer_pair` nodes in
  the sample, 87.5% field-designator style (`.field = value`) — **4,954** field-designator instances —
  a high-frequency, everyday kernel idiom (`.ops = &foo_ops` struct-literal init) explicitly flagged as
  MAP-scope, not residue, and as the number to plan the aggregate-promotion thread against.

Recorded for completeness, no frontend change: GNU `__attribute__` short-hand macros (`__init`,
`__must_check`, etc.) appear 718 times across 151/392 files (38.5%) — ~180x the 4 literal
`__attribute__` occurrences — and one macro-with-arguments case (`__must_check`/`__printf(3,4)` between
a storage-class specifier and a declarator) produces a **silent misparse** that doesn't set
`has_error()`, understating true breakage; source of the attribute-macro-allowlist follow-up (§6). K&R
function definitions: 3/7,767 (0.04%), all traced to the same attribute-macro artifact — treated as
extinct.

## 3. Scale results

`wp-k2-report.txt` staged four completed scans plus one abort, walking fs/ext4+btrfs → fs/ → drivers/net/
→ drivers/ (full tree was never attempted). Corpus LOC was recomputed directly (`find -print0 | xargs -0
cat | wc -l`) after an early `wc -l | tail -1` undercount for drivers/net/ (brief estimated ~750k lines;
actual is ~7x that).

| stage | root | LOC | files | wall (cold/warm) | peak RSS (cold/warm) | units | groups |
|---|---|---|---|---|---|---|---|
| 1 | fs/ext4 + fs/btrfs (summed, 2 single-root scans) | 238,691 | 191 | 1.14s / 0.94s (ext4) + 4.18s / 3.72s (btrfs) | 0.168 / 0.144 GB (ext4), 0.413 / 0.387 GB (btrfs) | 5,017 | 675 |
| 2 | fs/ | 1,625,788 | 2,176 | 37.43s / 33.68s | 3.324 / 3.102 GB | 39,367 | 6,487 (+170 test, +1,686 api, +240 weak) |
| 3 | drivers/net/ | 5,221,920 | 6,129 | 210.34s / 202.40s | 10.206 / 9.562 GB | 118,616 | 28,740 (+86 test, +8,995 api, +1,685 weak) |
| 4 | drivers/ | 25,675,291 | 33,606 | **ABORTED** at 36.5s (cold only) | **17.170 GB — breached the 16 GB abort cap**, watchdog SIGKILLed at 17 GB | n/a — killed before any output | n/a |
| 5 | full tree | 36,730,009 | 63,417 | NOT ATTEMPTED (per abort discipline) | — | — | — |

Stage 1 was run as two separate single-root scans and summed arithmetically — reprise takes exactly one
root path, and neither symlink-merging (WalkBuilder doesn't follow symlinks) nor hardlink-merging
(`cp -al` failed cross-filesystem, `/tmp` is tmpfs) could produce a combined fs/ext4+fs/btrfs root. This
means stage 1 has no visibility into ext4↔btrfs cross-file pairs, an accepted gap since the headline
cross-driver question lives in stages 3–4 (single directories, no workaround needed).

**The stage-4 abort** happened in under 37 seconds, entirely inside parse/extract/index — before the
matching phases that dominated stage 3's wall time even started, nowhere near the 900s wall-clock
ceiling. Memory is the resource that breaks first, and it breaks in the *cheapest* phase. The working
ceiling this campaign establishes: reprise scans comfortably through ~5M LOC / ~10GB peak RSS (stage 3,
still nominally inside PLAN.md's linear-scaled 500k-LOC/60s target), and breaks outright by ~26M LOC
(stage 4) on memory, not wall-clock — the architecture holds every unit's normalized representation
resident for the whole scan, and that stops fitting this box's budget well before wall-clock would.

**Assemble-phase superlinearity.** `assemble` grows far faster than unit count between stage 2 and stage
3: units grew 3.0x (39,367 → 118,616) but `assemble` grew **24.7x** (3,869ms → 95,741ms) — by stage 3 it
was 47% of total wall time (95.7s of 202–210s), more than the actual near-dup matcher (`near`, 50,269ms).
`ambiguity_skips` also appears for the first time at stage 3 (1,360; zero at every smaller stage). No
single dominant clone family was observed (`groups_subsumed_subset` stayed at 0–0.9%, never exploding),
so this isn't one giant family choking the matcher — a genuine phase-level scaling anomaly, flagged as
evidence but not root-caused (needs in-process profiling, out of scope for black-box measurement).

Both anomalies are filed as follow-ups (§6): the memory wall and the assemble-phase superlinear cost.

## 4. Findings legitimacy

`wp-k2-report.txt` triaged 12 stratified groups each from stage 1 and stage 3 (24 total, test-section
groups excluded, seeded/reproducible sampling) by hand: **9 LEGIT-DUP / 14 BOILERPLATE / 1 ARTIFACT of
24.** Stage 1 (fs/ext4+btrfs): 4 LEGIT-DUP, 8 BOILERPLATE, 0 ARTIFACT. Stage 3 (drivers/net/): 5
LEGIT-DUP, 6 BOILERPLATE, 1 ARTIFACT.

Named cases from the triage:

- **sfc / sfc-siena (g2564, exact-normalized) — the headline finding.** The sampled function pair
  (`efx_mcdi_phy_check_fcntl`, in both `drivers/net/ethernet/sfc/mcdi_port_common.c` and
  `sfc/siena/mcdi_port_common.c`) is byte-identical modulo a `static` qualifier. A follow-up whole-file
  `diff -u` (1288 vs 1282 lines) found the two files **~95% textually identical (216 diff lines)**,
  mostly `efx_` → `efx_siena_` renames — a whole MCDI-port-logic file forked between the mainline and
  legacy-chip sfc drivers. reprise's sample caught one function pair from it; whether the full unsampled
  stage-3 report surfaces more from the same file was not checked exhaustively (deferred).
- **igb / igbvf (g9752, exact-region, LEGIT-DUP).** Near-identical TSO context-descriptor setup. Unlike
  ixgbe/ixgbevf (which already share common core files), igb and igbvf have zero shared source files — a
  real, non-trivial consolidation gap, not an intentional split.
- **e1000 / e1000e (g12047, exact-region, BOILERPLATE) — known intentional fork.** Real duplicated
  `e1000_clean_tx_ring` logic, but e1000e was deliberately forked from e1000 as a separate driver for
  newer chips, kept apart for stability/risk reasons; not an actionable merge target, honestly labeled
  rather than counted as a miss.
- **il3945 / il4965 (g2635, exact-region, LEGIT-DUP, common.c precedent).** Near-line-for-line-identical
  rate-control window/throughput calc between the two Intel legacy wifi drivers. Notably,
  `drivers/net/wireless/intel/iwlegacy/common.c` already exists to hold logic shared between exactly
  these two drivers — the sharing mechanism is already in place, this routine just wasn't moved there.
- A 4-way rbtree-comparator family (g23) was named as "the trap" in miniature: a real syntactic match
  forced by the `rb_find()` API shape, zero consolidation value — the same lesson
  `docs/universal-ir-trap.md` names at the tooling-design level, recurring here as one finding.

**Coccinelle ground truth** (`cocci-validate-report.txt`) validated reprise against 8 real,
maintainer-accepted kernel commits that Coccinelle scripts (or certified-equivalent manual dedup
patches) removed duplication for. Strongest result: **drm/v3d's CPU-job-validation consolidation was
rediscovered unprompted**, at default settings, no config changes — near-normalized tier (234 tokens, 2
members, **90% similarity**) plus 4 additional "substantial" pairs and ~17 exact-region sub-window pairs,
together spanning **all 6 sibling handler functions** the real commit consolidated — essentially the
whole fan-out the commit's diff collapsed into one helper, without being told which functions were
siblings.

The **complementarity boundary**: of the 8 cases, half (cases 1–3, plus case 8's standalone form) are
classic Coccinelle micro-idioms — PTR_ERR_OR_ZERO (~10 raw tokens), min/max ternary (~8 raw tokens),
kmemdup (~22 raw tokens) — sitting below any sane clone-detector floor by design; this is read as the
intended trade-off (a substantiality floor exists precisely to keep noise down), not a reprise defect.
Coccinelle's specialty is exactly this territory that structurally cannot clear a size floor.

Within the above-floor half, two independent, reproducible **matching gaps** were found — neither fixable
by lowering the floor (both persist at `min_unit_tokens=1`/`min_seq_tokens=1`):

- **Guard-inversion.** `x = kmalloc(...); if (!x) return; memcpy(x, ...)` never matches
  `kmemdup_noprof`'s `p = ...; if (p) memcpy(...); return p;` — the same construct under De Morgan
  negation and return-placement inversion. Reproduces identically across two unrelated commits/files
  (ds2490.c, smb2pdu.c), so it reads as a systematic canonicalization gap, not a one-off.
- **Switch-arm clustering.** Five near-textually-identical `case` arms in one `switch`, one function, one
  file (the smb kstrdup mount-option series) never pair with each other or with sibling arms even at
  floor=1 (42 candidate pairs touch the five line ranges; zero pair two of them together) — hypothesized
  (black-box, not source-confirmed) as region/internal-repeat candidate windowing not starting a window
  at a brace-less `case LABEL:` boundary.

## 5. Preprocessor investigations

`expansion-probe-report.txt` measured whether running the real C preprocessor over fs/ext4 (all 40 `.c`
files, `make fs/ext4/*.i ARCH=x86_64`, defconfig) and clone-detecting the expanded output would justify
D44's opt-in requires-build tier. **Full-TU expansion is REJECTED, with data:**

- **99.5% header drag.** The 40 files' ~62,200 raw source lines / 1,357 hand-written functions balloon to
  3.05M lines / 255,043 function definitions after expansion — of which only 1,314 (0.5%) are
  self-attributed to ext4 at all; **253,729 of 255,043 (99.5%)** are pulled in unchanged from headers.
  Origin-dedupe (linemarker-based) must remove essentially the whole expanded corpus just to reach parity
  with the unexpanded view.
- **Cross-macro-family false positives survive name-based dedupe.** Of 25,351 qualifying near-dup pairs
  within the macro-generated pool, only 24.1% (6,108) share a generating macro name (the "true reuse"
  case dedupe is meant to remove); the other **75.9% (19,243)** are *different* macros (e.g.
  `EXT4_FEATURE_COMPAT_FUNCS` vs `RO_COMPAT_FUNCS` vs `INCOMPAT_FUNCS` accessors) whose masked bodies
  converge to identical or near-identical token sequences (jaccard up to 1.000) purely because masking
  erases the one field-name difference that distinguishes them. Same-name dedupe alone would suppress
  only ~24% of this false-positive mass.
- Mixed-level clone signal was weak even where present: only 17 of 582 macro-generated functions (2.9%)
  crossed a 0.70 masked-Jaccard threshold against hand-written code, all borderline (0.70–0.76), with a
  many-to-one pattern (several different `trace_raw_output_*` functions converging on the same generic
  `ext4_dx_csum_verify`) reading as coincidental resemblance, not genuine copy-paste.

**Trigger-once materialization** (harvest a macro's `#define` text buildlessly, synthesize one invocation
with placeholder args, run plain `cpp -P` with no kernel config) was evaluated as the cheaper alternative:

- **Mechanics proven: 156/206 candidates (~76%) hermetic success** on an unbiased sweep of function-like
  macros harvested from fs/ext4's own `.c`/`.h` files (388 headers reached via depth≤2 BFS, 3,084 macro
  names harvested). It independently reproduced the same weak (0.70–0.76) signal class found by full
  expansion (`EXT4_FEATURE_COMPAT_FUNCS` accessors near-matching small hand-written getters/setters,
  106 qualifying pairs) — at ~6.5s of compute total, no kernel `.config`, no `make prepare`/objtool build.
- **Guardrail round 1 (`guardrail-report.txt`) — an honest 0/10.** 10 function-like macros (rounding
  idioms, ring-buffer pair, `container_of`, `GENMASK`) were blindly reimplemented from their doc comments
  alone (blindness compromised during candidate selection — disclosed), then scanned against their real
  hermetically-expanded macro bodies at default settings. Zero pairs caught. Two confirmed root causes:
  **9 of 10 macros were floor-blocked** on at least one side (reimplementations or expanded bodies under
  reprise's ~32–33-token unit floor); the 1 pair clearing the floor on both sides (`CIRC_SPACE_TO_END`)
  was rejected at the structural-divergence stage, isolated (via a controlled A/B/C variant test) to the
  macro's GNU-statement-expression (`return ({ ...; });`) outer shape vs. a natural reimplementation's
  sequential statements — not the operator/naming differences also varied. Zero false positives anywhere
  in the 20-unit corpus, so precision is intact; this is a recall gap at the accept bar for this class.
- **Round 2 is not built.** The statement-expression-vs-sequential-statement normalization that Test C
  showed would flip the one recoverable case is filed as a follow-up (§6), not implemented — the guardrail
  report frames it as an axis-1 "shared mechanism" candidate (a provably behavior-preserving syntactic
  desugaring, not a cross-language claim) but explicitly stops at naming it.
- **Prior-art note.** No report in this campaign identifies an existing automated tool that performs
  trigger-once macro materialization the way reprise's probe does; Coccinelle, the closest prior art
  surveyed here (§4), works by manual, per-macro SmPL scripts — one script per idiom, hand-written by a
  maintainer. Trigger-once's distinguishing property, if built, would be genericity (harvest-and-invoke
  any function-like macro) rather than novelty of the underlying idea.

## 6. Follow-up ledger

- **Switch lowering** — driving case: the fs_context 5-way kstrdup `switch`-arm sweep (§4) that never
  self-clusters even at floor=1; motivates moving switch/goto off Native (§2) for real.
- **Guard-inversion canonicalization** — confirm/build a branch-return normalization that unifies
  `if (!p) return; BODY` with `if (p) { BODY }; return p;` (§4); reproduces across two independent
  kmemdup commits.
- **Statement-expression normalization** — treat a GNU `({ stmt*; expr; })` used purely as a value
  producer as equivalent to an ordinary compound statement ending in `return expr;` (§5); Test C shows
  this is very plausibly the difference between 0/10 and ≥1/10 on the guardrail corpus.
- **Continue-semantics for/while gap (shared with Go)** — an open cross-cutting lowering gap, not specific
  to this campaign's corpus but relevant to any future C loop-lowering work.
- **Attribute-macro allowlist** — a narrow, deterministic token-level allowlist for well-known
  attribute-macro identifiers (`__init`, `__must_check`, etc.) in declaration-specifier position, to catch
  the silent-misparse mode `has_error()` doesn't see (§2).
- **Memory wall** — the architecture holds every unit's normalized representation resident for the whole
  scan; this is what breaks stage 4 (§3). No profiled bisection of exactly where the ceiling sits was run.
- **Assemble quadratic** — the 24.7x-for-3x-units `assemble`-phase anomaly (§3) is flagged as evidence, not
  root-caused; needs in-process profiling.
- **Config unknown-key rejection** — reprise's TOML config silently ignores unknown keys/tables (no
  `deny_unknown_fields`), which made `[thresholds]` keys discoverable only by feeding wrong-typed values
  and watching for a deserialize error (`cocci-validate-report.txt`); a real validation error on unknown
  keys would have made this discoverable directly.
