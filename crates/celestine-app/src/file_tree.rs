use egui::{Color32, RichText, Sense, Stroke, Ui};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

#[derive(Default)]
pub struct DirNode {
    dirs: BTreeMap<String, DirNode>,
    files: BTreeMap<String, String>, // name -> full rel path
}

fn insert_path(root: &mut DirNode, rel: &str) {
    let parts: Vec<&str> = rel.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return;
    }
    let mut node = root;
    for (i, part) in parts.iter().enumerate() {
        if i + 1 == parts.len() {
            node.files.insert((*part).to_string(), rel.to_string());
        } else {
            node = node
                .dirs
                .entry((*part).to_string())
                .or_insert_with(DirNode::default);
        }
    }
}

pub fn build_tree(paths: &[String]) -> DirNode {
    let mut root = DirNode::default();
    for p in paths {
        insert_path(&mut root, p);
    }
    root
}

/// Built-in Lucide/flea marks for folders (right-click a folder to assign).
pub use crate::icons::FOLDER_ICON_CHOICES;

/// Sidebar tree typography (~25% larger than the old 12.5px).
pub const TREE_FONT: f32 = 15.6;
const TREE_ICON: f32 = 14.0;

/// Color for notes at the vault root (no parent folder) — theme muted, slightly lifted.
pub fn root_note_color(palette: &crate::theme_apply::ThemePalette) -> Color32 {
    crate::theme_apply::mix(palette.muted, palette.text, 0.35)
}

/// Rainbow color of a folder path (last component's sibling index under its parent).
pub fn color_for_folder(
    root: &DirNode,
    folder_path: &str,
    palette: &crate::theme_apply::ThemePalette,
) -> Option<Color32> {
    let parts: Vec<&str> = folder_path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return None;
    }
    let mut node = root;
    let mut color = None;
    for part in parts {
        let idx = node.dirs.keys().position(|k| k == part)?;
        color = Some(palette.folder_at(idx));
        node = node.dirs.get(part)?;
    }
    color
}

/// Color a note by the rainbow of its immediate parent folder.
pub fn color_for_rel(
    root: &DirNode,
    rel: &str,
    palette: &crate::theme_apply::ThemePalette,
) -> Color32 {
    let parent = Path::new(rel).parent().and_then(|p| {
        let s = p.to_string_lossy();
        if s.is_empty() || s == "." {
            None
        } else {
            Some(s.replace('\\', "/"))
        }
    });
    match parent {
        Some(folder) => color_for_folder(root, &folder, palette).unwrap_or_else(|| root_note_color(palette)),
        None => root_note_color(palette),
    }
}

pub struct TreeOutcome {
    pub open_rel: Option<String>,
    /// Folder path whose icon was changed (caller should persist).
    pub icons_changed: bool,
    /// Double-clicked (or requested) item to begin inline rename.
    pub start_rename: Option<TreeNavItem>,
    /// Clicked item — update keyboard cursor.
    pub focus_item: Option<TreeNavItem>,
    /// Rename field committed (Enter) → new basename only.
    pub rename_commit: Option<String>,
    pub rename_cancel: bool,
    /// Drag-and-drop move: source item → destination folder (`None` = vault root).
    pub move_request: Option<TreeMoveRequest>,
}

/// Request to move a tree entry into another folder (or vault root).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeMoveRequest {
    pub source: TreeNavItem,
    /// Destination parent folder (`None` / empty = vault root).
    pub dest_folder: Option<String>,
}

/// Inline rename editor for a tree row.
#[derive(Clone)]
pub struct RenameEdit {
    pub item: TreeNavItem,
    pub draft: String,
    pub request_focus: bool,
}

/// Keyboard / focus cursor in the file tree (distinct from the open note).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TreeNavItem {
    Dir(String),
    File(String),
}

impl TreeNavItem {
    pub fn rel(&self) -> &str {
        match self {
            Self::Dir(p) | Self::File(p) => p,
        }
    }

    pub fn is_dir(&self) -> bool {
        matches!(self, Self::Dir(_))
    }
}

