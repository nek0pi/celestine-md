//! `.canvas` files: the open JSON Canvas 1.0 format (<https://jsoncanvas.org>)
//! plus a Celestine-only `celestine` key holding freehand ink.
//!
//! Readers that only know JSON Canvas see valid `nodes` / `edges`; arrows that
//! touch a drawing live in `celestine.edges` so the standard lists never point
//! at ids those readers cannot resolve.

use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Canvas {
    #[serde(default)]
    pub nodes: Vec<CanvasNode>,
    #[serde(default)]
    pub edges: Vec<CanvasEdge>,
    #[serde(default, skip_serializing_if = "CelestineExt::is_empty")]
    pub celestine: CelestineExt,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CelestineExt {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drawings: Vec<Drawing>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<CanvasEdge>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl CelestineExt {
    fn is_empty(&self) -> bool {
        self.drawings.is_empty() && self.edges.is_empty() && self.extra.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Text,
    File,
    Link,
    Group,
    Other,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CanvasNode {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(serialize_with = "ser_round")]
    pub x: f32,
    #[serde(serialize_with = "ser_round")]
    pub y: f32,
    #[serde(serialize_with = "ser_round")]
    pub width: f32,
    #[serde(serialize_with = "ser_round")]
    pub height: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subpath: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl CanvasNode {
    fn base(kind: &str, x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            id: new_id(),
            kind: kind.into(),
            x,
            y,
            width,
            height,
            color: None,
            text: None,
            file: None,
            subpath: None,
            url: None,
            label: None,
            extra: Map::new(),
        }
    }

    pub fn new_text(x: f32, y: f32, width: f32, height: f32, text: &str) -> Self {
        let mut node = Self::base("text", x, y, width, height);
        node.text = Some(text.into());
        node
    }

    pub fn new_file(x: f32, y: f32, width: f32, height: f32, file: &str) -> Self {
        let mut node = Self::base("file", x, y, width, height);
        node.file = Some(file.into());
        node
    }

    pub fn node_kind(&self) -> NodeKind {
        match self.kind.as_str() {
            "text" => NodeKind::Text,
            "file" => NodeKind::File,
            "link" => NodeKind::Link,
            "group" => NodeKind::Group,
            _ => NodeKind::Other,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeEnd {
    None,
    Arrow,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasEdge {
    pub id: String,
    pub from_node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_side: Option<Side>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_end: Option<EdgeEnd>,
    pub to_node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_side: Option<Side>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_end: Option<EdgeEnd>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl CanvasEdge {
    pub fn new(from: &str, from_side: Side, to: &str, to_side: Side) -> Self {
        Self {
            id: new_id(),
            from_node: from.into(),
            from_side: Some(from_side),
            from_end: None,
            to_node: to.into(),
            to_side: Some(to_side),
            to_end: None,
            color: None,
            label: None,
            extra: Map::new(),
        }
    }

    /// Spec defaults: no head at the start, an arrow at the end.
    pub fn arrow_at_end(&self) -> bool {
        self.to_end.unwrap_or(EdgeEnd::Arrow) == EdgeEnd::Arrow
    }

    pub fn arrow_at_start(&self) -> bool {
        self.from_end.unwrap_or(EdgeEnd::None) == EdgeEnd::Arrow
    }
}

/// A freehand ink stroke. Each point is `[x, y, pressure]` in canvas units;
/// pressure is 0..=1 and scales `size`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Drawing {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub size: f32,
    #[serde(default)]
    pub points: Vec<[f32; 3]>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Drawing {
    pub fn new(size: f32, color: Option<String>, points: Vec<[f32; 3]>) -> Self {
        Self {
            id: new_id(),
            color,
            size,
            points,
            extra: Map::new(),
        }
    }

    /// `(min_x, min_y, max_x, max_y)` including half the brush size.
    pub fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let first = self.points.first()?;
        let mut b = (first[0], first[1], first[0], first[1]);
        for p in &self.points {
            b.0 = b.0.min(p[0]);
            b.1 = b.1.min(p[1]);
            b.2 = b.2.max(p[0]);
            b.3 = b.3.max(p[1]);
        }
        let pad = self.size * 0.5;
        Some((b.0 - pad, b.1 - pad, b.2 + pad, b.3 + pad))
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        for p in &mut self.points {
            p[0] += dx;
            p[1] += dy;
        }
    }
}

fn ser_round<S: Serializer>(v: &f32, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_i64(v.round() as i64)
}

/// A random 16-hex-digit id, the shape JSON Canvas files commonly use.
pub fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    if let Ok(t) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        h.write_u128(t.as_nanos());
    }
    format!("{:016x}", h.finish())
}

impl Canvas {
    /// Parse a canvas; an empty / whitespace file is an empty canvas.
    pub fn load(text: &str) -> Result<Self, serde_json::Error> {
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        let mut canvas: Self = serde_json::from_str(text)?;
        canvas.normalize();
        Ok(canvas)
    }

    pub fn to_pretty_string(&self) -> String {
        let mut canvas = self.clone();
        canvas.normalize();
        let mut out = Vec::new();
        let fmt = serde_json::ser::PrettyFormatter::with_indent(b"\t");
        let mut ser = serde_json::Serializer::with_formatter(&mut out, fmt);
        if canvas.serialize(&mut ser).is_err() {
            return String::from("{\"nodes\":[],\"edges\":[]}");
        }
        String::from_utf8(out).unwrap_or_default()
    }

    /// Drop edges with dangling endpoints and file each remaining edge in the
    /// list matching its endpoints (standard nodes vs. drawings).
    pub fn normalize(&mut self) {
        let node_ids: HashSet<&str> = self.nodes.iter().map(|n| n.id.as_str()).collect();
        let drawing_ids: HashSet<&str> = self
            .celestine
            .drawings
            .iter()
            .map(|d| d.id.as_str())
            .collect();
        let all: Vec<CanvasEdge> = self
            .edges
            .drain(..)
            .chain(self.celestine.edges.drain(..))
            .collect();
        let mut seen = HashSet::new();
        for edge in all {
            let known = |id: &str| node_ids.contains(id) || drawing_ids.contains(id);
            if !known(&edge.from_node) || !known(&edge.to_node) || !seen.insert(edge.id.clone()) {
                continue;
            }
            if node_ids.contains(edge.from_node.as_str()) && node_ids.contains(edge.to_node.as_str())
            {
                self.edges.push(edge);
            } else {
                self.celestine.edges.push(edge);
            }
        }
    }

    /// Every edge, standard ones first.
    pub fn all_edges(&self) -> impl Iterator<Item = &CanvasEdge> {
        self.edges.iter().chain(self.celestine.edges.iter())
    }

    pub fn push_edge(&mut self, edge: CanvasEdge) {
        self.celestine.edges.push(edge);
        self.normalize();
    }

    /// Remove a node or drawing and every edge touching it.
    pub fn remove_object(&mut self, id: &str) {
        self.nodes.retain(|n| n.id != id);
        self.celestine.drawings.retain(|d| d.id != id);
        self.remove_edges_touching(id);
    }

    pub fn remove_edge(&mut self, id: &str) {
        self.edges.retain(|e| e.id != id);
        self.celestine.edges.retain(|e| e.id != id);
    }

    fn remove_edges_touching(&mut self, id: &str) {
        let keep = |e: &CanvasEdge| e.from_node != id && e.to_node != id;
        self.edges.retain(keep);
        self.celestine.edges.retain(keep);
    }

    /// Point file nodes at a moved file or folder. Returns whether anything changed.
    pub fn rename_file_refs(&mut self, old_rel: &str, new_rel: &str, is_dir: bool) -> bool {
        let mut changed = false;
        for node in &mut self.nodes {
            let Some(file) = node.file.as_mut() else {
                continue;
            };
            if file == old_rel {
                *file = new_rel.to_string();
                changed = true;
            } else if is_dir {
                if let Some(rest) = file.strip_prefix(&format!("{old_rel}/")) {
                    *file = format!("{new_rel}/{rest}");
                    changed = true;
                }
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"{
        "nodes": [
            {"id":"a1","type":"text","x":-120,"y":40,"width":250,"height":60,"text":"Hello","color":"1"},
            {"id":"b2","type":"file","x":200,"y":0,"width":400,"height":400,"file":"Notes/Idea.md","subpath":"#Top"},
            {"id":"c3","type":"link","x":0,"y":500,"width":300,"height":200,"url":"https://example.com"},
            {"id":"g4","type":"group","x":-200,"y":-200,"width":900,"height":900,"label":"Box","background":"bg.png"}
        ],
        "edges": [
            {"id":"e1","fromNode":"a1","fromSide":"right","toNode":"b2","toSide":"left","toEnd":"arrow","label":"see"}
        ]
    }"##;

    #[test]
    fn round_trips_spec_sample() {
        let canvas = Canvas::load(SAMPLE).unwrap();
        assert_eq!(canvas.nodes.len(), 4);
        assert_eq!(canvas.edges.len(), 1);
        assert_eq!(canvas.nodes[1].node_kind(), NodeKind::File);
        assert_eq!(canvas.edges[0].from_side, Some(Side::Right));
        let again = Canvas::load(&canvas.to_pretty_string()).unwrap();
        assert_eq!(canvas, again);
        assert!(!canvas.to_pretty_string().contains("celestine"));
    }

    #[test]
    fn preserves_unknown_fields() {
        let text = r#"{"nodes":[{"id":"a","type":"text","x":0,"y":0,"width":10,"height":10,"text":"t","futureField":{"k":1}}],"edges":[],"metadata":{"v":2}}"#;
        let out = Canvas::load(text).unwrap().to_pretty_string();
        let value: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["metadata"]["v"], 2);
        assert_eq!(value["nodes"][0]["futureField"]["k"], 1);
    }

    #[test]
    fn drops_dangling_edges_and_routes_drawing_edges() {
        let mut canvas = Canvas::load(
            r#"{"nodes":[{"id":"a","type":"text","x":0,"y":0,"width":10,"height":10}],
               "edges":[{"id":"x","fromNode":"a","toNode":"missing"}]}"#,
        )
        .unwrap();
        assert!(canvas.edges.is_empty());
        let drawing = Drawing::new(4.0, None, vec![[0.0, 0.0, 1.0], [10.0, 5.0, 0.5]]);
        let did = drawing.id.clone();
        canvas.celestine.drawings.push(drawing);
        canvas.push_edge(CanvasEdge::new("a", Side::Right, &did, Side::Left));
        assert!(canvas.edges.is_empty());
        assert_eq!(canvas.celestine.edges.len(), 1);
        let again = Canvas::load(&canvas.to_pretty_string()).unwrap();
        assert_eq!(again.celestine.edges.len(), 1);
        canvas.remove_object(&did);
        assert_eq!(canvas.all_edges().count(), 0);
    }

    #[test]
    fn empty_file_is_empty_canvas() {
        assert_eq!(Canvas::load("  \n").unwrap(), Canvas::default());
    }

    #[test]
    fn rename_updates_file_refs() {
        let mut canvas = Canvas::load(SAMPLE).unwrap();
        assert!(canvas.rename_file_refs("Notes", "Archive", true));
        assert_eq!(canvas.nodes[1].file.as_deref(), Some("Archive/Idea.md"));
        assert!(!canvas.rename_file_refs("Other.md", "X.md", false));
    }

    #[test]
    fn ids_are_unique_hex() {
        let a = new_id();
        assert_eq!(a.len(), 16);
        assert_ne!(a, new_id());
    }
}
