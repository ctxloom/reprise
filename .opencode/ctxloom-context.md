AI code makes ~2x more concurrency mistakes than human code (CodeRabbit 2025, 470 PRs). Never parallelize sequential awaits; mark load-bearing ordering with `// SYNC-REQUIRED: [reason]`.

---

AI code: correct happy path, dangerous elsewhere. LLMs hallucinate packages at 5.2-21.7% rates (arXiv:2406.10279) — verify imports (`npm info`/`pip show`/`cargo search`), API signatures, endpoints. Test integrity: AI "fixes" by deleting tests, removing assertions, mocking away behavior (mutation testing catches this). Red flags: unexplained deletions, catch-alls replacing specific handlers, removed validation, async/sync flips, unjustified deps. Reject immediately: security vulns in sensitive code, deleted tests, hallucinated deps, race conditions, missing error handling in critical paths.

---

# is-this-a-standard

# Is This a Standard?

Before writing infrastructure, ask:

> Is this a standard? Did we already wire it in?

## Why

LLMs default to local construction. They've seen `grpc.reflection.v1.ServerReflection` a thousand times, but write end-to-end over looking up.

## Common Categories AIs Reinvent

- gRPC Server Reflection (`grpc.reflection.v1.ServerReflection`)
- OpenTelemetry instrumentation (OTLP exporters)
- JWT validation (vetted library)
- OAuth 2.0 Device Authorization Grant (library)
- JSON Schema validation (metaschema)
- Kubernetes AdmissionReview (standard webhook shape)
- Conventional Commits parsing (existing parser)
- Healthchecks (`grpc.health.v1.Health`)

## What to Do

When AI proposes infrastructure:

1. Name the category
2. Ask if a standard exists
3. If yes, ask if project already wires it in
4. Use the standard

## Load-Bearing Comments

At registration site, name the invariant (see sibling `no-provenance-comments`):

```go
// reflection.Register: canonical "what does this server speak"
// mechanism. Do not add a parallel descriptor service; ask reflection.
reflection.Register(grpcServer)
```

Lives next to the call that triggers reinvention. No external doc dependency.

---

Mutation testing is the deterministic check on test quality; LLM tests look plausible while hiding tautological assertions, boundary gaps, and implementation coupling. After the TDD round-trip: run mutation testing, analyze survivors, add tests that kill them or accept the gap explicitly. Tools: cargo-mutants (Rust), pitest (Java), mutmut/cosmic-ray (Python), stryker (JS/TS), gomutation/go-mutesting (Go). Kill-rate targets: 80-90% pure functions and business logic, 60-70% framework glue, logging-only paths exempt. Coverage measures execution, mutation measures verification (80-90% coverage teams routinely see 30% kill rates). Never game the score with adjacent `assert!(true)`.

---

# no-provenance-comments

# No Provenance in Comments

Comments state invariants. Git records timeline.

## The Rule

No git-provenance metadata in code comments or codebase prose:

- Commit hashes (`see commit abc123`)
- Milestone tags (`wired in P0.2`)
- PR numbers (`added in #1234`)
- Release versions as historical markers (`introduced in 1.4.0`)
- Author/date stamps (`added by Alice 2024-01-15`)
- TODO sprint/iteration references

## Why

- Commit hashes break after rebase/squash
- Milestone tags become noise post-project
- Author/date stamps age into tombstones surviving author departure and line rewrites
- `git blame`/`git log` already record this; duplication guarantees divergence

## What to Write Instead

State the load-bearing invariant. If the comment survives a history rewrite, keep it. Else it's a timeline entry in the wrong file.

Before:
```go
// reflection.Register: wired in P0.2 (commit abc123). Canonical.
```

After:
```go
// reflection.Register: canonical "what does this server speak"
// mechanism. Do not add a parallel descriptor service.
```

## Where Provenance Lives

- `git blame`/`git log`
- Commit message body, PR description
- `CHANGELOG.md`/release notes
- ADRs
- Issue tracker

## Exceptions (Not Provenance)

- External stable IDs: RFC numbers, CVE IDs, language-spec versions
- Public-API `@since` annotations: version is part of API contract
- "We tried X and it failed": invariant is the rejected approach; dated artifact lives in rationale

---

# TDD for LLM-Written Code

LLMs non-deterministic; tests are deterministic gate.

## Rule
Demand TDD: tests first, implementation second. Non-negotiable.

## Workflow
1. Describe requirement
2. LLM writes tests against requirement (not implementation)
3. Review tests: requirement, edge cases, failure modes captured?
4. LLM implements to pass tests
5. Run tests

## Why
Test suite = contract. Without tests-first, model fills contract with its own output — no separate ground truth.

## Anti-Patterns
- Implementation-first, then tests → tests confirm bug, not requirement
- Vacuous assertions (`assert!(true)`)
- Implementation-coupled tests (`assert_eq!(hash("x"), 0x7a3f...)`) — brittle, blind to behavior
- Skipping step 3: test review is load-bearing

