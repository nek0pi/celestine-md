//! Background runner for vault Git work (detect, status, diff, commit, sync, pull, init).
//!
//! Stays completely inert unless the open vault is the root of a Git repository
//! and the user has Git integration enabled.

use celestine_core::git::{
    default_commit_message, FileDiff, GitError, GitRepo, PullOutcome, RepoStatus, SyncOutcome,
};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

/// Catch commits / checkouts made in a terminal (the watcher ignores `.git/`).
const IDLE_REFRESH: Duration = Duration::from_secs(30);
/// Coalesce bursts of note saves into one `git status`.
pub const SAVE_REFRESH: Duration = Duration::from_millis(1200);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitJob {
    Detect,
    Refresh,
    Commit,
    Sync,
    Pull,
    Init,
}

enum Reply {
    Detected(Result<Option<GitRepo>, GitError>),
    Status(Result<RepoStatus, GitError>),
    Diff(String, Result<FileDiff, GitError>),
    Committed(Result<usize, GitError>),
    Synced(Result<SyncOutcome, GitError>),
    Pulled(Result<PullOutcome, GitError>),
    Initialized(Result<GitRepo, GitError>),
}

/// What the app should do after a finished job.
pub enum GitEvent {
    Message(String),
    /// A pull replaced these vault-relative files in the work tree.
    Pulled(Vec<String>),
}

pub struct VaultGit {
    root: Option<PathBuf>,
    repo: Option<GitRepo>,
    status: Option<RepoStatus>,
    git_missing: bool,
    /// Outcome of the last user-facing operation (or a background failure).
    last_result: Option<(Instant, Result<String, String>)>,
    busy: Option<GitJob>,
    /// File whose diff the panel wants (`None` while the panel is closed).
    diff_wanted: Option<String>,
    diff_in_flight: Option<String>,
    diff: Option<(String, Result<FileDiff, String>)>,
    /// The work tree may have changed since `diff` was computed.
    diff_stale: bool,
    /// Bumped on every attach so replies for a previous vault are dropped.
    generation: u64,
    refresh_at: Option<Instant>,
    tx: Sender<(u64, Reply)>,
    rx: Receiver<(u64, Reply)>,
    ctx: Option<egui::Context>,
}

impl Default for VaultGit {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            root: None,
            repo: None,
            status: None,
            git_missing: false,
            last_result: None,
            busy: None,
            diff_wanted: None,
            diff_in_flight: None,
            diff: None,
            diff_stale: false,
            generation: 0,
            refresh_at: None,
            tx,
            rx,
            ctx: None,
        }
    }
}

impl VaultGit {
    /// Point at a vault (or none). Detection runs in the background.
    pub fn attach(&mut self, root: Option<&Path>, enabled: bool, ctx: &egui::Context) {
        *self = Self {
            generation: self.generation + 1,
            ..Self::default()
        };
        self.root = root.filter(|_| enabled).map(Path::to_path_buf);
        self.ctx = Some(ctx.clone());
        if let Some(root) = self.root.clone() {
            self.spawn(GitJob::Detect, move || {
                Reply::Detected(GitRepo::detect(&root))
            });
        }
    }

    pub fn repo(&self) -> Option<&GitRepo> {
        self.repo.as_ref()
    }

    pub fn status(&self) -> Option<&RepoStatus> {
        self.status.as_ref()
    }

    pub fn last_error(&self) -> Option<&str> {
        match &self.last_result {
            Some((_, Err(e))) => Some(e),
            _ => None,
        }
    }

    /// Last outcome with its age, for the panel's result line.
    pub fn last_result(&self) -> Option<(Duration, Result<&str, &str>)> {
        self.last_result
            .as_ref()
            .map(|(at, r)| (at.elapsed(), r.as_deref().map_err(String::as_str)))
    }

    /// A Git message was shown within `window` (keep it in the status bar).
    pub fn recently_reported(&self, window: Duration) -> bool {
        self.last_result
            .as_ref()
            .is_some_and(|(at, _)| at.elapsed() < window)
    }

    /// A user-facing operation (not a background refresh) is running.
    pub fn is_working(&self) -> bool {
        matches!(
            self.busy,
            Some(GitJob::Commit | GitJob::Sync | GitJob::Pull | GitJob::Init)
        )
    }

