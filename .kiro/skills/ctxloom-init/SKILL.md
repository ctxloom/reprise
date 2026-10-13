---
name: "ctxloom-init"
description: "ctxloom-init"
---


Welcome to ctxloom! You are running the ctxloom **setup interview** — the same
guidance whether this is a brand-new project (launched by `ctxloom init`
directly into your engine's own CLI/TUI) or a reconfigure mid-project
(invoked as `/ctxloom-init` in an ordinary working session). Five phases, in
order:

1. Orient + scan
2. Companions (taskloom + ltk)
3. Profiles + content
4. Agents
5. Close

Work through them with the user conversationally. This is a **reconfigure**
tool, not a from-scratch wizard: phase 1 tells you what already exists, and
your standing rule for the rest of the interview is **reconfigure, don't
re-scaffold** — never re-propose, overwrite, or re-explain something already
in place unless the user actually wants to change it. If the user says
"skip", acknowledge and move on — nothing below is mandatory.

## Phase 1 — Open, orient, scan

**Open the interview by orienting the user** — this is the first thing they
see, and they may never have used ctxloom. Lead with this (say it in your own
voice, but keep the substance and the specifics; deliver it briefly, don't
belabor it):

> I'll help you set ctxloom up for this project. ctxloom's primary interface
> is its own **CLI/TUI** — `ctxloom run` drives your coding engine directly,
> with **assembled context** as the difference from a bare engine: your
> standards, tools, and commands as versioned, shareable *profiles* and
> *bundles* composed into every session, not a hand-kept CLAUDE.md; **many
> engines behind one interface** — bind different agents to different
> engines and models and switch without relearning each vendor's CLI;
> **per-agent isolation** — agents run in containers or git worktrees, so a
> delegated agent can't reach your host or another agent's state;
> **cross-engine delegation** — a coordinator agent spawns and collects work
> from child agents, even ones on a different engine; and **signed,
> trust-verified context** — bundles pulled from shared remotes carry
> signatures from publishers you've chosen to trust. If you also want to
> reach ctxloom from an editor's AI panel (Zed, VS Code, and other ACP
> clients), that's available too, as an optional add-on we can set up later
> — it's not required to get a working setup today.
>
> Setup takes a few minutes: we'll wire up companions, choose your profiles
> and agents, and verify it all.

Then lead straight into the scan — don't recite the phase list mechanically.

**Scan the current directory** for project indicators:
- go.mod, Cargo.toml, package.json, pyproject.toml, requirements.txt
- Dockerfile, docker-compose.yml, Makefile, justfile
- .github/ and other CI/CD configs
- Framework-specific files (next.config.js, vite.config.ts, etc.)

**Then check what ctxloom already has configured** — don't guess, enumerate:
- `ctxloom doctor --deps` — confirm the system tools ctxloom depends on are
  present (`git`, `ssh-keygen` for signing, a container runtime). A fresh
  `ctxloom init` already checked this up front (init needs `git` to clone),
  so this is mainly for the reconfigure path (`/ctxloom-init` in an existing
  session, which skipped init) — worth re-confirming here, cheaply, before
  anything below depends on them. (The FULL `ctxloom doctor` report — agents,
  profiles, hooks, trust — comes at Close, phase 5, once there's real
  configured state for it to describe; running it now would just be a wall
  of "not set up yet" on a brand-new project.)
- `ctxloom agent list` — existing agents (engine↔profile bindings)
- `ctxloom profile list` — existing local profiles
- `ctxloom llm list` — configured engines/models (the candidate `--engine`
  values for anything you bind later)
- `ctxloom container check` — whether containerized agents are viable here
- `ctxloom manage check` — hooks/MCP/statusline wiring per backend, plus
  which first-party companions (taskloom, ltk) are on PATH

If the dep check reports something missing, surface it to the user and
guide them to install it themselves (their own package manager, or the
tool's docs) before continuing — later phases depend on these. `git` is the
notable one: it's close to a hard prerequisite (worktrees, and the remote
pulls phase 3 depends on), so a missing `git` is worth resolving before
anything else here.

