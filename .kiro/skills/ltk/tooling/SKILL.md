---
name: "ltk-tooling"
description: "ltk/tooling"
---

This bundle's guidance assumes the ltk hook can actually run, so
agent environments (the agent container image) need the binary:

- ltk: Homebrew or a release binary from GitHub
  (https://github.com/ctxloom/ltk). After install, the project
  wires the hook with `ltk manage install` (writes the agent hook
  + .ltk/config.yaml) — don't wire it from the image build.
