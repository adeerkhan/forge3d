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
import types
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


_SERVICE_MODULE = None


def _engine_height_domain(heights):
    """The service's own helper, from the module loaded once."""
    global _SERVICE_MODULE
    if _SERVICE_MODULE is None:
        _SERVICE_MODULE = load_service()
    return _SERVICE_MODULE.engine_height_domain(heights)


class _WaterStub:
    """What the water and fog helpers read: the scaled heights, the scale that
    produced them, and the DEM's elevation range."""

    def __init__(self, heights, scale, range=(0.0, 100.0)):
        self.heights = heights
        self.scale = scale
        self.range = range
        # The renderer's domain for the heights it is given.
        self.scaled_range = _engine_height_domain(heights)


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


def test_fog_is_off_unless_the_project_asks_for_it():
    service = load_service()
    stub = _WaterStub(heights=np.zeros((4, 4), dtype=np.float32), scale=1.0)
    assert service.StudioRenderer.fog_settings(stub, {}) is None
    assert service.StudioRenderer.fog_settings(stub, {"fog": {"enabled": False}}) is None


def test_fog_carries_the_projects_density_and_falloff():
    service = load_service()
    stub = _WaterStub(
        heights=np.zeros((4, 4), dtype=np.float32), scale=1.0, range=(0.0, 450.0)
    )
    settings = service.StudioRenderer.fog_settings(
        stub,
        {"fog": {"enabled": True, "density": 1.5, "reach": 0.4, "inscatter": [1.0, 0.9, 0.8]}},
    )
    assert settings is not None
    assert settings.density == pytest.approx(1.5)
    assert settings.inscatter == pytest.approx((1.0, 0.9, 0.8))
    # Reach is a fraction of the terrain's height; the engine wants a falloff
    # per metre.
    assert settings.height_falloff > 0.0


def test_the_water_roughness_reaches_the_optics(water):
    """The roughness is the knob that widens the sun's highlight; it must
    reach the engine, not stay a constant in the service."""
    stub = _WaterStub(
        heights=np.zeros((8, 8), dtype=np.float32), scale=1.0, range=(0.0, 100.0)
    )
    _mask, settings = water(
        stub,
        {
            "quality": {
                "water": {
                    "enabled": True,
                    "level": 0.0,
                    "slopeThreshold": 1.0,
                    "roughness": 0.7,
                }
            }
        },
    )
    assert settings is not None
    assert settings.optics is not None
    assert settings.optics.roughness == pytest.approx(0.7)


def test_a_single_band_classification_overlay_is_palette_mapped():
    """A land-cover raster is class IDs, not a picture: reading band 1 as the
    red channel paints it grey. The classes go through the palette instead."""
    service = load_service()
    assert len(service.CLASSIFICATION_PALETTE) >= 8
    # The palette is the engine gallery's land cover: water, trees, crops, ...
    assert service.CLASSIFICATION_PALETTE[0] == (65, 155, 223)
    assert service.CLASSIFICATION_PALETTE[4] == (196, 40, 27)


def test_the_colormap_is_built_from_the_engines_own_catalogue():
    """`get_colormap` returns a Colormap; the terrain takes a Colormap1D built
    from stops over the elevation domain, wrapped in an OverlayLayer."""
    service = load_service()
    stub = _WaterStub(
        heights=np.zeros((4, 4), dtype=np.float32), scale=1.0, range=(100.0, 500.0)
    )
    assert service.StudioRenderer.colormap_overlay(stub, {}) is None
    layer = service.StudioRenderer.colormap_overlay(stub, {"colormap": "viridis"})
    assert layer is not None
    # Strength 0 is off. A bound colormap at strength 0 still shifted the
    # material layer blend by 162,905 samples (the shader normalizes the
    # height the layers sample by the colormap domain), so the service does
    # not bind one: measured 0 samples against the same scene with none.
    assert (
        service.StudioRenderer.colormap_overlay(
            stub, {"colormap": "viridis", "colormapStrength": 0.0}
        )
        is None
    )
    # An unknown name must fail loudly, not render a silent grey terrain.
    with pytest.raises(RuntimeError, match="unknown colormap"):
        service.StudioRenderer.colormap_overlay(stub, {"colormap": "nope"})
    # One LUT slot per frame: a raster overlay and a colormap cannot both
    # apply, and dropping one silently would be a lie about the image.
    with pytest.raises(RuntimeError, match="single colormap binding"):
        service.StudioRenderer.colormap_overlay(
            stub, {"colormap": "viridis"}, has_raster_overlay=True
        )


