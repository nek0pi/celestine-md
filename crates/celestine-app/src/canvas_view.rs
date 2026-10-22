//! Canvas board: an infinite pan/zoom surface with a pixel dot grid, TUI-style
//! cards (text, notes, images), ink drawings, and elbow arrows between them.

use crate::canvas_ink::{self, InkRecorder, StrokeGeom};
use crate::file_tree::TreeNavItem;
use crate::media::{self, ImageResolver};
use crate::preview::{self, LinkTarget};
use crate::theme_apply::ThemePalette;
use celestine_core::canvas::{Canvas, CanvasEdge, CanvasNode, Drawing, NodeKind, Side};
use celestine_core::{extract_note_data, Vault};
use egui::scroll_area::{ScrollBarVisibility, ScrollSource};
use egui::{
    pos2, vec2, Align, Align2, Color32, CursorIcon, FontId, Key, Layout, Mesh, Modifiers,
    PointerButton, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, TextEdit, Ui,
    UiBuilder, Vec2,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const GRID: f32 = 24.0;
const MIN_ZOOM: f32 = 0.12;
const MAX_ZOOM: f32 = 4.0;
const MAX_UNDO: usize = 100;
const PAD: f32 = 12.0;
const TEXT_FONT: f32 = 14.0;
const TITLE_FONT: f32 = 12.0;
const ARROW_OUT: f32 = 20.0;
const NOTE_SIZE: Vec2 = vec2(400.0, 320.0);
const TEXT_SIZE: Vec2 = vec2(260.0, 80.0);
const INK_SIZES: [f32; 3] = [2.5, 5.0, 10.0];
const INK_COLORS: [Option<&str>; 7] = [None, Some("1"), Some("2"), Some("3"), Some("4"), Some("5"), Some("6")];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Ink,
    Text,
    Note,
    Image,
    Arrow,
    Erase,
}

impl Tool {
    const ALL: [Tool; 7] = [
        Tool::Select,
        Tool::Ink,
        Tool::Text,
        Tool::Note,
        Tool::Image,
        Tool::Arrow,
        Tool::Erase,
    ];

    fn key(self) -> Key {
        match self {
            Tool::Select => Key::V,
            Tool::Ink => Key::P,
            Tool::Text => Key::T,
            Tool::Note => Key::N,
            Tool::Image => Key::I,
            Tool::Arrow => Key::A,
            Tool::Erase => Key::E,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Tool::Select => "[V] select",
            Tool::Ink => "[P] ink",
            Tool::Text => "[T] text",
            Tool::Note => "[N] note",
            Tool::Image => "[I] image",
            Tool::Arrow => "[A] arrow",
            Tool::Erase => "[E] erase",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Tool::Select => "drag cards · dbl-click edit/open · drag empty to pan",
            Tool::Ink => "draw with the nib · strokes smooth on release",
            Tool::Text => "click to place a text card",
            Tool::Note => "click to create a note here",
            Tool::Image => "click to pick images (or drop files anywhere)",
            Tool::Arrow => "drag from one object to another",
            Tool::Erase => "drag over ink to erase it",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum Sel {
    Object(String),
    Edge(String),
}

enum Drag {
    None,
    Pan,
    Move(Vec<String>),
    Resize { id: String, aspect: Option<f32> },
    Ink(InkRecorder),
    Arrow { from: String, to: Pos2 },
    Erase { checkpointed: bool },
}

struct NewNotePrompt {
    at: Pos2,
    folder: String,
    name: String,
    focus: bool,
}

#[derive(Clone)]
enum MenuAction {
    NewNote,
    Text,
    Image,
    Delete(Sel),
    Color(String, Option<String>),
    Open(String),
    Fit,
}

/// What the board needs from the app for one frame.
pub struct BoardEnv<'a> {
    pub vault: &'a mut Vault,
    pub canvas_rel: &'a str,
    pub palette: &'a ThemePalette,
    pub image_rels: &'a [String],
    pub image_resolver: &'a mut ImageResolver,
    pub font_size: f32,
    pub keys_enabled: bool,
}

#[derive(Default)]
pub struct BoardOutcome {
    pub changed: bool,
    pub open: Option<String>,
    pub files_changed: bool,
    pub status: Option<String>,
}

pub struct CanvasBoard {
    pub doc: Canvas,
    pan: Vec2,
    zoom: f32,
    tool: Tool,
    selection: Option<Sel>,
    drag: Drag,
    editing_text: Option<String>,
    editing_label: Option<String>,
    edit_focus_pending: bool,
    ink_size: f32,
    ink_color: Option<String>,
    undo: Vec<Canvas>,
    redo: Vec<Canvas>,
    new_note: Option<NewNotePrompt>,
    menu_at: Pos2,
    menu_target: Option<Sel>,
    geom: HashMap<String, (usize, f32, StrokeGeom)>,
    fitted: bool,
    /// Screen rect of the board last frame.
    view_rect: Rect,
}

impl CanvasBoard {
    pub fn new(doc: Canvas) -> Self {
        Self {
            doc,
            pan: Vec2::ZERO,
            zoom: 1.0,
            tool: Tool::Select,
            selection: None,
            drag: Drag::None,
            editing_text: None,
            editing_label: None,
            edit_focus_pending: false,
            ink_size: INK_SIZES[1],
            ink_color: None,
            undo: Vec::new(),
            redo: Vec::new(),
            new_note: None,
            menu_at: Pos2::ZERO,
            menu_target: None,
            geom: HashMap::new(),
            fitted: false,
            view_rect: Rect::NOTHING,
        }
    }

    pub fn tool_hint(&self) -> String {
        format!(
            "canvas · {} · scroll pan · Ctrl+scroll zoom · Mod+Z undo",
            self.tool.hint()
        )
    }

    /// Escape: close prompts, stop editing, then drop back to select.
    pub fn escape(&mut self) -> bool {
        if self.new_note.take().is_some() {
            return true;
        }
        if self.editing_text.take().is_some() || self.editing_label.take().is_some() {
            return true;
        }
        if self.tool != Tool::Select {
            self.tool = Tool::Select;
            return true;
        }
        self.selection.take().is_some()
    }

    pub fn delete_selection(&mut self) -> bool {
        let Some(sel) = self.selection.take() else {
            return false;
        };
        self.checkpoint();
        match sel {
            Sel::Object(id) => self.doc.remove_object(&id),
            Sel::Edge(id) => self.doc.remove_edge(&id),
        }
        true
    }

    fn checkpoint(&mut self) {
        self.undo.push(self.doc.clone());
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else {
            return false;
        };
        self.redo.push(std::mem::replace(&mut self.doc, prev));
        self.after_history();
        true
    }

    fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(std::mem::replace(&mut self.doc, next));
        self.after_history();
        true
    }

    fn after_history(&mut self) {
        self.editing_text = None;
        self.editing_label = None;
        self.selection = None;
        self.drag = Drag::None;
    }

    fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        self.editing_text = None;
        self.editing_label = None;
    }

    // ── coordinates ────────────────────────────────────────────────

    fn to_screen(&self, rect: Rect, p: Pos2) -> Pos2 {
        rect.center() + self.pan + p.to_vec2() * self.zoom
    }

    fn to_world(&self, rect: Rect, s: Pos2) -> Pos2 {
        ((s - rect.center() - self.pan) / self.zoom).to_pos2()
    }

    fn rect_to_screen(&self, rect: Rect, r: Rect) -> Rect {
        Rect::from_min_max(self.to_screen(rect, r.min), self.to_screen(rect, r.max))
    }

    fn zoom_at(&mut self, rect: Rect, screen: Pos2, factor: f32) {
        let world = self.to_world(rect, screen);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.pan = screen - rect.center() - world.to_vec2() * self.zoom;
    }

    fn fit(&mut self, rect: Rect) {
        let Some(bounds) = content_bounds(&self.doc) else {
            self.pan = Vec2::ZERO;
            self.zoom = 1.0;
            return;
        };
        let avail = rect.shrink(48.0).size().max(vec2(1.0, 1.0));
        let z = (avail.x / bounds.width().max(1.0))
            .min(avail.y / bounds.height().max(1.0))
            .clamp(0.25, 1.0);
        self.zoom = z;
        self.pan = -bounds.center().to_vec2() * z;
    }

    // ── geometry / hit testing ─────────────────────────────────────

    fn object_rect(&self, id: &str) -> Option<Rect> {
        object_rect(&self.doc, id)
    }

    fn hit_object(&self, p: Pos2, tol: f32) -> Option<String> {
        for d in self.doc.celestine.drawings.iter().rev() {
            if canvas_ink::distance_to(&d.points, p) <= d.size * 0.5 + tol {
                return Some(d.id.clone());
            }
        }
        let hit = |n: &&CanvasNode| node_rect(n).expand(tol * 0.5).contains(p);
        self.doc
            .nodes
            .iter()
            .rev()
            .filter(|n| n.node_kind() != NodeKind::Group)
            .find(hit)
            .or_else(|| {
                self.doc
                    .nodes
                    .iter()
                    .rev()
                    .filter(|n| n.node_kind() == NodeKind::Group)
                    .find(hit)
            })
            .map(|n| n.id.clone())
    }

    fn hit_edge(&self, p: Pos2, tol: f32) -> Option<String> {
        self.doc.all_edges().find_map(|e| {
            let pts = edge_route(&self.doc, e)?;
            pts.windows(2)
                .any(|w| canvas_ink::segment_distance(w[0], w[1], p) <= tol)
                .then(|| e.id.clone())
        })
    }