## What Tests Document
Document **problem**, not solution. Name requirement in test name; assert observable behavior.

## Verification
```
Show me the tests before the implementation. I will review the tests, approve them, then you write the implementation.
```

---

Never bake versions into AI context files; lockfiles are the source of truth (a static table ages out the moment the project moves). On session start read the lockfile and language-pin files; propose upgrades through the lockfile, never as one-offs in code. Verification prompt: "Your suggestion uses [pattern]. Project lockfile pins [package] at [version]. Confirm the pattern is supported at that version, or propose updating the lockfile."

---

# Warning Suppression
Do not suppress lint or compiler warnings without explicit user approval.  Warnings are signals of potential issues. Suppressing them without review risks hiding real problems.
Ensure that, when the user does approve a warning suppression, the comment includes the justification and details of the warning being suppressed. This creates a record for future reviewers to understand the context and reasoning behind the suppression.

---

# Mutation Testing

High coverage + low mutation kill rate = false confidence.

```bash
git worktree add --detach ../.mutants-worktree HEAD
cargo mutants -d ../.mutants-worktree --in-place --timeout 120 -f <file> -- --lib
git worktree remove ../.mutants-worktree --force
```

Worktree shares .git and copies only source; `--in-place` is safe because the worktree is disposable.

Kill-rate targets: pure utilities/validators 90%+; business logic/state machines 85%+; orchestration/coordinators 70%+; framework glue/adapters 50%+. Skip generated code (`*.pb.rs`, `src/proto/`), trivial delegation, framework boilerplate.

---

# TDD

Red-green-refactor is mandatory; verify the red test runs and fails for the right reason before implementing.

Integration/acceptance: build and run actual binaries, don't define hooks; tag slow tests; isolate and clean up after yourself.

Naming: `test_<action>_<condition>_<expected_result>` in the language's casing; readability over strict format. Order test files by complexity — usage-demonstrating examples at top, edge cases at bottom; tests are documentation.

---

# Test Coverage

Target 90%+ (unit + integration, full application). Acceptable gaps: main entrypoints (E2E-covered), generated code, impossible panic/fatal paths, default factory functions (exclude via tooling).

---

# Git

Branching: no branches or PRs unless explicitly asked; short descriptive names; one branch per task; branch off main/master only — push back and confirm anything else.

Commits: terse, describe code changes only, no meta-commentary; NEVER mention Claude, Anthropic, AI, or "Generated with".

Breaking changes: <1.0 and new major versions need no backwards compat — remove deprecated code immediately; post-1.0 minor/patch, discuss before implementing.

Pre-commit: lint, format, test before committing — never commit broken code; fix pre-commit errors automatically without asking. Bypass hooks (`--no-verify`) only for WIP on feature branches, with documented reasoning.

---

# Code Quality

Before writing: search the codebase for existing implementations; reuse or extend rather than duplicate or recreate.

Size: <500 lines per file (exceed only with very high coupling/cohesion); small single-purpose functions; optimize for reading, not performance; separate interfaces from implementations by file.

Naming: the interface is the thing — UserService is the Protocol, never IWhatever; implementations are named for how they implement: DefaultUserService (single), HttpUserService/CachedUserService (multiple).

Clean up: kill background processes when done; remove unused code, files, imports, variables; no dead code.

Comments explain why only. No change-tracking comments, no revision history in code, no commented-out code — git has it.

---

# Documentation Guidelines

Ask first before creating any *.md file, planning/strategy document, tracking file, or meta-documentation (exception: README updates when adding features). No progress notes in code ("refactored X to Y", changelog-style comments) and no change history in files ("Updated on...", "Previously this was..."); history belongs in version control and commit messages.

---

# String Handling

Never branch on string approximations of messages (startswith/contains/substring) unless explicitly instructed — use typed errors or error codes (`errors.Is(err, ErrConnectionRefused)`, not `strings.Contains(err.Error(), ...)`). Define all error messages as constants/sentinel errors and reuse them in test assertions — no magic strings.

---

# Decision Gating

Surface structural/interface decisions for sign-off before acting. The gate is a visible checkpoint — the user may approve or reject; what must not happen is the decision being made silently, buried in implementation.

## Gate these
- **Structural consolidation/splitting** — merging/splitting independent launchers, packages, modules; a shared abstraction across independent units. Changes *topology*, not just duplication.
- **Interface/seam changes** — public interface, trait, protocol, or extension seam. Ripples to every implementation; gate at least as hard as topology.
- **Operator-control trades** — trading the operator's control for author-side convenience (e.g. one polymorphic launcher vs discrete per-component launchers with full startup control).

## Don't gate
- DRY *within* a single unit (module, shared library).
- Local refactors that move no boundary and change no contract.

## Surface it
- **In plans:** a dedicated "Decisions for sign-off" section, each choice with 2–3 options. Never fold a topology/interface/control decision into a step bullet.
- **Mid-implementation:** a decision the approved plan didn't call out → stop and ask; don't pick and move on.
- **Default:** preserve existing separation and interface/seam contracts unless asked to change them. When unsure, treat it as a decision.

