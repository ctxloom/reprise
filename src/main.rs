use anyhow::bail;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

// On musl (the fully-static Linux release/`install-static` build) the default
// allocator is pathologically slow under reprise's rayon-parallel,
// allocation-heavy scan — ~16x vs glibc on the 500k perf corpus (77s vs 4.7s),
// which blows the ≤10s perf gate. mimalloc restores glibc-class performance.
// glibc and macOS builds keep the system allocator (already fast). Gated by
// target_env so it only affects musl, mirroring ripgrep. See DECISIONS.md D42.
#[cfg(target_env = "musl")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

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

/// Print to stdout, treating a closed pipe (`reprise scan | head`) as success.
fn emit(text: &str) -> anyhow::Result<()> {
    use std::io::Write;
    match std::io::stdout().write_all(text.as_bytes()) {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => std::process::exit(0),
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
            match format.as_str() {
                "terminal" => {
                    emit(&report.render_terminal(top.unwrap_or(config.report.top), verbose))?
                }
                "json" => emit(&format!("{}\n", serde_json::to_string_pretty(&report)?))?,
                "sarif" => emit(&format!(
                    "{}\n",
                    reprise::formats::sarif::scan_sarif(&report, &path, &config, verbose)
                ))?,
                "cpd" => emit(&reprise::formats::cpd::scan_cpd(&report, &path, verbose))?,
                "jscpd" => emit(&format!(
                    "{}\n",
                    reprise::formats::jscpd::scan_jscpd(&report, &path, verbose)
                ))?,
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
            match format.as_str() {
                "terminal" => emit(&report.render_terminal(verbose))?,
                "json" => emit(&format!("{}\n", serde_json::to_string_pretty(&report)?))?,
                "sarif" => emit(&format!(
                    "{}\n",
                    reprise::formats::sarif::check_sarif(&report, &path, &config)
                ))?,
                "cpd" => emit(&reprise::formats::cpd::check_cpd(&report, &path))?,
                "jscpd" => emit(&format!(
                    "{}\n",
                    reprise::formats::jscpd::check_jscpd(&report, &path)
                ))?,
                other => {
                    bail!("unknown format `{other}` (expected: terminal, json, sarif, cpd, jscpd)")
                }
            }
            // Exit 1 only for real findings; errors above already mapped to 2.
            Ok(if report.failed() { 1 } else { 0 })
        }
    }
}
