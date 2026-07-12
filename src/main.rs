use anyhow::bail;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

// jemalloc on every target EXCEPT Windows — D42's musl-only gate is lifted (mostly).
// Two independent, separately-measured reasons:
//
//  1. musl (D42's, unchanged): the default musl allocator is ~16x slower than glibc
//     under the rayon-parallel scan (77s vs 4.7s on the 500k perf corpus).
//
//  2. glibc (dev/CI build — the allocator-residual investigation): glibc malloc
//     cannot give back the trees the memory gate frees. Extraction packs ~12.7 GB of
//     `NormNode` (64 B each) into rayon's per-thread arenas as MILLIONS of small
//     chunks; the gate then spills the trees and frees them, leaving ~8.7 GB of free
//     chunks (18.3 M of them at drivers/ scale) scattered MID-ARENA — so almost no
//     page is wholly free and `malloc_trim` reclaims ~nothing. The near tier then
//     asks for LARGE contiguous Vecs (the landmark entries/events), which exceed the
//     mmap threshold and are served by FRESH mmap rather than out of that 8.7 GB —
//     the freed memory is the wrong SHAPE to satisfy the new demand, so both are
//     resident at once. jemalloc's arena/extent reclaim returns it, measured at
//     drivers/ scale (interleaved): peak RSS 22.70 GB -> 15.37 GB (-7.33 GB, -32%)
//     AND wall -29%. jemalloc also measured strictly better than mimalloc (which
//     D42 originally used on musl) on both drivers/ and fs/, with no fs-scale
//     regression, so it replaces mimalloc everywhere it can, rather than gating
//     per-target. Output identical.
//
// This also makes the dev/CI (glibc) build match the SHIPPED (musl) binary, which
// previously had mimalloc on musl only: the 22.6 GB peak that every Tier-2 memory
// budget was built on was a glibc-dev-build artifact the release binary never had.
//
// Windows is excluded, not by choice: tikv-jemalloc-sys fails to build for
// x86_64-pc-windows-gnu (reprise's shipped Windows target) — confirmed by attempting
// it — mirroring long-unresolved upstream Windows gaps in jemalloc itself. Windows
// keeps the system allocator, exactly as before this patch (no regression, no change).
#[cfg(not(target_os = "windows"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[derive(Parser)]
#[command(
    name = "reprise",
    version,
    about = "Code duplicate detection: repo scans and PR-time drift gating"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Full-repo scan: ranked report of duplicate groups.
    Scan {
        /// Repository root or directory to scan.
        path: PathBuf,
        /// Output format: terminal | json | sarif | cpd | jscpd.
        #[arg(long, default_value = "terminal")]
        format: String,
        /// Show at most N groups in terminal output; 0 shows all
        /// (config: [report] top, default 20).
        #[arg(long)]
        top: Option<usize>,
        /// Also show weak-similarity findings and the full test/api-profile
        /// sections (all suspicion-only; never fail CI).
        #[arg(long)]
        verbose: bool,
    },
    /// PR mode: findings involving units changed since a git ref fail CI
    /// unless present-and-unworsened in the base ref's own scan; partial
    /// edits of known duplicate groups surface as inconsistent-update
    /// findings (spec §6). The base ref IS the baseline (D40): pin one in
    /// [baseline] ref for a persistent acceptance point.
    #[command(after_help = CHECK_EXIT_CODES)]
    Check {
        /// Repository root or directory to scan.
        path: PathBuf,
        /// Git ref to diff against and to scan for base state (e.g.
        /// origin/main, HEAD~1, a pinned tag/sha). Defaults to [baseline] ref.
        #[arg(long)]
        base: Option<String>,
        /// Minimum tier that fails CI (inconsistent-update, exact-normalized,
        /// internal-repeat, exact-region, near-normalized, inline-assisted,
        /// or none). Overrides [report] fail_on (default exact-normalized).
        #[arg(long)]
        fail_on: Option<String>,
        /// Output format: terminal | json | sarif | cpd | jscpd.
        #[arg(long, default_value = "terminal")]
        format: String,
        /// Dump each member's actual source under its file:line reference.
        #[arg(long)]
        verbose: bool,
    },
}

/// Exit-code epilogue for `check` help (spec §8 / DECISIONS.md D20f, D33).
const CHECK_EXIT_CODES: &str = "\
Exit codes:
  0  clean — no findings at or above --fail-on
  1  findings at or above --fail-on (CI should fail)
  2  usage or runtime error (bad flag, unknown format, missing git ref)";