    /// Objects moved together: the object, plus everything inside a group.
    fn move_set(&self, id: &str) -> Vec<String> {
        let mut ids = vec![id.to_string()];
        let Some(group) = self
            .doc
            .nodes
            .iter()
            .find(|n| n.id == id && n.node_kind() == NodeKind::Group)
        else {
            return ids;
        };
        let g = node_rect(group);
        for n in &self.doc.nodes {
            if n.id != id && g.contains_rect(node_rect(n)) {
                ids.push(n.id.clone());
            }
        }
        for d in &self.doc.celestine.drawings {
            if d.bounds().is_some_and(|b| g.contains_rect(bounds_rect(b))) {
                ids.push(d.id.clone());
            }
        }
        ids
    }

    fn translate(&mut self, ids: &[String], d: Vec2) {
        let set: HashSet<&str> = ids.iter().map(String::as_str).collect();
        for n in &mut self.doc.nodes {
            if set.contains(n.id.as_str()) {
                n.x += d.x;
                n.y += d.y;
            }
        }
        for dr in &mut self.doc.celestine.drawings {
            if set.contains(dr.id.as_str()) {
                dr.translate(d.x, d.y);
            }
        }
        self.refresh_sides();
    }

    /// Re-anchor every edge on the facing sides of its endpoints.
    fn refresh_sides(&mut self) {
        let rects: HashMap<String, Rect> = self
            .doc
            .nodes
            .iter()
            .map(|n| (n.id.clone(), node_rect(n)))
            .chain(
                self.doc
                    .celestine
                    .drawings
                    .iter()
                    .filter_map(|d| Some((d.id.clone(), bounds_rect(d.bounds()?)))),
            )
            .collect();
        for e in self
            .doc
            .edges
            .iter_mut()
            .chain(self.doc.celestine.edges.iter_mut())
        {
            if let (Some(a), Some(b)) = (rects.get(&e.from_node), rects.get(&e.to_node)) {
                let (sa, sb) = auto_sides(*a, *b);
                e.from_side = Some(sa);
                e.to_side = Some(sb);
            }
        }
    }

    fn add_node(&mut self, node: CanvasNode) -> String {
        let id = node.id.clone();
        self.doc.nodes.push(node);
        self.selection = Some(Sel::Object(id.clone()));
        id
    }

    fn add_text_at(&mut self, at: Pos2) {
        self.checkpoint();
        let min = at - TEXT_SIZE * 0.5;
        let id = self.add_node(CanvasNode::new_text(
            min.x, min.y, TEXT_SIZE.x, TEXT_SIZE.y, "",
        ));
        self.editing_text = Some(id);
        self.edit_focus_pending = true;
        self.tool = Tool::Select;
    }

    /// Add a card for a vault file (note, image, or anything else) centred on `at`.
    fn add_file_card(&mut self, vault: &Vault, rel: &str, at: Pos2) {
        let size = if rel.to_lowercase().ends_with(".md") {
            NOTE_SIZE
        } else if media::is_image_rel(rel) {
            vault
                .abs_path(rel)
                .ok()
                .and_then(|p| image::image_dimensions(p).ok())
                .map(|(w, h)| {
                    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
                    let s = (480.0 / w).min(400.0 / h).min(1.0);
                    vec2((w * s).max(80.0), (h * s).max(60.0))
                })
                .unwrap_or(vec2(400.0, 300.0))
        } else {
            vec2(300.0, 110.0)
        };
        self.checkpoint();
        let min = at - size * 0.5;
        self.add_node(CanvasNode::new_file(min.x, min.y, size.x, size.y, rel));
    }

    /// Bring an outside file into the vault (images only) or reuse an in-vault path.
    fn import_path(&mut self, env: &mut BoardEnv<'_>, path: &Path, at: Pos2) -> Result<String, String> {
        if let Ok(rel) = env.vault.rel_of(path) {
            self.add_file_card(env.vault, &rel, at);
            return Ok(rel);
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !media::is_image_ext(&ext) {
            return Err(format!("only images can be imported ({})", path.display()));
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "image".into());
        let (rel, abs) = attachment_dest(env, &stem, &ext)?;
        std::fs::copy(path, &abs).map_err(|e| e.to_string())?;
        self.add_file_card(env.vault, &rel, at);
        Ok(rel)
    }

    /// Ctrl+V on the board: clipboard images become image cards, copied files
    /// (`file://` lists) are imported, and other text becomes a text card.
    fn handle_paste(&mut self, ctx: &egui::Context, rect: Rect, env: &mut BoardEnv<'_>, out: &mut BoardOutcome) {
        if !env.keys_enabled || self.new_note.is_some() || ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        let pasted: Vec<egui::Event> = ctx.input(|i| {
            i.events
                .iter()
                .filter(|e| matches!(e, egui::Event::Paste(_) | egui::Event::PasteImage(_)))
                .cloned()
                .collect()
        });
        if pasted.is_empty() {
            return;
        }
        let pointer = ctx.input(|i| i.pointer.hover_pos()).filter(|p| rect.contains(*p));
        let at = self.to_world(rect, pointer.unwrap_or(rect.center()));
        for event in pasted {
            match event {
                egui::Event::PasteImage(image) => match save_pasted_image(env, &image) {
                    Ok(rel) => {
                        self.add_file_card(env.vault, &rel, at);
                        out.changed = true;
                        out.files_changed = true;
                        out.status = Some(format!("pasted image → {rel}"));
                    }
                    Err(e) => out.status = Some(format!("paste failed: {e}")),
                },
                egui::Event::Paste(text) => {
                    if let Some(paths) = pasted_file_paths(&text) {
                        self.import_many(env, paths, at, out);
                    } else if !text.trim().is_empty() {
                        self.checkpoint();
                        let lines = text.lines().count().clamp(1, 30) as f32;
                        let size = vec2(320.0, PAD * 2.0 + lines * TEXT_FONT * 1.4);
                        let min = at - size * 0.5;
                        self.add_node(CanvasNode::new_text(min.x, min.y, size.x, size.y, &text));
                        out.changed = true;
                    }
                }
                _ => {}
            }
        }
    }

    fn import_many(
        &mut self,
        env: &mut BoardEnv<'_>,
        paths: Vec<PathBuf>,
        at: Pos2,
        out: &mut BoardOutcome,
    ) {
        let mut added = 0;
        for (i, path) in paths.iter().enumerate() {
            let offset = vec2(32.0, 32.0) * i as f32;
            match self.import_path(env, path, at + offset) {
                Ok(_) => added += 1,
                Err(e) => out.status = Some(e),
            }
        }
        if added > 0 {
            out.changed = true;
            out.files_changed = true;
            out.status.get_or_insert_with(|| format!("added {added} item(s) to canvas"));
        }
    }

    fn pick_images(&mut self, env: &mut BoardEnv<'_>, at: Pos2, out: &mut BoardOutcome) {
        let exts = ["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp"];
        let mut dialog = rfd::FileDialog::new().add_filter("images", &exts);
        if let Ok(dir) = env.vault.abs_path(
            Path::new(env.canvas_rel)
                .parent()
                .and_then(|p| p.to_str())
                .unwrap_or(""),
        ) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(paths) = dialog.pick_files() {
            self.import_many(env, paths, at, out);
        }
        self.tool = Tool::Select;
    }

    fn open_new_note_prompt(&mut self, at: Pos2, canvas_rel: &str) {
        let folder = Path::new(canvas_rel)
            .parent()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        self.new_note = Some(NewNotePrompt {
            at,
            folder,
            name: String::new(),
            focus: true,
        });
        self.tool = Tool::Select;
    }

    // ── frame ──────────────────────────────────────────────────────

    pub fn show(&mut self, ui: &mut Ui, env: &mut BoardEnv<'_>) -> BoardOutcome {
        let mut out = BoardOutcome::default();
        let p = env.palette;
        self.toolbar(ui, p, &mut out);

        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, Sense::hover());
        self.view_rect = rect;
        let mut board = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("canvas-board"));
        board.set_clip_rect(rect.intersect(ui.clip_rect()));
        let ui = &mut board;

        if !self.fitted {
            self.fit(rect);
            self.fitted = true;
        }
        let ctx = ui.ctx().clone();
        let painter = ui.painter().clone();

        painter.rect_filled(rect, 0.0, p.content_bg);
        paint_dot_grid(&painter, rect, self.pan, self.zoom, p);

        // Groups, then edges, then cards, then ink on top.
        for n in self.doc.nodes.iter().filter(|n| n.node_kind() == NodeKind::Group) {
            self.paint_group(&painter, rect, n, p);
        }
        self.paint_edges(&painter, rect, p);
        let selected_id = match &self.selection {
            Some(Sel::Object(id)) => Some(id.clone()),
            _ => None,
        };
        let live_card = selected_id
            .clone()
            .filter(|id| self.is_note_card(id))
            .or_else(|| self.editing_text.clone());
        let nodes: Vec<CanvasNode> = self
            .doc
            .nodes
            .iter()
            .filter(|n| n.node_kind() != NodeKind::Group)
            .cloned()
            .collect();
        for n in &nodes {
            let live = live_card.as_deref() == Some(n.id.as_str());
            self.paint_card(ui, &painter, rect, n, env, live, &mut out);
        }
        self.paint_drawings(&painter, rect, p);
        self.paint_pending(&painter, rect, p);

        let board_id = ui.scope_id().with("canvas-interact");
        let resp = ui.interact(rect, board_id, Sense::click_and_drag());

