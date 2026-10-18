use crate::file_tree;
use crate::graph_view::GraphState;
use crate::keybinds::{ensure_user_config, Command, Keymap};
use crate::md_edit;
use crate::outline_tree;
use crate::preview;
use crate::state::AppState;
use crate::theme_apply::{self, color, naked_button, naked_icon, sidebar_toggle};
use crate::update_check::UpdateChecker;
use celestine_core::{extract_note_data, NoteId, Vault};
use celestine_theme::{list_available_themes, load_theme, ThemeTokens};
use egui::text::{CCursor, CCursorRange};
use egui::{FontId, RichText, ScrollArea, Stroke, TextEdit, Ui};
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Overlay {
    None,
    QuickOpen,
    CommandPalette,
    VaultSearch,
    Shortcuts,
    Settings,
    Find,
    ThemePicker,
    RecentVaults,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FocusPane {
    Files,
    Editor,
    Outline,
    Graph,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TabKind {
    Note,
    Image,
}

struct OpenTab {
    rel: String,
    kind: TabKind,
    buffer: String,
    dirty: bool,
    preview_mode: bool,
    /// Jump outline → editor/preview
    scroll_to_line: Option<usize>,
    scroll_to_heading: Option<String>,
    /// Prefer this char offset (find / vault search hit) over line/heading.
    scroll_to_char: Option<usize>,
    editor_text_id: egui::Id,
}

/// Brief highlight after jumping to a find / search hit.
struct LineFlash {
    /// 0-based source line in the note buffer.
    line_0: usize,
    /// Text used to locate the block in preview (usually the find query).
    needle: String,
    started: Instant,
    /// Scroll-to-center still pending.
    do_scroll: bool,
}

const LINE_FLASH_SECS: f32 = 0.95;

pub struct CelestineApp {
    vault: Option<Vault>,
    tabs: Vec<OpenTab>,
    active_tab: usize,
    former_tab: usize,
    keymap: Keymap,
    theme: ThemeTokens,
    theme_name: String,
    overlay: Overlay,
    overlay_query: String,
    overlay_selected: usize,
    overlay_anim: f32,
    focus: FocusPane,
    show_graph: bool,
    graph: GraphState,
    status: String,
    nav_back: Vec<String>,
    nav_forward: Vec<String>,
    find_query: String,
    editor_focused: bool,
    /// One-shot: put the text caret back in the note editor (e.g. after Ctrl+Tab).
    editor_focus_pending: bool,
    last_frame: Instant,
    persist: AppState,
    tree_expanded: HashSet<String>,
    /// Keyboard cursor in the file tree (when Files pane is focused).
    tree_cursor: Option<file_tree::TreeNavItem>,
    /// Inline rename editor for a tree row.
    tree_rename: Option<file_tree::RenameEdit>,
    /// Outline paths that are collapsed (default = expanded).
    outline_collapsed: HashSet<String>,
    /// One-shot: grab keyboard focus into the overlay filter on open.
    overlay_focus_pending: bool,
    /// Per-vault custom folder icons (rel path → emoji).
    folder_icons: HashMap<String, String>,
    /// Most-recently-opened notes/images in the current vault (newest first).
    recent_notes: Vec<String>,
    /// Background GitHub release check (feature-line bumps only).
    update_checker: UpdateChecker,
    /// Hide the update banner for this session after dismiss/open.
    update_banner_hidden: bool,
    image_resolver: crate::media::ImageResolver,
    image_rels: Option<Vec<String>>,
    /// Fade-out wash on the jumped-to line (edit + preview).
    line_flash: Option<LineFlash>,
}

impl CelestineApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        ensure_user_config();
        egui_extras::install_image_loaders(&cc.egui_ctx);

        let mut persist = AppState::load();
        persist.clear_recent_missing();

        let theme_name = std::env::var("CELESTINE_THEME")
            .ok()
            .or_else(|| persist.theme.clone())
            .unwrap_or_else(|| {
                if crate::keybinds::is_omarchy() {
                    "terminal".into()
                } else {
                    "neovoid".into()
                }
            });
        let theme = load_theme(&theme_name).unwrap_or_default();
        crate::fonts::install(&cc.egui_ctx);
        theme_apply::apply_theme(&cc.egui_ctx, &theme, persist.ui_font_scale);

        let mut app = Self::with_parts(
            Keymap::detect_and_load(),
            theme,
            theme_name,
            persist,
            UpdateChecker::start(env!("CARGO_PKG_VERSION")),
        );

        if let Some(path) = std::env::args().nth(1) {
            app.open_vault(PathBuf::from(path), &cc.egui_ctx);
        } else if let Some(path) = app.persist.vault_to_restore() {
            app.open_vault(path, &cc.egui_ctx);
        }
        app
    }

    fn with_parts(
        keymap: Keymap,
        theme: ThemeTokens,
        theme_name: String,
        persist: AppState,
        update_checker: UpdateChecker,
    ) -> Self {
        Self {
            vault: None,
            tabs: Vec::new(),
            active_tab: 0,
            former_tab: 0,
            keymap,
            theme,
            theme_name,
            overlay: Overlay::None,
            overlay_query: String::new(),
            overlay_selected: 0,
            overlay_anim: 0.0,
            focus: FocusPane::Editor,
            show_graph: false,
            graph: GraphState::new(),
            status: format!(
                "open vault {} · recent {} · themes {} · ?",
                "Mod+Shift+O", "Mod+R", "Mod+T"
            ),
            nav_back: Vec::new(),
            nav_forward: Vec::new(),
            find_query: String::new(),
            editor_focused: false,
            editor_focus_pending: false,
            last_frame: Instant::now(),
            persist,
            tree_expanded: HashSet::new(),
            tree_cursor: None,
            tree_rename: None,
            outline_collapsed: HashSet::new(),
            overlay_focus_pending: false,
            folder_icons: HashMap::new(),
            recent_notes: Vec::new(),
            update_checker,
            update_banner_hidden: false,
            image_resolver: crate::media::ImageResolver::default(),
            image_rels: None,
            line_flash: None,
        }
    }

    fn open_vault(&mut self, path: PathBuf, ctx: &egui::Context) {
        let wake_ctx = ctx.clone();
        match Vault::open_with_wakeup(&path, move || wake_ctx.request_repaint()) {
            Ok(v) => {
                self.status = if let Some(error) = v.watcher_error() {
                    format!("vault {} · filesystem polling (watcher: {error})", path.display())
                } else {
                    format!("vault {}", path.display())
                };
                self.image_resolver.clear();
                self.image_rels = None;
                self.tabs.clear();
                self.active_tab = 0;
                self.show_graph = false;
                let first = v
                    .list_md_rel_paths()
                    .into_iter()
                    .find(|r| r.eq_ignore_ascii_case("Home.md"))
                    .or_else(|| v.list_md_rel_paths().into_iter().next());
                self.folder_icons = file_tree::load_folder_icons(&v);
                self.recent_notes = load_recent_notes(&v);
                self.vault = Some(v);
                self.persist.push_recent(&path);
                if let Some(rel) = first {
                    self.open_note(&rel, false);
                }
                ctx.request_repaint();
            }
            Err(e) => self.status = format!("failed to open vault: {e}"),
        }
    }

    fn close_vault(&mut self) {
        self.image_resolver.clear();
        self.image_rels = None;
        self.vault = None;
        self.tabs.clear();
        self.active_tab = 0;
        self.former_tab = 0;
        self.recent_notes.clear();
        self.show_graph = false;
        self.nav_back.clear();
        self.nav_forward.clear();
        self.folder_icons.clear();
        // Keep last_vault so the next launch still restores; clear only the live session.
        self.status = "vault closed — open vault · recent vaults".into();
    }

    fn persist_folder_icons(&self) {
        if let Some(vault) = &self.vault {
            file_tree::save_folder_icons(vault, &self.folder_icons);
        }
    }

    fn toggle_left_sidebar(&mut self) {
        self.persist.show_left_sidebar = !self.persist.show_left_sidebar;
        self.persist.save();
    }

    fn toggle_right_sidebar(&mut self) {
        self.persist.show_right_sidebar = !self.persist.show_right_sidebar;
        self.persist.save();
    }

    fn set_theme(&mut self, name: &str, ctx: &egui::Context) {
        if let Ok(t) = load_theme(name) {
            self.theme_name = name.to_string();
            self.theme = t;
            self.persist.theme = Some(name.to_string());
            self.persist.save();
            theme_apply::apply_theme(ctx, &self.theme, self.persist.ui_font_scale);
            self.status = format!("theme {name}");
        }
    }

    fn bump_editor_font(&mut self, delta: f32) {
        self.persist.editor_font_size =
            (self.persist.editor_font_size + delta).clamp(10.0, 36.0);
        self.persist.save();
        self.status = format!("editor font {:.0}px", self.persist.editor_font_size);
    }

    fn bump_ui_font(&mut self, delta: f32, ctx: &egui::Context) {
        self.persist.ui_font_scale =
            (self.persist.ui_font_scale + delta).clamp(0.85, 1.6);
        self.persist.save();
        theme_apply::apply_theme(ctx, &self.theme, self.persist.ui_font_scale);
        self.status = format!(
            "UI font {:.0}%",
            self.persist.ui_font_scale * 100.0
        );
    }

    fn bump_note_pad(&mut self, delta: f32) {
        self.persist.note_h_pad = (self.persist.note_h_pad + delta).clamp(0.0, 480.0);
        self.persist.save();
        self.status = format!("note padding {:.0}px", self.persist.note_h_pad);
    }

    /// Open a note or image from the sidebar / links / quick-open.
    fn open_path(&mut self, rel: &str, push_nav: bool) {
        if crate::media::is_image_rel(rel) {
            self.open_image(rel, push_nav);
        } else {
            self.open_note(rel, push_nav);
        }
    }

    fn open_image(&mut self, rel: &str, push_nav: bool) {
        self.outline_collapsed.clear();
        let Some(vault) = self.vault.as_ref() else {
            return;
        };
        let note_rel = self
            .tabs
            .get(self.active_tab)
            .map(|t| t.rel.as_str())
            .unwrap_or("");
        let images = self
            .image_rels
            .get_or_insert_with(|| vault.list_image_rel_paths());
        self.image_resolver.set_context(vault.root(), note_rel, "");
        let Some(image) = self.image_resolver.resolve(rel, images) else {
            self.status = format!("missing image {rel}");
            return;
        };
        let rel = image.rel.as_str();

        if push_nav {
            if let Some(cur) = self.tabs.get(self.active_tab) {
                self.nav_back.push(cur.rel.clone());
                self.nav_forward.clear();
            }
        }

        if let Some(i) = self.tabs.iter().position(|t| t.rel == rel) {
            self.former_tab = self.active_tab;
            self.active_tab = i;
            self.touch_recent_note(rel);
            return;
        }
        self.former_tab = self.active_tab;
        self.tabs.push(OpenTab {
            rel: rel.to_string(),
            kind: TabKind::Image,
            buffer: String::new(),
            dirty: false,
            preview_mode: true,
            scroll_to_line: None,
            scroll_to_heading: None,
            scroll_to_char: None,
            editor_text_id: egui::Id::new(("celestine-image", rel.to_string())),
        });
        self.active_tab = self.tabs.len() - 1;
        self.show_graph = false;
        self.focus = FocusPane::Editor;
        self.status = format!("image {rel}");
        self.touch_recent_note(rel);
    }

    fn open_note(&mut self, rel: &str, push_nav: bool) {
        self.outline_collapsed.clear();
        let Some(vault) = self.vault.as_mut() else {
            return;
        };
        // Create if missing (notes only — never invent image files).
        if vault.index.get_by_rel(rel).is_none() {
            if let Err(e) = vault.create_note(rel) {
                self.status = format!("create failed: {e}");
                return;
            }
            self.image_resolver.clear();
            self.image_rels = None;
        }
        let note_rel = if rel.to_lowercase().ends_with(".md") {
            rel.to_string()
        } else {
            format!("{rel}.md")
        };
        let canonical_rel = match vault.abs_path(&note_rel).and_then(|path| vault.rel_of(&path)) {
            Ok(rel) => rel,
            Err(e) => {
                self.status = format!("open failed: {e}");
                return;
            }
        };
        let rel = canonical_rel.as_str();
        let content = vault
            .index
            .get_by_rel(rel)
            .map(|n| n.content.clone())
            .or_else(|| vault.read_note_content(rel).ok())
            .unwrap_or_default();
        // Resolve :shortcode: → Unicode so the editor paints real emoji glyphs.
        let content = crate::emoji_util::expand_shortcodes(&content);

        if push_nav {
            if let Some(cur) = self.tabs.get(self.active_tab) {
                self.nav_back.push(cur.rel.clone());
                self.nav_forward.clear();
            }
        }

        if let Some(i) = self.tabs.iter().position(|t| t.rel == rel) {
            self.former_tab = self.active_tab;
            self.active_tab = i;
            self.touch_recent_note(rel);
            return;
        }
        self.former_tab = self.active_tab;
        self.tabs.push(OpenTab {
            rel: rel.to_string(),
            kind: TabKind::Note,
            buffer: content,
            dirty: false,
            preview_mode: false,
            scroll_to_line: None,
            scroll_to_heading: None,
            scroll_to_char: None,
            editor_text_id: egui::Id::new(("celestine-editor", rel.to_string())),
        });
        self.active_tab = self.tabs.len() - 1;
        self.show_graph = false;
        self.focus = FocusPane::Editor;
        self.touch_recent_note(rel);
    }

    fn touch_recent_note(&mut self, rel: &str) {
        let rel = rel.replace('\\', "/");
        if rel.is_empty() {
            return;
        }
        self.recent_notes.retain(|r| r != &rel);
        self.recent_notes.insert(0, rel);
        self.recent_notes.truncate(48);
        if let Some(vault) = &self.vault {
            save_recent_notes(vault, &self.recent_notes);
        }
    }

    fn save_active(&mut self) {
        let Some(vault) = self.vault.as_mut() else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        if tab.kind == TabKind::Image {
            self.status = "images are read-only".into();
            return;
        }
        match vault.write_note(&tab.rel, &tab.buffer) {
            Ok(()) => {
                self.image_resolver.clear();
                self.image_rels = None;
                tab.dirty = false;
                self.status = format!("saved {}", tab.rel);
            }
            Err(e) => self.status = format!("save error: {e}"),
        }
    }

    fn close_active_tab(&mut self) {
        self.close_tab(self.active_tab);
    }

    /// Close tab `i`, keeping `active_tab` / `former_tab` on the same notes.
    fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        self.tabs.remove(i);
        let len = self.tabs.len();
        let shift = |t: usize| -> usize {
            if len == 0 {
                0
            } else if t > i {
                t - 1
            } else {
                t.min(len - 1)
            }
        };
        self.active_tab = shift(self.active_tab);
        self.former_tab = shift(self.former_tab);
    }

    /// Act on a link clicked in the preview.
    fn follow_link(&mut self, link: preview::LinkTarget, ctx: &egui::Context) {
        use preview::LinkTarget;
        match link {
            LinkTarget::Image(rel) => self.open_path(&rel, true),
            LinkTarget::Note { rel, heading } => {
                let exists = self
                    .vault
                    .as_ref()
                    .and_then(|v| v.index.get_by_rel(&rel))
                    .is_some();
                if !exists {
                    self.status = format!("creating {rel}");
                }
                self.open_note(&rel, true);
                if let Some(h) = heading {
                    if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                        tab.scroll_to_heading = Some(h);
                    }
                }
            }
            LinkTarget::Heading(h) => {
                if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                    tab.scroll_to_heading = Some(h);
                }
            }
            LinkTarget::External(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
        }
    }

    fn active_note_id(&self) -> Option<NoteId> {
        let tab = self.tabs.get(self.active_tab)?;
        let vault = self.vault.as_ref()?;
        vault.index.get_by_rel(&tab.rel).map(|n| n.id)
    }

    fn handle_command(&mut self, cmd: Command, ctx: &egui::Context) {
        match cmd {
            Command::QuickOpen => {
                if self.vault.is_some() {
                    self.open_overlay(Overlay::QuickOpen);
                } else {
                    self.open_overlay(Overlay::RecentVaults);
                }
            }
            Command::CommandPalette => self.open_overlay(Overlay::CommandPalette),
            Command::VaultSearch => {
                if self.vault.is_some() {
                    self.open_overlay(Overlay::VaultSearch);
                }
            }
            Command::Shortcuts => self.open_overlay(Overlay::Shortcuts),
            Command::Settings => self.open_overlay(Overlay::Settings),
            Command::Find => self.open_overlay(Overlay::Find),
            Command::ThemePicker => self.open_overlay(Overlay::ThemePicker),
            Command::OpenVault => self.pick_vault_dialog(&ctx),
            Command::RecentVaults => self.open_overlay(Overlay::RecentVaults),
            Command::CloseVault => self.close_vault(),
            Command::FontBigger => self.bump_editor_font(1.5),
            Command::FontSmaller => self.bump_editor_font(-1.5),
            Command::FontReset => {
                self.persist.editor_font_size = 15.4;
                self.persist.save();
                self.status = "editor font reset".into();
            }
            Command::Graph => {
                if self.vault.is_some() {
                    self.show_graph = !self.show_graph;
                    if self.show_graph {
                        self.graph.restart_layout();
                    }
                    self.focus = FocusPane::Graph;
                }
            }
            Command::CloseTab => self.close_active_tab(),
            Command::NewNote => {
                if self.vault.is_some() {
                    let folder = self.tree_create_parent();
                    let name = format!("Untitled {}.md", chrono_lite_stamp());
                    let rel = if folder.is_empty() {
                        name
                    } else {
                        format!("{folder}/{name}")
                    };
                    self.expand_ancestors(&rel);
                    self.open_note(&rel, true);
                    self.focus = FocusPane::Editor;
                }
            }
            Command::Save => self.save_active(),
            Command::TogglePreview => self.toggle_preview_mode(),
            Command::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Command::Escape => {
                if self.tree_rename.is_some() {
                    self.tree_rename = None;
                } else if self.overlay != Overlay::None {
                    self.overlay = Overlay::None;
                } else if self.focus == FocusPane::Files {
                    self.focus_editor();
                } else if self.show_graph {
                    self.show_graph = false;
                }
            }
            Command::NextTab => {
                if !self.tabs.is_empty() {
                    self.former_tab = self.active_tab;
                    self.active_tab = (self.active_tab + 1) % self.tabs.len();
                }
            }
            Command::PrevTab => {
                if !self.tabs.is_empty() {
                    self.former_tab = self.active_tab;
                    self.active_tab =
                        (self.active_tab + self.tabs.len() - 1) % self.tabs.len();
                }
            }
            Command::FormerTab => {
                let f = self.former_tab.min(self.tabs.len().saturating_sub(1));
                self.former_tab = self.active_tab;
                self.active_tab = f;
            }
            Command::FocusFiles => {
                // Toggle: file tree ↔ note editor (caret follows).
                if self.focus == FocusPane::Files {
                    self.focus_editor();
                } else {
                    self.persist.show_left_sidebar = true;
                    self.focus = FocusPane::Files;
                    self.editor_focused = false;
                    self.editor_focus_pending = false;
                    self.show_graph = false;
                    // Cursor filled on next paint via ensure_tree_cursor.
                }
            }
            Command::FocusOutline => self.focus = FocusPane::Outline,
            Command::NextPane => self.cycle_pane(1),
            Command::PrevPane => self.cycle_pane(-1),
            Command::Tab(n) => {
                let i = (n as usize).saturating_sub(1);
                if i < self.tabs.len() {
                    self.former_tab = self.active_tab;
                    self.active_tab = i;
                }
            }
            Command::Rename => self.begin_rename_from_command(),
            Command::NewFolder => self.create_folder_here(),
            Command::DeleteItem => self.delete_tree_selection(),
            Command::ToggleLeftSidebar => self.toggle_left_sidebar(),
            Command::ToggleRightSidebar => self.toggle_right_sidebar(),
            Command::NavBack => self.nav_go_back(),
            Command::NavForward => self.nav_go_forward(),
            Command::RevealInTree => self.reveal_active_in_tree(),
        }
    }

    fn cycle_pane(&mut self, dir: i32) {
        let panes = [
            FocusPane::Files,
            FocusPane::Editor,
            FocusPane::Outline,
            FocusPane::Graph,
        ];
        let cur = panes.iter().position(|p| *p == self.focus).unwrap_or(1) as i32;
        let next = (cur + dir).rem_euclid(panes.len() as i32) as usize;
        if panes[next] == FocusPane::Editor {
            self.focus_editor();
        } else {
            self.focus = panes[next];
            self.editor_focus_pending = false;
            if self.focus == FocusPane::Graph {
                self.show_graph = true;
            }
        }
    }

    /// Hand keyboard focus (and the text caret) back to the note editor.
    fn focus_editor(&mut self) {
        self.focus = FocusPane::Editor;
        self.editor_focus_pending = true;
    }

    fn toggle_preview_mode(&mut self) {
        let Some(t) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        if t.kind == TabKind::Image {
            self.status = "image viewer (read-only)".into();
            return;
        }
        t.preview_mode = !t.preview_mode;
        let preview = t.preview_mode;
        let chord = self.keymap.display_chord(Command::TogglePreview);
        if preview {
            self.editor_focus_pending = false;
            self.status = format!("preview · {chord} to edit");
        } else {
            self.focus_editor();
            self.status = format!("edit · {chord} to preview");
        }
    }

    fn open_overlay(&mut self, o: Overlay) {
        self.overlay = o;
        self.overlay_query.clear();
        self.overlay_selected = 0;
        self.overlay_anim = 0.0;
        self.editor_focused = false;
        self.overlay_focus_pending = true;
    }

    fn pick_vault_dialog(&mut self, ctx: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
            self.open_vault(path, ctx);
        }
    }

    fn expand_ancestors(&mut self, rel: &str) {
        if let Some(parent) = Path::new(rel).parent() {
            let mut acc = String::new();
            for part in parent.components() {
                let p = part.as_os_str().to_string_lossy();
                if p == "." || p.is_empty() {
                    continue;
                }
                if !acc.is_empty() {
                    acc.push('/');
                }
                acc.push_str(&p);
                self.tree_expanded.insert(acc.clone());
            }
        }
    }

    fn ensure_tree_cursor(&mut self, tree: &file_tree::DirNode, active: Option<&str>) {
        let entries = file_tree::visible_entries(tree, &self.tree_expanded);
        if entries.is_empty() {
            self.tree_cursor = None;
            return;
        }
        let still_visible = self
            .tree_cursor
            .as_ref()
            .is_some_and(|c| entries.iter().any(|e| e == c));
        if !still_visible {
            self.tree_cursor = file_tree::default_cursor(tree, &self.tree_expanded, active);
        }
    }

    /// Arrow / Enter navigation while the file tree has focus.
    /// Returns a note path when Enter opens a file.
    fn handle_tree_keys(
        &mut self,
        ctx: &egui::Context,
        tree: &file_tree::DirNode,
    ) -> Option<String> {
        use egui::{Key, Modifiers};

        // Inline rename owns Enter/Esc/arrows.
        if self.tree_rename.is_some() {
            return None;
        }

        let entries = file_tree::visible_entries(tree, &self.tree_expanded);
        if entries.is_empty() {
            return None;
        }
        let active = self.tabs.get(self.active_tab).map(|t| t.rel.clone());
        self.ensure_tree_cursor(tree, active.as_deref());
        let Some(cursor) = self.tree_cursor.clone() else {
            return None;
        };
        let idx = entries.iter().position(|e| e == &cursor).unwrap_or(0);

        let up = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp));
        let down = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown));
        let left = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowLeft));
        let right = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowRight));
        let enter = ctx.input_mut(|i| {
            i.consume_key(Modifiers::NONE, Key::Enter)
                || i.consume_key(Modifiers::NONE, Key::Space)
        });

        if up {
            let next = idx.saturating_sub(1);
            self.tree_cursor = Some(entries[next].clone());
            return None;
        }
        if down {
            let next = (idx + 1).min(entries.len() - 1);
            self.tree_cursor = Some(entries[next].clone());
            return None;
        }
        if left {
            match &cursor {
                file_tree::TreeNavItem::Dir(path) if self.tree_expanded.contains(path) => {
                    self.tree_expanded.remove(path);
                }
                file_tree::TreeNavItem::Dir(path) | file_tree::TreeNavItem::File(path) => {
                    if let Some(parent) = Path::new(path).parent() {
                        let p = parent.to_string_lossy().replace('\\', "/");
                        if !p.is_empty() && p != "." {
                            if let Some(hit) = entries
                                .iter()
                                .find(|e| matches!(e, file_tree::TreeNavItem::Dir(d) if d == &p))
                            {
                                self.tree_cursor = Some(hit.clone());
                            }
                        }
                    }
                }
            }
            return None;
        }
        if right {
            if let file_tree::TreeNavItem::Dir(path) = &cursor {
                if !self.tree_expanded.contains(path) {
                    self.tree_expanded.insert(path.clone());
                } else {
                    // Move into first child if any became visible.
                    let refreshed = file_tree::visible_entries(tree, &self.tree_expanded);
                    if let Some(i) = refreshed.iter().position(|e| e == &cursor) {
                        if i + 1 < refreshed.len() {
                            self.tree_cursor = Some(refreshed[i + 1].clone());
                        }
                    }
                }
            }
            return None;
        }
        if enter {
            match &cursor {
                file_tree::TreeNavItem::Dir(path) => {
                    if self.tree_expanded.contains(path) {
                        self.tree_expanded.remove(path);
                    } else {
                        self.tree_expanded.insert(path.clone());
                    }
                    None
                }
                file_tree::TreeNavItem::File(rel) => Some(rel.clone()),
            }
        } else {
            None
        }
    }

    fn tree_create_parent(&self) -> String {
        match &self.tree_cursor {
            Some(file_tree::TreeNavItem::Dir(p)) => p.clone(),
            Some(file_tree::TreeNavItem::File(rel)) => Path::new(rel)
                .parent()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .filter(|p| !p.is_empty() && p != ".")
                .unwrap_or_default(),
            None => String::new(),
        }
    }

    fn start_tree_rename(&mut self, item: file_tree::TreeNavItem) {
        let draft = match &item {
            file_tree::TreeNavItem::Dir(p) => Path::new(p)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| p.clone()),
            file_tree::TreeNavItem::File(rel) => Path::new(rel)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| rel.clone()),
        };
        self.tree_cursor = Some(item.clone());
        self.focus = FocusPane::Files;
        self.editor_focused = false;
        self.tree_rename = Some(file_tree::RenameEdit {
            item,
            draft,
            request_focus: true,
        });
    }

    fn begin_rename_from_command(&mut self) {
        if self.vault.is_none() {
            return;
        }
        if let Some(item) = self.tree_cursor.clone() {
            if self.focus == FocusPane::Files {
                self.start_tree_rename(item);
                return;
            }
        }
        // Rename the active note when the editor has focus.
        if let Some(rel) = self.tabs.get(self.active_tab).map(|t| t.rel.clone()) {
            self.persist.show_left_sidebar = true;
            self.expand_ancestors(&rel);
            self.start_tree_rename(file_tree::TreeNavItem::File(rel));
        }
    }

    fn commit_tree_rename(&mut self, draft: String) {
        let Some(edit) = self.tree_rename.take() else {
            return;
        };
        let new_name = draft.trim().replace(['/', '\\'], "");
        if new_name.is_empty() {
            self.status = "rename cancelled (empty name)".into();
            return;
        }
        let old_rel = match &edit.item {
            file_tree::TreeNavItem::Dir(p) | file_tree::TreeNavItem::File(p) => p.clone(),
        };
        let new_name = match &edit.item {
            file_tree::TreeNavItem::File(_) => keep_file_extension(&old_rel, new_name),
            file_tree::TreeNavItem::Dir(_) => new_name,
        };
        let parent = Path::new(&old_rel)
            .parent()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .filter(|p| !p.is_empty() && p != ".")
            .unwrap_or_default();
        let new_rel = if parent.is_empty() {
            new_name
        } else {
            format!("{parent}/{new_name}")
        };
        if new_rel == old_rel {
            return;
        }
        let Some(vault) = self.vault.as_mut() else {
            return;
        };
        match vault.rename_entry(&old_rel, &new_rel) {
            Ok(()) => {
                self.image_resolver.clear();
                self.image_rels = None;
                self.remap_after_rename(&old_rel, &new_rel, matches!(edit.item, file_tree::TreeNavItem::Dir(_)));
                self.status = format!("renamed → {new_rel}");
            }
            Err(e) => {
                self.status = format!("rename failed: {e}");
                self.tree_rename = Some(edit);
            }
        }
    }

    fn remap_after_rename(&mut self, old_rel: &str, new_rel: &str, is_dir: bool) {
        let remap = |path: &str| -> String {
            if path == old_rel {
                new_rel.to_string()
            } else if is_dir {
                let prefix = format!("{old_rel}/");
                if let Some(rest) = path.strip_prefix(&prefix) {
                    format!("{new_rel}/{rest}")
                } else {
                    path.to_string()
                }
            } else {
                path.to_string()
            }
        };

        for tab in &mut self.tabs {
            tab.rel = remap(&tab.rel);
        }
        for p in &mut self.nav_back {
            *p = remap(p);
        }
        for p in &mut self.nav_forward {
            *p = remap(p);
        }

        let expanded: Vec<String> = self.tree_expanded.drain().collect();
        for p in expanded {
            self.tree_expanded.insert(remap(&p));
        }

        let icons: Vec<(String, String)> = self.folder_icons.drain().collect();
        for (k, v) in icons {
            self.folder_icons.insert(remap(&k), v);
        }
        self.persist_folder_icons();

        for r in &mut self.recent_notes {
            *r = remap(r);
        }
        if let Some(vault) = &self.vault {
            save_recent_notes(vault, &self.recent_notes);
        }

        self.tree_cursor = Some(if is_dir {
            file_tree::TreeNavItem::Dir(new_rel.to_string())
        } else {
            file_tree::TreeNavItem::File(new_rel.to_string())
        });
        if is_dir {
            self.tree_expanded.insert(new_rel.to_string());
        }
    }

    fn create_folder_here(&mut self) {
        let parent = self.tree_create_parent();
        let mut name = "New folder".to_string();
        let mut n = 2;
        let folder_rel = loop {
            let candidate = if parent.is_empty() {
                name.clone()
            } else {
                format!("{parent}/{name}")
            };
            let exists = self.vault.as_ref().is_some_and(|v| {
                v.abs_path(&candidate).is_ok_and(|path| path.exists())
            });
            if !exists {
                break candidate;
            }
            name = format!("New folder {n}");
            n += 1;
        };
        let Some(vault) = self.vault.as_mut() else {
            return;
        };
        match vault.create_folder(&folder_rel) {
            Ok(()) => {
                self.image_resolver.clear();
                self.image_rels = None;
                self.expand_ancestors(&format!("{folder_rel}/_"));
                self.tree_expanded.insert(folder_rel.clone());
                self.persist.show_left_sidebar = true;
                self.focus = FocusPane::Files;
                self.start_tree_rename(file_tree::TreeNavItem::Dir(folder_rel));
                self.status = "folder created".into();
            }
            Err(e) => self.status = format!("new folder failed: {e}"),
        }
    }

    fn delete_tree_selection(&mut self) {
        if self.focus != FocusPane::Files {
            return;
        }
        let Some(item) = self.tree_cursor.clone() else {
            return;
        };
        let rel = match &item {
            file_tree::TreeNavItem::Dir(p) | file_tree::TreeNavItem::File(p) => p.clone(),
        };
        let Some(vault) = self.vault.as_mut() else {
            return;
        };
        match vault.delete_entry(&rel) {
            Ok(()) => {
                self.image_resolver.clear();
                self.image_rels = None;
                // Close tabs under deleted path.
                let prefix = format!("{rel}/");
                self.tabs.retain(|t| t.rel != rel && !t.rel.starts_with(&prefix));
                if self.active_tab >= self.tabs.len() {
                    self.active_tab = self.tabs.len().saturating_sub(1);
                }
                self.former_tab = self.former_tab.min(self.tabs.len().saturating_sub(1));
                self.forget_history(&rel);
                self.tree_expanded.remove(&rel);
                self.folder_icons.remove(&rel);
                self.persist_folder_icons();
                self.tree_cursor = None;
                self.status = format!("deleted {rel}");
            }
            Err(e) => self.status = format!("delete failed: {e}"),
        }
    }

    /// Drop a deleted note/folder from navigation history and recents, so
    /// Back/Forward never re-opens (and thereby recreates) it.
    fn forget_history(&mut self, rel: &str) {
        let prefix = format!("{rel}/");
        let gone = |p: &String| p == rel || p.starts_with(&prefix);
        for list in [&mut self.nav_back, &mut self.nav_forward] {
            list.retain(|p| !gone(p));
            list.dedup();
        }
        self.recent_notes.retain(|p| !gone(p));
        if let Some(vault) = &self.vault {
            save_recent_notes(vault, &self.recent_notes);
        }
    }

    fn nav_go_back(&mut self) {
        let Some(prev) = self.nav_back.pop() else {
            self.status = "no back history".into();
            return;
        };
        if let Some(cur) = self.tabs.get(self.active_tab).map(|t| t.rel.clone()) {
            self.nav_forward.push(cur);
        }
        self.open_path(&prev, false);
    }

    fn nav_go_forward(&mut self) {
        let Some(next) = self.nav_forward.pop() else {
            self.status = "no forward history".into();
            return;
        };
        if let Some(cur) = self.tabs.get(self.active_tab).map(|t| t.rel.clone()) {
            self.nav_back.push(cur);
        }
        self.open_path(&next, false);
    }

    fn reveal_active_in_tree(&mut self) {
        let Some(rel) = self.tabs.get(self.active_tab).map(|t| t.rel.clone()) else {
            return;
        };
        self.persist.show_left_sidebar = true;
        self.expand_ancestors(&rel);
        self.tree_cursor = Some(file_tree::TreeNavItem::File(rel));
        self.focus = FocusPane::Files;
        self.editor_focused = false;
        self.status = "revealed in files".into();
    }
}

