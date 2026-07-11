//! Gate 2 — the post-extraction memory gate (memory architecture P2).
//!
//! One decision, made once, at a phase boundary (right after extraction, before
//! inline), from exact unit/token counts × a validated linear model — never
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

/// All-in resident bytes per normalized token for a canonical tree, validated
/// against measured peaks at two kernel scales (mem-arch report §5: the
/// ~150 B/node model reconciled within ~8% and <1%). Re-measure after the
/// interning WP lands (P2) — the constant, not the model, will move.
pub const PER_TOKEN_TREE_BYTES: u64 = 150;

/// Inline-variant inflation: variants measure ~49-55% of plain-unit count at
/// kernel scales (mem-arch report §0, two corroborating measurements), applied
/// to the tree term only when the inliner is enabled.
pub const VARIANT_INFLATION: f64 = 1.55;

/// RepData substrate per eligible unit (~20 subtrees × ~80 B; mem-arch report
/// §5's method) — resident from digest time since the fused pass.
pub const REPDATA_PER_UNIT_BYTES: u64 = 1600;

/// Hysteresis headroom (P2): trip at ~70% of budget, absorbing the estimator's
/// known undercounts (landmark index, verify bookkeeping, allocator slack).
pub const TRIP_FRACTION: f64 = 0.70;

/// The Gate-2 outcome, surfaced verbatim into `Stats::memory_*` so
/// measurements can confirm which path ran.
#[derive(Debug, Clone, Copy)]
pub struct GateDecision {
    pub budget_bytes: u64,
    pub estimated_bytes: u64,
    pub over: bool,
}

/// The pinned linear model: post-extraction plain-token/unit counts →
/// estimated peak tree+substrate residency. Pure and environment-free.
pub fn estimate_bytes(plain_tokens: u64, plain_units: usize, inline_enabled: bool) -> u64 {
    let base = plain_tokens * PER_TOKEN_TREE_BYTES + plain_units as u64 * REPDATA_PER_UNIT_BYTES;
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
    let estimated_bytes = estimate_bytes(plain_tokens, units.len(), cfg.inline.enabled);
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