def test_sun_visibility_is_off_unless_asked_and_maps_the_rays():
    """Terrain self-shadowing is a ray march over the heightfield: off means
    `None` (the engine's own default), on carries the reach and step count."""
    service = load_service()
    stub = _WaterStub(
        heights=np.zeros((4, 4), dtype=np.float32), scale=1.0, range=(0.0, 100.0)
    )
    assert service.StudioRenderer.sun_visibility_settings(stub, {}) is None
    settings = service.StudioRenderer.sun_visibility_settings(
        stub,
        {"sunVisibility": {"enabled": True, "distance": 5000.0, "steps": 48}},
    )
    assert settings is not None
    assert settings.max_distance == 5000.0
    assert settings.steps == 48
    # One hard ray: the engine's soft mode moved 490 of 256,000 samples.
    assert settings.mode == "hard"
    assert settings.samples == 1


def test_despeckle_absorbs_isolated_classes_and_keeps_blocks():
    """The gallery land-cover recipe's 3x3 rule: a cell whose own class holds
    at most 2 cells of its block joins the block's majority (>= 4 cells)."""
    service = load_service()
    classes = np.full((9, 9), 2, dtype=np.int32)
    classes[4, 4] = 7  # one speck of class 7 in class 2
    classes[0:3, 0:3] = 5  # a 3x3 block of class 5 is real, not a speck
    cleaned = service.despeckle_classes(classes)
    assert cleaned[4, 4] == 2, "an isolated cell must take its neighbours' class"
    assert (cleaned[0:3, 0:3] == 5).all(), "a 3x3 block is not a speck"
    # 81 cells: 9 stay class 5, so 72 are class 2.
    assert (cleaned == 2).sum() == 72
    # Nothing happens on a clean raster.
    assert np.array_equal(service.despeckle_classes(np.full((5, 5), 3)), np.full((5, 5), 3))


def test_the_legend_is_the_engines_and_needs_a_colormap():
    """`forge3d.legend.Legend` draws the bar and ticks with packaged text; the
    compositor only runs when a colormap is selected."""
    service = load_service()
    frame = np.full((120, 200, 3), 40, dtype=np.uint8)
    assert np.array_equal(
        service.compose_legend(frame, "viridis", (0.0, 4000.0)).shape, frame.shape
    )
    drawn = service.compose_legend(frame, "viridis", (0.0, 4000.0))
    assert drawn.shape == frame.shape
    # The legend lands bottom-right and leaves the top-left untouched.
    assert (drawn[:60, :100] == frame[:60, :100]).all()
    assert not (drawn[60:, 100:] == frame[60:, 100:]).all()


def test_the_overlay_texture_follows_the_source_not_the_frame():
    """A drape used to be decimated to the frame — 390 m blocks on a 900 px
    frame. The texture now keeps the source's own resolution up to the engine's
    overlay ceiling, so a close-up still shows the land cover's classes."""
    service = load_service()
    size = service.StudioRenderer.overlay_texture_size
    stub = object.__new__(service.StudioRenderer)
    # The bundled Swiss land cover, 10055 x 22069, capped on its long side.
    assert size(stub, 10055, 22069) == (1866, 4096)
    # A source smaller than the ceiling keeps its own pixels.
    assert size(stub, 900, 1200) == (900, 1200)


