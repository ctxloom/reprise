use anyhow::bail;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "reprise",
    version,
    about = "Code duplicate detection: scan, baseline, and PR-time drift gating"
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
    /// unless baselined and unworsened; partial edits of known duplicate
    /// groups surface as inconsistent-update findings (spec §6).
    #[command(after_help = CHECK_EXIT_CODES)]
    Check {
        /// Repository root or directory to scan.
        path: PathBuf,
        /// Git ref to diff against (e.g. origin/main, HEAD~1). Only units
        /// whose line span intersects the diff are gated.
        #[arg(long)]
        base: String,
        /// Minimum tier that fails CI (inconsistent-update, exact-normalized,
        /// internal-repeat, exact-region, near-normalized, inline-assisted,
        /// or none). Overrides [report] fail_on (default exact-normalized).
        #[arg(long)]
        fail_on: Option<String>,
        /// Output format: terminal | json | sarif | cpd | jscpd.
        #[arg(long, default_value = "terminal")]
        format: String,
    },
    /// Write all current findings, keyed by stable structural fingerprints,
    /// to a checked-in baseline (the legacy-repo adoption path, spec §2).
    /// Path/name: [baseline] file, default reprise-baseline.json.
    Baseline {
        /// Repository root or directory to baseline.
        path: PathBuf,
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
        } => {
            let config = reprise::Config::load(&path)?;
            let report = reprise::check::run(&path, &config, &base, fail_on.as_deref())?;
            match format.as_str() {
                "terminal" => emit(&report.render_terminal())?,
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
        Command::Baseline { path } => {
            let config = reprise::Config::load(&path)?;
            let report = reprise::scan(&path, &config)?;
            let baseline = reprise::baseline::create(&report, &path);
            let file = reprise::baseline::baseline_path(&path, &config);
            baseline.save(&file)?;
            emit(&format!(
                "baseline written: {} findings ({} main, {} test, {} api, {} weak) to {}\n",
                baseline.findings.len(),
                report.groups.len(),
                report.test_groups.len(),
                report.api_groups.len(),
                report.weak_groups.len(),
                file.display(),
            ))?;
            Ok(0)
        }
    }
}
