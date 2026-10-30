//! Recipe editing: the layer list, its order, per-layer opacity and the output
//! size.
//!
//! The MapScene recipe is the scene's source of truth, so these operations patch
//! the recipe JSON itself — the same bytes the bridge renders. Pure functions
//! over a recipe string, so the panel's behaviour is testable without a window.

use serde_json::Value;

/// One row of the recipe's layer list, as the cartography panel shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct RecipeLayer {
    /// Position in `recipe.layers`.
    pub index: usize,
    /// Engine layer kind (`raster_overlay`, `vector_overlay`, `label_layer`, …).
    pub kind: String,
    /// The layer's own id, when it has one.
    pub id: String,
    /// Raster layers carry opacity; others report 1.0.
    pub opacity: f32,
}

impl RecipeLayer {
    /// A short label for the panel: the id, else the kind.
    pub fn label(&self) -> String {
        if self.id.is_empty() {
            self.kind.clone()
        } else {
            self.id.clone()
        }
    }

    /// Friendly name for a kind.
    pub fn kind_label(&self) -> &'static str {
        match self.kind.as_str() {
            "raster_overlay" => "Raster",
            "vector_overlay" => "Vector",
            "label_layer" => "Labels",
            "terrain" => "Terrain",
            "tiles3d_layer" => "3D Tiles",
            "pointcloud_layer" => "Point cloud",
            _ => "Layer",
        }
    }
}

fn parse(recipe: &str) -> Option<Value> {
    serde_json::from_str(recipe).ok()
}

/// The recipe's layers, in paint order (bottom first, as the engine draws them).
pub fn layer_list(recipe: &str) -> Vec<RecipeLayer> {
    let Some(value) = parse(recipe) else {
        return Vec::new();
    };
    let Some(layers) = value.get("layers").and_then(|layers| layers.as_array()) else {
        return Vec::new();
    };
    layers
        .iter()
        .enumerate()
        .map(|(index, layer)| RecipeLayer {
            index,
            kind: layer
                .get("kind")
                .and_then(|value| value.as_str())
                .unwrap_or("layer")
                .to_owned(),
            id: layer
                .get("layer_id")
                .or_else(|| layer.get("id"))
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_owned(),
            opacity: layer
                .get("opacity")
                .and_then(|value| value.as_f64())
                .unwrap_or(1.0) as f32,
        })
        .collect()
}

/// Number of layers, without parsing the rest.
pub fn layer_count(recipe: &str) -> usize {
    parse(recipe)
        .and_then(|value| {
            value
                .get("layers")
                .and_then(|layers| layers.as_array())
                .map(|layers| layers.len())
        })
        .unwrap_or(0)
}

/// Remove one layer. Returns the new recipe, or `None` when the index is out of
/// range (so the caller never replaces a recipe with a broken one).
pub fn remove_layer(recipe: &str, index: usize) -> Option<String> {
    let mut value = parse(recipe)?;
    let layers = value.get_mut("layers")?.as_array_mut()?;
    if index >= layers.len() {
        return None;
    }
    layers.remove(index);
    serde_json::to_string(&value).ok()
}

/// Move a layer by `delta` places in paint order; clamped to the ends.
pub fn move_layer(recipe: &str, index: usize, delta: i32) -> Option<String> {
    let mut value = parse(recipe)?;
    let layers = value.get_mut("layers")?.as_array_mut()?;
    let len = layers.len();
    if index >= len || len < 2 {
        return None;
    }
    let target = (index as i32 + delta).clamp(0, len as i32 - 1) as usize;
    if target == index {
        return None;
    }
    let layer = layers.remove(index);
    layers.insert(target, layer);
    serde_json::to_string(&value).ok()
}

/// Set a layer's opacity (0–1). Only layers that carry the field accept it.
pub fn set_layer_opacity(recipe: &str, index: usize, opacity: f32) -> Option<String> {
    let mut value = parse(recipe)?;
    let layers = value.get_mut("layers")?.as_array_mut()?;
    let layer = layers.get_mut(index)?;
    layer.get("opacity")?;
    layer.as_object_mut()?.insert(
        "opacity".to_owned(),
        serde_json::json!(opacity.clamp(0.0, 1.0)),
    );
    serde_json::to_string(&value).ok()
}

/// Set the recipe's output size, which is what the renderer draws.
pub fn set_output_size(recipe: &str, width: u32, height: u32) -> Option<String> {
    let mut value = parse(recipe)?;
    let output = value.get_mut("output")?.as_object_mut()?;
    output.insert("width".to_owned(), serde_json::json!(width));
    output.insert("height".to_owned(), serde_json::json!(height));
    serde_json::to_string(&value).ok()
}

/// The recipe's output size, when it declares one.
pub fn output_size(recipe: &str) -> Option<(u32, u32)> {
    let value = parse(recipe)?;
    let output = value.get("output")?;
    Some((
        output.get("width")?.as_u64()? as u32,
        output.get("height")?.as_u64()? as u32,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn recipe() -> String {
        serde_json::json!({
            "kind": "scene_recipe",
            "layers": [
                { "kind": "raster_overlay", "layer_id": "ortho", "opacity": 0.8, "path": "a.tif" },
                { "kind": "vector_overlay", "layer_id": "lines", "style": {} },
                { "kind": "label_layer", "layer_id": "labels" }
            ],
            "output": { "width": 960, "height": 600 }
        })
        .to_string()
    }

    #[test]
    fn the_layer_list_matches_the_recipe_order() {
        let layers = layer_list(&recipe());
        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0].kind_label(), "Raster");
        assert_eq!(layers[0].label(), "ortho");
        assert!((layers[0].opacity - 0.8).abs() < 1e-6);
        assert_eq!(layers[1].kind_label(), "Vector");
        assert_eq!(layers[2].kind_label(), "Labels");
        assert_eq!(layer_count(&recipe()), 3);
    }

    #[test]
    fn removing_a_layer_keeps_the_rest() {
        let next = remove_layer(&recipe(), 0).expect("removed");
        let layers = layer_list(&next);
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0].label(), "lines");
        assert!(
            remove_layer(&recipe(), 9).is_none(),
            "out of range is refused"
        );
    }

    #[test]
    fn moving_a_layer_changes_paint_order() {
        let next = move_layer(&recipe(), 0, 1).expect("moved");
        let layers = layer_list(&next);
        assert_eq!(layers[0].label(), "lines");
        assert_eq!(layers[1].label(), "ortho");
        // A move past the end is clamped, and a no-op returns None.
        assert!(move_layer(&recipe(), 2, 1).is_none());
    }

    #[test]
    fn opacity_only_applies_where_the_layer_has_it() {
        let next = set_layer_opacity(&recipe(), 0, 0.25).expect("set");
        assert!((layer_list(&next)[0].opacity - 0.25).abs() < 1e-6);
        assert!(
            set_layer_opacity(&recipe(), 1, 0.5).is_none(),
            "a vector layer has no opacity field; do not invent one"
        );
    }

    #[test]
    fn the_output_size_round_trips() {
        let next = set_output_size(&recipe(), 1920, 1080).expect("set");
        assert_eq!(output_size(&next), Some((1920, 1080)));
        assert_eq!(output_size(&recipe()), Some((960, 600)));
    }
}