/// A tree rename that drops the extension would turn `Note.md` into `Note`,
/// which the index no longer sees. Notes always stay `.md`; other files keep
/// their old extension when the draft has none.
fn keep_file_extension(old_rel: &str, draft: String) -> String {
    let Some(old_ext) = Path::new(old_rel).extension().and_then(|e| e.to_str()) else {
        return draft;
    };
    if old_ext.eq_ignore_ascii_case("md") {
        if draft.to_lowercase().ends_with(".md") {
            draft
        } else {
            format!("{draft}.md")
        }
    } else if Path::new(&draft).extension().is_some() {
        draft
    } else {
        format!("{draft}.{old_ext}")
    }
}

fn chrono_lite_stamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() % 100_000)
        .unwrap_or(0);
    t.to_string()
}

impl eframe::App for CelestineApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Consume background work even when eframe suppresses hidden-window UI.
        if let Some(vault) = self.vault.as_mut() {
            let was_degraded = vault.watcher_error().is_some();
            let changed = vault.poll_fs();
            if changed || (!was_degraded && vault.watcher_error().is_some()) {
                if changed {
                    self.image_resolver.clear();
                    self.image_rels = None;
                }
                self.status = if let Some(error) = vault.watcher_error() {
                    format!("index refreshed · filesystem polling (watcher: {error})")
                } else {
                    "index refreshed".into()
                };
            }
            if let Some(delay) = vault.next_fs_poll() {
                ctx.request_repaint_after(delay);
            }
        }

        if self.update_checker.poll() {
            ctx.request_repaint();
        } else if self.update_checker.pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(400));
        }
        // Respect a previously dismissed feature line.
        if let Some(info) = self.update_checker.available() {
            let line = format!("{}.{}", info.version.major, info.version.minor);
            if self.persist.dismissed_update_line.as_deref() == Some(line.as_str()) {
                self.update_checker.clear();
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let dt = self.last_frame.elapsed().as_secs_f32();
        self.last_frame = Instant::now();
        if let Some(flash) = &self.line_flash {
            if flash.started.elapsed().as_secs_f32() >= LINE_FLASH_SECS {
                self.line_flash = None;
            } else {
                ctx.request_repaint();
            }
        }
        // Overlay fade-in
        if self.overlay != Overlay::None {
            self.overlay_anim = (self.overlay_anim + dt * 8.0).min(1.0);
        } else {
            self.overlay_anim = 0.0;
        }

        if let Some(cmd) = self.keymap.consume(&ctx) {
            self.handle_command(cmd, &ctx);
        }

        let palette = theme_apply::ThemePalette::from_tokens(&self.theme);
        let accent = palette.accent;
        let muted = palette.muted;
        let text_c = palette.text;

        // Top bar — flea chrome: path/brand left, naked icons right
        egui::Panel::top("top")
            .exact_size(36.0)
            .frame(theme_apply::panel_frame(&ctx))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new("celestine").strong().color(accent));
                    if let Some(v) = &self.vault {
                        ui.label(
                            RichText::new(format!("  {}", v.root().display()))
                                .color(muted)
                                .size(12.5),
                        );
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        if naked_icon(ui, "sliders", false, accent, muted)
                            .on_hover_text("Settings")
                            .clicked()
                        {
                            self.open_overlay(Overlay::Settings);
                        }
                        if naked_icon(
                            ui,
                            "panel-right",
                            self.persist.show_right_sidebar,
                            accent,
                            muted,
                        )
                        .on_hover_text("Toggle outline")
                        .clicked()
                        {
                            self.toggle_right_sidebar();
                        }
                        if naked_icon(
                            ui,
                            "panel-left",
                            self.persist.show_left_sidebar,
                            accent,
                            muted,
                        )
                        .on_hover_text("Toggle files")
                        .clicked()
                        {
                            self.toggle_left_sidebar();
                        }
                        if naked_icon(ui, "search", false, accent, muted)
                            .on_hover_text(format!(
                                "Quick open {}",
                                self.keymap.display_chord(Command::QuickOpen)
                            ))
                            .clicked()
                        {
                            self.open_overlay(Overlay::QuickOpen);
                        }
                        if naked_button(
                            ui,
                            RichText::new(self.theme_name.as_str()).size(12.0).color(muted),
                        )
                        .on_hover_text("Themes")
                        .clicked()
                        {
                            self.open_overlay(Overlay::ThemePicker);
                        }
                        if naked_button(
                            ui,
                            RichText::new("vault").size(12.0).color(muted),
                        )
                        .on_hover_text("Open vault")
                        .clicked()
                        {
                            self.pick_vault_dialog(&ctx);
                        }
                    });
                });
            });

        // Feature-release notice (GitHub). Patch-only bumps never appear here.
        if !self.update_banner_hidden {
            if let Some(info) = self.update_checker.available().cloned() {
                let mut open_release = false;
                let mut dismiss = false;
                egui::Panel::top("update_banner")
                    .exact_size(28.0)
                    .frame(theme_apply::panel_frame(&ctx))
                    .show(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "CelestineMD {} available",
                                    info.version.display()
                                ))
                                .size(12.0)
                                .color(accent),
                            );
                            ui.label(
                                RichText::new("· GitHub release")
                                    .size(11.5)
                                    .color(muted),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if naked_button(
                                        ui,
                                        RichText::new("×").size(13.0).color(muted),
                                    )
                                    .on_hover_text("Dismiss")
                                    .clicked()
                                    {
                                        dismiss = true;
                                    }
                                    if naked_button(
                                        ui,
                                        RichText::new("view")
                                            .size(12.0)
                                            .color(accent),
                                    )
                                    .on_hover_text("Open release page")
                                    .clicked()
                                    {
                                        open_release = true;
                                    }
                                },
                            );
                        });
                    });
                if open_release {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(&info.html_url));
                    let line = format!("{}.{}", info.version.major, info.version.minor);
                    self.persist.dismissed_update_line = Some(line);
                    self.persist.save();
                    self.update_banner_hidden = true;
                    self.update_checker.clear();
                } else if dismiss {
                    let line = format!("{}.{}", info.version.major, info.version.minor);
                    self.persist.dismissed_update_line = Some(line);
                    self.persist.save();
                    self.update_banner_hidden = true;
                    self.update_checker.clear();
                }
            }
        }

        // Tabs strip
        if !self.tabs.is_empty() {
            egui::Panel::top("tabs")
                .exact_size(30.0)
                .frame(theme_apply::panel_frame(&ctx))
                .show(ui, |ui| {
                    ui.horizontal_centered(|ui| {
                        let mut switch_to = None;
                        let mut close_i = None;
                        for (i, tab) in self.tabs.iter().enumerate() {
                            let dirty = if tab.dirty { "·" } else { "" };
                            let name = Path::new(&tab.rel)
                                .file_stem()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_else(|| tab.rel.clone());
                            let selected = i == self.active_tab;
                            let label = RichText::new(format!("{name}{dirty}"))
                                .size(12.5)
                                .color(if selected { text_c } else { muted });
                            let resp = naked_button(ui, label);
                            if selected {
                                let r = resp.rect;
                                ui.painter().hline(
                                    r.left()..=r.right(),
                                    r.bottom() - 1.0,
                                    Stroke::new(1.5_f32, accent),
                                );
                            }
                            if resp.clicked() {
                                switch_to = Some(i);
                            }
                            if naked_button(ui, RichText::new("×").size(12.0).color(muted))
                                .on_hover_text(self.keymap.display_chord(Command::CloseTab))
                                .clicked()
                            {
                                close_i = Some(i);
                            }
                            ui.add_space(6.0);
                        }
                        if let Some(i) = switch_to {
                            self.former_tab = self.active_tab;
                            self.active_tab = i;
                        }
                        if let Some(i) = close_i {
                            self.close_tab(i);
                        }
                    });
                });
        }

        // Footer cheat strip
        egui::Panel::bottom("foot").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&self.status).monospace().color(muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let strip = match self.focus {
                        FocusPane::Files => format!(
                            "files · ↑↓ ←→ · Enter open · {} editor",
                            self.keymap.display_chord(Command::FocusFiles)
                        ),
                        FocusPane::Editor => format!(
                            "{} tabs · {} files ↔ · save {} · ?",
                            self.keymap.display_chord(Command::NextTab),
                            self.keymap.display_chord(Command::FocusFiles),
                            self.keymap.display_chord(Command::Save)
                        ),
                        FocusPane::Outline => "outline · click heading".into(),
                        FocusPane::Graph => format!(
                            "graph {} · drag pan · scroll zoom",
                            self.keymap.display_chord(Command::Graph)
                        ),
                    };
                    ui.label(RichText::new(strip).monospace().color(accent));
                });
            });
        });

        // Left: files (collapsible like Obsidian)
        if self.persist.show_left_sidebar {
            egui::Panel::left("files")
                .default_size(220.0)
                .frame(theme_apply::panel_frame(&ctx))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("FILES")
                                .size(11.0)
                                .color(muted)
                                .strong(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if naked_icon(ui, "chevron-left", false, accent, muted)
                                .on_hover_text("Hide files")
                                .clicked()
                            {
                                self.toggle_left_sidebar();
                            }
                        });
                    });
                    ui.label(
                        RichText::new("click expand · dbl-click rename · F2")
                            .size(10.0)
                            .color(muted.gamma_multiply(0.75)),
                    );
                    ui.add_space(4.0);
                    if self.vault.is_some() {
                        let mut paths = self
                            .vault
                            .as_ref()
                            .map(|v| v.list_sidebar_rel_paths())
                            .unwrap_or_default();
                        paths.sort();
                        let tree = file_tree::build_tree(&paths);
                        let active_owned = self.tabs.get(self.active_tab).map(|t| t.rel.clone());
                        let active = active_owned.as_deref();
                        let tree_focused = self.focus == FocusPane::Files;
                        if tree_focused && self.overlay == Overlay::None {
                            if let Some(rel) = self.handle_tree_keys(&ctx, &tree) {
                                self.expand_ancestors(&rel);
                                self.open_path(&rel, true);
                                self.focus_editor();
                            }
                        }
                        self.ensure_tree_cursor(&tree, active);
                        let cursor = self.tree_cursor.clone();
                        let outcome = ScrollArea::vertical()
                            .show(ui, |ui| {
                                file_tree::show_tree(
                                    ui,
                                    &tree,
                                    &mut self.tree_expanded,
                                    &mut self.folder_icons,
                                    &palette,
                                    muted,
                                    active,
                                    cursor.as_ref(),
                                    tree_focused,
                                    &mut self.tree_rename,
                                )
                            })
                            .inner;
                        if outcome.icons_changed {
                            self.persist_folder_icons();
                            self.status = "folder icon saved".into();
                        }
                        if let Some(item) = outcome.focus_item {
                            self.tree_cursor = Some(item);
                            self.focus = FocusPane::Files;
                            self.editor_focused = false;
                        }
                        if let Some(item) = outcome.start_rename {
                            self.start_tree_rename(item);
                        }
                        if outcome.rename_cancel {
                            self.tree_rename = None;
                        }
                        if let Some(draft) = outcome.rename_commit {
                            self.commit_tree_rename(draft);
                        }
                        if let Some(rel) = outcome.open_rel {
                            self.expand_ancestors(&rel);
                            self.open_path(&rel, true);
                            self.focus_editor();
                        }
                    } else {
                        ui.label(RichText::new("(no vault)").color(muted));
                        if naked_button(ui, RichText::new("open folder…").color(accent)).clicked()
                        {
                            self.pick_vault_dialog(&ctx);
                        }
                    }
                });
        }

        // Right: outline + backlinks
        if self.persist.show_right_sidebar {
            egui::Panel::right("meta")
                .default_size(220.0)
                .min_size(160.0)
                .max_size(360.0)
                .frame(theme_apply::panel_frame(&ctx))
                .show(ui, |ui| {
                    ui.set_max_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("OUTLINE")
                                .size(11.0)
                                .color(muted)
                                .strong(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if naked_icon(ui, "chevron-right", false, accent, muted)
                                .on_hover_text("Hide outline")
                                .clicked()
                            {
                                self.toggle_right_sidebar();
                            }
                        });
                    });
                    ui.add_space(4.0);
                    let mut jump: Option<(usize, String)> = None;
                    let mut open_backlink: Option<String> = None;
                    if let (Some(vault), Some(tab)) =
                        (self.vault.as_ref(), self.tabs.get(self.active_tab))
                    {
                        if tab.kind == TabKind::Image {
                            ui.label(
                                RichText::new("(image — no outline)")
                                    .color(muted),
                            );
                        } else {
                        let parsed = extract_note_data(&tab.buffer);
                        let tree = outline_tree::build(&parsed.headings);
                        ScrollArea::vertical()
                            .max_height(ui.available_height() * 0.55)
                            .show(ui, |ui| {
                                ui.set_max_width(ui.available_width());
                                if tree.is_empty() {
                                    ui.label(RichText::new("(no headings)").color(muted));
                                } else {
                                    jump = outline_tree::show(
                                        ui,
                                        &tree,
                                        &mut self.outline_collapsed,
                                        &palette.heading,
                                        muted,
                                        text_c,
                                    );
                                }
                            });
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new("BACKLINKS").size(11.0).strong().color(muted),
                        );
                        if let Some(meta) = vault.index.get_by_rel(&tab.rel) {
                            let backs = vault.graph.backlinks(&vault.index, meta.id);
                            if backs.is_empty() {
                                ui.label(RichText::new("(none)").color(muted));
                            } else {
                                for id in backs {
                                    if let Some(n) = vault.index.get(id) {
                                        let w = ui.available_width().max(40.0);
                                        ui.set_max_width(w);
                                        let label = RichText::new(&n.rel).color(text_c);
                                        let resp = ui.add(
                                            egui::Label::new(label)
                                                .wrap()
                                                .sense(egui::Sense::click()),
                                        );
                                        if resp.clicked() {
                                            open_backlink = Some(n.rel.clone());
                                        }
                                    }
                                }
                            }
                        }
                        } // TabKind::Note outline/backlinks
                    }
                    if let Some((line, heading)) = jump {
                        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                            tab.scroll_to_line = Some(line);
                            tab.scroll_to_heading = Some(heading);
                        }
                    }
                    if let Some(rel) = open_backlink {
                        self.open_note(&rel, true);
                    }
                });
        }

        // Center
        egui::CentralPanel::default()
            .frame(theme_apply::content_frame(&self.theme))
            .show(ui, |ui| {
            if self.show_graph {
                ui.label(
                    RichText::new(format!(
                        "── graph {} ──",
                        self.keymap.display_chord(Command::Graph)
                    ))
                    .monospace()
                    .color(accent),
                );
                if let Some(vault) = &self.vault {
                    let center = self.active_note_id();
                    if let Some(rel) =
                        self.graph
                            .show(ui, vault, center, accent, muted, text_c, &palette)
                    {
                        self.open_note(&rel, true);
                        self.show_graph = false;
                    }
                }
            } else if self.vault.is_none() {
                ui.vertical_centered(|ui| {
                    ui.add_space(48.0);
                    ui.label(RichText::new("celestine").monospace().strong().size(28.0).color(accent));
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("no vault open")
                            .monospace()
                            .color(muted),
                    );
                    ui.add_space(16.0);
                    if naked_button(
                        ui,
                        RichText::new(format!(
                            "open vault…  {}",
                            self.keymap.display_chord(Command::OpenVault)
                        ))
                        .color(accent),
                    )
                    .clicked()
                    {
                        self.pick_vault_dialog(&ctx);
                    }
                    if naked_button(
                        ui,
                        RichText::new(format!(
                            "recent vaults  {}",
                            self.keymap.display_chord(Command::RecentVaults)
                        ))
                        .color(muted),
                    )
                    .clicked()
                    {
                        self.open_overlay(Overlay::RecentVaults);
                    }
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new(format!(
                            "themes {} · font +/- {} {}",
                            self.keymap.display_chord(Command::ThemePicker),
                            self.keymap.display_chord(Command::FontBigger),
                            self.keymap.display_chord(Command::FontSmaller),
                        ))
                        .monospace()
                        .color(muted),
                    );
                });
            } else if let Some(tab_idx) = self.tabs.get(self.active_tab).map(|_| self.active_tab) {
                // While an overlay is open, keep the note visible but disable the editor
                // so it cannot steal keyboard focus from the submenu.
                if self.overlay != Overlay::None {
                    ui.disable();
                }
                let tab_kind = self.tabs[tab_idx].kind;
                let preview_mode = self.tabs[tab_idx].preview_mode;
                let rel = self.tabs[tab_idx].rel.clone();
                let md_font = self.persist.editor_font_size;

                if tab_kind == TabKind::Image {
                    self.editor_focus_pending = false;
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("── {rel} ──"))
                                .monospace()
                                .color(accent),
                        );
                        ui.label(
                            RichText::new("IMAGE")
                                .monospace()
                                .color(muted),
                        );
                    });
                    let fill_h = ui.available_height().max(80.0);
                    let fill_w = ui.available_width().max(1.0);
                    if let Some(vault) = &self.vault {
                        ScrollArea::both()
                            .id_salt("image-scroll")
                            .auto_shrink([false, false])
                            .max_width(fill_w)
                            .min_scrolled_height(fill_h)
                            .show(ui, |ui| {
                                ui.set_min_size(egui::vec2(fill_w, fill_h));
                                self.image_resolver.set_context(vault.root(), &rel, "");
                                if let Some(image) = self.image_resolver.resolve(&rel, &[]) {
                                    crate::media::show_standalone(ui, &image.abs, muted);
                                } else {
                                    ui.centered_and_justified(|ui| {
                                        ui.label(
                                            RichText::new(format!("missing image\n{rel}"))
                                                .monospace()
                                                .color(muted),
                                        );
                                    });
                                }
                            });
                    }
                } else {
                let stats = text_stats(&self.tabs[tab_idx].buffer);
                let mode_color = palette.heading[1];
                let toggle_chord = self.keymap.display_chord(Command::TogglePreview);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("── {rel} ──")).monospace().color(accent));
                    ui.label(
                        RichText::new(format!(
                            "font {:.0}px ({} {})",
                            md_font,
                            self.keymap.display_chord(Command::FontBigger),
                            self.keymap.display_chord(Command::FontSmaller),
                        ))
                        .monospace()
                        .color(muted),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // RTL: add label first so the row reads [icon · MODE · chord].
                        let mode_icon = if preview_mode { "eye" } else { "pencil" };
                        let mode_label = if preview_mode { "PREVIEW" } else { "EDIT" };
                        let tip = if preview_mode {
                            format!("Switch to edit  {toggle_chord}")
                        } else {
                            format!("Switch to preview  {toggle_chord}")
                        };
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let label = ui
                            .add(
                                egui::Button::new(
                                    RichText::new(format!("{mode_label}  {toggle_chord}"))
                                        .monospace()
                                        .size(12.5)
                                        .strong()
                                        .color(mode_color),
                                )
                                .frame(false),
                            )
                            .on_hover_text(&tip);
                        let icon = crate::icons::button(ui, mode_icon, 15.0, mode_color)
                            .on_hover_text(&tip);
                        if label.clicked() || icon.clicked() {
                            self.toggle_preview_mode();
                        }
                    });
                });

                // Pin stats to the bottom of the note pane; editor/preview fills the rest.
                let stats_h = 24.0_f32;
                egui::Panel::bottom("note-stats")
                    .exact_size(stats_h)
                    .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(0, 2)))
                    .show_inside(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "{} lines · {} words · {} symbols",
                                    stats.lines, stats.words, stats.symbols
                                ))
                                .monospace()
                                .color(muted),
                            );
                        });
                    });

                // Full-width note pane: line gutter on the far left, solid scrollbar on
                // the far right; L/R padding shrinks with the window so text still fits.
                let fill_h = ui.available_height().max(80.0);
                let fill_w = ui.available_width().max(1.0);
                let preferred_pad = self.persist.note_h_pad;
                const MIN_TEXT_W: f32 = 200.0_f32;

                if preview_mode {
                    self.editor_focus_pending = false;
                    let body = {
                        let buf = &self.tabs[tab_idx].buffer;
                        let parsed = extract_note_data(buf);
                        parsed.body
                    };
                    let scroll_heading = self.tabs[tab_idx].scroll_to_heading.clone();
                    let flash_alpha = self.line_flash_alpha();
                    let flash_needle = self
                        .line_flash
                        .as_ref()
                        .map(|f| f.needle.clone());
                    let scroll_text = self
                        .line_flash
                        .as_ref()
                        .filter(|f| f.do_scroll)
                        .map(|f| f.needle.clone());
                    let outcome = if let Some(vault) = &self.vault {
                        let image_rels = self.image_rels
                            .get_or_insert_with(|| vault.list_image_rel_paths());
                        ScrollArea::vertical()
                            .id_salt("preview-scroll")
                            .auto_shrink([false, false])
                            .max_width(fill_w)
                            .min_scrolled_height(fill_h)
                            .show(ui, |ui| {
                                ui.set_min_height(fill_h);
                                ui.add_space(NOTE_V_PAD);
                                let inner_w = ui.available_width().max(1.0);
                                let (pad, text_w) =
                                    note_side_pad(inner_w, 0.0, preferred_pad, MIN_TEXT_W);
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 0.0;
                                    ui.add_space(pad);
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(text_w, fill_h),
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            ui.set_max_width(text_w);
                                            ui.set_width(text_w);
                                            preview::show_preview(
                                                ui,
                                                &body,
                                                &rel,
                                                vault.root(),
                                                &vault.index,
                                                image_rels,
                                                accent,
                                                muted,
                                                text_c,
                                                md_font,
                                                scroll_heading.as_deref(),
                                                scroll_text.as_deref(),
                                                flash_needle.as_deref(),
                                                flash_alpha.unwrap_or(0.0),
                                                &palette.heading,
                                                &mut self.image_resolver,
                                            )
                                        },
                                    )
                                    .inner
                                })
                                .inner;
                                ui.add_space(NOTE_V_PAD);
                            })
                            .inner
                    } else {
                        preview::PreviewOutcome {
                            link: None,
                            heading_rects: vec![],
                            find_scrolled: false,
                        }
                    };
                    if scroll_heading.is_some() {
                        if let Some(tab) = self.tabs.get_mut(tab_idx) {
                            tab.scroll_to_heading = None;
                            tab.scroll_to_line = None;
                            tab.scroll_to_char = None;
                        }
                    }
                    if outcome.find_scrolled {
                        if let Some(flash) = self.line_flash.as_mut() {
                            flash.do_scroll = false;
                        }
                    }
                    if let Some(link) = outcome.link {
                        self.follow_link(link, &ctx);
                    }
                } else {
                    let font = FontId::monospace(md_font);
                    let text_id = self.tabs[tab_idx].editor_text_id;
                    let jump_char_direct = self.tabs[tab_idx].scroll_to_char.take();
                    let jump_line = self.tabs[tab_idx].scroll_to_line.take();
                    let jump_heading = self.tabs[tab_idx].scroll_to_heading.take();
                    let jump_char = jump_char_direct.or_else(|| {
                        jump_heading
                            .as_deref()
                            .and_then(|h| {
                                char_offset_for_heading(&self.tabs[tab_idx].buffer, h)
                            })
                            .or_else(|| {
                                jump_line.map(|line| {
                                    char_offset_at_line(&self.tabs[tab_idx].buffer, line)
                                })
                            })
                    });
                    let flash_alpha = self.line_flash_alpha();
                    let flash_line_0 = self.line_flash.as_ref().map(|f| f.line_0);
                    let flash_do_scroll = self
                        .line_flash
                        .as_ref()
                        .map(|f| f.do_scroll)
                        .unwrap_or(false);
                    let gutter_w = line_gutter_width(stats.lines, md_font);
                    let gutter_gap = 6.0_f32;
                    let row_h = (md_font * 1.35).max(1.0);
                    let desired_rows =
                        ((fill_h / row_h).ceil() as usize).clamp(24, 200);
                    let (changed, focused) = ScrollArea::vertical()
                        .id_salt("editor-scroll")
                        .auto_shrink([false, false])
                        .max_width(fill_w)
                        .min_scrolled_height(fill_h)
                        .show(ui, |ui| {
                            ui.set_min_height(fill_h);
                            let inner_w = ui.available_width().max(1.0);
                            let (pad, text_w) = note_side_pad(
                                inner_w,
                                gutter_w + gutter_gap,
                                preferred_pad,
                                MIN_TEXT_W,
                            );
                            ui.horizontal(|ui| {
                                // Manual gaps only — avoid item_spacing blowing past inner_w.
                                ui.spacing_mut().item_spacing.x = 0.0;
                                let headings = palette.heading;
                                let mut layouter =
                                    |ui: &egui::Ui,
                                     text: &dyn egui::TextBuffer,
                                     wrap_width: f32| {
                                        let text = text.as_str();
                                        let w = if wrap_width.is_finite()
                                            && wrap_width > 1.0
                                        {
                                            wrap_width.min(text_w)
                                        } else {
                                            text_w
                                        };
                                        md_edit::layout(
                                            ui,
                                            text,
                                            w,
                                            md_font,
                                            text_c,
                                            muted,
                                            accent,
                                            &headings,
                                        )
                                    };

                                // Far-left line gutter (scrolls with content).
                                let gutter_placeholder = ui
                                    .allocate_exact_size(
                                        egui::vec2(gutter_w, fill_h),
                                        egui::Sense::hover(),
                                    )
                                    .0;
                                ui.add_space(gutter_gap + pad);

                                let editor_live = self.focus != FocusPane::Files;
                                let te = TextEdit::multiline(
                                    &mut self.tabs[tab_idx].buffer,
                                )
                                .id(text_id)
                                .font(font)
                                .frame(egui::Frame::NONE)
                                .desired_width(text_w)
                                .desired_rows(desired_rows)
                                .interactive(editor_live)
                                .layouter(&mut layouter);
                                let mut out = te.show(ui);

                                ui.add_space(pad);

                                let need_h = fill_h.max(out.galley.size().y);
                                if let Some(char_pos) = jump_char {
                                    let id = out.response.id;
                                    out.response.request_focus();
                                    out.state.cursor.set_char_range(Some(
                                        CCursorRange::one(CCursor::new(char_pos)),
                                    ));
                                    out.state.store(ui.ctx(), id);
                                    ui.scroll_to_cursor(Some(egui::Align::Center));
                                    self.editor_focus_pending = false;
                                    if let Some(flash) = self.line_flash.as_mut() {
                                        flash.do_scroll = false;
                                    }
                                } else if flash_do_scroll {
                                    if let Some(line_0) = flash_line_0 {
                                        if let Some(rect) = source_line_rect(
                                            &out.galley,
                                            out.galley_pos,
                                            line_0,
                                            text_w,
                                        ) {
                                            ui.scroll_to_rect(
                                                rect,
                                                Some(egui::Align::Center),
                                            );
                                        }
                                    }
                                    if let Some(flash) = self.line_flash.as_mut() {
                                        flash.do_scroll = false;
                                    }
                                    if self.editor_focus_pending && editor_live {
                                        out.response.request_focus();
                                        self.editor_focus_pending = false;
                                    }
                                } else if self.editor_focus_pending && editor_live {
                                    out.response.request_focus();
                                    self.editor_focus_pending = false;
                                }
                                if !editor_live {
                                    out.response.surrender_focus();
                                }

                                if let (Some(alpha), Some(line_0)) =
                                    (flash_alpha, flash_line_0)
                                {
                                    if let Some(rect) = source_line_rect(
                                        &out.galley,
                                        out.galley_pos,
                                        line_0,
                                        text_w,
                                    ) {
                                        // Soft wash — keep text readable.
                                        let a = (alpha * 55.0).clamp(0.0, 255.0) as u8;
                                        ui.painter().rect_filled(
                                            rect.expand2(egui::vec2(4.0, 1.0)),
                                            3.0,
                                            egui::Color32::from_rgba_unmultiplied(
                                                accent.r(),
                                                accent.g(),
                                                accent.b(),
                                                a,
                                            ),
                                        );
                                    }
                                }

                                let gutter_rect = egui::Rect::from_min_size(
                                    gutter_placeholder.min,
                                    egui::vec2(gutter_w, need_h),
                                );
                                paint_line_numbers(
                                    ui,
                                    gutter_rect,
                                    &out.galley,
                                    out.galley_pos,
                                    md_font,
                                    muted,
                                );

                                (out.response.changed(), out.response.has_focus())
                            })
                            .inner
                        })
                        .inner;
                    if focused {
                        self.focus = FocusPane::Editor;
                    }
                    self.editor_focused = focused;
                    if changed {
                        let expanded =
                            crate::emoji_util::expand_shortcodes(&self.tabs[tab_idx].buffer);
                        if expanded != self.tabs[tab_idx].buffer {
                            self.tabs[tab_idx].buffer = expanded;
                        }
                        self.tabs[tab_idx].dirty = true;
                    }
                }
                } // TabKind::Note
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "no note open\n{} quick open · {} new note",
                            self.keymap.display_chord(Command::QuickOpen),
                            self.keymap.display_chord(Command::NewNote)
                        ))
                        .monospace()
                        .color(muted),
                    );
                });
            }
        });

        self.draw_overlay(&ctx, accent, muted);
        if self.overlay != Overlay::None && self.overlay_anim < 1.0 {
            ctx.request_repaint();
        }
    }
}

