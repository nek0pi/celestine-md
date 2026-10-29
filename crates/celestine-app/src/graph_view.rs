use crate::file_tree;
use celestine_core::{NoteId, Vault};
use egui::epaint::CircleShape;
use egui::{Color32, FontId, PointerButton, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Force layout runs briefly after open / topology change, then freezes.
const SETTLE_SECS: f32 = 1.6;
const SIM_ITERS_SMALL: usize = 4;
/// Repulsion becomes negligible beyond this distance; spatial grid uses it as cell size.
const REPEL_CUTOFF: f32 = 72.0;
const DRAG_REPEL_CUTOFF: f32 = 40.0;

#[derive(Default)]
pub struct GraphState {
    pub positions: HashMap<NoteId, Pos2>,
    pub local_mode: bool,
    pub depth: usize,
    pub pan: Vec2,
    pub zoom: f32,
    initialized: bool,
    settle_t: f32,
    last_node_count: usize,
    last_local: bool,
    last_depth: usize,
    last_center: Option<NoteId>,
    hovered: Option<NoteId>,
    dragged_node: Option<NoteId>,
    drag_relax_t: f32,
    focus_node: Option<NoteId>,
    focus_amount: f32,
    /// Gentle layout targets; isolated notes stay on the outer ring.
    layout_targets: HashMap<NoteId, Pos2>,
    /// Rebuilt only when topology / mode changes — avoids per-frame tree walks.
    cache: DrawCache,
}

#[derive(Default)]
struct DrawCache {
    palette_name: String,
    nodes: Vec<NoteId>,
    edges: Vec<(NoteId, NoteId)>,
    linked: HashSet<NoteId>,
    degrees: HashMap<NoteId, usize>,
    colors: HashMap<NoteId, Color32>,
    folders: HashMap<NoteId, String>,
    /// Nodes sorted by degree ascending (draw hubs last).
    draw_order: Vec<NoteId>,
    /// High-degree nodes used when edge LOD thins the graph.
    hubs: HashSet<NoteId>,
}

impl GraphState {
    pub fn new() -> Self {
        Self {
            depth: 2,
            zoom: 1.0,
            ..Default::default()
        }
    }

    pub fn restart_layout(&mut self) {
        self.initialized = false;
        self.positions.clear();
        self.layout_targets.clear();
        self.dragged_node = None;
        self.drag_relax_t = 0.0;
        self.focus_node = None;
        self.focus_amount = 0.0;
        self.cache = DrawCache::default();
        self.settle_t = SETTLE_SECS;
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        vault: &Vault,
        center: Option<NoteId>,
        accent: Color32,
        muted: Color32,
        text: Color32,
        palette: &crate::theme_apply::ThemePalette,
    ) -> Option<String> {
        let mut open = None;
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = response.rect;
        let frame_dt = ui.input(|i| i.stable_dt).clamp(1.0 / 120.0, 1.0 / 30.0);

        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            self.zoom = (self.zoom * (1.0 + scroll * 0.0015)).clamp(0.35, 3.5);
        }

        let nodes: Vec<NoteId> = if self.local_mode {
            if let Some(c) = center {
                vault.graph.local(c, self.depth).into_iter().collect()
            } else {
                vault.graph.node_ids()
            }
        } else {
            vault.graph.node_ids()
        };

        let topology_changed = !self.initialized
            || self.last_node_count != nodes.len()
            || self.last_local != self.local_mode
            || self.last_depth != self.depth
            || (self.local_mode && self.last_center != center);
        let theme_changed = self.cache.palette_name != palette.name;

        if topology_changed || theme_changed {
            self.cache = build_draw_cache(vault, &nodes, palette);
        }
        if topology_changed {
            self.dragged_node = None;
            self.drag_relax_t = 0.0;
            self.focus_node = None;
            self.focus_amount = 0.0;
            seed_cluster_layout(
                &mut self.positions,
                &mut self.layout_targets,
                &self.cache,
                rect,
            );
            self.initialized = true;
            self.settle_t = SETTLE_SECS;
            self.last_node_count = nodes.len();
            self.last_local = self.local_mode;
            self.last_depth = self.depth;
            self.last_center = center;
        }

        if response.drag_started_by(PointerButton::Primary) {
            let press = ui.input(|i| i.pointer.press_origin());
            self.dragged_node = press.and_then(|pos| {
                node_at_screen(&self.cache, &self.positions, rect, self.pan, self.zoom, pos)
            });
            if self.dragged_node.is_some() {
                self.settle_t = 0.0;
                self.drag_relax_t = 0.55;
            }
        }
        if response.dragged() {
            if let Some(id) = self.dragged_node {
                let delta = response.drag_delta() / self.zoom;
                if let Some(position) = self.positions.get_mut(&id) {
                    *position += delta;
                }
                for neighbor in vault.graph.neighbors(id) {
                    if let Some(position) = self.positions.get_mut(&neighbor) {
                        *position += delta * 0.55;
                    }
                }
                self.drag_relax_t = 0.55;
            } else {
                self.pan += response.drag_delta();
            }
        }
        if response.drag_stopped() {
            self.dragged_node = None;
        }

        if self.dragged_node.is_some() || self.drag_relax_t > 0.0 {
            let iters = if self.cache.nodes.len() > 400 {
                1
            } else if self.cache.nodes.len() > 120 {
                2
            } else {
                3
            };
            for _ in 0..iters {
                settle_clustered(
                    &mut self.positions,
                    &self.layout_targets,
                    &self.cache,
                    rect,
                    frame_dt * 5.0,
                    self.dragged_node,
                    true,
                );
            }
            if self.dragged_node.is_none() {
                self.drag_relax_t = (self.drag_relax_t - frame_dt).max(0.0);
            }
            ui.ctx().request_repaint();
        } else if self.settle_t > 0.0 {
            let dt = frame_dt;
            let progress = 1.0 - (self.settle_t / SETTLE_SECS).clamp(0.0, 1.0);
            let strength = (1.0 - progress).powf(1.15);
            let n = self.cache.nodes.len();
            let iters = if n > 2_000 {
                1
            } else if n > 800 {
                2
            } else {
                SIM_ITERS_SMALL
            };
            for _ in 0..iters {
                settle_clustered(
                    &mut self.positions,
                    &self.layout_targets,
                    &self.cache,
                    rect,
                    dt * 1.8 * strength,
                    None,
                    false,
                );
            }
            self.settle_t = (self.settle_t - dt).max(0.0);
            ui.ctx().request_repaint();
        }

        let n = self.cache.nodes.len();
        let lod = paint_lod(n, self.zoom);
        let view_pad = 48.0;
        let view = rect.expand(view_pad);
        self.hovered = self.dragged_node.or_else(|| {
            response.hover_pos().and_then(|pointer| {
                node_at_screen(
                    &self.cache,
                    &self.positions,
                    rect,
                    self.pan,
                    self.zoom,
                    pointer,
                )
            })
        });
        if let Some(id) = self.hovered {
            self.focus_node = Some(id);
        }
        let focus_target = if self.hovered.is_some() { 1.0 } else { 0.0 };
        self.focus_amount += (focus_target - self.focus_amount) * (1.0 - (-11.0 * frame_dt).exp());
        if (self.focus_amount - focus_target).abs() < 0.01 {
            self.focus_amount = focus_target;
        } else {
            ui.ctx().request_repaint();
        }
        if self.focus_amount == 0.0 && self.hovered.is_none() {
            self.focus_node = None;
        }
        let focused_neighbors: HashSet<NoteId> = self
            .focus_node
            .map(|id| vault.graph.neighbors(id).into_iter().collect())
            .unwrap_or_default();
        let focus_color = self
            .focus_node
            .and_then(|id| self.cache.colors.get(&id).copied());

        // --- edges ---
        let active_neighbors: HashSet<NoteId> = center
            .map(|c| vault.graph.neighbors(c).into_iter().collect())
            .unwrap_or_default();
        let mut edge_shapes: Vec<Shape> = Vec::with_capacity(self.cache.edges.len().min(8_192));
        for &(a, b) in &self.cache.edges {
            let touches_focus = self.focus_node == Some(a) || self.focus_node == Some(b);
            let touches_active = center == Some(a)
                || center == Some(b)
                || active_neighbors.contains(&a)
                || active_neighbors.contains(&b);
            if lod.thin_edges {
                let hub_edge = self.cache.hubs.contains(&a) || self.cache.hubs.contains(&b);
                if !hub_edge && !touches_active && !touches_focus {
                    continue;
                }
            }
            let Some(&pa) = self.positions.get(&a) else {
                continue;
            };
            let Some(&pb) = self.positions.get(&b) else {
                continue;
            };
            let sa = map(pa, rect.center(), self.pan, self.zoom);
            let sb = map(pb, rect.center(), self.pan, self.zoom);
            if !segment_may_hit_view(sa, sb, view) {
                continue;
            }
            let (base_width, base_alpha) = if touches_active {
                (2.2_f32, 190_u8)
            } else {
                (1.6_f32, if lod.thin_edges { 75_u8 } else { 95_u8 })
            };
            let base_color =
                Color32::from_rgba_unmultiplied(muted.r(), muted.g(), muted.b(), base_alpha);
            let (width, color) = if let Some(focus_color) = focus_color {
                let (target_width, target_color) = if touches_focus {
                    (
                        2.6,
                        Color32::from_rgba_unmultiplied(
                            focus_color.r(),
                            focus_color.g(),
                            focus_color.b(),
                            235,
                        ),
                    )
                } else {
                    (
                        1.2,
                        Color32::from_rgba_unmultiplied(muted.r(), muted.g(), muted.b(), 22),
                    )
                };
                (
                    base_width + (target_width - base_width) * self.focus_amount,
                    crate::theme_apply::mix(base_color, target_color, self.focus_amount),
                )
            } else {
                (base_width, base_color)
            };
            edge_shapes.push(Shape::line_segment([sa, sb], Stroke::new(width, color)));
        }
        painter.extend(edge_shapes);

        // --- nodes ---
        let mut node_shapes: Vec<Shape> = Vec::with_capacity(self.cache.draw_order.len() * 2);

        for id in self
            .cache
            .draw_order
            .iter()
            .copied()
            .filter(|id| Some(*id) != self.focus_node)
            .chain(self.focus_node)
        {
            let Some(meta) = vault.index.get(id) else {
                continue;
            };
            let Some(&p) = self.positions.get(&id) else {
                continue;
            };
            let screen = map(p, rect.center(), self.pan, self.zoom);
            let deg = *self.cache.degrees.get(&id).unwrap_or(&0) as f32;
            let r = (node_radius(deg) * self.zoom).clamp(2.2, 12.0);

            // Sub-pixel nodes when zoomed way out: skip unless hub / active.
            let is_center = center == Some(id);
            if lod.skip_tiny && r < 2.2 && !is_center && deg < 6.0 {
                continue;
            }
            if !circle_may_hit_view(screen, r * 1.8, view) {
                // Still allow hover when the pointer is near (expanded cull already covers this).
                continue;
            }

            let hovering = self.hovered == Some(id);
            let is_focus = self.focus_node == Some(id);
            let is_focused_neighbor = focused_neighbors.contains(&id);
            let folder_color = self.cache.colors.get(&id).copied().unwrap_or(muted);
            let base = if is_center {
                crate::theme_apply::mix(accent, folder_color, self.focus_amount)
            } else {
                folder_color
            };
            let alpha = if self.focus_node.is_some() && !is_focus && !is_focused_neighbor {
                (255.0 - 200.0 * self.focus_amount) as u8
            } else {
                255
            };
            let fill = Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), alpha);

            if hovering || (lod.show_glows && is_center && self.hovered.is_none()) {
                node_shapes.push(Shape::circle_filled(
                    screen,
                    r * 1.7,
                    Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), 65),
                ));
            }

            if lod.show_strokes || hovering || (is_center && self.hovered.is_none()) {
                node_shapes.push(Shape::Circle(CircleShape {
                    center: screen,
                    radius: r,
                    fill,
                    stroke: Stroke::new(
                        if hovering || (is_center && self.hovered.is_none()) {
                            2.0_f32
                        } else {
                            1.0_f32
                        },
                        if hovering || (is_center && self.hovered.is_none()) {
                            text
                        } else {
                            Color32::from_rgba_unmultiplied(0, 0, 0, 90)
                        },
                    ),
                }));
            } else {
                node_shapes.push(Shape::circle_filled(screen, r, fill));
            }

            if response.clicked() && hovering {
                open = Some(meta.rel.clone());
            }
        }
        painter.extend(node_shapes);

        // Reveal the hovered note and its direct, visible neighbors.
        if let Some(hid) = self.hovered {
            for id in focused_neighbors
                .iter()
                .copied()
                .chain(std::iter::once(hid))
            {
                if let (Some(meta), Some(&p)) = (vault.index.get(id), self.positions.get(&id)) {
                    let screen = map(p, rect.center(), self.pan, self.zoom);
                    if !circle_may_hit_view(screen, 20.0, view) {
                        continue;
                    }
                    let deg = *self.cache.degrees.get(&id).unwrap_or(&0) as f32;
                    let radius = (node_radius(deg) * self.zoom).clamp(2.2, 12.0);
                    let title = Path::new(&meta.rel)
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| meta.rel.clone());
                    painter.text(
                        screen + Vec2::new(radius + 5.0, -7.0),
                        egui::Align2::LEFT_TOP,
                        title,
                        FontId::monospace((11.5 * self.zoom.sqrt()).clamp(10.0, 15.0)),
                        text,
                    );
                }
            }
        }

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("── graph ──").monospace().color(accent));
            if ui.selectable_label(!self.local_mode, "global").clicked() {
                self.local_mode = false;
            }
            if ui.selectable_label(self.local_mode, "local").clicked() {
                self.local_mode = true;
            }
            if self.local_mode {
                ui.add(egui::Slider::new(&mut self.depth, 1..=4).text("depth"));
            }
            ui.label(
                egui::RichText::new(format!(
                    "{} notes · drag node to move · drag background to pan · scroll zoom · click open",
                    n
                ))
                    .monospace()
                    .size(11.0)
                    .color(muted),
            );
            if self.settle_t > 0.0 {
                ui.label(
                    egui::RichText::new("clustering…")
                        .monospace()
                        .size(11.0)
                        .color(muted),
                );
            }
        });

        open
    }
}

