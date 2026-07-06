# reprise

**Code duplicate detection** for **Rust, Python, TypeScript/TSX, Go, and
Kotlin**. A *reprise* is a theme that returns in altered form — a near-duplicate.
reprise finds them across a codebase — renamed, restructured, or reimplemented —
explains each group as a shared template with marked divergences, and gates
duplicate drift at PR time.

## The problem

Duplicated code is where bugs breed: copies **drift** as a feature or fix lands
in one and not the others. Juergens et al. (ICSE 2009) found 52% of clones
change inconsistently, and roughly every second unintentional inconsistency is
a fault. The damaging cases are rarely literal copy-paste — they are **divergent
duplicates**: an existing helper reimplemented instead of called, with different
identifiers, a `while` where the original had a `for`, an extra guard clause.
Token-window tools miss most of that band.

reprise targets clone Types 1–3 plus the "reimplemented helper" slice of Type-4,
**within a single codebase**, at repo-scan and PR-check granularity.

That type vocabulary is standard in the clone-detection literature and is used
throughout this README, so in plain terms:

- **Type-1** — identical code modulo whitespace and comments.
- **Type-2** — identical modulo identifier names and literal values: a renamed copy.
- **Type-3** — a near-miss: statements added, removed, or reordered; a `while`
  where the original had a `for`; small structural edits on a shared skeleton.
- **Type-4** — same behavior, structurally unrelated code. Full Type-4 is out of
  scope by design; reprise goes after its most common practical slice — an
  existing helper **reimplemented** instead of called — via inlining and
  API-profile matching. It is
**report-only** — it never transforms your code — and it is built to run in CI: its
flagship finding, `inconsistent-update`, fires at exactly the moment a change
edits one copy of a known duplicate group but not the others.

**A common and acute use case: LLM coding agents.** Agents reimplement existing
helpers at scale — GitClear's 2025 analysis of 211M changed lines found an
**8× increase in duplicated code blocks**, with copy/pasted lines exceeding
refactored lines for the first time (and that study counted only near-literal
clones). reprise was motivated by exactly this workload and dogfoods it (see
"What reprise found in its own code"), but the detector is
workload-agnostic — the calibration catches below are all long-lived,
human-written codebases: ripgrep, flask, serde.

Full design and rationale: [`docs/PLAN.md`](docs/PLAN.md) (the authoritative spec).
Every deviation from it is logged in [`DECISIONS.md`](DECISIONS.md).

## Install

From source (Rust 2024 edition toolchain required):

```sh
git clone https://github.com/ctxloom/reprise && cd reprise
cargo build --release        # binary at target/release/reprise
just install                 # install onto your PATH (cargo install --path .)
just install-static          # fully static Linux binary (crt-static), then install
```

crates.io publish is **coming soon** — the crate name was verified free as of
2026-07-02 but nothing is published yet. Licensed under **BSD-3-Clause**.

## The commands

reprise runs with **zero configuration**. An optional `reprise.toml` at the scan
root tunes it — see [`docs/CONFIG.md`](docs/CONFIG.md).

The examples below run against a three-file demo project whose report helpers were
written by copy-paste-and-edit — the exact pattern reprise exists to catch:

```python
# src/report_users.py
def active_user_report(records):
    lines = []
    total = 0
    for record in records:
        if not record.enabled:
            continue
        total += 1
        lines.append(f"{record.name}: active")
    lines.append(f"total: {total}")
    return "\n".join(lines)
```

`src/report_orders.py` and `src/report_tasks.py` are the same shape with renamed
variables and a different attribute (`.pending`, `.open`).

### `reprise scan <path>` — full-repo scan

```
$ reprise scan .
reprise scan: 3 files, 3 units indexed (0 below floor, 0 parse-degraded, 0 suppressed), inline [0 variants, 0 scc units, 0 ambiguity skips], 0 api signatures, cache [3 hits, 0 misses], findings [near-normalized: 1] in 3ms

#1 [near-normalized] value 128 · 3 members · 64 tokens · similarity 98%
    ./src/report_orders.py:1-10  pending_order_report
    ./src/report_tasks.py:1-10  open_task_report
    ./src/report_users.py:1-10  active_user_report
    template:
      fn v0 (v1) {
        v2 list
        v3 0
        while BOOL {
          if not __has_next (v1) {
            break_statement
          }
          v4 __next (v1)
          if not v4 ⟨h1⟩ {
            continue_statement
          }
          v3 += 1
      …
```

