---
title: Transform seam — implementation hand-off (D-IR-12)
status: design for the similarity-ir (IR) effort to execute
audience: the IR agent / whoever owns src/frontend + src/ir
related:
  - docs/SIMILARITY-IR.md §15.3 (the authority: why + the D-IR-12 decision) and §15 / §15.1 / §15.2
  - DECISIONS.md D-IR-9 (reversal), D-IR-10 (log lifecycle)
  - locus: src/frontend/mod.rs `normalize()`; src/ir/pass.rs; src/ir/transform.rs
---

# Transform seam — implementation hand-off

This is the *how* for **D-IR-12 / §15.3** (the *why* and the decision live there). Goal: make the
transform log the structural core of canonicalization so that **a transform cannot happen without
emitting its event** — enforced by the type system, not review. The five passes in `normalize()`
currently mutate the tree with the log inert (`canonicalize_order` is actively dropping operand
order — a live D-IR-9 losslessness break); this converts them.

**The single load-bearing invariant:** the log and witnesses are **hash-excluded**, so this is a
**parity-neutral refactor** — the canonical tree, every `u128` fingerprint, and every finding stay
byte-identical. The only observable change is the event stream going partial → complete. Gate every
step on that.

## 1. The seam (`src/ir/edit.rs`, new)

```rust
/// A described mutation. Detectors emit these; only `apply` performs them.
/// Loci are byte spans, NOT child indices (indices shift under other edits).
pub enum Edit {
    CommSort   { locus: (u32,u32), order: Vec<u32> },   // reorder children; witness = original order
    IterProtocol { locus: (u32,u32), coll: /*…*/, ivar: /*…*/ },
    LoopExit   { locus: (u32,u32) },
    DropDead   { locus: (u32,u32) },                    // witness = the removed subtree
    AbstractIdents { map: Vec<(u32 /*occurrence*/, Box<str> /*original name*/)> }, // whole-tree, positional
}

/// The event sink. Bulk scan disables recording; consumers enable it (D-IR-10).
/// Keep it a flag on the existing TransformLog to avoid making 55 lowering
/// signatures generic — `TransformLog::disabled()` IS the null sink. (A generic
/// `EventSink` trait with a `NullSink` impl is the zero-cost alternative if you
/// prefer it; the structural guarantee below does NOT depend on the sink type.)
impl TransformLog {
    pub fn disabled() -> Self;                 // record() early-returns, allocates nothing
    fn enabled(&self) -> bool;
}

/// The ONLY front-half canonicalization mutator: performs each edit AND records
/// its event, atomically. Witnesses are materialized only when the sink is enabled.
pub fn apply(tree: NormNode, edits: &[Edit], log: &mut TransformLog) -> NormNode;
```

The structural guarantee is **entirely** in the detector signature: `fn detect(&NormNode) ->
Vec<Edit>` has no `&mut`, so a pass physically cannot mutate — "sort without recording" does not
compile. `apply` is the sole mutator and always records. Nothing else in the front half may take
`&mut NormNode` for canonicalization.

## 2. Convert the five passes (`src/ir/pass.rs`)

Each `pub fn x(mut node: NormNode) -> NormNode` becomes `pub fn detect_x(node: &NormNode) ->
Vec<Edit>`. The mutation logic moves into `apply`'s per-`Edit` arm; the detection logic (find the
sites, compute the params) stays in the pass. Recipe per pass:

| Pass (current) | Detector | Edit + witness |
|---|---|---|
| `canonicalize_order` **(do first — live data loss)** | `detect_comm_sort` — find commutative operator chains, compute the sorted permutation over field-stripped s-exprs | `CommSort{ locus, order }`; witness = **original** order (reversal restores it) |
| `rewrite_iteration` | `detect_iter_protocol` — match `0..len(xs)` / `range(len(xs))` index loops whose body uses `xs[i]` only | `IterProtocol{ locus, coll, ivar }`; witness = the original index form |
| `normalize_loop_exit` | `detect_loop_exit` — find `Loop{…}; Return E` block tails | `LoopExit{ locus }`; witness = the original break/return shape |
| `strip_dead` | `detect_dead` — find dead nodes (trailing `continue`, `pass`, empty `else`) | `DropDead{ locus }`; witness = the removed subtree |
| `abstract_idents` | `detect_abstract_idents` — compute the Raw→`Local(n)`/`External` map | one `AbstractIdents{ map }`; witness = position→original-name |

