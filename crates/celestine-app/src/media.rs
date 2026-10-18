//! Image path helpers for vault media (preview embeds + standalone viewer).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp"];
const MAX_CACHED_RESOLUTIONS: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedImage {
    /// Canonical path to the image target, suitable for rendering and opening.
    pub abs: PathBuf,
    /// Vault-relative path for opening the image in the standalone viewer.
    pub rel: String,
}

/// Caches image lookups for the currently previewed note.
#[derive(Default)]
pub struct ImageResolver {
    context: Option<ResolverContext>,
    resolutions: HashMap<String, Option<ResolvedImage>>,
    #[cfg(test)]
    uncached_resolutions: usize,
}

#[derive(PartialEq, Eq)]
struct ResolverContext {
    root: PathBuf,
    note_rel: String,
    body: String,
}

impl ImageResolver {
    /// Set the current preview context. The vault root must already be canonical.
    /// Changing the root, note, or body drops both successful and failed lookups.
    pub fn set_context(&mut self, canonical_root: &Path, note_rel: &str, body: &str) {
        if self.context.as_ref().is_some_and(|context| {
            context.root == canonical_root && context.note_rel == note_rel && context.body == body
        }) {
            return;
        }
        let context = ResolverContext {
            root: canonical_root.to_path_buf(),
            note_rel: note_rel.to_string(),
            body: body.to_string(),
        };
        if self.context.as_ref() != Some(&context) {
            self.context = Some(context);
            self.resolutions.clear();
        }
    }

    /// Clear cached lookups after filesystem changes or a change to the image inventory.
    pub fn clear(&mut self) {
        self.resolutions.clear();
    }

    /// Resolve a destination once per preview context and image inventory.
    /// Call `clear` when the inventory changes.
    pub fn resolve(&mut self, dest: &str, known_image_rels: &[String]) -> Option<ResolvedImage> {
        let context = self.context.as_ref()?;
        let key = dest.to_string();
        if let Some(cached) = self.resolutions.get(&key) {
            return cached
                .as_ref()
                .filter(|image| cached_target_is_safe(&context.root, &image.abs))
                .cloned();
        }

        #[cfg(test)]
        {
            self.uncached_resolutions += 1;
        }

        let resolved =
            resolve_image_uncached(&context.root, &context.note_rel, dest, known_image_rels);
        if self.resolutions.len() >= MAX_CACHED_RESOLUTIONS {
            self.resolutions.clear();
        }
        self.resolutions.insert(key, resolved.clone());
        resolved
    }
}

fn cached_target_is_safe(canonical_root: &Path, abs: &Path) -> bool {
    abs.canonicalize()
        .is_ok_and(|canonical| canonical == abs && canonical.starts_with(canonical_root))
}

pub fn is_image_ext(ext: &str) -> bool {
    let e = ext.trim_start_matches('.').to_ascii_lowercase();
    IMAGE_EXTS.iter().any(|x| *x == e)
}

pub fn is_image_rel(rel: &str) -> bool {
    Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(is_image_ext)
}

pub fn is_image_target(target: &str) -> bool {
    let t = target.split('#').next().unwrap_or(target).trim();
    let t = t.split('|').next().unwrap_or(t).trim();
    is_image_rel(t)
}

/// `file://` URI for egui_extras FileLoader.
pub fn path_to_file_uri(path: &Path) -> String {
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let s = abs.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        // Windows drive paths: C:/...
        format!("file:///{s}")
    }
}

/// Resolve a markdown / wikilink image destination to an absolute path under the vault.
#[cfg(test)]
pub fn resolve_image_abs(
    vault_root: &Path,
    note_rel: &str,
    dest: &str,
    known_image_rels: &[String],
) -> Option<PathBuf> {
    resolve_image_uncached(vault_root, note_rel, dest, known_image_rels).map(|image| image.abs)
}

