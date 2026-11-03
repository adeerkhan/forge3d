"""Forge3D Studio's embedded renderer.

A persistent offscreen renderer driven by the Studio shell over stdin/stdout.
No viewer window: this is what "Rendered" mode in the Studio viewport uses, and
what a still render will use.

Protocol (newline-delimited JSON):

  stdin  line 1   {"dem": "/path/dem.tif", "zScale": 1.0}
  stdin  line n   {"id": 1, "width": 960, "height": 600,
                   "camera": {"phiDeg": .., "thetaDeg": .., "radius": ..,
                              "target": [x, y, z], "fovDeg": ..},
                   "sun": {"azimuthDeg": .., "elevationDeg": .., "intensity": ..},
                   "quality": {"samples": 1, "shadows": true, "pom": true},
                   "output": "/optional/path.png"}
  stdout          {"event": "ready", "dem": "...", "span": [w, h]}
                  {"id": 1, "ok": true, "width": 960, "height": 600,
                   "ms": 1234, "png": "<base64>"}

The renderer, session, and environment map are built once: shader compilation and
IBL prefiltering dominate start-up, and a viewport needs frames in sequence.
"""

from __future__ import annotations

import base64
import io
import json
import os
import sys
import sysconfig
import tempfile
import traceback
from pathlib import Path
from typing import Any, Dict, Optional

import numpy as np


def ensure_native_runtime() -> None:
    """Make the interpreter's libpython loadable (uv keeps it outside venvs)."""
    if not sys.platform.startswith("linux"):
        return
    libdir = sysconfig.get_config_var("LIBDIR")
    if not libdir or not os.path.isdir(libdir):
        return
    existing = [part for part in os.environ.get("LD_LIBRARY_PATH", "").split(os.pathsep) if part]
    if libdir not in existing:
        os.environ["LD_LIBRARY_PATH"] = os.pathsep.join([libdir, *existing])


def emit(payload: Dict[str, Any]) -> None:
    print(json.dumps(payload), flush=True)


def fail(message: str) -> None:
    emit({"event": "error", "message": message})


def write_sky_hdr(path: Path, width: int = 128, height: int = 64) -> None:
    """A neutral studio sky.

    The repository ships no HDR environment and the renderer requires one, so the
    service writes its own: predictable, licence-free, and identical every run.
    """
    v = np.linspace(0.0, 1.0, height)[:, None]
    zenith = np.array([0.25, 0.40, 0.75])
    horizon = np.array([0.85, 0.72, 0.55])
    ground = np.array([0.10, 0.09, 0.08])
    sky = horizon[None, None, :] * (1.0 - v[..., None]) + zenith[None, None, :] * v[..., None]
    image = np.where(v[..., None] > 0.5, sky, np.broadcast_to(ground, sky.shape))
    image = np.broadcast_to(image.astype(np.float32) * 1.6, (height, width, 3)).copy()

    peak = image.max(axis=2)
    exponent = np.zeros_like(peak)
    nonzero = peak > 1e-32
    _, exp = np.frexp(peak)
    exponent[nonzero] = exp[nonzero]
    scale = np.where(nonzero, 256.0 / np.exp2(exponent), 0.0)
    rgb = np.clip(image * scale[..., None], 0, 255).astype(np.uint8)
    rgbe = np.concatenate([rgb, exponent.astype(np.uint8)[..., None]], axis=2)

    header = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n" + f"-Y {height} +X {width}\n".encode()
    path.write_bytes(header + rgbe.tobytes())


def load_dem(path: str) -> tuple[np.ndarray, float, float]:
    import rasterio

    with rasterio.open(path) as source:
        heights = source.read(1).astype(np.float32)
        transform = source.transform
        span_x = abs(float(transform.a)) * heights.shape[1]
        span_y = abs(float(transform.e)) * heights.shape[0]
    if span_x <= 0.0 or span_y <= 0.0:
        span_x = float(heights.shape[1])
        span_y = float(heights.shape[0])
    return heights, span_x, span_y


