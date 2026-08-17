//! The ctxloom family's companion-version contract.
//!
//! ctxloom probes every companion binary at boot with exactly
//! `<binary> version --format json` and parses STDOUT as {"name","version"}.
//! The probe reads the whole stream, so any extra byte — a banner, a log line,
//! a progress note — breaks it and the parent prints a warning on every run.
//! These tests therefore assert the BYTES and the parsed fields, never merely
//! that the command exited 0: a `version` that exits 0 while printing the wrong
//! thing is the exact failure this subcommand was added to fix.

use std::process::Command;

fn reprise(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_reprise"))
        .args(args)
        .output()
        .expect("reprise runs")
}

#[test]
fn version_json_is_the_companion_probe_shape_and_nothing_else() {
    let out = reprise(&["version", "--format", "json"]);

    assert_eq!(
        out.status.code(),
        Some(0),
        "`version --format json` must exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8(out.stdout).expect("stdout is utf-8");
    // The WHOLE stream, not a line picked out of it — that is what ctxloom parses.
    let info: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!("stdout is not one JSON document ({e}); stdout was: {stdout:?}")
    });

    assert_eq!(
        info.get("name").and_then(|v| v.as_str()),
        Some("reprise"),
        "probe shape requires name == \"reprise\"; got {info}"
    );

    let version = info
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("`version` must be a JSON string; got {info}"));
    assert!(!version.is_empty(), "`version` must not be empty");
    assert_eq!(
        version,
        env!("CARGO_PKG_VERSION"),
        "`version` must report the crate version, not some other value"
    );
}

#[test]
fn version_terminal_agrees_with_clap_s_own_version_flag() {
    let sub = reprise(&["version"]);
    assert_eq!(sub.status.code(), Some(0));
    let sub_out = String::from_utf8(sub.stdout).expect("stdout is utf-8");

    let flag = reprise(&["--version"]);
    let flag_out = String::from_utf8(flag.stdout).expect("stdout is utf-8");

    assert!(!sub_out.trim().is_empty(), "`version` printed nothing");
    assert_eq!(
        sub_out, flag_out,
        "`reprise version` and `reprise --version` must not word the same fact differently"
    );
    assert!(
        sub_out.contains(env!("CARGO_PKG_VERSION")),
        "human line omits the version: {sub_out:?}"
    );
}

#[test]
fn version_default_format_is_terminal_not_json() {
    let bare = reprise(&["version"]);
    let explicit = reprise(&["version", "--format", "terminal"]);
    assert_eq!(bare.stdout, explicit.stdout);
    assert!(
        serde_json::from_slice::<serde_json::Value>(&bare.stdout).is_err(),
        "bare `version` must be the human rendering, not JSON"
    );
}

#[test]
fn version_unknown_format_is_a_usage_error() {
    let out = reprise(&["version", "--format", "yaml"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "unknown --format must exit 2 (usage/runtime error), per the exit-code contract"
    );
    assert!(
        out.stdout.is_empty(),
        "a usage error must not write to stdout; got {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}
