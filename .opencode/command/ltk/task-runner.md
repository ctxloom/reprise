---
description: ltk/task-runner
---

# ltk task-runner setup

Configure ltk to redirect raw tool invocations (`go test`, `npm test`,
`cargo test`, …) through this project's task runner, the way the
project's own maintainers run them.

## 1. Detect the task runner(s) in use

Look for these in the project root (and, for a monorepo, in the
package/module directory you are working in):

| File present                                    | Task runner |
|--------------------------------------------------|-------------|
| `justfile` / `Justfile`                           | `just`      |
| `Makefile`                                        | `make`      |
| `package.json` with a `"scripts"` block            | `npm run` / `pnpm run` / `yarn run` — pick by lockfile: `package-lock.json` → npm, `pnpm-lock.yaml` → pnpm, `yarn.lock` → yarn |
| `Cargo.toml` with `[workspace]` xtask / `cargo-make` config | `cargo xtask` / `cargo make` |
| `Rakefile`                                         | `rake`      |
| `tasks.py` / `invoke.yaml`                          | `invoke`    |
| `noxfile.py` / `tox.ini`                            | `nox` / `tox` |

**If more than one candidate is present, STOP and ask the user which one
to standardize on.** Do not guess — a monorepo legitimately mixing `just`
at the root with `npm` inside a JS package is a real, valid shape, and
silently picking one could redirect the wrong tool for the wrong
directory. If exactly one is found, proceed with it.

## 2. Author or merge `.ltk/config.yaml`

Read the existing `.ltk/config.yaml` first (it may already have rules —
**merge**, never overwrite). If it does not exist, start from:

```yaml
version: 1
defaults:
  on_parse_error: allow
  repeat_window_seconds: 30
rules: []
```

For each raw tool you want redirected, append a rule under `rules:`.
Follow these constraints exactly — every one of them is a real footgun,
not a style preference:

- **Rule `id` must be unique and must NOT collide with a shipped default
  rule.** In particular, never reuse `tests-via-task-runner` — that id
  ships in ltk's own default rule set (`ltk manage install`'s starter
  config) and a duplicate id is a config error. Pick a specific,
  descriptive id instead, e.g. `go-test-via-just`, `npm-test-via-just`.
- **Only emit YAML keys that exist in the schema.** ltk parses
  `.ltk/config.yaml` with unknown-field rejection: a single typo'd or
  invented key (e.g. `commands:` instead of `command:`, or a field that
  doesn't exist) makes the WHOLE FILE fail to parse, which fails
  CLOSED — every guarded tool call (Bash, Edit, Write, …) gets denied
  for the rest of the project, not just the one rule you were adding.
  The allowed rule fields are exactly: `id`, `match` (`command`,
  `args_any`, `args_all`, `unless`, `shells`, `path`), `action`,
  `message`, `suggest`, `mode`, `window_seconds`, `delay_seconds`. Do
  not add anything else.
- **`match.command` is an ARGV PREFIX, not a glob or a regex.** Write it
  as a list of literal tokens: `[go, test]`, `[npm, run, test]`,
  `[cargo, test]`. The first token (the program) matches by basename, so
  it also catches an absolute-path invocation. The remaining tokens are
  matched as an ORDERED SUBSEQUENCE among the command's non-option
  arguments — they do not need to be adjacent, but they must appear in
  that relative order. Never write `"go test*"` or `"go .*test.*"` —
  those are not valid `match.command` syntax and will not do what you
  expect (or will not parse at all).
- Prefer `mode: confirm` over the firm default (`mode: enable`) for a
  redirect rule unless the project's maintainers want it to be a hard
  block — a redirect is guidance, and `confirm` lets a determined agent
  repeat the exact command within the window to proceed anyway (see
  `defaults.repeat_window_seconds`).
- Set `message` to name the replacement command directly (e.g. "Run
  tests through `just test`, not `go test` directly, so the suite
  matches CI.") and `suggest` to the literal replacement command.

Example rule redirecting `go test` to `just test`:

```yaml
  - id: go-test-via-just
    match: { command: [go, test] }
    mode: confirm
    message: "Run tests through `just test`, not `go test` directly, so the suite matches CI."
    suggest: "just test"
```

## 3. Validate BEFORE finishing

Run `ltk check --command "<the raw command your rule targets>" --format
json` for every rule you added or changed, e.g.:

```
ltk check --command "go test ./..." --format json
```

Confirm the JSON output has `"decision": "deny"` with the `message`/
`suggestion` you expect. This also doubles as a parse check: if
`.ltk/config.yaml` is malformed, `ltk check` fails loudly (exit 1)
rather than silently allowing everything.

**If validation fails — parse error, wrong decision, or anything other
than the expected deny — REVERT your edit to `.ltk/config.yaml` before
finishing.** Do not leave a broken or wrong config in place: an invalid
`.ltk/config.yaml` fails CLOSED (every Bash/Edit/Write call the hook
guards gets denied, project-wide) and a config with unintended matches
can block the user's own legitimate commands. Only report success once
`ltk check` confirms the rule behaves as intended.

## 4. Tell the user to install the hook

Authoring `.ltk/config.yaml` only writes the rules — it does not wire
the pre-tool hook into the backend's settings on its own. After
validating, tell the user to run:

```
ltk manage install
```

(Under ctxloom, the hook is usually already registered via this
bundle — `ltk manage install` is for a project running ltk standalone,
or to confirm the hook is present.)

## What ltk is, honestly

ltk is a guardrail against *reflexive* mistakes — typing the raw tool
out of habit instead of the project's task runner. It is **not a
security boundary**: an agent explicitly instructed to work around a
rule can (edit the config, use a different shell trick, etc.). Its job
is to make the easy, unthinking path the *correct* one, not to enforce
a boundary nothing can cross. Frame it that way to the user — do not
oversell it as a sandbox or an enforcement mechanism.
