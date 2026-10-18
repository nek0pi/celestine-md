use crate::index::{normalize_rel, NoteId, VaultIndex};
use crate::parse::has_uri_scheme;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedLink {
    Note(NoteId),
    Missing { target: String },
    External { url: String },
}

/// Resolve a wikilink / internal target against the index.
pub fn resolve_link(index: &VaultIndex, from_rel: &str, target: &str) -> ResolvedLink {
    let target = target.trim();
    if has_uri_scheme(target) {
        return ResolvedLink::External {
            url: target.to_string(),
        };
    }

    // `[[#Heading]]` / `[t](#anchor)`: an empty target is the current note.
    let target = if target.is_empty() { from_rel.trim() } else { target };

    let cleaned = target
        .strip_suffix(".md")
        .unwrap_or(target)
        .replace('\\', "/");
    let key = normalize_rel(&cleaned);

    // Exact relative path from vault root
    if let Some(n) = index.get_by_rel(&cleaned) {
        return ResolvedLink::Note(n.id);
    }
    // With .md
    if let Some(n) = index.get_by_rel(&format!("{cleaned}.md")) {
        return ResolvedLink::Note(n.id);
    }

    // Relative to current note's folder
    if let Some(parent) = Path::new(from_rel).parent() {
        let joined = parent.join(&cleaned).to_string_lossy().replace('\\', "/");
        if let Some(n) = index.get_by_rel(&joined) {
            return ResolvedLink::Note(n.id);
        }
    }

    // Unique basename / stem
    let stem = Path::new(&cleaned)
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| key.clone());
    if let Some(ids) = index.stems().get(&stem) {
        if ids.len() == 1 {
            return ResolvedLink::Note(ids[0]);
        }
        // Prefer shortest path when multiple
        if let Some(best) = ids
            .iter()
            .filter_map(|id| index.get(*id))
            .min_by_key(|n| n.rel.len())
        {
            return ResolvedLink::Note(best.id);
        }
    }

    // Alias
    if let Some(ids) = index.aliases().get(&cleaned.to_lowercase()) {
        if let Some(&id) = ids.first() {
            return ResolvedLink::Note(id);
        }
    }

    ResolvedLink::Missing {
        target: cleaned,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn index_with(rels: &[(&str, &str)]) -> (tempfile::TempDir, VaultIndex) {
        let dir = tempfile::tempdir().unwrap();
        let mut index = VaultIndex::new();
        for (rel, content) in rels {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, content).unwrap();
            index.upsert_file(dir.path(), &path).unwrap();
        }
        (dir, index)
    }

    #[test]
    fn empty_target_resolves_to_current_note() {
        let (_d, index) = index_with(&[("Home.md", "# Home\n[[#Section]]\n")]);
        let home = index.get_by_rel("Home.md").unwrap().id;
        assert_eq!(resolve_link(&index, "Home.md", ""), ResolvedLink::Note(home));
        assert_eq!(resolve_link(&index, "Home.md", "  "), ResolvedLink::Note(home));
    }

    #[test]
    fn empty_target_from_unknown_note_is_not_dot_md() {
        let (_d, index) = index_with(&[("Home.md", "# Home\n")]);
        match resolve_link(&index, "Ghost.md", "") {
            ResolvedLink::Missing { target } => assert_eq!(target, "Ghost"),
            other => panic!("expected Missing, got {other:?}"),
        }
    }

    #[test]
    fn any_scheme_is_external() {
        let (_d, index) = index_with(&[("Home.md", "# Home\n")]);
        for url in ["mailto:x@y.z", "ftp://host/f", "obsidian://open?vault=x", "file:///tmp/a"] {
            assert_eq!(
                resolve_link(&index, "Home.md", url),
                ResolvedLink::External { url: url.to_string() },
                "{url}"
            );
        }
        // A colon followed by a space is a title, not a scheme.
        assert!(matches!(
            resolve_link(&index, "Home.md", "Meeting: notes"),
            ResolvedLink::Missing { .. }
        ));
        let _ = Path::new("");
    }
}
