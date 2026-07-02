# Changelog

All notable changes to reprise. Format follows [Keep a Changelog](https://keepachangelog.com/);
this project adheres to [Semantic Versioning](https://semver.org/).

Per-phase measured evidence is in [`CALIBRATION.md`](CALIBRATION.md); every
deviation from the spec is in [`DECISIONS.md`](DECISIONS.md).

## [0.1.0-dev] — unreleased

The initial implementation, built in four phases against the Rev 9 spec
(`docs/PLAN.md`). Not yet published to crates.io (license pending — DECISIONS.md D33).

### Phase 1 — skeleton + exact tier (Rust, Python)
- tree-sitter parsing, unit extraction, and normalization (strip/loop-decompose/
  abstract identifiers+literals/dead-syntax); whole-unit Merkle structural hash and
  bucket grouping; minimal terminal report.
- Gate met: 100% Type-1/Type-2 mutation recall; scans its own repo; a ~100k-LOC
  scan runs in 0.11 s (target < 30 s).

### Phase 2 — sequence tier, near-miss tier, folding
- Suffix-array sequence tier for exact runs; full Rust/Python desugaring; sibling-run
  folding; subtree-hash bags and Shazam-style landmark pairs with offset-histogram
  verification; anti-unification with Smith-Waterman list alignment and factorability
  classification; ranking, template rendering, JSON output, and the test-code policy.
- Retrieval rivalry resolved: landmark pairs kept, hole-context hashes dropped (D16).
- Gate met: 100% per Type-3 mutation class and tail-recursion; 0% on the designed-to-fail
  controls; 100% precision on the (small) manual sample.

### Phase 3 — inliner, baseline/drift, PR mode, remaining languages
- Best-effort inliner with the mutual-recursion SCC chain, and the suspicion-only
  api-profile tier (M3a).
- `reprise baseline`, baseline-aware `check`, the `inconsistent-update` drift finding,
  `reprise:ignore` pragma, and a version-keyed on-disk cache; retrieval/parallelism
  hardening (M3b, audited in D27).
- TypeScript/TSX, Go, and Kotlin language profiles (M3c).
- SARIF / CPD-XML / jscpd-JSON output formats and the §7.3 jscpd comparison, which
  showed 438 reprise-only findings on ripgrep and 149 on flask (M3d).
- Gate met: inline and mutual-recursion recall above bar; scripted end-to-end drift
  scenario passes; 500k-LOC warm check within the renegotiated ≤10 s budget (D27).

### Phase 4 — hardening, calibration, and publish preparation
- Calibration/quality wave (M4a): api-profile retuned and kept (`api_profile_sim`
  0.8 → 0.5, D29); stratified 50-finding precision sample at 70% overall; dispatch-arm
  internal-repeat mitigation (D30); the `benches/wild/` recall net (D31); lossless
  perf pass to 8.1 s warm at 500k LOC (D32).
- Documentation and publish preparation (M4b): this README/CHANGELOG, `docs/CONFIG.md`,
  CLI help polish and a documented 0/1/2 exit-code split, Cargo.toml publish metadata
  (license is an unconfirmed placeholder — D33), and CI release-smoke job.

[0.1.0-dev]: https://github.com/OWNER/reprise