struct PaintLod {
    show_glows: bool,
    show_strokes: bool,
    thin_edges: bool,
    skip_tiny: bool,
}

fn paint_lod(node_count: usize, zoom: f32) -> PaintLod {
    // Large vaults: drop decoration so egui isn't tessellating tens of thousands of shapes.
    let large = node_count > 400;
    let huge = node_count > 1_200;
    let overview = zoom < 0.85 || huge;
    PaintLod {
        show_glows: !overview && node_count <= 600,
        show_strokes: !overview,
        thin_edges: huge || (large && zoom < 0.95),
        skip_tiny: huge || (large && zoom < 0.7),
    }
}

fn map(p: Pos2, center: Pos2, pan: Vec2, zoom: f32) -> Pos2 {
    center + (p.to_vec2() - center.to_vec2()) * zoom + pan
}

fn node_radius(degree: f32) -> f32 {
    3.6 + degree.max(0.0).ln_1p().min(3.0) * 1.15
}

fn node_at_screen(
    cache: &DrawCache,
    positions: &HashMap<NoteId, Pos2>,
    view: Rect,
    pan: Vec2,
    zoom: f32,
    pointer: Pos2,
) -> Option<NoteId> {
    // Nodes with more links are painted last, so prefer them when hit areas overlap.
    cache.draw_order.iter().rev().copied().find(|id| {
        let Some(&position) = positions.get(id) else {
            return false;
        };
        let degree = cache.degrees.get(id).copied().unwrap_or(0) as f32;
        let radius = (node_radius(degree) * zoom).clamp(2.2, 12.0);
        let screen = map(position, view.center(), pan, zoom);
        Rect::from_center_size(screen, Vec2::splat(radius * 2.8)).contains(pointer)
    })
}