/// Print to stdout, treating a closed pipe (`reprise scan | head`) as terminal. On a
/// broken pipe the process exits with `broken_pipe_code` — 0 for `scan` (always clean),
/// but the COMPUTED gate code for `check`, so `reprise check … | head` closing the pipe
/// can never silently mask a CI-failing gate (spec §8 exit contract).
fn emit(text: &str, broken_pipe_code: i32) -> anyhow::Result<()> {
    use std::io::Write;
    match std::io::stdout().write_all(text.as_bytes()) {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
            std::process::exit(broken_pipe_code)
        }
        result => Ok(result?),
    }
}

/// Exit codes (spec §8, DECISIONS.md D20f/D33): 0 clean, 1 `check` findings at
/// or above `fail_on`, 2 usage/runtime error. clap's own argument errors also
/// exit 2, so the three codes never collide.
fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::exit(2);
        }
    }
}

fn run() -> anyhow::Result<i32> {
    match Cli::parse().command {
        Command::Scan {
            path,
            format,
            top,
            verbose,
        } => {
            let config = reprise::Config::load(&path)?;
            let report = reprise::scan(&path, &config)?;
            // Instrumented builds only (`--features hash-counter`, never shipped): the
            // scan-wide node-hash count. Wall time on the dev box drifts ±50%
            // batch-to-batch; this count does not drift, so it — not a stopwatch — is the
            // primary evidence for a change to the hashing path.
            #[cfg(feature = "hash-counter")]
            eprintln!(
                "REPRISE_HASH_OPS {} AU_NODES {}",
                reprise::fingerprint::ops::global(),
                reprise::fingerprint::ops::au_nodes()
            );
            // `scan` always exits 0, so a broken pipe exits 0 too.
            match format.as_str() {
                "terminal" => emit(
                    &report.render_terminal(top.unwrap_or(config.report.top), verbose),
                    0,
                )?,
                "json" => emit(&format!("{}\n", serde_json::to_string_pretty(&report)?), 0)?,
                "sarif" => emit(
                    &format!(
                        "{}\n",
                        reprise::formats::sarif::scan_sarif(&report, &path, &config, verbose)
                    ),
                    0,
                )?,
                "cpd" => emit(&reprise::formats::cpd::scan_cpd(&report, &path, verbose), 0)?,
                "jscpd" => emit(
                    &format!(
                        "{}\n",
                        reprise::formats::jscpd::scan_jscpd(&report, &path, verbose)
                    ),
                    0,
                )?,
                other => {
                    bail!("unknown format `{other}` (expected: terminal, json, sarif, cpd, jscpd)")
                }
            }
            Ok(0)
        }
        Command::Check {
            path,
            base,
            fail_on,
            format,
            verbose,
        } => {
            let config = reprise::Config::load(&path)?;
            let base = base
                .or_else(|| config.baseline.pinned.clone())
                .ok_or_else(|| anyhow::anyhow!(
                    "no base ref: pass --base <ref> or pin one in reprise.toml ([baseline] ref = \"...\")"
                ))?;
            let report = reprise::check::run(&path, &config, &base, fail_on.as_deref())?;
            // Compute the gate code UP FRONT: exit 1 only for real findings, else 0
            // (errors above already mapped to 2). A broken pipe (`… | head`) must exit
            // with THIS code, not an unconditional 0 — else closing the pipe silently
            // passes a gate that should fail CI.
            let gate_code = if report.failed() { 1 } else { 0 };
            match format.as_str() {
                "terminal" => emit(&report.render_terminal(verbose), gate_code)?,
                "json" => emit(
                    &format!("{}\n", serde_json::to_string_pretty(&report)?),
                    gate_code,
                )?,
                "sarif" => emit(
                    &format!(
                        "{}\n",
                        reprise::formats::sarif::check_sarif(&report, &path, &config)
                    ),
                    gate_code,
                )?,
                "cpd" => emit(&reprise::formats::cpd::check_cpd(&report, &path), gate_code)?,
                "jscpd" => emit(
                    &format!("{}\n", reprise::formats::jscpd::check_jscpd(&report, &path)),
                    gate_code,
                )?,
                other => {
                    bail!("unknown format `{other}` (expected: terminal, json, sarif, cpd, jscpd)")
                }
            }
            Ok(gate_code)
        }
    }
}
