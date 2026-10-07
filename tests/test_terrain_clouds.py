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
    return CloudSettings(
        enabled=True,
        coverage=0.7,
        density=0.9,
        altitude_m=0.5,
        thickness_m=3.0,
        scatter_strength=3.0,
        phase_g=0.8,
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

    assert np.abs(low).mean() > 0.25
    assert np.abs(high).mean() > 0.25
    assert np.abs(low - high).mean() > 0.25
