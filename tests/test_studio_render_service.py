"""Studio render service: sky encoding and colour-space helpers.

`studio_render.py` is the shell's embedded renderer, not an importable package
module, so it is loaded by path. These tests cover the pieces that are pure
maths — the parts a GPU golden cannot see:

* the studio HDR is a *valid* Radiance file (one broken exponent makes the
  engine decode a black environment and a blown-out sky pass), and
* the denoise path converts the display-referred frame into the linear space
  OIDN and its guidance images live in.
"""

from __future__ import annotations

import importlib.util
from pathlib import Path

import numpy as np
import pytest

SERVICE = (
    Path(__file__).resolve().parents[1]
    / "studio-tauri"
    / "src-tauri"
    / "src"
    / "studio_render.py"
)


def load_service():
    spec = importlib.util.spec_from_file_location("studio_render_service", SERVICE)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def decode_radiance_rgbe(data: bytes):
    """Decode an uncompressed `-Y h +X w` Radiance file to linear RGB."""
    marker = data.index(b"\n-Y ")
    header_end = data.index(b"\n", marker + 1)
    resolution = data[marker + 1 : header_end + 1].decode().strip()
    parts = resolution.split()
    height, width = int(parts[1]), int(parts[3])
    pixels = np.frombuffer(data[header_end + 1 :], dtype=np.uint8)
    assert pixels.size == height * width * 4
    rgbe = pixels.reshape(height, width, 4).astype(np.float32)
    exponent = rgbe[..., 3:4]
    return rgbe[..., :3] / 256.0 * np.exp2(exponent - 128.0)


