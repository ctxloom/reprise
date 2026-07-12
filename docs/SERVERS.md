---
title: LSP & MCP server surfaces for reprise
status: authoritative design; D-SRV-1 signed off (D46); M1–M2 done, M3 diagnostics slice landed
sessions:
  - fatal-main-storm
related:
  - docs/PLAN.md §non-goals ("a daemon/server/IDE plugin — CLI + CI only" — reversed by D46)
  - docs/SIMILARITY-IR.md (house style; the sign-off-gated design doc this mirrors)
  - DECISIONS.md D46 (servers promoted in-scope), D19 (per-file cache), D40 (git-ref baseline)
---

# LSP & MCP server surfaces for reprise

> **Reference convention:** bare **§X** = a section of *this* document; **spec §X** =
> docs/PLAN.md; **D-SRV-N** = a sign-off decision in this doc; bare **DN** = a
> DECISIONS.md entry. File:line refs are to the tree at design time.

## 1. What this builds, and the one framing that governs it

Two consumers, two surfaces, **one substrate**:

- **MCP** — the *agent* surface. reprise's founding thesis is LLM agents reimplementing
  helpers at scale (README, GitClear 8×). MCP hands the agent reprise as a tool it can
  consult mid-write. **Best protocol fit**: MCP is request/response, matching reprise's
  batch shape with zero impedance, and the full JSON model survives intact because an LLM
  reads structured content directly.
- **LSP** — the *human-in-editor* surface. Broad multi-editor reach, but a **lossy
  projection**: reprise's model is group-primary (one finding = N locations across N files)
  and template-centric; LSP is document-primary and squiggle-centric. There is room in the
  spec (§4), but the group→N-diagnostics projection loses "one finding" identity, and the
  standout value is not the static scan — it's moving `inconsistent-update` upstream to
  authoring time (§4.4, Milestone 4).
- **Substrate** — the existing serde report model (`ScanReport`/`CheckReport`), already
  full-fidelity and format-agnostic. **Neither server is the source of truth; both are thin
  projections over the lib.** Non-negotiable and already true today.

## 2. Grounding — what the codebase already gives us (verified)

**Already a library + thin main (near-zero core change for v1).**
- `reprise::scan(root: &Path, cfg: &Config) -> anyhow::Result<ScanReport>` (`src/lib.rs:38`)
  and `reprise::check::run(root, cfg, base, fail_on) -> anyhow::Result<CheckReport>`
  (`src/check.rs:314`) are pure compute-and-return: **no `println!`, no `process::exit`**
  inside the lib. Reports are owned + `Serialize`.
- `Config` is programmatically constructible — `Default` + all-public fields
  (`src/config.rs:7`); `cache.enabled`, `cache.shared_root`, `thresholds.*`,
  `baseline.pinned` are the server knobs.
- No global mutable state (only the `#[global_allocator]` — jemalloc on every target but Windows); concurrent scans are safe.

**One report model, all formats project from it.**
- `Group { id, tier, fingerprint, token_count, value, divergence, template: Option<String>,
  inline_chain, members: Vec<Member> }` (`src/report.rs:98`); `Member { file, lang, name,
  line_span: (u32,u32), parse_degraded }` (`src/report.rs:89`); `Tier` (8 variants,
  `src/report.rs:10`); `CheckFinding { kind, fails, group, touched, untouched,
  trend: Option<(f64,f64)> }` (`src/check.rs:149`).
- `similarity = 1 - divergence` (derived at render). `template` is a **rendered String**
  (holes `⟨h1⟩`, locals `v0..vN`).
- **The SARIF emitter is the LSP blueprint** (`src/formats/sarif.rs`): one result per
  `Group`, primary member in `locations[0]`, rest in `relatedLocations`, `Group.fingerprint`
  → `partialFingerprints`. That is exactly the LSP diagnostic + `relatedInformation` +
  stable-identity projection (§4.2).

**`find_similar` is mostly plumbing, not new algorithms.**
- In-memory single-unit extraction already exists: `unit::units_from_source(src, lang, cfg)
  -> Vec<Unit>` (`src/unit.rs:262`) — parses a snippet with a fake path (`"memory.in"`), no
  filesystem. A `Unit` carries `fingerprint: u128` + `tree: NormNode`.
- Pairwise comparator is ready-made: `au::anti_unify(a, b, profile) -> AuOutcome`
  (`src/au.rs:76`) → `divergence`, `holes`, `template`. Exact tier is a fingerprint map
  lookup (`group::build_exact_groups`, `src/group.rs:13`).
- Missing conveniences: no `&str -> Lang` (only `Lang::from_path`, `src/lang/mod.rs:30`); no
  exposed "all corpus units" accessor (scan keeps `units: Vec<Unit>` local, `src/lib.rs:82`).

**The two hard constraints.**
- **Spec collision → reversed by D46.** spec §non-goals said *"a daemon/server/IDE plugin —
  CLI + CI only"* / *"do not build a daemon."* D46 promotes servers in-scope.