The three functions land in **one** group. The `template:` block is the report's
primary evidence: it is the shared skeleton rendered as pseudo-source, with `v0…v4`
for consistently-renamed locals and `⟨h1⟩` marking the one place the three copies
diverge (the differing attribute). The `for` loops all normalized to the same
`while`/`break`/`continue` core, which is why the divergence is 2% and not more.

`--format json|sarif|cpd|jscpd` switches output; `--top N` caps the terminal list
(0 = all); `--verbose` adds the weak-similarity and full test/api sections **and
dumps each member's actual source** (line-numbered) under its file:line reference
— review findings without opening files. `check --verbose` does the same for
touched/untouched members.

### The baseline is a git ref — never a file

Adoption is zero-setup: `check` scans the base ref itself for comparison state
(cached transiently under `.reprise/`, keyed by commit), so pre-existing
duplication is exempt automatically and only *new or worsened* duplication
gates. There is no baseline file, no derived artifact in VCS — the codebase's
own history is the acceptance record.

For a **fixed** reference instead of a moving one — a persistent acceptance
point whose drift trend doesn't reset every PR — pin a git hash or tag:

```toml
# reprise.toml
[baseline]
ref = "v1.4.0"   # or a commit sha — check uses this when --base is omitted
```

`reprise check .` then measures against that ref until you deliberately move
the pin (an ordinary, reviewable one-line diff). Rolling gates just pass
`--base origin/main` explicitly, as CI does.

### Ignoring units and accepting drift

Two source pragmas put acceptance decisions where they belong — in reviewed
source, not a derived file. A pragma is any **comment containing the marker as a
substring**, so you can append a reason:

```python
# reprise:ignore — hand-tuned parallel impl, kept separate on purpose
def fast_path(records):
    ...
```

**Placement (both pragmas).** The marker is recognized on the unit's **first
line** (a trailing comment on the `def`/`fn`/… line) or on a **line above it**.
The upward scan skips single-line attributes and decorators — Rust `#[…]`,
Python/TypeScript `@decorator`, Kotlin `@Annotation` — so the pragma can sit
above them:

```rust
// reprise:ignore
#[inline]
fn shim(x: u32) -> u32 { ... }
```

(Multi-line attribute arguments between the pragma and the declaration are not
skipped — keep the pragma adjacent to the unit in that case.)

**`reprise:ignore` — drop the unit entirely.** The unit is removed from **every
tier**: it is never normalized, matched, or reported, and it cannot be a member
of anyone else's group either (it simply isn't in the corpus). Use it for code
that is duplicated *on purpose* and should never be flagged — a generated shim, a
deliberately parallel implementation.

**`reprise:accept-drift` — keep coverage, waive the gate.** The unit stays
**fully covered** — indexed, matched, and reported like any other. The pragma
changes only the `check` gate: when a change edits some members of a duplicate
group but not others (an `inconsistent-update`), the finding **reports as
information instead of failing CI** — but only when *every touched member* of the
group carries `accept-drift`. Touch a member that is **not** marked and the
finding fails as usual. It is the reviewed, source-located way to say "this copy
is allowed to diverge" (D41), and needs no separate accept file to drift out of
sync with the git-ref baseline.

Neither pragma hides silently: the **suppressed count** is printed in the scan
summary (`… 0 suppressed …`), so ignores can't accumulate unnoticed.

For coarser exclusions — whole paths, generated files, or test code — use
`reprise.toml` (`scan.exclude` globs, generated-file detection, `tests.mode`);
pre-existing duplication is exempted automatically by the git-ref baseline (the
two-scan `check`). See [`docs/CONFIG.md`](docs/CONFIG.md).

### `reprise check <path> --base <ref>` — the drift workflow (flagship)

This is what runs in CI on a PR — zero setup: it diffs against a git ref, scans
the base ref for comparison state (or uses a baseline file if one exists), and
only units the diff touched are gated. Its most valuable output is `inconsistent-update`: **you
edited one member of a known duplicate group and left the others behind.**

Walkthrough — baseline the demo, then add a guard clause to *one* of the three
report functions (`report_orders.py`), simulating a feature applied to one copy:

```
$ reprise check . --base HEAD
reprise check vs HEAD: 1 touched units of 3 indexed; baseline: 1 findings (1 matched, 0 exempt); 0 suppressed; cache [2 hits, 1 misses]; 3ms

#1 [inconsistent-update] FAIL · inconsistent-update · similarity 92%
    this change touches 1 of 3 members of a duplicate group; 2 member(s) were NOT updated:
    touched:   src/report_orders.py:1-12  pending_order_report
    UNTOUCHED: src/report_tasks.py:1-10  open_task_report
    UNTOUCHED: src/report_users.py:1-10  active_user_report
    drifting: divergence 0.016 at baseline → 0.077 now
    template:
      fn v0 (v1) {
        ...
      …

FAIL: 1 finding(s) at or above fail_on exact-normalized
```

