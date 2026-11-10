"""Forge3D Studio's embedded renderer.

A persistent offscreen renderer driven by the Studio shell over stdin/stdout.
No viewer window: this is what "Rendered" mode in the Studio viewport uses, and
what a still render will use.

Protocol (newline-delimited JSON):

  stdin  line 1   {"dem": "/path/dem.tif", "zScale": 1.0}
  stdin  line n   {"id": 1, "width": 960, "height": 600,
                   "camera": {"phiDeg": .., "thetaDeg": .., "radius": ..,
                              "target": [x, y, z], "fovDeg": ..},
                   "sun": {"azimuthDeg": .., "elevationDeg": ..,
                           "intensity": .., "color": [r, g, b]},
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
import sys
import tempfile
import traceback
from pathlib import Path
from typing import Any, Dict, Optional

import numpy as np


def emit(payload: Dict[str, Any]) -> None:
    print(json.dumps(payload), flush=True)


def fail(message: str) -> None:
    emit({"event": "error", "message": message})


def srgb_to_linear(values: np.ndarray) -> np.ndarray:
    """Decode sRGB-encoded floats in [0, 1] to linear radiance."""
    v = np.clip(np.asarray(values, dtype=np.float32), 0.0, 1.0)
    return np.where(v <= 0.04045, v / 12.92, ((v + 0.055) / 1.055) ** 2.4).astype(np.float32)


def linear_to_srgb(values: np.ndarray) -> np.ndarray:
    """Encode linear radiance in [0, 1] to sRGB for an 8-bit frame."""
    v = np.clip(np.asarray(values, dtype=np.float32), 0.0, 1.0)
    return np.where(v <= 0.0031308, v * 12.92, 1.055 * (v ** (1.0 / 2.4)) - 0.055).astype(np.float32)


def write_sky_hdr(path: Path, width: int = 128, height: int = 64) -> None:
    """A neutral studio sky: the same gradient the viewport previews.

    Equirectangular, `-Y` (row 0 = zenith). The repository ships no default HDR
    environment and the renderer requires one, so the service writes its own:
    predictable, licence-free, and identical every run. The viewport builds the
    same gradient in JavaScript, so Solid, Material and Rendered share one sky.
    The RGBE encoding itself lives in the engine (`forge3d.io.write_hdr`).
    """
    from forge3d import io as fio

    v = np.linspace(0.0, 1.0, height)[:, None]  # 0 = zenith, 1 = nadir
    zenith = np.array([0.25, 0.40, 0.75])
    horizon = np.array([0.85, 0.72, 0.55])
    ground = np.array([0.10, 0.09, 0.08])
    # Upper hemisphere: horizon at the equator, zenith at row 0.
    t = np.clip((0.5 - v) / 0.5, 0.0, 1.0)[..., None]
    sky = horizon[None, None, :] * (1.0 - t) + zenith[None, None, :] * t
    image = np.where(v[..., None] < 0.5, sky, np.broadcast_to(ground, sky.shape))
    image = np.broadcast_to(image.astype(np.float32) * 1.6, (height, width, 3)).copy()
    fio.write_hdr(path, image)


def dem_span(dem) -> tuple[float, float]:
    """Metric footprint of a DEM: pixel size when projected, local UTM when not.

    A geographic raster's degrees are not metres — treating them as metres is
    what made a 353 km DEM report 0.00 km. The zone comes from the engine's own
    estimator, so the studio and the viewer cannot disagree about it.
    """
    from forge3d import gis

    height, width = np.asarray(dem.data).shape[:2]
    crs = str(dem.crs or "")
    if ("4326" in crs or "wgs84" in crs.lower()) and dem.bounds is not None:
        local = gis.estimate_local_utm(dem.bounds)["crs"]
        left, bottom, right, top = gis.transform_bounds(dem.crs, local, dem.bounds, densify=21)
        return abs(right - left), abs(top - bottom)
    resolution = dem.resolution or (1.0, 1.0)
    return abs(float(resolution[0])) * width, abs(float(resolution[1])) * height


def load_dem(path: str) -> tuple[np.ndarray, float, float, tuple[float, float]]:
    """Read a DEM through the engine's loader and compute the render frame.

    Returns heights (nodata filled, outliers clamped), the metric span, and the
    robust elevation range. Reading, nodata masking and CRS handling are the
    engine's (`forge3d.io`, `forge3d.gis`); only the presentation choices —
    clamp to [0.01, 99.99] percentiles, fill holes with the floor — are the
    studio's.
    """
    from forge3d import io as fio

    dem = fio.load_dem(path, fill_nodata_values=False)
    raw = np.asarray(dem.data, dtype="float64")
    nodata = dem.nodata_value
    if nodata is not None:
        raw = np.where(np.isclose(raw, float(nodata)), np.nan, raw)
    valid = np.isfinite(raw)
    if not valid.any():
        raise RuntimeError("the DEM contains no valid elevation samples")
    low, high = np.percentile(raw[valid], [0.01, 99.99])
    heights = np.where(valid, np.clip(np.nan_to_num(raw, nan=low), low, high), low)
    span_x, span_y = dem_span(dem)
    if span_x <= 0.0 or span_y <= 0.0:
        span_x, span_y = float(raw.shape[1]), float(raw.shape[0])
    return heights.astype(np.float32), span_x, span_y, (float(low), float(high))


class StudioRenderer:
    def __init__(self, dem_path: str) -> None:
        import forge3d as f3d

        self.f3d = f3d
        self.heights, self.span_x, self.span_y, self.range = load_dem(dem_path)
        self.dem_path = dem_path
        # The renderer rejects view coordinates beyond +/-65,536 units, so a
        # country-scale DEM is scaled into that budget (proportions preserved).
        # radius is 1.5 x span by default and the view limit is 65,536, so the
        # scaled world must stay well below that (30,000 leaves headroom).
        self.scale = min(1.0, 30_000.0 / max(self.span_x, self.span_y))
        self.heights = self.heights * self.scale

        env_path = Path(tempfile.gettempdir()) / "forge3d-studio-sky.hdr"
        write_sky_hdr(env_path)
        self.sky_path = env_path
        # The built-in sky is the fallback; a project HDRI replaces it. IBLs
        # are cached because `from_hdr` uploads and pre-filters the map, and
        # Rendered mode re-requests frames while the camera moves.
        self._ibl_cache: Dict[tuple, Any] = {}
        self._ibl_cache_limit = 4
        # The AETHER LUT is a shipped table: resolving it per frame costs a few
        # ms, and its parameters are fixed by the gate in `sky_settings`.
        self._aether_lut: Any = None

        self.session = f3d.Session(window=False, backend="vulkan")
        self.renderer = f3d.TerrainRenderer(self.session)
        self.materials = f3d.MaterialSet.terrain_default()
        self.size: Optional[tuple[int, int]] = None

    def environment_map(self, path: Optional[str]):
        """The IBL for one frame: the project's HDRI, or the built-in studio sky.

        Cached by file identity (path, mtime, size) so replacing the .hdr in
        place still takes effect, while orbiting reuses the prefiltered map.
        """
        if not path:
            key: tuple = ("studio-sky",)
            source = str(self.sky_path)
        else:
            target = Path(path)
            try:
                stat = target.stat()
            except OSError as exc:
                raise RuntimeError("HDRI environment not found: %s (%s)" % (path, exc))
            if not target.is_file():
                raise RuntimeError("HDRI environment is not a file: %s" % (path,))
            if stat.st_size == 0:
                raise RuntimeError("HDRI environment is empty: %s" % (path,))
            key = (str(target), stat.st_mtime_ns, stat.st_size)
            source = str(target)

        cached = self._ibl_cache.get(key)
        if cached is None:
            cached = self.f3d.IBL.from_hdr(source, intensity=1.0)
            self._ibl_cache[key] = cached
            # A user trying several HDRIs should not leak every prefiltered
            # map; keep a few and drop the oldest insertion.
            while len(self._ibl_cache) > self._ibl_cache_limit:
                self._ibl_cache.pop(next(iter(self._ibl_cache)))
        return cached

    def water(self, request: Dict[str, Any]):
        """The engine's water mask and settings for the project's water settings.

        The terrain path has no water-level knob: it shades water where a mask
        says so. The mask is derived from the DEM by the engine's own
        `forge3d.gis.derive_water_mask` — low ground below the level, and only
        where it is flat enough to hold water.

        The project's level is in the DEM's own metres and the heights here are
        scaled into the renderer's view budget, so both the level and the slope
        threshold are scaled with them: the control then means the same thing
        on a 4 km amphitheatre and on a 350 km country.
        """
        water = (request.get("quality") or {}).get("water") or {}
        if not bool(water.get("enabled", False)):
            return None, None
        try:
            from forge3d.gis import derive_water_mask
            from forge3d.terrain_params import WaterOpticsSettings, WaterSettings
        except Exception as exc:  # noqa: BLE001
            print(json.dumps({"event": "warning", "message": "water skipped: %s" % (exc,)}), flush=True)
            return None, None
        level = float(water.get("level", 0.0) or 0.0)
        slope = float(water.get("slopeThreshold", 0.02) or 0.0)
        # 0 means auto: the engine's own default, the lowest 15%.
        scaled_level = None if level == 0.0 else level * self.scale
        try:
            mask = derive_water_mask(
                self.heights,
                level=scaled_level,
                slope_threshold=slope * self.scale,
            )
        except Exception as exc:  # noqa: BLE001
            print(json.dumps({"event": "warning", "message": "water skipped: %s" % (exc,)}), flush=True)
            return None, None
        mask = np.asarray(mask, dtype=np.float32) > 0.5
        keep = _keep_water_blobs(mask, MIN_WATER_BLOB)
        # The water surface: a plane at the water line over the water cells and
        # the terrain everywhere else. Its normals are that grid's own gradient,
        # so the sun's highlight is a smooth band. Inheriting the DEM's normals
        # instead (the non-physical path) breaks the highlight into a field of
        # per-cell glints — measured: 302 sparkle pixels in a 1180x330 frame,
        # 2 with this surface.
        if scaled_level is None:
            finite = np.isfinite(self.heights)
            water_line = float(np.nanquantile(self.heights[finite], 0.15)) if finite.any() else 0.0
        else:
            water_line = float(scaled_level)
        surface_height = np.where(keep, np.float32(water_line), self.heights).astype(np.float32)
        gradient_y, gradient_x = np.gradient(surface_height)
        surface_normals = np.stack(
            (-gradient_x, -gradient_y, np.ones_like(gradient_x)), axis=-1
        ).astype(np.float32)
        surface_normals /= np.linalg.norm(surface_normals, axis=-1, keepdims=True)
        settings = WaterSettings(
            enabled=True,
            auto_mask=True,
            level=scaled_level,
            slope_threshold=slope * self.scale,
            # The physical water path gives the water its own smooth surface and a
            # roughness. Without it the water inherits the DEM's noisy normals
            # and the sun's specular breaks into a field of per-cell glints —
            # measured: 302 sparkle pixels in a 1180x330 frame, 2 with it.
            optics=WaterOpticsSettings(
                mode="physical",
                roughness=WATER_ROUGHNESS,
                surface_height=surface_height,
                surface_normals=surface_normals,
            ),
        )
        return np.ascontiguousarray(keep.astype(np.float32)), settings

    def build_overlay(self, overlay: Dict[str, Any], width: int, height: int):
        """Read a raster overlay decimated to the frame, aiming for the drape.

        Deliberately no reprojection: the overlay is stretched across the terrain
        footprint, which is what the gallery examples do. (Reprojecting into a
        0..N destination grid put a georeferenced raster at coordinates far from
        its own extent and produced a flat grey layer.)
        """
        import rasterio
        from rasterio.enums import Resampling as RasterioResampling

        with rasterio.open(overlay["path"]) as src:
            bands = min(3, src.count)
            planes = [
                src.read(
                    band + 1,
                    out_shape=(height, width),
                    resampling=RasterioResampling.average,
                    masked=True,
                )
                for band in range(bands)
            ]
            valid = np.ones((height, width), dtype=bool)
            for plane in planes:
                valid &= ~np.ma.getmaskarray(plane)
            rgb = np.zeros((height, width, 3), dtype=np.float32)
            for index, plane in enumerate(planes):
                values = np.asarray(plane.filled(0.0), dtype=np.float32)
                rgb[:, :, index] = np.clip(values, 0.0, None)
            if bands == 1:
                rgb[:, :, 1] = rgb[:, :, 0]
                rgb[:, :, 2] = rgb[:, :, 0]

            # The shell serializes OverlayRequest as camelCase ("preserveColors").
            if overlay.get("preserve_colors", overlay.get("preserveColors")):
                # Draped imagery (NAIP, land cover) keeps its source colours:
                # 8-bit data passes through, 16-bit data is bit-shifted, and
                # only nodata pixels become transparent.
                peak = max(
                    (float(np.ma.max(plane)) if plane.count() else 0.0) for plane in planes
                )
                if peak > 255.0:
                    rgb *= 255.0 / 65535.0
                rgba = np.concatenate(
                    [np.clip(rgb, 0.0, 255.0).astype(np.uint8), np.where(valid, 255, 0).astype(np.uint8)[..., None]],
                    axis=2,
                )
                return self.f3d.OverlayLayer.from_raster_rgba(
                    rgba, strength=float(overlay.get("opacity", 0.82))
                )

        flat = rgb.reshape(-1, 3)
        finite = np.isfinite(flat).all(axis=1)
        low, high = (
            np.percentile(flat[finite], [1, 99]) if finite.any() else (0.0, 255.0)
        )
        if high - low < 1e-6:
            low, high = 0.0, 255.0
        scaled = np.clip((rgb - low) / (high - low), 0.0, 1.0)
        alpha = np.where(valid, 255, 0).astype(np.uint8)
        rgba = np.concatenate(
            [(scaled * 255.0).astype(np.uint8), alpha[..., None]], axis=2
        )
        return self.f3d.OverlayLayer.from_raster_rgba(
            rgba, strength=float(overlay.get("opacity", 0.82))
        )

    #: Model token -> the engine's `SkySettings.model` spelling.
    SKY_MODELS = {"hosek": "hosek-wilkie", "preetham": "preetham", "aether": "aether"}

    def sky_settings(self, request: Dict[str, Any], quality: Dict[str, Any], environment):
        """The visible sky for one frame.

        An HDRI owns the sky (it is the same map lighting the terrain) and keeps
        the project's backdrop intensity. Without one, the project's analytic
        model draws the backdrop with its own exposure; that exposure moves the
        sky pass only — terrain radiance and IBL intensity are untouched.

        AETHER renders from a *baked* LUT. The wheel ships one for turbidity 2.0
        / ground albedo 0.3; any other pair needs a feature-gated bake, so the
        service refuses instead of silently rendering the wrong atmosphere.
        """
        from forge3d.terrain_params import SkySettings

        enabled = bool(quality.get("sky", True))
        # Nothing to draw: do not evaluate a model (AETHER would demand a LUT
        # for parameters that no longer matter).
        if not enabled:
            return SkySettings(enabled=False)
        if environment is not None:
            return SkySettings(
                enabled=enabled,
                model="hdri",
                aerial_perspective=False,
                sky_exposure=float(request.get("environmentBackgroundIntensity", 1.0)),
            )

        model = str(quality.get("skyModel") or "hosek").lower()
        turbidity = float(quality.get("skyTurbidity", 2.0))
        ground_albedo = float(quality.get("skyGroundAlbedo", 0.3))
        lut_handle = None
        if model == "aether":
            if abs(turbidity - 2.0) > 1e-6 or abs(ground_albedo - 0.3) > 1e-6:
                raise RuntimeError(
                    "the AETHER sky needs a baked LUT for turbidity %.2f / ground albedo %.2f; "
                    "the shipped bake is 2.0 / 0.3 — set those, or bake the atmosphere first"
                    % (turbidity, ground_albedo)
                )
            import forge3d as f3d

            if self._aether_lut is None:
                self._aether_lut = f3d.atmosphere_bake_luts()
            lut_handle = self._aether_lut

        return SkySettings(
            enabled=enabled,
            model=self.SKY_MODELS.get(model, "hosek-wilkie"),
            turbidity=turbidity,
            ground_albedo=ground_albedo,
            aerial_perspective=False,
            sky_exposure=float(quality.get("skyExposure", 0.08)),
            lut_handle=lut_handle,
        )

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

        # Work in the scaled world the heights were prepared for.
        span_x = self.span_x * self.scale
        span_y = self.span_y * self.scale

        camera = request.get("camera") or {}
        sun = request.get("sun") or {}
        quality = request.get("quality") or {}
        # Final-quality extras: AOV export and OIDN denoise of the beauty.
        # Viewport previews send neither, so the interactive path is unchanged.
        aov = quality.get("aov") or {}
        aov_flags = {channel: bool(aov.get(channel)) for channel in ("albedo", "normal", "depth")}
        want_aov = any(aov_flags.values())
        want_denoise = bool(quality.get("denoise"))
        overlay = request.get("overlay") or None
        environment = request.get("environment") or None
        overlays = []
        if overlay:
            try:
                overlays = [self.build_overlay(overlay, width, height)]
            except Exception as exc:  # noqa: BLE001
                print(json.dumps({"event": "warning", "message": "overlay skipped: %s" % (exc,)}), flush=True)

        span = span_x
        radius = float(camera.get("radius") or 1.5 * max(self.span_x, self.span_y)) * self.scale
        raw_target = camera.get("target") or [self.span_x / 2, self.span_y / 2, float(self.heights.mean())]
        # Targets arrive in metres; the whole scene (heights, span, radius) is
        # scaled into the renderer's view budget together.
        target = [
            float(raw_target[0]) * self.scale,
            float(raw_target[1]) * self.scale,
            float(raw_target[2]) * self.scale,
        ]

        # The shader's u_ibl.intensity comes from the IBL object itself
        # (prepare_ibl_bind_group reads env_maps.intensity), not from
        # TerrainRenderParams — set it here so the project's Environment
        # intensity actually reaches the frame. The map itself is the project's
        # HDRI when set, otherwise the built-in studio sky.
        ibl = self.environment_map(environment)
        ibl.intensity = float(request.get("iblIntensity", 1.0))

        sky_settings = self.sky_settings(request, quality, environment)
        water_mask, water_settings = self.water(request)

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
            clip=(
                # Near tracks the zoom: the fixed 10 m floor clipped close
                # cameras' near terrain away (Solid draws it), and a free-float
                # near would wreck depth precision against the 400 km far plane.
                min(10.0, max(radius * 0.001, 0.01)),
                400000.0,
            ),
            domain=self.range,
            overlays=overlays,
            albedo_mode="material",
            # One lighting model across the app. The viewport's Solid/Material
            # preview is three.js `MeshStandardMaterial` — radiance =
            # albedo/pi * sun_color*intensity * n.l plus one IBL irradiance
            # term. The engine's "stylized" terrain model answers the same sun
            # with a hardcoded, style-matched response (sun_peak 0.36, ambient
            # floors), so identical sun/exposure lit the still ~4x darker than
            # the preview. `lambert_physical` evaluates the same physical
            # equation the preview does, so a frame matches what the user saw
            # while authoring it.
            terrain_shading_model="lambert_physical",
            hue_variation_strength=0.0,
            colormap_strength=1.0,
            light_azimuth_deg=float(sun.get("azimuthDeg", 135.0)),
            light_elevation_deg=float(sun.get("elevationDeg", 35.0)),
            sun_intensity=float(sun.get("intensity", 3.0)),
            # The starter sun's colour; the shell sends the scene's light
            # colour so Rendered matches the Solid view's lighting hue.
            sun_color=tuple(float(c) for c in sun.get("color", (1.0, 0.97, 0.92))),
            ibl_enabled=True,
            ibl_intensity=float(request.get("iblIntensity", 1.0)),
            exposure=float(request.get("exposure", 1.35)),
            # The SSAO pass costs ~4.8 s per frame by itself (measured); every
            # other feature combined is ~150 ms. Viewport frames therefore skip
            # it, and only a final still pays for it.
            ao_weight=0.6 if quality.get("ambientOcclusion") else 0.0,
            pom=PomSettings(bool(quality.get("pom", True)), "Occlusion", 0.3, 8, 32, 4, False, False),
            detail=DetailSettings(enabled=False),
            height_ao=HeightAoSettings(enabled=bool(quality.get("heightAo", False))),
            aov=AovSettings(
                enabled=want_aov or want_denoise,
                # Denoise gets albedo/normal guidance from the same frame —
                # capturing them costs the AOV pass we are already in.
                albedo=aov_flags["albedo"] or want_denoise,
                normal=aov_flags["normal"] or want_denoise,
                depth=aov_flags["depth"],
                format="png",
            ),
            sky=sky_settings,
            water=water_settings,
        )
        settings.shadows.enabled = bool(quality.get("shadows", True))
        # Shadow filter and map size are the project's, not the service's: PCF
        # is the cheap inset filter, PCSS trades a blocker search for
        # contact-hardening, and the map size is the quality/cost dial.
        settings.shadows.technique = str(quality.get("shadowTechnique") or "PCF").upper()
        settings.shadows.resolution = int(quality.get("shadowResolution") or 4096)
        # The camera orbits far outside the 4000 m default max_distance
        # (6+ km away), so every fragment beyond it faded to unshadowed —
        # the entire terrain rendered without a single shadow.
        settings.shadows.max_distance = 30000.0
        # The bias defaults (0.0005/0.001/0.0002 in NORMALIZED depth) were
        # sized for small scenes; over this engine's ~6 km light box they eat
        # ~10 m of world depth and swallow every shadow. Keep them near zero so
        # cast shadows survive, with just enough constant/normal bias to keep
        # heightmap interpolation off acne.
        settings.shadows.depth_bias = 0.00005
        settings.shadows.slope_scale_bias = 0.0001
        settings.shadows.normal_bias = 0.00001

        params = f3d.TerrainRenderParams(settings)
        if want_aov or want_denoise:
            frame, aov_frame = self.renderer.render_with_aov(
                self.materials, ibl, params, self.heights, water_mask=water_mask
            )
        else:
            frame = self.renderer.render_terrain_pbr_pom(
                self.materials, ibl, params, self.heights, water_mask=water_mask
            )
            aov_frame = None
        image = np.asarray(frame.to_numpy())
        rgb = image[:, :, :3].astype(np.uint8)

        if want_denoise:
            albedo = aov_frame.albedo() if aov_frame is not None and aov_frame.has_albedo else None
            normal = aov_frame.normal() if aov_frame is not None and aov_frame.has_normal else None
            try:
                from forge3d.denoise_oidn import oidn_denoise

                # OIDN operates on linear radiance and its albedo/normal
                # guidance is linear too, but the engine's frame is
                # display-referred sRGB (ACES + exposure already applied).
                # Denoising the sRGB-encoded 8-bit values in place fed the
                # filter the wrong space against linear guidance; decode to
                # linear first and re-encode the result.
                beauty = srgb_to_linear(rgb.astype(np.float32) / 255.0)
                denoised = oidn_denoise(beauty, albedo, normal, hdr=False, quality="high")
                rgb = np.clip(np.rint(linear_to_srgb(denoised) * 255.0), 0, 255).astype(np.uint8)
            except Exception as exc:
                # The frame must not come back silently un-denoised; fail it.
                raise RuntimeError(
                    "denoise requested but the OIDN runtime is not available or failed: %s "
                    "(install it with `pip install oidn` / `pip install pyoidn` next to the engine)" % (exc,)
                ) from exc

        rgb = apply_post_effects(rgb, quality)

        png = encode_png(rgb)

        output = request.get("output")
        if output:
            Path(output).parent.mkdir(parents=True, exist_ok=True)
            if want_aov and aov_frame is not None:
                # Engine naming, next to the still / the frame. Written before
                # the beauty: a failed AOV save must not leave a still on disk
                # that looks complete while the command reports failure.
                stem = Path(output).with_suffix("")
                if aov_flags["albedo"]:
                    aov_frame.save_albedo(str(stem) + "_albedo.png")
                if aov_flags["normal"]:
                    aov_frame.save_normal(str(stem) + "_normal.png")
                if aov_flags["depth"]:
                    aov_frame.save_depth(str(stem) + "_depth.png")
            Path(output).write_bytes(png)

        return {
            "width": width,
            "height": height,
            "png": base64.b64encode(png).decode("ascii"),
        }


def _box_blur_axis(image: np.ndarray, radius: int, axis: int) -> np.ndarray:
    """One separable box-blur pass along `axis`."""
    n = image.shape[axis]
    if radius < 1 or n == 0:
        return image
    pad = [(0, 0)] * image.ndim
    pad[axis] = (radius + 1, radius)
    padded = np.pad(image, pad, mode="edge")
    zero = np.zeros_like(np.take(padded, [0], axis=axis))
    cumulative = np.concatenate([zero, np.cumsum(padded, axis=axis)], axis=axis)
    high = np.take(cumulative, np.arange(2 * radius + 1, n + 2 * radius + 1), axis=axis)
    low = np.take(cumulative, np.arange(0, n), axis=axis)
    return (high - low) / float(2 * radius + 1)


def _box_blur(image: np.ndarray, radius: int) -> np.ndarray:
    """A separable box blur. Three passes approximate a Gaussian closely
    enough for a glow, and stay linear in the pixel count."""
    out = image
    for _ in range(3):
        out = _box_blur_axis(out, radius, 0)
        out = _box_blur_axis(out, radius, 1)
    return out


def apply_bloom(
    image: np.ndarray,
    threshold: float,
    softness: float,
    intensity: float,
    radius: int,
) -> np.ndarray:
    """Glow the bright pixels of a display-referred frame.

    The engine's GPU bloom works on HDR radiance (its own default threshold is
    1.5, "HDR only"). The frame here is display-referred — the terrain shader
    already tone-mapped it — so the threshold is a *display* brightness: 0.8
    glows the highlights of an already-exposed image.
    """
    luma = image @ np.array([0.2126, 0.7152, 0.0722], dtype=np.float32)
    if softness > 0.0:
        weight = np.clip((luma - (threshold - softness)) / (2.0 * softness), 0.0, 1.0)
    else:
        weight = (luma > threshold).astype(np.float32)
    glow = _box_blur(image * weight[:, :, None], radius)
    return np.clip(image + glow * float(intensity), 0.0, 1.0)


def apply_vignette(image: np.ndarray, strength: float) -> np.ndarray:
    """Corner darkening: 0 leaves the frame alone, 1 blacks the corners."""
    if strength <= 0.0:
        return image
    height, width = image.shape[:2]
    y = (np.arange(height, dtype=np.float32) + 0.5) / height * 2.0 - 1.0
    x = (np.arange(width, dtype=np.float32) + 0.5) / width * 2.0 - 1.0
    radius = np.sqrt(x[None, :] ** 2 + y[:, None] ** 2) / np.sqrt(2.0)
    falloff = np.clip((radius - 0.5) / 0.5, 0.0, 1.0)
    falloff = falloff * falloff * (3.0 - 2.0 * falloff)
    return image * (1.0 - float(strength) * falloff)[:, :, None]


def apply_post_effects(rgb: np.ndarray, quality: Dict[str, Any]) -> np.ndarray:
    """Bloom, then vignette, on the finished frame.

    Both are display-referred: the terrain shader has already tone-mapped the
    frame, so the engine's HDR bloom threshold has no meaning here. The
    engine's own GPU bloom and vignette live in render paths this one does not
    use — measured, they changed nothing there.
    """
    bloom = quality.get("bloom") or {}
    vignette = float(quality.get("vignette", 0.0) or 0.0)
    if not bool(bloom.get("enabled", False)) and vignette <= 0.0:
        # Nothing to do: hand the frame straight back, byte for byte.
        return rgb
    image = rgb.astype(np.float32) / 255.0
    if bool(bloom.get("enabled", False)):
        height, width = image.shape[:2]
        # The glow scales with the frame, so a 4K still glows like a preview.
        radius = max(2, int(round(min(height, width) / 120.0)))
        image = apply_bloom(
            image,
            float(bloom.get("threshold", 0.8)),
            float(bloom.get("softness", 0.2)),
            float(bloom.get("intensity", 0.3)),
            radius,
        )
    if vignette > 0.0:
        image = apply_vignette(image, vignette)
    return np.clip(np.rint(image * 255.0), 0, 255).astype(np.uint8)


#: The water surface's roughness. Higher is a softer, wider highlight.
WATER_ROUGHNESS = 0.35

#: A water blob smaller than this many DEM cells is a speckle, not a body.
#: The mask marks every low, flat cell, and on a rugged DEM that leaves
#: hundreds of *single* cells over the slopes; the water shader gives each one
#: a specular highlight, so the frame reads as a field of sparkles.
MIN_WATER_BLOB = 8


def _keep_water_blobs(mask: np.ndarray, minimum: int) -> np.ndarray:
    """Keep only the water blobs big enough to be a body.

    Blobs are four-connected: a diagonal chain of single cells is a speckle
    field, not a lake, so it must not be allowed to add up to a body.

    Measured on the Bryce DEM: 2,220 cells in 820 blobs (471 of them a single
    cell) becomes a handful of real pools. On the engine's own lake-basin demo
    DEM the single 465-cell lake survives untouched.
    """
    ys, xs = np.nonzero(mask)
    if ys.size == 0:
        return mask
    cells = set(zip(ys.tolist(), xs.tolist()))
    keep = np.zeros_like(mask, dtype=bool)
    while cells:
        seed = cells.pop()
        stack = [seed]
        blob = [seed]
        while stack:
            y, x = stack.pop()
            for dy, dx in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                neighbour = (y + dy, x + dx)
                if neighbour in cells:
                    cells.discard(neighbour)
                    stack.append(neighbour)
                    blob.append(neighbour)
        if len(blob) >= minimum:
            for y, x in blob:
                keep[y, x] = True
    return keep


def encode_png(rgb: np.ndarray) -> bytes:
    from PIL import Image

    buffer = io.BytesIO()
    Image.fromarray(rgb, mode="RGB").save(buffer, format="PNG")
    return buffer.getvalue()


def main() -> int:
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