fn circle_may_hit_view(center: Pos2, radius: f32, view: Rect) -> bool {
    view.expand(radius).contains(center)
}

fn segment_may_hit_view(a: Pos2, b: Pos2, view: Rect) -> bool {
    // Cheap AABB reject; false positives are fine.
    let min_x = a.x.min(b.x);
    let max_x = a.x.max(b.x);
    let min_y = a.y.min(b.y);
    let max_y = a.y.max(b.y);
    !(max_x < view.left() || min_x > view.right() || max_y < view.top() || min_y > view.bottom())
}

/// Top-level folder name, or `_root` for vault-root notes.
fn cluster_key(rel: &str) -> String {
    let path = Path::new(rel);
    let comps: Vec<_> = path
        .components()
        .filter_map(|c| {
            let s = c.as_os_str().to_string_lossy();
            if s == "." || s.is_empty() {
                None
            } else {
                Some(s.into_owned())
            }
        })
        .collect();
    if comps.len() <= 1 {
        "_root".into()
    } else {
        comps[0].clone()
    }
}

fn group_by_cluster(vault: &Vault, nodes: &[NoteId]) -> HashMap<String, Vec<NoteId>> {
    let mut map: HashMap<String, Vec<NoteId>> = HashMap::new();
    for &id in nodes {
        if let Some(meta) = vault.index.get(id) {
            map.entry(cluster_key(&meta.rel)).or_default().push(id);
        }
    }
    map
}

