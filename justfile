default: test

build:
    cargo build

test:
    cargo test --all-targets

lint:
    cargo clippy --all-targets -- -D warnings
    cargo fmt --check

fmt:
    cargo fmt

# Mutation-recall benchmark (spec §7.1) — the Phase-1/2 gate metric
bench-mutations:
    cargo test --test mutation_recall -- --nocapture

# Dogfood: the tool scans its own repo (spec §3)
scan-self:
    cargo run --release -- scan .

# Install onto PATH from this checkout.
install:
    cargo install --path . --locked

# Fully static Linux binary: +crt-static on the gnu target (works without a
# musl C toolchain, which the tree-sitter grammar C code would need for
# x86_64-unknown-linux-musl; use that target instead when musl-tools is
# installed). See DECISIONS.md D34.
build-static:
    RUSTFLAGS="-C target-feature=+crt-static" cargo build --release --target x86_64-unknown-linux-gnu
    @file target/x86_64-unknown-linux-gnu/release/reprise | grep -o "static-pie linked" || true

install-static: build-static
    RUSTFLAGS="-C target-feature=+crt-static" cargo install --path . --locked --target x86_64-unknown-linux-gnu
