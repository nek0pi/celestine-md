use crate::file_tree;
use celestine_core::{NoteId, Vault};
use egui::epaint::CircleShape;
use egui::{pos2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Force layout runs briefly after open / topology change, then freezes.
const SETTLE_SECS: f32 = 1.6;
const SIM_ITERS_SMALL: usize = 4;
/// Repulsion becomes negligible beyond this distance; spatial grid uses it as cell size.
const REPEL_CUTOFF: f32 = 160.0;

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
    /// World-space anchors for topic clusters (top-level folder).
    cluster_centers: HashMap<String, Pos2>,
    /// Rebuilt only when topology / mode changes — avoids per-frame tree walks.
    cache: DrawCache,
}

#[derive(Default)]
struct DrawCache {
    palette_name: String,
    nodes: Vec<NoteId>,
    edges: Vec<(NoteId, NoteId)>,
    degrees: HashMap<NoteId, usize>,
    colors: HashMap<NoteId, Color32>,
    cluster_members: HashMap<String, Vec<NoteId>>,
    cluster_colors: HashMap<String, Color32>,
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
        self.cluster_centers.clear();
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

        if response.dragged() {
            self.pan += response.drag_delta();
        }
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

        if topology_changed {
            seed_cluster_layout(
                &mut self.positions,
                &mut self.cluster_centers,
                vault,
                &nodes,
                rect,
            );
            self.initialized = true;
            self.settle_t = SETTLE_SECS;
            self.last_node_count = nodes.len();
            self.last_local = self.local_mode;
            self.last_depth = self.depth;
            self.last_center = center;
        }
        if topology_changed || theme_changed {
            self.cache = build_draw_cache(vault, &nodes, palette);
        }