        // Live content sits above the board interaction so it can take input.
        let mut live_rect = None;
        if let Some(id) = &live_card {
            if let Some(n) = self.doc.nodes.iter().find(|n| &n.id == id).cloned() {
                let r = self.rect_to_screen(rect, node_rect(&n));
                live_rect = Some(card_content_rect(r, self.zoom));
                if self.editing_text.as_deref() == Some(id.as_str()) {
                    self.text_editor(ui, rect, &n, p, &mut out);
                } else {
                    self.note_preview(ui, card_content_rect(r, self.zoom), &n, env, true, &mut out);
                }
            }
        }
        self.label_editor(ui, rect, p, &mut out);
        self.paint_selection(&painter, rect, p);

        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let over_live = pointer.is_some_and(|ptr| live_rect.is_some_and(|r| r.contains(ptr)));
        self.handle_view_input(&ctx, rect, &resp, over_live);
        self.handle_pointer(&ctx, rect, &resp, env, &mut out);
        self.handle_keys(&ctx, env.keys_enabled, &mut out);
        self.context_menu(&resp, rect, env, &mut out);
        self.handle_drops(&ctx, &painter, rect, &resp, env, p, &mut out);
        self.handle_paste(&ctx, rect, env, &mut out);
        self.new_note_prompt(&ctx, rect, env, p, &mut out);
        self.set_cursor(&ctx, rect, &resp);
        out
    }

    fn is_note_card(&self, id: &str) -> bool {
        self.doc.nodes.iter().any(|n| {
            n.id == id
                && n.node_kind() == NodeKind::File
                && n.file.as_deref().is_some_and(|f| f.to_lowercase().ends_with(".md"))
        })
    }

    fn toolbar(&mut self, ui: &mut Ui, p: &ThemePalette, out: &mut BoardOutcome) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for tool in Tool::ALL {
                let active = self.tool == tool;
                let text = RichText::new(tool.label())
                    .monospace()
                    .size(12.0)
                    .color(if active { p.content_bg } else { p.muted });
                let button = egui::Button::new(text)
                    .fill(if active { p.accent } else { Color32::TRANSPARENT })
                    .stroke(Stroke::NONE)
                    .corner_radius(0.0);
                if ui.add(button).on_hover_text(tool.hint()).clicked() {
                    self.set_tool(tool);
                }
            }
            if self.tool == Tool::Ink {
                ui.label(RichText::new(" │ ").monospace().color(p.border));
                for c in INK_COLORS {
                    let color = c.and_then(|c| preset_color(p, c)).unwrap_or(p.text);
                    let active = self.ink_color.as_deref() == c;
                    let glyph = if active { "▣" } else { "■" };
                    let b = egui::Button::new(RichText::new(glyph).monospace().size(14.0).color(color))
                        .frame(false);
                    if ui.add(b).clicked() {
                        self.ink_color = c.map(str::to_string);
                    }
                }
                ui.label(RichText::new(" │ ").monospace().color(p.border));
                for (i, s) in INK_SIZES.iter().enumerate() {
                    let active = (self.ink_size - s).abs() < 0.01;
                    let label = ["S", "M", "L"][i];
                    let text = RichText::new(label)
                        .monospace()
                        .size(12.0)
                        .color(if active { p.content_bg } else { p.muted });
                    let b = egui::Button::new(text)
                        .fill(if active { p.accent } else { Color32::TRANSPARENT })
                        .stroke(Stroke::NONE)
                        .corner_radius(0.0);
                    if ui.add(b).clicked() {
                        self.ink_size = *s;
                    }
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let small = |s: &str| RichText::new(s).monospace().size(12.0).color(p.muted);
                if ui.add(egui::Button::new(small("[redo]")).frame(false)).clicked() && self.redo() {
                    out.changed = true;
                }
                if ui.add(egui::Button::new(small("[undo]")).frame(false)).clicked() && self.undo() {
                    out.changed = true;
                }
                if ui.add(egui::Button::new(small("[fit]")).frame(false)).clicked() {
                    self.fitted = false;
                }
                ui.label(small(&format!("{:>3.0}%", self.zoom * 100.0)));
            });
        });
        ui.add_space(2.0);
    }

    // ── painting ───────────────────────────────────────────────────

    fn paint_group(&self, painter: &egui::Painter, rect: Rect, n: &CanvasNode, p: &ThemePalette) {
        let r = self.rect_to_screen(rect, node_rect(n));
        let c = node_accent(n, p).unwrap_or(p.border);
        painter.rect_filled(r, 0.0, c.gamma_multiply(0.06));
        dashed_rect(painter, r, Stroke::new(1.0, c.gamma_multiply(0.8)), 6.0, 4.0);
        let label = n.label.clone().unwrap_or_else(|| "group".into());
        tui_title(painter, r, &label, self.zoom, c, p.content_bg);
    }

    fn paint_edges(&self, painter: &egui::Painter, rect: Rect, p: &ThemePalette) {
        for e in self.doc.all_edges() {
            let Some(route) = edge_route(&self.doc, e) else {
                continue;
            };
            let pts: Vec<Pos2> = route.iter().map(|w| crisp(self.to_screen(rect, *w))).collect();
            let selected = self.selection == Some(Sel::Edge(e.id.clone()));
            let color = e
                .color
                .as_deref()
                .and_then(|c| preset_color(p, c))
                .unwrap_or(p.muted);
            let color = if selected { p.accent } else { color };
            let width = if selected { 2.0 } else { 1.0 };
            painter.add(Shape::line(pts.clone(), Stroke::new(width, color)));
            let n = pts.len();
            if e.arrow_at_end() && n >= 2 {
                arrow_head(painter, pts[n - 1], pts[n - 2], self.zoom, color);
            }
            if e.arrow_at_start() && n >= 2 {
                arrow_head(painter, pts[0], pts[1], self.zoom, color);
            }
            let editing = self.editing_label.as_deref() == Some(e.id.as_str());
            if let Some(label) = e.label.as_deref().filter(|l| !l.is_empty() && !editing) {
                let at = route_mid(&pts);
                let font = FontId::monospace((TITLE_FONT * self.zoom).clamp(6.0, 28.0));
                let galley = painter.layout_no_wrap(label.to_string(), font, p.text);
                let box_r = Rect::from_center_size(at, galley.size() + vec2(10.0, 4.0));
                painter.rect_filled(box_r, 0.0, p.content_bg);
                painter.rect_stroke(box_r, 0.0, Stroke::new(1.0, color), StrokeKind::Inside);
                painter.galley(box_r.center() - galley.size() * 0.5, galley, p.text);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_card(
        &self,
        ui: &mut Ui,
        painter: &egui::Painter,
        rect: Rect,
        n: &CanvasNode,
        env: &mut BoardEnv<'_>,
        live: bool,
        out: &mut BoardOutcome,
    ) {
        let p = env.palette;
        let r = self.rect_to_screen(rect, node_rect(n));
        if !r.intersects(rect) {
            return;
        }
        let accent = node_accent(n, p);
        let border = accent.unwrap_or(p.border);
        // Hard pixel drop-shadow, no blur.
        let shadow = (3.0 * self.zoom).clamp(1.0, 6.0).round();
        painter.rect_filled(r.translate(vec2(shadow, shadow)), 0.0, Color32::from_black_alpha(70));
        painter.rect_filled(r, 0.0, p.panel_bg);
        painter.rect_stroke(r, 0.0, Stroke::new(1.0, border), StrokeKind::Inside);
        let content = card_content_rect(r, self.zoom);
        let title_color = accent.unwrap_or(p.accent);

        match n.node_kind() {
            NodeKind::Text => {
                if self.editing_text.as_deref() != Some(n.id.as_str()) {
                    let text = n.text.clone().unwrap_or_default();
                    let size = TEXT_FONT * self.zoom;
                    if size >= 3.0 {
                        let color = accent.unwrap_or(p.text);
                        let galley = painter.layout(
                            if text.is_empty() { "…".into() } else { text },
                            FontId::monospace(size),
                            color,
                            content.width().max(1.0),
                        );
                        painter.with_clip_rect(content.intersect(rect)).galley(content.min, galley, color);
                    }
                }
                tui_corner_marks(painter, r, border);
            }
            NodeKind::File => {
                let file = n.file.clone().unwrap_or_default();
                let name = Path::new(&file)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| file.clone());
                if media::is_image_rel(&file) {
                    let inner = r.shrink(1.0);
                    match env.vault.abs_path(&file) {
                        Ok(abs) if abs.is_file() => {
                            egui::Image::new(media::path_to_file_uri(&abs)).paint_at(ui, inner);
                        }
                        _ => missing(painter, content, "missing image", p.muted, self.zoom),
                    }
                    tui_title(painter, r, &name, self.zoom, title_color, p.panel_bg);
                } else if file.to_lowercase().ends_with(".md") {
                    let stem = Path::new(&file)
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or(name);
                    if !live {
                        self.note_preview(ui, content, n, env, false, out);
                    }
                    tui_title(painter, r, &stem, self.zoom, title_color, p.panel_bg);
                    if live {
                        open_tag(painter, r, self.zoom, title_color, p.panel_bg);
                    }
                } else {
                    tui_title(painter, r, &name, self.zoom, title_color, p.panel_bg);
                    missing(painter, content, "file · dbl-click to open", p.muted, self.zoom);
                }
            }
            NodeKind::Link => {
                let url = n.url.clone().unwrap_or_default();
                tui_title(painter, r, "link", self.zoom, title_color, p.panel_bg);
                missing(painter, content, &format!("↗ {url}"), p.text, self.zoom);
            }
            NodeKind::Group | NodeKind::Other => {
                tui_title(painter, r, &n.kind, self.zoom, title_color, p.panel_bg);
            }
        }
    }

    /// Render a note's Markdown into a card. Live cards scroll and take clicks.
    fn note_preview(
        &self,
        ui: &mut Ui,
        content: Rect,
        n: &CanvasNode,
        env: &mut BoardEnv<'_>,
        live: bool,
        out: &mut BoardOutcome,
    ) {
        let file = n.file.clone().unwrap_or_default();
        let p = env.palette;
        let Some(meta) = env.vault.index.get_by_rel(&file) else {
            missing(ui.painter(), content, "missing note", p.muted, self.zoom);
            return;
        };
        let font = (env.font_size * 0.8 * self.zoom).clamp(2.0, 48.0);
        if font < 5.0 {
            // Too small to read: draw placeholder bars like a minimap.
            let mut y = content.top();
            for (i, line) in meta.content.lines().filter(|l| !l.trim().is_empty()).take(24).enumerate() {
                let h = (font * 1.1).max(1.0);
                if y + h > content.bottom() {
                    break;
                }
                let w = (line.chars().count() as f32 * font * 0.6).min(content.width());
                let c = if i == 0 { p.heading[0] } else { p.muted };
                ui.painter().rect_filled(
                    Rect::from_min_size(pos2(content.left(), y), vec2(w, h * 0.6)),
                    0.0,
                    c.gamma_multiply(0.5),
                );
                y += h * 1.4;
            }
            return;
        }
        let body = extract_note_data(&meta.content).body;
        let clip = content.intersect(ui.clip_rect());
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(content)
                .layout(Layout::top_down(Align::Min))
                .id_salt(("canvas-card", &n.id)),
        );
        child.set_clip_rect(clip);
        let source = if live {
            ScrollSource::SCROLL_BAR | ScrollSource::MOUSE_WHEEL
        } else {
            ScrollSource::NONE
        };
        let outcome = egui::ScrollArea::vertical()
            .id_salt(("canvas-card-scroll", &n.id))
            .auto_shrink([false, false])
            .scroll_source(source)
            .scroll_bar_visibility(if live {
                ScrollBarVisibility::VisibleWhenNeeded
            } else {
                ScrollBarVisibility::AlwaysHidden
            })
            .show(&mut child, |ui| {
                ui.set_max_width(content.width());
                preview::show_preview(
                    ui,
                    &body,
                    &file,
                    env.vault.root(),
                    &env.vault.index,
                    env.image_rels,
                    p.accent,
                    p.muted,
                    p.text,
                    p.border,
                    font,
                    None,
                    None,
                    &p.heading,
                    env.image_resolver,
                )
            })
            .inner;
        if live {
            match outcome.link {
                Some(LinkTarget::Note { rel, .. }) | Some(LinkTarget::Image(rel)) => {
                    out.open = Some(rel)
                }
                Some(LinkTarget::Heading(_)) => out.open = Some(file),
                Some(LinkTarget::External(url)) => {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                }
                None => {}
            }
        }
    }

    fn paint_drawings(&mut self, painter: &egui::Painter, rect: Rect, p: &ThemePalette) {
        let live: HashSet<&str> = self.doc.celestine.drawings.iter().map(|d| d.id.as_str()).collect();
        self.geom.retain(|id, _| live.contains(id.as_str()));
        let zoom = self.zoom;
        let (pan, center) = (self.pan, rect.center());
        let to_screen = |w: Pos2| center + pan + w.to_vec2() * zoom;
        for d in &self.doc.celestine.drawings {
            if let Some(b) = d.bounds() {
                let sr = Rect::from_min_max(to_screen(pos2(b.0, b.1)), to_screen(pos2(b.2, b.3)));
                if !sr.intersects(rect) {
                    continue;
                }
            }
            let entry = self.geom.entry(d.id.clone()).or_insert_with(|| {
                (d.points.len(), d.size, StrokeGeom::new(&d.points, d.size))
            });
            if entry.0 != d.points.len() || entry.1 != d.size {
                *entry = (d.points.len(), d.size, StrokeGeom::new(&d.points, d.size));
            }
            let color = d.color.as_deref().and_then(|c| preset_color(p, c)).unwrap_or(p.text);
            painter.add(canvas_ink::tessellate(&d.points, &entry.2, zoom, color, to_screen));
        }
    }

    /// The stroke being drawn and the arrow being dragged out.
    fn paint_pending(&self, painter: &egui::Painter, rect: Rect, p: &ThemePalette) {
        match &self.drag {
            Drag::Ink(rec) if !rec.points().is_empty() => {
                let pts = canvas_ink::smooth(rec.points());
                let geom = StrokeGeom::new(&pts, self.ink_size);
                let color = self
                    .ink_color
                    .as_deref()
                    .and_then(|c| preset_color(p, c))
                    .unwrap_or(p.text);
                painter.add(canvas_ink::tessellate(&pts, &geom, self.zoom, color, |w| {
                    self.to_screen(rect, w)
                }));
            }
            Drag::Arrow { from, to } => {
                if let Some(a) = self.object_rect(from) {
                    let target = Rect::from_center_size(*to, Vec2::splat(1.0));
                    let (sa, sb) = auto_sides(a, target);
                    let route: Vec<Pos2> = elbow(a, sa, target, sb, 0.0)
                        .into_iter()
                        .map(|w| crisp(self.to_screen(rect, w)))
                        .collect();
                    for seg in route.windows(2) {
                        painter.extend(Shape::dashed_line(seg, Stroke::new(1.0, p.accent), 5.0, 3.0));
                    }
                    if let [.., a2, b2] = route.as_slice() {
                        arrow_head(painter, *b2, *a2, self.zoom, p.accent);
                    }
                }
            }
            _ => {}
        }
    }

    fn paint_selection(&self, painter: &egui::Painter, rect: Rect, p: &ThemePalette) {
        let Some(Sel::Object(id)) = &self.selection else {
            return;
        };
        let Some(r) = self.object_rect(id) else {
            return;
        };
        let r = crisp_rect(self.rect_to_screen(rect, r).expand(4.0));
        dashed_rect(painter, r, Stroke::new(1.0, p.accent), 4.0, 3.0);
        if self.doc.nodes.iter().any(|n| &n.id == id) {
            for c in [r.left_top(), r.right_top(), r.left_bottom(), r.right_bottom()] {
                let h = Rect::from_center_size(c, Vec2::splat(5.0));
                painter.rect_filled(h, 0.0, p.accent);
            }
        }
    }

    // ── editors ────────────────────────────────────────────────────

    fn text_editor(&mut self, ui: &mut Ui, rect: Rect, n: &CanvasNode, p: &ThemePalette, out: &mut BoardOutcome) {
        let r = self.rect_to_screen(rect, node_rect(n));
        let content = card_content_rect(r, self.zoom);
        let mut text = n.text.clone().unwrap_or_default();
        let color = node_accent(n, p).unwrap_or(p.text);
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(content)
                .layout(Layout::top_down(Align::Min))
                .id_salt(("canvas-text-edit", &n.id)),
        );
        let resp = child.add(
            TextEdit::multiline(&mut text)
                .font(FontId::monospace((TEXT_FONT * self.zoom).max(4.0)))
                .text_color(color)
                .frame(egui::Frame::NONE)
                .desired_width(content.width())
                .desired_rows(1),
        );
        if self.edit_focus_pending {
            resp.request_focus();
            self.edit_focus_pending = false;
        }
        if resp.changed() {
            let need_h = (resp.rect.height() / self.zoom) + PAD * 2.0;
            if let Some(node) = self.doc.nodes.iter_mut().find(|m| m.id == n.id) {
                node.text = Some(text.clone());
                if need_h > node.height {
                    node.height = need_h;
                }
            }
            out.changed = true;
        }
        if resp.lost_focus() {
            self.editing_text = None;
            if text.trim().is_empty() {
                self.doc.remove_object(&n.id);
                self.selection = None;
                out.changed = true;
            }
        }
    }

    fn label_editor(&mut self, ui: &mut Ui, rect: Rect, p: &ThemePalette, out: &mut BoardOutcome) {
        let Some(id) = self.editing_label.clone() else {
            return;
        };
        let Some(edge) = self.doc.all_edges().find(|e| e.id == id).cloned() else {
            self.editing_label = None;
            return;
        };
        let Some(route) = edge_route(&self.doc, &edge) else {
            return;
        };
        let pts: Vec<Pos2> = route.iter().map(|w| self.to_screen(rect, *w)).collect();
        let at = route_mid(&pts);
        let mut label = edge.label.clone().unwrap_or_default();
        let r = Rect::from_center_size(at, vec2(160.0, 22.0));
        ui.painter().rect_filled(r, 0.0, p.content_bg);
        ui.painter().rect_stroke(r, 0.0, Stroke::new(1.0, p.accent), StrokeKind::Inside);
        let mut child = ui.new_child(UiBuilder::new().max_rect(r.shrink(3.0)).id_salt(("canvas-label", &id)));
        let resp = child.add(
            TextEdit::singleline(&mut label)
                .font(FontId::monospace(TITLE_FONT))
                .frame(egui::Frame::NONE)
                .hint_text("label")
                .desired_width(r.width() - 6.0),
        );
        if self.edit_focus_pending {
            resp.request_focus();
            self.edit_focus_pending = false;
        }
        if resp.changed() {
            for e in self
                .doc
                .edges
                .iter_mut()
                .chain(self.doc.celestine.edges.iter_mut())
                .filter(|e| e.id == id)
            {
                e.label = (!label.is_empty()).then(|| label.clone());
            }
            out.changed = true;
        }
        if resp.lost_focus() {
            self.editing_label = None;
        }
    }

    // ── input ──────────────────────────────────────────────────────

    fn handle_view_input(&mut self, ctx: &egui::Context, rect: Rect, resp: &egui::Response, over_live: bool) {
        if resp.dragged_by(PointerButton::Middle) {
            self.pan += resp.drag_delta();
        }
        if !resp.contains_pointer() || over_live {
            return;
        }
        let (zoom, scroll, pointer) =
            ctx.input(|i| (i.zoom_delta(), i.smooth_scroll_delta, i.pointer.hover_pos()));
        if let Some(ptr) = pointer {
            if (zoom - 1.0).abs() > 1e-4 {
                self.zoom_at(rect, ptr, zoom);
            }
        }
        if scroll != Vec2::ZERO {
            self.pan += scroll;
            ctx.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
        }
    }

    fn handle_pointer(
        &mut self,
        ctx: &egui::Context,
        rect: Rect,
        resp: &egui::Response,
        env: &mut BoardEnv<'_>,
        out: &mut BoardOutcome,
    ) {
        let tol = 6.0 / self.zoom;
        let time = ctx.input(|i| i.time);

        if resp.drag_started_by(PointerButton::Primary) {
            let origin = ctx
                .input(|i| i.pointer.press_origin())
                .unwrap_or(rect.center());
            let o = self.to_world(rect, origin);
            self.editing_text = None;
            self.editing_label = None;
            self.drag = match self.tool {
                Tool::Ink => {
                    let mut rec = InkRecorder::default();
                    rec.push(o, time);
                    Drag::Ink(rec)
                }
                Tool::Erase => {
                    let mut d = Drag::Erase { checkpointed: false };
                    self.erase_at(o, tol, &mut d, out);
                    d
                }
                Tool::Arrow => match self.hit_object(o, tol) {
                    Some(from) => Drag::Arrow { from, to: o },
                    None => Drag::Pan,
                },
                _ => {
                    if let Some(id) = self.resize_handle_hit(rect, origin) {
                        self.checkpoint();
                        let aspect = self.doc.nodes.iter().find(|n| n.id == id).and_then(|n| {
                            let image = n.file.as_deref().is_some_and(media::is_image_rel);
                            image.then(|| n.width / n.height.max(1.0))
                        });
                        Drag::Resize { id, aspect }
                    } else if let Some(id) = self.hit_object(o, tol) {
                        self.selection = Some(Sel::Object(id.clone()));
                        self.checkpoint();
                        Drag::Move(self.move_set(&id))
                    } else {
                        self.selection = None;
                        Drag::Pan
                    }
                }
            };
        }

        if resp.dragged_by(PointerButton::Primary) {
            let delta = resp.drag_delta();
            let world_ptr = resp.interact_pointer_pos().map(|s| self.to_world(rect, s));
            let mut drag = std::mem::replace(&mut self.drag, Drag::None);
            match &mut drag {
                Drag::Pan => self.pan += delta,
                Drag::Move(ids) => {
                    if delta != Vec2::ZERO {
                        let ids = ids.clone();
                        self.translate(&ids, delta / self.zoom);
                        out.changed = true;
                    }
                }
                Drag::Resize { id, aspect } => {
                    if let Some(n) = self.doc.nodes.iter_mut().find(|n| &n.id == id) {
                        n.width = (n.width + delta.x / self.zoom).max(60.0);
                        n.height = match aspect {
                            Some(a) => n.width / *a,
                            None => (n.height + delta.y / self.zoom).max(40.0),
                        };
                        out.changed = true;
                    }
                    self.refresh_sides();
                }
                Drag::Ink(rec) => {
                    if let Some(w) = world_ptr {
                        rec.push(w, time);
                    }
                }
                Drag::Arrow { to, .. } => {
                    if let Some(w) = world_ptr {
                        *to = w;
                    }
                }
                Drag::Erase { .. } => {
                    if let Some(w) = world_ptr {
                        self.erase_at(w, tol, &mut drag, out);
                    }
                }
                Drag::None => {}
            }
            self.drag = drag;
        }

        if resp.drag_stopped() {
            match std::mem::replace(&mut self.drag, Drag::None) {
                Drag::Ink(rec) => {
                    let points = rec.finish();
                    if !points.is_empty() {
                        self.checkpoint();
                        let d = Drawing::new(self.ink_size, self.ink_color.clone(), points);
                        self.doc.celestine.drawings.push(d);
                        out.changed = true;
                    }
                }
                Drag::Arrow { from, to } => {
                    if let Some(target) = self.hit_object(to, tol).filter(|t| *t != from) {
                        if let (Some(a), Some(b)) = (self.object_rect(&from), self.object_rect(&target)) {
                            self.checkpoint();
                            let (sa, sb) = auto_sides(a, b);
                            let edge = CanvasEdge::new(&from, sa, &target, sb);
                            self.selection = Some(Sel::Edge(edge.id.clone()));
                            self.doc.push_edge(edge);
                            out.changed = true;
                        }
                    }
                }
                _ => {}
            }
        }

        let click_world = resp.interact_pointer_pos().map(|s| self.to_world(rect, s));
        if resp.double_clicked() {
            if let Some(w) = click_world {
                self.double_click(w, tol, out, ctx);
            }
        } else if resp.clicked() {
            if let Some(w) = click_world {
                self.editing_text = None;
                self.editing_label = None;
                match self.tool {
                    Tool::Text => self.add_text_at(w),
                    Tool::Note => self.open_new_note_prompt(w, env.canvas_rel),
                    Tool::Image => self.pick_images(env, w, out),
                    _ => {
                        self.selection = self
                            .hit_object(w, tol)
                            .map(Sel::Object)
                            .or_else(|| self.hit_edge(w, tol).map(Sel::Edge));
                    }
                }
            }
        }
        if resp.secondary_clicked() {
            if let Some(w) = click_world {
                self.menu_at = w;
                self.menu_target = self
                    .hit_object(w, tol)
                    .map(Sel::Object)
                    .or_else(|| self.hit_edge(w, tol).map(Sel::Edge));
                if self.menu_target.is_some() {
                    self.selection = self.menu_target.clone();
                }
            }
        }
    }

    fn double_click(&mut self, w: Pos2, tol: f32, out: &mut BoardOutcome, ctx: &egui::Context) {
        if let Some(id) = self.hit_object(w, tol) {
            let Some(n) = self.doc.nodes.iter().find(|n| n.id == id).cloned() else {
                return; // drawings have nothing to edit
            };
            match n.node_kind() {
                NodeKind::Text => {
                    self.checkpoint();
                    self.editing_text = Some(id);
                    self.edit_focus_pending = true;
                }
                NodeKind::File => out.open = n.file.clone(),
                NodeKind::Link => {
                    if let Some(url) = n.url {
                        ctx.open_url(egui::OpenUrl::new_tab(url));
                    }
                }
                _ => {}
            }
        } else if let Some(id) = self.hit_edge(w, tol) {
            self.checkpoint();
            self.selection = Some(Sel::Edge(id.clone()));
            self.editing_label = Some(id);
            self.edit_focus_pending = true;
        } else if self.tool == Tool::Select {
            self.add_text_at(w);
            out.changed = true;
        }
    }

    fn erase_at(&mut self, w: Pos2, tol: f32, drag: &mut Drag, out: &mut BoardOutcome) {
        let hits: Vec<String> = self
            .doc
            .celestine
            .drawings
            .iter()
            .filter(|d| canvas_ink::distance_to(&d.points, w) <= d.size * 0.5 + tol)
            .map(|d| d.id.clone())
            .collect();
        if hits.is_empty() {
            return;
        }
        if let Drag::Erase { checkpointed } = drag {
            if !*checkpointed {
                self.checkpoint();
                *checkpointed = true;
            }
        }
        for id in hits {
            self.doc.remove_object(&id);
        }
        out.changed = true;
    }

    fn resize_handle_hit(&self, rect: Rect, screen: Pos2) -> Option<String> {
        let Some(Sel::Object(id)) = &self.selection else {
            return None;
        };
        let n = self.doc.nodes.iter().find(|n| &n.id == id)?;
        let r = self.rect_to_screen(rect, node_rect(n)).expand(4.0);
        (r.right_bottom().distance(screen) <= 9.0).then(|| id.clone())
    }

    fn handle_keys(&mut self, ctx: &egui::Context, enabled: bool, out: &mut BoardOutcome) {
        if !enabled || ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        // Shifted chord first: consume_key ignores extra Shift.
        let (redo, undo, backspace) = ctx.input_mut(|i| {
            let redo = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)
                || i.consume_key(Modifiers::COMMAND, Key::Y);
            let undo = !redo && i.consume_key(Modifiers::COMMAND, Key::Z);
            (redo, undo, i.consume_key(Modifiers::NONE, Key::Backspace))
        });
        if redo && self.redo() {
            out.changed = true;
        }
        if undo && self.undo() {
            out.changed = true;
        }
        if backspace && self.delete_selection() {
            out.changed = true;
        }
        for tool in Tool::ALL {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, tool.key())) {
                self.set_tool(tool);
            }
        }
    }

    fn context_menu(&mut self, resp: &egui::Response, rect: Rect, env: &mut BoardEnv<'_>, out: &mut BoardOutcome) {
        let mut action = None;
        let target = self.menu_target.clone();
        let p = env.palette;
        resp.context_menu(|ui| {
            let item = |ui: &mut Ui, s: &str| {
                ui.add(egui::Button::new(RichText::new(s).monospace().size(12.0)).frame(false))
                    .clicked()
            };
            if let Some(t) = &target {
                if let Sel::Object(id) = t {
                    if let Some(file) = self
                        .doc
                        .nodes
                        .iter()
                        .find(|n| &n.id == id)
                        .and_then(|n| n.file.clone())
                    {
                        if item(ui, "open") {
                            action = Some(MenuAction::Open(file));
                        }
                    }
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    ui.label(RichText::new("color ").monospace().size(12.0));
                    for c in INK_COLORS {
                        let color = c.and_then(|c| preset_color(p, c)).unwrap_or(p.muted);
                        let b = egui::Button::new(RichText::new("■").size(14.0).color(color)).frame(false);
                        if ui.add(b).clicked() {
                            let id = match t {
                                Sel::Object(id) | Sel::Edge(id) => id.clone(),
                            };
                            action = Some(MenuAction::Color(id, c.map(str::to_string)));
                        }
                    }
                });
                if item(ui, "delete") {
                    action = Some(MenuAction::Delete(t.clone()));
                }
                ui.separator();
            }
            if item(ui, "new note here…") {
                action = Some(MenuAction::NewNote);
            }
            if item(ui, "text here") {
                action = Some(MenuAction::Text);
            }
            if item(ui, "image here…") {
                action = Some(MenuAction::Image);
            }
            if item(ui, "fit view") {
                action = Some(MenuAction::Fit);
            }
            if action.is_some() {
                ui.close();
            }
        });
        let at = self.menu_at;
        match action {
            Some(MenuAction::NewNote) => self.open_new_note_prompt(at, env.canvas_rel),
            Some(MenuAction::Text) => {
                self.add_text_at(at);
                out.changed = true;
            }
            Some(MenuAction::Image) => self.pick_images(env, at, out),
            Some(MenuAction::Delete(sel)) => {
                self.selection = Some(sel);
                out.changed |= self.delete_selection();
            }
            Some(MenuAction::Color(id, color)) => {
                self.checkpoint();
                if let Some(n) = self.doc.nodes.iter_mut().find(|n| n.id == id) {
                    n.color = color.clone();
                }
                if let Some(d) = self.doc.celestine.drawings.iter_mut().find(|d| d.id == id) {
                    d.color = color.clone();
                }
                for e in self
                    .doc
                    .edges
                    .iter_mut()
                    .chain(self.doc.celestine.edges.iter_mut())
                    .filter(|e| e.id == id)
                {
                    e.color = color.clone();
                }
                out.changed = true;
            }
            Some(MenuAction::Open(file)) => out.open = Some(file),
            Some(MenuAction::Fit) => self.fit(rect),
            None => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_drops(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        rect: Rect,
        resp: &egui::Response,
        env: &mut BoardEnv<'_>,
        p: &ThemePalette,
        out: &mut BoardOutcome,
    ) {
        let pointer = ctx.input(|i| i.pointer.hover_pos()).filter(|ptr| rect.contains(*ptr));
        let at = self.to_world(rect, pointer.unwrap_or(rect.center()));

        let tree_hover = resp.dnd_hover_payload::<TreeNavItem>().is_some();
        let os_hover = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if tree_hover || os_hover {
            dashed_rect(painter, rect.shrink(3.0), Stroke::new(1.0, p.accent), 6.0, 4.0);
            painter.text(
                rect.center_top() + vec2(0.0, 14.0),
                Align2::CENTER_TOP,
                "[ drop to add to canvas ]",
                FontId::monospace(12.0),
                p.accent,
            );
        }

        if let Some(item) = resp.dnd_release_payload::<TreeNavItem>() {
            match item.as_ref() {
                TreeNavItem::File(rel) => {
                    self.add_file_card(env.vault, rel, at);
                    out.changed = true;
                    out.status = Some(format!("linked {rel}"));
                }
                TreeNavItem::Dir(_) => out.status = Some("folders can't be placed on a canvas".into()),
            }
        }

        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if !dropped.is_empty() {
            self.import_many(env, dropped, at, out);
        }
    }

    fn new_note_prompt(
        &mut self,
        ctx: &egui::Context,
        rect: Rect,
        env: &mut BoardEnv<'_>,
        p: &ThemePalette,
        out: &mut BoardOutcome,
    ) {
        let Some(prompt) = self.new_note.as_mut() else {
            return;
        };
        let screen = (rect.center() + self.pan + prompt.at.to_vec2() * self.zoom)
            .clamp(rect.min, rect.max - vec2(280.0, 170.0));
        let mut create = false;
        let mut cancel = false;
        let folders: Vec<String> = {
            let mut set: Vec<String> = env
                .vault
                .list_md_rel_paths()
                .iter()
                .filter_map(|r| {
                    Path::new(r)
                        .parent()
                        .map(|p| p.to_string_lossy().replace('\\', "/"))
                })
                .filter(|f| !f.is_empty())
                .collect();
            set.sort();
            set.dedup();
            let q = prompt.folder.to_lowercase();
            set.into_iter()
                .filter(|f| f.to_lowercase().contains(&q) && *f != prompt.folder)
                .take(5)
                .collect()
        };
        egui::Area::new(egui::Id::unique("canvas-new-note"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen)
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(p.panel_bg)
                    .stroke(Stroke::new(1.0, p.accent))
                    .inner_margin(10.0)
                    .show(ui, |ui| {
                        ui.set_width(260.0);
                        let mono = |s: &str| RichText::new(s).monospace().size(12.0);
                        ui.label(mono("┌─ new note ─").color(p.accent));
                        ui.label(mono("folder").color(p.muted));
                        ui.add(
                            TextEdit::singleline(&mut prompt.folder)
                                .font(FontId::monospace(12.0))
                                .hint_text("(vault root)")
                                .desired_width(f32::INFINITY),
                        );
                        if !folders.is_empty() {
                            ui.horizontal_wrapped(|ui| {
                                for f in &folders {
                                    let b = egui::Button::new(mono(&format!("[{f}]")).color(p.muted)).frame(false);
                                    if ui.add(b).clicked() {
                                        prompt.folder = f.clone();
                                    }
                                }
                            });
                        }
                        ui.label(mono("name").color(p.muted));
                        let name = ui.add(
                            TextEdit::singleline(&mut prompt.name)
                                .font(FontId::monospace(12.0))
                                .hint_text("Untitled")
                                .desired_width(f32::INFINITY),
                        );
                        if prompt.focus {
                            name.request_focus();
                            prompt.focus = false;
                        }
                        if name.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            create = true;
                        }
                        ui.label(mono(&format!("→ {}", new_note_rel(&prompt.folder, &prompt.name))).color(p.muted));
                        ui.horizontal(|ui| {
                            let b = egui::Button::new(mono("[create ⏎]").color(p.content_bg))
                                .fill(p.accent)
                                .corner_radius(0.0);
                            if ui.add(b).clicked() {
                                create = true;
                            }
                            if ui.add(egui::Button::new(mono("[cancel]").color(p.muted)).frame(false)).clicked() {
                                cancel = true;
                            }
                        });
                    });
            });
        if cancel {
            self.new_note = None;
        } else if create {
            let Some(prompt) = self.new_note.take() else {
                return;
            };
            let rel = new_note_rel(&prompt.folder, &prompt.name);
            let existed = env.vault.index.get_by_rel(&rel).is_some();
            match env.vault.create_note(&rel) {
                Ok(()) => {
                    self.add_file_card(env.vault, &rel, prompt.at);
                    out.changed = true;
                    out.files_changed = true;
                    out.status = Some(if existed {
                        format!("linked existing {rel}")
                    } else {
                        format!("created {rel}")
                    });
                }
                Err(e) => out.status = Some(format!("create failed: {e}")),
            }
        }
    }

    fn set_cursor(&self, ctx: &egui::Context, rect: Rect, resp: &egui::Response) {
        if !resp.hovered() && !resp.dragged() {
            return;
        }
        let icon = match (&self.drag, self.tool) {
            (Drag::Pan, _) => CursorIcon::Grabbing,
            (Drag::Move(_), _) => CursorIcon::Move,
            (Drag::Resize { .. }, _) => CursorIcon::ResizeNwSe,
            (_, Tool::Ink) | (_, Tool::Arrow) | (_, Tool::Erase) => CursorIcon::Crosshair,
            (_, Tool::Text) => CursorIcon::Text,
            (_, Tool::Note) | (_, Tool::Image) => CursorIcon::Cell,
            (_, Tool::Select) => {
                let ptr = resp.hover_pos();
                if ptr.is_some_and(|s| self.resize_handle_hit(rect, s).is_some()) {
                    CursorIcon::ResizeNwSe
                } else if ptr.is_some_and(|s| self.hit_object(self.to_world(rect, s), 6.0 / self.zoom).is_some()) {
                    CursorIcon::Grab
                } else {
                    CursorIcon::Default
                }
            }
        };
        ctx.set_cursor_icon(icon);
    }
}

