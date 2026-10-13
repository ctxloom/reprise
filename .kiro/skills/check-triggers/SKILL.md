---
name: "check-triggers"
description: "check-triggers"
---


Review the project's Deferred tasks and decide which ones their **revive
trigger** says should come back now. A Deferred task is one parked on a named
condition (the same idea as an ADR revive trigger): it stays out of the active
list until that condition is met.

## How to run it

1. **List the Deferred tasks.** Call the `task_list` MCP tool with
   `statuses: ["Deferred"]` (Deferred tasks are hidden from the default view, so
   you must name the status). Each task carries a `trigger` field — the
   free-text condition that should revive it. If there are none, say so and stop.

2. **Get machine verdicts.** Call the `evaluate_triggers` MCP tool. It gathers
   evidence (git history since each task was deferred, changed files, the
   repository's current state, and the status of other tasks), batches every
   Deferred task's trigger into one model call, and returns a verdict per
   task: **fired**, **not-fired**, **needs-investigation**, or
   **cannot-determine** — each with evidence and reasoning. For a
   needs-investigation verdict, ctxloom may already have run one bounded
   follow-up look (file existence, a targeted grep, recent commits on a path,
   another task's status) before returning, so what you get back is often
   already the settled answer. Do NOT re-derive your own judgement of whether
   a trigger fired — this tool is the mechanism now; your job is to present
   its output and get a decision.

3. **Report — do not act yet.** Present the verdicts: for each task its harp
   id, text, trigger, verdict, evidence, and reasoning. Lead with
   **fired**. The evidence is the case for the verdict — cite it. Never
   substitute your own sense of how likely a trigger is for what the evidence
   shows; a verdict with thin evidence is thin, and should read that way. Explain what each outcome means for the decision ahead:
   - **fired** — the evidence shows the condition occurred. Propose reviving it.
   - **not-fired** — positive evidence shows the condition hasn't occurred yet. Leave it Deferred.
   - **needs-investigation** — the evidence is suggestive but not conclusive, even after ctxloom's own follow-up look. Worth a closer look from you before deciding; leave it Deferred until then.
   - **cannot-determine** — the trigger can't be judged from evidence available inside this system (e.g. it depends on something a person has to say). Treat it the same as "not yet" — never as fired. Leave it Deferred.

   If `evaluate_triggers` reports `degraded: true`, say so plainly: the
   verdicts it returned are a fallback (cannot-determine across the board),
   not a real judgement, and nothing should be revived on their basis this
   run.

4. **Confirm before moving anything.** Ask the user which of the **fired**
   tasks to revive. Only for the ones they confirm, call `task_set_status`
   with `status: "To Do"` (no trigger needed — moving off Deferred preserves
   the stored trigger). Leave every other task Deferred. Never move a task
   back automatically; the transition is always the user's call.

## Notes

- `evaluate_triggers` proposes; it never changes a task's status itself —
  this command is what turns a confirmed fired verdict into an actual status
  change, and only after the user confirms.
- Verdicts are cached against the evidence that produced them, so a repeat run
  with nothing changed makes no further model calls. Pass `refresh: true` to
  `evaluate_triggers` to force a fresh look regardless.
- This is the read/decide half of the Deferred workflow. The defer half is
  `task_set_status` with `status: "Deferred"` and a `trigger`, or `task_add`
  with the same.