Present a short summary: detected stack, what's already configured, what
looks missing. That summary is what makes the rest of this interview a
reconfigure instead of a re-scaffold — you now know what to leave alone.

## Phase 2 — Companions: taskloom + ltk

ctxloom ships alongside two first-party companion binaries that extend it
without any ctxloom code change:

- **taskloom** — per-project task tracking (the `task_list`/`task_add`/
  `task_set_status`/`task_edit` MCP tools, plus the `taskloom` CLI).
- **ltk** — a command-redirect pre-tool hook that intercepts risky shell
  invocations before they run.

**Detect:** check PATH yourself (`which taskloom`, `which ltk`) and/or run
`ctxloom manage check`, which reports each companion's presence and what it
enables (and what's disabled without it).

**Explain + guide install** for anything missing — give the install command
(e.g. `brew install ctxloom/tap/taskloom`, `brew install ctxloom/tap/ltk`),
but the user runs it themselves; ctxloom never installs a binary for you.

**Verify:** once a companion is on PATH, its loadout is picked up
automatically (no separate registration step) — `ctxloom manage check`
should now show it, and `ctxloom manage hooks check` should show its hooks
applied to the backends in use. ltk self-installs its own pre-tool hook the
first time it's invoked; taskloom's MCP tools ride the same `ctxloom manage
hooks install` pass that wires ctxloom's own MCP server.

## Phase 3 — Profiles + content

Find and reference the context this project needs — profiles, fragments,
commands — then move straight into phase 4 to bind the agents that use it;
don't break the conversation between the two, profiles are only half the
picture. Five beats, the same shape as phase 2:

### 3a. Survey — the `default` set, plus what belongs to this project

Two sources, and neither is your own guess about what they need:

1. **Everything tagged `default`.** That tag is the probable-inclusion set —
   content a curator has marked as wanted by most projects regardless of stack.
   Present all of it.
2. **What is logically part of, or shared with, THIS project** — its stack, and
   anything the project or its organisation already references.

Remotes are already cloned locally: read `ctxloom://remotes`, then
`ctxloom://remotes/{name}/contents` per remote to see what is actually there;
`search_library` takes a query (`tag:default`) and is the right tool for
pulling the tagged set specifically. It cannot enumerate — an empty query is an
error — so use the contents resource when you need to see the whole shape.

**Treat the `default` set as probable inclusions: on unless the user says
otherwise.** Present them as "these come as standard, tell me if you'd rather
not" — not as an invitation to opt in.

Why the tag exists, because the alternative was tried and is worse. This
instruction used to say "present a short, relevant list", with relevance
computed from the detected STACK — so anything whose value was a CAPABILITY
rather than a language was never mentioned, and the user never learned it
existed. A code-intelligence MCP that makes editing cleaner and cheaper in
context (serena, tagged `mcp, code-intelligence, lsp`) is invisible to a search
for `tag:golang` and useful to nearly every project. The fix is NOT for you to
enumerate the entire catalog and make them sit through it — it is that
`default` carries that judgment explicitly, made by whoever published the
content. Trust the tag; do not re-derive relevance yourself, and do not silently
drop something because it looks unrelated to the stack.

Every item you present gets a decision: **silence is not exclusion**, and an
item nobody was shown was not declined. Group and batch so it stays a
conversation — capability tools, stack content, standards and process, commands
and skills — and say what each thing is FOR in one line.

Before leaving this phase, say back what was EXCLUDED and why, not just what
was taken — a user who later wonders "why don't I have X" should have heard the
answer once already. Say it; ctxloom has nowhere durable to keep it today, so
do not imply it is saved anywhere and do not invent a file to put it in.

### 3b. Reference

Author a local profile that points at what they picked — you don't
"install" remote content. `ctxloom profile create <name> --parent
<pull_ref>` inherits a remote profile; `-b <pull_ref>` pulls in a bundle (or
one fragment of it); a `@<tag-or-sha>` suffix on the ref pins a version. Then
`ctxloom deps pull` so the reference actually resolves and the lockfile
updates. `ctxloom profile create --help` has the exact flags.

### 3c. Role profiles — one per job, not one for everything

A profile is a composition for a JOB, and the jobs differ enough that one
profile serving all of them serves none of them well. Author a profile per role
the user actually wants, with them, and steer HARD away from a single
do-everything profile — the same argument phase 4 makes about agents, one layer
down.

The roles worth naming, because they are the ones that exist whether or not
anyone sets them up:

- **the working profile** — whatever this project's actual development needs:
  stack conventions, standards, process. The one a bare `ctxloom run` uses.
- **reader / searcher** — for agents that go and find out and report back.
  Wants domain vocabulary and navigation, and almost none of the behavioural
  standards: a finder is not writing code, so a linter's rules are dead weight
  in its context and dilute what it is actually for.
- **triage** — for judging deferred tasks' revive conditions. Wants domain
  context above all: whether "the v2 API ships" has fired is answerable only by
  someone who knows what this project's terms mean.
- **distiller** — for compressing sessions and content. Same instinct as
  triage: domain vocabulary so a summary reads in the project's own terms, and
  NOT behavioural guidance, which distorts a summariser rather than helping it.

These are configurable, not fixed: the user decides what composes each, and can
skip any they do not want. Explain what each is for and let them choose — but
do offer all of them, because an agent whose profile nobody authored falls back
to nothing, not to something sensible.

What each profile CONSIDERED is recorded per profile — included and declined
alike, keyed by the item's canonical ref — so an item turned down for the
distiller is still offered for the working profile. Ask in each profile's own
context rather than once globally.

### 3d. Default

Bind the chosen profile(s) into an agent and point `default_agent` there
(`ctxloom agent create dev --profiles <name>`, `ctxloom agent default dev`) so
a bare `ctxloom run` picks it up — confirm the choice with the user.

### 3e. Review

Anything pulled from a remote the user hasn't seen before is held, not
silently active. Run `ctxloom review` (`--list` for just the queue) and walk
them through accept/reject — a standing surface, not a one-time setup step,
so it's fine to point it out again on a later reconfigure too.

## Phase 4 — Agents

Bind **ctxloom agents** — named, LOCAL bindings of an **engine** (LLM
backend/model) to one or more **profiles**, optionally a **runtime**
(host | container) — right after phase 3, same conversation. `ctxloom run
--agent <name>` drives one directly; a running coordinator fans work across
several by spawning them as children with the `agent_run` MCP tool. Agents
live only in this project's `.ctxloom` — never shipped in bundles or
remotes; engine choice is always the user's, you facilitate. (Workspace
isolation is a separate, per-invocation choice — `--workspace worktree` on
`run`/`acp`, or the `workspace` field on an `agent_run` spawn — not an agent
property.) Work this the same shape as phase 3: **SCAN → DISCUSS → SET**.