// ── free helpers ───────────────────────────────────────────────────

/// A free `attachments/<stem>[-n].<ext>` path beside the canvas, with its folder created.
fn attachment_dest(env: &BoardEnv<'_>, stem: &str, ext: &str) -> Result<(String, PathBuf), String> {
    let parent = Path::new(env.canvas_rel)
        .parent()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .filter(|p| !p.is_empty())
        .unwrap_or_default();
    let dir = if parent.is_empty() {
        "attachments".to_string()
    } else {
        format!("{parent}/attachments")
    };
    let mut n = 0;
    let (rel, abs) = loop {
        let name = if n == 0 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem}-{n}.{ext}")
        };
        let rel = format!("{dir}/{name}");
        let abs = env.vault.abs_path(&rel).map_err(|e| e.to_string())?;
        if !abs.exists() {
            break (rel, abs);
        }
        n += 1;
    };
    if let Some(p) = abs.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    Ok((rel, abs))
}

/// Write a clipboard image as `attachments/Pasted image <stamp>.png`.
fn save_pasted_image(env: &BoardEnv<'_>, image: &egui::ColorImage) -> Result<String, String> {
    let [w, h] = image.size;
    let rgba: Vec<u8> = image
        .pixels
        .iter()
        .flat_map(|c| c.to_srgba_unmultiplied())
        .collect();
    let buffer = image::RgbaImage::from_raw(w as u32, h as u32, rgba)
        .ok_or_else(|| "clipboard image has an invalid size".to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (rel, abs) = attachment_dest(env, &format!("Pasted image {stamp}"), "png")?;
    buffer
        .save_with_format(&abs, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(rel)
}

/// Paths from a file-manager copy: every non-empty line is a `file://` URI
/// or an existing absolute path. `None` when the text is anything else.
fn pasted_file_paths(text: &str) -> Option<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let path = if let Some(rest) = line.strip_prefix("file://") {
            // Drop an optional host (`file://localhost/...`).
            let rest = &rest[rest.find('/')?..];
            PathBuf::from(percent_decode(rest)?)
        } else {
            let p = PathBuf::from(line);
            if !(p.is_absolute() && p.is_file()) {
                return None;
            }
            p
        };
        paths.push(path);
    }
    (!paths.is_empty()).then_some(paths)
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn new_note_rel(folder: &str, name: &str) -> String {
    let folder = folder.trim().trim_matches('/');
    let name = name.trim().replace(['/', '\\'], "");
    let name = if name.is_empty() { "Untitled".to_string() } else { name };
    let name = if name.to_lowercase().ends_with(".md") {
        name
    } else {
        format!("{name}.md")
    };
    if folder.is_empty() {
        name
    } else {
        format!("{folder}/{name}")
    }
}

fn node_rect(n: &CanvasNode) -> Rect {
    Rect::from_min_size(pos2(n.x, n.y), vec2(n.width.max(1.0), n.height.max(1.0)))
}

fn bounds_rect(b: (f32, f32, f32, f32)) -> Rect {
    Rect::from_min_max(pos2(b.0, b.1), pos2(b.2, b.3))
}

fn object_rect(doc: &Canvas, id: &str) -> Option<Rect> {
    doc.nodes
        .iter()
        .find(|n| n.id == id)
        .map(node_rect)
        .or_else(|| {
            doc.celestine
                .drawings
                .iter()
                .find(|d| d.id == id)
                .and_then(|d| d.bounds())
                .map(bounds_rect)
        })
}

fn content_bounds(doc: &Canvas) -> Option<Rect> {
    doc.nodes
        .iter()
        .map(node_rect)
        .chain(doc.celestine.drawings.iter().filter_map(|d| d.bounds().map(bounds_rect)))
        .reduce(|a, b| a.union(b))
}

fn card_content_rect(r: Rect, zoom: f32) -> Rect {
    let pad = PAD * zoom;
    Rect::from_min_max(r.min + vec2(pad, pad * 1.1), r.max - vec2(pad, pad * 0.6))
}

fn auto_sides(a: Rect, b: Rect) -> (Side, Side) {
    let d = b.center() - a.center();
    // Normalise by size so wide cards prefer left/right connections.
    let nx = d.x / (a.width() + b.width()).max(1.0);
    let ny = d.y / (a.height() + b.height()).max(1.0);
    if nx.abs() >= ny.abs() {
        if d.x >= 0.0 {
            (Side::Right, Side::Left)
        } else {
            (Side::Left, Side::Right)
        }
    } else if d.y >= 0.0 {
        (Side::Bottom, Side::Top)
    } else {
        (Side::Top, Side::Bottom)
    }
}

fn anchor(r: Rect, side: Side) -> (Pos2, Vec2) {
    match side {
        Side::Top => (r.center_top(), vec2(0.0, -1.0)),
        Side::Right => (r.right_center(), vec2(1.0, 0.0)),
        Side::Bottom => (r.center_bottom(), vec2(0.0, 1.0)),
        Side::Left => (r.left_center(), vec2(-1.0, 0.0)),
    }
}

/// Orthogonal (TUI-style) route between two anchored sides.
fn elbow(a: Rect, sa: Side, b: Rect, sb: Side, out: f32) -> Vec<Pos2> {
    let (p0, n0) = anchor(a, sa);
    let (p1, n1) = anchor(b, sb);
    let out = if out > 0.0 { out } else { ARROW_OUT };
    let a1 = p0 + n0 * out;
    let b1 = p1 + n1 * out;
    if n0.x != 0.0 {
        let mx = (a1.x + b1.x) * 0.5;
        vec![p0, a1, pos2(mx, a1.y), pos2(mx, b1.y), b1, p1]
    } else {
        let my = (a1.y + b1.y) * 0.5;
        vec![p0, a1, pos2(a1.x, my), pos2(b1.x, my), b1, p1]
    }
}

fn edge_route(doc: &Canvas, e: &CanvasEdge) -> Option<Vec<Pos2>> {
    let a = object_rect(doc, &e.from_node)?;
    let b = object_rect(doc, &e.to_node)?;
    let (auto_a, auto_b) = auto_sides(a, b);
    let route = elbow(a, e.from_side.unwrap_or(auto_a), b, e.to_side.unwrap_or(auto_b), 0.0);
    let mut out: Vec<Pos2> = Vec::with_capacity(route.len());
    for p in route {
        if out.last().is_none_or(|q: &Pos2| q.distance(p) > 0.01) {
            out.push(p);
        }
    }
    Some(out)
}

fn route_mid(pts: &[Pos2]) -> Pos2 {
    match pts.len() {
        0 => Pos2::ZERO,
        1 => pts[0],
        n => {
            let i = (n - 1) / 2;
            pts[i].lerp(pts[i + 1], 0.5)
        }
    }
}

/// Snap to pixel centres so 1px lines stay crisp.
fn crisp(p: Pos2) -> Pos2 {
    pos2(p.x.round() + 0.5, p.y.round() + 0.5)
}

fn crisp_rect(r: Rect) -> Rect {
    Rect::from_min_max(crisp(r.min), crisp(r.max))
}

fn arrow_head(painter: &egui::Painter, tip: Pos2, from: Pos2, zoom: f32, color: Color32) {
    let dir = (tip - from).normalized();
    if !dir.x.is_finite() {
        return;
    }
    let len = (9.0 * zoom.sqrt()).clamp(6.0, 14.0).round();
    let half = (len * 0.55).round();
    let base = tip - dir * len;
    let perp = dir.rot90();
    painter.add(Shape::convex_polygon(
        vec![tip, base + perp * half, base - perp * half],
        color,
        Stroke::NONE,
    ));
}

fn dashed_rect(painter: &egui::Painter, r: Rect, stroke: Stroke, dash: f32, gap: f32) {
    let path = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    painter.extend(Shape::dashed_line(&path, stroke, dash, gap));
}

/// TUI frame title punched into the top border: `┌─ title ───┐`.
fn tui_title(painter: &egui::Painter, r: Rect, title: &str, zoom: f32, color: Color32, bg: Color32) {
    let size = (TITLE_FONT * zoom).clamp(0.0, 30.0);
    if size < 5.0 {
        return;
    }
    let font = FontId::monospace(size);
    let max_chars = ((r.width() - 3.0 * size) / (size * 0.62)).max(0.0) as usize;
    let mut text: String = title.chars().take(max_chars).collect();
    if text.chars().count() < title.chars().count() && max_chars > 1 {
        text.pop();
        text.push('…');
    }
    if text.is_empty() {
        return;
    }
    let galley = painter.layout_no_wrap(format!(" {text} "), font.clone(), color);
    let at = pos2(r.left() + size * 1.2, r.top() - galley.size().y * 0.5);
    painter.rect_filled(Rect::from_min_size(at, galley.size()), 0.0, bg);
    painter.galley(at, galley, color);
    let tick = painter.layout_no_wrap("─".into(), font, color);
    let tick_at = pos2(r.left() + size * 0.3, r.top() - tick.size().y * 0.5);
    painter.rect_filled(Rect::from_min_size(tick_at, tick.size()), 0.0, bg);
    painter.galley(tick_at, tick, color);
}

/// `[↗]` marker on a live note card's top-right border.
fn open_tag(painter: &egui::Painter, r: Rect, zoom: f32, color: Color32, bg: Color32) {
    let size = (TITLE_FONT * zoom).clamp(0.0, 24.0);
    if size < 6.0 {
        return;
    }
    let galley = painter.layout_no_wrap(" dbl-click ↗ ".into(), FontId::monospace(size * 0.85), color);
    let at = pos2(r.right() - galley.size().x - size, r.top() - galley.size().y * 0.5);
    painter.rect_filled(Rect::from_min_size(at, galley.size()), 0.0, bg);
    painter.galley(at, galley, color);
}

/// Little `+` glyph ticks on the corners of text cards.
fn tui_corner_marks(painter: &egui::Painter, r: Rect, color: Color32) {
    let r = crisp_rect(r);
    for c in [r.left_top(), r.right_top(), r.left_bottom(), r.right_bottom()] {
        painter.rect_filled(Rect::from_center_size(c, Vec2::splat(3.0)), 0.0, color);
    }
}

fn missing(painter: &egui::Painter, r: Rect, text: &str, color: Color32, zoom: f32) {
    let size = (TITLE_FONT * zoom).clamp(0.0, 28.0);
    if size < 5.0 {
        return;
    }
    let galley = painter.layout(text.to_string(), FontId::monospace(size), color, r.width().max(1.0));
    painter.with_clip_rect(r).galley(r.min, galley, color);
}

fn node_accent(n: &CanvasNode, p: &ThemePalette) -> Option<Color32> {
    n.color.as_deref().and_then(|c| preset_color(p, c))
}

/// JSON Canvas colours: presets `"1"`..`"6"` (themed) or `#rrggbb`.
fn preset_color(p: &ThemePalette, c: &str) -> Option<Color32> {
    if let Some(hex) = c.strip_prefix('#') {
        if hex.len() == 6 {
            let v = u32::from_str_radix(hex, 16).ok()?;
            return Some(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8));
        }
        return None;
    }
    let i: usize = c.parse().ok()?;
    (1..=6).contains(&i).then(|| p.folder[i - 1])
}