---

# Elegant Redo: Informed Reimplementation

Discard current impl and rebuild using lessons from first attempt.

## Philosophy
First impl teaches the problem. Second solves it well. Knowledge from edge cases, hidden requirements, and dead ends is the valuable output—not the code.

## Process
1. **Inventory knowledge** - Before deleting, document:
   - Hidden requirements, surprising edge cases
   - Architectural constraints revealed
   - Dependencies/interfaces to preserve
   - What worked vs what was over-engineered

2. **Identify elegant core** - With full problem knowledge:
   - Simplest abstraction covering all cases?
   - Natural data structures?
   - Where did first attempt fight the language/framework?
   - What's deletable vs essential?

3. **Scrap and rebuild** - Start clean:
   - No copy-paste from old impl
   - Let structure emerge from problem
   - Less code, fewer abstractions, simpler interfaces

4. **Validate** - New solution must:
   - Handle all discovered edge cases
   - Pass existing/improved tests
   - Be demonstrably simpler

## When to Apply
- Impl works but feels forced/overcomplicated
- Real problem shape doesn't match code
- Accumulated patches obscure design intent
- Developer explicitly requests fresh take

## Rules
- No sunk cost preservation—judge on current merit
- Simpler is better, but not at correctness cost
- Goal: minimum structure handling full problem naturally
- Keep tests (requirements); rewrite implementation

---

# Grill Me: Change Comprehension Gate

Quiz developer on their changes before PR creation to verify understanding.

## Process
1. Analyze diff (staged + unstaged)
2. Identify key decisions, trade-offs, edge cases, non-obvious implications
3. Ask pointed questions one at a time testing genuine understanding
4. Evaluate answers for real comprehension
5. Gate PR on sufficient understanding

## Question Categories
- **Intent**: What problem solved? Why this approach?
- **Impact**: What else affected? What breaks if this fails?
- **Edge cases**: Null/empty/concurrent behavior?
- **Trade-offs**: What sacrificed? Technical debt introduced?
- **Rollback**: How to revert safely? Blast radius?

## Grading
- **Pass**: Explains intent, impact, trade-offs clearly → proceed with PR
- **Partial**: Gaps exist → point out gaps, re-quiz weak areas
- **Fail**: Can't explain core decisions → no PR, suggest review areas

## Rules
- Rigorous but fair; test understanding, not memorization
- Scale difficulty to change complexity
- Explain wrong answers before continuing
- Never create PR until developer passes

---

# Prove It: Behavioural Diff Demonstration

Show concrete behaviour difference between main branch and current branch.

## Process
1. Identify behavioural claims - what should differ?
2. Design demonstrations exercising changed behaviour
3. Show before (main branch behaviour)
4. Show after (current branch behaviour)
5. Present side-by-side diff of inputs, outputs, effects

## Demonstration Methods
- Test output comparison across branches
- CLI invocation: same command, different output
- Code walkthrough: trace input through both paths, show divergence
- API calls: same request, different responses
- Error scenarios: differing failure modes

## Format

Per behavioural change:
```
## [Behaviour description]
### Main branch
Input: ... Output/Behaviour: ...
### This branch
Input: ... Output/Behaviour: ...
### What changed
[Concise diff explanation + why it matters]
```

## Rules
- Cover all intended changes, not just happy path
- Include edge cases and error conditions
- For pure refactors, demonstrate preserved behaviour
- Show evidence, don't just claim
- If local demo impossible, describe what would be tested and how

---

# Rust Test Support File Pattern

Keep test-only code out of production files: in foo.rs add `#[cfg(test)] #[path = "foo_test_support.rs"] pub(crate) mod test_support;` and put helpers in sibling `foo_test_support.rs`; tests.rs imports via `super::foo::test_support::...`. Visibility: `pub(super)` if only the parent module needs access, `pub(crate)` if sibling test modules do. Use when production code needs conditional test behavior, helpers exceed ~20 lines, or you want to cut context load reading production code; skip for tiny `#[cfg(test)]` helpers (<10 lines) or test code never called from production — plain `foo.test.rs` covers pure unit tests.

---

# just: Command Runner

Language-agnostic task runner. Define tasks (`just test`, `just lint`, `just build`) in a `justfile`.

## TOP: standard repo-root variable

Every justfile defines `TOP`:

```just
TOP := `git rev-parse --show-toplevel`
```

All paths relative to `TOP`. Non-negotiable. Hard-coded relative paths or `{{justfile_directory()}}` break when invoked from subdirs or composed by parent justfiles.

```just
TOP := `git rev-parse --show-toplevel`

build:
    cargo build --manifest-path {{TOP}}/Cargo.toml --release
```

## Local justfiles, composed at root

Place justfile next to code it manages. Compose via `mod`:

