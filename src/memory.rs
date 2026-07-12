//! Gate 2 — the post-extraction memory gate (memory architecture P2).
//!
//! One decision, made once, at a phase boundary (right after extraction, before
//! inline), from exact unit/token counts × a validated phase-max model — never
//! runtime RSS-watching, so the decision is deterministic per
//! (corpus, config, budget). Under budget: trees stay resident, verify reads
//! them directly, ZERO new work — the common case pays nothing. Over budget:
//! `scan()` spills trees (and sequence streams) to the scan-scoped pack
//! ([`crate::pack`]) and near-tier verify materializes pairs through the LRU.
//!
//! **Invariant: the gate changes performance, never output** — pinned by
//! tests/memory_gate.rs and tests/characterization.rs on both sides.

use crate::config::{Config, MemoryCfg};
use crate::unit::Unit;

/// Resident bytes per normalized token for the canonical trees ALONE — the
/// near-phase tree residency, with the landmark index **excluded**.
///
/// This constant was 355 and it was *fused*: 355 was fit against measured peaks
/// that necessarily included the landmark index at the then-hard-coded
/// `FAN_OUT = 3`, so the index was in the model all along — invisibly, and welded
/// to one fan-out. It could not respond to `retrieval.landmark_fan_out`, which is
/// precisely the dial that moves the index (the largest memory row in a big scan).
/// Adding an index term *on top of* 355 would have double-counted it; the fix is
/// this decomposition, and 234 + 3 × [`PER_FANOUT_TOKEN_INDEX_BYTES`] = 342
/// reproduces the old 355 at the default fan-out, as it must.
///
/// Measured by a fan_out sweep (0,1,2,3,4,6) on linux-7.1 fs / net / drivers, under
/// **jemalloc — the shipped global allocator** (`src/main.rs`). This matters: an
/// out-of-tree harness that misses `#[global_allocator]` measures glibc malloc, which
/// runs ~4% LOW at fs/net and ~13% HIGH at drivers. Calibrate against the real binary.
/// reading the index-free intercept of VmHWM vs index size: 233.3 / 215.5 / 204.8
/// B/token. Pinned to the max — the gate must never UNDER-predict (that is an OOM;
/// over-predicting merely spills early).
pub const PER_TOKEN_TREE_BYTES: u64 = 234;

/// Resident bytes per normalized token at the **extraction** peak, where the
/// landmark index does not exist yet. The process peak is a MAX over phases, not a
/// sum: below fan_out ≈ 2 the index is too small to bind and extraction sets the
/// peak. Without this term the model would under-predict every low-fan_out scan.
///
/// Same sweep, the `fan_out = 0` peak: 264.7 / 243.7 / 208.7 B/token on fs / net /
/// drivers. Pinned to the max.
pub const PER_TOKEN_EXTRACT_BYTES: u64 = 265;

/// Resident bytes of landmark index per (fan_out × normalized token) — the term
/// the estimator was blind to.
///
/// Derived, not assumed. The chain, measured on all three corpora and in BOTH gate
/// regimes (tripped at drivers, untripped at fs/net):
///   * admitted rare peaks per token:      0.80 / 0.93 / 0.88  (mean 0.87)
///   * index hashes per (fan_out × peak):  0.89 / 0.87 / 0.85  (mean 0.87)
///   * RSS bytes per index hash:           65.3 / 69.2 / ~58   (mean ~64)
///
/// A `u128` hash is 16 B, but a hash COSTS ~64 B resident: `shared_count_pairs`
/// materializes a 32-B `(u128, u32)` entry per hash, the per-unit `Vec<u128>`
/// keeps its own copy plus growth slack, and the `Vec<(u32,u32)>` pair-event
/// buffer rides on top. Counting struct sizes instead of measuring RSS is the
/// error that once put the index at 2.6 GB when it was 8.49 GB.
///
/// Composing and dividing out [`VARIANT_INFLATION`]: B per
/// fan_out per token: 35.8 / 30.0 / 28.1. Pinned to the max.
pub const PER_FANOUT_TOKEN_INDEX_BYTES: u64 = 36;

/// Inline-variant inflation: variants measure ~49-55% of plain-unit count at
/// kernel scales (mem-arch report §0, two corroborating measurements). Applied to
/// the whole phase-max, not just the tree term: inline variants are units, so they
/// carry landmark hashes into the index too.
pub const VARIANT_INFLATION: f64 = 1.55;

/// RepData substrate per eligible unit (~20 subtrees × ~80 B; mem-arch report
/// §5's method) — resident from digest time since the fused pass.
pub const REPDATA_PER_UNIT_BYTES: u64 = 1600;