fn cluster_color(
    tree: &file_tree::DirNode,
    key: &str,
    palette: &crate::theme_apply::ThemePalette,
) -> Color32 {
    if key == "_root" {
        file_tree::root_note_color(palette)
    } else {
        file_tree::color_for_folder(tree, key, palette)
            .unwrap_or_else(|| file_tree::root_note_color(palette))
    }
}

fn build_draw_cache(
    vault: &Vault,
    nodes: &[NoteId],
    palette: &crate::theme_apply::ThemePalette,
) -> DrawCache {
    let visible: HashSet<NoteId> = nodes.iter().copied().collect();
    let mut paths = vault.list_md_rel_paths();
    paths.sort();
    let tree = file_tree::build_tree(&paths);

    let mut degrees = HashMap::with_capacity(nodes.len());
    let mut colors = HashMap::with_capacity(nodes.len());
    let mut folders = HashMap::with_capacity(nodes.len());
    let cluster_members = group_by_cluster(vault, nodes);
    let mut cluster_colors = HashMap::with_capacity(cluster_members.len());
    for key in cluster_members.keys() {
        cluster_colors.insert(key.clone(), cluster_color(&tree, key, palette));
    }
    for &id in nodes {
        degrees.insert(id, vault.graph.degree(id));
        if let Some(meta) = vault.index.get(id) {
            let key = cluster_key(&meta.rel);
            colors.insert(id, cluster_colors[&key]);
            folders.insert(id, key);
        }
    }

    let mut draw_order = nodes.to_vec();
    draw_order.sort_by_key(|id| degrees.get(id).copied().unwrap_or(0));

    // Top ~8% by degree (min 24) kept when edge LOD thins the view.
    let mut by_deg = draw_order.clone();
    by_deg.sort_by_key(|id| std::cmp::Reverse(degrees.get(id).copied().unwrap_or(0)));
    let hub_n = (nodes.len() / 12).clamp(24, 180).min(nodes.len());
    let hubs: HashSet<NoteId> = by_deg.into_iter().take(hub_n).collect();

    let edges = vault.graph.edges_among(&visible);
    let linked = edges.iter().flat_map(|&(a, b)| [a, b]).collect();

    DrawCache {
        palette_name: palette.name.clone(),
        nodes: nodes.to_vec(),
        edges,
        linked,
        degrees,
        colors,
        folders,
        draw_order,
        hubs,
    }
}