        if self.settle_t > 0.0 {
            let dt = ui.input(|i| i.stable_dt).clamp(1.0 / 120.0, 1.0 / 30.0);
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
                    &mut self.cluster_centers,
                    vault,
                    &self.cache,
                    rect,
                    dt * 1.8 * strength,
                );
            }
            self.settle_t = (self.settle_t - dt).max(0.0);
            ui.ctx().request_repaint();
        }

        let n = self.cache.nodes.len();
        let lod = paint_lod(n, self.zoom);
        let view_pad = 48.0;
        let view = rect.expand(view_pad);

        // --- cluster halos + topic labels (behind edges/nodes) ---
        if lod.show_cluster_halos {
            let mut halo_shapes: Vec<Shape> = Vec::new();
            for (key, members) in &self.cache.cluster_members {
                if members.len() < 2 {
                    continue;
                }
                let Some(centroid) = cluster_centroid(&self.positions, members) else {
                    continue;
                };
                let screen = map(centroid, rect.center(), self.pan, self.zoom);
                let spread = cluster_spread(&self.positions, members).max(40.0) * self.zoom;
                // Skip halos fully outside the viewport.
                if !circle_may_hit_view(screen, spread * 0.85, view) {
                    continue;
                }
                let color = self
                    .cache
                    .cluster_colors
                    .get(key)
                    .copied()
                    .unwrap_or(muted);
                halo_shapes.push(Shape::circle_filled(
                    screen,
                    spread * 0.85,
                    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 22),
                ));
                halo_shapes.push(Shape::circle_stroke(
                    screen,
                    spread * 0.85,
                    Stroke::new(
                        1.0_f32,
                        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 50),
                    ),
                ));
                if lod.show_cluster_labels {
                    let label = if key == "_root" {
                        "vault root".to_string()
                    } else {
                        key.clone()
                    };
                    painter.text(
                        screen + Vec2::new(0.0, -spread * 0.85 - 6.0),
                        egui::Align2::CENTER_BOTTOM,
                        label,
                        FontId::monospace((12.0 * self.zoom.sqrt()).clamp(11.0, 16.0)),
                        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 200),
                    );
                }
            }
            painter.extend(halo_shapes);
        }

        // --- edges ---
        let active_neighbors: HashSet<NoteId> = center
            .map(|c| vault.graph.neighbors(c).into_iter().collect())
            .unwrap_or_default();
        let mut edge_shapes: Vec<Shape> = Vec::with_capacity(self.cache.edges.len().min(8_192));
        for &(a, b) in &self.cache.edges {
            if lod.thin_edges {
                let hub_edge = self.cache.hubs.contains(&a) || self.cache.hubs.contains(&b);
                let touches_active = center == Some(a)
                    || center == Some(b)
                    || active_neighbors.contains(&a)
                    || active_neighbors.contains(&b);
                if !hub_edge && !touches_active {
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
            let touches_active = center == Some(a)
                || center == Some(b)
                || active_neighbors.contains(&a)
                || active_neighbors.contains(&b);
            let (w, alpha) = if touches_active {
                (1.6_f32, 160_u8)
            } else {
                (1.0_f32, if lod.thin_edges { 40_u8 } else { 55_u8 })
            };
            edge_shapes.push(Shape::line_segment(
                [sa, sb],
                Stroke::new(
                    w,
                    Color32::from_rgba_unmultiplied(muted.r(), muted.g(), muted.b(), alpha),
                ),
            ));
        }
        painter.extend(edge_shapes);

        // --- nodes ---
        self.hovered = None;
        let pointer = response.hover_pos();
        let show_all_labels = lod.show_all_labels;

        let mut node_shapes: Vec<Shape> = Vec::with_capacity(self.cache.draw_order.len() * 2);
        let mut label_jobs: Vec<(Pos2, String, Color32, f32)> = Vec::new();

        for &id in &self.cache.draw_order {
            let Some(meta) = vault.index.get(id) else {
                continue;
            };
            let Some(&p) = self.positions.get(&id) else {
                continue;
            };
            let screen = map(p, rect.center(), self.pan, self.zoom);
            let deg = *self.cache.degrees.get(&id).unwrap_or(&0) as f32;
            let r = ((5.5 + deg.sqrt() * 4.2) * self.zoom).clamp(4.0, 22.0);

            // Sub-pixel nodes when zoomed way out: skip unless hub / active.
            let is_center = center == Some(id);
            if lod.skip_tiny && r < 2.2 && !is_center && deg < 6.0 {
                continue;
            }
            if !circle_may_hit_view(screen, r * 1.8, view) {
                // Still allow hover when the pointer is near (expanded cull already covers this).
                continue;
            }

            let fill = if is_center {
                accent
            } else {
                self.cache
                    .colors
                    .get(&id)
                    .copied()
                    .unwrap_or(muted)
            };

            let hit = Rect::from_center_size(screen, Vec2::splat(r * 2.8));
            let hovering = pointer.is_some_and(|pos| hit.contains(pos));
            if hovering {
                self.hovered = Some(id);
            }

            if lod.show_glows && (is_center || hovering || deg >= 3.0) {
                node_shapes.push(Shape::circle_filled(
                    screen,
                    r * 1.7,
                    Color32::from_rgba_unmultiplied(fill.r(), fill.g(), fill.b(), 40),
                ));
            }

            if lod.show_strokes || is_center || hovering {
                node_shapes.push(Shape::Circle(CircleShape {
                    center: screen,
                    radius: r,
                    fill,
                    stroke: Stroke::new(
                        if is_center || hovering {
                            2.0_f32
                        } else {
                            1.0_f32
                        },
                        if is_center {
                            text
                        } else {
                            Color32::from_rgba_unmultiplied(0, 0, 0, 90)
                        },
                    ),
                }));
            } else {
                node_shapes.push(Shape::circle_filled(screen, r, fill));
            }

            let label_here = show_all_labels
                || hovering
                || is_center
                || (lod.show_hub_labels && deg >= 4.0)
                || self.zoom >= 1.4;
            if label_here {
                let title = Path::new(&meta.rel)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| meta.rel.clone());
                let label_c = if hovering || is_center {
                    text
                } else {
                    let folder_c = fill;
                    Color32::from_rgba_unmultiplied(
                        folder_c.r().saturating_add(40).min(255),
                        folder_c.g().saturating_add(40).min(255),
                        folder_c.b().saturating_add(40).min(255),
                        230,
                    )
                };
                label_jobs.push((
                    screen + Vec2::new(r + 5.0, -7.0),
                    title,
                    label_c,
                    (11.5 * self.zoom.sqrt()).clamp(10.0, 15.0),
                ));
            }

            if response.clicked() && hovering {
                open = Some(meta.rel.clone());
            }
        }
        painter.extend(node_shapes);
        for (pos, title, color, size) in label_jobs {
            painter.text(
                pos,
                egui::Align2::LEFT_TOP,
                title,
                FontId::monospace(size),
                color,
            );
        }

        // Hover card
        if let Some(hid) = self.hovered {
            if let Some(meta) = vault.index.get(hid) {
                if let Some(&p) = self.positions.get(&hid) {
                    let screen = map(p, rect.center(), self.pan, self.zoom);
                    let deg = *self.cache.degrees.get(&hid).unwrap_or(&0);
                    let tip = format!("{}  ·  {} links", meta.rel, deg);
                    painter.text(
                        screen + Vec2::new(0.0, 18.0 * self.zoom),
                        egui::Align2::CENTER_TOP,
                        tip,
                        FontId::monospace(11.0),
                        muted,
                    );
                }
            }
        }

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("── graph ──").monospace().color(accent));
            if ui
                .selectable_label(!self.local_mode, "global")
                .clicked()
            {
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
                    "{} notes · drag pan · scroll zoom · click open",
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
    show_cluster_halos: bool,
    show_cluster_labels: bool,
    show_glows: bool,
    show_strokes: bool,
    show_all_labels: bool,
    show_hub_labels: bool,
    thin_edges: bool,
    skip_tiny: bool,
}