/// Hysteresis headroom (P2). **Was 0.70, and its own comment said it existed to
/// absorb "the estimator's known undercounts (landmark index, …)". That was
/// false**: measured against resident VmHWM, the old estimator did not undercount
/// — it OVER-predicted by +6% (fs) and +18% (net), because the index was already
/// fused into its per-token constant. The 30% haircut was covering a term that was
/// never missing.
///
/// With the index modelled explicitly the estimate lands +0.8% (fs) / +11.7% (net)
/// over measured resident peak — conservative by construction (every constant is
/// pinned to the max over three corpora). What is left for this fraction to cover
/// is the genuinely unmodelled residual, and only that:
///   * fixed overheads outside the model: pack/spill buffers, the verify LRU, the
///     grouped report, tree-sitter parser scratch;
///   * allocator fragmentation (jemalloc);
///   * coefficient drift on corpora unlike the C calibration set.
///
/// 15% covers that with room; it is hysteresis, not a fudge factor absorbing a
/// term we can now compute.
pub const TRIP_FRACTION: f64 = 0.85;

/// The Gate-2 outcome, surfaced verbatim into `Stats::memory_*` so
/// measurements can confirm which path ran.
#[derive(Debug, Clone, Copy)]
pub struct GateDecision {
    pub budget_bytes: u64,
    pub estimated_bytes: u64,
    pub over: bool,
}

/// The pinned model: post-extraction plain-token/unit counts × the landmark
/// fan-out → estimated peak residency. Pure and environment-free — no RSS
/// sampling, no allocator introspection, same answer on every machine.
///
/// The peak is a **max over two phases**, not one scalar:
///   * extraction — trees + parser scratch, no landmark index yet;
///   * near — trees + the landmark constellation index, which is the largest
///     single row at scale and scales ~linearly in `fan_out`.
///
/// Whichever phase is taller sets VmHWM. At the default `fan_out = 3` the near
/// phase binds on every corpus measured; at `fan_out ≤ 1` extraction binds.
pub fn estimate_bytes(
    plain_tokens: u64,
    plain_units: usize,
    inline_enabled: bool,
    landmark_fan_out: usize,
) -> u64 {
    let near = plain_tokens * PER_TOKEN_TREE_BYTES
        + plain_tokens * PER_FANOUT_TOKEN_INDEX_BYTES * landmark_fan_out as u64;
    let extract = plain_tokens * PER_TOKEN_EXTRACT_BYTES;
    let base = near.max(extract) + plain_units as u64 * REPDATA_PER_UNIT_BYTES;
    if inline_enabled {
        (base as f64 * VARIANT_INFLATION).round() as u64
    } else {
        base
    }
}

/// Whether `estimated` trips the gate against `budget` (the ~70% hysteresis
/// point). A zero/absurd budget fails safe: everything trips.
pub fn trips(estimated: u64, budget: u64) -> bool {
    estimated as f64 >= TRIP_FRACTION * budget as f64
}

/// The scan's byte budget: pinned `budget_bytes` when set, else
/// `budget_fraction` of detected system RAM.
pub fn resolve_budget(cfg: &MemoryCfg) -> u64 {
    if let Some(bytes) = cfg.budget_bytes {
        return bytes;
    }
    let sys = sysinfo::System::new_with_specifics(
        sysinfo::RefreshKind::nothing()
            .with_memory(sysinfo::MemoryRefreshKind::nothing().with_ram()),
    );
    (sys.total_memory() as f64 * cfg.budget_fraction) as u64
}

/// The force override: `REPRISE_MEMORY_FORCE_GATE` env var (harness knob),
/// then `memory.force_gate` config. `Some(true)` = spill, `Some(false)` =
/// resident, `None` = decide on the estimate.
fn force_override(cfg: &Config) -> Option<bool> {
    let parse = |v: &str| match v {
        "always" => Some(true),
        "never" => Some(false),
        _ => None,
    };
    if let Ok(env) = std::env::var("REPRISE_MEMORY_FORCE_GATE")
        && let Some(f) = parse(&env)
    {
        return Some(f);
    }
    parse(&cfg.memory.force_gate)
}

/// Gate 2, composed: called by `scan()` once, right after `corpus_units`
/// returns (every unit is plain at that point — the variant term is modeled
/// by [`VARIANT_INFLATION`], since inline has not run yet).
pub fn decide(units: &[Unit], cfg: &Config) -> GateDecision {
    let budget_bytes = resolve_budget(&cfg.memory);
    let plain_tokens: u64 = units.iter().map(|u| u64::from(u.token_count)).sum();
    let estimated_bytes = estimate_bytes(
        plain_tokens,
        units.len(),
        cfg.inline.enabled,
        cfg.retrieval.landmark_fan_out,
    );
    let over = match force_override(cfg) {
        Some(forced) => forced,
        None => trips(estimated_bytes, budget_bytes),
    };
    GateDecision {
        budget_bytes,
        estimated_bytes,
        over,
    }
}
