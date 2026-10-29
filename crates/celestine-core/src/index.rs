use crate::parse::{extract_note_data, Wikilink};
use crate::path::{canonical_note_target, checked_rel, resolve_vault_abs};
use crate::CoreError;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NoteId(pub u32);

#[derive(Debug, Clone)]
pub struct Heading {
    pub level: u8,
    pub text: String,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct NoteMeta {
    pub id: NoteId,
    pub path: PathBuf,
    /// Path relative to vault root, using `/`
    pub rel: String,
    pub title: String,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub headings: Vec<Heading>,
    pub wikilinks: Vec<Wikilink>,
    pub md_link_targets: Vec<String>,
    pub content: String,
    pub body: String,
}

#[derive(Debug, Default)]
pub struct VaultIndex {
    notes: HashMap<NoteId, NoteMeta>,
    /// lowercase basename (stem) -> note ids
    by_stem: HashMap<String, Vec<NoteId>>,
    /// lowercase alias -> note ids
    by_alias: HashMap<String, Vec<NoteId>>,
    /// rel path lowercase -> id
    by_rel: HashMap<String, NoteId>,
    next_id: u32,
}

impl VaultIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.notes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    pub fn notes(&self) -> impl Iterator<Item = &NoteMeta> {
        self.notes.values()
    }

    pub fn get(&self, id: NoteId) -> Option<&NoteMeta> {
        self.notes.get(&id)
    }

    pub fn get_by_rel(&self, rel: &str) -> Option<&NoteMeta> {
        let key = normalize_rel(rel);
        self.by_rel.get(&key).and_then(|id| self.notes.get(id))
    }

    pub fn clear(&mut self) {
        self.notes.clear();
        self.by_stem.clear();
        self.by_alias.clear();
        self.by_rel.clear();
        self.next_id = 0;
    }

    pub fn upsert_file(&mut self, vault_root: &Path, path: &Path) -> Result<NoteId, CoreError> {
        self.upsert_file_impl(vault_root, path).map(|(id, _)| id)
    }

    pub(crate) fn upsert_file_if_changed(
        &mut self,
        vault_root: &Path,
        path: &Path,
    ) -> Result<bool, CoreError> {
        self.upsert_file_impl(vault_root, path)
            .map(|(_, changed)| changed)
    }

    fn upsert_file_impl(
        &mut self,
        vault_root: &Path,
        path: &Path,
    ) -> Result<(NoteId, bool), CoreError> {
        let resolved = resolve_vault_abs(vault_root, path)?;
        let (path, rel) = canonical_note_target(resolved)?;
        if !rel.to_lowercase().ends_with(".md") {
            return Err(CoreError::NoteNotFound(rel));
        }
        // `rel` has already been checked against the canonical vault root.
        // The caller's root may use a symlink or a non-canonical OS spelling.
        if rel.split('/').any(|part| part.starts_with('.')) {
            return Err(CoreError::NoteNotFound(rel));
        }

        let content = std::fs::read_to_string(&path)?;
        let rel_key = normalize_rel(&rel);
        if let Some(&existing) = self.by_rel.get(&rel_key) {
            if let Some(meta) = self.notes.get(&existing) {
                if meta.path.as_path() == path.as_path() && meta.content == content {
                    return Ok((existing, false));
                }
            }
        }

        let parsed = extract_note_data(&content);
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| rel.clone());
        let title = parsed
            .frontmatter
            .title
            .clone()
            .unwrap_or_else(|| stem.clone());

        let id = if let Some(&existing) = self.by_rel.get(&rel_key) {
            self.remove_lookups(existing);
            existing
        } else {
            let id = NoteId(self.next_id);
            self.next_id += 1;
            id
        };

        let meta = NoteMeta {
            id,
            path: path.to_path_buf(),
            rel: rel.clone(),
            title,
            aliases: parsed.frontmatter.aliases.clone(),
            tags: parsed.tags.clone(),
            headings: parsed
                .headings
                .into_iter()
                .map(|(level, text, line)| Heading { level, text, line })
                .collect(),
            wikilinks: parsed.wikilinks,
            md_link_targets: parsed
                .md_links
                .into_iter()
                .map(|l| decode_md_dest(&l.dest))
                .collect(),
            content,
            body: parsed.body,
        };

