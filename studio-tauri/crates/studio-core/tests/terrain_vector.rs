//! The terrain's vector layer.
//!
//! A vector layer is a file (GeoPackage or GeoJSON), a colour and the line's
//! real width in the DEM's metres. The studio stores it on the terrain object
//! so a project round-trips it, and the service turns it into the engine's own
//! projection and rasterizer at render time.

use studio_core::command::Command;
use studio_core::editor::Editor;
use studio_core::ids::{self, ObjectId};
use studio_core::project::{Asset, AssetKind, Project, TerrainSeed};
use studio_core::scene::TerrainVectorLayer;
use studio_core::ObjectKind;

fn vector_asset() -> Asset {
    Asset {
        id: ids::new_id(),
        name: "luxembourg_rail.gpkg".to_owned(),
        path: "/data/luxembourg_rail.gpkg".to_owned(),
        kind: AssetKind::Vector,
        byte_size: 8_000_000,
    }
}

fn seed() -> TerrainSeed {
    TerrainSeed {
        title: Some("Luxembourg".to_owned()),
        name: "luxembourg_dem.tif".to_owned(),
        path: "/data/luxembourg_dem.tif".to_owned(),
        byte_size: 1,
        dimensions: Some([2929, 2705]),
        elevation_range: Some([200.0, 560.0]),
        world_origin_xz: Some([0.0, 0.0]),
        world_span_xz: Some([65_000.0, 60_000.0]),
        overlay: None,
    }
}

fn project_with_terrain() -> (Editor, ObjectId, Asset) {
    let asset = vector_asset();
    let mut project = Project::starter("Luxembourg", Some(seed()));
    project.assets.push(asset.clone());
    let terrain = project.terrain_object().expect("terrain").id;
    (Editor::new(project), terrain, asset)
}

#[test]
fn a_geopackage_is_a_vector_asset() {
    assert_eq!(
        AssetKind::from_path(std::path::Path::new("rail.gpkg")),
        Some(AssetKind::Vector)
    );
    assert_eq!(
        AssetKind::from_path(std::path::Path::new("rivers.geojson")),
        Some(AssetKind::Vector)
    );
}

#[test]
fn attaching_a_vector_layer_is_one_undoable_command() {
    let (mut editor, terrain, asset) = project_with_terrain();
    let layer = TerrainVectorLayer {
        asset: asset.id,
        colour: [0.9, 0.29, 0.1, 1.0],
        width_px: 3.0,
    };
    editor
        .apply(Command::SetTerrainVector {
            id: terrain,
            layer: Some(layer),
        })
        .expect("a vector layer");

    let stored = editor
        .project()
        .scene
        .object(terrain)
        .and_then(|object| match &object.kind {
            ObjectKind::Terrain(terrain) => terrain.vector,
            _ => None,
        })
        .expect("the layer is on the terrain");
    assert_eq!(stored.asset, asset.id);
    assert_eq!(stored.width_px, 3.0);

    editor.undo().expect("undo is available");
    assert!(
        editor
            .project()
            .scene
            .object(terrain)
            .and_then(|object| match &object.kind {
                ObjectKind::Terrain(terrain) => terrain.vector,
                _ => None,
            })
            .is_none(),
        "undo removes the layer"
    );
}

#[test]
fn the_renderer_refuses_a_colour_or_width_it_cannot_take() {
    let (mut editor, terrain, asset) = project_with_terrain();
    for layer in [
        // A colour channel outside 0..=1.
        TerrainVectorLayer {
            asset: asset.id,
            colour: [1.4, 0.2, 0.1, 1.0],
            width_px: 3.0,
        },
        // A width outside the 0.1..=1000 m the rasterizer is asked for.
        TerrainVectorLayer {
            asset: asset.id,
            colour: [0.9, 0.29, 0.1, 1.0],
            width_px: 0.5,
        },
        TerrainVectorLayer {
            asset: asset.id,
            colour: [0.9, 0.29, 0.1, 1.0],
            width_px: 50.0,
        },
    ] {
        let error = editor
            .apply(Command::SetTerrainVector {
                id: terrain,
                layer: Some(layer),
            })
            .expect_err("the value must be refused")
            .to_string();
        assert!(error.contains("vector"), "{error}");
    }
}

#[test]
fn a_vector_layer_needs_a_registered_asset() {
    let (mut editor, terrain, _asset) = project_with_terrain();
    let error = editor
        .apply(Command::SetTerrainVector {
            id: terrain,
            layer: Some(TerrainVectorLayer {
                asset: ids::new_id(),
                colour: [0.9, 0.29, 0.1, 1.0],
                width_px: 3.0,
            }),
        })
        .expect_err("an unregistered asset must be refused")
        .to_string();
    assert!(error.contains("does not exist"), "{error}");
}