```just
# /justfile (root)
TOP := `git rev-parse --show-toplevel`

mod web   "{{TOP}}/web/justfile"
mod api   "{{TOP}}/api/justfile"
```

Each submodule defines own `TOP`, owns own recipes. Root: `just web build`. Inside `web/`: `just build`. DO NOT use monolithic root justfile.

## Recipe shape

- Used 3+ times: lift to target.
- Top of file: short comment (purpose, prerequisites, side effects).
- Args via `+ARGS` (preserved through delegation).

## Cross-platform

Prefer `[unix]` / `[windows]` attributes over parallel platform justfiles. Reserve parallel files (`platform_justfile` import) for differing recipe shapes.

```just
[unix]
clean:
    rm -rf {{TOP}}/target

[windows]
clean:
    Remove-Item -Recurse -Force {{TOP}}\target
```

## Anti-Patterns

- Hard-coded relative paths (`./src/...`): break under composition.
- `{{justfile_directory()}}` as `TOP` stand-in: scoped to local file, not composing parent.
- Monolithic root justfile.

For container-delegated recipes, see `just-container-overlay` fragment.

---

# just-container-overlay

# just: Container Overlay Pattern

Host justfile delegates to container; inside container, different justfile mounted over host's runs actual command. Same `just build` works both contexts. No duplicate target names, no `container-build` vs `build` split.

## Setup

```just
# Host /justfile
TOP := `git rev-parse --show-toplevel`

_run +ARGS:
    docker run --rm \
      -v {{TOP}}:/workspace \
      -v {{TOP}}/justfile.container:/workspace/justfile:ro \
      -w /workspace build-env:latest just {{ARGS}}

build: (_run "build")
test:  (_run "test")
```

```just
# /justfile.container
TOP := `git rev-parse --show-toplevel`

build:
    cargo build --manifest-path {{TOP}}/Cargo.toml --release

test:
    cargo test --manifest-path {{TOP}}/Cargo.toml
```

## Why it works

Load-bearing: `-v {{TOP}}/justfile.container:/workspace/justfile:ro`. Bind mount obscures host `justfile` with `justfile.container` inside container. Inside: `just build` → container recipe (cargo). On host: `just build` → delegation spawning container.

## Separation

- Host file: orchestrates container, never builds.
- Container file: builds, never knows about container.
- Build changes → container file only.
- Orchestration changes → host file only.

## Real-world

Angzarr uses this across coordinators (aggregate, saga, projector, process-manager, stream, log, grpc-gateway). Each has local `justfile` + `justfile.container`. Root composes via `mod`; `just aggregate build` from root and `just build` from coordinator dir or container all produce same result.

## Anti-Patterns

- `_run` running on host instead of container: defeats overlay.
- Container justfile re-invoking `docker run`: accidental docker-in-docker.
- Mounting container justfile read-write instead of `:ro`: container build edits host file.
- Arg-escaping ceremony: just's `+ARGS` preserves multi-word args through delegation without `$$`-escaping. Use it.

---

Dev Containers: `.devcontainer` config for consistent dev env w/ tools, deps, system reqs. Reproducible builds.

---

Use bacon as the continuous checker; version-control project bacon.toml (`bacon --init`). House config: default_job "clippy"; jobs.clippy-all = `cargo clippy --all-targets -- -D warnings` (need_stdout=false); jobs.check-win = `cargo check --target x86_64-pc-windows-gnu`; jobs.check-examples = `cargo check --examples` with watch=["examples"].

---

Rust: tracing for logging; cucumber-rs Gherkin acceptance tests. Layout: unit inline `#[cfg(test)]`, integration `tests/*.rs`, acceptance `tests/acceptance/features/*.feature`. Message constants: logmsg.rs consts used as `info!(message = logmsg::USER_CREATED, field = %v)`; errmsg module consts in thiserror `#[error("{}", errmsg::X)]`. IoC via generics plus a default alias: `pub type DefaultUserService = UserService<SqlUserRepository, SmtpEmailService>`.

---

# ltk

**llm-tool-killer (ltk)** — pre-tool hook that inspects and redirects shell commands per `.ltk/config.yaml` rules.

## How it works

Parses real command (resolving variables, unwrapping wrappers) → matches against project rules → first matching `deny` returns `message`/`suggest`:

    go test ./...   →   blocked: "Run tests through the task runner."
                    →   retry: `just test`

## How to use

- Treat redirects as guidance; read suggestion and retry as specified
- Prefer project task runner (`just <target>`) over invoking tools directly  
- **Agents do not cut releases** — ltk blocks `git tag`/release commands; prepare version bump/PR for human or CI

## What it is not

Cooperative redirect, not a sandbox. Explicit workarounds are possible; for strict boundaries use a container.

---

# Deferral Tracking

Deferred work lives in taskloom, not in conversation memory or plan prose.

## Search before you create

Before `task_add`, search existing log using `task_list` (term filter) or `tag_query` on distinctive noun: file, symbol, command, subsystem.