/// Resolve a drag-and-drop move into `(old_rel, new_rel, is_dir)`.
/// Returns `None` when the move is a no-op or would nest a folder under itself.
pub fn resolve_tree_move(
    source: &TreeNavItem,
    dest_folder: Option<&str>,
) -> Option<(String, String, bool)> {
    let old_rel = source.rel();
    let is_dir = source.is_dir();
    let name = Path::new(old_rel).file_name()?.to_str()?;
    let dest = dest_folder.unwrap_or("").trim_matches('/');
    if is_dir {
        if dest == old_rel {
            return None;
        }
        if !dest.is_empty() && dest.starts_with(&format!("{old_rel}/")) {
            return None;
        }
    }
    let new_rel = if dest.is_empty() {
        name.to_string()
    } else {
        format!("{dest}/{name}")
    };
    if new_rel == old_rel {
        return None;
    }
    Some((old_rel.to_string(), new_rel, is_dir))
}

fn parent_folder_of(rel: &str) -> Option<String> {
    Path::new(rel)
        .parent()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .filter(|p| !p.is_empty() && p != ".")
}

/// Whether `source` can be dropped into `dest_folder` (`None` = vault root).
fn can_drop_into(source: &TreeNavItem, dest_folder: Option<&str>) -> bool {
    resolve_tree_move(source, dest_folder).is_some()
}

/// Visible rows in depth-first order (respects `expanded`).
pub fn visible_entries(root: &DirNode, expanded: &HashSet<String>) -> Vec<TreeNavItem> {
    let mut out = Vec::new();
    collect_visible(root, "", expanded, &mut out);
    out
}

fn collect_visible(
    node: &DirNode,
    prefix: &str,
    expanded: &HashSet<String>,
    out: &mut Vec<TreeNavItem>,
) {
    for (name, child) in &node.dirs {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        out.push(TreeNavItem::Dir(path.clone()));
        if expanded.contains(&path) {
            collect_visible(child, &path, expanded, out);
        }
    }
    for rel in node.files.values() {
        out.push(TreeNavItem::File(rel.clone()));
    }
}

/// Pick a sensible starting cursor: open note if visible, else first row.
pub fn default_cursor(
    root: &DirNode,
    expanded: &HashSet<String>,
    active_rel: Option<&str>,
) -> Option<TreeNavItem> {
    let entries = visible_entries(root, expanded);
    if entries.is_empty() {
        return None;
    }
    if let Some(rel) = active_rel {
        if let Some(hit) = entries
            .iter()
            .find(|e| matches!(e, TreeNavItem::File(p) if p == rel))
        {
            return Some(hit.clone());
        }
        // Prefer the note's parent folder if that row is visible.
        if let Some(parent) = Path::new(rel).parent() {
            let p = parent.to_string_lossy().replace('\\', "/");
            if !p.is_empty() && p != "." {
                if let Some(hit) = entries
                    .iter()
                    .find(|e| matches!(e, TreeNavItem::Dir(d) if d == &p))
                {
                    return Some(hit.clone());
                }
            }
        }
    }
    entries.into_iter().next()
}

/// Draw a collapsible file tree with rainbow folders + custom icons.
pub fn show_tree(
    ui: &mut Ui,
    root: &DirNode,
    expanded: &mut HashSet<String>,
    folder_icons: &mut HashMap<String, String>,
    palette: &crate::theme_apply::ThemePalette,
    muted: Color32,
    active_rel: Option<&str>,
    cursor: Option<&TreeNavItem>,
    tree_focused: bool,
    rename: &mut Option<RenameEdit>,
) -> TreeOutcome {
    let active_ancestors = active_ancestors(active_rel);
    let mut outcome = TreeOutcome {
        open_rel: None,
        icons_changed: false,
        start_rename: None,
        focus_item: None,
        rename_commit: None,
        rename_cancel: false,
        move_request: None,
    };
    show_dir(
        ui,
        root,
        "",
        root_note_color(palette),
        expanded,
        folder_icons,
        palette,
        muted,
        active_rel,
        &active_ancestors,
        cursor,
        tree_focused,
        rename,
        &mut outcome,
    );
    // Empty space below the tree (and leftover scroll height) → drop to vault root.
    let leftover = ui.available_height().max(24.0);
    let (root_rect, root_resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), leftover), Sense::hover());
    if drop_hover_active(&root_resp, None, rename.is_some()) {
        paint_drop_hover(ui, root_rect, palette);
    }
    accept_drop(&root_resp, None, rename.is_some(), &mut outcome);
    outcome
}

