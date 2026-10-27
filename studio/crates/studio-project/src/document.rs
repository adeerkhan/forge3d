//! The scene document: an ordered layer tree (GUI.md III.3, Part VI).
//!
//! Layer order is Photoshop's: index 0 is the **topmost** row (painted last),
//! the last index is the bottom of the stack, where the terrain sits. Groups
//! are ordinary layers whose `kind` is [`LayerKind::Group`], and a group's
//! children are stored **contiguously** directly after their parent, so a
//! subtree is one slice of the vector and reordering it is a single drain.
//!
//! The document deliberately holds only the scene graph. Camera, sun, output,
//! the terrain reference and the engine-facing parameters stay on [`crate::Project`]:
//! one owner per fact, no mirrored state to drift.
//!
//! Capability honesty lives in [`LayerKind::renders`]: a kind that Studio
//! cannot draw yet reports `false`, and the UI says so instead of pretending.

use serde::{Deserialize, Serialize};

/// Format version of the document structure (project v3).
pub const DOCUMENT_VERSION: u32 = 3;

/// Stable layer identity. Survives reordering, renaming and undo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LayerId(pub u32);

impl LayerId {
    /// The id reserved for a layer created before any document existed.
    pub const ROOT: LayerId = LayerId(0);
}

/// What a layer holds. The names are the stored truth (`snake_case`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerKind {
    /// The DEM the terrain renderer paints.
    Terrain,
    /// A dataset registered in the project whose renderer has not landed yet
    /// (point clouds, tilesets, meshes, vectors imported as references).
    Reference,
    /// Colour ramp or thematic drape over the terrain.
    RasterOverlay,
    /// Lines and polygons from GeoJSON / GeoPackage.
    Vector,
    /// GeoJSON / CityJSON / 3D Tiles extrusions.
    Buildings,
    /// Text labels with collision and fade.
    Labels,
    /// LAZ / COPC / EPT point cloud.
    PointCloud,
    /// glTF / OBJ geometry with materials.
    Mesh,
    /// Graticule, scale bar, north arrow, legend, map plate.
    Furniture,
    /// A folder that carries visibility/opacity for its children.
    Group,
}

impl LayerKind {
    /// Every kind, for tests and future add-layer menus.
    pub const ALL: &'static [LayerKind] = &[
        LayerKind::Terrain,
        LayerKind::Reference,
        LayerKind::RasterOverlay,
        LayerKind::Vector,
        LayerKind::Buildings,
        LayerKind::Labels,
        LayerKind::PointCloud,
        LayerKind::Mesh,
        LayerKind::Furniture,
        LayerKind::Group,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LayerKind::Terrain => "Terrain",
            LayerKind::Reference => "Reference",
            LayerKind::RasterOverlay => "Raster overlay",
            LayerKind::Vector => "Vector",
            LayerKind::Buildings => "Buildings",
            LayerKind::Labels => "Labels",
            LayerKind::PointCloud => "Point cloud",
            LayerKind::Mesh => "Mesh",
            LayerKind::Furniture => "Map furniture",
            LayerKind::Group => "Group",
        }
    }

    /// Whether Studio can draw this kind today. `false` is not an error — the
    /// Inspector names the milestone that implements it.
    pub fn renders(self) -> bool {
        matches!(self, LayerKind::Terrain | LayerKind::Group)
    }

    pub fn is_group(self) -> bool {
        self == LayerKind::Group
    }

    /// Map a `dataset.inspect` kind onto a layer kind.
    pub fn from_dataset_kind(kind: &str) -> LayerKind {
        match kind {
            "terrain" => LayerKind::Terrain,
            "raster" => LayerKind::RasterOverlay,
            "vector" => LayerKind::Vector,
            "pointcloud" => LayerKind::PointCloud,
            "tileset" => LayerKind::Buildings,
            "mesh" => LayerKind::Mesh,
            _ => LayerKind::Reference,
        }
    }

    /// Map a serialized `SceneRecipe` layer type onto a layer kind.
    pub fn from_recipe_type(kind: &str) -> LayerKind {
        match kind {
            "terrain" => LayerKind::Terrain,
            "raster" => LayerKind::RasterOverlay,
            "vector" | "vector_lines" => LayerKind::Vector,
            "buildings" => LayerKind::Buildings,
            "labels" | "label" => LayerKind::Labels,
            "points" | "pointcloud" => LayerKind::PointCloud,
            "mesh" | "gltf" => LayerKind::Mesh,
            "graticule" | "scale_bar" | "north_arrow" | "legend" | "map_plate" => {
                LayerKind::Furniture
            }
            _ => LayerKind::Reference,
        }
    }
}