If entry covers the work, UPDATE it (`task_edit`, append dated note). Create only when nothing matches.

**Critical:** Duplicates generate contradictory fixes; whoever picks up later cannot tell which is current.

Check for existing entry even when confident finding is new.

## Agent deferrals

Record deferred work with `task_add` before moving on (descoped, mid-implementation, postponed):

- Concrete revive condition ("after X merges", "when CI green") → "Deferred" status with condition as trigger
- No condition → "To Do"

Don't close work spawning deferrals until each is recorded.

## User deferrals

Offer brief taskloom entry when user defers. Create on confirmation. Don't push if declined.

## Durability

Chat/plan deferrals disappear when session ends. Taskloom is durable record; summaries reference by harp ID.

---

# reprise

This project runs **reprise**, duplicate detection built for
LLM-generated code (a *reprise* is a theme that returns in altered
form — a near-duplicate). LLM assistants systematically
reimplement existing helpers instead of calling them, and the
copies then drift apart; reprise catches clone Types 1-3 plus the
reimplemented-helper slice of Type-4, within this codebase. It is
REPORT-ONLY: it never edits code — responding to findings is your
job.

## The two commands

- `reprise scan` — full-repo ranked report of duplicate groups.
- `reprise check` — PR/commit mode: findings involving units
  changed since a git ref fail when they are new or worsened.
  The base ref defaults to the merge-base with the default
  branch; override with `--base <ref>` or `[baseline].ref` in
  `reprise.toml`. Its flagship finding is `inconsistent-update`:
  a change edited ONE copy of a known duplicate group but not the
  others. Exit codes: 0 clean, 1 findings at/above `--fail-on`,
  2 usage/runtime error.

There is no `baseline` subcommand: the baseline is a pinned git
ref, not a stored file. Adopt reprise on a legacy codebase by
pinning `[baseline].ref` to the current commit, so only NEW drift
fails while existing duplication is grandfathered.

## How to respond to findings

- **Before writing a helper**, search for an existing one and
  call it — that is the failure mode reprise exists to catch.
- **`inconsistent-update`**: the fix you just made belongs to
  every copy in the group. Prefer extracting the shared helper
  and calling it everywhere; at minimum, apply the change to all
  copies.
- **A new duplicate group**: replace your new implementation with
  a call to the existing unit (or extract one shared helper).
- **Intentional parallelism**: mark deliberate duplication at the
  source — `reprise:accept-drift` waives the drift gate while
  keeping the unit tracked; `reprise:ignore` drops the unit
  entirely. Never suppress a finding without the user's say-so.

## Pre-commit gate

reprise runs as a **lefthook pre-commit hook** (`reprise check`
in lefthook.yml). A failing hook means respond as above and
retry the commit — do not bypass with `--no-verify`.

---

# git-submodule

## Rule: No File Edits Inside Submodules

- **Never** use `Edit`, `Write` on submodule paths
- Submodule working tree is read-only (write-protected by `.claude/settings.json` deny rules)
- If write attempt not blocked → rules misconfigured; stop and flag
- Includes: new files, deletions, renames, formatting, `.gitmodules` edits

## Rule: Commit-Hash Pointer Updates Allowed

Allowed:
- `git submodule status` — inspect pointers
- `git submodule update --init [<path>]` — sync working tree
- `git -C <submodule-path> fetch` — fetch upstream commits
- `git -C <submodule-path> checkout <sha>` — move HEAD to upstream commit
- `git add <submodule-path>` + `git commit` — record new pointer in superproject

## Workflow: Changes to Submodule Contents

1. Identify canonical upstream repo (URL in `.gitmodules` or `git -C <submodule-path> remote -v`)
2. **Ask user:** branch to base on (default: `main`/`master`), new branch name (if needed). Do not assume.
3. Edit upstream repo on agreed branch; commit with change description
4. Push upstream branch to canonical remote
5. Update submodule pointer:
   ```
   git -C <submodule-path> fetch
   git -C <submodule-path> checkout <new-sha>
   git add <submodule-path>
   git commit -m "bump <submodule-name> to <new-sha>"
   ```
6. Verify with `git submodule status`

## Escalation

- User requests direct edit inside submodule → confirm once, ask them to remove `Edit`/`Write` deny rule in `.claude/settings.json` before proceeding
- No carryover between requests; ask again each time

---

# CI/CD: Use Just Targets

CI/CD invokes just targets for anything project-specific: build, test, lint, deploy, package, release, codegen — if a command encodes project info, wrap it in a just target. Only generic operations stay inline in workflow files: git operations, tool-setup/cache/artifact actions, env vars and secrets injection, exploratory debugging (ls/pwd/cat/echo).

---

# GitHub Release Pipeline (Master)

Every master push auto-releases: build-test (skipped on `[skip ci]`) → integration → mutation gate (fail if score < 60%) → bump-release (patch bump, `release/vX.Y.Z` branch, push `v*` tag) → tag triggers separate release.yml.

