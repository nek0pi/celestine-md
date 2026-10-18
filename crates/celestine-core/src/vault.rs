use crate::graph::LinkGraph;
use crate::index::{is_visible_path, VaultIndex};
use crate::path::{canonical_note_target, checked_rel, resolve_vault_path};
use crate::CoreError;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant, SystemTime};
use walkdir::WalkDir;

const FALLBACK_SCAN_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq)]
enum FsEntry {
    Directory,
    Note(FileStamp),
    Image(FileStamp),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
}

impl FileStamp {
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        }
    }
}

pub struct Vault {
    root: PathBuf,
    pub index: VaultIndex,
    pub graph: LinkGraph,
    _watcher: Option<RecommendedWatcher>,
    watcher_error: Option<notify::Error>,
    rx: Option<Receiver<Result<Event, notify::Error>>>,
    last_fs_reconcile: Instant,
    fs_entries: HashMap<PathBuf, FsEntry>,
    #[cfg(test)]
    note_reads: usize,
    #[cfg(test)]
    full_scans: usize,
}

impl Vault {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, CoreError> {
        Self::open_with_wakeup(root, || {})
    }

    /// Open a vault and wake its consumer after publishing filesystem changes.
    /// The callback runs on the watcher thread; indexing remains in `poll_fs`.
    pub fn open_with_wakeup(
        root: impl AsRef<Path>,
        wakeup: impl Fn() + Send + 'static,
    ) -> Result<Self, CoreError> {
        let root = root.as_ref().canonicalize()?;
        if !root.is_dir() {
            return Err(CoreError::NotADirectory(root));
        }
        let (tx, rx) = mpsc::channel();
        let watcher = RecommendedWatcher::new(
            fs_event_handler(root.clone(), tx, wakeup),
            notify::Config::default(),
        )
        .and_then(|mut watcher| {
            watcher.watch(&root, RecursiveMode::Recursive)?;
            Ok(watcher)
        });

        // Start watching before the initial scan so changes during it are queued.
        Ok(Self::from_watcher(root, watcher, rx))
    }

    fn from_watcher(
        root: PathBuf,
        watcher: notify::Result<RecommendedWatcher>,
        rx: Receiver<Result<Event, notify::Error>>,
    ) -> Self {
        let (watcher, watcher_error) = match watcher {
            Ok(watcher) => (Some(watcher), None),
            Err(error) => (None, Some(error)),
        };
        let watching = watcher_error.is_none();
        let mut vault = Self {
            root,
            index: VaultIndex::new(),
            graph: LinkGraph::default(),
            _watcher: watcher,
            watcher_error,
            rx: watching.then_some(rx),
            last_fs_reconcile: Instant::now(),
            fs_entries: HashMap::new(),
            #[cfg(test)]
            note_reads: 0,
            #[cfg(test)]
            full_scans: 0,
        };
        vault.reconcile_fs(false, &HashSet::new());
        vault
    }

    /// Startup or runtime watcher failures enable periodic disk scans.
    pub fn watcher_error(&self) -> Option<&notify::Error> {
        self.watcher_error.as_ref()
    }

    /// When the consumer must call `poll_fs` again without a watcher wakeup.
    /// A healthy watcher needs no periodic timer.
    pub fn next_fs_poll(&self) -> Option<Duration> {
        self.watcher_error
            .is_some()
            .then(|| FALLBACK_SCAN_INTERVAL.saturating_sub(self.last_fs_reconcile.elapsed()))
    }