    pub fn working_label(&self) -> Option<&'static str> {
        match self.busy? {
            GitJob::Commit => Some("committing…"),
            GitJob::Sync => Some("syncing…"),
            GitJob::Pull => Some("pulling…"),
            GitJob::Init => Some("initializing…"),
            GitJob::Detect | GitJob::Refresh => None,
        }
    }

    pub fn git_missing(&self) -> bool {
        self.git_missing
    }

    /// Integration is on, Git is installed, and the vault is not yet a repository.
    pub fn can_init(&self) -> bool {
        self.root.is_some() && self.repo.is_none() && !self.git_missing && self.busy.is_none()
    }

    pub fn request_refresh(&mut self, after: Duration) {
        if self.repo.is_none() {
            return;
        }
        let at = Instant::now() + after;
        self.refresh_at = Some(self.refresh_at.map_or(at, |cur| cur.min(at)));
    }

    /// Message the commit would get right now if the user doesn't write one.
    pub fn suggested_message(&self) -> String {
        self.status
            .as_ref()
            .map(|s| default_commit_message(&s.changes))
            .unwrap_or_default()
    }

    /// Ask for `rel`'s diff (or stop diffing with `None`). Fetched in `poll`.
    pub fn select_diff(&mut self, rel: Option<&str>) {
        if self.diff_wanted.as_deref() != rel {
            self.diff_wanted = rel.map(str::to_string);
        }
    }

    /// The diff for the selected file, once loaded.
    pub fn diff(&self) -> Option<&Result<FileDiff, String>> {
        let (rel, diff) = self.diff.as_ref()?;
        (Some(rel) == self.diff_wanted.as_ref()).then_some(diff)
    }

    pub fn start_commit(&mut self, message: Option<String>) -> Result<(), String> {
        let repo = self.ready_repo()?;
        self.spawn(GitJob::Commit, move || {
            Reply::Committed(repo.commit_all(message.as_deref()))
        });
        Ok(())
    }

    pub fn start_sync(&mut self, message: Option<String>) -> Result<(), String> {
        let repo = self.ready_repo()?;
        self.spawn(GitJob::Sync, move || Reply::Synced(repo.sync(message.as_deref())));
        Ok(())
    }

    pub fn start_pull(&mut self) -> Result<(), String> {
        let repo = self.ready_repo()?;
        self.spawn(GitJob::Pull, move || Reply::Pulled(repo.pull()));
        Ok(())
    }

    pub fn start_init(&mut self) -> Result<(), String> {
        if !self.can_init() {
            return Err(if self.git_missing {
                "git is not installed".into()
            } else if self.repo.is_some() {
                "vault is already a git repository".into()
            } else {
                "git integration is off or busy".into()
            });
        }
        let root = self.root.clone().expect("can_init checked root");
        self.spawn(GitJob::Init, move || Reply::Initialized(GitRepo::init(&root)));
        Ok(())
    }

    fn ready_repo(&self) -> Result<GitRepo, String> {
        let Some(repo) = self.repo.clone() else {
            return Err("vault is not a git repository".into());
        };
        if self.is_working() {
            return Err("git is already working…".into());
        }
        // A refresh in flight is harmless; its reply only updates status.
        Ok(repo)
    }

    fn spawn(&mut self, job: GitJob, work: impl FnOnce() -> Reply + Send + 'static) {
        if self.run_in_background(work) {
            self.busy = Some(job);
        }
    }

    fn run_in_background(&self, work: impl FnOnce() -> Reply + Send + 'static) -> bool {
        let tx = self.tx.clone();
        let generation = self.generation;
        let ctx = self.ctx.clone();
        thread::Builder::new()
            .name("celestine-git".into())
            .spawn(move || {
                let _ = tx.send((generation, work()));
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            })
            .is_ok()
    }

    /// Drain finished jobs and kick off due refreshes / diffs. Call once per frame.
    pub fn poll(&mut self) -> Vec<GitEvent> {
        let mut events = Vec::new();
        while let Ok((generation, reply)) = self.rx.try_recv() {
            if generation == self.generation {
                self.handle(reply, &mut events);
            }
        }
        if self.busy.is_none() && self.refresh_at.is_some_and(|at| Instant::now() >= at) {
            self.refresh_at = None;
            if let Some(repo) = self.repo.clone() {
                self.spawn(GitJob::Refresh, move || Reply::Status(repo.status()));
            }
        }
        self.maybe_start_diff();
        events
    }

    fn maybe_start_diff(&mut self) {
        let Some(rel) = self.diff_wanted.clone() else {
            return;
        };
        let cached = self.diff.as_ref().is_some_and(|(r, _)| *r == rel);
        if (cached && !self.diff_stale) || self.diff_in_flight.is_some() || self.is_working() {
            return;
        }
        let (Some(repo), Some(status)) = (self.repo.clone(), self.status.as_ref()) else {
            return;
        };
        let Some(change) = status.changes.iter().find(|c| c.rel == rel).cloned() else {
            // Committed or reverted since it was selected: nothing left to show.
            self.diff = Some((rel, Ok(FileDiff::default())));
            self.diff_stale = false;
            return;
        };
        self.diff_stale = false;
        self.diff_in_flight = Some(rel.clone());
        if !self.run_in_background(move || Reply::Diff(rel, repo.diff_file(&change))) {
            self.diff_in_flight = None;
        }
    }

    fn finish(&mut self, job: GitJob) {
        if self.busy == Some(job) {
            self.busy = None;
        }
    }

    fn report(&mut self, result: Result<String, String>, events: &mut Vec<GitEvent>) {
        let msg = match &result {
            Ok(m) | Err(m) => m.clone(),
        };
        self.last_result = Some((Instant::now(), result));
        events.push(GitEvent::Message(msg));
    }

    fn handle(&mut self, reply: Reply, events: &mut Vec<GitEvent>) {
        match reply {
            Reply::Detected(result) => {
                self.finish(GitJob::Detect);
                match result {
                    Ok(repo) => self.repo = repo,
                    Err(GitError::NotInstalled) => self.git_missing = true,
                    Err(e) => self.last_result = Some((Instant::now(), Err(format!("git: {e}")))),
                }
                self.request_refresh(Duration::ZERO);
            }
            Reply::Status(result) => {
                self.finish(GitJob::Refresh);
                match result {
                    Ok(status) => {
                        self.status = Some(status);
                        self.diff_stale = true;
                    }
                    Err(e) => self.last_result = Some((Instant::now(), Err(format!("git: {e}")))),
                }
                self.request_refresh(IDLE_REFRESH);
            }
            Reply::Diff(rel, result) => {
                self.diff_in_flight = None;
                self.diff = Some((rel, result.map_err(|e| e.to_string())));
            }
            Reply::Committed(result) => {
                self.finish(GitJob::Commit);
                let result = match result {
                    Ok(0) => Ok("git: nothing to commit".into()),
                    Ok(n) => Ok(format!(
                        "git: committed {n} file{}",
                        if n == 1 { "" } else { "s" }
                    )),
                    Err(e) => Err(failure_message(e)),
                };
                self.report(result, events);
                self.request_refresh(Duration::ZERO);
            }
            Reply::Synced(result) => {
                self.finish(GitJob::Sync);
                let result = result.map(|outcome| {
                    if outcome.pulled {
                        events.push(GitEvent::Pulled(outcome.changed_files.clone()));
                    }
                    outcome.summary()
                });
                self.report(result.map_err(failure_message), events);
                self.request_refresh(Duration::ZERO);
            }
            Reply::Pulled(result) => {
                self.finish(GitJob::Pull);
                let result = result.map(|outcome| {
                    if outcome.pulled {
                        events.push(GitEvent::Pulled(outcome.changed_files.clone()));
                    }
                    outcome.summary()
                });
                self.report(result.map_err(failure_message), events);
                self.request_refresh(Duration::ZERO);
            }
            Reply::Initialized(result) => {
                self.finish(GitJob::Init);
                let result = match result {
                    Ok(repo) => {
                        self.repo = Some(repo);
                        Ok("git: initialized repository · sync to make the first commit".into())
                    }
                    Err(e) => Err(failure_message(e)),
                };
                self.report(result, events);
                self.request_refresh(Duration::ZERO);
            }
        }
    }

    /// How long until `poll` has scheduled work, for `request_repaint_after`.
    pub fn next_wake(&self) -> Option<Duration> {
        if self.busy.is_some() {
            return None;
        }
        self.refresh_at
            .map(|at| at.saturating_duration_since(Instant::now()))
    }

    /// Compact footer label, e.g. `main · 3 changes · ↑1 ↓2`.
    pub fn chip_text(&self) -> Option<String> {
        self.repo.as_ref()?;
        if let Some(label) = self.working_label() {
            return Some(label.into());
        }
        let Some(s) = &self.status else {
            return Some("git".into());
        };
        let mut parts = vec![s.branch.clone().unwrap_or_else(|| "detached".into())];
        match s.changes.len() {
            0 => {}
            1 => parts.push("1 change".into()),
            n => parts.push(format!("{n} changes")),
        }
        let ab = ahead_behind(s);
        if !ab.is_empty() {
            parts.push(ab);
        }
        if self.last_error().is_some() {
            parts.push("!".into());
        }
        Some(parts.join(" · "))
    }

    /// Multi-line hover text for the footer chip.
    pub fn chip_tooltip(&self, panel_chord: &str, sync_chord: &str) -> String {
        let mut lines = Vec::new();
        if let Some(s) = &self.status {
            let branch = s.branch.as_deref().unwrap_or("detached HEAD");
            lines.push(match &s.upstream {
                Some(up) => format!("{branch} → {up}"),
                None => format!("{branch} (not published yet)"),
            });
            lines.push(match s.changes.len() {
                0 => "no uncommitted changes".into(),
                1 => "1 uncommitted change".into(),
                n => format!("{n} uncommitted changes"),
            });
        }
        if let Some(e) = self.last_error() {
            lines.push(e.to_string());
        }
        lines.push(String::new());
        lines.push(format!("click for the git panel {panel_chord} · sync {sync_chord}"));
        lines.join("\n")
    }
}