reprise names the two functions a reviewer would otherwise miss, and reports the
**drift trend** (divergence rose from 0.016 to 0.077 since the baseline). Revert
the edit — or apply the guard to all three — and `check` exits 0.

Exit codes: **0** clean · **1** findings at or above `--fail-on` · **2** usage or
runtime error. `--fail-on <tier>` (or `[report] fail_on`) sets the gate; `none`
disables it.

## Tier taxonomy

Findings are labelled by tier, in descending confidence — this is also the CI-gate
order (`fail_on = <tier>` fails on that tier and every stronger one). `api-profile`
and `weak-similarity` **never** fail CI regardless of `fail_on`.

| Tier | Clone type | One-line meaning |
|---|---|---|
| `inconsistent-update` | drift | A diff touched some but not all members of a baselined group — the untouched copies may need the same change (`check` mode only). |
| `exact-normalized` | Type-1/2 | Whole functions identical after normalization (renames, literals, formatting, loop form all folded away). |
| `internal-repeat` | intra-function | One function contains ≥3 near-identical statement groups — extract a loop or helper. |
| `exact-region` | Type-2 sub-unit | An exact normalized run shared across units below whole-function scope. |
| `near-normalized` | Type-3 | Near-miss: members share an anti-unification template with a bounded number of factorable holes. |
| `inline-assisted` | Type-4 (helper) | Match found only after inlining a called helper — an existing helper reimplemented inline. |
| `api-profile` | Type-4 (suspicion) | Two functions call the same rare helpers in similar contexts — same task, likely reimplemented. Suspicion-only; never fails CI. |
| `weak-similarity` | — | Near-miss over the hole budget or with non-factorable holes; shown only with `--verbose`, never fails CI. |

## Supported languages

Rust, Python, TypeScript (including TSX), Go, and Kotlin. Matching is
**same-language only** — a Python unit is never compared against a Rust one — so
multi-language repos scan cleanly with each language partitioned independently.

**Planned:** Java, C#, C++, and C. A language is one `LanguageProfile` trait
implementation plus a set of probed grammar rules, so additions are incremental
— the five existing profiles are the templates.

## Output formats

| `--format` | Use |
|---|---|
| `terminal` (default) | Human-readable ranked report with templates. |
| `json` | The full report model, for tooling. |
| `sarif` | SARIF 2.1.0 for GitHub code-scanning and other consumers. |
| `cpd` | PMD/CPD-XML — drop into an existing CPD pipeline. |
| `jscpd` | jscpd-JSON — drop into an existing jscpd pipeline. |

The SARIF emitter makes two clone-specific choices (spec §6.1): a duplicate group
is **one result at N locations** (`relatedLocations[]`, with the template embedded
in the message), and `partialFingerprints` is set to the **structural/template
hash**, not a line hash. That means a code-scanning alert stays the *same* alert as
its members drift through renames and whitespace edits, instead of churning
closed-and-reopened on every cosmetic change. Set `[report] sarif_fingerprint =
"line"` to fall back to GitHub's line-hash default.

## Headline calibration numbers

Full measured evidence, per phase, is in [`CALIBRATION.md`](CALIBRATION.md).
Precision is **measured, not asserted** — but honestly, on small samples:

- **Mutation benchmark (recall).** Type-1/Type-2 classes: **100%** (16/16). Type-3
  classes (loop-swap, reorder, 3× unroll, subtree-sub, light-edit-every-statement),
  tail-recursion, and the inline/mutual-recursion chain: **100% per class** — but
  each class is only 2 variants (Rust + Python), a regression guard, not a large
  sample. Designed-to-fail controls (tree recursion, random pairs): **0%**
  convergence, as required.
- **Stratified precision sample (§7.2).** 50 findings across five real repos
  (ripgrep, flask, serde, click, gin), seeded-random (not top-value-biased),
  single labeler: **70% overall** (35/50). Per tier: `exact-normalized` 5/5,
  `near-normalized` 8/8, `exact-region` 17/27 (63%), `inline-assisted` 3/5,
  `internal-repeat` 2/5 (40%, since mitigated — D30). n=50, one labeler; treat as
  a floor.