    pub fn rebuild_graph(&mut self) {
        self.graph = LinkGraph::rebuild(&self.index);
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn abs_path(&self, rel: &str) -> Result<PathBuf, CoreError> {
        resolve_vault_path(&self.root, Path::new(rel))
    }

    /// Return the canonical vault-relative path for an existing in-vault file.
    pub fn rel_of(&self, path: &Path) -> Result<String, CoreError> {
        let resolved = crate::path::resolve_vault_abs(&self.root, path)?;
        let (_, rel) = canonical_note_target(resolved)?;
        Ok(rel)
    }

    pub fn read_metadata(&self, name: &str) -> Result<String, CoreError> {
        let path = self.metadata_path(name)?;
        Ok(std::fs::read_to_string(path)?)
    }

    pub fn write_metadata(&self, name: &str, content: &str) -> Result<(), CoreError> {
        let path = self.metadata_path(name)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
        Ok(())
    }

    fn metadata_path(&self, name: &str) -> Result<PathBuf, CoreError> {
        let name_path = Path::new(name);
        if name.is_empty()
            || name.contains('/')
            || name.contains('\\')
            || name_path.components().count() != 1
            || !matches!(
                name_path.components().next(),
                Some(std::path::Component::Normal(_))
            )
        {
            return Err(CoreError::InvalidVaultPath(format!(
                "{name:?}: metadata name must be one path component"
            )));
        }
        resolve_vault_path(&self.root, &Path::new(".celestine").join(name))
    }

    fn reconcile_fs(&mut self, force_all: bool, force_paths: &HashSet<PathBuf>) -> bool {
        #[cfg(test)]
        {
            self.full_scans += 1;
        }
        let mut found = HashMap::new();
        let mut unreadable = Vec::new();
        for entry in WalkDir::new(&self.root)
            .into_iter()
            .filter_entry(|entry| is_visible_path(&self.root, entry.path()))
        {
            match entry {
                Ok(entry) => match fs_entry(entry.path()) {
                    Ok(Some(state)) => {
                        found.insert(entry.into_path(), state);
                    }
                    Ok(None) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => unreadable.push(entry.into_path()),
                },
                Err(error) => {
                    if let Some(path) = error.path() {
                        unreadable.push(path.to_path_buf());
                    }
                }
            }
        }
        // A failed read is not evidence of deletion. Retain the last good state
        // until that subtree becomes accessible, including its note identities.
        for (path, state) in &self.fs_entries {
            if !found.contains_key(path)
                && (unreadable.iter().any(|prefix| path.starts_with(prefix))
                    || std::fs::metadata(path)
                        .is_err_and(|error| error.kind() != std::io::ErrorKind::NotFound))
            {
                found.insert(path.clone(), state.clone());
            }
        }
        let mut notes_changed = false;
        let mut sidebar_changed = false;
        // App writes update the index before their notifications arrive, so
        // compare indexed notes too, not just the last filesystem snapshot.
        let missing: Vec<_> = self
            .index
            .notes()
            .filter(|note| !matches!(found.get(&note.path), Some(FsEntry::Note(_))))
            .filter(|note| {
                !unreadable
                    .iter()
                    .any(|prefix| note.path.starts_with(prefix))
            })
            .filter(|note| {
                !std::fs::metadata(&note.path)
                    .is_err_and(|error| error.kind() != std::io::ErrorKind::NotFound)
            })
            .map(|note| note.path.clone())
            .collect();
        for path in missing {
            self.index.remove_path(&self.root, &path);
            notes_changed = true;
        }
        for (path, state) in &self.fs_entries {
            if !matches!(state, FsEntry::Note(_)) && found.get(path) != Some(state) {
                sidebar_changed = true;
            }
        }
        let mut failed_reads = Vec::new();
        for (path, state) in &found {
            let old = self.fs_entries.get(path);
            match state {
                FsEntry::Note(stamp) => {
                    // Mutation hints bypass the metadata shortcut: an editor can
                    // replace text without changing either its length or mtime.
                    if force_all
                        || stamp.modified.is_none()
                        || old != Some(state)
                        || path
                            .ancestors()
                            .any(|ancestor| force_paths.contains(ancestor))
                    {
                        match self.refresh_note(path) {
                            Ok(changed) => notes_changed |= changed,
                            Err(_) => failed_reads.push(path.clone()),
                        }
                    }
                }
                _ => sidebar_changed |= old != Some(state),
            }
        }
        // Cache only successful reads so transient IO/UTF-8 failures are retried
        // on the next fallback scan, even if metadata then stays unchanged.
        for path in failed_reads {
            found.insert(
                path,
                FsEntry::Note(FileStamp {
                    len: 0,
                    modified: None,
                }),
            );
        }
        self.fs_entries = found;
        if notes_changed {
            self.rebuild_graph();
        }
        self.last_fs_reconcile = Instant::now();
        notes_changed || sidebar_changed
    }

    fn refresh_note(&mut self, path: &Path) -> Result<bool, CoreError> {
        #[cfg(test)]
        {
            self.note_reads += 1;
        }
        self.index.upsert_file_if_changed(&self.root, path)
    }

    /// Drain filesystem events and report actual note, image, or directory changes.
    /// Unchanged fallback scans and watcher failures alone return false.
    pub fn poll_fs(&mut self) -> bool {
        let mut reconcile = self.next_fs_poll().is_some_and(|delay| delay.is_zero());
        let mut force_all = false;
        let mut paths = HashSet::new();
        let mut directory_modifications = HashSet::new();
        let mut failure = None;
        if let Some(rx) = &self.rx {
            while let Ok(result) = rx.try_recv() {
                match result {
                    Err(error) => {
                        failure = Some(error);
                        reconcile = true;
                        force_all = true;
                        break;
                    }
                    Ok(event) => {
                        if event.need_rescan() {
                            reconcile = true;
                            force_all = true;
                        }
                        let visible_paths: Vec<_> = event
                            .paths
                            .into_iter()
                            .filter(|path| is_visible_path(&self.root, path))
                            .collect();
                        match event.kind {
                            EventKind::Modify(notify::event::ModifyKind::Name(_)) => {
                                // Use disk spelling for split and case-only renames.
                                reconcile = true;
                                force_all |= visible_paths.is_empty();
                                paths.extend(visible_paths);
                            }
                            EventKind::Create(_) | EventKind::Remove(_) => {
                                reconcile |= visible_paths.iter().any(|path| {
                                    path.is_dir()
                                        || matches!(
                                            self.fs_entries.get(path),
                                            Some(FsEntry::Directory)
                                        )
                                });
                                paths.extend(visible_paths);
                            }
                            EventKind::Modify(_) => {
                                for path in visible_paths {
                                    if path.is_dir() {
                                        reconcile |= matches!(
                                            self.fs_entries.get(&path),
                                            Some(FsEntry::Note(_))
                                        );
                                        // Windows reports parent-directory writes
                                        // alongside child creation/removal events.
                                        directory_modifications.insert(path);
                                    } else {
                                        reconcile |= matches!(
                                            self.fs_entries.get(&path),
                                            Some(FsEntry::Directory)
                                        );
                                        paths.insert(path);
                                    }
                                }
                            }
                            EventKind::Any | EventKind::Other => {
                                reconcile = true;
                                force_all = true;
                            }
                            EventKind::Access(_) => {}
                        }
                    }
                }
            }
        }
        if let Some(error) = failure {
            self.watcher_error = Some(error);
            // Close publication before dropping the watcher. A recovery scan sees
            // the current disk state, including any events left in this queue.
            self.rx = None;
            self._watcher = None;
        }
        if reconcile {
            return self.reconcile_fs(force_all, &paths);
        }
        let mut notes_changed = false;
        let mut sidebar_changed = false;
        for path in directory_modifications {
            sidebar_changed |= self.fs_entries.insert(path, FsEntry::Directory).is_none();
        }
        for path in paths {
            match fs_entry(&path) {
                Ok(Some(FsEntry::Note(stamp))) => match self.refresh_note(&path) {
                    Ok(changed) => {
                        notes_changed |= changed;
                        self.fs_entries.insert(path, FsEntry::Note(stamp));
                    }
                    Err(_) => {
                        self.fs_entries.insert(
                            path,
                            FsEntry::Note(FileStamp {
                                len: 0,
                                modified: None,
                            }),
                        );
                    }
                },
                Ok(Some(state)) => {
                    sidebar_changed |= self.fs_entries.get(&path) != Some(&state);
                    self.fs_entries.insert(path, state);
                }
                Ok(None) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    notes_changed |= self
                        .index
                        .get_by_rel(&relative_path(&self.root, &path))
                        .is_some();
                    self.index.remove_path(&self.root, &path);
                    sidebar_changed |= self
                        .fs_entries
                        .remove(&path)
                        .is_some_and(|state| !matches!(state, FsEntry::Note(_)));
                }
                Err(_) => {}
            }
        }
        if notes_changed {
            self.rebuild_graph();
        }
        notes_changed || sidebar_changed
    }

