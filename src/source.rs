//! Where a scan's file content comes from.
//!
//! `scan` reads a filesystem tree. `check` must read **the commit** — the git
//! index (what is about to be committed) or a base ref — and it must do so
//! WITHOUT materialising either as a checkout. Both requirements are the same
//! requirement: make the content source an abstraction instead of a directory.
//!
//! Why this is not merely a convenience:
//!
//! - **Correctness.** A pre-commit hook on a shared checkout sees edits nobody
//!   is committing. Reading the worktree makes `check` fail commits over other
//!   sessions' work-in-progress (`inconsistent-update` on a file the committer
//!   never touched). The bytes judged must be the bytes committed.
//! - **Report-only.** The previous base-state path ran `git worktree add` INSIDE
//!   the scanned repo and cleaned up in `Drop` — which does not run on SIGKILL,
//!   and an OOM-killed scan is exactly that. Reading blobs from the object store
//!   creates nothing to leak, so the guarantee is structural, not best-effort.
//!   It also removes the need for the `git archive` fallback (D41): object reads
//!   work on every checkout layout, which is what that fallback existed to
//!   paper over.

use crate::config::Config;
use crate::lang::Lang;
use crate::walk;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// The two places file content enters a scan: enumeration and read. Every
/// consumer of source text goes through one of them, so an implementation of
/// this trait fully determines what a scan sees.
pub trait ContentSource: Sync {
    /// The scan root. Paths from [`files`](Self::files) live under it, and unit
    /// identity (`Unit::file`, cache keys, report paths) is relative to it — so
    /// a git source and a filesystem source over the same repo produce
    /// identical unit identities.
    fn root(&self) -> &Path;

    /// The files to extract, sorted, already filtered by language and by
    /// `config`'s excludes.
    fn files(&self, config: &Config) -> anyhow::Result<Vec<(PathBuf, Lang)>>;

    /// The text of a file that [`files`](Self::files) listed. `None` when it
    /// cannot be read as UTF-8 — the caller counts that as `files_unreadable`,
    /// exactly as a failed `read_to_string` always did.
    fn read(&self, path: &Path) -> Option<String>;
}

/// The live checkout: what `scan` has always read.
pub struct FsSource<'a> {
    root: &'a Path,
}

impl<'a> FsSource<'a> {
    pub fn new(root: &'a Path) -> Self {
        FsSource { root }
    }
}

impl ContentSource for FsSource<'_> {
    fn root(&self) -> &Path {
        self.root
    }

    fn files(&self, config: &Config) -> anyhow::Result<Vec<(PathBuf, Lang)>> {
        walk::collect_files(self.root, config)
    }

    fn read(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }
}

/// Which git tree to read.
pub enum GitRev {
    /// The index — **what is about to be committed**. This is what a pre-commit
    /// `check` must judge.
    Index,
    /// A committed tree (`check`'s base ref).
    Ref(String),
}

/// A tree read straight out of the git object store — no checkout, no worktree,
/// no temp directory, and so nothing left behind in the scanned repo.
///
/// Blob text is materialised in memory up front (one `git cat-file --batch`
/// pass), because extraction runs in parallel across rayon workers and a shared
/// long-lived `cat-file` pipe would have to be serialised behind a lock anyway.
/// The scan already holds every file's text transiently during extraction, so
/// this is not a new order of memory.
pub struct GitSource {
    root: PathBuf,
    blobs: HashMap<PathBuf, String>,
}

/// Git's mode for a regular file / an executable regular file. Anything else in
/// a tree listing is a symlink (120000) or a submodule gitlink (160000): the
/// filesystem walk never yields those as source files, and neither do we.
const REGULAR_MODES: [&str; 2] = ["100644", "100755"];

impl GitSource {
    pub fn new(root: &Path, rev: GitRev, config: &Config) -> anyhow::Result<Self> {
        let entries = list_entries(root, &rev)?;
        let overrides = walk::exclude_overrides(root, config)?;

        // The same filters the walk applies, in the same order: language first
        // (cheap), then the config excludes. Gitignore needs no handling here —
        // a git tree contains only tracked files by construction.
        let wanted: Vec<(PathBuf, String)> = entries
            .into_iter()
            .filter(|(rel, _)| Lang::from_path(rel).is_some())
            .filter(|(rel, _)| !overrides.matched(rel, false).is_ignore())
            .collect();

        let texts = cat_blobs(root, wanted.iter().map(|(_, sha)| sha.as_str()))?;

        let mut blobs = HashMap::new();
        for ((rel, _), text) in wanted.into_iter().zip(texts) {
            // A non-UTF-8 blob is unreadable, exactly as it is on the fs path.
            if let Some(text) = text {
                blobs.insert(root.join(rel), text);
            }
        }
        Ok(GitSource {
            root: root.to_path_buf(),
            blobs,
        })
    }
}