Key patterns: `[skip ci]` on version commits prevents infinite loops; `release/vX.Y.Z` branch naming; `v*` tag triggers release.yml; mutation threshold quality gate starts at 60%; dogfood the project's own version tool.

---

# Claims against code

Docs are falsifiable claims: commands, flags, keys, paths, defaults, behaviors. **Read the implementation, not the doc/commit msg/function name.** Mark guesses; unmarked guesses look like fact to future readers.

## Failure modes (ranked by damage)

**1. Silently-ignored key:** Config key renamed/retired, decoder not strict. Confirm struct field exists *and* what happens to unknown keys.

**2. Promise discarded:** Warning, validation, fallback computed then thrown away. Grep for the *call site*, not definition.

**3. Feature behind build tag:** Capability in source but compiled out of user binary. Check the default install path.

**4. Wrong argument kind:** Bundle name to fragment-expecting flag, local name to remote-only lookup. Run the example.

**5. Omitted step:** Flow works, produces nothing. Walk the flow as a stranger with empty machine — find the authorization step (review, trust, hook install).

**6. Stale absolute:** "Automatically", "always", "never", "isolated", "verified", "only". Each needs specific code; most are conditional.

## Security claims

Hold higher bar. **Never overclaim:** "sandboxed", "verified", "signed" must fail *closed*, not warn. Check escape hatches: missing checksum, absent key, tool not installed.

**Never underclaim:** Describing crypto guarantee as weaker teaches readers to distrust protective mechanisms.

## Generated pages

- Never hand-edit; next regeneration reverts you.
- Authoritative for surface only. If schema drifted from struct, fix the source, not prose.

## Aspirational tense

Never present-tense unshipped features. "Will" doesn't fix it — skimming readers see it as shipped.

---

# Lead with why

Every hand-written docs page: pain and payoff first, mechanism last. A reader stopping after paragraph one should know if it's for them.

## The order

**Impactful → useful → detail.**

1. **Pain.** What fails without this—name concretely (failing command, lost work, wasted tokens). Not abstractions.
2. **Payoff.** One sentence a reader can repeat to colleagues.
3. **Shape.** How it works (diagram or three-step flow).
4. **Detail.** Flags, keys, schemas, edge cases—reference material readers skip after getting what they came for.

## What this rules out

- Opening with definitions, taxonomy, or "Concepts" preamble
- Opening with installation or syntax
- Burying motivating examples below reference tables
- "Why?" section placed after "How?" — belongs at top

## Exempt

Generated reference (CLI pages, schema dumps, tool listings)—indexed and grepped, not read start-to-finish. Never hand-edit.

---

# Log decisions as you make them

Keep a `decisions.md` in the CURRENT SESSION DIRECTORY — the harp
directory that also holds this session's `*.plan.md` files
(`~/.ctxloom/sessions/<harp>/decisions.md`). Append to it the moment
a decision is settled, not at the end of the session: a session that
ends abruptly loses everything not yet written, and end-of-session
summarizing is exactly when reasoning gets compressed into
uselessness.

## What counts as a decision

Log it when a question had more than one defensible answer and one
was chosen:

- A fork the user resolved (which approach, which library, which
  scope).
- A trade-off accepted deliberately — including one you argued
  against and the user overrode.
