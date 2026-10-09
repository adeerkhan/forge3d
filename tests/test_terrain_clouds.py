# tests/test_terrain_clouds.py
# Phase 1: the offline cloud composite, and its disabled no-op guarantee.
import numpy as np
import pytest

from _terrain_runtime import (
    _build_heightmap,
    _build_overlay,
    _write_test_hdr,
    terrain_rendering_available,
)
from forge3d.terrain_params import (
    CloudSettings,
    OfflineQualitySettings,
    PomSettings,
    make_terrain_params_config,
)

forge3d = pytest.importorskip("forge3d")

if not terrain_rendering_available():
    pytest.skip(
        "Terrain cloud tests require a terrain-capable hardware-backed forge3d runtime",
        allow_module_level=True,
    )


def _render_offline(renderer, material_set, heightmap, env_maps, clouds, cam_radius=4.6):
    params = forge3d.TerrainRenderParams(
        make_terrain_params_config(
            size_px=(96, 64),
            render_scale=1.0,
            terrain_span=2.8,
            msaa_samples=1,
            z_scale=1.35,
            exposure=1.0,
            domain=(0.0, 1.0),
            albedo_mode="colormap",
            colormap_strength=1.0,
            ibl_enabled=True,
            light_azimuth_deg=138.0,
            light_elevation_deg=24.0,
            sun_intensity=2.4,
            cam_radius=cam_radius,
            cam_phi_deg=138.0,
            cam_theta_deg=58.0,
            fov_y_deg=54.0,
            camera_mode="mesh",
            overlays=[_build_overlay()],
            pom=PomSettings(False, "Occlusion", 0.0, 1, 1, 0, False, False),
            clouds=clouds,
        )
    )
    settings = OfflineQualitySettings(
        enabled=True, adaptive=False, batch_size=1, max_samples=4, min_samples=4
    )
    result = forge3d.render_offline(
        renderer, material_set, env_maps, params, heightmap, settings=settings
    )
    return np.asarray(result.frame.to_numpy())


def _setup(tmp_path):
    hdr_path = tmp_path / "clouds_env.hdr"
    _write_test_hdr(hdr_path)
    session = forge3d.Session(window=False)
    renderer = forge3d.TerrainRenderer(session)
    material_set = forge3d.MaterialSet.terrain_default()
    heightmap = _build_heightmap()
    env_maps = forge3d.IBL.from_hdr(str(hdr_path), intensity=1.0)
    return renderer, material_set, heightmap, env_maps


def test_disabled_clouds_render_is_noop(tmp_path):
    """Disabled clouds, even at extreme values, must match the no-config baseline."""
    renderer, material_set, heightmap, env_maps = _setup(tmp_path)

    baseline = _render_offline(renderer, material_set, heightmap, env_maps, None)
    disabled = _render_offline(
        renderer,
        material_set,
        heightmap,
        env_maps,
        CloudSettings(
            enabled=False,
            coverage=0.99,
            density=0.99,
            shadow_strength=0.99,
            altitude_m=2.0,
            thickness_m=4.0,
            scatter_strength=3.0,
            phase_g=0.95,
            detail=0.9,
            wind_dir=180.0,
            wind_speed=9.0,
            powder=0.1,
        ),
    )

    assert baseline.shape == disabled.shape
    assert baseline.tobytes() == disabled.tobytes()


def test_enabled_clouds_change_output(tmp_path):
    """An enabled cloud slab in front of the terrain must change the pixels."""
    renderer, material_set, heightmap, env_maps = _setup(tmp_path)

    baseline = _render_offline(renderer, material_set, heightmap, env_maps, None)
    enabled = _render_offline(renderer, material_set, heightmap, env_maps, _in_view_clouds())

    assert enabled.shape == baseline.shape
    assert enabled.tobytes() != baseline.tobytes()


def _in_view_clouds():
    # Mirror the example so the test exercises the same look (deck above the
    # ~1.35 peaks, moderate scatter). A deck based below the peaks, or an
    # extreme scatter_strength, hides the very quality the tests guard.
    return CloudSettings(
        enabled=True,
        coverage=0.42,
        density=0.7,
        altitude_m=1.5,
        thickness_m=1.7,
        scatter_strength=2.5,
        phase_g=0.85,
        detail=0.5,
        powder=0.5,
    )


def _cloud_delta(renderer, material_set, heightmap, env_maps, clouds, cam_radius):
    baseline = _render_offline(
        renderer, material_set, heightmap, env_maps, None, cam_radius=cam_radius
    )
    enabled = _render_offline(
        renderer, material_set, heightmap, env_maps, clouds, cam_radius=cam_radius
    )
    return enabled.astype(np.float32) - baseline.astype(np.float32)


def test_degenerate_slab_is_noop(tmp_path):
    """A zero-thickness slab has no march interval, so it must not paint."""
    renderer, material_set, heightmap, env_maps = _setup(tmp_path)
    baseline = _render_offline(renderer, material_set, heightmap, env_maps, None)
    degenerate = _render_offline(
        renderer,
        material_set,
        heightmap,
        env_maps,
        CloudSettings(enabled=True, altitude_m=0.5, thickness_m=0.0, density=0.9),
    )
    assert degenerate.tobytes() == baseline.tobytes()


def test_cloud_field_tracks_camera_pose(tmp_path):
    """The cloud contribution is view-dependent (parallax), not a flat overlay."""
    renderer, material_set, heightmap, env_maps = _setup(tmp_path)
    clouds = _in_view_clouds()
    low = _cloud_delta(renderer, material_set, heightmap, env_maps, clouds, 4.6)
    high = _cloud_delta(renderer, material_set, heightmap, env_maps, clouds, 7.0)

    # `render_offline(...).frame.to_numpy()` is uint8 (0..255). Assert a visible
    # (not one-trivial-code) cloud contribution and a real pose-dependence, so a
    # near-empty or washed-out deck fails rather than passing on noise.
    near_mean = float(np.abs(low).mean())
    far_mean = float(np.abs(high).mean())
    parallax_mean = float(np.abs(low - high).mean())
    assert near_mean > 1.0, f"clouds barely changed the still (mean |delta| = {near_mean})"
    assert far_mean > 1.0, f"clouds barely changed the farther still (mean |delta| = {far_mean})"
    assert parallax_mean > 1.0, (
        f"cloud field did not track camera pose (mean |pose delta| = {parallax_mean})"
    )