impl ContentSource for GitSource {
    fn root(&self) -> &Path {
        &self.root
    }

    fn files(&self, _config: &Config) -> anyhow::Result<Vec<(PathBuf, Lang)>> {
        // Excludes were applied when the tree was listed; every retained blob is
        // a file we want. Sorted to match the filesystem walk's ordering, so unit
        // order — and therefore report order — does not depend on the source.
        let mut files: Vec<(PathBuf, Lang)> = self
            .blobs
            .keys()
            .filter_map(|p| Lang::from_path(p).map(|l| (p.clone(), l)))
            .collect();
        files.sort();
        Ok(files)
    }

    fn read(&self, path: &Path) -> Option<String> {
        self.blobs.get(path).cloned()
    }
}

/// `(relative path, blob sha)` for every regular file in the tree.
fn list_entries(root: &Path, rev: &GitRev) -> anyhow::Result<Vec<(PathBuf, String)>> {
    // -z: NUL-terminated records with paths emitted RAW. Without it git quotes
    // and escapes any path with a special character, and we would have to unquote
    // it correctly to find the file again.
    let out = match rev {
        GitRev::Index => crate::check::git_cmd(root)
            .args(["ls-files", "--stage", "-z"])
            .output()?,
        GitRev::Ref(r) => crate::check::git_cmd(root)
            .args(["ls-tree", "-r", "-z"])
            .arg(r)
            .output()?,
    };
    anyhow::ensure!(
        out.status.success(),
        "listing the git tree failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The two listings put the sha in DIFFERENT columns, and both have exactly
    // three fields — so the layout cannot be inferred from the field count, it
    // must be keyed off the command that produced it:
    //   ls-files --stage: "<mode> <sha> <stage>\t<path>"
    //   ls-tree -r:       "<mode> blob <sha>\t<path>"
    // (Guessing here fed the literal string "blob" to `cat-file` as if it were a
    // sha, and every base-ref scan died with "blob missing".)
    let sha_col = match rev {
        GitRev::Index => 1,
        GitRev::Ref(_) => 2,
    };

    let mut entries = Vec::new();
    for record in out.stdout.split(|b| *b == 0) {
        if record.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(record);
        let Some((meta, path)) = text.split_once('\t') else {
            continue;
        };
        let fields: Vec<&str> = meta.split_whitespace().collect();
        let (Some(mode), Some(sha)) = (fields.first(), fields.get(sha_col)) else {
            continue;
        };
        if !REGULAR_MODES.contains(mode) {
            continue;
        }
        entries.push((PathBuf::from(path), (*sha).to_string()));
    }
    Ok(entries)
}

/// Read every blob in one `git cat-file --batch` pass. Order of the returned
/// texts matches the order of `shas`; `None` marks a blob that is not valid
/// UTF-8 (a binary file that happens to carry a source extension).
fn cat_blobs<'a>(
    root: &Path,
    shas: impl Iterator<Item = &'a str>,
) -> anyhow::Result<Vec<Option<String>>> {
    let shas: Vec<&str> = shas.collect();
    if shas.is_empty() {
        return Ok(Vec::new());
    }

    let mut child = crate::check::git_cmd(root)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // Feed every sha, then close stdin so git finishes. Writing from a thread
    // avoids the classic pipe deadlock: git blocks writing its output once the
    // OS pipe buffer fills, and would never drain our stdin if we wrote it all
    // before reading a byte.
    let mut stdin = child.stdin.take().expect("stdin piped");
    let query: String = shas.iter().map(|s| format!("{s}\n")).collect();
    let writer = std::thread::spawn(move || stdin.write_all(query.as_bytes()));

    let out = child.wait_with_output()?;
    writer.join().ok();
    anyhow::ensure!(
        out.status.success(),
        "git cat-file failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Response per query: "<sha> <type> <size>\n<size bytes>\n"
    let mut texts = Vec::with_capacity(shas.len());
    let buf = out.stdout;
    let mut at = 0usize;
    for _ in 0..shas.len() {
        let Some(nl) = buf[at..].iter().position(|b| *b == b'\n') else {
            anyhow::bail!("git cat-file: truncated response header");
        };
        let header = String::from_utf8_lossy(&buf[at..at + nl]).to_string();
        at += nl + 1;
        let size: usize = header
            .rsplit(' ')
            .next()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("git cat-file: bad response header `{header}`"))?;
        anyhow::ensure!(
            at + size <= buf.len(),
            "git cat-file: response shorter than its declared size"
        );
        texts.push(String::from_utf8(buf[at..at + size].to_vec()).ok());
        at += size + 1; // payload, then git's trailing newline
    }
    Ok(texts)
}
