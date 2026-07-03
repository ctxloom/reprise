# Third-party notices — `benches/wild/`

The fixture directories under `benches/wild/` contain **small source excerpts
copied from third-party open-source projects**, used as a non-synthetic
recall-regression corpus (see [`README.md`](README.md) for the testing rationale
and exact provenance lines). This directory is a test corpus only; it is **not**
part of the `reprise` binary and is **excluded from the published crate**
(`Cargo.toml` `exclude = ["benches/**"]`).

Each excerpt remains under its **original upstream license**, reproduced verbatim
in [`licenses/`](licenses/). `reprise`'s own BSD-3-Clause license (repository
root `LICENSE`) does **not** apply to these excerpts.

## Modifications

Excerpts are copied **verbatim** from the provenance commit, with two mechanical
exceptions that do not alter any function body (per the fixture policy in
`README.md`):

- where a member is a method, the minimal enclosing `impl`/`class` wrapper is
  reproduced (headers only);
- `w7_click_chunkpump/b.py` is the upstream nested `def test` dedented one level
  to stand alone.

All three licenses below (MIT, BSD-3-Clause) permit modification and
redistribution provided the copyright notice and license text are retained —
which this file and `licenses/` do.

## Attribution

| Fixture directory | Upstream project | License | Copyright | Provenance (repo @ commit — path:lines) |
|---|---|---|---|---|
| `w1_gin_marshalxml`   | [gin](https://github.com/gin-gonic/gin)             | MIT (see [`licenses/gin-MIT.txt`](licenses/gin-MIT.txt))                     | © 2014-present Manuel Martínez-Almeida | gin @ 34dac20 — utils.go:64-83, render/render_test.go:287-306 |
| `w2_ripgrep_bytecount`| [ripgrep](https://github.com/BurntSushi/ripgrep)    | MIT OR Unlicense (see [`licenses/ripgrep-MIT.txt`](licenses/ripgrep-MIT.txt))| © 2015 Andrew Gallant | ripgrep @ 48b0c79 — crates/searcher/src/searcher/glue.rs:133-138, 345-350 |
| `w3_ripgrep_sort`     | [ripgrep](https://github.com/BurntSushi/ripgrep)    | MIT OR Unlicense (see [`licenses/ripgrep-MIT.txt`](licenses/ripgrep-MIT.txt))| © 2015 Andrew Gallant | ripgrep @ 48b0c79 — crates/core/flags/defs.rs:6358-6372, 6460-6474 |
| `w4_serde_end`        | [serde](https://github.com/serde-rs/serde)          | MIT OR Apache-2.0 (see [`licenses/serde-MIT.txt`](licenses/serde-MIT.txt))   | © the Serde developers | serde @ 1023d07 — serde/src/private/de.rs:1524-1536, 2482-2494 |
| `w5_serde_tagorcontent`| [serde](https://github.com/serde-rs/serde)         | MIT OR Apache-2.0 (see [`licenses/serde-MIT.txt`](licenses/serde-MIT.txt))   | © the Serde developers | serde @ 1023d07 — serde/src/private/de.rs:668-679, 681-692 |
| `w6_flask_maxprops`   | [flask](https://github.com/pallets/flask)           | BSD-3-Clause (see [`licenses/flask-BSD-3-Clause.txt`](licenses/flask-BSD-3-Clause.txt)) | © 2010 Pallets | flask @ 36e4a82 — src/flask/wrappers.py:59-86, 92-113 |
| `w7_click_chunkpump`  | [click](https://github.com/pallets/click)           | BSD-3-Clause (see [`licenses/click-BSD-3-Clause.txt`](licenses/click-BSD-3-Clause.txt)) | © 2014 Pallets | click @ 6ec99f8 — examples/inout/inout.py:6-30, tests/test_testing.py:17-25 (dedented) |
| `w8_ripgrep_convert`  | [ripgrep](https://github.com/BurntSushi/ripgrep)    | MIT OR Unlicense (see [`licenses/ripgrep-MIT.txt`](licenses/ripgrep-MIT.txt))| © 2015 Andrew Gallant | ripgrep @ 48b0c79 — crates/core/flags/defs.rs:7575-7601 |
| `w9_ripgrep_cpufeatures`| [ripgrep](https://github.com/BurntSushi/ripgrep)  | MIT OR Unlicense (see [`licenses/ripgrep-MIT.txt`](licenses/ripgrep-MIT.txt))| © 2015 Andrew Gallant | ripgrep @ 48b0c79 — crates/core/flags/doc/version.rs:83-116 |

Dual-licensed projects (ripgrep, serde) are retained here under their **MIT**
option; the full text of the alternative license is available in each upstream
repository.

## Adding a fixture

When adding a `benches/wild/` pair (per `README.md`), if it comes from a project
not already listed here: add its row above, and if its license is new, vendor the
upstream license text verbatim into `licenses/`.