def test_studio_sky_hdr_is_a_valid_radiance_file(tmp_path):
    service = load_service()
    path = tmp_path / "sky.hdr"
    service.write_sky_hdr(path)

    data = path.read_bytes()
    assert data.startswith(b"#?RADIANCE\n")
    assert b"FORMAT=32-bit_rle_rgbe" in data

    rgb = decode_radiance_rgbe(data)
    assert np.isfinite(rgb).all(), "every pixel must decode (biased exponent, not raw frexp)"
    assert rgb.max() > 0.5, "the sky must carry real radiance, not a black environment"

    # Equirectangular, row 0 = zenith: the upper half is the sky (blue at the
    # zenith), the lower half is the ground. An inverted map lights the terrain
    # from below.
    zenith = rgb[0].mean(axis=0)
    horizon = rgb[rgb.shape[0] // 2 - 1].mean(axis=0)
    nadir = rgb[-1].mean(axis=0)
    assert zenith[2] > zenith[0], "zenith must read blue"
    assert horizon[0] > nadir[0] and horizon[1] > nadir[1], "ground must be darker than the horizon"
    assert nadir.mean() < zenith.mean()


def test_srgb_round_trip_is_lossless_enough_for_8_bit():
    service = load_service()
    values = np.linspace(0.0, 1.0, 256, dtype=np.float32)
    linear = service.srgb_to_linear(values)
    back = service.linear_to_srgb(linear)
    assert np.allclose(back, values, atol=1e-6)
    # And the decode is the real transfer function, not a plain gamma.
    assert service.srgb_to_linear(np.array([0.5], dtype=np.float32))[0] == pytest.approx(
        0.21404114, rel=1e-5
    )


def test_denoise_is_applied_in_linear_space():
    """The frame is sRGB; OIDN and its albedo/normal guidance are linear."""
    service = load_service()
    source = SERVICE.read_text()
    assert "srgb_to_linear(rgb.astype(np.float32) / 255.0)" in source, (
        "the still must be decoded to linear before OIDN and re-encoded after"
    )
    assert "linear_to_srgb(denoised)" in source


def test_still_uses_the_physical_shading_model():
    """Solid/Material previews are three.js PBR; the still must match them."""
    service = load_service()
    source = SERVICE.read_text()
    assert 'terrain_shading_model="lambert_physical"' in source


def test_shared_hdr_writer_reproduces_the_canonical_environment_bytes(tmp_path):
    """`map_scene`'s recipe fallback must stay byte-identical through the shared
    writer — its pixels are exact 1/256 steps, so the bytes are the contract."""
    import numpy as np

    from forge3d.io import write_hdr

    path = tmp_path / "canonical.hdr"
    write_hdr(path, np.full((2, 2, 3), [180 / 256, 190 / 256, 205 / 256], np.float32))
    assert path.read_bytes() == (
        b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2 +X 2\n"
        + bytes([180, 190, 205, 128]) * 4
    )


def test_dem_span_reprojects_geographic_bounds_to_metres():
    """A degree-grid DEM must report a metric footprint, not degrees."""
    import numpy as np

    service = load_service()

    class FakeDem:
        data = np.zeros((32, 64), np.float32)
        crs = "EPSG:4326"
        bounds = (5.955885327836842, 45.81802022980842, 10.4924973226525, 47.80830791604507)
        resolution = (0.0006, 0.0008)
        transform = None

    span_x, span_y = service.dem_span(FakeDem())
    # The Swiss national DEM's footprint, from the same UTM zone the viewer uses.
    assert span_x == pytest.approx(352_426.79, abs=1.0)
    assert span_y == pytest.approx(225_653.68, abs=1.0)


def test_dem_span_uses_pixel_size_for_a_projected_raster():
    """Bryce's UTM raster needs no reprojection: resolution x shape is metres."""
    import numpy as np

    service = load_service()

    class FakeDem:
        data = np.zeros((1024, 1024), np.float32)
        crs = "EPSG:26912"
        bounds = (395998.5, 4161774.5, 399998.5, 4165574.5)
        resolution = (3.90625, 3.7109375)
        transform = None

    assert service.dem_span(FakeDem()) == (4000.0, 3800.0)


class _SkyStub:
    """`sky_settings` only needs the class's model map, not a GPU session."""

    SKY_MODELS = {}


@pytest.fixture
def sky_settings():
    service = load_service()
    stub = _SkyStub()
    stub.SKY_MODELS = service.StudioRenderer.SKY_MODELS
    return lambda request, quality, environment: service.StudioRenderer.sky_settings(
        stub, request, quality, environment
    )


def test_hdri_owns_the_sky_and_keeps_the_backdrop_intensity(sky_settings):
    """The project HDRI branch must not read the analytic fields at all."""
    settings = sky_settings(
        {"environmentBackgroundIntensity": 0.7},
        {"sky": True, "skyModel": "preetham", "skyTurbidity": 8.0, "skyExposure": 0.9},
        "/maps/studio.hdr",
    )
    assert settings.model == "hdri"
    assert settings.sky_exposure == pytest.approx(0.7)
    # The analytic values are ignored, not merged into the HDRI sky.
    assert settings.turbidity == pytest.approx(2.0)


def test_the_analytic_sky_carries_the_projects_values(sky_settings):
    settings = sky_settings(
        {"environmentBackgroundIntensity": 1.0},
        {
            "sky": True,
            "skyModel": "preetham",
            "skyTurbidity": 6.5,
            "skyGroundAlbedo": 0.4,
            "skyExposure": 0.03,
        },
        None,
    )
    assert settings.model == "preetham"
    assert settings.turbidity == pytest.approx(6.5)
    assert settings.ground_albedo == pytest.approx(0.4)
    assert settings.sky_exposure == pytest.approx(0.03)


def test_aether_refuses_parameters_the_shipped_bake_does_not_cover(sky_settings):
    """A way-off turbidity must fail loudly, not render the wrong atmosphere."""
    with pytest.raises(RuntimeError, match="baked LUT"):
        sky_settings(
            {"environmentBackgroundIntensity": 1.0},
            {"sky": True, "skyModel": "aether", "skyTurbidity": 3.0, "skyGroundAlbedo": 0.3},
            None,
        )


class _WaterStub:
    """The two attributes `water_mask` reads: the scaled heights and the
    scale that produced them."""

    def __init__(self, heights, scale):
        self.heights = heights
        self.scale = scale


@pytest.fixture
def water():
    service = load_service()
    return service.StudioRenderer.water


def test_water_is_off_unless_the_project_asks_for_it(water):
    """No mask means no water: the terrain path shades the DEM as land."""
    stub = _WaterStub(heights=np.zeros((4, 4), dtype=np.float32), scale=1.0)
    assert water(stub, {}) == (None, None)
    assert water(stub, {"quality": {"water": {"enabled": False}}}) == (None, None)


def test_the_mask_floods_low_flat_ground_and_leaves_cliffs_dry(water):
    heights = np.zeros((4, 4), dtype=np.float32)
    heights[2, 2] = 10.0  # one cliff in the middle
    stub = _WaterStub(heights=heights, scale=1.0)
    mask, settings = water(
        stub,
        {"quality": {"water": {"enabled": True, "level": 0.0, "slopeThreshold": 0.02}}},
    )
    assert mask is not None
    assert mask.dtype == np.float32
    assert mask[0, 0] == 1.0  # flat and low: water
    assert mask[2, 2] == 0.0  # the cliff: dry
    # The water gets its own surface: the physical path, so the sun's
    # highlight is a band and not a field of per-cell glints.
    assert settings is not None
    assert settings.optics is not None
    assert settings.optics.mode == "physical"


def test_the_level_is_scaled_into_the_renderers_units(water):
    """The project speaks DEM metres; the renderer works in scaled heights."""
    heights = np.full((3, 3), 80.0, dtype=np.float32)
    stub = _WaterStub(heights=heights, scale=0.5)
    # 100 m in the DEM is 50 m in the scaled heights the renderer sees, so
    # ground at 80 m stays dry.
    mask, _settings = water(
        stub,
        {"quality": {"water": {"enabled": True, "level": 100.0, "slopeThreshold": 1.0}}},
    )
    assert mask is not None
    assert mask[1, 1] == 0.0


def test_level_zero_is_the_engines_own_auto_default(water):
    heights = np.zeros((4, 4), dtype=np.float32)
    heights[3, 3] = 100.0
    stub = _WaterStub(heights=heights, scale=1.0)
    mask, _settings = water(
        stub,
        {"quality": {"water": {"enabled": True, "level": 0.0, "slopeThreshold": 1.0}}},
    )
    assert mask is not None
    # Auto is the engine's 15th percentile, so most of the flat ground is wet.
    assert mask.mean() > 0.5
    assert mask[3, 3] == 0.0


def test_bloom_leaves_a_flat_frame_alone():
    """Below the threshold there is nothing to glow."""
    service = load_service()
    frame = np.full((16, 16, 3), 100, dtype=np.uint8)
    out = service.apply_bloom(frame.astype(np.float32) / 255.0, 0.8, 0.2, 0.5, 2)
    assert np.allclose(out, frame / 255.0, atol=1e-6)


def test_bloom_spreads_a_bright_pixel_into_its_neighbours():
    service = load_service()
    frame = np.zeros((32, 32, 3), dtype=np.float32)
    frame[16, 16] = 1.0
    out = service.apply_bloom(frame, 0.5, 0.0, 1.0, 2)
    assert out[16, 16, 0] > 0.0
    # The glow reaches a neighbour, which is the whole point of bloom.
    assert out[16, 17, 0] > 0.0
    # ... and a pixel far away stays dark.
    assert out[0, 0, 0] == 0.0


def test_bloom_intensity_scales_the_glow():
    service = load_service()
    frame = np.zeros((32, 32, 3), dtype=np.float32)
    frame[16, 16] = 1.0
    weak = service.apply_bloom(frame, 0.5, 0.0, 0.2, 2)
    strong = service.apply_bloom(frame, 0.5, 0.0, 1.0, 2)
    assert strong[16, 17, 0] > weak[16, 17, 0]


def test_vignette_darkens_the_corners_and_not_the_centre():
    service = load_service()
    frame = np.ones((64, 64, 3), dtype=np.float32)
    out = service.apply_vignette(frame, 1.0)
    assert out[32, 32, 0] == pytest.approx(1.0)   # the centre
    assert out[0, 0, 0] < 0.2                      # the corner


def test_vignette_zero_leaves_the_frame_alone():
    service = load_service()
    frame = np.ones((16, 16, 3), dtype=np.float32)
    assert np.allclose(service.apply_vignette(frame, 0.0), frame)


def test_post_effects_are_off_until_the_project_asks():
    service = load_service()
    frame = np.full((16, 16, 3), 128, dtype=np.uint8)
    out = service.apply_post_effects(frame, {})
    assert np.array_equal(out, frame)


def test_isolated_water_cells_are_dropped_and_a_lake_is_kept():
    """A blob of one cell is a speckle; a lake is not."""
    service = load_service()
    mask = np.zeros((16, 16), dtype=bool)
    mask[2, 2] = True                      # a single cell
    mask[8:14, 8:14] = True                # a real body
    kept = service._keep_water_blobs(mask, service.MIN_WATER_BLOB)
    assert kept[2, 2] == False
    assert kept[10, 10] == True
    assert int(kept.sum()) == 36


def test_a_diagonal_chain_of_cells_is_not_a_lake():
    """Four-connected blobs: a diagonal chain must not add up to a body."""
    service = load_service()
    mask = np.zeros((16, 16), dtype=bool)
    for i in range(10):
        mask[i, i] = True
    kept = service._keep_water_blobs(mask, service.MIN_WATER_BLOB)
    assert int(kept.sum()) == 0
