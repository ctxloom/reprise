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

/// This process's peak resident set size — the high-water mark, `VmHWM`.
///
/// **Observation only. It does not, and must not, feed the gate decision**, which
/// stays deterministic per (corpus, config, budget) per this module's contract.
/// What it feeds is the REPORT: the measured peak is emitted alongside
/// `memory_estimated_bytes` so the model-vs-reality delta is visible on every
/// ordinary run.
///
/// That delta is the whole point. Until now reprise could not observe its own
/// memory, so every measurement bolted on an external `wait4`/`ru_maxrss`
/// harness — and a model nobody could check against reality is how a 6.8 GB
/// dev/prod allocator skew, a 3.3x-understated index row, and a double-counted
/// memory "hump" all survived. The estimator is wrong by 0.59x-1.53x and cannot
/// be fitted; the number it approximates can simply be read.
///
/// Linux only (`/proc/self/status`): `None` elsewhere, and the stat then reports
/// 0 rather than a fabricated figure.
pub fn peak_rss_bytes() -> Option<u64> {
    proc_status_bytes("VmHWM:")
}

/// This process's CURRENT resident set size (`VmRSS`) — a phase-boundary reading,
/// so a scan can attribute its peak (extraction vs the phases after it) instead
/// of reporting one scalar nobody can decompose.
pub fn current_rss_bytes() -> Option<u64> {
    proc_status_bytes("VmRSS:")
}

/// `/proc/self/status` reports these in kB; the field is absent on non-Linux.
fn proc_status_bytes(key: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix(key))
        .and_then(|rest| rest.split_whitespace().next()?.parse::<u64>().ok())
        .map(|kb| kb * 1024)
}

/// The scan's byte budget: pinned `budget_bytes` when set, else
/// `budget_fraction` of the RAM the process can actually use — which under a
/// container memory limit is the CGROUP's limit, not the host's RAM.
pub fn resolve_budget(cfg: &MemoryCfg) -> u64 {
    if let Some(bytes) = cfg.budget_bytes {
        return bytes;
    }
    let mut sys = sysinfo::System::new_with_specifics(
        sysinfo::RefreshKind::nothing()
            .with_memory(sysinfo::MemoryRefreshKind::nothing().with_ram()),
    );
    let ram = effective_ram(sys.total_memory(), our_cgroup_limit(&mut sys));
    (ram as f64 * cfg.budget_fraction) as u64
}

/// This process's own cgroup memory limit, if it is in a constrained cgroup.
///
/// Deliberately the PROCESS accessor, not `System::cgroup_limits()`: the latter
/// reads the ROOT cgroup (`/sys/fs/cgroup`), which on any normal host is
/// unlimited — it answers a question nobody asked. Only
/// `Process::cgroup_limits()` resolves `/proc/self/cgroup` to the cgroup this
/// process actually lives in, which is the one that will OOM-kill it. (sysinfo
/// handles v2 `memory.max` — including the `"max"` sentinel — and v1
/// `memory.limit_in_bytes`, walking up to the effective parent limit.)
fn our_cgroup_limit(sys: &mut sysinfo::System) -> Option<u64> {
    let pid = sysinfo::Pid::from_u32(std::process::id());
    sys.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[pid]),
        true,
        sysinfo::ProcessRefreshKind::nothing(),
    );
    sys.process(pid)
        .and_then(|p| p.cgroup_limits())
        .map(|l| l.total_memory)
}

