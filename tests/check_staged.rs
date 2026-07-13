//! `reprise check` must judge THE COMMIT, not the checkout.
//!
//! A pre-commit hook runs against a working tree that, on a shared or
//! multi-agent checkout, contains edits nobody is committing. `check` currently
//! resolves both its changed-unit set (`git diff` with no `--cached`) and its
//! file CONTENT (a filesystem walk) from that working tree — so it reports on
//! code that is not in the prospective commit, and fails it.
//!
//! Field instance: a commit of five staged files was blocked by findings in two
//! files that were merely dirty in the tree, left by a different session.
//!
//! The contract these tests pin: the bytes `check` judges are THE STAGED BYTES
//! (the git index), and nothing else.

use reprise::config::Config;
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

// ---------- corpus ----------

/// Copy `n` of a ~50-token function family: same normalized structure,
/// different locals and literals — a Type-2 clone family that converges on the
/// exact-normalized tier, so all copies land in ONE group.
fn family_member(n: usize) -> String {
    let names = ["entries", "items", "rows", "records"];
    let acc = ["total", "acc", "sum", "tally"];
    let lit = [90, 85, 70, 60];
    format!(
        r#"
pub fn process_batch_{n}({xs}: &[(String, i64)], cutoff: i64) -> Vec<String> {{
    let mut out = Vec::new();
    let mut {acc} = 0;
    for (label, score) in {xs} {{
        if *score < cutoff {{
            continue;
        }}
        {acc} += score;
        let grade = if *score >= {lit} {{ "high" }} else {{ "low" }};
        out.push(format!("{{label}}: {{score}} ({{grade}})"));
    }}
    if {acc} > {lit2} {{
        out.push(String::from("aggregate: high"));
    }}
    out
}}
"#,
        xs = names[n % 4],
        acc = acc[n % 4],
        lit = lit[n % 4],
        lit2 = lit[n % 4] * 5,
    )
}

/// The drift edit: adds a guard clause to ONE member of the family. Applied to
/// a single copy, this is the `inconsistent-update` finding — the one that
/// makes `check` fail.
fn drifted_member(n: usize) -> String {
    family_member(n).replace(
        "let mut out = Vec::new();",
        "let mut out = Vec::new();\n    if cutoff < 0 {\n        return out;\n    }",
    )
}

const UNRELATED: &str = r#"
pub fn parse_header_line(line: &str) -> Option<(String, String)> {
    let mut parts = line.splitn(2, ':');
    let key = parts.next()?.trim().to_string();
    let value = parts.next()?.trim().to_string();
    if key.is_empty() { None } else { Some((key, value)) }
}
"#;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A repo whose HEAD commit holds a 3-member clone family. HEAD is the base
/// ref: any drift introduced on top of it is what `check` weighs.
fn repo_with_committed_family() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    for n in 0..3 {
        fs::write(root.join(format!("m{n}.rs")), family_member(n)).unwrap();
    }
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "t@example.com"]);
    git(root, &["config", "user.name", "t"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "the family"]);
    dir
}

// ---------- the contract ----------

/// THE FIELD BUG. Another session's UNSTAGED edit drifts one copy of the
/// family. My commit stages an unrelated new file and nothing else. The drift
/// is not in my commit, so it must not fail my commit.
///
/// Today this FAILS: `git diff` without `--cached` sees the dirty worktree, and
/// the scan reads worktree bytes, so the un-staged drift is reported as mine.
#[test]
fn an_unstaged_edit_does_not_fail_a_commit_that_does_not_contain_it() {
    let dir = repo_with_committed_family();
    let root = dir.path();

    // A concurrent session dirties the tree. NOT staged.
    fs::write(root.join("m0.rs"), drifted_member(0)).unwrap();

    // My commit: one unrelated file, staged.
    fs::write(root.join("headers.rs"), UNRELATED).unwrap();
    git(root, &["add", "headers.rs"]);

    let report = reprise::check::run(root, &Config::default(), "HEAD", None).unwrap();
    assert!(
        !report.failed(),
        "check failed a commit over an UNSTAGED edit it does not contain — \
         the m0.rs drift is not staged. Findings: {:#?}",
        report.findings
    );
}