- **No persistent index.** `scan()` is stateless-per-run; every match stage (exact/near/seq/
  api) rebuilds a whole-corpus index and discards it. The on-disk cache (D19) is **per-file
  extraction only** — a warm hit restores trees+fingerprints, *not* matches. A one-file edit
  is cheap to *extract* but needs a **full global re-match** (~85% of scan cost). Warm
  real-repo scans are sub-second (ripgrep 0.31s, flask 0.08s cold; 500k-LOC synthetic 4.2s
  warm) — so a **debounced full re-scan is a viable v1** and the held incremental index is a
  *deferred, latency-gated* escalation (§6).

## 3. Decisions register (D-SRV-1…7)

- **D-SRV-1 · Scope reversal — SIGNED OFF (D46).** Servers are in scope, superseding the
  spec §non-goal, recorded as DECISIONS.md D46 (fatal-main-storm, 2026-07-03).
- **D-SRV-2 · Topology — RESOLVED: separate workspace crates.** `reprise-mcp`, `reprise-lsp`,
  and a thin `reprise-server-core` (shared config/baseline/projection helpers) depend on
  `reprise` as a lib. The core lib + static CLI stay sync and tokio-free; server async/proto
  deps never enter the CLI binary.
- **D-SRV-3 · State model (v1) — RESOLVED: stateless-per-request.** Hold the warm on-disk
  cache, re-run `scan()`/`check::run()` on demand, debounced. The held in-memory index +
  incremental re-match is deferred behind a measured latency trigger (§6).
- **D-SRV-4 · LSP trigger (v1) — RESOLVED: on `didSave`.** Scans the real on-disk tree,
  sidestepping the dirty-buffer problem. Live-as-you-type (`didChange`) is a v2 feature
  riding the §6 refactor.
- **D-SRV-5 · Build order — RESOLVED: both in parallel.** M1 stands up the workspace +
  shared core; M2 (MCP) and M3 (LSP) then proceed concurrently. (Chosen over MCP-first.)
- **D-SRV-6 · `find_similar` — RESOLVED: append-and-rescan MVP.** Push the candidate `Unit`
  onto the corpus `Vec<Unit>`, run existing exact+near builders, filter to groups containing
  the candidate. An extracted one-vs-many matcher is an optimization gated by the same §6
  latency trigger.
- **D-SRV-7 · Dependency footprint — RESOLVED: `rmcp` 2.1.0 (MCP) + `tower-lsp-server` 0.23
  (LSP).** Both tokio, each confined to its crate. LSP uses the maintained community fork
  **`tower-lsp-server`** (root module `tower_lsp_server`; protocol types re-exported as
  `ls_types`; `Uri` not `Url`; native `async fn` traits, no `#[async_trait]`) — the original
  `tower-lsp` is unmaintained (last release 2023). Revisit `lsp-server` (sync, no tokio) only if
  we want LSP tokio-free.

## 4. LSP — where the spec has room, concretely

Answering "is there room to make reprise an advisory LSP": **yes**, via these surfaces.

### 4.1 Advisory is native
`DiagnosticSeverity.Hint` / `.Information` carry "FYI, not an error." Clone findings →
`Information`; `inconsistent-update` (live drift) → `Warning` if we want it to nag,
`Information` otherwise (configurable).

### 4.2 The group projection (mirror the SARIF choice exactly)
Per clone `Group`: one diagnostic **per member**, all sharing `data.groupId` = `Group.id`
and carrying `Group.fingerprint` for **stable identity across drift** (the SARIF
`partialFingerprints` trick — the alert survives renames/whitespace instead of churning).
Each member's diagnostic gets `relatedInformation[]` pointing at its siblings.
**Accepted loss:** LSP has no "these N are ONE finding" atom, so the client renders N linked
problems — the group-primary→location-primary impedance; documented, not hidden.