### 4a. Scan

Don't guess — you have this already from phase 1 (engines via `ctxloom llm
list`, existing agents via `ctxloom agent list`, container viability via
`ctxloom container check`) and phase 3 (profiles). If containers would
degrade here, plan on `--runtime host` and say why. If they're viable and a
bundle declares tooling needs, `ctxloom container tooling` shows the proposed
Containerfile diff — apply only what's approved, then `ctxloom container
build`.

### 4b. Discuss

Lead with the standard trio, bound to the phase-3 profiles:
- **coordinator** — the session the user drives, delegating to children via
  `agent_run`. Most powerful engine.
- **developer** — the implementer the coordinator delegates to. Powerful
  engine, `--runtime container`, plus an escalation-discipline fragment
  (ctxloom-default ships `agent-roles#fragments/developer-escalation`, or
  author an equivalent) so structural/interface changes get escalated back
  up rather than decided alone.
- **finder** — cheap, parallel, terse: look something up and report
  straight back, no elaboration. Cheap engine, the
  `agent-roles#fragments/finder` fragment (or an equivalent).

Then the open palette. **Code-review agents** are the notable one: build the
ensemble THIS project needs now and persist it, so the coordinator can fan it
out later via `agent_run`. The pattern is one tight, single-lens profile per
lens × language present — composed from the code-review base fragment plus
that lens's and that language's fragments (search_library surfaces the exact
refs). Prefer many narrow lenses over one broad reviewer; the coordinator
reads each child's `agent_report` back and synthesizes the findings itself —
no separate reduce agent needed. Beyond that: docs writer, migration runner,
test author, whatever the user wants — same pattern each time: tight scope, a
composed profile, a fitting engine.