/// The converse, and the one that matters for correctness: a finding that IS
/// staged must fail, EVEN THOUGH the working-tree copy of that same file is
/// clean.
///
/// This is the test that forbids the cheap fix. Merely filtering findings to
/// the staged FILE LIST while still reading worktree CONTENT would pass the
/// test above and fail this one: it would fingerprint bytes that are not being
/// committed. `check` must read the INDEX.
#[test]
fn a_staged_edit_fails_even_when_the_working_tree_copy_is_clean() {
    let dir = repo_with_committed_family();
    let root = dir.path();

    // Stage the drift...
    fs::write(root.join("m0.rs"), drifted_member(0)).unwrap();
    git(root, &["add", "m0.rs"]);

    // ...then revert the WORKING TREE copy to the pristine family member.
    // The index holds drift; the checkout on disk does not.
    fs::write(root.join("m0.rs"), family_member(0)).unwrap();

    let report = reprise::check::run(root, &Config::default(), "HEAD", None).unwrap();
    assert!(
        report.failed(),
        "check passed a STAGED inconsistent-update because the worktree copy of \
         m0.rs looked clean — it is judging the checkout, not the commit. \
         Findings: {:#?}",
        report.findings
    );
}

/// REPORT-ONLY means report-only: `check` must not leave git admin state behind
/// in the repository it scans.
///
/// `BaseWorktree` runs `git worktree add` INSIDE the scanned repo and cleans up
/// in `Drop` — which does not run when the process is SIGKILLed, and an
/// OOM-killed scan is exactly that. Reading base state from the object store
/// creates no worktree at all, so there is nothing to leak.
#[test]
fn check_registers_no_worktree_in_the_scanned_repo() {
    let dir = repo_with_committed_family();
    let root = dir.path();

    fs::write(root.join("m0.rs"), drifted_member(0)).unwrap();
    git(root, &["add", "m0.rs"]);
    let _ = reprise::check::run(root, &Config::default(), "HEAD", None).unwrap();

    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .expect("git runs");
    let listed = String::from_utf8_lossy(&out.stdout);
    let worktrees = listed
        .lines()
        .filter(|l| l.starts_with("worktree "))
        .count();
    assert_eq!(
        worktrees, 1,
        "check registered a worktree in the scanned repo (expected only the main \
         checkout). A SIGKILL mid-scan would strand this in the user's .git:\n{listed}"
    );
}

// ---------- the base IS the PR base (spec §2: "PR mode") ----------

/// `check` is a PR check: "what does my branch add, relative to the default
/// branch?" So a bare `reprise check .` — no `--base`, no pinned ref — must
/// resolve the base the way the servers already do
/// (`reprise::baseline::resolve_base`): explicit → pinned → **merge-base with
/// the default branch** → HEAD.
///
/// It used to be a hard error ("no base ref: pass --base"), which is why the one
/// deployment that mattered — ctxloom's pre-commit hook — pinned `--base HEAD`
/// and silently became a PER-COMMIT gate instead of a PR gate: a duplicate
/// introduced in branch commit 1 stops being "touched" by commit 2, passes the
/// local hook, and then fails in CI, which diffs against the merge-base.
#[test]
fn a_bare_check_defaults_to_the_merge_base_with_the_default_branch() {
    let dir = repo_with_committed_family();
    let root = dir.path();

    // A feature branch whose COMMIT drifts one member of the family. This is
    // the PR. Nothing is staged and the worktree is clean — the drift lives in
    // the branch's history, which is exactly what a PR check must weigh.
    git(root, &["checkout", "-q", "-b", "feature"]);
    fs::write(root.join("m0.rs"), drifted_member(0)).unwrap();
    git(root, &["add", "m0.rs"]);
    git(root, &["commit", "-qm", "guard clause on m0 only"]);

    let out = Command::new(env!("CARGO_BIN_EXE_reprise"))
        .args(["check"])
        .arg(root)
        .output()
        .expect("reprise runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        !stderr.contains("no base ref"),
        "bare `check` still errors instead of resolving the PR base: {stderr}"
    );
    assert!(
        stdout.contains("inconsistent-update"),
        "bare `check` did not gate the branch against the default branch — the \
         committed drift on `feature` should be an inconsistent-update vs the \
         merge-base with `master`/`main`.\nstdout: {stdout}\nstderr: {stderr}"
    );
}