    pub fn read_note_content(&self, rel: &str) -> Result<String, CoreError> {
        let path = resolve_vault_path(&self.root, Path::new(rel))?;
        Ok(std::fs::read_to_string(path)?)
    }

    pub fn write_note(&mut self, rel: &str, content: &str) -> Result<(), CoreError> {
        let path = resolve_vault_path(&self.root, Path::new(rel))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
        let _ = self.index.upsert_file(&self.root, &path);
        self.rebuild_graph();
        Ok(())
    }

    pub fn create_note(&mut self, rel: &str) -> Result<(), CoreError> {
        let mut rel = rel.to_string();
        if !rel.to_lowercase().ends_with(".md") {
            rel.push_str(".md");
        }
        let path = resolve_vault_path(&self.root, Path::new(&rel))?;
        if std::fs::symlink_metadata(&path).is_ok() {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let title = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".into());
        let content = format!("# {title}\n\n");
        std::fs::write(&path, content)?;
        let _ = self.index.upsert_file(&self.root, &path);
        self.rebuild_graph();
        Ok(())
    }

    pub fn list_md_rel_paths(&self) -> Vec<String> {
        let mut v: Vec<_> = self.index.notes().map(|n| n.rel.clone()).collect();
        v.sort();
        v
    }

    /// Image files under the vault (png/jpg/gif/webp/svg/bmp), for the sidebar tree.
    pub fn list_image_rel_paths(&self) -> Vec<String> {
        let image_exts = ["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp"];
        let mut v = Vec::new();
        for entry in WalkDir::new(&self.root)
            .into_iter()
            .filter_entry(|entry| is_visible_path(&self.root, entry.path()))
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let path = entry.path();
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if !image_exts.iter().any(|x| x.eq_ignore_ascii_case(ext)) {
                continue;
            }
            let Ok(rel) = checked_rel(&self.root, path) else {
                continue;
            };
            if rel.split('/').any(|p| p.starts_with('.')) {
                continue;
            }
            v.push(rel);
        }
        v.sort();
        v.dedup();
        v
    }

    /// Notes + images for the files sidebar.
    pub fn list_sidebar_rel_paths(&self) -> Vec<String> {
        let mut v = self.list_md_rel_paths();
        v.extend(self.list_image_rel_paths());
        v.sort();
        v.dedup();
        v
    }

    /// Create an empty folder under the vault root (`rel` uses `/` separators).
    pub fn create_folder(&mut self, rel: &str) -> Result<(), CoreError> {
        let path = resolve_vault_path(&self.root, Path::new(rel))?;
        std::fs::create_dir_all(&path)?;
        Ok(())
    }

    /// Rename a folder or note. `old_rel` / `new_rel` are vault-relative (`/` separators).
    /// For folders, all notes under the prefix are reindexed.
    pub fn rename_entry(&mut self, old_rel: &str, new_rel: &str) -> Result<(), CoreError> {
        let old_path = resolve_vault_path(&self.root, Path::new(old_rel))?;
        let new_path = resolve_vault_path(&self.root, Path::new(new_rel))?;
        let old_rel = checked_rel(&self.root, &old_path)?;
        let new_rel = checked_rel(&self.root, &new_path)?;
        if old_rel.is_empty() || new_rel.is_empty() || old_rel == new_rel {
            return Ok(());
        }
        if !old_path.exists() {
            return Err(CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("missing {old_rel}"),
            )));
        }
        // On case-insensitive filesystems `Note.md` "exists" when `note.md`
        // does; a rename that only changes case is still allowed.
        if new_path.exists() && !same_entry(&old_path, &new_path) {
            return Err(CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{new_rel} already exists"),
            )));
        }
        if let Some(parent) = new_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&old_path, &new_path)?;

        let is_dir = new_path.is_dir();
        if is_dir {
            let prefix = format!("{old_rel}/");
            let affected: Vec<String> = self
                .index
                .notes()
                .filter(|n| n.rel == old_rel || n.rel.starts_with(&prefix))
                .map(|n| n.rel.clone())
                .collect();
            for rel in affected {
                let abs_old = resolve_vault_path(&self.root, Path::new(&rel))?;
                self.index.remove_path(&self.root, &abs_old);
                let rest = rel.strip_prefix(&old_rel).unwrap_or("");
                let new_note_rel = format!("{new_rel}{rest}");
                let abs_new = resolve_vault_path(&self.root, Path::new(&new_note_rel))?;
                if abs_new.exists() {
                    let _ = self.index.upsert_file(&self.root, &abs_new);
                }
            }
        } else {
            self.index.remove_path(&self.root, &old_path);
            if new_path.exists() {
                let _ = self.index.upsert_file(&self.root, &new_path);
            }
        }
        self.rebuild_graph();
        Ok(())
    }

    /// Delete a note file, or an empty folder.
    pub fn delete_entry(&mut self, rel: &str) -> Result<(), CoreError> {
        let path = resolve_vault_path(&self.root, Path::new(rel))?;
        let rel = checked_rel(&self.root, &path)?;
        if path.is_dir() {
            let empty = std::fs::read_dir(&path)?.next().is_none();
            if !empty {
                return Err(CoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "folder is not empty",
                )));
            }
            std::fs::remove_dir(&path)?;
        } else if path.is_file() {
            std::fs::remove_file(&path)?;
            self.index.remove_path(&self.root, &path);
            self.rebuild_graph();
        } else {
            return Err(CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("missing {rel}"),
            )));
        }
        Ok(())
    }
}