### 4.3 The whole-repo, non-open-file scan
Primary channel: **`workspace/diagnostic`** (pull, LSP 3.17) — built for project-wide
analysis not tied to the focused document (rust-analyzer's cargo-check model); its
`previousResultIds` caching maps onto our per-file cache + git baseline. Fall back to
`textDocument/publishDiagnostics` (push) for open files.

### 4.4 The template + the actionable surface
- **CodeLens** above each member: `▲ N-member duplicate · show template` — the unobtrusive
  relational surface that fits "advisory" better than a squiggle.
- **Hover**: render `Group.template` (`v0..vN`, `⟨h1⟩`) as markdown.
- **Code actions**: insert `reprise:ignore` / `reprise:accept-drift` pragmas (already source
  pragmas — natural quick-fixes); "jump to siblings".
- **Live drift (the standout, Milestone 4):** on save, re-check touched units vs the baseline
  and publish `Information` diagnostics with `relatedInformation` → *untouched* siblings — the
  "you're about to create drift" nudge, before the PR. Needs the baseline held warm (§6), so
  it is v2.

### 4.5 Capabilities advertised
`diagnosticProvider` (workspace + inter-file), `codeLensProvider`, `hoverProvider`,
`codeActionProvider`. No formatting, no rename — reprise is report-only.

## 5. MCP — the better fit, on-thesis

Tools (thin wrappers over the lib; report-only — reprise advises, the agent acts):

- **`reprise_scan(path, format?)`** → `reprise::scan` → JSON `ScanReport`. Near-trivial.
- **`reprise_check(path, base?)`** → `check::run` → JSON `CheckReport` (drift + `trend`).
  Base defaults to merge-base with the default branch (shared baseline policy, M1).
- **`reprise_find_similar(snippet, lang)`** → the on-thesis tool: "before you write this
  helper, here's the existing unit it duplicates — call it instead." MVP = D-SRV-6
  append-and-rescan. Needs a corpus-units accessor (factor the extract half of `scan()`,
  `src/lib.rs:49-107`), `&str -> Lang`, a candidate-filter.

**Result format:** the terminal template (`v0..vN` + holes) is *more* token-legible to an LLM
than raw JSON — return template + a compact member list for `find_similar`; `scan`/`check`
return the JSON model.

**Latency:** hold the warm cache; corpus rebuild per call is cheap via the file cache. If
`find_similar`'s per-call rebuild is too slow on a target repo, that is the §6 trigger.

## 6. The deferred escalation — held index + incrementality (v2, latency-gated)

Only if v1's debounced full re-scan proves too slow on a *real* target repo (not the 500k
synthetic).

- **Hold in memory:** `Vec<Unit>` (tree + `u128` fingerprint) + `sources`, **plus** the
  derived index structures ephemeral today — the exact `BTreeMap<(Lang,u128),Vec<usize>>`,
  the near-tier `RepData` + hash→owner posting list + `df`, the sequence `Corpus`/suffix
  array, the api signatures/`df`.
- **Incremental re-match:** none of those tiers has an incremental-update path today (each
  takes `&[Unit]` and rebuilds; D32 lists incremental def-tables as out of scope). Per-tier
  new code — bounded by the language partition, but the real cost.
- **Unlocks:** dirty-buffer live-as-you-type (D-SRV-4) via an extracted `analyze(units,
  sources, cfg) -> ScanReport` (from the ~450-line `scan()`); and warm baseline state making
  the live drift guardrail (M4) affordable per-save instead of a fresh git worktree each time.
- **Gate:** a written latency trigger (target repo + threshold), not a vibe (cf. D-IR-7).

## 7. Milestones

- **M0 · Sign-off — DONE.** D-SRV-1…7 resolved (§3); DECISIONS.md D46 landed.
- **M1 · Workspace + shared core.** Cargo workspace; `reprise` lib stays sync/tokio-free. Add
  `Lang: FromStr` (or public `lang_from_str`); expose a corpus-units accessor by factoring the
  extract half of `scan()`. Shared baseline-resolution helper (merge-base policy) +
  report→protocol projection helpers in `reprise-server-core`.
- **M2 · MCP server — DONE (rmcp 2.1.0, stdio).** `reprise_scan`, `reprise_check`, and
  `reprise_find_similar` (append-and-rescan) — all proven end-to-end over the wire.
- **M3 · LSP server** *(tower-lsp-server 0.23).* DONE (diagnostics slice): group→one-diagnostic-
  per-member with `relatedInformation` (§4.2), Information severity, tier as `code`, fingerprint
  in `data`; published on initialize + `didSave` via a spawn_blocking scan, stale cleared —
  proven over framed stdio. FOLLOW-ONS: `workspace/diagnostic` pull, CodeLens, hover,
  code-action pragmas, debounce.
- **M4 · LSP live drift guardrail.** `inconsistent-update` on save vs baseline — the standout
  feature. Leans on M5's warm baseline state.
- **M5 · Held index + incrementality (deferred, §6).** Only if a real repo trips the latency
  gate. Also unlocks dirty-buffer live-as-you-type.

## 8. Validation

- **Substrate parity (M2/M3):** the servers must project the *same* findings the CLI reports.
  Golden test: `reprise scan --format json` vs MCP `reprise_scan` vs the LSP diagnostic set,
  for a fixed fixture, agree on (member set, tier, fingerprint).
- **MCP:** per-tool schema + golden-result tests; a `find_similar` test feeding a known
  duplicate of an indexed helper and asserting the existing unit is returned.
- **LSP:** one real-client smoke test (VS Code or Neovim) — diagnostics, relatedInformation,
  CodeLens, and a pragma quick-fix round-trip.
- **Latency budget:** record warm re-scan wall-clock on ripgrep/flask/serde per server; that
  measurement *is* the M5 trigger.

## 9. Non-goals (unchanged from spec)

- No change to the buildless / error-tolerant / single-static-binary **CLI** profile — the
  servers are additive crates; the CLI never gains tokio.
- No cross-language matching; no semantic/type resolution (still the separate opt-in Type-4
  tier).
- No rewrite of the matching engine; servers are projections, not new detectors.
- reprise stays **report-only** on every surface — LSP publishes advice, MCP returns advice,
  neither transforms code.