impl CelestineApp {
    fn draw_overlay(&mut self, ctx: &egui::Context, accent: egui::Color32, muted: egui::Color32) {
        if self.overlay == Overlay::None {
            return;
        }
        let alpha = self.overlay_anim;
        let title = match self.overlay {
            Overlay::QuickOpen => format!("quick open {}", self.keymap.display_chord(Command::QuickOpen)),
            Overlay::CommandPalette => {
                format!("commands {}", self.keymap.display_chord(Command::CommandPalette))
            }
            Overlay::VaultSearch => {
                format!("search {}", self.keymap.display_chord(Command::VaultSearch))
            }
            Overlay::Shortcuts => "shortcuts  ?".into(),
            Overlay::Settings => "settings".into(),
            Overlay::Find => format!("find {}", self.keymap.display_chord(Command::Find)),
            Overlay::ThemePicker => {
                format!("themes {}", self.keymap.display_chord(Command::ThemePicker))
            }
            Overlay::RecentVaults => {
                format!("recent {}", self.keymap.display_chord(Command::RecentVaults))
            }
            Overlay::None => return,
        };

        // Dim stays in Middle; the modal is Foreground so it can never be covered
        // when the full-screen dim is clicked (same-Order areas reorder on interact).
        let dim_a = ((140.0 * alpha.clamp(0.0, 1.0)) as u8).max(1);
        egui::Area::new(egui::Id::new("overlay-dim"))
            .fixed_pos(egui::pos2(0.0, 0.0))
            .order(egui::Order::Middle)
            .interactable(true)
            .sense(egui::Sense::click())
            .show(&ctx, |ui| {
                let screen = ctx.content_rect();
                let resp = ui.allocate_response(screen.size(), egui::Sense::click());
                ui.painter().rect_filled(
                    screen,
                    0.0,
                    egui::Color32::from_black_alpha(dim_a),
                );
                if resp.clicked() {
                    self.overlay = Overlay::None;
                }
            });

        // Tall, stable list viewport (~2/3 of the window). Do NOT let ScrollArea
        // shrink to the filtered hit count — that was the “tiny quick open” bug.
        let screen = ctx.content_rect();
        let overlay_top = 48.0 + 16.0 * alpha;
        let chrome_h = 88.0; // title + filter + margins
        let list_h = ((screen.height() - overlay_top - chrome_h) * 0.92)
            .clamp(420.0, screen.height() * 0.72);
        let panel_w = 640.0_f32;

        egui::Area::new(egui::Id::new("overlay"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, overlay_top))
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(&ctx, |ui| {
                theme_apply::overlay_frame(&self.theme).show(ui, |ui| {
                    ui.set_width(panel_w);
                    ui.set_max_width(panel_w);
                    ui.label(
                        RichText::new(title)
                            .strong()
                            .size(15.0)
                            .color(accent),
                    );
                    ui.add_space(6.0);
                    match self.overlay {
                        Overlay::Shortcuts => self.ui_shortcuts(ui, muted, list_h),
                        Overlay::Settings => self.ui_settings(ui, ctx, accent, muted),
                        Overlay::ThemePicker => self.ui_theme_picker(ui, ctx, accent, muted, list_h),
                        Overlay::RecentVaults => {
                            self.ui_recent_vaults(ui, ctx, accent, muted, list_h)
                        }
                        Overlay::Find => self.ui_find(ui, accent, muted, list_h),
                        Overlay::VaultSearch => self.ui_vault_search(ui, accent, muted, list_h),
                        Overlay::QuickOpen | Overlay::CommandPalette => {
                            self.ui_fuzzy(ui, accent, muted, list_h);
                        }
                        Overlay::None => {}
                    }
                });
            });
    }