/// The RAM this process may actually use: the tighter of the host's RAM and any
/// cgroup memory limit.
///
/// The memory gate spills trees to avoid an OOM kill — and inside a container
/// the kill comes from the CGROUP, long before the host runs out. Budgeting from
/// host RAM there means the gate does not trip when it must: measured, a scan in
/// a 2 GiB cgroup resolved a 15.51 GiB budget (7.75x its real ceiling). This is
/// the containerised-CI case, which is exactly where `reprise check` runs.
///
/// A cgroup limit at or above host RAM carries no information (cgroup v2 spells
/// "unlimited" as a sentinel at least that large), and a zero/garbage limit must
/// never starve the budget to nothing — that would spill unconditionally on
/// every scan. Both degrade to host RAM.
fn effective_ram(host: u64, cgroup: Option<u64>) -> u64 {
    match cgroup {
        Some(limit) if limit > 0 && limit < host => limit,
        _ => host,
    }
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

#[cfg(test)]
mod budget_tests {
    use super::*;

    /// The gate exists to spill trees and avoid OOM. Under a container memory
    /// limit — Docker `--memory`, a k8s pod limit, containerised CI, which is
    /// exactly where `reprise check` runs — the OOM comes from the CGROUP, not
    /// from the host running out of RAM. A budget computed from the host's RAM
    /// therefore does not trip when it must, and the runtime kills the scan.
    ///
    /// Measured before the fix: inside a 2 GiB cgroup, reprise resolved a
    /// 15.51 GiB budget — 7.75x the real limit.
    #[test]
    fn a_cgroup_limit_below_host_ram_is_what_bounds_the_budget() {
        let host = 31 << 30; // 31 GiB
        let cgroup = 2 << 30; // a 2 GiB container
        assert_eq!(effective_ram(host, Some(cgroup)), cgroup);
    }

    /// No cgroup, or an unlimited one, must leave today's behavior untouched —
    /// and a cgroup limit ABOVE host RAM is not a licence to over-budget: the
    /// host is still the real ceiling. cgroup v2 reports "max" for unlimited,
    /// which sysinfo surfaces as a value at or above host RAM.
    #[test]
    fn an_absent_or_unlimited_cgroup_falls_back_to_host_ram() {
        let host = 31 << 30;
        assert_eq!(effective_ram(host, None), host);
        assert_eq!(effective_ram(host, Some(u64::MAX)), host);
        assert_eq!(effective_ram(host, Some(64 << 30)), host);
    }

    /// A zero/garbage limit must never yield a zero budget — that would trip the
    /// gate on every scan, spilling unconditionally.
    #[test]
    fn a_zero_cgroup_limit_is_ignored_rather_than_starving_the_budget() {
        let host = 31 << 30;
        assert_eq!(effective_ram(host, Some(0)), host);
    }
}

#[cfg(test)]
mod peak_tests {
    use super::*;

    /// reprise could not observe its own memory. Every gate battery in this
    /// project bolted on an external `wait4`/`ru_maxrss` harness *purely because
    /// the tool cannot report its own peak* — and that missing feedback loop is
    /// why a 6.8 GB dev/prod allocator skew, a 3.3x-understated index row and a
    /// double-counted "hump" all survived unchallenged. Read the number.
    #[test]
    fn peak_rss_is_observable_and_nonzero() {
        let peak = peak_rss_bytes().expect("Linux exposes VmHWM in /proc/self/status");
        assert!(peak > 0, "peak RSS reported as zero");
    }

    /// Not a tautology: the peak must actually TRACK a real allocation. A stub
    /// returning a constant, or reading the wrong field, passes the non-zero test
    /// above and fails this one.
    #[test]
    fn peak_rss_rises_to_cover_a_large_allocation() {
        let before = peak_rss_bytes().unwrap();
        // Touch every page — an untouched Vec may never become resident.
        let mut hog: Vec<u8> = vec![0; 256 << 20];
        for i in (0..hog.len()).step_by(4096) {
            hog[i] = 1;
        }
        let after = peak_rss_bytes().unwrap();
        assert!(
            after >= before + (200 << 20),
            "peak did not track a 256 MiB resident allocation: {before} -> {after}"
        );
        drop(hog);
        // And it is a HIGH-WATER mark, not current RSS: after the free, RSS
        // collapses to a few MB while the peak must stay up at ~256 MiB. This is
        // the assertion that kills a `VmRSS:` typo, which would pass every other
        // check here.
        //
        // Deliberately NOT `later >= after`: the kernel reports
        // `max(stored_hiwater, current_rss)` and only refreshes `stored_hiwater`
        // at certain events, so successive reads can DIP slightly (measured: 488
        // KiB, 0.2%) once RSS falls away. VmHWM is therefore monotone in spirit
        // but not across arbitrary reads — and it can under-report a brief
        // transient spike by that margin.
        let later = peak_rss_bytes().unwrap();
        assert!(
            later >= before + (200 << 20),
            "peak collapsed toward current RSS after the free ({before} -> {later}) \
             — that is VmRSS, not the high-water mark"
        );
    }
}