        self.by_rel.insert(rel_key, id);
        let stem_key = stem.to_lowercase();
        self.by_stem.entry(stem_key).or_default().push(id);
        for a in &meta.aliases {
            self.by_alias.entry(a.to_lowercase()).or_default().push(id);
        }
        self.notes.insert(id, meta);
        Ok((id, true))
    }

    pub fn remove_path(&mut self, vault_root: &Path, path: &Path) {
        let Ok(rel) = checked_rel(vault_root, path) else {
            return;
        };
        let key = normalize_rel(&rel);
        if let Some(id) = self.by_rel.remove(&key) {
            self.remove_lookups(id);
            self.notes.remove(&id);
        }
    }

    fn remove_lookups(&mut self, id: NoteId) {
        if let Some(meta) = self.notes.get(&id) {
            let stem = Path::new(&meta.rel)
                .file_stem()
                .map(|s| s.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if let Some(v) = self.by_stem.get_mut(&stem) {
                v.retain(|x| *x != id);
            }
            for a in &meta.aliases {
                if let Some(v) = self.by_alias.get_mut(&a.to_lowercase()) {
                    v.retain(|x| *x != id);
                }
            }
        }
    }

    pub fn stems(&self) -> &HashMap<String, Vec<NoteId>> {
        &self.by_stem
    }

    pub fn aliases(&self) -> &HashMap<String, Vec<NoteId>> {
        &self.by_alias
    }
}

pub(crate) fn is_visible_path(vault_root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(vault_root) else {
        return false;
    };
    !relative
        .components()
        .any(|component| component.as_os_str().to_string_lossy().starts_with('.'))
}

pub fn normalize_rel(rel: &str) -> String {
    let mut s = rel.replace('\\', "/");
    if let Some(stripped) = s.strip_suffix(".md") {
        s = stripped.to_string();
    }
    // Also store with .md in by_rel — we normalize both ways in lookup
    s.to_lowercase()
}

pub fn decode_md_dest(dest: &str) -> String {
    let dest = dest.split(['#', '?']).next().unwrap_or(dest);
    percent_decode(dest)
        .strip_suffix(".md")
        .unwrap_or(&percent_decode(dest))
        .to_string()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{is_visible_path, VaultIndex};
    use std::fs;

    #[test]
    fn upsert_if_changed_skips_unchanged_content_and_updates_edits() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("note.md");
        fs::write(&path, "# Original\n").unwrap();

        let mut index = VaultIndex::new();
        assert!(index.upsert_file_if_changed(root.path(), &path).unwrap());
        let id = index.get_by_rel("note.md").unwrap().id;
        let content_ptr = index.get(id).unwrap().content.as_ptr();

        assert!(!index.upsert_file_if_changed(root.path(), &path).unwrap());
        assert_eq!(index.get_by_rel("note.md").unwrap().id, id);
        assert_eq!(index.get(id).unwrap().content.as_ptr(), content_ptr);

        fs::write(&path, "# Edited\n").unwrap();
        assert!(index.upsert_file_if_changed(root.path(), &path).unwrap());
        let note = index.get_by_rel("note.md").unwrap();
        assert_eq!(note.id, id);
        assert_eq!(note.title, "note");
        assert_eq!(note.headings[0].text, "Edited");
    }

    #[test]
    fn case_only_path_change_reuses_id_and_refreshes_metadata() {
        let root = tempfile::tempdir().unwrap();
        let old_path = root.path().join("note.md");
        let new_path = root.path().join("Note.md");
        fs::write(&old_path, "---\ntitle: Old\n---\n").unwrap();

        let mut index = VaultIndex::new();
        let id = index.upsert_file(root.path(), &old_path).unwrap();
        fs::rename(&old_path, &new_path).unwrap();
        fs::write(&new_path, "---\ntitle: New\n---\n").unwrap();

        assert!(index
            .upsert_file_if_changed(root.path(), &new_path)
            .unwrap());
        let note = index.get_by_rel("note.md").unwrap();
        assert_eq!(note.id, id);
        assert_eq!(note.path, new_path.canonicalize().unwrap());
        assert_eq!(note.title, "New");
    }

    #[test]
    fn visibility_rejects_dot_components_and_accepts_hidden_root() {
        let root = std::path::Path::new("/vault/.hidden/root");
        assert!(is_visible_path(root, root));
        assert!(is_visible_path(root, &root.join("notes/note.md")));
        assert!(!is_visible_path(root, &root.join(".private/note.md")));
        assert!(!is_visible_path(root, &root.join("notes/.draft/note.md")));
        assert!(!is_visible_path(root, &root.join("../outside.md")));
    }
}