    fn ui_shortcuts(&self, ui: &mut Ui, muted: egui::Color32, list_h: f32) {
        overlay_scroll_list(ui, list_h, |ui| {
            for (cmd, chord) in self.keymap.all_listed() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{chord:16}")).monospace().color(muted));
                    let label = if let Command::Tab(n) = cmd {
                        format!("Jump tab {n}")
                    } else {
                        cmd.label().to_string()
                    };
                    ui.label(RichText::new(label).monospace());
                });
            }
        });
        ui.label(RichText::new("esc dismiss").monospace().color(muted));
    }

    fn ui_settings(
        &mut self,
        ui: &mut Ui,
        ctx: &egui::Context,
        accent: egui::Color32,
        muted: egui::Color32,
    ) {
        if self.overlay == Overlay::Settings {
            if naked_button(ui, RichText::new("open vault folder…").color(accent)).clicked() {
                self.pick_vault_dialog(&ctx);
                self.overlay = Overlay::None;
            }
            if naked_button(ui, RichText::new("recent vaults…").color(muted)).clicked() {
                self.open_overlay(Overlay::RecentVaults);
                return;
            }
            if self.vault.is_some()
                && naked_button(ui, RichText::new("close vault").color(muted)).clicked()
            {
                self.close_vault();
                self.overlay = Overlay::None;
            }
            ui.add_space(10.0);
            ui.label(RichText::new("UI FONT").size(11.0).strong().color(muted));
            ui.horizontal(|ui| {
                if naked_button(ui, RichText::new("−").color(accent)).clicked() {
                    self.bump_ui_font(-0.05, ctx);
                }
                ui.label(RichText::new(format!(
                    "{:.0}%",
                    self.persist.ui_font_scale * 100.0
                )));
                if naked_button(ui, RichText::new("+").color(accent)).clicked() {
                    self.bump_ui_font(0.05, ctx);
                }
                if naked_button(ui, RichText::new("reset").color(muted)).clicked() {
                    self.persist.ui_font_scale = 1.10;
                    self.persist.save();
                    theme_apply::apply_theme(ctx, &self.theme, self.persist.ui_font_scale);
                    self.status = "UI font reset".into();
                }
            });
            ui.add_space(8.0);
            ui.label(RichText::new("EDITOR FONT").size(11.0).strong().color(muted));
            ui.horizontal(|ui| {
                if naked_button(ui, RichText::new("−").color(accent)).clicked() {
                    self.bump_editor_font(-1.5);
                }
                ui.label(RichText::new(format!("{:.0}px", self.persist.editor_font_size)));
                if naked_button(ui, RichText::new("+").color(accent)).clicked() {
                    self.bump_editor_font(1.5);
                }
                if naked_button(ui, RichText::new("reset").color(muted)).clicked() {
                    self.handle_command(Command::FontReset, ctx);
                }
            });
            ui.add_space(8.0);
            ui.label(
                RichText::new("NOTE PADDING (L/R)")
                    .size(11.0)
                    .strong()
                    .color(muted),
            );
            ui.horizontal(|ui| {
                if naked_button(ui, RichText::new("−").color(accent)).clicked() {
                    self.bump_note_pad(-16.0);
                }
                ui.label(RichText::new(format!("{:.0}px", self.persist.note_h_pad)));
                if naked_button(ui, RichText::new("+").color(accent)).clicked() {
                    self.bump_note_pad(16.0);
                }
                if naked_button(ui, RichText::new("reset").color(muted)).clicked() {
                    self.persist.note_h_pad = 120.0;
                    self.persist.save();
                    self.status = "note padding reset".into();
                }
            });
            ui.label(
                RichText::new(
                    "preferred L/R pad — shrinks automatically when the window is narrow",
                )
                .size(10.0)
                .color(muted.gamma_multiply(0.75)),
            );
            ui.add_space(10.0);
            ui.label(RichText::new("SIDEBARS").size(11.0).strong().color(muted));
            ui.horizontal(|ui| {
                if sidebar_toggle(
                    ui,
                    "files",
                    self.persist.show_left_sidebar,
                    accent,
                    muted,
                )
                .clicked()
                {
                    self.toggle_left_sidebar();
                }
                ui.add_space(8.0);
                if sidebar_toggle(
                    ui,
                    "outline",
                    self.persist.show_right_sidebar,
                    accent,
                    muted,
                )
                .clicked()
                {
                    self.toggle_right_sidebar();
                }
            });
            ui.add_space(10.0);
            ui.label(RichText::new("THEME").size(11.0).strong().color(muted));
            if naked_button(ui, RichText::new("choose theme…").color(accent)).clicked() {
                self.open_overlay(Overlay::ThemePicker);
                return;
            }
            ui.label(
                RichText::new(format!("current: {}", self.theme_name)).color(muted),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!("keymap profile: {}", self.keymap.profile)),
            );
            ui.label(
                RichText::new("edit ~/.config/celestine/keybindings.toml to remap").color(muted),
            );
        }
    }

    fn ui_theme_picker(
        &mut self,
        ui: &mut Ui,
        ctx: &egui::Context,
        accent: egui::Color32,
        muted: egui::Color32,
        list_h: f32,
    ) {
        let filter_id = egui::Id::new("celestine-theme-filter");
        let resp = ui.add(
            TextEdit::singleline(&mut self.overlay_query)
                .id(filter_id)
                .hint_text("filter themes — ↑↓ enter")
                .desired_width(f32::INFINITY),
        );
        if self.overlay_focus_pending {
            resp.request_focus();
            self.overlay_focus_pending = false;
            // Start selection on the active theme
            let themes = list_available_themes();
            if let Some(i) = themes.iter().position(|t| t == &self.theme_name) {
                self.overlay_selected = i;
            }
        }

        let q = self.overlay_query.to_lowercase();
        let themes: Vec<String> = list_available_themes()
            .into_iter()
            .filter(|t| q.is_empty() || t.to_lowercase().contains(&q))
            .collect();

        if self.overlay_selected >= themes.len() && !themes.is_empty() {
            self.overlay_selected = themes.len() - 1;
        }

        if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
            self.overlay_selected =
                (self.overlay_selected + 1).min(themes.len().saturating_sub(1));
        }
        if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
            self.overlay_selected = self.overlay_selected.saturating_sub(1);
        }

        let text_c = color(self.theme.text_normal);
        let mut pick = None;
        overlay_scroll_list(ui, list_h, |ui| {
            for (i, name) in themes.iter().enumerate() {
                let selected = i == self.overlay_selected;
                let is_current = name == &self.theme_name;
                let label = if is_current {
                    format!("★  {name}")
                } else {
                    format!("   {name}")
                };
                let row_color = if selected {
                    accent
                } else if is_current {
                    text_c
                } else {
                    muted
                };
                let row = ui.add(
                    egui::Button::new(RichText::new(label).color(row_color))
                        .frame(false)
                        .selected(selected),
                );
                if selected {
                    row.scroll_to_me(Some(egui::Align::Center));
                }
                if row.clicked() {
                    pick = Some(name.clone());
                }
            }
        });

        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            if let Some(name) = themes.get(self.overlay_selected) {
                pick = Some(name.clone());
            }
        }

        ui.add_space(4.0);
        ui.label(
            RichText::new("↑↓ move · enter apply · esc dismiss")
                .size(11.0)
                .color(muted),
        );

        if let Some(name) = pick {
            self.set_theme(&name, ctx);
            self.overlay = Overlay::None;
        }
    }

    fn ui_recent_vaults(
        &mut self,
        ui: &mut Ui,
        ctx: &egui::Context,
        accent: egui::Color32,
        muted: egui::Color32,
        list_h: f32,
    ) {
        self.persist.clear_recent_missing();
        if naked_button(
            ui,
            RichText::new(format!(
                "browse for vault…  {}",
                self.keymap.display_chord(Command::OpenVault)
            ))
            .color(accent),
        )
        .clicked()
        {
            self.pick_vault_dialog(&ctx);
            self.overlay = Overlay::None;
            return;
        }
        ui.add_space(8.0);
        if self.persist.recent_vaults.is_empty() {
            ui.label(RichText::new("(no recent vaults)").color(muted));
            return;
        }

        let vaults = self.persist.recent_vaults.clone();
        if self.overlay_selected >= vaults.len() {
            self.overlay_selected = vaults.len().saturating_sub(1);
        }
        // Prefer last vault on first open of this overlay.
        if self.overlay_focus_pending {
            if let Some(last) = &self.persist.last_vault {
                if let Some(i) = vaults.iter().position(|p| p == last) {
                    self.overlay_selected = i;
                }
            }
            self.overlay_focus_pending = false;
        }

        if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
            self.overlay_selected = (self.overlay_selected + 1).min(vaults.len().saturating_sub(1));
        }
        if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
            self.overlay_selected = self.overlay_selected.saturating_sub(1);
        }

        let mut open = None;
        overlay_scroll_list(ui, list_h, |ui| {
            for (i, path) in vaults.iter().enumerate() {
                let selected = i == self.overlay_selected;
                let is_last = self.persist.last_vault.as_deref() == Some(path.as_str());
                let mark = if is_last { "★" } else { " " };
                let label = format!("{mark}  {path}");
                let color = if selected {
                    accent
                } else if is_last {
                    accent.gamma_multiply(0.85)
                } else {
                    muted
                };
                let row = ui.add(
                    egui::Button::new(RichText::new(label).monospace().color(color))
                        .frame(false)
                        .selected(selected),
                );
                if selected {
                    row.scroll_to_me(Some(egui::Align::Center));
                }
                if row.clicked() {
                    open = Some(path.clone());
                }
            }
        });

        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            if let Some(path) = vaults.get(self.overlay_selected) {
                open = Some(path.clone());
            }
        }

        ui.add_space(4.0);
        ui.label(
            RichText::new("↑↓ move · enter open · esc dismiss")
                .size(11.0)
                .color(muted),
        );

        if let Some(path) = open {
            self.overlay = Overlay::None;
            self.open_vault(PathBuf::from(path), ctx);
        }
    }

    fn line_flash_alpha(&self) -> Option<f32> {
        let flash = self.line_flash.as_ref()?;
        let t = flash.started.elapsed().as_secs_f32();
        if t >= LINE_FLASH_SECS {
            return None;
        }
        let u = 1.0 - t / LINE_FLASH_SECS;
        // Ease-out so the wash is strongest right after the jump.
        Some(u * u)
    }

    fn ui_find(
        &mut self,
        ui: &mut Ui,
        accent: egui::Color32,
        muted: egui::Color32,
        list_h: f32,
    ) {
        let resp = ui.add(
            TextEdit::singleline(&mut self.find_query)
                .id(egui::Id::new("celestine-find"))
                .hint_text("find in note")
                .desired_width(f32::INFINITY),
        );
        if self.overlay_focus_pending {
            resp.request_focus();
            self.overlay_focus_pending = false;
        }

        let hits: Vec<(usize, usize, String)> = self
            .tabs
            .get(self.active_tab)
            .filter(|t| t.kind == TabKind::Note)
            .map(|t| find_in_buffer_hits(&t.buffer, &self.find_query))
            .unwrap_or_default();

        if !self.find_query.is_empty() {
            ui.label(
                RichText::new(format!("{} matches", hits.len()))
                    .monospace()
                    .color(muted),
            );
        }

        if self.overlay_selected >= hits.len() && !hits.is_empty() {
            self.overlay_selected = hits.len() - 1;
        }

        if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
            self.overlay_selected = (self.overlay_selected + 1).min(hits.len().saturating_sub(1));
        }
        if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
            self.overlay_selected = self.overlay_selected.saturating_sub(1);
        }

        let mut chosen: Option<usize> = None;
        overlay_scroll_list(ui, list_h, |ui| {
            for (i, (_char_off, line_1based, snippet)) in hits.iter().enumerate() {
                let selected = i == self.overlay_selected;
                let row = format!("L{line_1based}  {snippet}");
                let resp = ui.add(
                    egui::Button::new(
                        RichText::new(row)
                            .monospace()
                            .color(if selected { accent } else { muted }),
                    )
                    .frame(false)
                    .selected(selected),
                );
                if selected {
                    resp.scroll_to_me(Some(egui::Align::Center));
                }
                if resp.clicked() {
                    chosen = Some(i);
                }
            }
        });

        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            if !hits.is_empty() {
                chosen = Some(self.overlay_selected.min(hits.len() - 1));
            }
        }

        if let Some(i) = chosen {
            if let Some((_char_off, line_1based, _snippet)) = hits.get(i) {
                let line_0 = line_1based.saturating_sub(1);
                let needle = self.find_query.trim().to_string();
                let (end_char, in_preview) = self
                    .tabs
                    .get(self.active_tab)
                    .map(|t| {
                        (
                            char_offset_end_of_line(&t.buffer, line_0),
                            t.preview_mode,
                        )
                    })
                    .unwrap_or((0, false));
                self.overlay = Overlay::None;
                if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                    if in_preview {
                        // Preview scrolls via line_flash needle; don't leave a
                        // stale caret jump for when the user toggles edit later.
                        tab.scroll_to_char = None;
                    } else {
                        // Caret at end of the matched line for quick editing.
                        tab.scroll_to_char = Some(end_char);
                    }
                    tab.scroll_to_line = None;
                    tab.scroll_to_heading = None;
                }
                self.line_flash = Some(LineFlash {
                    line_0,
                    needle,
                    started: Instant::now(),
                    do_scroll: true,
                });
                self.focus = FocusPane::Editor;
                self.editor_focus_pending = !in_preview;
            }
        }
    }

    fn ui_vault_search(
        &mut self,
        ui: &mut Ui,
        accent: egui::Color32,
        muted: egui::Color32,
        list_h: f32,
    ) {
        let resp = ui.add(
            TextEdit::singleline(&mut self.overlay_query)
                .id(egui::Id::new("celestine-vault-search"))
                .hint_text("search notes — names and contents")
                .desired_width(f32::INFINITY),
        );
        if self.overlay_focus_pending {
            resp.request_focus();
            self.overlay_focus_pending = false;
        }

        let hits = if let Some(vault) = &self.vault {
            collect_vault_search_hits(vault, &self.overlay_query, &self.recent_notes)
        } else {
            Vec::new()
        };

        if self.overlay_selected >= hits.len() && !hits.is_empty() {
            self.overlay_selected = hits.len() - 1;
        }

        if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
            self.overlay_selected = (self.overlay_selected + 1).min(hits.len().saturating_sub(1));
        }
        if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
            self.overlay_selected = self.overlay_selected.saturating_sub(1);
        }

        let mut chosen: Option<usize> = None;
        overlay_scroll_list(ui, list_h, |ui| {
            for (i, hit) in hits.iter().enumerate() {
                let selected = i == self.overlay_selected;
                let row = if hit.snippet.is_empty() {
                    hit.rel.clone()
                } else {
                    format!("{}  {}", hit.rel, hit.snippet)
                };
                let resp = ui.add(
                    egui::Button::new(
                        RichText::new(row)
                            .monospace()
                            .color(if selected { accent } else { muted }),
                    )
                    .frame(false)
                    .selected(selected),
                );
                if selected {
                    resp.scroll_to_me(Some(egui::Align::Center));
                }
                if resp.clicked() {
                    chosen = Some(i);
                }
            }
        });

        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            if !hits.is_empty() {
                chosen = Some(self.overlay_selected.min(hits.len() - 1));
            }
        }

        if let Some(i) = chosen {
            if let Some(hit) = hits.get(i) {
                let rel = hit.rel.clone();
                let line_0 = hit.line_1based.saturating_sub(1);
                let needle = if self.overlay_query.trim().is_empty() {
                    hit.snippet.clone()
                } else {
                    self.overlay_query.trim().to_string()
                };
                self.overlay = Overlay::None;
                self.open_path(&rel, true);
                let end_char = self
                    .tabs
                    .get(self.active_tab)
                    .map(|t| char_offset_end_of_line(&t.buffer, line_0));
                if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                    if let Some(end) = end_char {
                        tab.scroll_to_char = Some(end);
                        tab.scroll_to_line = None;
                    } else {
                        tab.scroll_to_line = Some(line_0);
                        tab.scroll_to_char = None;
                    }
                    tab.scroll_to_heading = None;
                }
                self.line_flash = Some(LineFlash {
                    line_0,
                    needle,
                    started: Instant::now(),
                    do_scroll: true,
                });
                self.focus = FocusPane::Editor;
                self.editor_focus_pending = true;
            }
        }
    }

    fn ui_fuzzy(
        &mut self,
        ui: &mut Ui,
        accent: egui::Color32,
        muted: egui::Color32,
        list_h: f32,
    ) {
        let resp = ui.add(
            TextEdit::singleline(&mut self.overlay_query)
                .id(egui::Id::new("celestine-fuzzy-filter"))
                .hint_text(if self.overlay == Overlay::QuickOpen {
                    "filter notes — recent first"
                } else {
                    "type to filter"
                })
                .desired_width(f32::INFINITY),
        );
        if self.overlay_focus_pending {
            resp.request_focus();
            self.overlay_focus_pending = false;
        }

        let matcher = SkimMatcherV2::default();
        let mut items: Vec<(i64, String, String)> = Vec::new();
        let recent = self.recent_notes.clone();
        let recency_rank = |rel: &str| -> Option<usize> {
            recent.iter().position(|r| r == rel)
        };
        let empty_q = self.overlay_query.is_empty();

        match self.overlay {
            Overlay::QuickOpen => {
                if let Some(vault) = &self.vault {
                    for n in vault.index.notes() {
                        let hay = format!("{} {}", n.rel, n.aliases.join(" "));
                        let score = if empty_q {
                            // Prefer recently opened notes for this vault.
                            Some(match recency_rank(&n.rel) {
                                Some(i) => 1_000_000 - i as i64,
                                None => 0,
                            })
                        } else {
                            matcher.fuzzy_match(&hay, &self.overlay_query).map(|s| {
                                let boost = recency_rank(&n.rel)
                                    .map(|i| ((48 - i.min(48)) as i64) * 8)
                                    .unwrap_or(0);
                                s + boost
                            })
                        };
                        if let Some(s) = score {
                            let preview = if recency_rank(&n.rel).is_some() && empty_q {
                                "recent".into()
                            } else {
                                n.aliases.first().cloned().unwrap_or_default()
                            };
                            items.push((s, n.rel.clone(), preview));
                        }
                    }
                    for rel in vault.list_image_rel_paths() {
                        let score = if empty_q {
                            Some(match recency_rank(&rel) {
                                Some(i) => 1_000_000 - i as i64,
                                None => 0,
                            })
                        } else {
                            matcher.fuzzy_match(&rel, &self.overlay_query).map(|s| {
                                let boost = recency_rank(&rel)
                                    .map(|i| ((48 - i.min(48)) as i64) * 8)
                                    .unwrap_or(0);
                                s + boost
                            })
                        };
                        if let Some(s) = score {
                            let tag = if recency_rank(&rel).is_some() && empty_q {
                                "recent · image"
                            } else {
                                "image"
                            };
                            items.push((s, rel, tag.into()));
                        }
                    }
                }
            }
            Overlay::CommandPalette => {
                let cmds = [
                    Command::OpenVault,
                    Command::RecentVaults,
                    Command::CloseVault,
                    Command::QuickOpen,
                    Command::ThemePicker,
                    Command::Graph,
                    Command::NewNote,
                    Command::NewFolder,
                    Command::Rename,
                    Command::DeleteItem,
                    Command::Save,
                    Command::TogglePreview,
                    Command::VaultSearch,
                    Command::FontBigger,
                    Command::FontSmaller,
                    Command::FontReset,
                    Command::ToggleLeftSidebar,
                    Command::ToggleRightSidebar,
                    Command::NavBack,
                    Command::NavForward,
                    Command::RevealInTree,
                    Command::Settings,
                    Command::Shortcuts,
                    Command::CloseTab,
                    Command::FocusFiles,
                    Command::FocusOutline,
                    Command::NextTab,
                    Command::PrevTab,
                ];
                for c in cmds {
                    let label = format!("{}  {}", c.label(), self.keymap.display_chord(c));
                    let score = if empty_q {
                        Some(0)
                    } else {
                        matcher.fuzzy_match(&label, &self.overlay_query)
                    };
                    if let Some(s) = score {
                        items.push((s, format!("cmd:{}", c.id()), label));
                    }
                }
            }
            _ => {}
        }

        // Higher score first; for equal scores keep alphabetical (stable browse).
        items.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        if self.overlay_selected >= items.len() && !items.is_empty() {
            self.overlay_selected = items.len() - 1;
        }

        if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
            self.overlay_selected = (self.overlay_selected + 1).min(items.len().saturating_sub(1));
        }
        if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
            self.overlay_selected = self.overlay_selected.saturating_sub(1);
        }

        let mut chosen = None;
        overlay_scroll_list(ui, list_h, |ui| {
            for (i, (_s, id, label)) in items.iter().enumerate() {
                let selected = i == self.overlay_selected;
                let row = if label.is_empty() {
                    id.clone()
                } else {
                    format!("{id}  {label}")
                };
                if ui
                    .selectable_label(
                        selected,
                        RichText::new(row)
                            .monospace()
                            .color(if selected { accent } else { muted }),
                    )
                    .clicked()
                {
                    chosen = Some(id.clone());
                }
            }
        });

        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            if let Some((_s, id, _)) = items.get(self.overlay_selected) {
                chosen = Some(id.clone());
            } else if self.overlay == Overlay::QuickOpen && !self.overlay_query.is_empty() {
                let q = self.overlay_query.trim();
                chosen = Some(if crate::media::is_image_rel(q) {
                    q.replace('\\', "/")
                } else {
                    ensure_md_path(q)
                });
            }
        }

        if let Some(id) = chosen {
            if let Some(cmd_id) = id.strip_prefix("cmd:") {
                self.overlay = Overlay::None;
                if let Some(cmd) = cmd_from_id(cmd_id) {
                    self.handle_command(cmd, ui.ctx());
                }
            } else {
                self.overlay = Overlay::None;
                self.open_path(&id, true);
            }
        }
    }
}

