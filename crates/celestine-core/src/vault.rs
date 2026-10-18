use crate::graph::LinkGraph;
use crate::index::VaultIndex;
use crate::path::{canonical_note_target, checked_rel, resolve_vault_path};
use crate::CoreError;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use walkdir::WalkDir;

pub struct Vault {
    root: PathBuf,
    pub index: VaultIndex,
    pub graph: LinkGraph,
    _watcher: Option<RecommendedWatcher>,
    rx: Option<Receiver<Result<Event, notify::Error>>>,
}

impl Vault {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, CoreError> {
        let root = root.as_ref().canonicalize()?;
        if !root.is_dir() {
            return Err(CoreError::NotADirectory(root));
        }
        let mut index = VaultIndex::new();
        for entry in WalkDir::new(&root)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let _ = index.upsert_file(&root, path);
        }
        let graph = LinkGraph::rebuild(&index);

        let (tx, rx) = mpsc::channel();
        let watcher = RecommendedWatcher::new(
            move |res| {
                let _ = tx.send(res);
            },
            notify::Config::default(),
        )
        .ok()
        .and_then(|mut w| {
            if w.watch(&root, RecursiveMode::Recursive).is_ok() {
                Some(w)
            } else {
                None
            }
        });

        Ok(Self {
            root,
            index,
            graph,
            _watcher: watcher,
            rx: Some(rx),
        })
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

    /// Drain filesystem events and update index.
    pub fn poll_fs(&mut self) -> bool {
        let Some(rx) = &self.rx else {
            return false;
        };
        let mut dirty = false;
        let mut graph_dirty = false;
        while let Ok(Ok(event)) = rx.try_recv() {
            match event.kind {
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                    for path in event.paths {
                        dirty = true;
                        if path.extension().and_then(|e| e.to_str()) != Some("md") {
                            continue;
                        }
                        if path.exists() {
                            let _ = self.index.upsert_file(&self.root, &path);
                        } else {
                            self.index.remove_path(&self.root, &path);
                        }
                        graph_dirty = true;
                    }
                }
                _ => {}
            }
        }
        if graph_dirty {
            self.rebuild_graph();
        }
        dirty
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
        if new_path.exists() {
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
                let abs_old = self.root.join(&rel);
                self.index.remove_path(&self.root, &abs_old);
                let rest = rel.strip_prefix(&old_rel).unwrap_or("");
                let new_note_rel = format!("{new_rel}{rest}");
                let abs_new = self.root.join(&new_note_rel);
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
