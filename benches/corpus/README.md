# benches/corpus — opt-in real-world benchmark corpus

External repositories used as a large real-world corpus for benchmarking
reprise. Each is a git submodule pinned to a recent stable release tag; the
tag (and the URL it resolves against) is recorded per entry in `.gitmodules`,
and the pinned commit is the usual gitlink.

Content is **opt-in**: a normal clone of reprise does not fetch it (the
entries are `update = none`, so even `--recurse-submodules` skips them).
Fetch it, shallow, with:

    just corpus-fetch

Hygiene: reprise.toml's `[scan] exclude = ["benches/**"]` keeps self-scans
(`just scan-self`) clean of the corpus, and the test suite reads only
`benches/wild` and `benches/mutations`, so `just test` never touches it.

Licensing: nothing here is vendored or redistributed — the submodules are
pointers to the upstream projects, and all content remains under its
upstream license (see each project's own LICENSE).