fn ensure_md_path(q: &str) -> String {
    if q.to_lowercase().ends_with(".md") {
        q.to_string()
    } else {
        format!("{q}.md")
    }
}

/// Fixed-height scroll viewport for overlays.
///
/// `ScrollArea` alone can shrink to the filtered hit count (egui only keeps
/// `min_scrolled_height` when scrollbars are needed). Allocating an exact rect
/// first keeps quick-open / commands tall even with 1–2 matches.
fn overlay_scroll_list(ui: &mut Ui, list_h: f32, add_contents: impl FnOnce(&mut Ui)) {
    let w = ui.available_width().max(1.0);
    let h = list_h.max(200.0);
    ui.allocate_ui_with_layout(
        egui::vec2(w, h),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_min_size(egui::vec2(w, h));
            ui.set_max_size(egui::vec2(w, h));
            ScrollArea::vertical()
                .id_salt("overlay-scroll-list")
                .max_height(h)
                .min_scrolled_height(h)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_min_width(w - 8.0);
                    add_contents(ui);
                });
        },
    );
}

fn load_recent_notes(vault: &Vault) -> Vec<String> {
    #[derive(serde::Deserialize, Default)]
    struct File {
        #[serde(default)]
        notes: Vec<String>,
    }
    let Ok(text) = vault.read_metadata("recent-notes.toml") else {
        return Vec::new();
    };
    toml::from_str::<File>(&text)
        .map(|f| f.notes)
        .unwrap_or_default()
}