/// True when a compatible payload is hovering this drop target.
fn drop_hover_active(
    resp: &egui::Response,
    dest_folder: Option<&str>,
    renaming: bool,
) -> bool {
    if renaming {
        return false;
    }
    resp.dnd_hover_payload::<TreeNavItem>()
        .is_some_and(|payload| can_drop_into(&payload, dest_folder))
}

/// Accept a release onto `dest_folder` (`None` = vault root).
fn accept_drop(
    resp: &egui::Response,
    dest_folder: Option<&str>,
    renaming: bool,
    outcome: &mut TreeOutcome,
) {
    if renaming || outcome.move_request.is_some() {
        return;
    }
    if let Some(payload) = resp.dnd_release_payload::<TreeNavItem>() {
        if can_drop_into(&payload, dest_folder) {
            outcome.move_request = Some(TreeMoveRequest {
                source: (*payload).clone(),
                dest_folder: dest_folder
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string()),
            });
        }
    }
}

fn paint_drop_hover(ui: &Ui, row_rect: egui::Rect, palette: &crate::theme_apply::ThemePalette) {
    ui.painter().rect_filled(
        row_rect,
        2.0,
        palette.accent.gamma_multiply(0.16),
    );
    ui.painter().rect_stroke(
        row_rect,
        2.0,
        Stroke::new(1.5, palette.accent),
        egui::StrokeKind::Inside,
    );
}

fn active_ancestors(active_rel: Option<&str>) -> HashSet<String> {
    let mut set = HashSet::new();
    let Some(rel) = active_rel else {
        return set;
    };
    let mut acc = String::new();
    if let Some(parent) = Path::new(rel).parent() {
        for part in parent.components() {
            let p = part.as_os_str().to_string_lossy();
            if p == "." || p.is_empty() {
                continue;
            }
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(&p);
            set.insert(acc.clone());
        }
    }
    set
}