/// Linked components fill the middle; notes without links form a quiet outer ring.
fn seed_cluster_layout(
    positions: &mut HashMap<NoteId, Pos2>,
    targets: &mut HashMap<NoteId, Pos2>,
    cache: &DrawCache,
    view: Rect,
) {
    positions.clear();
    targets.clear();
    let mut neighbors: HashMap<NoteId, Vec<NoteId>> = HashMap::with_capacity(cache.nodes.len());
    for &(a, b) in &cache.edges {
        neighbors.entry(a).or_default().push(b);
        neighbors.entry(b).or_default().push(a);
    }
    let mut seen = HashSet::with_capacity(cache.nodes.len());
    let mut components = Vec::new();
    let mut isolated = Vec::new();
    let mut sorted = cache.nodes.clone();
    sorted.sort();
    for id in sorted {
        if !seen.insert(id) {
            continue;
        }
        if !neighbors.contains_key(&id) {
            isolated.push(id);
            continue;
        }
        let mut component = Vec::new();
        let mut stack = vec![id];
        while let Some(node) = stack.pop() {
            component.push(node);
            if let Some(adjacent) = neighbors.get(&node) {
                for &next in adjacent {
                    if seen.insert(next) {
                        stack.push(next);
                    }
                }
            }
        }
        component.sort_by_key(|node| (cache.folders.get(node).cloned(), *node));
        components.push(component);
    }
    components.sort_by_key(|component| std::cmp::Reverse(component.len()));
    let center = view.center();
    let extent = view.width().min(view.height());
    let core_radius = extent * if isolated.is_empty() { 0.36 } else { 0.29 };
    for (index, component) in components.iter().enumerate() {
        let (origin, budget) = if index == 0 {
            (center, core_radius * 0.75)
        } else {
            let angle = index as f32 * 2.399_963_1;
            let radius =
                (core_radius * (0.48 + 0.11 * (index as f32).sqrt())).min(core_radius * 1.15);
            (center + Vec2::angled(angle) * radius, core_radius * 0.23)
        };
        // Give folders neighboring sectors inside a connected component. The
        // link springs can still pull notes across those loose boundaries.
        let mut folders: Vec<String> = component
            .iter()
            .filter_map(|id| cache.folders.get(id).cloned())
            .collect();
        folders.sort();
        folders.dedup();
        let folder_count = folders.len().max(1) as f32;
        let mut folder_offsets: HashMap<String, usize> = HashMap::new();
        for &id in component {
            let key = cache.folders.get(&id).cloned().unwrap_or_default();
            let sector = folders.binary_search(&key).unwrap_or(0) as f32;
            let ordinal = folder_offsets.entry(key).or_default();
            let angle =
                sector / folder_count * std::f32::consts::TAU + *ordinal as f32 * 2.399_963_1;
            let folder_center = if folders.len() > 1 {
                origin + Vec2::angled(sector / folder_count * std::f32::consts::TAU) * budget * 0.45
            } else {
                origin
            };
            let radius = (12.0 * (*ordinal as f32).sqrt()).min(budget * 0.65);
            let point = folder_center + Vec2::angled(angle) * radius;
            positions.insert(id, point);
            targets.insert(id, point);
            *ordinal += 1;
        }
    }

    let mut placed = 0;
    let mut orbit = extent * 0.43;
    while placed < isolated.len() {
        // Add concentric tracks when one circumference cannot fit the notes.
        let capacity = ((std::f32::consts::TAU * orbit / 12.0) as usize).max(1);
        let count = (isolated.len() - placed).min(capacity);
        for index in 0..count {
            let angle = index as f32 / count as f32 * std::f32::consts::TAU
                - std::f32::consts::FRAC_PI_2
                + (placed as f32 * 0.37);
            let point = center + Vec2::angled(angle) * orbit;
            let id = isolated[placed + index];
            positions.insert(id, point);
            targets.insert(id, point);
        }
        placed += count;
        orbit = (orbit - 15.0).max(extent * 0.31);
    }
}