fn save_recent_notes(vault: &Vault, notes: &[String]) {
    #[derive(serde::Serialize)]
    struct File<'a> {
        notes: &'a [String],
    }
    if let Ok(body) = toml::to_string_pretty(&File { notes }) {
        let _ = vault.write_metadata("recent-notes.toml", &body);
    }
}

#[cfg(test)]
mod recent_notes_boundary_tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn recent_notes_do_not_read_or_write_through_external_metadata_symlink() {
        use std::os::unix::fs::symlink;

        let vault = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let sentinel = outside.path().join("recent-notes.toml");
        std::fs::write(&sentinel, "notes = [\"outside-secret.md\"]\n").unwrap();
        symlink(outside.path(), vault.path().join(".celestine")).unwrap();

        let vault = Vault::open(vault.path()).unwrap();
        assert!(load_recent_notes(&vault).is_empty());
        save_recent_notes(&vault, &["inside.md".to_string()]);
        assert_eq!(
            std::fs::read_to_string(sentinel).unwrap(),
            "notes = [\"outside-secret.md\"]\n"
        );
    }
}

/// Case-insensitive substring hits in a note buffer.
/// Returns `(char_offset, line_1based, snippet)`.
fn find_in_buffer_hits(buffer: &str, query: &str) -> Vec<(usize, usize, String)> {
    let q = query.trim();
    if q.is_empty() {
        return Vec::new();
    }
    let q_lower = q.to_lowercase();
    let q_chars = q_lower.chars().count();
    let mut hits = Vec::new();
    let mut char_at_line_start = 0usize;

    for (line_idx, line) in buffer.split_inclusive('\n').enumerate() {
        let line_text = line.trim_end_matches(['\n', '\r']);
        let line_lower = line_text.to_lowercase();
        let mut search_from = 0usize;
        while let Some(rel) = line_lower[search_from..].find(&q_lower) {
            let match_byte = search_from + rel;
            let match_char_in_line = line_lower[..match_byte].chars().count();
            let char_off = char_at_line_start + match_char_in_line;
            hits.push((
                char_off,
                line_idx + 1,
                truncate_snippet(line_text, 96),
            ));
            // Advance by one char so overlapping matches still surface.
            let advance = q_chars.max(1);
            let mut chars = line_lower[match_byte..].chars();
            let mut byte_adv = 0;
            for _ in 0..advance {
                if let Some(c) = chars.next() {
                    byte_adv += c.len_utf8();
                }
            }
            search_from = match_byte + byte_adv.max(1);
            if search_from >= line_lower.len() {
                break;
            }
        }
        char_at_line_start += line.chars().count();
    }
    hits
}