fn show_dir(
    ui: &mut Ui,
    node: &DirNode,
    prefix: &str,
    // Color of this directory — files listed here inherit it; child folders pick their own.
    folder_color: Color32,
    expanded: &mut HashSet<String>,
    folder_icons: &mut HashMap<String, String>,
    palette: &crate::theme_apply::ThemePalette,
    muted: Color32,
    active_rel: Option<&str>,
    active_ancestors: &HashSet<String>,
    cursor: Option<&TreeNavItem>,
    tree_focused: bool,
    rename: &mut Option<RenameEdit>,
    outcome: &mut TreeOutcome,
) {
    for (sibling_i, (name, child)) in node.dirs.iter().enumerate() {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let is_open = expanded.contains(&path);
        let on_path = active_ancestors.contains(&path);
        let is_cursor = matches!(cursor, Some(TreeNavItem::Dir(p)) if p == &path);
        let is_renaming = matches!(
            rename.as_ref(),
            Some(r) if matches!(&r.item, TreeNavItem::Dir(p) if p == &path)
        );
        // Child folders cycle the theme rainbow among siblings (not inherited from parent).
        let own_color = palette.folder_at(sibling_i);
        let mut color = own_color;
        if on_path {
            // Brighten ancestors of the open note so the active branch pops.
            color = brighten(color, 1.18);
        }
        let custom = folder_icons.get(&path).map(|s| s.as_str());
        let folder_mark = match custom {
            Some(c) => crate::icons::normalize_icon_name(c),
            None if is_open => "folder-open",
            None => "folder",
        };
        let chevron = if is_open {
            "chevron-down"
        } else {
            "chevron-right"
        };

        let row_h = TREE_FONT + 6.0;
        let row_w = ui.available_width();
        let sense = if is_renaming {
            Sense::hover()
        } else {
            Sense::click_and_drag()
        };
        let (row_rect, resp) = ui.allocate_exact_size(egui::vec2(row_w, row_h), sense);
        let drop_hover = drop_hover_active(&resp, Some(&path), is_renaming);
        if drop_hover {
            paint_drop_hover(ui, row_rect, palette);
        } else if is_cursor && tree_focused && !is_renaming {
            ui.painter().rect_filled(
                row_rect,
                2.0,
                palette.accent.gamma_multiply(0.28),
            );
        } else if on_path {
            ui.painter().rect_filled(
                row_rect,
                2.0,
                color.gamma_multiply(0.18),
            );
        }

        let mut rename_resp: Option<egui::Response> = None;
        ui.scope_builder(egui::UiBuilder::new().max_rect(row_rect), |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(row_h);
                ui.spacing_mut().item_spacing.x = 4.0;
                crate::icons::show(ui, chevron, TREE_ICON * 0.85, color);
                crate::icons::show(ui, folder_mark, TREE_ICON, color);
                if is_renaming {
                    if let Some(edit) = rename.as_mut() {
                        let te = egui::TextEdit::singleline(&mut edit.draft)
                            .font(egui::FontId::proportional(TREE_FONT))
                            .desired_width((row_w - 48.0).max(60.0))
                            .frame(egui::Frame::NONE);
                        let r = ui.add(te);
                        if edit.request_focus {
                            r.request_focus();
                            edit.request_focus = false;
                        }
                        rename_resp = Some(r);
                    }
                } else {
                    // Non-selectable so clicks hit the row (selectable labels steal them).
                    ui.add(
                        egui::Label::new(
                            RichText::new(name).size(TREE_FONT).color(color).strong(),
                        )
                        .selectable(false),
                    );
                }
            });
        });

        if !is_renaming {
            resp.dnd_set_drag_payload(TreeNavItem::Dir(path.clone()));
            // Auto-expand while hovering a valid drop so nested targets become reachable.
            if drop_hover {
                expanded.insert(path.clone());
            }
            accept_drop(&resp, Some(&path), false, outcome);
        }

        if let Some(_r) = rename_resp {
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                outcome.rename_cancel = true;
            } else if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                outcome.rename_commit = rename.as_ref().map(|e| e.draft.clone());
            }
        } else if resp.double_clicked() {
            outcome.start_rename = Some(TreeNavItem::Dir(path.clone()));
            outcome.focus_item = Some(TreeNavItem::Dir(path.clone()));
        } else if resp.clicked() {
            outcome.focus_item = Some(TreeNavItem::Dir(path.clone()));
            if is_open {
                expanded.remove(&path);
            } else {
                expanded.insert(path.clone());
            }
        }

        resp.context_menu(|ui| {
            if ui
                .add(
                    egui::Button::new(RichText::new("rename").size(12.0).color(color))
                        .frame(false),
                )
                .clicked()
            {
                outcome.start_rename = Some(TreeNavItem::Dir(path.clone()));
                ui.close();
            }
            ui.label(RichText::new("folder mark").size(11.0).color(muted));
            ui.add_space(4.0);
            egui::Grid::new(format!("icon-grid-{path}"))
                .num_columns(6)
                .spacing([6.0, 6.0])
                .show(ui, |ui| {
                    for (i, mark) in FOLDER_ICON_CHOICES.iter().enumerate() {
                        let r = crate::icons::button(ui, mark, 18.0, color);
                        if r.clicked() {
                            folder_icons
                                .insert(path.clone(), (*mark).to_string());
                            outcome.icons_changed = true;
                            ui.close();
                        }
                        r.on_hover_text(*mark);
                        if (i + 1) % 6 == 0 {
                            ui.end_row();
                        }
                    }
                    ui.end_row();
                });
            ui.add_space(4.0);
            if ui
                .add(
                    egui::Button::new(RichText::new("reset mark").size(12.0).color(muted))
                        .frame(false),
                )
                .clicked()
            {
                folder_icons.remove(&path);
                outcome.icons_changed = true;
                ui.close();
            }
        });

        if expanded.contains(&path) {
            ui.indent(format!("dir-{path}"), |ui| {
                // Notes inside this folder take its rainbow; nested folders get new sibling hues.
                show_dir(
                    ui,
                    child,
                    &path,
                    own_color,
                    expanded,
                    folder_icons,
                    palette,
                    muted,
                    active_rel,
                    active_ancestors,
                    cursor,
                    tree_focused,
                    rename,
                    outcome,
                );
            });
        }
    }

    for (name, rel) in &node.files {
        let selected = active_rel == Some(rel.as_str());
        let is_cursor = matches!(cursor, Some(TreeNavItem::File(p)) if p == rel);
        let is_renaming = matches!(
            rename.as_ref(),
            Some(r) if matches!(&r.item, TreeNavItem::File(p) if p == rel)
        );
        // Files match the enclosing folder's color (latest parent), not muted/accent.
        let mut color = folder_color;
        if selected {
            color = brighten(color, 1.2);
        }
        let row_h = TREE_FONT + 6.0;
        let row_w = ui.available_width();
        let sense = if is_renaming {
            Sense::hover()
        } else {
            Sense::click_and_drag()
        };
        let (row_rect, resp) = ui.allocate_exact_size(egui::vec2(row_w, row_h), sense);
        // Dropping onto a file moves into that file's parent folder.
        let file_drop_dest = parent_folder_of(rel);
        let drop_hover = drop_hover_active(&resp, file_drop_dest.as_deref(), is_renaming);
        if drop_hover {
            paint_drop_hover(ui, row_rect, palette);
        } else if is_cursor && tree_focused && !is_renaming {
            ui.painter().rect_filled(
                row_rect,
                2.0,
                palette.accent.gamma_multiply(0.28),
            );
        } else if selected {
            ui.painter().rect_filled(
                row_rect,
                2.0,
                folder_color.gamma_multiply(0.22),
            );
        }

        let mut rename_resp: Option<egui::Response> = None;
        ui.scope_builder(egui::UiBuilder::new().max_rect(row_rect), |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(row_h);
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.add_space(TREE_ICON * 0.85 + 4.0);
                let file_icon = if crate::media::is_image_rel(rel) {
                    "image"
                } else if rel.to_lowercase().ends_with(".canvas") {
                    "canvas"
                } else {
                    "file-text"
                };
                crate::icons::show(ui, file_icon, TREE_ICON, color);
                if is_renaming {
                    if let Some(edit) = rename.as_mut() {
                        let te = egui::TextEdit::singleline(&mut edit.draft)
                            .font(egui::FontId::proportional(TREE_FONT))
                            .desired_width((row_w - 48.0).max(60.0))
                            .frame(egui::Frame::NONE);
                        let r = ui.add(te);
                        if edit.request_focus {
                            r.request_focus();
                            edit.request_focus = false;
                        }
                        rename_resp = Some(r);
                    }
                } else {
                    let mut label = RichText::new(name).size(TREE_FONT).color(color);
                    if selected {
                        label = label.strong();
                    }
                    ui.add(egui::Label::new(label).selectable(false));
                }
            });
        });

        if !is_renaming {
            resp.dnd_set_drag_payload(TreeNavItem::File(rel.clone()));
            accept_drop(&resp, file_drop_dest.as_deref(), false, outcome);
        }

        if let Some(_r) = rename_resp {
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                outcome.rename_cancel = true;
            } else if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                outcome.rename_commit = rename.as_ref().map(|e| e.draft.clone());
            }
        } else if resp.double_clicked() {
            outcome.start_rename = Some(TreeNavItem::File(rel.clone()));
            outcome.focus_item = Some(TreeNavItem::File(rel.clone()));
        } else if resp.clicked() {
            outcome.focus_item = Some(TreeNavItem::File(rel.clone()));
            outcome.open_rel = Some(rel.clone());
        }
    }
}