## 3. Rewire `normalize()` (`src/frontend/mod.rs`)

Thread one sink through lowering and the pass fold; keep the current pass **order** (each detector
runs on the prior step's output — fold between passes, so loci stay valid):

```rust
pub fn normalize(lang: Lang, node: Node, src: &str, log: &mut TransformLog) -> NormNode {
    let tree = lower(lang, node, src, log);                    // frontends already record here
    let tree = apply(tree, &detect_iter_protocol(&tree), log);
    let tree = apply(tree, &detect_loop_exit(&tree), log);
    let tree = apply(tree, &detect_abstract_idents(&tree), log);
    let tree = apply(tree, &detect_comm_sort(&tree), log);
    apply(tree, &detect_dead(&tree), log)
}
```

Callers: `extract_ir_units` (bulk scan, `src/unit.rs:180`) passes `&mut TransformLog::disabled()`
and keeps only the tree — zero recording cost, and the `_log` build-and-discard at
`frontend/mod.rs:170` goes away. The reporter / calibration harness re-normalize the touched units
with `&mut TransformLog::new()` (D-IR-10) and fold the stream.

## 4. Landing order (each step parity-gated)

0. **`ir::edit` skeleton** — `Edit`, `apply`, `TransformLog::disabled()`. No behavior change.
1. **`canonicalize_order` → `detect_comm_sort` + `apply`** (fix the live order loss first). Gate.
2–5. Convert the other four passes, **one per step**. Gate each.
6. **Thread the sink** through `normalize()` + lowering; bulk = disabled; add a recording path for
   tests. Gate.
7. **Lowering completeness** — verify each frontend records every normalization it performs; Go is
   newest and records the least (`ParenDrop`, multi-return likely silent). Add coverage tests.

Steps 1–5 are independent under the parity invariant, so they interleave with live frontend work.

## 5. Validation (per step)

- **The guarantee is a type fact.** `detect_*: fn(&NormNode) -> Vec<Edit>` — assert no detector
  takes `&mut`; silent mutation won't compile. This *is* §15.3's bar.
- **Parity (the gate).** `pair_ab` shows **zero** canonical-tree diff (Rust/Python/Go, mutation-recall
  + wild corpus) before/after. Non-negotiable — the tree must not move.
- **Completeness.** A unit exercising the converted transform now yields its event in the stream
  (was absent before).
- **Reversal round-trip.** `N⁻¹(N(x)) ≈ x` under a recording sink for the converted transform — the
  free D-IR-9 harness; catches a wrong/missing witness.
- **Determinism.** Same source → same `TransformLog::to_text()` (existing snapshot style).

## 6. Gotchas

- **Address edits by span/locus, apply stably.** `DropDead` / any removal shifts sibling indices —
  never key an `Edit` by child index. Collect a pass's edits, then apply as one rebuild (e.g. a
  filtering/mapping walk), not sequential index mutations.
- **`abstract_idents` is global + positional** (the D2 cascade). It is **one** `AbstractIdents{map}`
  edit, not N local edits; `apply` relabels deterministically from the map.
- **`detect_comm_sort` runs post-abstraction** in the pipeline — compute the sort key on the tree as
  it is at that point (abstracted labels), matching today's output exactly.
- **Cheap witnesses.** Prefer indices / spans / `&str` slices into `src`; materialize owned data
  (`Box<str>`, a removed subtree) only when `log.enabled()`. A disabled sink then costs nothing.
- **Detect-then-apply walks the tree twice per pass.** Acceptable (extraction is ~12% of a scan);
  fuse the local cases only if a perf gate demands it.

## 7. Out of scope (do not build now)

- **Genesis purity** (§15.3 open sub-decision) — lowering keeps interleaving bucket/paren-drop.
- **Projectors beyond the aggressive tree** — display un-lowering, match-explanation, reversal-for-
  display, calibration attribution land when their consumer (reporter / benches) does (§15.3).
- **Any ES infrastructure** — persistence, event store, async projectors (D-IR-10 holds).