struct VaultSearchHit {
    score: i64,
    rel: String,
    line_1based: usize,
    snippet: String,
}

const VAULT_SEARCH_MAX_HITS: usize = 150;
const VAULT_SEARCH_MAX_PER_NOTE: usize = 3;

fn collect_vault_search_hits(
    vault: &Vault,
    query: &str,
    recent: &[String],
) -> Vec<VaultSearchHit> {
    let matcher = SkimMatcherV2::default();
    let empty_q = query.trim().is_empty();
    let recency_rank = |rel: &str| -> Option<usize> { recent.iter().position(|r| r == rel) };
    let mut hits: Vec<VaultSearchHit> = Vec::new();

    for n in vault.index.notes() {
        if empty_q {
            let score = match recency_rank(&n.rel) {
                Some(i) => 1_000_000 - i as i64,
                None => 0,
            };
            let snippet = n
                .body
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .to_string();
            hits.push(VaultSearchHit {
                score,
                rel: n.rel.clone(),
                line_1based: 1,
                snippet: truncate_snippet(&snippet, 96),
            });
            continue;
        }

        let hay = format!("{} {}", n.rel, n.aliases.join(" "));
        let path_score = matcher.fuzzy_match(&hay, query).map(|s| {
            let boost = recency_rank(&n.rel)
                .map(|i| ((48 - i.min(48)) as i64) * 8)
                .unwrap_or(0);
            s + boost
        });

        let mut line_hits: Vec<(i64, usize, String)> = Vec::new();
        for (line_idx, line) in n.content.split_inclusive('\n').enumerate() {
            let line_text = line.trim_end_matches(['\n', '\r']);
            if line_text.trim().is_empty() {
                continue;
            }
            if let Some(s) = matcher.fuzzy_match(line_text, query) {
                let boost = recency_rank(&n.rel)
                    .map(|i| ((48 - i.min(48)) as i64) * 4)
                    .unwrap_or(0);
                line_hits.push((
                    s + boost,
                    line_idx + 1,
                    truncate_snippet(line_text, 96),
                ));
            }
        }
        line_hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        line_hits.truncate(VAULT_SEARCH_MAX_PER_NOTE);

        if line_hits.is_empty() {
            if let Some(s) = path_score {
                let snippet = n
                    .body
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .to_string();
                hits.push(VaultSearchHit {
                    score: s,
                    rel: n.rel.clone(),
                    line_1based: 1,
                    snippet: truncate_snippet(&snippet, 96),
                });
            }
        } else {
            for (s, line_1based, snippet) in line_hits {
                // Prefer content hits; boost if path also matched.
                let score = if path_score.is_some() { s + 40 } else { s };
                hits.push(VaultSearchHit {
                    score,
                    rel: n.rel.clone(),
                    line_1based,
                    snippet,
                });
            }
        }
    }

    hits.sort_by(|a, b| b.score.cmp(&a.score).then(a.rel.cmp(&b.rel)));
    hits.truncate(VAULT_SEARCH_MAX_HITS);
    hits
}

fn truncate_snippet(s: &str, max_chars: usize) -> String {
    let trimmed = s.trim();
    let count = trimmed.chars().count();
    if count <= max_chars {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod search_helpers_tests {
    use super::*;

    #[test]
    fn find_in_buffer_hits_case_insensitive_with_line_and_offset() {
        let buf = "Hello World\nfoo BAR baz\n";
        let hits = find_in_buffer_hits(buf, "bar");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, 2);
        assert!(hits[0].2.to_lowercase().contains("bar"));
        // "Hello World\n" = 12 chars; "foo " = 4 → offset 16
        assert_eq!(hits[0].0, 16);
    }

    #[test]
    fn vault_search_finds_body_content_not_just_filename() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("alpha.md"),
            "---\ntitle: Alpha\n---\n\nUniqueZebraPhrase lives here.\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("beta.md"), "nothing interesting\n").unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let hits = collect_vault_search_hits(&vault, "UniqueZebraPhrase", &[]);
        assert!(
            hits.iter().any(|h| h.rel == "alpha.md"),
            "expected alpha.md in hits: {:?}",
            hits.iter().map(|h| &h.rel).collect::<Vec<_>>()
        );
        assert!(hits.iter().any(|h| h.snippet.contains("UniqueZebraPhrase")));
    }
}

fn char_offset_at_line(text: &str, line: usize) -> usize {
    let mut chars = 0usize;
    for (i, l) in text.split_inclusive('\n').enumerate() {
        if i == line {
            return chars;
        }
        chars += l.chars().count();
    }
    chars
}

/// Char offset of the end of `line` (0-based), before the trailing newline.
fn char_offset_end_of_line(text: &str, line: usize) -> usize {
    let mut chars = 0usize;
    for (i, l) in text.split_inclusive('\n').enumerate() {
        if i == line {
            let content = l.trim_end_matches(['\n', '\r']);
            return chars + content.chars().count();
        }
        chars += l.chars().count();
    }
    chars
}

/// Screen rect covering source line `line_0` in a laid-out editor galley.
fn source_line_rect(
    galley: &egui::Galley,
    galley_pos: egui::Pos2,
    line_0: usize,
    width: f32,
) -> Option<egui::Rect> {
    let mut src_line = 0usize;
    let mut at_line_start = true;
    let mut min_y = f32::MAX;
    let mut max_y = f32::MIN;
    let mut found = false;
    for row in &galley.rows {
        if at_line_start && src_line == line_0 {
            found = true;
        }
        if found && src_line == line_0 {
            let r = row.rect();
            min_y = min_y.min(r.min.y);
            max_y = max_y.max(r.max.y);
        }
        if row.ends_with_newline {
            if found && src_line == line_0 {
                break;
            }
            src_line += 1;
            at_line_start = true;
        } else {
            at_line_start = false;
        }
    }
    if !found || min_y > max_y {
        return None;
    }
    Some(egui::Rect::from_min_max(
        egui::pos2(galley_pos.x, galley_pos.y + min_y),
        egui::pos2(galley_pos.x + width.max(1.0), galley_pos.y + max_y),
    ))
}

fn char_offset_for_heading(text: &str, heading: &str) -> Option<usize> {
    let heading = heading.trim();
    let candidates = text.lines().enumerate().filter_map(|(i, line)| {
        let trimmed = line.trim_start();
        trimmed
            .starts_with('#')
            .then(|| (i, trimmed.trim_start_matches('#').trim_start()))
    });
    // Exact text first, so `# A B` and `# A-B` stay distinct; slug anchors after.
    let mut slug_hit = None;
    for (i, after_hashes) in candidates {
        if after_hashes == heading {
            return Some(char_offset_at_line(text, i));
        }
        if slug_hit.is_none() && preview::heading_matches(after_hashes, heading) {
            slug_hit = Some(i);
        }
    }
    slug_hit.map(|i| char_offset_at_line(text, i))
}

struct TextStats {
    lines: usize,
    words: usize,
    symbols: usize,
}

fn text_stats(text: &str) -> TextStats {
    let lines = if text.is_empty() {
        1
    } else {
        text.bytes().filter(|&b| b == b'\n').count() + 1
    };
    let words = text.split_whitespace().count();
    let symbols = text.chars().count();
    TextStats {
        lines,
        words,
        symbols,
    }
}