fn brighten(c: Color32, factor: f32) -> Color32 {
    Color32::from_rgb(
        ((c.r() as f32 * factor).min(255.0)) as u8,
        ((c.g() as f32 * factor).min(255.0)) as u8,
        ((c.b() as f32 * factor).min(255.0)) as u8,
    )
}

/// Load/save per-vault folder icon map from `.celestine/folder-icons.toml`.
pub fn load_folder_icons(vault: &celestine_core::Vault) -> HashMap<String, String> {
    let Ok(text) = vault.read_metadata("folder-icons.toml") else {
        return HashMap::new();
    };
    #[derive(serde::Deserialize, Default)]
    struct File {
        #[serde(default)]
        icons: HashMap<String, String>,
    }
    let mut icons = toml::from_str::<File>(&text)
        .map(|f| f.icons)
        .unwrap_or_default();
    // Migrate legacy emoji assignments → Lucide/flea mark names.
    for v in icons.values_mut() {
        *v = crate::icons::normalize_icon_name(v).to_string();
    }
    icons
}

pub fn save_folder_icons(vault: &celestine_core::Vault, icons: &HashMap<String, String>) {
    let mut map = BTreeMap::new();
    for (k, v) in icons {
        map.insert(k.clone(), v.clone());
    }
    let body = match toml::to_string_pretty(&FolderIconsFile { icons: map }) {
        Ok(s) => s,
        Err(_) => return,
    };
    let _ = vault.write_metadata("folder-icons.toml", &body);
}

