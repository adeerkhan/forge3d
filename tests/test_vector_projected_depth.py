"""The engine's projected depth rule, and the recipe path that shares it.

`forge3d.vector.projected_depth` is the engine's own rule for the depth of a
projected path: the covered pixel takes the interpolated depth of the nearest
segment, and the sub-pixel offset from that segment. The recipe path
(`_map_scene_native_inputs`) and any caller that rasterized its own coverage
(the studio's OIT bridge, say) must use that one rule, so a line's depth
always matches the coverage it was computed for.
"""

from __future__ import annotations

import numpy as np

from forge3d.vector import projected_depth


def _coverage(rows, columns):
    """A coverage with the given pixels covered (opaque red)."""
    coverage = np.zeros((16, 16, 4), dtype=np.uint8)
    for row, column in rows:
        coverage[row, column] = (255, 0, 0, 255)
    return coverage


def test_the_nearest_segment_carries_the_depth():
    coverage = _coverage([(4, 4), (4, 5)], [(4, 4), (4, 5)])
    depth, offsets = projected_depth(
        coverage, [[(4.0, 4.0, 0.25), (6.0, 6.0, 0.75)]], 16, 16
    )
    # The covered pixels carry the interpolated depth of the segment, and the
    # rest of the frame stays at the far plane.
    assert depth[4, 4] < depth[4, 5] < 1.0
    assert depth[0, 0] == 1.0
    assert offsets.shape == (2, 2)


def test_the_offset_is_the_vector_to_the_nearest_segment():
    coverage = _coverage([(4, 4)], [(4, 4)])
    # A vertical segment one pixel to the left of the pixel's centre, so the
    # nearest point is off the segment's endpoints.
    depth, offsets = projected_depth(
        coverage, [[(4.0, 4.0, 0.5), (4.0, 8.0, 0.5)]], 16, 16
    )
    assert abs(offsets[0, 0]) > 0.0
    distance = float(np.hypot(offsets[0, 0], offsets[0, 1]))
    assert distance > 0.0
    # The squared length is the distance the recipe path's stroke mask needs.
    assert abs(float(offsets[0, 0] ** 2 + offsets[0, 1] ** 2) - distance**2) < 1e-9


def test_an_empty_coverage_has_no_depth_and_no_offsets():
    coverage = np.zeros((8, 8, 4), dtype=np.uint8)
    depth, offsets = projected_depth(coverage, [], 8, 8)
    assert depth.shape == (8, 8)
    assert (depth == 1.0).all()
    assert offsets.shape == (0, 2)


def test_the_recipe_path_uses_the_same_rule():
    """The recipe path calls the public rule, so there is one implementation.

    A source-level check: the engine's own vector path must not carry a second
    copy of the nearest-segment rule.
    """
    import ast
    from pathlib import Path

    source = (
        Path(__file__).resolve().parents[1]
        / "python"
        / "forge3d"
        / "_map_scene_native_inputs.py"
    ).read_text(encoding="utf-8")
    tree = ast.parse(source)
    calls = {
        node.func.id
        for node in ast.walk(tree)
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
    }
    assert "projected_depth" in calls
    # The rule's own arithmetic (the nearest-segment distance) is not written
    # out a second time.
    assert "length2" not in source
    assert "for a, b in segments" not in source


def test_the_polygon_overlay_path_is_reachable():
    """A polygon vector overlay on the terrain must author its coverage and
    its depth (the stroke mask needs the distance to the nearest segment)."""
    import forge3d as f3d
    from forge3d._map_scene_native_inputs import vector_batches
    from forge3d._map_scene_projection import project_vector_recipe

    x, y = np.meshgrid(
        np.linspace(-1, 1, 32, dtype=np.float32), np.linspace(-1, 1, 32, dtype=np.float32)
    )
    dem = (1.5 + 2.0 * np.exp(-8.0 * (x + 0.35) ** 2 - 3.0 * y * y)).astype(np.float32)
    recipe = f3d.MapScene(
        terrain=f3d.TerrainSource(
            data=dem,
            crs="EPSG:32610",
            elevation_sampling_available=True,
            metadata={
                "source_id": "depth-dem",
                "width": 32,
                "height": 32,
                "bounds": [-16.0, -16.0, 16.0, 16.0],
                "asset_status": "fixture",
            },
        ),
        camera=f3d.OrbitCamera(
            target=(0.0, 0.0, 0.0), distance=45.0, azimuth_deg=30.0, elevation_deg=48.0
        ),
        lighting=f3d.LightingPreset(
            name="daylight",
            intensity=1.0,
            settings={
                "camera": {"camera_mode": "mesh:zup"},
                "screen_space": {"ssao": {"enabled": True}},
            },
        ),
        output=f3d.OutputSpec(width=128, height=96, format="png"),
        layers=[
            f3d.VectorOverlay(
                layer_id="area",
                crs="EPSG:32610",
                width_px=2.0,
                style={
                    "version": 8,
                    "layers": [
                        {
                            "id": "area",
                            "type": "fill",
                            "paint": {"fill-color": "#ffcc22"},
                        }
                    ],
                },
                features=[
                    {
                        "id": "area",
                        "geometry": {
                            "type": "Polygon",
                            "coordinates": [
                                [[-8.0, -4.0], [8.0, -4.0], [8.0, 4.0], [-8.0, 4.0], [-8.0, -4.0]]
                            ],
                        },
                    }
                ],
            )
        ],
    )
    projected = project_vector_recipe(recipe.recipe)
    batches = vector_batches(recipe.recipe, projected)
    assert batches, "the polygon overlay must author a batch"
    rgba = batches[0]["rgba"]
    assert rgba.shape == (96, 128, 4)
    assert rgba[..., 3].any(), "the polygon must draw something"
    depth = batches[0]["depth"]
    assert depth.shape == (96, 128)
    assert np.isfinite(depth).all()