fn line_gutter_width(lines: usize, font_size: f32) -> f32 {
    let digits = ((lines.max(1) as f32).log10().floor() as usize) + 1;
    digits.max(2) as f32 * font_size * 0.62 + 10.0
}

/// Adaptive L/R note padding.
///
/// `preferred_pad` is the *maximum* inset (settings / `[` `]`). On narrow panes
/// (typical with both sidebars open) padding collapses to a few pixels so the
/// text can use the column. Padding only grows once usable width exceeds a
/// comfortable reading measure.
///
/// `chrome` is space already reserved on the left (line gutter + gap).
/// Returns `(pad_each_side, text_width)`.
fn note_side_pad(inner_w: f32, chrome: f32, preferred_pad: f32, min_text: f32) -> (f32, f32) {
    let usable = (inner_w - chrome).max(1.0);
    let max_pad = ((usable - min_text) * 0.5).max(0.0);
    let preferred = preferred_pad.max(0.0);

    // Target text measure before we start centering with pad.
    const COMFORT_TEXT: f32 = 680.0;
    // Below this, pad is essentially just an edge breath.
    const NARROW: f32 = 520.0;
    const EDGE: f32 = 6.0;

    let target = if usable <= NARROW {
        EDGE
    } else {
        // Leftover past the comfort column becomes pad (both sides), capped.
        let excess = (usable - COMFORT_TEXT).max(0.0);
        let from_excess = excess * 0.5;
        // Smooth ramp between NARROW and the width where preferred fully fits.
        let full_at = COMFORT_TEXT + 2.0 * preferred;
        let t = ((usable - NARROW) / (full_at - NARROW).max(1.0)).clamp(0.0, 1.0);
        let t = t * t; // ease-in so mid widths stay tighter
        let ramped = EDGE + (preferred - EDGE) * t;
        from_excess.max(EDGE).min(ramped).min(preferred)
    };

    let pad = target.clamp(0.0, max_pad);
    let text_w = (usable - 2.0 * pad).max(40.0_f32.min(usable));
    (pad, text_w)
}

fn paint_line_numbers(
    ui: &Ui,
    gutter_rect: egui::Rect,
    galley: &egui::Galley,
    galley_pos: egui::Pos2,
    font_size: f32,
    color: egui::Color32,
) {
    let painter = ui.painter_at(gutter_rect);
    let font = FontId::monospace(font_size * 0.9);
    let mut src_line = 1usize;
    let mut at_line_start = true;
    for row in &galley.rows {
        if at_line_start {
            let y = galley_pos.y + row.rect().min.y;
            painter.text(
                egui::pos2(gutter_rect.right() - 4.0, y),
                egui::Align2::RIGHT_TOP,
                format!("{src_line}"),
                font.clone(),
                color,
            );
        }
        if row.ends_with_newline {
            src_line += 1;
            at_line_start = true;
        } else {
            at_line_start = false;
        }
    }
}

fn cmd_from_id(id: &str) -> Option<Command> {
    Some(match id {
        "quick_open" => Command::QuickOpen,
        "graph" => Command::Graph,
        "new_note" => Command::NewNote,
        "new_folder" => Command::NewFolder,
        "rename" => Command::Rename,
        "delete_item" => Command::DeleteItem,
        "save" => Command::Save,
        "toggle_preview" => Command::TogglePreview,
        "vault_search" => Command::VaultSearch,
        "settings" => Command::Settings,
        "shortcuts" => Command::Shortcuts,
        "close_tab" => Command::CloseTab,
        "focus_files" => Command::FocusFiles,
        "focus_outline" => Command::FocusOutline,
        "command_palette" => Command::CommandPalette,
        "theme_picker" => Command::ThemePicker,
        "open_vault" => Command::OpenVault,
        "recent_vaults" => Command::RecentVaults,
        "close_vault" => Command::CloseVault,
        "font_bigger" => Command::FontBigger,
        "font_smaller" => Command::FontSmaller,
        "font_reset" => Command::FontReset,
        "toggle_left_sidebar" => Command::ToggleLeftSidebar,
        "toggle_right_sidebar" => Command::ToggleRightSidebar,
        "nav_back" => Command::NavBack,
        "nav_forward" => Command::NavForward,
        "reveal_in_tree" => Command::RevealInTree,
        "next_tab" => Command::NextTab,
        "prev_tab" => Command::PrevTab,
        _ => return None,
    })
}


#[cfg(test)]
mod note_open_boundary_tests {
    use super::*;

    // Avoid loading user configuration, persisting app state, or checking releases.
    fn test_app(vault: Vault) -> CelestineApp {
        CelestineApp {
            vault: Some(vault),
            tabs: Vec::new(),
            active_tab: 0,
            former_tab: 0,
            keymap: Keymap::default(),
            theme: ThemeTokens::default(),
            theme_name: "test".into(),
            overlay: Overlay::None,
            overlay_query: String::new(),
            overlay_selected: 0,
            overlay_anim: 0.0,
            focus: FocusPane::Editor,
            show_graph: false,
            graph: GraphState::new(),
            status: format!(
                "open vault {} · recent {} · themes {} · ?",
                "Mod+Shift+O", "Mod+R", "Mod+T"
            ),
            nav_back: Vec::new(),
            nav_forward: Vec::new(),
            find_query: String::new(),
            editor_focused: false,
            editor_focus_pending: false,
            last_frame: Instant::now(),
            persist: AppState::default(),
            tree_expanded: HashSet::new(),
            tree_cursor: None,
            tree_rename: None,
            outline_collapsed: HashSet::new(),
            overlay_focus_pending: false,
            folder_icons: HashMap::new(),
            recent_notes: Vec::new(),
            update_checker: UpdateChecker::start("test-disabled"),
            update_banner_hidden: false,
            image_resolver: crate::media::ImageResolver::default(),
            image_rels: None,
            line_flash: None,
        }
    }

    #[test]
    fn opening_a_traversal_link_preserves_outside_file_and_editor_state() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Vault");
        let outside = temp.path().join("Outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(root.join("Home.md"), "# Home").unwrap();
        let sentinel = outside.join("probe.md");
        std::fs::write(&sentinel, "outside sentinel").unwrap();
        let mut app = test_app(Vault::open(&root).unwrap());
        app.open_note("Home.md", false);

        for rel in ["../Outside/probe.md", "../Outside/new-probe.md"] {
            app.open_note(rel, true);
            assert_eq!(app.tabs.len(), 1);
            assert_eq!(app.tabs[0].rel, "Home.md");
            assert_eq!(app.tabs[0].buffer, "# Home");
            assert!(app.nav_back.is_empty());
            assert!(app.status.starts_with("create failed:"));
            assert_eq!(
                std::fs::read_to_string(&sentinel).unwrap(),
                "outside sentinel"
            );
            assert!(!outside.join("new-probe.md").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn opening_a_note_through_internal_symlink_reuses_the_same_tab() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("real")).unwrap();
        std::fs::write(temp.path().join("real/note.md"), "# Note").unwrap();
        symlink(temp.path().join("real"), temp.path().join("alias")).unwrap();
        let mut app = test_app(Vault::open(temp.path()).unwrap());
        app.open_note("alias/note.md", false);
        app.tabs[0].buffer.push_str("\nunsaved edit");
        app.tabs[0].dirty = true;
        app.open_note("real/note.md", true);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].rel, "real/note.md");
        assert!(app.tabs[0].buffer.ends_with("unsaved edit"));
        assert!(app.tabs[0].dirty);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An app over a throwaway vault; nothing touches the user's config dir.
    fn app_with_notes(rels: &[&str]) -> (tempfile::TempDir, CelestineApp) {
        let dir = tempfile::tempdir().unwrap();
        for rel in rels {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, format!("# {rel}\n\n## Section\n")).unwrap();
        }
        let mut app = CelestineApp::with_parts(
            Keymap::obsidian_defaults(),
            ThemeTokens::default(),
            "test".into(),
            AppState::default(),
            UpdateChecker::idle(),
        );
        app.vault = Some(Vault::open(dir.path()).unwrap());
        (dir, app)
    }

    fn active_rel(app: &CelestineApp) -> &str {
        &app.tabs[app.active_tab].rel
    }

    fn vault_rels(app: &CelestineApp) -> Vec<String> {
        app.vault.as_ref().unwrap().list_md_rel_paths()
    }

    #[test]
    fn tree_rename_without_extension_keeps_md() {
        let (_d, mut app) = app_with_notes(&["Note.md"]);
        app.open_note("Note.md", false);
        app.start_tree_rename(file_tree::TreeNavItem::File("Note.md".into()));
        app.commit_tree_rename("Renamed".into());
        assert_eq!(vault_rels(&app), vec!["Renamed.md"], "{}", app.status);
        assert_eq!(active_rel(&app), "Renamed.md");
    }

    #[test]
    fn tree_rename_of_image_keeps_its_extension() {
        let (d, mut app) = app_with_notes(&["Note.md"]);
        std::fs::write(d.path().join("pic.png"), b"x").unwrap();
        app.start_tree_rename(file_tree::TreeNavItem::File("pic.png".into()));
        app.commit_tree_rename("photo".into());
        let root = &app.vault.as_ref().unwrap().root();
        assert!(root.join("photo.png").is_file(), "{}", app.status);
    }

    #[test]
    fn tree_rename_with_extension_is_used_as_typed() {
        let (_d, mut app) = app_with_notes(&["Note.md"]);
        app.start_tree_rename(file_tree::TreeNavItem::File("Note.md".into()));
        app.commit_tree_rename("Other.md".into());
        assert_eq!(vault_rels(&app), vec!["Other.md"]);
    }

    #[test]
    fn closing_a_tab_left_of_the_active_one_keeps_the_active_note() {
        let (_d, mut app) = app_with_notes(&["A.md", "B.md", "C.md"]);
        app.open_note("A.md", false);
        app.open_note("B.md", true);
        app.open_note("C.md", true);
        assert_eq!(active_rel(&app), "C.md");
        assert_eq!(app.former_tab, 1);
        app.close_tab(0);
        assert_eq!(active_rel(&app), "C.md");
        assert_eq!(app.tabs[app.former_tab].rel, "B.md");
    }

    #[test]
    fn closing_the_active_tab_moves_to_a_neighbour() {
        let (_d, mut app) = app_with_notes(&["A.md", "B.md", "C.md"]);
        app.open_note("A.md", false);
        app.open_note("B.md", true);
        app.open_note("C.md", true);
        app.close_tab(2);
        assert_eq!(active_rel(&app), "B.md");
        app.close_tab(0);
        assert_eq!(active_rel(&app), "B.md");
        app.close_tab(0);
        assert!(app.tabs.is_empty());
        assert_eq!(app.active_tab, 0);
    }

    #[test]
    fn back_after_delete_does_not_resurrect_the_note() {
        let (_d, mut app) = app_with_notes(&["A.md", "B.md", "C.md"]);
        app.open_note("A.md", false);
        app.open_note("B.md", true);
        app.open_note("C.md", true);
        assert_eq!(app.nav_back, vec!["A.md", "B.md"]);

        app.focus = FocusPane::Files;
        app.tree_cursor = Some(file_tree::TreeNavItem::File("B.md".into()));
        app.delete_tree_selection();
        assert_eq!(vault_rels(&app), vec!["A.md", "C.md"], "{}", app.status);
        assert!(!app.nav_back.iter().any(|r| r == "B.md"), "{:?}", app.nav_back);

        app.nav_go_back();
        assert_eq!(active_rel(&app), "A.md");
        assert_eq!(vault_rels(&app), vec!["A.md", "C.md"], "Back must not recreate B.md");
        let root = &app.vault.as_ref().unwrap().root();
        assert!(!root.join("B.md").exists());
    }

    #[test]
    fn forward_history_is_purged_on_delete_too() {
        let (_d, mut app) = app_with_notes(&["A.md", "B.md"]);
        app.open_note("A.md", false);
        app.open_note("B.md", true);
        app.nav_go_back();
        assert_eq!(app.nav_forward, vec!["B.md"]);
        app.focus = FocusPane::Files;
        app.tree_cursor = Some(file_tree::TreeNavItem::File("B.md".into()));
        app.delete_tree_selection();
        assert!(app.nav_forward.is_empty(), "{:?}", app.nav_forward);
        assert!(!app.recent_notes.iter().any(|r| r == "B.md"));
    }

    #[test]
    fn editor_heading_jump_prefers_exact_match_over_slug_twin() {
        let text = "# A B\n\ntext\n\n# A-B\n";
        let second = text.find("# A-B").unwrap();
        assert_eq!(char_offset_for_heading(text, "A-B"), Some(second));
        assert_eq!(char_offset_for_heading(text, "A B"), Some(0));
        // Slug anchors still resolve when nothing matches exactly.
        assert_eq!(char_offset_for_heading(text, "a-b"), Some(0));
    }

    #[test]
    fn heading_link_scrolls_the_current_note_instead_of_creating_dot_md() {
        let (_d, mut app) = app_with_notes(&["Home.md"]);
        app.open_note("Home.md", false);
        let ctx = egui::Context::default();
        app.follow_link(preview::LinkTarget::Heading("Section".into()), &ctx);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].scroll_to_heading.as_deref(), Some("Section"));
        assert_eq!(vault_rels(&app), vec!["Home.md"]);
    }

    #[test]
    fn note_link_with_heading_opens_and_scrolls() {
        let (_d, mut app) = app_with_notes(&["Home.md", "Other.md"]);
        app.open_note("Home.md", false);
        let ctx = egui::Context::default();
        app.follow_link(
            preview::LinkTarget::Note {
                rel: "Other.md".into(),
                heading: Some("Section".into()),
            },
            &ctx,
        );
        assert_eq!(active_rel(&app), "Other.md");
        assert_eq!(app.tabs[app.active_tab].scroll_to_heading.as_deref(), Some("Section"));
    }

    #[test]
    fn external_link_opens_in_browser_not_as_note() {
        let (_d, mut app) = app_with_notes(&["Home.md"]);
        app.open_note("Home.md", false);
        let ctx = egui::Context::default();
        app.follow_link(preview::LinkTarget::External("mailto:x@y.z".into()), &ctx);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(vault_rels(&app), vec!["Home.md"]);
        let opened = ctx.output(|o| {
            o.commands.iter().any(|c| {
                matches!(c, egui::OutputCommand::OpenUrl(u) if u.url == "mailto:x@y.z")
            })
        });
        assert!(opened, "ctx.open_url must have been called");
    }
}