/// One row of the scene graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layer {
    pub id: LayerId,
    /// Enclosing group, if any.
    pub parent: Option<LayerId>,
    pub kind: LayerKind,
    pub name: String,
    pub visible: bool,
    /// 0.0–1.0, applied by the renderer when the kind supports it.
    pub opacity: f32,
    /// Locked layers cannot be edited by selection-driven actions.
    pub locked: bool,
    /// Groups only: whether the children are shown in the panel.
    pub expanded: bool,
    /// Dataset path or engine-side source reference.
    pub source: Option<String>,
    /// Engine payload kept verbatim (a recipe layer object, a style dict, …).
    pub style: serde_json::Value,
}

impl Default for Layer {
    fn default() -> Self {
        Self {
            id: LayerId::ROOT,
            parent: None,
            kind: LayerKind::Reference,
            name: String::new(),
            visible: true,
            opacity: 1.0,
            locked: false,
            expanded: true,
            source: None,
            style: serde_json::Value::Null,
        }
    }
}

/// A flattened row: the layer plus its indentation depth (groups nest).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub id: LayerId,
    pub depth: usize,
}

/// The scene graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Document {
    pub version: u32,
    /// Topmost first (see the module docs).
    pub layers: Vec<Layer>,
    /// Next free id; monotonic so ids are never reused after a delete.
    next_id: u32,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            version: DOCUMENT_VERSION,
            layers: Vec::new(),
            next_id: 1,
        }
    }
}

impl Document {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    pub fn len(&self) -> usize {
        self.layers.len()
    }

    /// Allocate an id (never reused, even across undo, so undo cannot make two
    /// live layers share an identity).
    fn allocate(&mut self) -> LayerId {
        let id = LayerId(self.next_id);
        self.next_id += 1;
        id
    }