- **Reason to exist (§7.3 vs jscpd).** Against jscpd on unseen repos, reprise makes
  **438 findings jscpd cannot on ripgrep, 149 on flask** — dominated by exactly the
  Type-2/3 (renamed exact runs, near-misses) and Type-4 (inline-assisted) catches
  jscpd's raw-token windowing can't reach. jscpd-only findings are all outside
  reprise's declared scope (comments, non-function code, sub-floor fragments).
- **Performance.** Warm `check --base` on a synthetic **500k-LOC** corpus: **8.1 s**
  (against a ≤10 s gate); real repos scan in well under a second warm.

## What reprise found in its own code

Every finding below is a real duplication that reprise's own author (an LLM
coding agent — fittingly, the use case that motivated the tool) introduced
*while building reprise*, caught by the tool during development. Each is hand-verified; together
they are the tool's most honest demo. DECISIONS.md records the full history.

**`synth_call` — the first-ever finding (Phase 1, `exact-normalized`).** The very
first self-scan reported exactly one group: the helper that synthesizes call
nodes had been written twice, minutes apart, in `src/lang/rust.rs` and
`src/lang/python.rs`. The two copies differed only in grammar kind-name strings
(`"call_expression"`/`"arguments"` vs `"call"`/`"argument_list"`) — string
literals, which normalization buckets, so the exact tier saw identical trees. A
textbook Type-2 clone, and exactly the "expression holes → parameters" recipe:
consolidated into `lang::synth_call` with the kind names as parameters.

**`synth_ident` — a triple (Phase 2, `exact-normalized`).** The identifier-leaf
builder existed three times: `ident` in the Python profile, and — under two
different names, `ident` and `ident_node`, in the *same file* — in the Rust
profile. Author didn't notice; exact tier did. Consolidated.

**`dump` — copy-paste caught within the hour (`exact-normalized`, 194 tokens).**
A CST-dumping dev tool was copy-pasted from `examples/probe.rs` into a second
probe binary during a debugging session. The next self-scan flagged the pair;
the second binary was merged back into the first. Elapsed time from paste to
detection: about an hour of wall-clock development.

**`render_template_block` (Phase 3, self-scan during delegated development).**
The agent building check-mode reporting re-implemented report.rs's
template-rendering block rather than calling it — precisely the "reimplemented
an existing helper" pattern from the problem statement — and its own self-scan
caught it mid-milestone. Consolidated into a shared `report::render_template_block`.

**The standing findings — accepted parallel implementations.** The self-scan
persistently reports the five language profiles (`src/lang/{rust,python,typescript,go,kotlin}.rs`)
as duplication: `lower_recursion` across profiles at 96% similarity
(`near-normalized`, 224 shared tokens), `reassign_stmts` as a 289-token
`exact-region`, upgraded by the inliner to `inline-assisted` once their shared
helpers (`child_field`, `is_raw_ident`, `synth_ident`) are expanded. These are
*correct findings about deliberate duplication*: the profiles are parallel
implementations of one trait, kept separate on purpose. They stay in the report,
and `check`'s two-scan mode exempts them automatically as pre-existing — which is
the intended adoption story for any legacy codebase.

**The drift gate red-teamed its own rollout (D39).** The first CI run of
two-scan mode failed on the commit that introduced it — eight
`inconsistent-update` findings, all "touching" the 450-line `scan()` function.
That was a genuine bug the gate had just exposed in itself: region members were
touch-mapped at enclosing-unit granularity, so any edit anywhere in a large
function "touched" every shared idiom-run inside it. After the fix, the *next*
CI run failed with exactly one finding — the now span-precise gate correctly
noticing that the fix itself had legitimately diverged one side of a code run
shared between `check.rs` and `lib.rs`. That second, *true* finding drove the
final policy: unit-copy drift gates hard; shared-run drift reports as
information. Two commits, two findings, one bug fixed and one design decision
made — by the tool, about the tool.

**One finding traded away, on the record.** The thin-delegation suppression
(wrappers whose whole body is a single call — the residue of the recommended
fix pattern) silenced a fixture the calibration sample had labeled a true
positive: ripgrep's `convert::usize`/`u64`, single-statement wrappers around an
already-extracted helper. Field judgment ("stop flagging the fix pattern") won;
the fixture stays in the wild corpus as a must-stay-silent negative, pinning
the suppression. Precision trades are recorded, not hidden (D37).

## How it works

Four stages, per language partition (details in [`docs/PLAN.md`](docs/PLAN.md)):

