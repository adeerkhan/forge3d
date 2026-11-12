"""The cloud shadows must reach the physical lighting model.

The CONCORDIA path specializes the terrain shader: it rewrites the stylized
path's `combined_shadow` so the cloud visibility multiplies it, and it has to
do the same for the physical model (OD-9c), which takes its visibility from
the production estimators instead of `combined_shadow`.

Without the second rewrite the clouds are invisible to every surface that
uses the physical model — measured: 0 changed samples on the studio's own
frame before, 361,779 with it.

The contract lives in a source-level test because the CONCORDIA module is
behind the `extension-module` feature, which CI never builds: a Rust unit
test there would never run.
"""

from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPECIALIZATION = ROOT / "src/terrain/renderer/interactive/shaders.rs"
SHADER = ROOT / "src/shaders/terrain_pbr_pom.wgsl"


def _specialization_source() -> str:
    return SPECIALIZATION.read_text(encoding="utf-8")


def test_the_physical_model_consumes_the_cloud_visibility() -> None:
    source = _specialization_source()
    assert (
        "let terrain_visibility = det_barrier(shadow_visibility * sun_vis_sample "
        "* concordia_cloud_visibility(input.world_position));" in source
    ), "the physical lighting path must consume the cloud visibility"


def test_the_stylized_model_still_consumes_the_cloud_visibility() -> None:
    source = _specialization_source()
    assert (
        "let combined_shadow = shadow_clamped * sun_vis_clamped "
        "* concordia_cloud_visibility(input.world_position);" in source
    ), "the stylized lighting path must consume the cloud visibility"


def test_both_rewrites_target_lines_the_shader_still_has() -> None:
    """A rewrite whose anchor text drifted would fail at render time; the
    contract is checked here so the drift is caught by the test lane."""
    shader = SHADER.read_text(encoding="utf-8")
    assert (
        "let terrain_visibility = det_barrier(shadow_visibility * sun_vis_sample);"
        in shader
    ), "the physical model's visibility line moved; update the CONCORDIA contract"
    assert (
        "let combined_shadow = shadow_clamped * sun_vis_clamped;" in shader
    ), "the stylized model's shadow line moved; update the CONCORDIA contract"


def test_the_cloud_field_has_an_absolute_world_scale() -> None:
    """The cloud pattern is a world feature, not a fraction of the scene: the
    same clouds must look the same on a 256 m plot and on a 350 km DEM."""
    interactive = ROOT / "src/terrain/renderer/interactive.rs"
    source = interactive.read_text(encoding="utf-8")
    assert "CLOUD_WORLD_PERIOD_M" in source, (
        "the cloud field's world scale must be an absolute period, not the "
        "terrain's span: a pattern stretched over the whole terrain is "
        "invisible on a country-sized DEM"
    )
    assert "1.0 / params.terrain_span" not in source, (
        "the cloud field cannot be scaled by the terrain's span: the pattern "
        "changes with the scene size"
    )