    pub fn index_of(&self, id: LayerId) -> Option<usize> {
        self.layers.iter().position(|layer| layer.id == id)
    }

    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.iter().find(|layer| layer.id == id)
    }

    pub fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|layer| layer.id == id)
    }

    /// Children of `parent` (None = top level), in display order.
    pub fn children(&self, parent: Option<LayerId>) -> Vec<LayerId> {
        self.layers
            .iter()
            .filter(|layer| layer.parent == parent)
            .map(|layer| layer.id)
            .collect()
    }

    pub fn ancestors(&self, id: LayerId) -> Vec<LayerId> {
        let mut out = Vec::new();
        let mut current = self.layer(id).and_then(|layer| layer.parent);
        while let Some(parent) = current {
            out.push(parent);
            current = self.layer(parent).and_then(|layer| layer.parent);
        }
        out
    }

    pub fn depth(&self, id: LayerId) -> usize {
        self.ancestors(id).len()
    }

    /// The contiguous subtree of `id`, in display order (the layer first).
    pub fn subtree(&self, id: LayerId) -> Vec<LayerId> {
        let Some(start) = self.index_of(id) else {
            return Vec::new();
        };
        let end = self.subtree_end(start);
        self.layers[start..end]
            .iter()
            .map(|layer| layer.id)
            .collect()
    }

    /// End (exclusive) of the contiguous subtree that starts at `start`.
    fn subtree_end(&self, start: usize) -> usize {
        let mut end = start + 1;
        while end < self.layers.len() {
            let candidate = self.layers[end].id;
            if !self.ancestors(candidate).contains(&self.layers[start].id) {
                break;
            }
            end += 1;
        }
        end
    }

    /// Rows visible in the layer panel: children of collapsed groups are hidden.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for (index, layer) in self.layers.iter().enumerate() {
            let hidden = self.ancestors(layer.id).into_iter().any(|parent| {
                self.layer(parent)
                    .map(|group| !group.expanded)
                    .unwrap_or(false)
            });
            if hidden {
                continue;
            }
            let _ = index;
            rows.push(Row {
                id: layer.id,
                depth: self.depth(layer.id),
            });
        }
        rows
    }

    /// Append a layer at the bottom of the stack (or inside `parent`).
    pub fn push(
        &mut self,
        kind: LayerKind,
        name: impl Into<String>,
        parent: Option<LayerId>,
    ) -> LayerId {
        self.push_detailed(
            kind,
            name.into(),
            true,
            None,
            serde_json::Value::Null,
            parent,
        )
    }

    /// Append a layer carrying migrated state (visibility, source, style).
    pub fn push_detailed(
        &mut self,
        kind: LayerKind,
        name: String,
        visible: bool,
        source: Option<String>,
        style: serde_json::Value,
        parent: Option<LayerId>,
    ) -> LayerId {
        let id = self.allocate();
        let layer = Layer {
            id,
            parent,
            kind,
            name,
            visible,
            source,
            style,
            ..Layer::default()
        };
        match parent {
            Some(parent) if self.layer(parent).is_some() => {
                let insert = self.subtree_end(self.index_of(parent).expect("checked"));
                self.layers.insert(insert, layer);
            }
            _ => self.layers.push(layer),
        }
        id
    }

    /// Insert a fully-formed layer (migration and tests use this).
    pub fn insert_at(&mut self, index: usize, mut layer: Layer) -> LayerId {
        if layer.id == LayerId::ROOT {
            layer.id = self.allocate();
        } else {
            self.next_id = self.next_id.max(layer.id.0 + 1);
        }
        let index = index.min(self.layers.len());
        let id = layer.id;
        self.layers.insert(index, layer);
        id
    }

    /// Remove a layer and its subtree.
    pub fn remove(&mut self, id: LayerId) -> Vec<Layer> {
        let Some(start) = self.index_of(id) else {
            return Vec::new();
        };
        let end = self.subtree_end(start);
        self.layers.drain(start..end).collect()
    }

    /// Duplicate a layer (and its subtree) directly above the original.
    pub fn duplicate(&mut self, id: LayerId) -> Option<LayerId> {
        let start = self.index_of(id)?;
        let end = self.subtree_end(start);
        let originals: Vec<Layer> = self.layers[start..end].to_vec();
        let mut id_map = std::collections::HashMap::new();
        for layer in &originals {
            id_map.insert(layer.id, self.allocate());
        }
        let mut copies: Vec<Layer> = originals
            .iter()
            .map(|layer| {
                let mut copy = layer.clone();
                copy.id = id_map[&layer.id];
                if let Some(parent) = layer.parent {
                    if let Some(mapped) = id_map.get(&parent) {
                        copy.parent = Some(*mapped);
                    }
                }
                copy
            })
            .collect();
        let root = copies.first_mut()?;
        root.name = format!("{} copy", root.name);
        if root.kind.is_group() {
            root.expanded = true;
        }
        let new_id = root.id;
        self.layers.splice(start..start, copies);
        Some(new_id)
    }

    /// Move a layer so it sits immediately before/after `target`, adopting the
    /// target's parent. Dropping a group into itself is refused.
    pub fn move_beside(&mut self, id: LayerId, target: LayerId, after: bool) -> bool {
        if id == target {
            return false;
        }
        if self.subtree(id).contains(&target) {
            return false; // no dropping a group into its own subtree
        }
        let Some((start, end)) = self.range(id) else {
            return false;
        };
        let parent = self.layer(target).and_then(|layer| layer.parent);
        let mut moved: Vec<Layer> = self.layers.drain(start..end).collect();
        for layer in &mut moved {
            if layer.id == id {
                layer.parent = parent;
            }
        }
        let Some(target_index) = self.index_of(target) else {
            // Target vanished with the drain (cannot happen: target is outside
            // the subtree), put the block back at the end.
            let at = self.layers.len();
            self.layers.splice(at..at, moved);
            return false;
        };
        let offset = usize::from(after);
        let insert = if after {
            self.subtree_end(target_index)
        } else {
            target_index
        };
        let _ = offset;
        self.layers.splice(insert..insert, moved);
        true
    }

    /// Drop a layer into the top of a group.
    pub fn move_into(&mut self, id: LayerId, group: LayerId) -> bool {
        if id == group
            || !self
                .layer(group)
                .map(|layer| layer.kind.is_group())
                .unwrap_or(false)
        {
            return false;
        }
        if self.subtree(id).contains(&group) {
            return false;
        }
        let Some((start, end)) = self.range(id) else {
            return false;
        };
        let mut moved: Vec<Layer> = self.layers.drain(start..end).collect();
        moved[0].parent = Some(group);
        let insert = match self.index_of(group) {
            Some(index) => index + 1,
            None => self.layers.len(),
        };
        self.layers.splice(insert..insert, moved);
        if let Some(group_layer) = self.layer_mut(group) {
            group_layer.expanded = true;
        }
        true
    }

    /// Move one step up/down among siblings.
    pub fn move_by(&mut self, id: LayerId, delta: i32) -> bool {
        let siblings = self.children(self.layer(id).and_then(|layer| layer.parent));
        let Some(position) = siblings.iter().position(|sibling| *sibling == id) else {
            return false;
        };
        let target_position = position as i32 - delta; // up = earlier in the vector
        if target_position < 0 || target_position as usize >= siblings.len() {
            return false;
        }
        let target = siblings[target_position as usize];
        self.move_beside(id, target, delta < 0)
    }

    /// Move to the first/last position among siblings.
    pub fn move_to_edge(&mut self, id: LayerId, top: bool) -> bool {
        let siblings = self.children(self.layer(id).and_then(|layer| layer.parent));
        if siblings.len() < 2 {
            return false;
        }
        let target = if top {
            siblings[0]
        } else {
            *siblings.last().expect("non-empty")
        };
        if target == id {
            return false;
        }
        self.move_beside(id, target, !top)
    }

    /// Group the given layers into a new folder placed at the topmost one's
    /// position. Only the topmost contiguous run is grouped — grouping across
    /// unrelated branches would silently reorder the stack.
    pub fn group(&mut self, ids: &[LayerId], name: impl Into<String>) -> Option<LayerId> {
        let mut ordered: Vec<LayerId> = ids
            .iter()
            .copied()
            .filter(|id| self.layer(*id).is_some())
            .collect();
        if ordered.is_empty() {
            return None;
        }
        ordered.sort_by_key(|id| self.index_of(*id).unwrap_or(usize::MAX));
        ordered.dedup();
        let parent = self.layer(ordered[0]).and_then(|layer| layer.parent);
        // Keep only siblings of the first (contiguous run from the top).
        ordered.retain(|id| self.layer(*id).and_then(|layer| layer.parent) == parent);
        if ordered.is_empty() {
            return None;
        }
        let first_index = self.index_of(ordered[0])?;
        let group_id = self.allocate();
        let group = Layer {
            id: group_id,
            parent,
            kind: LayerKind::Group,
            name: name.into(),
            expanded: true,
            ..Layer::default()
        };
        self.layers.insert(first_index, group);
        // Each child lands at group_index + 1, so the selection is walked in
        // reverse to keep its original order inside the new folder.
        for id in ordered.iter().rev() {
            self.move_into(*id, group_id);
        }
        Some(group_id)
    }

    /// Dissolve a group: children take its place and inherit its parent.
    pub fn ungroup(&mut self, id: LayerId) -> Option<LayerId> {
        let group = self.layer(id)?.clone();
        if !group.kind.is_group() {
            return None;
        }
        let children = self.children(Some(id));
        for child in &children {
            if let Some(layer) = self.layer_mut(*child) {
                layer.parent = group.parent;
            }
        }
        self.remove(id);
        Some(id)
    }

    /// The terrain layer, if the document has one.
    pub fn terrain_layer(&self) -> Option<&Layer> {
        self.layers
            .iter()
            .find(|layer| layer.kind == LayerKind::Terrain)
    }

    pub fn terrain_layer_mut(&mut self) -> Option<&mut Layer> {
        self.layers
            .iter_mut()
            .find(|layer| layer.kind == LayerKind::Terrain)
    }

    /// Whether every ancestor of `id` is visible.
    pub fn effective_visible(&self, id: LayerId) -> bool {
        let self_visible = self.layer(id).map(|layer| layer.visible).unwrap_or(false);
        self_visible
            && self.ancestors(id).into_iter().all(|parent| {
                self.layer(parent)
                    .map(|layer| layer.visible)
                    .unwrap_or(false)
            })
    }

    /// Resolve a layer's displayed opacity (its own × ancestors').
    pub fn effective_opacity(&self, id: LayerId) -> f32 {
        let mut opacity = self.layer(id).map(|layer| layer.opacity).unwrap_or(1.0);
        for parent in self.ancestors(id) {
            if let Some(group) = self.layer(parent) {
                opacity *= group.opacity;
            }
        }
        opacity.clamp(0.0, 1.0)
    }

    fn range(&self, id: LayerId) -> Option<(usize, usize)> {
        let start = self.index_of(id)?;
        Some((start, self.subtree_end(start)))
    }
}

