use crate::index::{NoteId, VaultIndex};
use crate::resolve::{resolve_link, ResolvedLink};
use petgraph::graph::{NodeIndex, UnGraph};
use petgraph::visit::EdgeRef;
use std::collections::{HashMap, HashSet, VecDeque};

pub type NodeId = NoteId;

#[derive(Debug, Default)]
pub struct LinkGraph {
    graph: UnGraph<NoteId, ()>,
    note_to_node: HashMap<NoteId, NodeIndex>,
}

impl LinkGraph {
    pub fn rebuild(index: &VaultIndex) -> Self {
        let mut g = LinkGraph::default();
        for note in index.notes() {
            let idx = g.graph.add_node(note.id);
            g.note_to_node.insert(note.id, idx);
        }

        let mut edges: HashSet<(NoteId, NoteId)> = HashSet::new();
        for note in index.notes() {
            for wl in &note.wikilinks {
                if let ResolvedLink::Note(target) = resolve_link(index, &note.rel, &wl.target) {
                    if target != note.id {
                        let a = note.id.min(target);
                        let b = note.id.max(target);
                        edges.insert((a, b));
                    }
                }
            }
            for dest in &note.md_link_targets {
                if let ResolvedLink::Note(target) = resolve_link(index, &note.rel, dest) {
                    if target != note.id {
                        let a = note.id.min(target);
                        let b = note.id.max(target);
                        edges.insert((a, b));
                    }
                }
            }
        }

        for (a, b) in edges {
            if let (Some(&na), Some(&nb)) = (g.note_to_node.get(&a), g.note_to_node.get(&b)) {
                g.graph.add_edge(na, nb, ());
            }
        }
        g
    }

    pub fn neighbors(&self, id: NoteId) -> Vec<NoteId> {
        let Some(&idx) = self.note_to_node.get(&id) else {
            return vec![];
        };
        self.graph
            .neighbors(idx)
            .map(|n| self.graph[n])
            .collect()
    }

    pub fn degree(&self, id: NoteId) -> usize {
        let Some(&idx) = self.note_to_node.get(&id) else {
            return 0;
        };
        // Count without allocating a neighbor Vec (hot path for graph paint/layout).
        self.graph.neighbors(idx).count()
    }

    /// Edges whose both endpoints are in `visible`.
    pub fn edges_among(&self, visible: &HashSet<NoteId>) -> Vec<(NoteId, NoteId)> {
        self.graph
            .edge_references()
            .filter_map(|e| {
                let a = self.graph[e.source()];
                let b = self.graph[e.target()];
                if visible.contains(&a) && visible.contains(&b) {
                    Some((a, b))
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn backlinks(&self, index: &VaultIndex, target: NoteId) -> Vec<NoteId> {
        let mut out = Vec::new();
        for note in index.notes() {
            if note.id == target {
                continue;
            }
            let mut hits = false;
            for wl in &note.wikilinks {
                if let ResolvedLink::Note(t) = resolve_link(index, &note.rel, &wl.target) {
                    if t == target {
                        hits = true;
                        break;
                    }
                }
            }
            if !hits {
                for dest in &note.md_link_targets {
                    if let ResolvedLink::Note(t) = resolve_link(index, &note.rel, dest) {
                        if t == target {
                            hits = true;
                            break;
                        }
                    }
                }
            }
            if hits {
                out.push(note.id);
            }
        }
        out.sort_by_key(|id| index.get(*id).map(|n| n.rel.clone()).unwrap_or_default());
        out
    }

    /// Local neighborhood by BFS depth.
    pub fn local(&self, center: NoteId, depth: usize) -> HashSet<NoteId> {
        let mut seen = HashSet::new();
        let mut q = VecDeque::new();
        q.push_back((center, 0));
        seen.insert(center);
        while let Some((id, d)) = q.pop_front() {
            if d >= depth {
                continue;
            }
            for n in self.neighbors(id) {
                if seen.insert(n) {
                    q.push_back((n, d + 1));
                }
            }
        }
        seen
    }

    pub fn all_edges(&self) -> Vec<(NoteId, NoteId)> {
        self.graph
            .edge_references()
            .map(|e| (self.graph[e.source()], self.graph[e.target()]))
            .collect()
    }

    pub fn node_ids(&self) -> Vec<NoteId> {
        self.note_to_node.keys().copied().collect()
    }
}