fn settle_clustered(
    positions: &mut HashMap<NoteId, Pos2>,
    targets: &HashMap<NoteId, Pos2>,
    cache: &DrawCache,
    view: Rect,
    dt: f32,
    pinned: Option<NoteId>,
    interactive: bool,
) {
    let visible = &cache.nodes;
    if dt <= 0.0 || visible.is_empty() {
        return;
    }

    let mut forces: HashMap<NoteId, Vec2> = HashMap::with_capacity(visible.len());
    for &id in visible {
        forces.insert(id, Vec2::ZERO);
    }

    // A weak tether preserves the circular composition while links form groups.
    if !interactive {
        for &id in visible {
            if !cache.linked.contains(&id) {
                continue;
            }
            let Some(&target) = targets.get(&id) else {
                continue;
            };
            let Some(&p) = positions.get(&id) else {
                continue;
            };
            *forces.get_mut(&id).unwrap() += (target - p) * 0.025 * dt;
        }
    }

    // Node repulsion via spatial grid — O(n · k) instead of O(n²).
    let cell = if interactive {
        DRAG_REPEL_CUTOFF
    } else {
        REPEL_CUTOFF
    };
    let cutoff2 = cell * cell;
    let mut grid: HashMap<(i32, i32), Vec<NoteId>> = HashMap::new();
    for &id in visible {
        if !interactive && !cache.linked.contains(&id) {
            continue;
        }
        if let Some(&p) = positions.get(&id) {
            let key = ((p.x / cell).floor() as i32, (p.y / cell).floor() as i32);
            grid.entry(key).or_default().push(id);
        }
    }
    for &a in visible {
        if !interactive && !cache.linked.contains(&a) {
            continue;
        }
        let Some(&pa) = positions.get(&a) else {
            continue;
        };
        let cx = (pa.x / cell).floor() as i32;
        let cy = (pa.y / cell).floor() as i32;
        let ka = cache.folders.get(&a);
        for dx in -1..=1 {
            for dy in -1..=1 {
                let Some(bucket) = grid.get(&(cx + dx, cy + dy)) else {
                    continue;
                };
                for &b in bucket {
                    if b <= a {
                        // Each pair once; NoteId ordering is stable.
                        continue;
                    }
                    let Some(&pb) = positions.get(&b) else {
                        continue;
                    };
                    let delta = pa - pb;
                    let dist2 = delta.length_sq();
                    if dist2 > cutoff2 || (!interactive && dist2 < 1e-6) {
                        continue;
                    }
                    let dist = dist2.sqrt().max(10.0);
                    let d = if dist2 < 1e-6 { Vec2::X } else { delta / dist };
                    let same_cluster = match (ka, cache.folders.get(&b)) {
                        (Some(x), Some(y)) => x == y,
                        _ => false,
                    };
                    if dist2 >= 1e-6 {
                        let k = if same_cluster { 9_000.0 } else { 18_000.0 };
                        let f = d * (k / dist2) * dt;
                        *forces.get_mut(&a).unwrap() += f;
                        *forces.get_mut(&b).unwrap() -= f;
                    }
                    if interactive {
                        let ra = node_radius(*cache.degrees.get(&a).unwrap_or(&0) as f32);
                        let rb = node_radius(*cache.degrees.get(&b).unwrap_or(&0) as f32);
                        let overlap = ra + rb + 4.0 - dist2.sqrt();
                        if overlap > 0.0 {
                            let push = d * overlap * 0.55;
                            *forces.get_mut(&a).unwrap() += push;
                            *forces.get_mut(&b).unwrap() -= push;
                        }
                    }
                }
            }
        }
    }

    // Link springs — ideal length shorter inside a cluster, longer across topics.
    for &(a, b) in &cache.edges {
        if !forces.contains_key(&a) || !forces.contains_key(&b) {
            continue;
        }
        let Some(&pa) = positions.get(&a) else {
            continue;
        };
        let Some(&pb) = positions.get(&b) else {
            continue;
        };
        let same = match (cache.folders.get(&a), cache.folders.get(&b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        };
        let mut d = pb - pa;
        let dist = d.length().max(1.0);
        d /= dist;
        let ideal = if same { 52.0 } else { 140.0 };
        let spring = if interactive {
            if pinned == Some(a) || pinned == Some(b) {
                0.35
            } else {
                0.12
            }
        } else {
            0.055
        };
        let f = d * (dist - ideal) * spring * dt;
        if let Some(fa) = forces.get_mut(&a) {
            *fa += f;
        }
        if let Some(fb) = forces.get_mut(&b) {
            *fb -= f;
        }
    }

    let max_step = if interactive {
        10.0
    } else {
        22.0 * dt.max(0.04)
    };
    let pad = 36.0;
    for (id, f) in forces {
        if pinned == Some(id) {
            continue;
        }
        if let Some(p) = positions.get_mut(&id) {
            let step = if f.length() > max_step {
                f.normalized() * max_step
            } else {
                f
            };
            *p += step;
            p.x = p.x.clamp(view.left() + pad, view.right() - pad);
            p.y = p.y.clamp(view.top() + pad, view.bottom() - pad);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault_with(rels: &[&str]) -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().unwrap();
        for rel in rels {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, format!("# {rel}\n[[{}]]\n", rels[0])).unwrap();
        }
        let vault = Vault::open(dir.path()).unwrap();
        (dir, vault)
    }

    #[test]
    fn settle_skips_nodes_without_a_position() {
        let (_dir, vault) = vault_with(&["Home.md", "a/One.md", "a/Two.md"]);
        let visible = vault.graph.node_ids();
        let view = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let mut positions = HashMap::new();
        let mut targets = HashMap::new();
        let palette = test_palette();
        let cache = build_draw_cache(&vault, &visible, &palette);
        seed_cluster_layout(&mut positions, &mut targets, &cache, view);
        // A note renamed/created during the settle window has no position yet.
        let missing = visible[1];
        positions.remove(&missing);

        settle_clustered(&mut positions, &targets, &cache, view, 0.02, None, false);

        assert!(
            !positions.contains_key(&missing),
            "settle must not invent positions"
        );
        for (id, p) in &positions {
            assert!(
                p.x.is_finite() && p.y.is_finite(),
                "{id:?} drifted to {p:?}"
            );
        }
    }

    fn test_palette() -> crate::theme_apply::ThemePalette {
        crate::theme_apply::ThemePalette::from_tokens(&celestine_theme::ThemeTokens::default())
    }

    #[test]
    fn edges_among_matches_visible_filter() {
        let (_dir, vault) = vault_with(&["Home.md", "a/One.md", "a/Two.md"]);
        let all = vault.graph.node_ids();
        let visible: HashSet<_> = all.iter().copied().take(2).collect();
        let edges = vault.graph.edges_among(&visible);
        for (a, b) in edges {
            assert!(visible.contains(&a) && visible.contains(&b));
        }
    }

    #[test]
    fn node_pick_uses_screen_coordinates_and_topmost_node() {
        let lower = NoteId(1);
        let upper = NoteId(2);
        let view = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let position = view.center();
        let positions = HashMap::from([(lower, position), (upper, position)]);
        let cache = DrawCache {
            draw_order: vec![lower, upper],
            degrees: HashMap::from([(lower, 0), (upper, 12)]),
            ..Default::default()
        };
        let pan = Vec2::new(40.0, -20.0);
        assert_eq!(
            node_at_screen(&cache, &positions, view, pan, 2.0, position + pan),
            Some(upper)
        );
        assert_eq!(
            node_at_screen(&cache, &positions, view, pan, 2.0, position),
            None
        );
    }

    #[test]
    fn drag_relaxation_pulls_links_and_separates_unlinked_collisions() {
        let dragged = NoteId(1);
        let linked = NoteId(2);
        let loose_a = NoteId(3);
        let loose_b = NoteId(4);
        let view = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let cache = DrawCache {
            nodes: vec![dragged, linked, loose_a, loose_b],
            edges: vec![(dragged, linked)],
            linked: HashSet::from([dragged, linked]),
            degrees: HashMap::from([(dragged, 1), (linked, 1), (loose_a, 0), (loose_b, 0)]),
            ..Default::default()
        };
        let mut positions = HashMap::from([
            (dragged, Pos2::new(200.0, 200.0)),
            (linked, Pos2::new(400.0, 200.0)),
            (loose_a, Pos2::new(500.0, 200.0)),
            (loose_b, Pos2::new(500.0, 200.0)),
        ]);
        settle_clustered(
            &mut positions,
            &HashMap::new(),
            &cache,
            view,
            0.12,
            Some(dragged),
            true,
        );
        assert_eq!(positions[&dragged], Pos2::new(200.0, 200.0));
        assert!(positions[&linked].x < 400.0, "linked note should follow");
        assert!(
            (positions[&loose_a] - positions[&loose_b]).length() > 0.0,
            "unlinked notes should separate when they collide"
        );
    }

    #[test]
    fn linked_notes_are_inside_isolates_and_nested_folders_share_color() {
        let dir = tempfile::tempdir().unwrap();
        for (rel, body) in [
            ("topic/One.md", "[[Two]]"),
            ("topic/nested/Two.md", "[[One]]"),
            ("topic/nested/Alone.md", "# Alone"),
            ("other/Separate.md", "# Separate"),
        ] {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        let vault = Vault::open(dir.path()).unwrap();
        let visible = vault.graph.node_ids();
        let cache = build_draw_cache(&vault, &visible, &test_palette());
        let view = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 800.0));
        let mut positions = HashMap::new();
        let mut targets = HashMap::new();
        seed_cluster_layout(&mut positions, &mut targets, &cache, view);
        let ids: HashMap<_, _> = visible
            .iter()
            .map(|&id| (vault.index.get(id).unwrap().rel.clone(), id))
            .collect();
        let one = ids["topic/One.md"];
        let two = ids["topic/nested/Two.md"];
        let alone = ids["topic/nested/Alone.md"];
        assert_eq!(cache.colors[&one], cache.colors[&two]);
        assert_eq!(cache.colors[&one], cache.colors[&alone]);
        assert!(
            (positions[&one] - view.center()).length()
                < (positions[&alone] - view.center()).length()
        );
        assert!(
            (positions[&two] - view.center()).length()
                < (positions[&alone] - view.center()).length()
        );
        assert!(node_radius(20.0) < node_radius(0.0) * 2.0);
    }

    #[test]
    fn spatial_settle_scales_past_pairwise_limit() {
        // Build a denser synthetic vault and ensure one settle step stays finite.
        let dir = tempfile::tempdir().unwrap();
        for i in 0..120 {
            let folder = if i % 3 == 0 {
                "alpha"
            } else if i % 3 == 1 {
                "beta"
            } else {
                "gamma"
            };
            let path = dir.path().join(folder).join(format!("n{i}.md"));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let link = if i == 0 {
                String::new()
            } else {
                "[[n0]]".to_string()
            };
            std::fs::write(&path, format!("# n{i}\n{link}\n")).unwrap();
        }
        let vault = Vault::open(dir.path()).unwrap();
        let visible = vault.graph.node_ids();
        assert!(visible.len() >= 100);
        let view = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 800.0));
        let mut positions = HashMap::new();
        let mut targets = HashMap::new();
        let palette = test_palette();
        let cache = build_draw_cache(&vault, &visible, &palette);
        seed_cluster_layout(&mut positions, &mut targets, &cache, view);
        settle_clustered(&mut positions, &targets, &cache, view, 0.02, None, false);
        assert_eq!(positions.len(), visible.len());
    }

    #[test]
    fn settle_step_stays_interactive_on_large_vault() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..800 {
            let folder = match i % 5 {
                0 => "a",
                1 => "b",
                2 => "c",
                3 => "d",
                _ => "e",
            };
            let path = dir.path().join(folder).join(format!("n{i}.md"));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let body = if i == 0 {
                "# n0\n".to_string()
            } else {
                format!("# n{i}\n[[n0]]\n[[n{}]]\n", i - 1)
            };
            std::fs::write(&path, body).unwrap();
        }
        let vault = Vault::open(dir.path()).unwrap();
        let visible = vault.graph.node_ids();
        assert!(visible.len() >= 800);
        let view = Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 900.0));
        let mut positions = HashMap::new();
        let mut targets = HashMap::new();
        let palette = test_palette();
        let cache = build_draw_cache(&vault, &visible, &palette);
        seed_cluster_layout(&mut positions, &mut targets, &cache, view);

        let start = std::time::Instant::now();
        for _ in 0..SIM_ITERS_SMALL {
            settle_clustered(&mut positions, &targets, &cache, view, 0.02, None, false);
        }
        let elapsed = start.elapsed();
        // Old O(n²) repulsion routinely blew past this on ~800 notes in debug builds.
        assert!(
            elapsed < std::time::Duration::from_millis(400),
            "settle too slow: {elapsed:?}"
        );

        let drag_start = std::time::Instant::now();
        settle_clustered(
            &mut positions,
            &targets,
            &cache,
            view,
            0.12,
            visible.first().copied(),
            true,
        );
        let drag_elapsed = drag_start.elapsed();
        assert!(
            drag_elapsed < std::time::Duration::from_millis(400),
            "drag relaxation too slow: {drag_elapsed:?}"
        );
    }
}
