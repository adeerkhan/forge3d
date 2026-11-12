"""The public label compositor.

`forge3d.composite_label_layers` is the engine's text compositor as a public
entry: a caller that owns its pixels (the studio's terrain frame) compiles a
`LabelPlan` and draws the accepted glyphs over its own image, with the
engine's packaged atlas, halo and rasterizer — the same stream
`MapScene.render` uses.

The contract these tests lock:

* the public function exists and is exported;
* a plan with an accepted label changes pixels, and the glyph lands where the
  plan's screen bounds say it does;
* a plan with nothing accepted draws nothing and says so;
* the private MapScene path still goes through the same code (no second
  compositor).
"""

from __future__ import annotations

import numpy as np
import pytest

import forge3d as f3d


def _plan_for(text: str, anchor, *, viewport=(640, 480), typography=None):
    """A compiled plan for one point label at a screen anchor."""
    atlas = f3d.FontAtlas.default_latin()
    layer = f3d.LabelLayer.from_features(
        [
            {
                "id": "label-0",
                "geometry": {"type": "Point", "coordinates": [0.0, 0.0, 0.0]},
                "properties": {"name": text},
            }
        ],
        text="name",
        crs=None,
        target_crs=None,
        occlusion="none",
        layer_id="labels",
        atlas=atlas,
    )
    layer.labels[0]["projected_anchor"] = list(anchor)
    plan = f3d.LabelPlan.compile(
        labels=layer.labels,
        camera=None,
        viewport=viewport,
        glyph_atlas=layer.glyph_atlas,
        typography=typography or {
            "font_size": 16,
            "color": [255, 255, 255, 255],
            "halo_color": [0, 0, 0, 220],
            "halo_width_px": 1.5,
        },
    )
    return layer, plan


def test_the_compositor_is_public():
    assert "composite_label_layers" in f3d.__all__
    assert callable(f3d.composite_label_layers)


def test_an_accepted_label_changes_pixels_where_the_plan_says():
    layer, plan = _plan_for("Luxembourg City", (320.0, 200.0, 0.5))
    assert [label.text for label in plan.accepted] == ["Luxembourg City"]

    base = np.zeros((480, 640, 3), dtype=np.uint8)
    composited, rendered = f3d.composite_label_layers(
        base, [layer], {layer.layer_id: plan}
    )
    assert rendered is True
    pixels = np.asarray(composited)
    assert pixels.shape[2] == 4

    changed = np.abs(pixels[:, :, :3].astype(np.int32) - base.astype(np.int32)).max(axis=2)
    assert changed.sum() > 0, "the label must reach the pixels"

    # The glyphs land inside the bounds the plan published for the label.
    x0, y0, x1, y1 = plan.accepted[0].screen_bounds
    window = changed[int(y0) - 4 : int(y1) + 4, int(x0) - 4 : int(x1) + 4]
    assert window.sum() > 0, "the glyph must land on the planned screen bounds"


def test_a_plan_with_nothing_accepted_draws_nothing():
    layer, plan = _plan_for("", (320.0, 200.0, 0.5))
    assert list(plan.accepted) == []

    base = np.zeros((480, 640, 3), dtype=np.uint8)
    composited, rendered = f3d.composite_label_layers(
        base, [layer], {layer.layer_id: plan}
    )
    assert rendered is False
    assert np.asarray(composited).shape[:2] == base.shape[:2]


def test_the_mapscene_path_uses_the_same_compositor():
    """The public entry and MapScene's own path must not drift apart."""
    from forge3d import map_scene

    layer, plan = _plan_for("Luxembourg City", (320.0, 200.0, 0.5))

    class _Recipe:
        layers = [layer]

    base = np.zeros((480, 640, 3), dtype=np.uint8)
    through_recipe, rendered_recipe = map_scene._composite_native_label_layers(
        base, _Recipe(), {layer.layer_id: plan}
    )
    through_public, rendered_public = f3d.composite_label_layers(
        base, [layer], {layer.layer_id: plan}
    )
    assert rendered_recipe is rendered_public is True
    assert np.array_equal(np.asarray(through_recipe), np.asarray(through_public))


def test_a_missing_glyph_is_rejected_by_the_plan_not_drawn():
    """The packaged atlas is Basic Latin: a label outside it is rejected."""
    layer, plan = _plan_for("五合目", (320.0, 200.0, 0.5))
    assert list(plan.accepted) == []
    assert [label.reason for label in plan.rejected] == ["missing_glyph"]
