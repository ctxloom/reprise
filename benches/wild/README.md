# benches/wild — real duplicated pairs (D6 mandate)

Non-synthetic recall regression net: each directory holds the ACTUAL source of a
true-positive finding hand-labeled during the Phase-4 M4a stratified precision
sample (CALIBRATION.md Phase 4, §7.2). `tests/wild.rs` scans every directory and
asserts the pair still converges at its labeled tier — unlike the mutation
benchmark, these duplicates were written by the upstream projects, not generated
by inverting our own normalizer rules.

Licensing: each excerpt stays under its upstream license (MIT / BSD-3-Clause /
GPL-2.0), not reprise's. Full attribution and verbatim license texts are in
[`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md) and [`licenses/`](licenses/).
When adding a pair from a new project, update both.

Fixture policy: member source is copied VERBATIM from the provenance commit.
Where a member is a method, the minimal enclosing `impl`/`class` wrapper is
reproduced (headers only); w7's `b.py` is the nested `def test` dedented one
level to stand alone. Nothing inside any function body was altered.

`wc*`-prefixed directories (`wc1_…`, `wc2_…`) are the C-frontend's wild
fixtures (WP-K1b), harvested from a fresh `reprise scan` of the Linux kernel
v7.1 clone's `fs/ext4/` (174 groups found; these two are clear, medium-size
picks — one exact-tier, one near-tier), not from the Phase-4 M4a hand-labeled
sample the `w1`-`w9` fixtures come from. C has **no historical mode**
(`Lang::has_historical_profile` is false for C), so these belong ONLY to the
IR-normalizer expectations in `tests/wild.rs` (a separate `WILD_C` table), never
to the historical `WILD` table — see that file's module doc comment.

| dir | tier asserted | provenance (repo @ commit — path:lines) |
|---|---|---|
| w1_gin_marshalxml | near-normalized | gin @ 34dac20 — utils.go:64-83 and render/render_test.go:287-306 |
| w2_ripgrep_bytecount | exact-normalized | ripgrep @ 48b0c79 — crates/searcher/src/searcher/glue.rs:133-138, 345-350 |
| w3_ripgrep_sort | exact-normalized | ripgrep @ 48b0c79 — crates/core/flags/defs.rs:6358-6372 (--sort), 6460-6474 (--sortr) |
| w4_serde_end | exact-normalized | serde @ 1023d07 — serde/src/private/de.rs:1524-1536, 2482-2494 |
| w5_serde_tagorcontent | near-normalized | serde @ 1023d07 — serde/src/private/de.rs:668-679, 681-692 |
| w6_flask_maxprops | near-normalized | flask @ 36e4a82 — src/flask/wrappers.py:59-86, 92-113 |
| w7_click_chunkpump | exact-region | click @ 6ec99f8 — examples/inout/inout.py:6-30 and tests/test_testing.py:17-25 (dedented) |
| w8_ripgrep_convert | none (relabeled, D37: thin-delegation non-finding) | ripgrep @ 48b0c79 — crates/core/flags/defs.rs:7575-7601 (`convert::{str,usize,u64}`) |
| w9_ripgrep_cpufeatures | internal-repeat | ripgrep @ 48b0c79 — crates/core/flags/doc/version.rs:83-116 |
| wc1_ext4_extspaceroot | exact-normalized (IR only) | linux @ 8cd9520 (v7.1) — fs/ext4/extents.c:303-327 (`ext4_ext_space_root`, `ext4_ext_space_root_idx`) |
| wc2_ext4_mbbits | near-normalized (IR only) | linux @ 8cd9520 (v7.1) — fs/ext4/mballoc.c:1865-1881, 1910-1926 (`mb_clear_bits`, `mb_set_bits`) |

Adding a pair: create a directory, copy the members verbatim (README row with
repo + path + commit), and add a row to the table in `tests/wild.rs` with the
tier the finding carried when it was labeled TP. C-only fixtures (no historical
mode) go in `tests/wild.rs`'s `WILD_C` table instead of `WILD`.