- A constraint discovered that removed an option ("cannot do X,
  because Y").
- A default deliberately NOT taken.
- Something ruled out of scope. (Also file it in taskloom if it is
  work someone should pick up later; the two are not redundant —
  the task says what to do, the decision says why it was parked.)

Do NOT log routine execution: which file you edited, that a test
passed, that you ran a command. If reversing it would cost nothing
and surprise no one, it is not a decision.

## Format

Append an entry per decision. Keep each one standalone-complete —
assume the reader has none of this session's context:

```markdown
## <short imperative title>
- **Date:** <YYYY-MM-DD>
- **Decided by:** user | agent
- **Chosen:** <option name>

### Options considered
<one block per option, INCLUDING the chosen one>

**<option name>** — chosen | rejected
- **For:** the genuine case in its favour.
- **Against:** its real costs and risks.
- **Evidence:** what this rested on (`file:line`, a measurement, a
  doc, or "assumed, not verified").

### Rationale
Why the chosen option beat the others — the specific consideration
that decided it, not a restatement of its merits.

### Revisit if
The condition that should reopen this. Omit only when genuinely
permanent.
```

## Every option gets a real case, not just the winner

Write the FOR and AGAINST for each option, the rejected ones
included. This is the part that makes the log worth keeping, and
the part that decays first.

- A rejected option recorded only as "rejected because slow" is
  unusable later: the reader cannot tell whether it was a weak idea
  or a good idea beaten by a constraint that has since gone away.
  Most reopened decisions reopen because a constraint moved, and you
  can only notice that if the constraint was written down against
  the option it killed.
- State each rejected option's case at its STRONGEST — the version
  its advocate would recognise. An entry that strawmans the losers
  makes the winner look inevitable and hides the fact that it was a
  close call. If it WAS close, say so.
- Give the chosen option an **Against** too. Every real choice has
  costs; an option with no downsides recorded is a sign the entry
  was written to justify rather than to inform.
- If an option was dismissed without serious evaluation, record that
  honestly ("not evaluated — no time") rather than inventing a
  rationale. That is itself useful signal later.

Record who decided when it matters — a decision the user made
directly is not the same as one you inferred, and a later session
must not silently overturn the former.

## Honesty rules

- Log the decision that was ACTUALLY made, including the expedient
  one. A decision log that only contains defensible choices is
  worthless, because the entries you would most want later are the
  ones under time pressure.
- When a decision is reversed, append a NEW entry that references
  the old one. Never edit or delete the original — the fact that it
  was believed, and then stopped being true, is the useful part.
- If a decision rests on something you have not verified, say so in
  the entry ("assumed, not measured").

---

# Repository and Worktree Layout

**Primary checkout** — leaf must be project name:
```
~/workspace/<project>
```

**Worktrees** — flat structure outside every repo:
```
~/workspace/worktrees/<project>--<branch-slug>    # feature/auth → feature-auth
```

Leaf directory carries both project + branch; visible in tooling. Slugify `/` → `-` for directory names only.

**Common Mistakes:**
- Primary checkout named `main` — must name project
- Worktree inside or beside repo — place in root only
- Reusing removed worktree directory — run `git worktree prune` first

---

# Worktree Lifecycle

**One worktree = one branch = one merge.** Agents in one work unit take turns in the same worktree or return read-only patches.

Every agent plan gets a worktree.

## Commit always

**Loss prevented only by committing.**

- Commit at every checkpoint (WIP, red)
- `--no-verify` OK on work-unit branches
- Merge is quality gate; clean history on entry
- Dirty tree: `remove` refuses without `--force`
- Uncommitted work in deleted worktrees is lost forever

## Done = merged + removed + deleted

Done ≠ "merged"; done = **merged, worktree removed, branch deleted.**

Worktrees ARE the ledger of open work. Stop at merge → accumulation.

Verify integration against actual branch (often not `main`). Use `git cherry <integration-branch> <branch>` (−-prefix = already upstream).

## Never force, never adopt

- **Never `git worktree remove --force` or `git branch -D`** — destroys uncommitted work
- **Never remove worktrees you didn't create**
- **Reaping = TRIAGE:** merged & clean → remove; dirty/unmerged → report human
- **`.git` is SHARED** — `branch -D`, `remove`, `gc`, `reflog expire` hit whole repo

## Worktrees from harnesses

Typical issues:

- **Stale base:** branches from ancestor/`origin/HEAD` (missing unpushed commits). Pin base SHA; verify `git log -1`.
- **Placement:** `/tmp` or scratchpad (wiped without warning). Keep only as commits.
- **Auto-clean:** directory vanishes; branch survives. Must commit.
- **`git worktree list --porcelain`** = ground truth.

## Recovering deleted worktrees

Order: `git worktree list` → branch ref → `git reflog` → `git fsck --lost-found`.

Only uncommitted work is lost.

---

# Code Walkthrough Pattern

## Purpose
Interactive review: AI presents small chunks, human approves/requests changes before proceeding.

## Flow

1. **Present one function at a time**
   - Show actual code w/ file path + line numbers
   - Keep chunks small (one function)

2. **Explain non-obvious aspects**
   - What it does, dependencies, side effects
   - Key design decisions

3. **Identify issues proactively**
   - Question unusual/odd patterns (see: questioning-patterns)
   - Note design concerns, suggest alternatives

4. **Wait for human input**
   - Don't proceed until human says continue
   - Make changes before moving to next chunk

## Interaction
```
AI: [function + explanation + questions]
    "Changes, or continue?"
H:  [feedback / changes / continue]
AI: [apply changes if requested, then next function]
```

## When to Use
- Codebase onboarding
- Code review requiring per-change approval
- Incremental refactoring
- Teaching/learning

## Principles
- Small chunks → focused review
- Human controls pace
- Immediate fixes before proceeding
- Curiosity over passive explanation (see: questioning-patterns)

---

# Status Document Pattern

Track multi-session code-review progress in a gitignored status doc with sections: Approach (what/why); Flow Map (ASCII diagram); Files to Review (table: File | Status | Notes); Session Log (per-session Reviewed/Next checklists); Questions/Notes; Summary.

---

# Working Context Directory

Keep a gitignored `_ctx/` directory for ephemeral LLM working files: plans/ (impl plans, decision drafts), checklists/, status/ (multi-session task state, review progress), research/, scratch/, prompts/, summaries/ (module overviews). Lifecycle: create status/plan at session start; update with completed items, decisions, blockers during; periodically delete stale files (>2 weeks) and archive keepers to real docs/ADRs. Start sessions by reading `_ctx/status/`; end by updating status with next steps.

---

CLAUDE.md: 100-200 lines max, overflow to per-folder files. Include: tech stack with versions, architecture (folder purposes), build/test/lint/deploy commands, project-specific rules AI would otherwise violate. Exclude: language syntax, linter-enforced patterns, anything Claude gets right unprompted. Test each line: would Claude err without it? No → delete. When Claude errs, add the correction immediately.

---

## Isolation: specify both axes

Creating, configuring, or delegating to a ctxloom agent (`ctxloom agent
set`, `run --agent`, `agent_run`)? Set both axes explicitly — never rely
on the default:

- **runtime** (`host`|`container`, the agent binding's `runtime:`)
  isolates the PROCESS.
- **workspace** (`none`|`worktree`, per-invocation `--workspace` /
  `agent_run`'s `workspace`) isolates the FILES.

They're independent: `container` can still mount the workspace at the
SAME absolute path as the live project (process isolated, edits still
land where the editor already looks); `worktree` still runs the engine
on the host (the editor goes blind to that tree by design — results
return via the delegated-agent merge flow, not live edits). Picking one
says nothing about the other.

Unspecified means `host`+`none` — isolated on NEITHER axis. That's a
default, not a decision.

Containers make isolation a property of the runtime, not a request to
the engine: some vendor CLIs ignore env-var isolation hints and write
credentials/state to a global path regardless.

A bad or missing agent name silently degrades to `host`+`none` with only
a stderr warning, discarding the runtime and permissions you asked for —
confirm the name resolves before trusting the isolation you requested.

---

# llm-tool-killer (ltk)

This project may run **ltk**, a pre-tool hook that inspects each shell
command before it executes and redirects it when a rule matches. Where
ctxloom shapes the context you see, ltk guides the commands you run.

## What it does

ltk parses the real command (resolving variables, unwrapping trivial
wrappers and sub-shells) and matches it against the project's rules in
`.ltk/config.yaml`. The first matching `deny` wins and returns a
`message`/`suggest` telling you what to run instead. Example:

    go test ./...   ->   blocked: "Run tests through the task runner."
                    ->   retry with `just test`

## How to work with it

- Treat a redirect as guidance, not a failure: read the suggestion and
  retry the command the way the rule asks.
- Prefer the project's task runner (e.g. `just <target>`) over invoking
  build/test/lint tools directly.
- **Agents do not cut releases.** ltk blocks `git tag` and release
  commands. Prepare the version bump and PR; a human (or CI) cuts the tag.

## What it is not

ltk is a cooperative redirect, not a sandbox. If explicitly instructed
to work around a rule the agent can, so it makes the easy, accidental
path the right one rather than enforcing hard isolation. For strict
"never" boundaries, run the agent in a container.

---

# taskloom

Persistent task tracking. Tasks live in a per-project append-only log
(~/.ctxloom/tasks/<project-id>.jsonl) and are keyed by harp IDs
(e.g. `swift-amber-falcon`). Statuses: `In Progress`, `To Do`,
`Deferred`, `Done`, `Archived`.

## MCP tools (served by `taskloom mcp`)

- `task_list({statuses?, term?, include_completed?, include_summary?})`
  — list/filter tasks. Set `include_summary: true` to also get
  per-status counts plus the in-progress harp IDs.
- `task_add({text, status?, trigger?})` — add a task with a fresh
  harp ID. Default status is `"To Do"`; `"Deferred"` requires a
  `trigger` (the condition that should revive it).
- `task_set_status({harp_id, status, trigger?})` — move a task
  between statuses.
- `task_edit({harp_id, text})` — replace a task's text in place.

Tasks are created and updated only through these tools (or the
`taskloom` CLI). The harp ID appears in `task_list` output so you can
reference a specific task in later calls.

Write `text` subject-first: the first line is what the task IS
(~80 characters or fewer — list views show only that line,
truncated there). Put provenance like dates or session names on a
later line, not the first.

## Check the log before you start, and again before you finish

**Before starting work**, look for open tasks that touch what you are
about to change. One may already hold the root cause, a decision
someone already made, or a constraint you would otherwise rediscover
the hard way — and it may show that someone else is mid-flight in the
same files. Search by AREA, not just by title, since a task about your
code may be named for its symptom:

    taskloom list --term <symbol, path, or error string>
    taskloom list --tag-query <area>

**Before finishing**, scan again for tasks in the same area. If your
change satisfies one, say so and offer to close it — quote what the
task asked for and what you actually did, so the reader can judge
rather than take your word. If it satisfies a task only in part, edit
the task to record what is now done and what remains, instead of
leaving it whole and letting the next person redo the finished half.

Both halves matter for the same reason: a task nobody rereads gets
solved twice, and a task silently satisfied but left open is
indistinguishable from work never done.

## Plan stamping

When you edit a plan file (`CURRENT_PLAN.md`, `*-plan.md`,
`docs/*-plan.md`), the active session's harp name is auto-stamped
into the file's YAML frontmatter `sessions:` list. Plans and
sessions cross-reference without a separate database.