/// Pixel dot grid: 1px squares, snapped to whole pixels, a touch off the base bg.
fn paint_dot_grid(painter: &egui::Painter, rect: Rect, pan: Vec2, zoom: f32, p: &ThemePalette) {
    let mut step = GRID * zoom;
    while step < 12.0 {
        step *= 4.0;
    }
    let origin = rect.center() + pan;
    let dot = p.content_bg.lerp_to_gamma(p.text, 0.11);
    let major = p.content_bg.lerp_to_gamma(p.text, 0.2);
    let size = if zoom >= 2.0 { 2.0 } else { 1.0 };
    let start_x = origin.x + ((rect.left() - origin.x) / step).floor() * step;
    let start_y = origin.y + ((rect.top() - origin.y) / step).floor() * step;
    let mut mesh = Mesh::default();
    let mut y = start_y;
    while y <= rect.bottom() {
        let iy = ((y - origin.y) / step).round() as i64;
        let mut x = start_x;
        while x <= rect.right() {
            let ix = ((x - origin.x) / step).round() as i64;
            let c = if ix.rem_euclid(8) == 0 && iy.rem_euclid(8) == 0 { major } else { dot };
            let at = pos2(x.round(), y.round());
            mesh.add_colored_rect(Rect::from_min_size(at, Vec2::splat(size)), c);
            x += step;
        }
        y += step;
    }
    painter.add(mesh);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_note_rel_joins_and_adds_extension() {
        assert_eq!(new_note_rel("", "Idea"), "Idea.md");
        assert_eq!(new_note_rel("/Projects/", "Plan.md"), "Projects/Plan.md");
        assert_eq!(new_note_rel("A", "  "), "A/Untitled.md");
        assert_eq!(new_note_rel("A", "x/y"), "A/xy.md");
    }

    #[test]
    fn sides_face_each_other() {
        let a = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 50.0));
        let right = a.translate(vec2(300.0, 20.0));
        let below = a.translate(vec2(10.0, 300.0));
        assert_eq!(auto_sides(a, right), (Side::Right, Side::Left));
        assert_eq!(auto_sides(a, below), (Side::Bottom, Side::Top));
        let route = elbow(a, Side::Right, right, Side::Left, 0.0);
        assert_eq!(route.first(), Some(&a.right_center()));
        assert_eq!(route.last(), Some(&right.left_center()));
        // Every segment is axis-aligned.
        assert!(route.windows(2).all(|w| w[0].x == w[1].x || w[0].y == w[1].y));
    }

    #[test]
    fn view_round_trips_coordinates() {
        let mut b = CanvasBoard::new(Canvas::default());
        let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(800.0, 600.0));
        b.zoom_at(rect, pos2(300.0, 200.0), 1.7);
        let w = pos2(123.0, -45.0);
        let back = b.to_world(rect, b.to_screen(rect, w));
        assert!(back.distance(w) < 1e-3);
        // Zoom keeps the point under the cursor fixed.
        let before = b.to_world(rect, pos2(300.0, 200.0));
        b.zoom_at(rect, pos2(300.0, 200.0), 0.5);
        assert!(b.to_world(rect, pos2(300.0, 200.0)).distance(before) < 1e-3);
    }

    #[test]
    fn undo_restores_deleted_object() {
        let mut b = CanvasBoard::new(Canvas::default());
        b.add_text_at(pos2(0.0, 0.0));
        let id = b.doc.nodes[0].id.clone();
        b.selection = Some(Sel::Object(id));
        assert!(b.delete_selection());
        assert!(b.doc.nodes.is_empty());
        assert!(b.undo());
        assert_eq!(b.doc.nodes.len(), 1);
        assert!(b.redo());
        assert!(b.doc.nodes.is_empty());
    }

    struct Harness {
        ctx: egui::Context,
        vault: Vault,
        _dir: tempfile::TempDir,
        resolver: ImageResolver,
        palette: ThemePalette,
        time: f64,
    }

    impl Harness {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("Note.md"), "# Note\n\nSome **text** and [[Other]].\n").unwrap();
            let vault = Vault::open(dir.path()).unwrap();
            Self {
                ctx: egui::Context::default(),
                vault,
                _dir: dir,
                resolver: ImageResolver::default(),
                palette: ThemePalette::from_tokens(&celestine_theme::ThemeTokens::default()),
                time: 0.0,
            }
        }

        fn frame(&mut self, board: &mut CanvasBoard, events: Vec<egui::Event>) -> BoardOutcome {
            self.time += 1.0 / 60.0;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let mut outcome = BoardOutcome::default();
            let ctx = self.ctx.clone();
            let mut full = ctx.run_ui(input, |ui| {
                let mut env = BoardEnv {
                    vault: &mut self.vault,
                    canvas_rel: "Board.canvas",
                    palette: &self.palette,
                    image_rels: &[],
                    image_resolver: &mut self.resolver,
                    font_size: 15.0,
                    keys_enabled: true,
                };
                let o = board.show(ui, &mut env);
                outcome.changed |= o.changed;
                outcome.open = outcome.open.take().or(o.open);
            });
            full.textures_delta.clear();
            outcome
        }

        /// Press at `from`, move in steps to `to`, release. Returns whether anything changed.
        fn drag(&mut self, board: &mut CanvasBoard, from: Pos2, to: Pos2) -> bool {
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            };
            let mut changed = false;
            changed |= self.frame(board, vec![egui::Event::PointerMoved(from)]).changed;
            changed |= self.frame(board, vec![button(from, true)]).changed;
            for i in 1..=12 {
                let p = from.lerp(to, i as f32 / 12.0);
                changed |= self.frame(board, vec![egui::Event::PointerMoved(p)]).changed;
            }
            changed |= self.frame(board, vec![button(to, false)]).changed;
            changed |= self.frame(board, vec![]).changed;
            changed
        }
    }

    #[test]
    fn headless_frames_draw_ink_and_connect_arrows() {
        let mut h = Harness::new();
        let mut doc = Canvas::default();
        let text = CanvasNode::new_text(-400.0, -50.0, 200.0, 80.0, "hello");
        let note = CanvasNode::new_file(100.0, -150.0, 400.0, 300.0, "Note.md");
        let image = CanvasNode::new_file(-400.0, 200.0, 200.0, 150.0, "missing.png");
        let (tid, nid) = (text.id.clone(), note.id.clone());
        doc.nodes = vec![text, note, image];
        let mut b = CanvasBoard::new(doc);
        for _ in 0..3 {
            h.frame(&mut b, vec![]);
        }
        let rect = b.view_rect;
        assert!(rect.width() > 100.0 && rect.height() > 100.0, "{rect:?}");

        b.set_tool(Tool::Ink);
        let a = b.to_screen(rect, pos2(-350.0, 450.0));
        let c = b.to_screen(rect, pos2(150.0, 520.0));
        assert!(h.drag(&mut b, a, c));
        assert_eq!(b.doc.celestine.drawings.len(), 1);
        let did = b.doc.celestine.drawings[0].id.clone();

        b.set_tool(Tool::Arrow);
        let from = b.to_screen(rect, pos2(-300.0, -10.0));
        let to = b.to_screen(rect, pos2(300.0, 0.0));
        assert!(h.drag(&mut b, from, to));
        assert!(b.doc.edges.iter().any(|e| e.from_node == tid && e.to_node == nid));

        // Arrows into a drawing land in the Celestine extension list.
        let to_ink = b.to_screen(rect, b.doc.celestine.drawings[0].points[30].map_pos());
        assert!(h.drag(&mut b, from, to_ink));
        assert!(b.doc.celestine.edges.iter().any(|e| e.to_node == did));

        // Select tool: dragging the text card moves it.
        b.set_tool(Tool::Select);
        let before = b.doc.nodes[0].x;
        assert!(h.drag(&mut b, from, from + vec2(60.0, 0.0)));
        assert!(b.doc.nodes[0].x > before);
        for _ in 0..2 {
            h.frame(&mut b, vec![]);
        }
        let saved = Canvas::load(&b.doc.to_pretty_string()).unwrap();
        assert_eq!(saved.all_edges().count(), 2);
    }

    #[test]
    fn pasting_images_files_and_text_adds_cards() {
        let mut h = Harness::new();
        let mut b = CanvasBoard::new(Canvas::default());
        h.frame(&mut b, vec![]);

        let image = egui::ColorImage::new([4, 3], vec![Color32::from_rgb(200, 10, 10); 12]);
        h.frame(&mut b, vec![egui::Event::PasteImage(std::sync::Arc::new(image))]);
        let rel = b.doc.nodes[0].file.clone().unwrap();
        assert!(rel.starts_with("attachments/Pasted image "), "{rel}");
        let abs = h.vault.abs_path(&rel).unwrap();
        assert_eq!(image::image_dimensions(&abs).unwrap(), (4, 3));

        let outside = tempfile::tempdir().unwrap();
        let src = outside.path().join("my shot.png");
        std::fs::copy(&abs, &src).unwrap();
        let uri = format!("file://{}", src.display().to_string().replace(' ', "%20"));
        h.frame(&mut b, vec![egui::Event::Paste(uri)]);
        assert_eq!(b.doc.nodes[1].file.as_deref(), Some("attachments/my shot.png"));

        h.frame(&mut b, vec![egui::Event::Paste("just words\nsecond".into())]);
        assert_eq!(b.doc.nodes[2].text.as_deref(), Some("just words\nsecond"));
    }

    #[test]
    fn file_paste_parsing() {
        assert_eq!(
            pasted_file_paths("file:///tmp/a%20b.png\nfile://localhost/x/y.jpg\n"),
            Some(vec![PathBuf::from("/tmp/a b.png"), PathBuf::from("/x/y.jpg")])
        );
        assert_eq!(pasted_file_paths("hello world"), None);
        assert_eq!(pasted_file_paths("file:///ok.png\nnot a path"), None);
    }

    trait MapPos {
        fn map_pos(&self) -> Pos2;
    }

    impl MapPos for [f32; 3] {
        fn map_pos(&self) -> Pos2 {
            pos2(self[0], self[1])
        }
    }

    #[test]
    fn moving_a_group_carries_its_children() {
        let mut doc = Canvas::default();
        let mut g = CanvasNode::new_text(0.0, 0.0, 500.0, 500.0, "");
        g.kind = "group".into();
        let inner = CanvasNode::new_text(50.0, 50.0, 100.0, 100.0, "in");
        let outer = CanvasNode::new_text(900.0, 0.0, 100.0, 100.0, "out");
        let (gid, iid, oid) = (g.id.clone(), inner.id.clone(), outer.id.clone());
        doc.nodes = vec![g, inner, outer];
        let mut b = CanvasBoard::new(doc);
        let set = b.move_set(&gid);
        assert!(set.contains(&iid) && !set.contains(&oid));
        b.translate(&set, vec2(10.0, 0.0));
        assert_eq!(b.doc.nodes[1].x, 60.0);
        assert_eq!(b.doc.nodes[2].x, 900.0);
    }
}
