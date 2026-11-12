"""The clouds must reach the physical lighting model, measured.

The CONCORDIA path's cloud visibility used to multiply only the stylized
path's `combined_shadow`; the physical model (OD-9c) — the one the studio's
still uses — took its visibility from the production estimators, so the clouds
were invisible to it: 0 changed samples on the studio's own frame.

This renders the same flat scene through the engine's quality path twice,
with the clouds off and on, and requires the pixels to move. It is a GPU
test: the generic CI runners have no hardware adapter, so it is skipped
there and runs on the physical lanes.
"""

from __future__ import annotations

import os

import numpy as np
import pytest

import forge3d as f3d
from forge3d.terrain_params import (
    CloudSettings,
    ScreenSpaceSettings,
    make_terrain_params_config,
)


def _has_gpu() -> bool:
    if os.environ.get("FORGE3D_NO_BOOTSTRAP"):
        return False
    try:
        return bool(f3d.has_gpu())
    except Exception:  # noqa: BLE001 - no adapter is a skip, not a failure
        return False


pytestmark = pytest.mark.skipif(not _has_gpu(), reason="requires a hardware adapter")


def _sky_hdr(tmp_path) -> str:
    """A tiny deterministic Radiance fixture the engine's IBL accepts."""
    path = tmp_path / "sky.hdr"
    payload = bytearray(b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2 +X 4\n")
    payload.extend(bytes([128, 128, 128, 128]) * 8)
    path.write_bytes(bytes(payload))
    return str(path)


def _render(tmp_path, clouds):
    """One frame through the engine's quality path, on a flat DEM."""
    session = f3d.Session(window=False, backend="vulkan")
    renderer = f3d.TerrainRenderer(session)
    materials = f3d.MaterialSet.terrain_default()
    ibl = f3d.IBL.from_hdr(_sky_hdr(tmp_path), intensity=1.0)
    heights = np.zeros((256, 256), dtype=np.float32)
    params = f3d.TerrainRenderParams(
        make_terrain_params_config(
            size_px=(320, 240),
            render_scale=1.0,
            msaa_samples=1,
            terrain_span=256.0,
            z_scale=1.0,
            exposure=1.35,
            domain=(0.0, 10.0),
            camera_mode="mesh:zup:north:dem",
            cam_radius=300.0,
            cam_phi_deg=60.0,
            cam_theta_deg=45.0,
            cam_target=(0.0, 0.0, 0.0),
            fov_y_deg=45.0,
            clip=(1.0, 10000.0),
            # The physical model: the one the studio renders with.
            terrain_shading_model="lambert_physical",
            screen_space=ScreenSpaceSettings(enabled=True, taa_enabled=True),
        )
    )
    frame = renderer.render_interactive(
        materials, ibl, params, heights, clouds=clouds
    )
    return np.asarray(frame.to_numpy())[:, :, :3].astype(np.int32)


def test_the_clouds_reach_the_physical_lighting_model(tmp_path):
    clouds = CloudSettings(
        enabled=True,
        shadows_enabled=True,
        coverage=0.9,
        density=0.9,
        shadow_strength=1.0,
        quality="high",
    )
    without = _render(tmp_path, None)
    with_clouds = _render(tmp_path, clouds)
    difference = np.abs(without - with_clouds)
    changed = int((difference > 0.5).sum())
    assert changed > 1000, (
        "the cloud shadows must reach the physical lighting model "
        f"(changed {changed} samples)"
    )
