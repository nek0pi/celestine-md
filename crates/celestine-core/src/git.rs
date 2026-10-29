//! Optional Git integration for vaults that are repository roots.
//!
//! Shells out to the user's own `git` (not libgit2) so SSH keys, credential
//! helpers, signing, hooks and config behave exactly like in their terminal.
//! Every call is blocking; run them off the UI thread.

use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// Per-machine vault metadata that should never be committed.
pub const GITIGNORE_DEFAULTS: &str = ".celestine/recent-notes.toml\n.DS_Store\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitError {
    /// No `git` executable on `PATH`.
    NotInstalled,
    /// Pull stopped on conflicts; the rebase was aborted and local commits kept.
    Conflict(Vec<String>),
    Failed(String),
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GitError::NotInstalled => write!(f, "git is not installed"),
            GitError::Conflict(files) => {
                write!(f, "sync stopped: conflicting edits in {}", files.join(", "))
            }
            GitError::Failed(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for GitError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Repo-relative path with `/` separators.
    pub rel: String,
    pub kind: ChangeKind,
    /// Previous path of a rename.
    pub orig_rel: Option<String>,
}

impl ChangeKind {
    /// One-letter tag for compact lists.
    pub fn letter(self) -> char {
        match self {
            ChangeKind::Added => 'A',
            ChangeKind::Modified => 'M',
            ChangeKind::Deleted => 'D',
            ChangeKind::Renamed => 'R',
            ChangeKind::Untracked => 'U',
            ChangeKind::Conflicted => 'C',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Hunk,
    Context,
    Added,
    Removed,
    /// `\ No newline at end of file` and similar markers.
    Meta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// Line text without the leading `+`/`-`/space marker (hunk headers kept whole).
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileDiff {
    pub lines: Vec<DiffLine>,
    pub binary: bool,
    /// More lines existed than `DIFF_LINE_CAP`.
    pub truncated: bool,
}

/// Diffs beyond this many lines are cut off (the panel is a preview, not a pager).
pub const DIFF_LINE_CAP: usize = 3000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoOperation {
    Rebase,
    Merge,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoStatus {
    /// `None` when HEAD is detached.
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub changes: Vec<FileChange>,
    pub operation: Option<RepoOperation>,
}

impl RepoStatus {
    pub fn has_conflicts(&self) -> bool {
        self.changes.iter().any(|c| c.kind == ChangeKind::Conflicted)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    /// Number of files in the commit made before pulling (0 = nothing to commit).
    pub committed: usize,
    /// The pull brought in new commits (open notes may be stale).
    pub pulled: bool,
    pub pushed: bool,
    /// The repository has no remote, so only a local commit happened.
    pub no_remote: bool,
    /// Files whose contents the pull replaced in the work tree.
    pub changed_files: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PullOutcome {
    pub pulled: bool,
    pub changed_files: Vec<String>,
    /// The branch has never been pushed, so there is nothing to pull from.
    pub no_upstream: bool,
}

impl PullOutcome {
    pub fn summary(&self) -> String {
        if self.no_upstream {
            "git: nothing to pull — sync once to publish this branch".into()
        } else if self.pulled {
            let n = self.changed_files.len();
            format!("git: pulled changes to {n} file{}", if n == 1 { "" } else { "s" })
        } else {
            "git: already up to date".into()
        }
    }
}

impl SyncOutcome {
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.committed > 0 {
            parts.push(format!(
                "committed {} file{}",
                self.committed,
                if self.committed == 1 { "" } else { "s" }
            ));
        }
        if self.pulled {
            parts.push("pulled changes".into());
        }
        if self.pushed {
            parts.push("pushed".into());
        }
        if self.no_remote {
            parts.push("no remote configured".into());
        }
        if parts.is_empty() {
            "git: already up to date".into()
        } else {
            format!("git: {}", parts.join(" · "))
        }
    }
}

/// A vault whose root is the top level of a Git work tree.
#[derive(Debug, Clone)]
pub struct GitRepo {
    root: PathBuf,
    git_dir: PathBuf,
}

impl GitRepo {
    /// `Ok(None)` when the vault is not a repository root. A vault nested
    /// inside a larger repository (e.g. a dotfiles repo at `$HOME`) is
    /// deliberately ignored so Git UI only appears for intentional setups.
    pub fn detect(vault_root: &Path) -> Result<Option<Self>, GitError> {
        let out = run_git(
            vault_root,
            ["rev-parse", "--show-toplevel", "--absolute-git-dir"],
            &[],
        )?;
        if !out.status.success() {
            return Ok(None);
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut lines = text.lines();
        let (Some(top), Some(git_dir)) = (lines.next(), lines.next()) else {
            return Ok(None);
        };
        if !same_file::is_same_file(top, vault_root).unwrap_or(false) {
            return Ok(None);
        }
        Ok(Some(Self {
            root: vault_root.to_path_buf(),
            git_dir: PathBuf::from(git_dir),
        }))
    }

    /// `git init` the vault and add a `.gitignore` for per-machine metadata.
    pub fn init(vault_root: &Path) -> Result<Self, GitError> {
        let out = run_git(vault_root, ["init"], &[])?;
        check(&out)?;
        let ignore = vault_root.join(".gitignore");
        if !ignore.exists() {
            std::fs::write(&ignore, GITIGNORE_DEFAULTS)
                .map_err(|e| GitError::Failed(format!("could not write .gitignore: {e}")))?;
        }
        Self::detect(vault_root)?
            .ok_or_else(|| GitError::Failed("git init did not create a repository".into()))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn git<I, S>(&self, args: I) -> Result<Output, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        run_git(&self.root, args, &[])
    }

    fn git_ok<I, S>(&self, args: I) -> Result<String, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let out = self.git(args)?;
        check(&out)?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    pub fn status(&self) -> Result<RepoStatus, GitError> {
        // Background refreshes must not take index.lock away from the user's terminal.
        let out = run_git(
            &self.root,
            ["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all"],
            &[("GIT_OPTIONAL_LOCKS", "0")],
        )?;
        check(&out)?;
        let mut status = parse_status_v2(&out.stdout);
        status.operation = self.operation();
        Ok(status)
    }

    fn operation(&self) -> Option<RepoOperation> {
        if self.git_dir.join("rebase-merge").exists() || self.git_dir.join("rebase-apply").exists()
        {
            Some(RepoOperation::Rebase)
        } else if self.git_dir.join("MERGE_HEAD").exists() {
            Some(RepoOperation::Merge)
        } else {
            None
        }
    }

    /// Stage everything and commit. Returns how many files went in (0 = nothing to do).
    pub fn commit_all(&self, message: Option<&str>) -> Result<usize, GitError> {
        let status = self.status()?;
        ensure_clean_state(&status)?;
        if status.changes.is_empty() {
            return Ok(0);
        }
        let message = message
            .map(str::to_string)
            .unwrap_or_else(|| default_commit_message(&status.changes));
        self.git_ok(["add", "--all"])?;
        self.git_ok(["commit", "--quiet", "-m", &message])?;
        Ok(status.changes.len())
    }

    /// Commit local changes, pull with rebase, then push.
    pub fn sync(&self, message: Option<&str>) -> Result<SyncOutcome, GitError> {
        let status = self.status()?;
        ensure_clean_state(&status)?;
        if status.branch.is_none() {
            return Err(GitError::Failed(
                "HEAD is detached — check out a branch before syncing".into(),
            ));
        }
        let mut outcome = SyncOutcome {
            committed: self.commit_all(message)?,
            ..Default::default()
        };

        if status.upstream.is_none() {
            let Some(remote) = self.default_remote()? else {
                outcome.no_remote = true;
                return Ok(outcome);
            };
            if self.head().is_some() {
                self.git_ok(["push", "--quiet", "--set-upstream", &remote, "HEAD"])?;
                outcome.pushed = true;
            }
            return Ok(outcome);
        }

        (outcome.pulled, outcome.changed_files) = self.pull_rebase()?;

        let ahead = self
            .git_ok(["rev-list", "--count", "@{upstream}..HEAD"])?
            .parse::<u32>()
            .unwrap_or(0);
        if ahead > 0 {
            let push = self.git(["push", "--quiet"])?;
            if !push.status.success() {
                return Err(GitError::Failed(summarize_failure("push", &push)));
            }
            outcome.pushed = true;
        }
        Ok(outcome)
    }

    /// Pull with rebase without committing; local edits are stashed and restored.
    pub fn pull(&self) -> Result<PullOutcome, GitError> {
        let status = self.status()?;
        ensure_clean_state(&status)?;
        if status.branch.is_none() {
            return Err(GitError::Failed(
                "HEAD is detached — check out a branch before pulling".into(),
            ));
        }
        if status.upstream.is_none() {
            return Ok(PullOutcome {
                no_upstream: true,
                ..Default::default()
            });
        }
        let (pulled, changed_files) = self.pull_rebase()?;
        Ok(PullOutcome {
            pulled,
            changed_files,
            no_upstream: false,
        })
    }

    /// Returns whether upstream commits arrived and which files they changed.
    fn pull_rebase(&self) -> Result<(bool, Vec<String>), GitError> {
        let before = self.head();
        let pull = self.git(["pull", "--rebase", "--autostash", "--quiet"])?;
        if !pull.status.success() {
            if self.operation() == Some(RepoOperation::Rebase) {
                let conflicts = self
                    .status()
                    .map(|s| {
                        s.changes
                            .into_iter()
                            .filter(|c| c.kind == ChangeKind::Conflicted)
                            .map(|c| c.rel)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let _ = self.git(["rebase", "--abort"]);
                return Err(GitError::Conflict(conflicts));
            }
            return Err(GitError::Failed(summarize_failure("pull", &pull)));
        }
        let after = self.head();
        if after == before || !self.upstream_contains(before.as_deref()) {
            return Ok((false, Vec::new()));
        }
        let changed = match (&before, &after) {
            (Some(b), Some(a)) => self
                .git_ok(["diff", "--name-only", "--no-renames", b.as_str(), a.as_str()])?
                .lines()
                .map(str::to_string)
                .collect(),
            _ => self
                .git_ok(["ls-tree", "-r", "--name-only", "HEAD"])?
                .lines()
                .map(str::to_string)
                .collect(),
        };
        Ok((true, changed))
    }

    /// Changes in one file relative to the last commit.
    pub fn diff_file(&self, change: &FileChange) -> Result<FileDiff, GitError> {
        if change.kind == ChangeKind::Untracked || self.head().is_none() {
            return Ok(self.added_file_diff(&change.rel));
        }
        let mut args = vec![
            "--literal-pathspecs".to_string(),
            "diff".into(),
            "HEAD".into(),
            "--no-color".into(),
            "--no-ext-diff".into(),
            "-M".into(),
            "--".into(),
            change.rel.clone(),
        ];
        if let Some(orig) = &change.orig_rel {
            args.push(orig.clone());
        }
        let out = run_git(&self.root, &args, &[("GIT_OPTIONAL_LOCKS", "0")])?;
        check(&out)?;
        Ok(parse_unified_diff(&String::from_utf8_lossy(&out.stdout), DIFF_LINE_CAP))
    }

    /// A file git has no history for: every line is new.
    fn added_file_diff(&self, rel: &str) -> FileDiff {
        let Ok(bytes) = std::fs::read(self.root.join(rel)) else {
            return FileDiff::default();
        };
        if bytes.iter().take(8000).any(|b| *b == 0) {
            return FileDiff {
                binary: true,
                ..Default::default()
            };
        }
        let text = String::from_utf8_lossy(&bytes);
        let mut diff = FileDiff::default();
        for line in text.lines() {
            if diff.lines.len() >= DIFF_LINE_CAP {
                diff.truncated = true;
                break;
            }
            diff.lines.push(DiffLine {
                kind: DiffLineKind::Added,
                text: line.to_string(),
            });
        }
        diff
    }

    fn head(&self) -> Option<String> {
        self.git_ok(["rev-parse", "--verify", "--quiet", "HEAD"]).ok()
    }

    /// True when the rebase moved HEAD because upstream had new commits
    /// (not merely because our own commits were replayed onto the same base).
    fn upstream_contains(&self, before: Option<&str>) -> bool {
        let Some(before) = before else {
            return true;
        };
        self.git_ok(["rev-list", "--count", &format!("{before}..@{{upstream}}")])
            .map(|n| n != "0")
            .unwrap_or(true)
    }

    fn default_remote(&self) -> Result<Option<String>, GitError> {
        let remotes = self.git_ok(["remote"])?;
        let remotes: Vec<&str> = remotes.lines().map(str::trim).filter(|r| !r.is_empty()).collect();
        Ok(remotes
            .iter()
            .find(|r| **r == "origin")
            .or_else(|| remotes.first())
            .map(|r| r.to_string()))
    }
}

/// Whether a `git` executable can be run at all.
pub fn git_available() -> bool {
    run_git(Path::new("."), ["--version"], &[]).is_ok_and(|o| o.status.success())
}

fn ensure_clean_state(status: &RepoStatus) -> Result<(), GitError> {
    match status.operation {
        Some(RepoOperation::Rebase) => Err(GitError::Failed(
            "a rebase is in progress — finish or abort it in a terminal".into(),
        )),
        Some(RepoOperation::Merge) => Err(GitError::Failed(
            "a merge is in progress — finish or abort it in a terminal".into(),
        )),
        None if status.has_conflicts() => Err(GitError::Failed(
            "the repository has unresolved conflicts".into(),
        )),
        None => Ok(()),
    }
}

fn run_git<I, S>(dir: &Path, args: I, env: &[(&str, &str)]) -> Result<Output, GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        // Fail fast instead of waiting on a prompt nobody can see.
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            GitError::NotInstalled
        } else {
            GitError::Failed(format!("could not run git: {e}"))
        }
    })
}

fn check(out: &Output) -> Result<(), GitError> {
    if out.status.success() {
        Ok(())
    } else {
        Err(GitError::Failed(summarize_failure("git", out)))
    }
}

/// One human-readable line from a failed git invocation.
fn summarize_failure(what: &str, out: &Output) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lower = stderr.to_lowercase();
    if lower.contains("authentication failed")
        || lower.contains("could not read username")
        || lower.contains("permission denied (publickey")
        || lower.contains("terminal prompts disabled")
    {
        return format!("{what} failed: authentication — set up git credentials in a terminal");
    }
    if lower.contains("could not resolve host") || lower.contains("unable to access") {
        return format!("{what} failed: remote unreachable — check your connection");
    }
    if lower.contains("please tell me who you are") || lower.contains("user.email") {
        return "commit failed: set git user.name and user.email first".into();
    }
    if lower.contains("[rejected]") || lower.contains("non-fast-forward") {
        return format!("{what} rejected: remote has new commits — sync again");
    }
    let line = stderr
        .lines()
        .map(|l| l.trim().trim_start_matches("fatal:").trim_start_matches("error:").trim())
        .find(|l| !l.is_empty() && !l.starts_with("hint:"))
        .unwrap_or("unknown error");
    format!("{what} failed: {line}")
}

/// Short, readable message like `Update Ideas, Daily/2026-10-22 and 3 more`.
pub fn default_commit_message(changes: &[FileChange]) -> String {
    const SHOWN: usize = 3;
    let name = |rel: &str| {
        rel.strip_suffix(".md")
            .unwrap_or(rel)
            .trim_end_matches('/')
            .to_string()
    };
    if let [only] = changes {
        let verb = match only.kind {
            ChangeKind::Added | ChangeKind::Untracked => "Add",
            ChangeKind::Deleted => "Delete",
            ChangeKind::Renamed => "Rename to",
            ChangeKind::Modified | ChangeKind::Conflicted => "Update",
        };
        return format!("{verb} {}", name(&only.rel));
    }
    let shown: Vec<String> = changes.iter().take(SHOWN).map(|c| name(&c.rel)).collect();
    let rest = changes.len().saturating_sub(SHOWN);
    if rest == 0 {
        format!("Update {}", shown.join(", "))
    } else {
        format!("Update {} and {rest} more", shown.join(", "))
    }
}

/// Parse `git status --porcelain=v2 --branch -z`.
pub fn parse_status_v2(raw: &[u8]) -> RepoStatus {
    let text = String::from_utf8_lossy(raw);
    let mut records = text.split('\0').filter(|r| !r.is_empty());
    let mut status = RepoStatus::default();
    while let Some(rec) = records.next() {
        if let Some(header) = rec.strip_prefix("# ") {
            if let Some(head) = header.strip_prefix("branch.head ") {
                status.branch = (head != "(detached)").then(|| head.to_string());
            } else if let Some(up) = header.strip_prefix("branch.upstream ") {
                status.upstream = Some(up.to_string());
            } else if let Some(ab) = header.strip_prefix("branch.ab ") {
                for part in ab.split_whitespace() {
                    if let Some(n) = part.strip_prefix('+') {
                        status.ahead = n.parse().unwrap_or(0);
                    } else if let Some(n) = part.strip_prefix('-') {
                        status.behind = n.parse().unwrap_or(0);
                    }
                }
            }
            continue;
        }
        // The path is the last field; it may contain spaces, so split a fixed count.
        let fields = |n: usize| {
            let f: Vec<&str> = rec.splitn(n, ' ').collect();
            (f.len() == n).then_some(f)
        };
        let mut orig_rel = None;
        let (kind, rel) = match rec.as_bytes().first() {
            Some(b'1') => match fields(9) {
                Some(f) => (xy_kind(f[1]), f[8]),
                None => continue,
            },
            Some(b'2') => {
                // Renames carry the original path as an extra NUL-separated record.
                orig_rel = records.next().map(str::to_string);
                match fields(10) {
                    Some(f) => (ChangeKind::Renamed, f[9]),
                    None => continue,
                }
            }
            Some(b'u') => match fields(11) {
                Some(f) => (ChangeKind::Conflicted, f[10]),
                None => continue,
            },
            Some(b'?') => match rec.strip_prefix("? ") {
                Some(path) => (ChangeKind::Untracked, path),
                None => continue,
            },
            _ => continue,
        };
        status.changes.push(FileChange {
            rel: rel.to_string(),
            kind,
            orig_rel,
        });
    }
    status
}

/// Parse `git diff` output for a single file, skipping the per-file headers.
pub fn parse_unified_diff(raw: &str, cap: usize) -> FileDiff {
    let mut diff = FileDiff::default();
    let mut in_hunks = false;
    for line in raw.lines() {
        if !in_hunks {
            if line.starts_with("Binary files ") || line == "GIT binary patch" {
                diff.binary = true;
                return diff;
            }
            if !line.starts_with("@@") {
                continue;
            }
            in_hunks = true;
        }
        if diff.lines.len() >= cap {
            diff.truncated = true;
            break;
        }
        let (kind, text) = if line.starts_with("@@") {
            (DiffLineKind::Hunk, line)
        } else if let Some(t) = line.strip_prefix('+') {
            (DiffLineKind::Added, t)
        } else if let Some(t) = line.strip_prefix('-') {
            (DiffLineKind::Removed, t)
        } else if let Some(t) = line.strip_prefix(' ') {
            (DiffLineKind::Context, t)
        } else if line.starts_with('\\') {
            (DiffLineKind::Meta, line)
        } else if line.is_empty() {
            (DiffLineKind::Context, "")
        } else {
            continue;
        };
        diff.lines.push(DiffLine {
            kind,
            text: text.to_string(),
        });
    }
    diff
}

fn xy_kind(xy: &str) -> ChangeKind {
    if xy.contains('D') {
        ChangeKind::Deleted
    } else if xy.starts_with('A') {
        ChangeKind::Added
    } else {
        ChangeKind::Modified
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branch_headers_and_entries() {
        let raw = b"# branch.oid 1234\0# branch.head main\0# branch.upstream origin/main\0\
# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 aaa aaa Ideas.md\0\
1 A. N... 000000 100644 100644 000 bbb Daily/2026-10-22.md\0\
1 D. N... 100644 000000 000000 ccc 000 Old note.md\0\
2 R. N... 100644 100644 100644 ddd ddd R100 New name.md\0Old name.md\0\
u UU N... 100644 100644 100644 100644 e1 e2 e3 Clash.md\0\
? scratch/with space.md\0";
        let s = parse_status_v2(raw);
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.upstream.as_deref(), Some("origin/main"));
        assert_eq!((s.ahead, s.behind), (2, 1));
        let got: Vec<_> = s.changes.iter().map(|c| (c.rel.as_str(), c.kind)).collect();
        assert_eq!(
            got,
            vec![
                ("Ideas.md", ChangeKind::Modified),
                ("Daily/2026-10-22.md", ChangeKind::Added),
                ("Old note.md", ChangeKind::Deleted),
                ("New name.md", ChangeKind::Renamed),
                ("Clash.md", ChangeKind::Conflicted),
                ("scratch/with space.md", ChangeKind::Untracked),
            ]
        );
        assert!(s.has_conflicts());
        assert_eq!(s.changes[3].orig_rel.as_deref(), Some("Old name.md"));
    }

    #[test]
    fn detached_head_has_no_branch() {
        let s = parse_status_v2(b"# branch.oid abc\0# branch.head (detached)\0");
        assert_eq!(s.branch, None);
        assert!(s.changes.is_empty());
    }

    #[test]
    fn commit_message_names_a_few_notes() {
        let ch = |rel: &str, kind| FileChange {
            rel: rel.into(),
            kind,
            orig_rel: None,
        };
        assert_eq!(
            default_commit_message(&[ch("Ideas.md", ChangeKind::Untracked)]),
            "Add Ideas"
        );
        assert_eq!(
            default_commit_message(&[
                ch("a.md", ChangeKind::Modified),
                ch("b.md", ChangeKind::Modified),
                ch("c.png", ChangeKind::Added),
                ch("d.md", ChangeKind::Deleted),
                ch("e.md", ChangeKind::Deleted),
            ]),
            "Update a, b, c.png and 2 more"
        );
    }

    fn sh(dir: &Path, args: &[&str]) {
        let out = run_git(dir, args, &[]).unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn identity(dir: &Path) {
        sh(dir, &["config", "user.name", "Test"]);
        sh(dir, &["config", "user.email", "test@example.com"]);
        sh(dir, &["config", "commit.gpgsign", "false"]);
    }

    #[test]
    fn plain_folder_and_nested_vault_are_not_repos() {
        if !git_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        assert!(GitRepo::detect(dir.path()).unwrap().is_none());
        sh(dir.path(), &["init", "--quiet"]);
        let nested = dir.path().join("notes");
        std::fs::create_dir(&nested).unwrap();
        assert!(GitRepo::detect(&nested).unwrap().is_none());
        assert!(GitRepo::detect(dir.path()).unwrap().is_some());
    }

    #[test]
    fn init_commit_and_sync_round_trip() {
        if !git_available() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let remote = tmp.path().join("remote.git");
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::fs::create_dir_all(&remote).unwrap();
        std::fs::create_dir_all(&a).unwrap();
        sh(&remote, &["init", "--quiet", "--bare", "--initial-branch=main"]);

        let repo_a = GitRepo::init(&a).unwrap();
        identity(&a);
        sh(&a, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        assert!(std::fs::read_to_string(a.join(".gitignore"))
            .unwrap()
            .contains("recent-notes"));
        std::fs::write(a.join("Ideas.md"), "# Ideas").unwrap();

        // No remote yet: sync is a local commit only.
        let out = repo_a.sync(None).unwrap();
        assert!(out.no_remote && out.committed == 2, "{out:?}");
        assert!(repo_a.status().unwrap().changes.is_empty());

        // First sync with a remote sets the upstream.
        sh(&a, &["remote", "add", "origin", remote.to_str().unwrap()]);
        let out = repo_a.sync(None).unwrap();
        assert!(out.pushed, "{out:?}");
        assert_eq!(repo_a.status().unwrap().upstream.as_deref(), Some("origin/main"));

        sh(tmp.path(), &["clone", "--quiet", remote.to_str().unwrap(), "b"]);
        identity(&b);
        let repo_b = GitRepo::detect(&b).unwrap().unwrap();
        std::fs::write(b.join("From B.md"), "hello").unwrap();
        let out = repo_b.sync(None).unwrap();
        assert!(out.pushed && !out.pulled, "{out:?}");

        // A edits a different note and picks up B's commit in the same sync.
        std::fs::write(a.join("Ideas.md"), "# Ideas\nmore").unwrap();
        let out = repo_a.sync(Some("Expand ideas")).unwrap();
        assert!(out.committed == 1 && out.pulled && out.pushed, "{out:?}");
        assert_eq!(out.changed_files, vec!["From B.md".to_string()]);
        assert!(a.join("From B.md").is_file());
        let subject = run_git(&a, ["log", "-1", "--format=%s", "HEAD"], &[]).unwrap();
        assert_eq!(String::from_utf8_lossy(&subject.stdout).trim(), "Expand ideas");

        // Pull alone keeps uncommitted edits and reports what arrived.
        std::fs::write(b.join("From B.md"), "hello again").unwrap();
        repo_b.sync(None).unwrap();
        std::fs::write(a.join("Scratch.md"), "wip").unwrap();
        let out = repo_a.pull().unwrap();
        assert!(out.pulled, "{out:?}");
        assert_eq!(out.changed_files, vec!["From B.md".to_string()]);
        assert_eq!(std::fs::read_to_string(a.join("Scratch.md")).unwrap(), "wip");
        assert!(!repo_a.pull().unwrap().pulled);
        repo_a.sync(None).unwrap();

        // Same line edited on both sides: conflict is reported and rolled back.
        assert!(repo_b.sync(None).unwrap().pulled);
        std::fs::write(b.join("Ideas.md"), "# Ideas from B").unwrap();
        repo_b.sync(None).unwrap();
        std::fs::write(a.join("Ideas.md"), "# Ideas from A").unwrap();
        match repo_a.sync(None) {
            Err(GitError::Conflict(files)) => assert_eq!(files, vec!["Ideas.md".to_string()]),
            other => panic!("expected conflict, got {other:?}"),
        }
        let s = repo_a.status().unwrap();
        assert_eq!(s.operation, None);
        assert!(!s.has_conflicts());
        assert_eq!(std::fs::read_to_string(a.join("Ideas.md")).unwrap(), "# Ideas from A");
    }

    #[test]
    fn parses_unified_diff_hunks() {
        let raw = "diff --git a/n.md b/n.md\nindex 1..2 100644\n--- a/n.md\n+++ b/n.md\n\
@@ -1,2 +1,2 @@\n # Title\n-old\n+new\n\\ No newline at end of file\n";
        let d = parse_unified_diff(raw, DIFF_LINE_CAP);
        let kinds: Vec<_> = d.lines.iter().map(|l| (l.kind, l.text.as_str())).collect();
        assert_eq!(
            kinds,
            vec![
                (DiffLineKind::Hunk, "@@ -1,2 +1,2 @@"),
                (DiffLineKind::Context, "# Title"),
                (DiffLineKind::Removed, "old"),
                (DiffLineKind::Added, "new"),
                (DiffLineKind::Meta, "\\ No newline at end of file"),
            ]
        );
        assert!(!d.binary && !d.truncated);

        let bin = parse_unified_diff("diff --git a/x.png b/x.png\nBinary files a/x.png and b/x.png differ\n", 10);
        assert!(bin.binary && bin.lines.is_empty());

        let cut = parse_unified_diff("@@ -0,0 +1,3 @@\n+a\n+b\n+c\n", 2);
        assert!(cut.truncated && cut.lines.len() == 2);
    }

    #[test]
    fn diffs_modified_untracked_and_renamed_files() {
        if !git_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        sh(root, &["init", "--quiet"]);
        identity(root);
        std::fs::write(root.join("Keep.md"), "one\ntwo\n").unwrap();
        std::fs::write(root.join("Move me.md"), "same body\n").unwrap();
        let repo = GitRepo::detect(root).unwrap().unwrap();
        // Before the first commit everything is shown as added.
        let first = repo
            .diff_file(&repo.status().unwrap().changes[0])
            .unwrap();
        assert!(first.lines.iter().all(|l| l.kind == DiffLineKind::Added));
        repo.commit_all(None).unwrap();

        std::fs::write(root.join("Keep.md"), "one\nTWO\n").unwrap();
        std::fs::write(root.join("New.md"), "fresh\n").unwrap();
        sh(root, &["mv", "Move me.md", "Moved.md"]);
        let changes = repo.status().unwrap().changes;
        let find = |rel: &str| changes.iter().find(|c| c.rel == rel).unwrap().clone();

        let keep = repo.diff_file(&find("Keep.md")).unwrap();
        let edits: Vec<_> = keep
            .lines
            .iter()
            .filter(|l| matches!(l.kind, DiffLineKind::Added | DiffLineKind::Removed))
            .map(|l| (l.kind, l.text.as_str()))
            .collect();
        assert_eq!(
            edits,
            vec![(DiffLineKind::Removed, "two"), (DiffLineKind::Added, "TWO")]
        );

        let new = repo.diff_file(&find("New.md")).unwrap();
        assert_eq!(new.lines.len(), 1);
        assert_eq!(new.lines[0].kind, DiffLineKind::Added);

        let moved = find("Moved.md");
        assert_eq!(moved.kind, ChangeKind::Renamed);
        assert_eq!(moved.orig_rel.as_deref(), Some("Move me.md"));
        // A pure rename has no content hunks.
        let moved = repo.diff_file(&moved).unwrap();
        assert!(!moved.lines.iter().any(|l| l.kind == DiffLineKind::Removed), "{moved:?}");
    }
}