class StudioRenderer:
    def __init__(self, dem_path: str) -> None:
        import forge3d as f3d

        self.f3d = f3d
        self.heights, self.span_x, self.span_y = load_dem(dem_path)
        self.dem_path = dem_path
        self.range = (float(self.heights.min()), float(self.heights.max()))

        env_path = Path(tempfile.gettempdir()) / "forge3d-studio-sky.hdr"
        write_sky_hdr(env_path)

        self.session = f3d.Session(window=False, backend="vulkan")
        self.renderer = f3d.TerrainRenderer(self.session)
        self.ibl = f3d.IBL.from_hdr(str(env_path), intensity=1.0)
        self.materials = f3d.MaterialSet.terrain_default()
        self.size: Optional[tuple[int, int]] = None

    def render(self, request: Dict[str, Any]) -> Dict[str, Any]:
        f3d = self.f3d
        from forge3d.terrain_params import (
            AovSettings,
            DetailSettings,
            HeightAoSettings,
            PomSettings,
            SkySettings,
            make_terrain_params_config,
        )

        width = int(request.get("width", 960))
        height = int(request.get("height", 600))
        width = max(64, min(4096, width))
        height = max(64, min(4096, height))

        camera = request.get("camera") or {}
        sun = request.get("sun") or {}
        quality = request.get("quality") or {}

        span = self.span_x
        radius = float(camera.get("radius") or 1.5 * max(self.span_x, self.span_y))
        target = camera.get("target") or [self.span_x / 2, self.span_y / 2, float(self.heights.mean())]

        settings = make_terrain_params_config(
            size_px=(width, height),
            render_scale=1.0,
            msaa_samples=1,
            terrain_span=span,
            z_scale=float(request.get("zScale", 1.0)),
            camera_mode="mesh:zup:north:dem",
            cam_radius=radius,
            cam_phi_deg=float(camera.get("phiDeg", 135.0)),
            cam_theta_deg=float(camera.get("thetaDeg", 45.0)),
            cam_target=(float(target[0]), float(target[1]), float(target[2])),
            fov_y_deg=float(camera.get("fovDeg", 45.0)),
            clip=(10.0, 400000.0),
            domain=self.range,
            overlays=[],
            albedo_mode="colormap",
            hue_variation_strength=0.0,
            colormap_strength=1.0,
            light_azimuth_deg=float(sun.get("azimuthDeg", 135.0)),
            light_elevation_deg=float(sun.get("elevationDeg", 35.0)),
            sun_intensity=float(sun.get("intensity", 3.0)),
            ibl_enabled=True,
            exposure=1.35,
            # The SSAO pass costs ~4.8 s per frame by itself (measured); every
            # other feature combined is ~150 ms. Viewport frames therefore skip
            # it, and only a final still pays for it.
            ao_weight=0.6 if quality.get("ambientOcclusion") else 0.0,
            pom=PomSettings(bool(quality.get("pom", True)), "Occlusion", 0.3, 8, 32, 4, False, False),
            detail=DetailSettings(enabled=False),
            height_ao=HeightAoSettings(enabled=bool(quality.get("heightAo", False))),
            aov=AovSettings(enabled=False),
            sky=SkySettings(enabled=bool(quality.get("sky", True))),
        )
        settings.shadows.enabled = bool(quality.get("shadows", True))
        settings.shadows.technique = "PCF"

        params = f3d.TerrainRenderParams(settings)
        frame = self.renderer.render_terrain_pbr_pom(
            self.materials, self.ibl, params, self.heights
        )
        image = np.asarray(frame.to_numpy())
        rgb = image[:, :, :3].astype(np.uint8)
        png = encode_png(rgb)

        output = request.get("output")
        if output:
            Path(output).parent.mkdir(parents=True, exist_ok=True)
            Path(output).write_bytes(png)

        return {
            "width": width,
            "height": height,
            "png": base64.b64encode(png).decode("ascii"),
        }


def encode_png(rgb: np.ndarray) -> bytes:
    from PIL import Image

    buffer = io.BytesIO()
    Image.fromarray(rgb, mode="RGB").save(buffer, format="PNG")
    return buffer.getvalue()


def main() -> int:
    ensure_native_runtime()

    try:
        config = json.loads(sys.stdin.readline())
        dem = config["dem"]
    except Exception as exc:  # noqa: BLE001
        fail(f"bad renderer configuration: {exc}")
        return 2

    try:
        renderer = StudioRenderer(dem)
    except Exception as exc:  # noqa: BLE001
        emit({"event": "error", "message": f"the renderer did not start: {exc}", "trace": traceback.format_exc()})
        return 3

    emit({
        "event": "ready",
        "dem": dem,
        "span": [renderer.span_x, renderer.span_y],
        "range": list(renderer.range),
    })

    import time

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except Exception as exc:  # noqa: BLE001
            emit({"id": None, "ok": False, "error": f"bad request: {exc}"})
            continue

        request_id = request.get("id")
        if request.get("cmd") == "close":
            emit({"id": request_id, "ok": True})
            return 0

        try:
            started = time.perf_counter()
            result = renderer.render(request)
            result.update({"id": request_id, "ok": True, "ms": round((time.perf_counter() - started) * 1000.0, 1)})
            emit(result)
        except Exception as exc:  # noqa: BLE001
            emit({"id": request_id, "ok": False, "error": str(exc)})

    return 0


if __name__ == "__main__":
    sys.exit(main())
