---
name: "docs-authoring-verify-docs"
description: "docs-authoring/verify-docs"
---

Fact-check docs at {{TARGET}} against code. Assume drift; REFUTE claims, not confirm.

## Oracles (priority order)

1. **Implementation** — source tree at named commit (record it)
2. **Generated reference** — CLI help, schema dumps, tool registries (authoritative for surface)
3. **Current binary** — read-only checks (`--help`, list commands). Verify built from commit.

Never trust summaries of code provided elsewhere.

## Tiers

- **FALSE** — contradicts code (command, flag, key, default, behavior)
- **STALE** — true once; renamed, moved, retired, or inverted
- **UNSUPPORTED** — asserted with no implementation, or absolutes (`always`, `automatically`, `verified`) not guaranteed
- **MISSING** — real feature/key/tool undocumented

## Check hardest

- Every fenced code block is a claim. Failing invocation is critical.
- Two-way diffs for enumerables (config keys, env vars, subcommands): documented-but-absent AND absent-but-real.
- Security/trust claims. Overclaiming (e.g., "fails closed" when only warns) is top priority.
- Silent failures (ignored keys, discarded warnings, inert flags).

## Output

Per page:
- Tier
- Exact claim (quoted, line number)
- Evidence from code (`file:line`)
- Correction

No findings? State explicitly with checklist of what was verified.

Separate **doc bugs** (fix prose) from **code bugs** (prose fix would enshrine defect).

Report only; fix nothing.