fn paint_lod(node_count: usize, zoom: f32) -> PaintLod {
    // Large vaults: drop decoration so egui isn't tessellating tens of thousands of shapes.
    let large = node_count > 400;
    let huge = node_count > 1_200;
    let overview = zoom < 0.85 || huge;
    PaintLod {
        show_cluster_halos: !huge && zoom >= 0.55,
        show_cluster_labels: !large && zoom >= 0.75,
        show_glows: !overview && node_count <= 600,
        show_strokes: !overview,
        show_all_labels: zoom >= 1.05 && node_count <= 40,
        show_hub_labels: !huge && zoom >= 0.7,
        thin_edges: huge || (large && zoom < 0.95),
        skip_tiny: huge || (large && zoom < 0.7),
    }
}

fn map(p: Pos2, center: Pos2, pan: Vec2, zoom: f32) -> Pos2 {
    center + (p.to_vec2() - center.to_vec2()) * zoom + pan
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

fn cluster_centroid(positions: &HashMap<NoteId, Pos2>, members: &[NoteId]) -> Option<Pos2> {
    let mut sum = Vec2::ZERO;
    let mut n = 0.0;
    for id in members {
        if let Some(p) = positions.get(id) {
            sum += p.to_vec2();
            n += 1.0;
        }
    }
    if n < 1.0 {
        None
    } else {
        Some(Pos2::new(sum.x / n, sum.y / n))
    }
}

fn cluster_spread(positions: &HashMap<NoteId, Pos2>, members: &[NoteId]) -> f32 {
    let Some(c) = cluster_centroid(positions, members) else {
        return 40.0;
    };
    let mut max_d = 0.0_f32;
    for id in members {
        if let Some(p) = positions.get(id) {
            max_d = max_d.max((*p - c).length());
        }
    }
    max_d.max(30.0)
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
    for &id in nodes {
        degrees.insert(id, vault.graph.degree(id));
        if let Some(meta) = vault.index.get(id) {
            colors.insert(id, file_tree::color_for_rel(&tree, &meta.rel, palette));
        }
    }

    let cluster_members = group_by_cluster(vault, nodes);
    let mut cluster_colors = HashMap::with_capacity(cluster_members.len());
    for key in cluster_members.keys() {
        cluster_colors.insert(key.clone(), cluster_color(&tree, key, palette));
    }

    let mut draw_order = nodes.to_vec();
    draw_order.sort_by_key(|id| degrees.get(id).copied().unwrap_or(0));

    // Top ~8% by degree (min 24) kept when edge LOD thins the view.
    let mut by_deg = draw_order.clone();
    by_deg.sort_by_key(|id| std::cmp::Reverse(degrees.get(id).copied().unwrap_or(0)));
    let hub_n = (nodes.len() / 12).clamp(24, 180).min(nodes.len());
    let hubs: HashSet<NoteId> = by_deg.into_iter().take(hub_n).collect();

    DrawCache {
        palette_name: palette.name.clone(),
        nodes: nodes.to_vec(),
        edges: vault.graph.edges_among(&visible),
        degrees,
        colors,
        cluster_members,
        cluster_colors,
        draw_order,
        hubs,
    }
}

/// Place each topic on a ring, notes jittered near their topic anchor.
fn seed_cluster_layout(
    positions: &mut HashMap<NoteId, Pos2>,
    cluster_centers: &mut HashMap<String, Pos2>,
    vault: &Vault,
    visible: &[NoteId],
    view: Rect,
) {
    positions.retain(|id, _| vault.index.get(*id).is_some());
    let groups = group_by_cluster(vault, visible);
    let mut keys: Vec<String> = groups.keys().cloned().collect();
    keys.sort();
    let n = keys.len().max(1) as f32;
    let center = view.center();
    let ring = (view.width().min(view.height()) * 0.28).clamp(120.0, 280.0);

    cluster_centers.clear();
    for (i, key) in keys.iter().enumerate() {
        let ang = (i as f32) / n * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let c = pos2(center.x + ring * ang.cos(), center.y + ring * ang.sin());
        cluster_centers.insert(key.clone(), c);

        let members = &groups[key];
        let m = members.len().max(1) as f32;
        let local_r = 18.0 + m.sqrt() * 14.0;
        for (j, id) in members.iter().enumerate() {
            if positions.contains_key(id) {
                continue;
            }
            let a = (j as f32) / m * std::f32::consts::TAU + i as f32 * 0.3;
            let jitter = 0.35 + (j as f32 * 0.17) % 0.65;
            positions.insert(
                *id,
                pos2(
                    c.x + local_r * jitter * a.cos(),
                    c.y + local_r * jitter * a.sin(),
                ),
            );
        }
    }
}

fn settle_clustered(
    positions: &mut HashMap<NoteId, Pos2>,
    cluster_centers: &mut HashMap<String, Pos2>,
    vault: &Vault,
    cache: &DrawCache,
    view: Rect,
    dt: f32,
) {
    let visible = &cache.nodes;
    if dt <= 0.0 || visible.is_empty() {
        return;
    }

    // Refresh cluster anchors from member centroids (smooth follow).
    for (key, members) in &cache.cluster_members {
        if let Some(c) = cluster_centroid(positions, members) {
            let entry = cluster_centers.entry(key.clone()).or_insert(c);
            *entry = Pos2::new(entry.x * 0.7 + c.x * 0.3, entry.y * 0.7 + c.y * 0.3);
        }
    }

    let mut forces: HashMap<NoteId, Vec2> = HashMap::with_capacity(visible.len());
    for &id in visible {
        forces.insert(id, Vec2::ZERO);
    }

    let view_c = view.center();

    // Precompute cluster keys once per settle step.
    let mut keys: HashMap<NoteId, String> = HashMap::with_capacity(visible.len());
    for &id in visible {
        if let Some(meta) = vault.index.get(id) {
            keys.insert(id, cluster_key(&meta.rel));
        }
    }

    // Pull each note toward its topic cluster (forms the readable groups).
    for &id in visible {
        let Some(key) = keys.get(&id) else {
            continue;
        };
        let Some(&anchor) = cluster_centers.get(key) else {
            continue;
        };
        let Some(&p) = positions.get(&id) else {
            continue;
        };
        *forces.get_mut(&id).unwrap() += (anchor - p) * 0.22 * dt;
    }

    // Soft gravity to keep the whole map framed.
    for &id in visible {
        let Some(&p) = positions.get(&id) else {
            continue;
        };
        *forces.get_mut(&id).unwrap() += (view_c - p) * 0.03 * dt;
    }

    // Cluster anchors repel each other so topics stay separated.
    let anchors: Vec<(String, Pos2)> = cluster_centers
        .iter()
        .map(|(k, p)| (k.clone(), *p))
        .collect();
    for i in 0..anchors.len() {
        for j in (i + 1)..anchors.len() {
            let (_, pa) = anchors[i];
            let (_, pb) = anchors[j];
            let mut d = pa - pb;
            let dist = d.length().max(40.0);
            d /= dist;
            let push = d * (55_000.0 / (dist * dist)) * dt;
            if let Some(ma) = cache.cluster_members.get(&anchors[i].0) {
                let share = push / ma.len().max(1) as f32;
                for id in ma {
                    if let Some(f) = forces.get_mut(id) {
                        *f += share;
                    }
                }
            }
            if let Some(mb) = cache.cluster_members.get(&anchors[j].0) {
                let share = push / mb.len().max(1) as f32;
                for id in mb {
                    if let Some(f) = forces.get_mut(id) {
                        *f -= share;
                    }
                }
            }
        }
    }

    // Node repulsion via spatial grid — O(n · k) instead of O(n²).
    let cell = REPEL_CUTOFF;
    let cutoff2 = REPEL_CUTOFF * REPEL_CUTOFF;
    let mut grid: HashMap<(i32, i32), Vec<NoteId>> = HashMap::new();
    for &id in visible {
        if let Some(&p) = positions.get(&id) {
            let key = ((p.x / cell).floor() as i32, (p.y / cell).floor() as i32);
            grid.entry(key).or_default().push(id);
        }
    }
    for &a in visible {
        let Some(&pa) = positions.get(&a) else {
            continue;
        };
        let cx = (pa.x / cell).floor() as i32;
        let cy = (pa.y / cell).floor() as i32;
        let ka = keys.get(&a);
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
                    if dist2 > cutoff2 || dist2 < 1e-6 {
                        continue;
                    }
                    let dist = dist2.sqrt().max(10.0);
                    let d = delta / dist;
                    let same_cluster = match (ka, keys.get(&b)) {
                        (Some(x), Some(y)) => x == y,
                        _ => false,
                    };
                    let k = if same_cluster { 9_000.0 } else { 18_000.0 };
                    let f = d * (k / dist2) * dt;
                    *forces.get_mut(&a).unwrap() += f;
                    *forces.get_mut(&b).unwrap() -= f;
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
        let same = match (keys.get(&a), keys.get(&b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        };
        let mut d = pb - pa;
        let dist = d.length().max(1.0);
        d /= dist;
        let ideal = if same { 52.0 } else { 140.0 };
        let f = d * (dist - ideal) * 0.055 * dt;
        if let Some(fa) = forces.get_mut(&a) {
            *fa += f;
        }
        if let Some(fb) = forces.get_mut(&b) {
            *fb -= f;
        }
    }

    let max_step = 22.0 * dt.max(0.04);
    let pad = 36.0;
    for (id, f) in forces {
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
        let mut centers = HashMap::new();
        seed_cluster_layout(&mut positions, &mut centers, &vault, &visible, view);
        let palette = test_palette();
        let cache = build_draw_cache(&vault, &visible, &palette);
        // A note renamed/created during the settle window has no position yet.
        let missing = visible[1];
        positions.remove(&missing);

        settle_clustered(&mut positions, &mut centers, &vault, &cache, view, 0.02);

        assert!(!positions.contains_key(&missing), "settle must not invent positions");
        for (id, p) in &positions {
            assert!(p.x.is_finite() && p.y.is_finite(), "{id:?} drifted to {p:?}");
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
        let mut centers = HashMap::new();
        seed_cluster_layout(&mut positions, &mut centers, &vault, &visible, view);
        let palette = test_palette();
        let cache = build_draw_cache(&vault, &visible, &palette);
        settle_clustered(&mut positions, &mut centers, &vault, &cache, view, 0.02);
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
        let mut centers = HashMap::new();
        seed_cluster_layout(&mut positions, &mut centers, &vault, &visible, view);
        let palette = test_palette();
        let cache = build_draw_cache(&vault, &visible, &palette);

        let start = std::time::Instant::now();
        for _ in 0..SIM_ITERS_SMALL {
            settle_clustered(&mut positions, &mut centers, &vault, &cache, view, 0.02);
        }
        let elapsed = start.elapsed();
        // Old O(n²) repulsion routinely blew past this on ~800 notes in debug builds.
        assert!(
            elapsed < std::time::Duration::from_millis(400),
            "settle too slow: {elapsed:?}"
        );
    }
}