fn resolve_image_uncached(
    vault_root: &Path,
    note_rel: &str,
    dest: &str,
    known_image_rels: &[String],
) -> Option<ResolvedImage> {
    let dest = dest.trim().strip_prefix("wikilink:").unwrap_or(dest.trim());
    if dest.is_empty()
        || dest.starts_with("http://")
        || dest.starts_with("https://")
        || dest.starts_with("data:")
    {
        return None;
    }
    let cleaned = dest.replace('\\', "/");
    let cleaned = cleaned.trim_start_matches('/');

    let mut candidates: Vec<PathBuf> = Vec::new();
    candidates.push(PathBuf::from(cleaned));
    if let Some(parent) = Path::new(note_rel).parent() {
        let parent_s = parent.to_string_lossy();
        if !parent_s.is_empty() && parent_s != "." {
            candidates.push(parent.join(cleaned));
        }
    }

    for rel in candidates {
        if let Ok(path) = celestine_core::resolve_vault_path(vault_root, &rel) {
            if path.is_file() {
                return resolved_image(vault_root, &path);
            }
        }
    }

    // Basename search (Obsidian-style) against known vault images.
    let want = Path::new(cleaned)
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())?;
    for rel in known_image_rels {
        let name = Path::new(rel)
            .file_name()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if name == want {
            if let Ok(path) = celestine_core::resolve_vault_path(vault_root, Path::new(rel)) {
                if path.is_file() {
                    return resolved_image(vault_root, &path);
                }
            }
        }
    }
    None
}

fn resolved_image(vault_root: &Path, path: &Path) -> Option<ResolvedImage> {
    let canonical_root = vault_root.canonicalize().ok()?;
    let abs = path.canonicalize().ok()?;
    let rel = abs.strip_prefix(&canonical_root).ok()?;
    let rel = rel.to_string_lossy().replace('\\', "/");
    Some(ResolvedImage { abs, rel })
}

pub fn skip_hidden_rel(rel: &str) -> bool {
    rel.split('/').any(|p| p.starts_with('.'))
}

/// Show an image fitted to the available width (preview / inline).
pub fn show_fitted_image(ui: &mut egui::Ui, abs: &Path, max_width: f32) -> egui::Response {
    let uri = path_to_file_uri(abs);
    let max_w = max_width.max(32.0);
    ui.add(
        egui::Image::new(uri)
            .max_width(max_w)
            .maintain_aspect_ratio(true)
            .sense(egui::Sense::click()),
    )
}