#[cfg(test)]
mod tree_move_tests {
    use super::*;

    #[test]
    fn move_file_into_folder() {
        let src = TreeNavItem::File("Note.md".into());
        let (old, new, is_dir) = resolve_tree_move(&src, Some("Projects")).unwrap();
        assert_eq!(old, "Note.md");
        assert_eq!(new, "Projects/Note.md");
        assert!(!is_dir);
    }

    #[test]
    fn move_file_to_vault_root() {
        let src = TreeNavItem::File("Projects/Note.md".into());
        let (old, new, _) = resolve_tree_move(&src, None).unwrap();
        assert_eq!(old, "Projects/Note.md");
        assert_eq!(new, "Note.md");
    }

    #[test]
    fn same_parent_is_noop() {
        let src = TreeNavItem::File("Projects/Note.md".into());
        assert!(resolve_tree_move(&src, Some("Projects")).is_none());
    }

    #[test]
    fn cannot_move_folder_into_itself_or_descendant() {
        let src = TreeNavItem::Dir("Projects".into());
        assert!(resolve_tree_move(&src, Some("Projects")).is_none());
        assert!(resolve_tree_move(&src, Some("Projects/Nested")).is_none());
        let (old, new, is_dir) = resolve_tree_move(&src, Some("Archive")).unwrap();
        assert_eq!(old, "Projects");
        assert_eq!(new, "Archive/Projects");
        assert!(is_dir);
    }
}

#[cfg(test)]
mod folder_icon_boundary_tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn folder_icons_do_not_read_or_write_through_external_metadata_symlink() {
        use std::os::unix::fs::symlink;

        let vault = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let sentinel = outside.path().join("folder-icons.toml");
        std::fs::write(&sentinel, "[icons]\n\"notes\" = \"heart\"\n").unwrap();
        symlink(outside.path(), vault.path().join(".celestine")).unwrap();

        let vault = celestine_core::Vault::open(vault.path()).unwrap();
        assert!(load_folder_icons(&vault).is_empty());
        save_folder_icons(&vault, &HashMap::from([("notes".into(), "star".into())]));
        assert_eq!(
            std::fs::read_to_string(sentinel).unwrap(),
            "[icons]\n\"notes\" = \"heart\"\n"
        );
    }
}

#[derive(serde::Serialize)]
struct FolderIconsFile {
    icons: BTreeMap<String, String>,
}