def test_the_overlay_rgba_is_decoded_once_per_file(tmp_path):
    """A national 10 m raster costs seconds to decode; opacity changes and the
    next frames must reuse it, not read the file again."""
    service = load_service()
    source = tmp_path / "overlay.tif"
    source.write_bytes(b"not really a tif")  # identity only: stat() is all we read
    stub = object.__new__(service.StudioRenderer)
    stub._overlay_rgba_cache = {}
    stub._overlay_rgba_limit = 2
    stub.f3d = types.SimpleNamespace(
        OverlayLayer=types.SimpleNamespace(
            from_raster_rgba=lambda rgba, strength: ("layer", strength, rgba.shape)
        )
    )
    calls = []

    def read(overlay):
        calls.append(overlay["path"])
        return np.zeros((4, 6, 4), dtype=np.uint8)

    stub.read_overlay_rgba = read
    overlay = {"path": str(source), "opacity": 0.82}
    first = service.StudioRenderer.build_overlay(stub, overlay)
    second = service.StudioRenderer.build_overlay(stub, {**overlay, "opacity": 0.4})
    assert len(calls) == 1, "the second frame must not re-read the raster"
    assert first[1] == 0.82 and second[1] == 0.4, "opacity is per request"
    # A different despeckle is a different decode.
    service.StudioRenderer.build_overlay(stub, {**overlay, "despeckle": True})
    assert len(calls) == 2


def test_compose_plate_cuts_out_on_depth_and_lays_the_subject_on_white():
    """The plate's cut-out is the renderer's depth buffer, not a colour key:
    a dark valley (depth > 0) must survive against the white page."""
    service = load_service()
    frame = np.zeros((40, 60, 3), dtype=np.uint8)
    frame[:, :] = (20, 24, 30)  # background: dark, but never used
    frame[10:30, 15:45] = (120, 90, 60)  # the "subject"
    depth = np.zeros((40, 60), dtype=np.uint8)
    depth[10:30, 15:45] = 29
    plate = service.compose_plate(
        frame, depth, title="Land cover", legend=None, caption="Studio - dem"
    )
    assert plate.ndim == 3 and plate.shape[2] == 3
    # The page is white outside the subject.
    assert tuple(plate[0, 0]) == (255, 255, 255)
    # The subject keeps its own pixels and its own size inside the page: the
    # plate crops to the depth mask, it does not resample the frame.
    subject = (plate == np.array([120, 90, 60], dtype=np.uint8)).all(axis=2)
    rows = np.flatnonzero(subject.any(axis=1))
    cols = np.flatnonzero(subject.any(axis=0))
    assert rows.size > 0 and cols.size > 0
    assert (rows[-1] - rows[0] + 1, cols[-1] - cols[0] + 1) == (20, 30)
    # Without a mask (background frames) the frame comes back untouched.
    empty = np.zeros((8, 8), dtype=np.uint8)
    same = service.compose_plate(np.zeros((8, 8, 3), dtype=np.uint8), empty, title="x")
    assert same.shape == (8, 8, 3)


def test_the_scale_bar_picks_a_round_length_and_labels_it():
    """A scale bar reads in round distances: 1, 2 or 5 times a power of ten.
    (The first version of this loop multiplied by 1.0 and never terminated.)"""
    service = load_service()
    nice = service._nice_length
    assert nice(98_900.0) == 50_000.0
    assert nice(1_000.0) == 1_000.0
    assert nice(1_500.0) == 1_000.0
    assert nice(6_000.0) == 5_000.0
    assert nice(0.0) == 0.0
    assert nice(float("inf")) == 0.0
    assert service._scale_label(50_000.0) == "50 km"
    assert service._scale_label(500.0) == "500 m"


def test_a_class_map_export_snaps_back_to_the_palette():
    """A land-cover export is often an *image* of the classes. The gallery's own
    recipe snaps each pixel to the nearest palette entry; the same snap lets
    the plate name the classes. An orthophoto is not a class map and is left
    alone."""
    service = load_service()
    palette = service.CLASSIFICATION_PALETTE
    image = np.zeros((4, 4, 3), dtype=np.float32)
    image[0, 0] = palette[0]  # water
    image[0, 1] = palette[3]  # crops
    image[1, :] = palette[1]  # trees
    classes, ids = service.snap_to_classes(image)
    assert classes is not None, "a palette-coloured raster is a class map"
    assert ids == [1, 2, 4], ids
    assert classes[0, 0] == 1 and classes[0, 1] == 4 and classes[1, 0] == 2

    # An image that is not made of palette colours stays an image — including
    # a grey one, which would otherwise snap to the palette's spare grey.
    photo = np.full((8, 8, 3), 120.0, dtype=np.float32)
    photo[0, 0] = (17.0, 200.0, 233.0)
    classes, ids = service.snap_to_classes(photo)
    assert classes is None and ids == []

    # Everything below the invalid threshold is nodata, as the gallery's rule
    # has it.
    dark = np.zeros((2, 2, 3), dtype=np.float32)
    assert service.snap_to_classes(dark) == (None, [])