/// `↑1 ↓2`, or empty when in step with the upstream.
pub fn ahead_behind(s: &RepoStatus) -> String {
    let mut ab = Vec::new();
    if s.ahead > 0 {
        ab.push(format!("↑{}", s.ahead));
    }
    if s.behind > 0 {
        ab.push(format!("↓{}", s.behind));
    }
    ab.join(" ")
}

fn failure_message(e: GitError) -> String {
    match &e {
        GitError::Conflict(_) => format!(
            "git: {e} — your commit is kept locally; resolve in a terminal or edit and sync again"
        ),
        _ => format!("git: {e}"),
    }
}

/// `14s ago`, `3m ago`, `2h ago`.
pub fn format_age(age: Duration) -> String {
    let s = age.as_secs();
    if s < 60 {
        format!("{s}s ago")
    } else if s < 3600 {
        format!("{}m ago", s / 60)
    } else {
        format!("{}h ago", s / 3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle(git: &mut VaultGit) -> Vec<String> {
        let mut msgs = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            for ev in git.poll() {
                if let GitEvent::Message(m) = ev {
                    msgs.push(m);
                }
            }
            if git.busy.is_none()
                && git.diff_in_flight.is_none()
                && git.refresh_at.is_none_or(|at| at > Instant::now())
            {
                return msgs;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("git worker did not settle");
    }

    #[test]
    fn plain_folder_shows_no_git_ui_and_repo_shows_chip() {
        if !celestine_core::git::git_available() {
            return;
        }
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let mut git = VaultGit::default();

        git.attach(Some(dir.path()), true, &ctx);
        settle(&mut git);
        assert!(git.chip_text().is_none());
        assert!(git.can_init());

        git.start_init().unwrap();
        settle(&mut git);
        assert!(git.repo().is_some());
        // .gitignore written by init is the one pending change.
        assert_eq!(git.status().unwrap().changes.len(), 1, "{:?}", git.status());
        assert!(git.chip_text().unwrap().contains("1 change"));

        // Disabled integration is fully inert even for a repository.
        git.attach(Some(dir.path()), false, &ctx);
        settle(&mut git);
        assert!(git.chip_text().is_none() && !git.can_init());
    }

    #[test]
    fn selected_diff_loads_and_commit_uses_custom_message() {
        if !celestine_core::git::git_available() {
            return;
        }
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut git = VaultGit::default();
        git.attach(Some(root), true, &ctx);
        settle(&mut git);
        git.start_init().unwrap();
        settle(&mut git);
        for args in [
            ["config", "user.name", "Test"],
            ["config", "user.email", "test@example.com"],
            ["config", "commit.gpgsign", "false"],
        ] {
            std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .status()
                .unwrap();
        }
        std::fs::write(root.join("Ideas.md"), "# Ideas\n").unwrap();
        git.request_refresh(Duration::ZERO);
        settle(&mut git);
        assert_eq!(git.suggested_message(), "Update .gitignore, Ideas");

        git.select_diff(Some("Ideas.md"));
        settle(&mut git);
        let diff = git.diff().expect("diff loaded").as_ref().unwrap();
        assert_eq!(diff.lines.len(), 1);
        assert_eq!(diff.lines[0].text, "# Ideas");

        git.start_commit(Some("First notes".into())).unwrap();
        let msgs = settle(&mut git);
        assert_eq!(msgs, vec!["git: committed 2 files".to_string()]);
        assert!(git.recently_reported(Duration::from_secs(5)));
        assert!(git.status().unwrap().changes.is_empty());
        // The selected file is no longer changed, so its diff empties out.
        assert!(git.diff().unwrap().as_ref().unwrap().lines.is_empty());
        let log = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["log", "-1", "--format=%s"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "First notes");
    }
}
