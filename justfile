default: test

build:
    cargo build --workspace

# ===== Version management (versionator) =====
#
# Releases are merge-triggered: bump the version here, open a PR, merge. CI's
# version-guard rejects a stale VERSION; on merge, auto-release.yml tags
# v$VERSION and release-completer publishes the release + Homebrew cask. Do not
# tag by hand.

# Show the current release version.
show-version:
    @versionator output version

# Set the release version — the supported way to bump. Writes both VERSION
# (versionator's source of truth) and Cargo.toml, so `reprise --version` and the
# release tag always agree (the invariant ci.yml's version-guard enforces).
# Cargo.lock refreshes on the next build. Example: just set-version 0.2.0
set-version version:
    versionator set {{version}}
    sed -i -E '0,/^version = "[^"]*"$/ s//version = "{{version}}"/' Cargo.toml
    @echo 'VERSION + Cargo.toml set to {{version}}. Run `just build` to refresh Cargo.lock, then commit.'

# Validate the full release build locally without publishing: cross-compiles
# every target via cargo-zigbuild + zig (both must be installed) and assembles
# the archives + cask exactly as a tag would, minus the upload. Catches config
# or cross-compile breakage before a release.
release-snapshot:
    goreleaser release --snapshot --clean --skip=publish

test:
    cargo test --workspace --all-targets

lint:
    cargo clippy --workspace --all-targets -- -D warnings
    cargo fmt --check

fmt:
    cargo fmt

# Mutation-recall benchmark (spec §7.1) — the Phase-1/2 gate metric. Runs on the
# IR normalizer, the default since the §8 switchover.
bench-mutations:
    cargo test --test mutation_recall -- --nocapture

# Same benchmark, IR normalizer pinned explicitly (`[normalize] normalizer =
# "ir"`). Redundant with `bench-mutations` now that IR is the default, but kept
# as an explicit, self-documenting gate. The bench reads REPRISE_NORMALIZER to
# select the normalizer, so IR-path mutation recall is measured repeatably.
bench-mutations-ir:
    REPRISE_NORMALIZER=ir cargo test --test mutation_recall -- --nocapture

# Same benchmark on the still-supported historical normalizer path
# (`[normalize] normalizer = "historical"`) — keeps the historical path gated now
# that it is no longer the default, so its recall can't silently regress.
bench-mutations-historical:
    REPRISE_NORMALIZER=historical cargo test --test mutation_recall -- --nocapture

# Retrieval bake-off (docs/substantiality-metric.md §0.4): races the REAL
# `matchtree::Retriever` seam — the shipping landmark retriever over
# `matchtree::build_reps` — against canonical alternatives (minhash-lsh,
# winnowing/MOSS, sourcerer-rare) that implement the SAME trait bench-side, judged
# by a retriever-independent oracle (verify-on-union ACCEPT + synthetic mutants).
# Fidelity is automatic: the incumbent IS the production code, not a reconstruction.
# Pass roots as extra args (default: src). Mirrors `just bench-mutations`.
bench-retrieval *ROOTS:
    cargo run --release --example bakeoff -- {{ROOTS}}

# Dogfood: the tool scans its own repo (spec §3)
scan-self:
    cargo run --release -- scan .

# Install onto PATH from this checkout.
install:
    cargo install --path . --locked

# Portable Linux binary: musl-static via cargo-zigbuild. This is the SHIPPABLE
# build. zig supplies the musl C toolchain the tree-sitter grammars need, so the
# old "musl needs musl-tools" blocker (DECISIONS.md D34) no longer applies. The
# result has NO libc dependency and runs on ANY Linux regardless of glibc —
# unlike `just install` / `cargo build`, whose glibc-dynamic binary built on a
# newer-glibc host fails on an older image (e.g. Debian bookworm agent images).
# Requires: cargo-zigbuild + zig, and `rustup target add x86_64-unknown-linux-musl`.
build-static target="x86_64-unknown-linux-musl":
    cargo zigbuild --release --target {{target}}
    @file target/{{target}}/release/reprise | grep -q "statically linked" && echo "OK: {{target}} is statically linked" || { echo "ERROR: {{target}} is not static"; exit 1; }

# Build the musl-static binary and install it onto PATH (${CARGO_HOME:-~/.cargo}/bin),
# overwriting any glibc-dynamic `just install` build. Use this to bundle reprise
# into a host/image that may have an older glibc than the build host.
install-static: build-static
    install -m 0755 target/x86_64-unknown-linux-musl/release/reprise "${CARGO_HOME:-$HOME/.cargo}/bin/reprise"
    @echo "Installed musl-static reprise to ${CARGO_HOME:-$HOME/.cargo}/bin/reprise"