def test_the_class_legend_lists_the_classes_the_map_shows():
    service = load_service()
    legend = service.plate_class_legend([1, 2, 4])
    assert legend.shape[0] > 40 and legend.shape[1] > 100, legend.shape
    assert legend.shape[2] == 4
    # No classes, no legend.
    assert service.plate_class_legend([]).shape == (1, 1, 4)


def test_the_engine_height_domain_describes_the_heights_it_is_given():
    """The renderer clamps and normalizes the heightmap against the domain, so
    a scaled DEM needs the *scaled* range. Handing it the DEM's own range made
    every country-scale DEM a flat plate (Switzerland rendered flat; Bryce, at
    scale 1, did not)."""
    service = load_service()
    import numpy as np

    raw = (189.0, 4220.0)
    scale = 0.085
    scaled = np.asarray([raw[0] * scale, raw[1] * scale], dtype=np.float64)
    domain = service.engine_height_domain(scaled)
    assert domain == pytest.approx((scaled.min(), scaled.max()))
    # The scaled domain is not the DEM's own range, and it is the one the
    # renderer needs.
    assert domain != pytest.approx(raw)
    assert domain[0] <= scaled.min() and domain[1] >= scaled.max()
    # A flat DEM still gets a usable domain.
    flat = service.engine_height_domain(np.full((4, 4), 12.0))
    assert flat[1] > flat[0]
    # No finite samples must not crash.
    assert service.engine_height_domain(np.array([np.nan, np.nan])) == (0.0, 1.0)


def test_the_vector_reader_reads_a_geopackage_without_a_third_party_package():
    """A GeoPackage is a SQLite database with GeoPackage binary geometry; the
    studio's environment has no geopandas, fiona or pyproj (verified)."""
    service = load_service()
    path = Path(__file__).resolve().parents[1] / "assets/gpkg/luxembourg_rail.gpkg"
    lines, crs = service.read_vector_lines(str(path))
    assert crs == "EPSG:4326"
    assert len(lines) > 2000, "the rail network has 2,035 features"
    points = [point for line in lines for point in line]
    assert all(len(line) >= 2 for line in lines)
    # The lines fall inside Luxembourg, which is the DEM's own footprint.
    assert 5.7 < min(p[0] for p in points) < 5.9
    assert 6.4 < max(p[0] for p in points) < 6.6
    assert 49.4 < min(p[1] for p in points) < 49.6
    assert 50.1 < max(p[1] for p in points) < 50.3


def test_the_vector_reader_rejects_a_file_it_cannot_read():
    service = load_service()
    with pytest.raises(RuntimeError, match="GeoPackage"):
        service.read_vector_lines("definitely-not-a-vector.tif")


def test_the_ground_scale_is_metres_per_pixel():
    """The vector width is a real width in metres; the rasterizer wants pixels,
    so the two are converted with the frame's ground scale."""
    service = load_service()
    # A camera 10,000 m away (in the renderer's units), a 45 degree field of
    # view and a 1,000x1,000 frame: the frame is about 8,284 m wide.
    scale = service.ground_scale_metres_per_pixel(10_000.0, 45.0, 1000, 1000, 1.0)
    assert 8.0 < scale < 8.5, scale
    # A scaled world (the service's country-scale budget) divides the metres.
    scaled = service.ground_scale_metres_per_pixel(10_000.0, 45.0, 1000, 1000, 0.5)
    assert scaled == pytest.approx(scale * 2.0)
    assert service.ground_scale_metres_per_pixel(0.0, 45.0, 1000, 1000, 1.0) == 0.0