/// Undo/redo over whole-document snapshots. Documents are small JSON graphs, so
/// snapshots are simpler and safer than inverse commands, and an edit can never
/// leave the stack half-applied.
#[derive(Debug, Default)]
pub struct DocHistory {
    undo: Vec<Document>,
    redo: Vec<Document>,
    limit: usize,
}

impl DocHistory {
    pub fn new(limit: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            limit,
        }
    }

    /// Record the document as it was *before* an edit.
    pub fn record(&mut self, before: Document) {
        self.undo.push(before);
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Swap `current` for the previous snapshot. Returns true when it moved.
    pub fn undo(&mut self, current: &mut Document) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo.push(std::mem::replace(current, previous));
        true
    }

    pub fn redo(&mut self, current: &mut Document) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(std::mem::replace(current, next));
        true
    }

    pub fn depth(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_with(names: &[&str]) -> (Document, Vec<LayerId>) {
        let mut document = Document::new();
        let mut ids = Vec::new();
        for name in names {
            ids.push(document.push(LayerKind::Reference, *name, None));
        }
        (document, ids)
    }

    fn names(document: &Document) -> Vec<String> {
        document
            .layers
            .iter()
            .map(|layer| layer.name.clone())
            .collect()
    }

    #[test]
    fn layers_are_topmost_first_and_ids_are_monotonic() {
        let (document, ids) = doc_with(&["a", "b", "c"]);
        assert_eq!(names(&document), vec!["a", "b", "c"]);
        assert_eq!(ids[0].0, 1);
        assert_eq!(ids[2].0, 3);
        // Deleting never recycles an id.
        let mut document = document;
        document.remove(ids[1]);
        let fresh = document.push(LayerKind::Reference, "d", None);
        assert!(fresh.0 > ids[2].0);
    }

    #[test]
    fn subtrees_stay_contiguous() {
        let (mut document, ids) = doc_with(&["a", "b", "c", "d"]);
        let group = document.group(&[ids[1], ids[2]], "G").expect("group");
        assert_eq!(names(&document), vec!["a", "G", "b", "c", "d"]);
        assert_eq!(document.children(Some(group)), vec![ids[1], ids[2]]);
        let subtree = document.subtree(group);
        assert_eq!(subtree.len(), 3);
    }

    #[test]
    fn collapsed_groups_hide_children_in_rows() {
        let (mut document, ids) = doc_with(&["a", "b", "c"]);
        let group = document.group(&[ids[0]], "G").expect("group");
        assert_eq!(document.rows().len(), 4);
        document.layer_mut(group).expect("group").expanded = false;
        let rows = document.rows();
        assert_eq!(rows.len(), 3, "collapsed children are hidden");
        assert_eq!(rows[0].id, group);
        assert_eq!(rows[0].depth, 0);
        assert_eq!(rows[1].id, ids[1]);
    }

    #[test]
    fn move_beside_reorders_and_reparents() {
        let (mut document, ids) = doc_with(&["a", "b", "c"]);
        assert!(document.move_beside(ids[2], ids[0], false));
        assert_eq!(names(&document), vec!["c", "a", "b"]);
        // Move c below a: it adopts a's (top-level) parent.
        assert!(document.move_beside(ids[2], ids[0], true));
        assert_eq!(names(&document), vec!["a", "c", "b"]);
        // A group cannot be dropped into its own subtree.
        let group = document.group(&[ids[1]], "G").expect("group");
        assert!(!document.move_beside(group, ids[1], true));
    }

    #[test]
    fn move_by_and_to_edge_stay_within_siblings() {
        let (mut document, ids) = doc_with(&["a", "b", "c"]);
        assert!(document.move_by(ids[2], 1)); // c up one
        assert_eq!(names(&document), vec!["a", "c", "b"]);
        assert!(!document.move_by(ids[0], 1), "already at the top");
        assert!(document.move_to_edge(ids[0], false)); // a to bottom
        assert_eq!(names(&document), vec!["c", "b", "a"]);
    }

    #[test]
    fn duplicate_copies_the_subtree_with_fresh_ids() {
        let (mut document, ids) = doc_with(&["a", "b"]);
        let group = document.group(&[ids[0], ids[1]], "G").expect("group");
        let copy = document.duplicate(group).expect("duplicate");
        assert_ne!(copy, group);
        assert_eq!(document.children(Some(copy)).len(), 2);
        assert_eq!(
            document.layer(copy).expect("copy").name,
            "G copy",
            "copies are named for the user"
        );
        let original_children = document.children(Some(group));
        let copy_children = document.children(Some(copy));
        assert!(copy_children
            .iter()
            .all(|id| !original_children.contains(id)));
    }

    #[test]
    fn ungroup_dissolves_the_folder_and_keeps_children() {
        let (mut document, ids) = doc_with(&["a", "b", "c"]);
        let group = document.group(&[ids[1]], "G").expect("group");
        assert_eq!(names(&document), vec!["a", "G", "b", "c"]);
        document.ungroup(group).expect("ungroup");
        assert_eq!(names(&document), vec!["a", "b", "c"]);
        assert_eq!(document.layer(ids[1]).expect("b").parent, None);
    }

    #[test]
    fn grouping_only_takes_siblings_of_the_topmost() {
        let (mut document, ids) = doc_with(&["a", "b", "c"]);
        let outer = document.group(&[ids[0]], "outer").expect("group");
        // `a` now lives inside `outer`; `c` is top level. Grouping both must
        // take the topmost one's siblings only, and never silently reorder the
        // branches it cannot join.
        let nested = document.group(&[ids[0], ids[2]], "nested").expect("group");
        assert_eq!(document.children(Some(nested)), vec![ids[0]]);
        assert_eq!(document.layer(ids[2]).expect("c").parent, None);
        assert_eq!(
            document.layer(nested).expect("nested").parent,
            Some(outer),
            "the new folder joins the run it grouped"
        );
    }

    #[test]
    fn effective_visibility_and_opacity_follow_groups() {
        let (mut document, ids) = doc_with(&["a", "b"]);
        let group = document.group(&[ids[0]], "G").expect("group");
        document.layer_mut(group).expect("group").opacity = 0.5;
        document.layer_mut(group).expect("group").visible = false;
        assert!(!document.effective_visible(ids[0]));
        assert!((document.effective_opacity(ids[0]) - 0.5).abs() < 1e-6);
        assert!(document.effective_visible(ids[1]));
        assert!((document.effective_opacity(ids[1]) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn remove_takes_the_whole_subtree() {
        let (mut document, ids) = doc_with(&["a", "b", "c"]);
        let group = document.group(&[ids[0], ids[1]], "G").expect("group");
        let removed = document.remove(group);
        assert_eq!(removed.len(), 3, "group plus its two children");
        assert_eq!(names(&document), vec!["c"]);
    }

    #[test]
    fn history_undoes_and_redoes_whole_documents() {
        let (mut document, ids) = doc_with(&["a", "b"]);
        let mut history = DocHistory::new(10);
        history.record(document.clone());
        document.remove(ids[0]);
        assert_eq!(names(&document), vec!["b"]);
        assert!(history.can_undo());
        assert!(history.undo(&mut document));
        assert_eq!(names(&document), vec!["a", "b"]);
        assert!(history.can_redo());
        assert!(history.redo(&mut document));
        assert_eq!(names(&document), vec!["b"]);
        assert!(!history.redo(&mut document), "nothing left to redo");
    }

    #[test]
    fn history_drops_the_redo_branch_on_a_new_edit() {
        let (mut document, ids) = doc_with(&["a"]);
        let mut history = DocHistory::new(10);
        history.record(document.clone());
        document.remove(ids[0]);
        history.undo(&mut document);
        assert!(history.can_redo());
        history.record(document.clone());
        assert!(!history.can_redo(), "a new edit truncates the redo branch");
    }

    #[test]
    fn history_is_bounded() {
        let mut document = Document::new();
        let mut history = DocHistory::new(2);
        for index in 0..5 {
            history.record(document.clone());
            document.push(LayerKind::Reference, format!("l{index}"), None);
        }
        assert_eq!(history.depth().0, 2);
    }

    #[test]
    fn document_round_trips_through_json() {
        let (mut document, ids) = doc_with(&["a", "b"]);
        let group = document.group(&[ids[0]], "G").expect("group");
        document.layer_mut(group).expect("group").opacity = 0.42;
        let text = serde_json::to_string_pretty(&document).expect("serialize");
        let back: Document = serde_json::from_str(&text).expect("parse");
        assert_eq!(document, back);
        // Ids never collide after a load followed by a new layer.
        let mut back = back;
        let fresh = back.push(LayerKind::Reference, "c", None);
        assert!(back.layer(fresh).is_some());
        assert!(back.index_of(group).is_some());
    }

    #[test]
    fn empty_json_object_is_a_valid_empty_document() {
        let document: Document = serde_json::from_str("{}").expect("parse");
        assert!(document.is_empty());
        assert_eq!(document.version, DOCUMENT_VERSION);
    }

    #[test]
    fn recipe_and_dataset_kinds_map_onto_layer_kinds() {
        assert_eq!(
            LayerKind::from_dataset_kind("pointcloud"),
            LayerKind::PointCloud
        );
        assert_eq!(
            LayerKind::from_dataset_kind("raster"),
            LayerKind::RasterOverlay
        );
        assert_eq!(LayerKind::from_recipe_type("vector"), LayerKind::Vector);
        assert_eq!(
            LayerKind::from_recipe_type("scale_bar"),
            LayerKind::Furniture
        );
        assert_eq!(
            LayerKind::from_recipe_type("nonsense"),
            LayerKind::Reference
        );
    }

    #[test]
    fn only_terrain_and_groups_claim_to_render_today() {
        for kind in LayerKind::ALL {
            let expected = matches!(kind, LayerKind::Terrain | LayerKind::Group);
            assert_eq!(kind.renders(), expected, "{kind:?} capability drift");
        }
    }
}