1. **Normalize.** Parse with tree-sitter (error-tolerant), strip comments, **lower
   every loop form — and tail recursion — to one minimal loop core**, abstract
   local identifiers positionally and literals to typed buckets, and canonicalize
   order-insensitive constructs. Source spans survive every rewrite, so findings
   always map back to real lines.
2. **Fingerprint.** A Merkle structural hash per unit (exact-match key), subtree-hash
   bags with MinHash (near-miss retrieval), and Shazam-style landmark-pair hashes
   (discriminating retrieval that degrades gracefully under local edits).
3. **Three matching tiers.** A **sequence tier** (generalized suffix array over the
   normalized token stream) finds exact duplicated runs; a **tree tier** retrieves
   near-miss candidates, verifies them with an offset-histogram diagonal check, and
   confirms with **anti-unification**; an **api-profile tier** flags same-task
   reimplementations by shared rare-callee profiles (suspicion-only).
4. **Anti-unification templates.** A near-miss finding is not a similarity
   percentage — it is the least-general generalization of the group: a shared
   template plus the holes where members diverge. The holes *are* the divergences,
   and factorable ones double as the refactoring recipe (expression holes →
   parameters, statement holes → closures).

## The musical part

A *reprise* is a theme that returns later in the piece changed — new key, new
instrumentation, same bones. That is exactly what a near-duplicate is. The name
is not the only thing here borrowed from music.

One of reprise's retrieval layers is lifted from **Shazam** (Wang, *An
Industrial-Strength Audio Search Algorithm*, ISMIR 2003). Shazam fingerprints a
track as a *constellation*: pick distinctive spectrogram peaks (landmarks), hash
them **in pairs with their time offset** — individually common peaks become
combinatorially rare pairs — then verify a match by checking that the surviving
hashes agree on a single time alignment (the diagonal). The design target was
hostile channels: phone microphones, bar noise, lossy codecs, primitive
streaming. The insight that makes it work is that damage is *local* — destroy
98% of the hashes and the surviving 2% still line up.

reprise runs the same play against a different adversary. Distinctive normalized
subtrees are the landmarks; pairs are hashed with their structural offset; a
candidate pair must produce a dominant bin in an offset histogram before the
expensive anti-unification verification runs. **Shazam was controlling for loss
— a signal damaged in transit. reprise is controlling for divergence — a copy
damaged by editing.** A rename here, a swapped loop there, an extra guard
clause: each edit destroys only the landmark pairs it touches, and the surviving
constellation still lines up on the diagonal. The adversary changed; the math
didn't.

This layer holds its place on evidence, not elegance: it went head-to-head
against a rival retrieval design in a measured rivalry, won on recall, and the
loser was deleted from the codebase ([`DECISIONS.md`](DECISIONS.md) D16). There
is a second, smaller borrowing from the same world: near-miss verification
aligns statements by *graded* similarity rather than demanding exact matches —
the cover-song lesson. After normalization, align with tolerance.

## Development

Everything runs through [`just`](https://github.com/casey/just):

```sh
just test             # cargo test --all-targets (127 tests)
just lint             # clippy -D warnings + cargo fmt --check
just bench-mutations  # the mutation-recall gate (spec §7.1)
just scan-self        # dogfood: scan reprise's own repo
```

**Dogfooding.** reprise scans itself. Its `reprise.toml` excludes `benches/**`
because that directory is a corpus of *planted* clones (mutation seeds/variants and
the wild-pair recall net) — including it would drown the self-scan in benchmark
duplication rather than the tool's own code. The self-scan legitimately reports the
**five parallel language profiles** (`src/lang/{rust,python,typescript,go,kotlin}.rs`)
as duplication: they are deliberate parallel implementations of one trait, and
`check`'s two-scan mode exempts them automatically as pre-existing (see
"What reprise found in its own code" above).

**`check` needs a commit.** `reprise check . --base HEAD` diffs against a git ref,
so the repo must have at least one commit. A freshly-initialized repo with no
commits has no `HEAD` — commit first, then `check` works.

## Repository map

- [`docs/PLAN.md`](docs/PLAN.md) — the authoritative design spec (Rev 9).
- [`docs/CONFIG.md`](docs/CONFIG.md) — every `reprise.toml` key, defaults, and drift from the spec.
- [`DECISIONS.md`](DECISIONS.md) — every deviation from the spec, with rationale.
- [`CALIBRATION.md`](CALIBRATION.md) — measured recall/precision/performance, per phase.
- [`CHANGELOG.md`](CHANGELOG.md) — release history.