Steer HARD toward tight, single-responsibility agents — many small focused
children keep `agent_run` fan-out cheap and accurate; push back if the user
proposes one big do-everything agent. As a rule of thumb: cheap engine →
finders and breadth review; powerful → coordinator/developers; per-lens →
code review (often cheaper than the developer).

### 4c. Set

Write what's agreed:

```
ctxloom agent create <name> --engine <engine> --profiles <p1,p2,...> --runtime host|container
```

(`ctxloom agent create --help` has the full flag list.) Worth knowing beyond
the syntax: `--profiles` composes several into one context; `--runtime
container` needs a reachable container runtime, else fall back to host and
say why; change an existing one with `agent edit <name>`, drop one with
`agent remove <name> --yes`. Confirm
with `ctxloom agent list`, then tell the user how to use what they built
(`ctxloom run --agent coordinator`, which fans the review ensemble out via
`agent_run` when asked).

### 4d. ctxloom's own agents — distiller and triage

ctxloom itself calls an LLM: to distil sessions and bundle content, and to
judge whether a deferred task's revive condition has fired. Those calls are
ordinary runs, so they need ordinary agents — set them up here, with the user,
the same as any other.

Create a `distiller` and a `triage` agent. They take the same flags as
anything above, and the two that matter are:

- `--engine` — these run often and on cheap work, so a small fast model is
  usually right; the user chooses, you explain the trade-off.
- `--runtime` — they honour the runtime axis like any agent. Say so plainly:
  if the user containerises their work agents and leaves these on the host,
  ctxloom's own calls run on the host.

Their `--profiles` are a real choice, not a formality. Behavioural guidance
(write tests first, use this linter) is noise to a summariser and can distort
it; domain guidance (the glossary, the architecture vocabulary, what this
project's terms mean) is what makes a distillation or a trigger verdict
sensible rather than generic. Steer toward the latter.

Nothing falls back if these are missing: a project without them cannot distil
or evaluate triggers, and the command says so rather than quietly using some
built-in default. That is deliberate — a hidden default is how ctxloom's own
LLM calls drifted out of the user's control in the first place.

If they want to stop, that's fine — `/ctxloom-init` reconfigures any time.
Don't write anything the user didn't agree to.

## Phase 5 — Close

Run `ctxloom doctor` and show the user its report. This is the full
postcondition check — marker dir and config validity, the engine resolving
and authenticating, seeded deps locked and context assembling, hooks/MCP
registered, companions detected, and agents present with resolvable
profiles. Now that phases 1–4 have built real configured state, `doctor`
describes it end to end (unlike phase 1's scoped `--deps`, which only asked
"are the system tools present"). Walk the user through any `warn` lines and
help resolve them.

Then tell the user, plainly:
- **You have a working setup.** `ctxloom run` (or `ctxloom run --agent
  <name>`) is the primary way to reach ctxloom from here — this vendor
  CLI/TUI session was just the bootstrap door.
- **Want to reach ctxloom from an editor's AI panel too?** That's optional
  and separate from this interview — invoke the **acp-setup** skill (or ask
  me to) any time to configure ctxloom as an ACP server for a client like
  Zed/VSCode, or as an ACP client to a different ACP-speaking agent.
- **`/ctxloom-init` reconfigures any time**, from any ordinary working
  session — nothing here was a one-shot wizard; come back whenever something
  needs to change (a new profile, a new agent, a new companion).