/// Full-pane standalone image viewer.
pub fn show_standalone(ui: &mut egui::Ui, abs: &Path, muted: egui::Color32) {
    if !abs.is_file() {
        ui.centered_and_justified(|ui| {
            ui.label(
                egui::RichText::new(format!("missing image\n{}", abs.display()))
                    .monospace()
                    .color(muted),
            );
        });
        return;
    }
    let uri = path_to_file_uri(abs);
    let avail = ui.available_size();
    ui.centered_and_justified(|ui| {
        ui.add(
            egui::Image::new(uri)
                .fit_to_exact_size(avail)
                .maintain_aspect_ratio(true),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn image_resolution_rejects_absolute_and_escaping_destinations() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("outside.png"), b"outside").unwrap();

        let sibling_name = outside.path().file_name().unwrap().to_string_lossy();
        let dest = format!("../{sibling_name}/outside.png");
        assert!(
            resolve_image_abs(root.path(), "notes/page.md", &dest, &[]).is_none(),
            "accepted outside destination {dest:?}"
        );
        let canonical_root = root.path().canonicalize().unwrap();
        let mut resolver = ImageResolver::default();
        resolver.set_context(&canonical_root, "notes/page.md", "![outside](outside.png)");
        assert!(resolver.resolve(&dest, &[]).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn image_resolution_rejects_external_symlinks_but_accepts_internal_ones() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.png"), b"outside").unwrap();
        fs::create_dir(root.path().join("images")).unwrap();
        fs::write(root.path().join("images/inside.png"), b"inside").unwrap();
        symlink(
            outside.path().join("secret.png"),
            root.path().join("external.png"),
        )
        .unwrap();
        symlink(
            outside.path().join("missing.png"),
            root.path().join("dangling.png"),
        )
        .unwrap();
        symlink(
            root.path().join("images/inside.png"),
            root.path().join("internal.png"),
        )
        .unwrap();

        assert!(resolve_image_abs(root.path(), "", "external.png", &[]).is_none());
        assert!(resolve_image_abs(root.path(), "", "dangling.png", &[]).is_none());
        let resolved = resolve_image_abs(root.path(), "", "internal.png", &[]).unwrap();
        assert_eq!(
            fs::canonicalize(resolved).unwrap(),
            fs::canonicalize(root.path().join("images/inside.png")).unwrap()
        );
    }

    #[test]
    fn image_resolution_keeps_in_vault_navigation_and_basename_search() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("images")).unwrap();
        fs::create_dir_all(root.path().join("notes/sub")).unwrap();
        fs::write(root.path().join("images/shared.png"), b"root-relative").unwrap();
        fs::write(root.path().join("notes/sub/near.png"), b"note-relative").unwrap();
        fs::write(root.path().join("notes/local.png"), b"parent navigation").unwrap();
        let known = vec!["images/shared.png".to_string()];

        for (note, dest, expected) in [
            (
                "notes/sub/page.md",
                "images/shared.png",
                "images/shared.png",
            ),
            ("notes/sub/page.md", "near.png", "notes/sub/near.png"),
            ("notes/sub/page.md", "../local.png", "notes/local.png"),
            ("notes/page.md", "shared.png", "images/shared.png"),
        ] {
            let resolved = resolve_image_abs(root.path(), note, dest, &known).unwrap();
            assert_eq!(
                fs::canonicalize(resolved).unwrap(),
                fs::canonicalize(root.path().join(expected)).unwrap(),
                "wrong target for {dest:?} from {note:?}"
            );
        }
    }

    #[test]
    fn image_resolver_reuses_positive_and_negative_results_until_cleared() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("present.png"), b"image").unwrap();
        let canonical_root = root.path().canonicalize().unwrap();
        let mut resolver = ImageResolver::default();
        resolver.set_context(
            &canonical_root,
            "page.md",
            "![](present.png) ![](later.png)",
        );

        let first = resolver.resolve("present.png", &[]).unwrap();
        let repeated = resolver.resolve("present.png", &[]).unwrap();
        assert_eq!(first, repeated);
        assert_eq!(resolver.uncached_resolutions, 1);

        assert!(resolver.resolve("later.png", &[]).is_none());
        fs::write(root.path().join("later.png"), b"image").unwrap();
        assert!(resolver.resolve("later.png", &[]).is_none());
        resolver.clear();
        assert!(resolver.resolve("later.png", &[]).is_some());
        assert_eq!(resolver.uncached_resolutions, 3);
    }

    #[cfg(unix)]
    #[test]
    fn image_resolver_rejects_symlink_escape_after_invalidation() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(root.path().join("inside.png"), b"inside").unwrap();
        fs::write(outside.path().join("secret.png"), b"outside").unwrap();
        let alias = root.path().join("alias.png");
        symlink(root.path().join("inside.png"), &alias).unwrap();

        let canonical_root = root.path().canonicalize().unwrap();
        let mut resolver = ImageResolver::default();
        resolver.set_context(&canonical_root, "page.md", "![](alias.png)");
        let resolved = resolver.resolve("alias.png", &[]).unwrap();
        assert_eq!(resolved.abs, canonical_root.join("inside.png"));

        fs::remove_file(&alias).unwrap();
        symlink(outside.path().join("secret.png"), &alias).unwrap();
        // The cached canonical target remains inside the vault. Clearing the cache
        // forces resolution of the redirected alias, which must be rejected.
        assert_eq!(resolver.resolve("alias.png", &[]).unwrap(), resolved);
        resolver.clear();
        assert!(resolver.resolve("alias.png", &[]).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn image_resolver_rejects_cached_target_replaced_by_external_symlink() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = root.path().join("inside.png");
        fs::write(&target, b"inside").unwrap();
        let outside_target = outside.path().join("secret.png");
        fs::write(&outside_target, b"outside").unwrap();

        let canonical_root = root.path().canonicalize().unwrap();
        let mut resolver = ImageResolver::default();
        resolver.set_context(&canonical_root, "page.md", "![](inside.png)");
        assert!(resolver.resolve("inside.png", &[]).is_some());

        fs::remove_file(&target).unwrap();
        symlink(&outside_target, &target).unwrap();
        assert!(resolver.resolve("inside.png", &[]).is_none());
    }
}