fn fs_event_handler(
    root: PathBuf,
    tx: Sender<Result<Event, notify::Error>>,
    wakeup: impl Fn() + Send + 'static,
) -> impl FnMut(Result<Event, notify::Error>) + Send {
    let mut failed = false;
    move |result| {
        // A failing backend can call repeatedly before the UI retires it.
        // Publish the first error only, bounding both queued errors and wakeups.
        if !failed {
            failed = result.is_err();
            publish_fs_event(result, &root, &tx, &wakeup);
        }
    }
}

fn publish_fs_event(
    result: Result<Event, notify::Error>,
    root: &Path,
    tx: &Sender<Result<Event, notify::Error>>,
    wakeup: &impl Fn(),
) {
    // Linux also reports our own directory/file reads. Discard access events
    // before enqueueing: waking on them loops, and queueing without waking leaks.
    if matches!(&result, Ok(event) if matches!(event.kind, EventKind::Access(_)) && !event.need_rescan())
    {
        return;
    }
    // Keep rescan/error signals even if their attached paths are hidden. A
    // mixed rename must still publish when either side is visible.
    if matches!(&result, Ok(event) if !event.need_rescan() && !event.paths.is_empty()
        && event.paths.iter().all(|path| !is_visible_path(root, path)))
    {
        return;
    }
    if tx.send(result).is_ok() {
        wakeup();
    }
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn fs_entry(path: &Path) -> std::io::Result<Option<FsEntry>> {
    let metadata = std::fs::metadata(path)?;
    if metadata.is_dir() {
        return Ok(Some(FsEntry::Directory));
    }
    if !metadata.is_file() {
        return Ok(None);
    }
    let stamp = FileStamp::from_metadata(&metadata);
    let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
    if extension.eq_ignore_ascii_case("md") {
        return Ok(Some(FsEntry::Note(stamp)));
    }
    if ["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp"]
        .iter()
        .any(|ext| extension.eq_ignore_ascii_case(ext))
    {
        return Ok(Some(FsEntry::Image(stamp)));
    }
    Ok(None)
}

/// Do both paths name the same on-disk entry (e.g. differ only by case)?
fn same_entry(a: &Path, b: &Path) -> bool {
    same_file::is_same_file(a, b).unwrap_or(false)
        || matches!(
            (a.canonicalize(), b.canonicalize()),
            (Ok(a), Ok(b)) if a == b
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{
        AccessKind, AccessMode, CreateKind, Flag, ModifyKind, RemoveKind, RenameMode,
    };
    use std::cell::Cell;

    // Inject backend notifications deterministically; disk reads, indexing and graph
    // updates still use the real implementation and a real temporary vault.
    fn queued_vault(root: &Path) -> (Vault, Sender<Result<Event, notify::Error>>) {
        let (tx, rx) = mpsc::channel();
        let mut vault = Vault {
            root: root.canonicalize().unwrap(),
            index: VaultIndex::new(),
            graph: LinkGraph::default(),
            _watcher: None,
            watcher_error: None,
            rx: Some(rx),
            last_fs_reconcile: Instant::now(),
            fs_entries: HashMap::new(),
            note_reads: 0,
            full_scans: 0,
        };
        vault.reconcile_fs(false, &HashSet::new());
        (vault, tx)
    }

    #[test]
    fn runtime_watcher_error_keeps_refreshing_later_unreported_changes() {
        let dir = tempfile::tempdir().unwrap();
        let (mut vault, tx) = queued_vault(dir.path());
        assert!(
            vault.next_fs_poll().is_none(),
            "fixture needs a healthy watcher"
        );
        tx.send(Err(notify::Error::generic("backend stopped")))
            .unwrap();
        assert!(!vault.poll_fs(), "an error alone is not a vault change");
        assert!(vault.rx.is_none(), "retire the failed event channel");
        assert!(tx
            .send(Err(notify::Error::generic("repeated error")))
            .is_err());
        std::fs::write(
            vault.root.join("Unreported.md"),
            "# Changed after the error",
        )
        .unwrap();
        vault.last_fs_reconcile = Instant::now() - FALLBACK_SCAN_INTERVAL;

        assert!(
            vault.poll_fs(),
            "a backend error must enable later fallback scans"
        );
        assert!(vault.index.get_by_rel("Unreported.md").is_some());
        assert!(vault.watcher_error().is_some());
    }

    #[test]
    fn read_notifications_are_neither_queued_nor_woken() {
        let (tx, rx) = mpsc::channel();
        let wakes = Cell::new(0);
        for kind in [
            AccessKind::Any,
            AccessKind::Read,
            AccessKind::Open(AccessMode::Read),
            AccessKind::Close(AccessMode::Read),
        ] {
            publish_fs_event(
                Ok(Event::new(EventKind::Access(kind))),
                Path::new("/vault"),
                &tx,
                &|| {
                    wakes.set(wakes.get() + 1);
                },
            );
        }
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        assert_eq!(wakes.get(), 0);
    }

    #[test]
    fn mutations_and_recovery_signals_are_published_before_wakeup() {
        let (tx, rx) = mpsc::channel();
        let wakes = Cell::new(0);
        for result in [
            Ok(Event::new(EventKind::Create(CreateKind::File))),
            Ok(Event::new(EventKind::Modify(ModifyKind::Any))),
            Ok(Event::new(EventKind::Remove(RemoveKind::Folder))),
            Ok(Event::new(EventKind::Any).set_flag(Flag::Rescan)),
            Ok(Event::new(EventKind::Access(AccessKind::Any)).set_flag(Flag::Rescan)),
            Err(notify::Error::generic("backend error")),
        ] {
            publish_fs_event(result, Path::new("/vault"), &tx, &|| {
                assert!(
                    rx.try_recv().is_ok(),
                    "wake must observe the published event"
                );
                wakes.set(wakes.get() + 1);
            });
        }
        assert_eq!(wakes.get(), 6);
        drop(rx);
        publish_fs_event(
            Ok(Event::new(EventKind::Any)),
            Path::new("/vault"),
            &tx,
            &|| {
                panic!("a closed vault must not wake its former consumer");
            },
        );
    }

    #[test]
    fn unavailable_watcher_reconciles_disk_only_when_the_fallback_is_due() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (_tx, rx) = mpsc::channel();
        // Substitute only the OS watcher registration result. The fallback scan,
        // deadline, file IO, index and graph are the production implementation.
        let mut vault = Vault::from_watcher(
            root,
            Err(notify::Error::generic("watch registration failed")),
            rx,
        );
        assert!(vault.watcher_error().is_some());
        assert!(vault.next_fs_poll().is_some_and(|delay| !delay.is_zero()));
        std::fs::write(vault.root.join("Late.md"), "# Discovered by polling").unwrap();
        assert!(!vault.poll_fs(), "do not scan on every input frame");
        assert!(vault.index.is_empty());

        vault.last_fs_reconcile = Instant::now() - FALLBACK_SCAN_INTERVAL;
        assert!(vault.poll_fs());
        let note = vault.index.get_by_rel("Late.md").unwrap();
        assert_eq!(note.content, "# Discovered by polling");
        assert_eq!(vault.graph.node_ids(), [note.id]);
        assert!(vault.next_fs_poll().is_some_and(|delay| !delay.is_zero()));
        assert!(!vault.poll_fs(), "a scan must reset the fallback deadline");
    }

    #[test]
    fn image_event_requests_refresh_without_a_markdown_change() {
        let dir = tempfile::tempdir().unwrap();
        let (mut vault, tx) = queued_vault(dir.path());
        let path = vault.root.join("picture.png");
        std::fs::write(&path, b"image bytes").unwrap();
        tx.send(Ok(
            Event::new(EventKind::Create(CreateKind::File)).add_path(path)
        ))
        .unwrap();

        assert!(vault.poll_fs(), "image creation must refresh the sidebar");
        assert_eq!(vault.list_sidebar_rel_paths(), ["picture.png"]);
        assert!(!vault.poll_fs(), "a drained queue must become idle");
    }

    #[test]
    fn watcher_error_does_not_strand_the_following_change() {
        let dir = tempfile::tempdir().unwrap();
        let (mut vault, tx) = queued_vault(dir.path());
        let path = vault.root.join("New.md");
        std::fs::write(&path, "# New\n").unwrap();
        tx.send(Err(notify::Error::generic("backend lost events")))
            .unwrap();
        tx.send(Ok(
            Event::new(EventKind::Create(CreateKind::File)).add_path(path)
        ))
        .unwrap();

        assert!(vault.poll_fs(), "recover and consume the whole event batch");
        assert!(vault.index.get_by_rel("New.md").is_some());
        assert!(
            !vault.poll_fs(),
            "valid events after errors must be drained"
        );
    }

    #[test]
    fn rescan_reconciles_disk_and_preserves_surviving_note_ids() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Keep.md"), "[[Old]]").unwrap();
        std::fs::write(dir.path().join("Old.md"), "# Old").unwrap();
        let (mut vault, tx) = queued_vault(dir.path());
        let keep_id = vault.index.get_by_rel("Keep.md").unwrap().id;
        let old_id = vault.index.get_by_rel("Old.md").unwrap().id;
        std::fs::remove_file(vault.root.join("Old.md")).unwrap();
        std::fs::write(vault.root.join("New.md"), "[[Keep]]").unwrap();
        std::fs::write(vault.root.join("Keep.md"), "# Updated").unwrap();
        tx.send(Ok(Event::new(EventKind::Any).set_flag(Flag::Rescan)))
            .unwrap();

        assert!(vault.poll_fs());
        assert_eq!(vault.list_md_rel_paths(), ["Keep.md", "New.md"]);
        assert_eq!(vault.index.get_by_rel("Keep.md").unwrap().id, keep_id);
        assert_eq!(vault.index.get(keep_id).unwrap().content, "# Updated");
        let new_id = vault.index.get_by_rel("New.md").unwrap().id;
        assert_ne!(new_id, old_id, "removed IDs must not be reused");
        assert_eq!(vault.graph.neighbors(keep_id), [new_id]);
    }

    #[test]
    fn directory_rename_reindexes_descendant_notes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Old/nested")).unwrap();
        std::fs::write(dir.path().join("Old/nested/Child.md"), "[[Home]]").unwrap();
        std::fs::write(dir.path().join("Home.md"), "# Home").unwrap();
        let (mut vault, tx) = queued_vault(dir.path());
        let home_id = vault.index.get_by_rel("Home.md").unwrap().id;
        let old = vault.root.join("Old");
        let new = vault.root.join("New");
        std::fs::rename(&old, &new).unwrap();
        tx.send(Ok(Event::new(EventKind::Modify(ModifyKind::Name(
            RenameMode::Both,
        )))
        .add_path(old)
        .add_path(new)))
            .unwrap();

        assert!(vault.poll_fs());
        assert_eq!(
            vault.list_md_rel_paths(),
            ["Home.md", "New/nested/Child.md"]
        );
        let child_id = vault.index.get_by_rel("New/nested/Child.md").unwrap().id;
        assert_eq!(vault.graph.neighbors(home_id), [child_id]);
    }

    #[test]
    fn directory_removal_drops_all_descendants_even_with_md_suffix() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Folder.md/nested")).unwrap();
        std::fs::write(dir.path().join("Folder.md/nested/Child.md"), "# Child").unwrap();
        let (mut vault, tx) = queued_vault(dir.path());
        let path = vault.root.join("Folder.md");
        std::fs::remove_dir_all(&path).unwrap();
        tx.send(Ok(
            Event::new(EventKind::Remove(RemoveKind::Folder)).add_path(path)
        ))
        .unwrap();

        assert!(vault.poll_fs());
        assert!(vault.index.is_empty(), "deleted directory left stale notes");
        assert!(vault.graph.node_ids().is_empty());
    }

    #[test]
    fn case_only_rename_notifications_reconcile_actual_disk_spelling() {
        for mode in [RenameMode::From, RenameMode::Both] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("Anchor.md"), "[[Note]]").unwrap();
            std::fs::write(dir.path().join("note.md"), "# Renamed note").unwrap();
            let (mut vault, tx) = queued_vault(dir.path());
            let anchor_id = vault.index.get_by_rel("Anchor.md").unwrap().id;
            let old_id = vault.index.get_by_rel("note.md").unwrap().id;
            let old_path = vault.root.join("note.md");
            let new_path = vault.root.join("Note.md");
            std::fs::rename(&old_path, &new_path).unwrap();
            let mut event =
                Event::new(EventKind::Modify(ModifyKind::Name(mode))).add_path(old_path);
            if mode == RenameMode::Both {
                event = event.add_path(new_path.clone());
            }
            tx.send(Ok(event)).unwrap();

            assert!(vault.poll_fs());
            assert_eq!(
                vault.list_md_rel_paths(),
                ["Anchor.md", "Note.md"],
                "{mode:?}"
            );
            let renamed = vault.index.get_by_rel("Note.md").unwrap();
            assert_eq!(renamed.path, new_path);
            assert_eq!(vault.index.get_by_rel("Anchor.md").unwrap().id, anchor_id);
            assert_eq!(vault.graph.neighbors(anchor_id), [renamed.id]);
            if renamed.id != old_id {
                assert!(!vault.graph.node_ids().contains(&old_id));
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn rescan_retains_note_ids_when_a_directory_is_temporarily_unreadable() {
        use std::os::unix::fs::PermissionsExt;

        struct RestorePermissions {
            path: PathBuf,
            permissions: std::fs::Permissions,
        }
        impl Drop for RestorePermissions {
            fn drop(&mut self) {
                std::fs::set_permissions(&self.path, self.permissions.clone())
                    .expect("restore temporary test directory permissions");
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let private = dir.path().join("Private");
        std::fs::create_dir(&private).unwrap();
        std::fs::write(private.join("Hidden.md"), "# Keep this note").unwrap();
        std::fs::write(dir.path().join("Anchor.md"), "[[Private/Hidden]]").unwrap();
        let (mut vault, tx) = queued_vault(dir.path());
        let anchor_id = vault.index.get_by_rel("Anchor.md").unwrap().id;
        let note_id = vault.index.get_by_rel("Private/Hidden.md").unwrap().id;
        let restore = RestorePermissions {
            path: private.clone(),
            permissions: std::fs::metadata(&private).unwrap().permissions(),
        };
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o0)).unwrap();
        match std::fs::metadata(private.join("Hidden.md")) {
            Ok(_) => {
                // Privileged users can bypass directory mode bits. No permission
                // failure was induced, so this environment cannot exercise it.
                eprintln!("skipping permission regression: metadata remained accessible");
                return;
            }
            Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied),
        }
        tx.send(Ok(Event::new(EventKind::Any).set_flag(Flag::Rescan)))
            .unwrap();

        assert!(
            !vault.poll_fs(),
            "unreadable but retained content did not change"
        );
        let retained = vault
            .index
            .get_by_rel("Private/Hidden.md")
            .expect("an inaccessible note must not be treated as deleted");
        assert_eq!(retained.id, note_id);
        assert_eq!(vault.graph.neighbors(anchor_id), [note_id]);

        drop(restore);
        std::fs::write(private.join("Hidden.md"), "# Accessible again").unwrap();
        tx.send(Ok(Event::new(EventKind::Any).set_flag(Flag::Rescan)))
            .unwrap();
        assert!(vault.poll_fs());
        let recovered = vault.index.get_by_rel("Private/Hidden.md").unwrap();
        assert_eq!(recovered.id, note_id);
        assert_eq!(recovered.content, "# Accessible again");
        assert_eq!(vault.graph.neighbors(anchor_id), [note_id]);
    }

    #[test]
    fn hidden_paths_do_not_publish_but_visible_rename_and_rescan_do() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".vault");
        std::fs::create_dir(&root).unwrap();
        let (tx, rx) = mpsc::channel();
        let wakes = Cell::new(0);
        let wakeup = || wakes.set(wakes.get() + 1);
        for rel in [
            ".git/index.lock",
            ".obsidian/workspace.json",
            ".trash/a.md",
            ".celestine/config.json",
            ".DS_Store",
            "notes/.private/a.md",
        ] {
            publish_fs_event(
                Ok(Event::new(EventKind::Modify(ModifyKind::Any)).add_path(root.join(rel))),
                &root,
                &tx,
                &wakeup,
            );
        }
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        assert_eq!(wakes.get(), 0);
        publish_fs_event(
            Ok(
                Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
                    .add_path(root.join(".tmp.md"))
                    .add_path(root.join("Visible.md")),
            ),
            &root,
            &tx,
            &wakeup,
        );
        publish_fs_event(
            Ok(Event::new(EventKind::Any)
                .set_flag(Flag::Rescan)
                .add_path(root.join(".git"))),
            &root,
            &tx,
            &wakeup,
        );
        assert_eq!(rx.try_iter().count(), 2);
        assert_eq!(wakes.get(), 2);
    }

    #[test]
    fn repeated_watcher_errors_publish_and_wake_only_once() {
        let (tx, rx) = mpsc::channel();
        let (wake_tx, wakes) = mpsc::channel();
        let mut handler = fs_event_handler(PathBuf::from("/vault"), tx, move || {
            wake_tx.send(()).unwrap();
        });
        for _ in 0..100 {
            handler(Err(notify::Error::generic("persistent read error")));
        }
        handler(Ok(Event::new(EventKind::Create(CreateKind::File))));
        assert_eq!(rx.try_iter().count(), 1);
        assert_eq!(wakes.try_iter().count(), 1);
    }

    #[test]
    fn scans_prune_hidden_subtrees_inside_a_hidden_vault_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".vault");
        std::fs::create_dir_all(root.join(".git/objects/deep")).unwrap();
        std::fs::create_dir_all(root.join("Notes/.private")).unwrap();
        std::fs::write(root.join(".git/objects/deep/Hidden.md"), "# Hidden").unwrap();
        std::fs::write(root.join("Notes/.private/Hidden.md"), "# Hidden").unwrap();
        std::fs::write(root.join("Notes/.private/hidden.png"), b"hidden").unwrap();
        std::fs::write(root.join("Notes/Visible.md"), "# Visible").unwrap();
        let (vault, _) = queued_vault(&root);
        assert_eq!(vault.list_md_rel_paths(), ["Notes/Visible.md"]);
        assert_eq!(vault.note_reads, 1);
        assert_eq!(
            vault.fs_entries.len(),
            3,
            "root, Notes, and its visible note only"
        );
        assert!(vault.list_image_rel_paths().is_empty());
    }

    #[test]
    fn unchanged_fallback_skips_note_reads_and_tracks_images_and_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Keep.md"), "# Keep").unwrap();
        std::fs::write(dir.path().join("picture.png"), b"first image").unwrap();
        std::fs::create_dir(dir.path().join("Empty")).unwrap();
        let (mut vault, _) = queued_vault(dir.path());
        vault.watcher_error = Some(notify::Error::generic("unavailable"));
        vault.note_reads = 0;
        vault.last_fs_reconcile = Instant::now() - FALLBACK_SCAN_INTERVAL;
        assert!(
            !vault.poll_fs(),
            "unchanged fallback must not report a refresh"
        );
        assert_eq!(vault.note_reads, 0);
        std::fs::write(vault.root.join("picture.png"), b"changed image bytes").unwrap();
        vault.last_fs_reconcile = Instant::now() - FALLBACK_SCAN_INTERVAL;
        assert!(vault.poll_fs(), "image content metadata changed");
        assert_eq!(vault.note_reads, 0);
        std::fs::remove_file(vault.root.join("picture.png")).unwrap();
        std::fs::remove_dir(vault.root.join("Empty")).unwrap();
        vault.last_fs_reconcile = Instant::now() - FALLBACK_SCAN_INTERVAL;
        assert!(vault.poll_fs());
        assert!(vault.list_image_rel_paths().is_empty());
        vault.last_fs_reconcile = Instant::now() - FALLBACK_SCAN_INTERVAL;
        assert!(!vault.poll_fs());
    }

    #[test]
    fn explicit_mutations_bypass_identical_length_and_mtime() {
        for kind in [
            EventKind::Modify(ModifyKind::Any),
            EventKind::Modify(ModifyKind::Name(RenameMode::Any)),
            EventKind::Any,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("Keep.md");
            std::fs::write(&path, "# Before").unwrap();
            let (mut vault, tx) = queued_vault(dir.path());
            let path = vault.root.join("Keep.md");
            let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
            std::fs::write(&path, "# After!").unwrap();
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(modified))
                .unwrap();
            assert_eq!(
                fs_entry(&path).unwrap().as_ref(),
                vault.fs_entries.get(&path)
            );
            vault.note_reads = 0;
            tx.send(Ok(Event::new(kind).add_path(path.clone())))
                .unwrap();
            assert!(
                vault.poll_fs(),
                "explicit mutation must refresh identical metadata"
            );
            assert_eq!(
                vault.index.get_by_rel("Keep.md").unwrap().content,
                "# After!"
            );
            assert_eq!(vault.note_reads, 1);
            tx.send(Ok(Event::new(kind).add_path(path))).unwrap();
            assert!(
                !vault.poll_fs(),
                "duplicate event with unchanged text is not a change"
            );
        }
    }

    #[test]
    fn directory_modify_does_not_rescan_or_reparse_notes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Keep.md"), "# Keep").unwrap();
        let (mut vault, tx) = queued_vault(dir.path());
        vault.note_reads = 0;
        vault.full_scans = 0;
        tx.send(Ok(
            Event::new(EventKind::Modify(ModifyKind::Any)).add_path(vault.root.clone())
        ))
        .unwrap();
        assert!(!vault.poll_fs());
        assert_eq!((vault.full_scans, vault.note_reads), (0, 0));
        let new_path = vault.root.join("New.md");
        std::fs::write(&new_path, "# New").unwrap();
        tx.send(Ok(
            Event::new(EventKind::Create(CreateKind::File)).add_path(new_path)
        ))
        .unwrap();
        tx.send(Ok(
            Event::new(EventKind::Modify(ModifyKind::Any)).add_path(vault.root.clone())
        ))
        .unwrap();
        assert!(vault.poll_fs());
        assert_eq!((vault.full_scans, vault.note_reads), (0, 1));
    }

    #[test]
    fn failed_note_read_keeps_previous_content_and_retries_unchanged_metadata() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Keep.md"), "# Keep").unwrap();
        let (mut vault, _) = queued_vault(dir.path());
        let path = vault.root.join("Keep.md");
        let id = vault.index.get_by_rel("Keep.md").unwrap().id;
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, [0xff; 6]).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        vault.note_reads = 0;
        // Force the failed read explicitly: fast same-size writes can retain
        // the original timestamp, so metadata alone need not detect this edit.
        assert!(!vault.reconcile_fs(false, &HashSet::from([path.clone()])));
        assert_eq!(
            vault.note_reads, 1,
            "the fixture must attempt the invalid read"
        );
        assert_eq!(vault.index.get(id).unwrap().content, "# Keep");
        let failed_stamp = fs_entry(&path).unwrap();
        std::fs::write(&path, "# Fine").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        assert_eq!(
            fs_entry(&path).unwrap(),
            failed_stamp,
            "retry with unchanged metadata"
        );
        assert!(vault.reconcile_fs(false, &HashSet::new()));
        assert_eq!(
            vault.note_reads, 2,
            "a failed read must be retried without a new hint"
        );
        assert_eq!(vault.index.get(id).unwrap().content, "# Fine");
    }

    #[test]
    fn deletion_before_the_app_write_notification_does_not_leave_a_stale_note() {
        for fallback in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let (mut vault, tx) = queued_vault(dir.path());
            vault.write_note("New.md", "# New").unwrap();
            let path = vault.root.join("New.md");
            std::fs::remove_file(&path).unwrap();
            if fallback {
                vault.watcher_error = Some(notify::Error::generic("unavailable"));
                vault.last_fs_reconcile = Instant::now() - FALLBACK_SCAN_INTERVAL;
            } else {
                tx.send(Ok(
                    Event::new(EventKind::Remove(RemoveKind::File)).add_path(path)
                ))
                .unwrap();
            }
            assert!(vault.poll_fs());
            assert!(vault.index.is_empty());
            assert!(vault.graph.node_ids().is_empty());
        }
    }

    #[test]
    #[ignore = "synthetic 3000-note timing workload; run explicitly in release mode"]
    fn large_vault_reconciliation_measurement() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".git/objects")).unwrap();
        std::fs::create_dir(root.join("images")).unwrap();
        let body = "A synthetic paragraph for reconciliation measurement.\n".repeat(20);
        let mut bytes = 0;
        for i in 0..3000 {
            let text = format!("# Note {i}\n\n[[Note{:04}]]\n{body}", (i + 1) % 3000);
            bytes += text.len();
            std::fs::write(root.join(format!("Note{i:04}.md")), text).unwrap();
            std::fs::write(
                root.join(format!(".git/objects/hidden{i:04}.md")),
                b"ignored",
            )
            .unwrap();
        }
        for i in 0..300 {
            std::fs::write(
                root.join(format!("images/picture{i:03}.png")),
                b"image fixture",
            )
            .unwrap();
        }
        let (mut vault, tx) = queued_vault(root);
        assert_eq!(vault.index.len(), 3000);
        assert_eq!(vault.list_image_rel_paths().len(), 300);
        println!("workload: 3000 notes, {bytes} note bytes, 300 image entries, 3000 hidden files");
        vault.watcher_error = Some(notify::Error::generic("benchmark fallback"));
        for sample in 0..5 {
            vault.note_reads = 0;
            vault.last_fs_reconcile = Instant::now() - FALLBACK_SCAN_INTERVAL;
            let started = Instant::now();
            assert!(!vault.poll_fs());
            println!(
                "unchanged_fallback sample={sample} ms={:.3} note_reads={}",
                started.elapsed().as_secs_f64() * 1000.0,
                vault.note_reads
            );
            assert_eq!(vault.note_reads, 0);
        }
        vault.watcher_error = None;
        for sample in 0..5 {
            let temporary = vault.root.join(".save.md");
            let target = vault.root.join("Note0000.md");
            std::fs::write(
                &temporary,
                format!("# Atomic save {sample}\n[[Note0001]]\n{body}"),
            )
            .unwrap();
            std::fs::rename(&temporary, &target).unwrap();
            tx.send(Ok(Event::new(EventKind::Modify(ModifyKind::Name(
                RenameMode::Any,
            )))
            .add_path(target)))
                .unwrap();
            vault.note_reads = 0;
            let started = Instant::now();
            assert!(vault.poll_fs());
            println!(
                "atomic_save sample={sample} ms={:.3} note_reads={}",
                started.elapsed().as_secs_f64() * 1000.0,
                vault.note_reads
            );
            assert_eq!(vault.note_reads, 1);
        }
        vault.full_scans = 0;
        vault.note_reads = 0;
        tx.send(Ok(
            Event::new(EventKind::Modify(ModifyKind::Any)).add_path(vault.root.clone())
        ))
        .unwrap();
        let started = Instant::now();
        assert!(!vault.poll_fs());
        println!(
            "parent_directory_modify ms={:.3} full_scans={} note_reads={}",
            started.elapsed().as_secs_f64() * 1000.0,
            vault.full_scans,
            vault.note_reads
        );
        assert_eq!((vault.full_scans, vault.note_reads), (0, 0));
    }

    fn write_rel(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn rename_note_differing_only_by_case() {
        let dir = tempfile::tempdir().unwrap();
        write_rel(dir.path(), "note.md", "# note\n");
        let mut v = Vault::open(dir.path()).unwrap();
        v.rename_entry("note.md", "Note.md")
            .expect("case-only rename must succeed");
        assert_eq!(v.list_md_rel_paths(), vec!["Note.md"]);
        let on_disk: Vec<String> = std::fs::read_dir(v.root())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert!(on_disk.contains(&"Note.md".to_string()), "{on_disk:?}");
        assert!(!on_disk.contains(&"note.md".to_string()), "{on_disk:?}");
    }

    #[test]
    fn rename_onto_a_different_existing_note_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write_rel(dir.path(), "a.md", "# a\n");
        write_rel(dir.path(), "b.md", "# b\n");
        let mut v = Vault::open(dir.path()).unwrap();
        assert!(v.rename_entry("a.md", "b.md").is_err());
        assert_eq!(v.list_md_rel_paths(), vec!["a.md", "b.md"]);
    }

    #[test]
    fn uppercase_md_extension_is_indexed() {
        let dir = tempfile::tempdir().unwrap();
        write_rel(dir.path(), "Home.md", "# Home\n");
        let mut v = Vault::open(dir.path()).unwrap();
        let root = v.root().to_path_buf();
        let path = root.join("Caps.MD");
        write_rel(&root, "Caps.MD", "# Caps\n");
        v.index.upsert_file(&root, &path).unwrap();
        assert!(v.index.get_by_rel("Caps.MD").is_some());
    }
}
