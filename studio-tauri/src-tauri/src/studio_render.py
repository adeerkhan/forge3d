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
import math
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


def engine_height_domain(heights) -> tuple[float, float]:
    """The height domain for the heights the renderer is given.

    The renderer clamps and normalizes the heightmap against the domain
    (`get_height_geom_t`), so the domain has to describe the heights *as they
    are uploaded*. A country-scale DEM is scaled into the view budget, and
    handing the renderer the DEM's original range collapsed every height to the
    floor: Switzerland rendered as a flat plate. The range in the DEM's own
    metres stays available for the levels and the legend.
    """
    values = np.asarray(heights, dtype=np.float64)
    finite = values[np.isfinite(values)]
    if finite.size == 0:
        return (0.0, 1.0)
    low = float(finite.min())
    high = float(finite.max())
    if high <= low:
        high = low + 1.0
    return (low, high)


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


def load_dem(path: str):
    """Read a DEM through the engine's loader and compute the render frame.

    Returns the heights (nodata filled, outliers clamped), the metric span, the
    robust elevation range, the raster's bounds and its CRS. Reading, nodata
    masking and CRS handling are the engine's (`forge3d.io`, `forge3d.gis`); only
    the presentation choices — clamp to [0.01, 99.99] percentiles, fill holes
    with the floor — are the studio's. The bounds and the CRS are what place a
    vector layer on the terrain.
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
    bounds = None
    if getattr(dem, "bounds", None) is not None:
        bounds = tuple(float(value) for value in dem.bounds)
    crs = str(getattr(dem, "crs", "") or "")
    return (
        heights.astype(np.float32),
        span_x,
        span_y,
        (float(low), float(high)),
        bounds,
        crs,
    )


class StudioRenderer:
    def __init__(self, dem_path: str) -> None:
        import forge3d as f3d

        self.f3d = f3d
        (
            self.heights,
            self.span_x,
            self.span_y,
            self.range,
            self.dem_bounds,
            self.dem_crs,
        ) = load_dem(dem_path)
        self.dem_path = dem_path
        # The renderer rejects view coordinates beyond +/-65,536 units, so a
        # country-scale DEM is scaled into that budget (proportions preserved).
        # radius is 1.5 x span by default and the view limit is 65,536, so the
        # scaled world must stay well below that (30,000 leaves headroom).
        self.scale = min(1.0, 30_000.0 / max(self.span_x, self.span_y))
        self.heights = self.heights * self.scale
        # The engine's height domain has to describe the heights it is given:
        # the renderer clamps and normalizes them against the domain, so a
        # country-scale DEM (whose heights are scaled into the view budget)
        # rendered with the *unscaled* range collapsed to the floor and came
        # out as a flat plate. The raw range stays for the levels and the
        # legend, which are in the DEM's metres.
        self.scaled_range = engine_height_domain(self.heights)

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
        # A land-cover raster is read whole (overviews or not, a 10 m national
        # raster costs ~9 s to decode), so the RGBA is cached by file identity.
        # Only the strength (the layer's opacity) is applied per request.
        self._overlay_rgba_cache: Dict[tuple, np.ndarray] = {}
        self._overlay_rgba_limit = 4

        self.session = f3d.Session(window=False, backend="vulkan")
        self.renderer = f3d.TerrainRenderer(self.session)
        self.materials = f3d.MaterialSet.terrain_default()
        self.size: Optional[tuple[int, int]] = None

    def quality_frame(self, params, ibl, water_mask, quality, output=None):
        """The frame through the engine's quality surface.

        The CONCORDIA path carries the screen-space effects — the cloud
        shadows among them — and the GPU cartography. It is equal-resolution,
        so the temporal anti-aliasing does the work the plain path's 2x
        supersampling does; a still averages several frames so it keeps the
        same smoothness (measured: 4 accumulated frames 2.496 edge energy
        against the plain 2x still's 2.582).

        The path is the engine's, not the studio's: the render scale is forced
        to 1 because the quality path refuses to supersample (the engine
        reports `taau_available: false`).
        """
        from forge3d.terrain_params import ScreenSpaceSettings

        # A viewport frame is one sample: the temporal history converges as
        # the camera moves. A still averages several, so it carries the same
        # smoothness the plain path's supersampling gave it.
        samples = 1 if not output else 4
        clouds = cloud_settings(quality.get("clouds"))
        frames = []
        for _ in range(samples):
            frame = self.renderer.render_interactive(
                self.materials,
                ibl,
                params,
                self.heights,
                water_mask=water_mask,
                clouds=clouds,
            )
            frames.append(np.asarray(frame.to_numpy())[:, :, :3].astype(np.float32))
        if len(frames) == 1:
            return frames[0].astype(np.uint8)
        return np.clip(np.rint(np.mean(np.stack(frames), axis=0)), 0, 255).astype(np.uint8)

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

    def colormap_overlay(self, layer: Optional[Dict[str, Any]]):
        """The engine's elevation colormap as a terrain overlay.

        `forge3d.get_colormap` returns a `Colormap`; the terrain takes a
        `Colormap1D` built from stops over the elevation domain, wrapped in an
        `OverlayLayer` — the engine's own path, not a second implementation.
        Returns None when the layer has no colormap (or its strength is 0).
        """
        if not layer:
            return None
        name = str(layer.get("colormap") or "").strip()
        if not name:
            return None
        strength = float(layer.get("strength", 1.0))
        if strength <= 0.0:
            # 0 means off: no overlay. The engine normalizes the height its
            # material layers sample by the colormap's domain, so a bound
            # colormap at strength 0 still shifted the layer blend by 162,905
            # samples (mean 4.4/255) against the same scene with no overlay.
            return None
        import forge3d as f3d

        token = name if ":" in name else f"forge3d:{name}"
        try:
            cmap = f3d.get_colormap(token)
        except KeyError as exc:
            raise RuntimeError(
                "unknown colormap %r; the engine offers %s"
                % (name, ", ".join(f3d.available_colormaps()))
            ) from exc
        rgba = np.asarray(cmap.rgba, dtype=np.float32)
        stops_count = min(64, len(rgba))
        indices = np.linspace(0, len(rgba) - 1, stops_count, dtype=int)
        low, high = self.range
        # The renderer works in the scaled heights the service built.
        domain = (float(low) * self.scale, float(high) * self.scale)
        span = max(domain[1] - domain[0], 1e-6)
        stops = [
            (
                domain[0] + span * index / max(stops_count - 1, 1),
                "#%02x%02x%02x"
                % tuple(int(round(float(rgba[row][channel]) * 255.0)) for channel in range(3)),
            )
            for index, row in enumerate(indices)
        ]
        colormap = f3d.Colormap1D.from_stops(stops=stops, domain=domain)
        return f3d.OverlayLayer.from_colormap1d(
            colormap,
            strength=strength,
            offset=0.0,
            blend_mode="Alpha",
            domain=domain,
        )

    def sun_visibility_settings(self, quality: Dict[str, Any]):
        """The engine's heightfield sun-visibility pass, or None when off.

        A ray march toward the sun per pixel: a ridge shadows the ground
        behind it, which the shadow map cannot resolve because it samples a
        different projection. Hard shadows with one ray — the engine's soft
        mode jittered 8 samples and moved 490 of 256,000 channel samples on
        the frame measured here, so penumbra and jitter are not worth a dial.
        """
        block = quality.get("sunVisibility") or {}
        if not bool(block.get("enabled", False)):
            return None
        from forge3d.terrain_params import SunVisibilitySettings

        return SunVisibilitySettings(
            enabled=True,
            mode="hard",
            samples=1,
            steps=int(block.get("steps", 32)),
            max_distance=float(block.get("distance", 2000.0)),
            softness=0.0,
            # Grain-scale self-shadowing; the engine default is 0.01.
            bias=0.005,
            # Full resolution: the engine's 0.5 default upsamples the pass and
            # moved 231 samples against 1.0 on the test frame.
            resolution_scale=1.0,
        )

    def fog_settings(self, quality: Dict[str, Any]):
        """The engine's height-based fog, or None when the project has none.

        The engine's `height_falloff` is per metre, so on a 450 m mountain a
        per-metre falloff leaves no haze above the valley floor. The Studio
        asks for a *reach*: the fraction of the terrain's height the fog
        climbs, and converts it here.
        """
        fog = quality.get("fog") or {}
        if not bool(fog.get("enabled", False)):
            return None
        from forge3d.terrain_params import FogSettings

        low, high = self.range
        span = max(1.0, (high - low) * self.scale)
        reach = float(fog.get("reach", 0.5) or 0.5)
        # Height-based exponential fog, applied after the PBR pass and before
        # the tonemap. The base height is left to the engine: it computes it
        # from the terrain bounds.
        inscatter = fog.get("inscatter") or (1.0, 1.0, 1.0)
        return FogSettings(
            density=float(fog.get("density", 0.05)),
            height_falloff=1.0 / max(reach * span, 1e-6),
            # The colour the haze scatters into the view. White is neutral; it
            # is what makes the fog read as air rather than as a white veil.
            inscatter=tuple(float(c) for c in inscatter),
        )

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
        roughness = float(water.get("roughness", WATER_ROUGHNESS) or WATER_ROUGHNESS)
        # 0 means auto: the engine's own default, the lowest 15%. A level
        # *below* the terrain means the same thing — there is no water down
        # there, and a silent empty frame reads as a broken control. Fall back
        # to auto rather than render nothing.
        below = self.scaled_range[0]
        scaled_level = None if level <= below else level * self.scale
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
                roughness=roughness,
                surface_height=surface_height,
                surface_normals=surface_normals,
            ),
        )
        return np.ascontiguousarray(keep.astype(np.float32)), settings

    #: Long side of the overlay texture, in texels. The engine samples it at
    #: the terrain's UV, so this — not the frame size — sets how much land-cover
    #: detail a close-up keeps. Decimating to the frame turned a 10 m land-cover
    #: map into 390 m blocks on a 900 px frame; 4096 is the engine's own overlay
    #: ceiling and keeps the class pattern crisp at a full-country view.
    OVERLAY_TEXTURE_LONG_SIDE = 4096

    def overlay_texture_size(self, height: int, width: int):
        """The texture size a source raster is read at: its own, capped by the
        long-side ceiling. Aspect is preserved."""
        scale = min(1.0, float(self.OVERLAY_TEXTURE_LONG_SIDE) / max(height, width, 1))
        return max(1, int(round(height * scale))), max(1, int(round(width * scale)))

    def overlay_classes(self, overlay: Dict[str, Any]):
        """The class IDs a cached class raster holds (empty for an RGB one)."""
        entry = self._overlay_rgba_cache.get(self.overlay_key(overlay))
        return list(entry[1]) if entry else []

    def overlay_key(self, overlay: Dict[str, Any]):
        """What the cached RGBA depends on: the file's identity and the class
        post-processing. The opacity is *not* in the key — it is applied when
        the overlay layer is built, so moving the slider reuses the pixels."""
        path = str(overlay["path"])
        try:
            stat = Path(path).stat()
            identity = (stat.st_mtime_ns, stat.st_size)
        except OSError:
            identity = (0, 0)
        return (path, *identity, bool(overlay.get("despeckle", False)))

    def build_overlay(self, overlay: Dict[str, Any]):
        """The engine's drape layer for a raster overlay.

        The texture's resolution follows the *source*, not the frame: the
        engine samples it per pixel, so a decimated-to-frame atlas loses the
        land cover's own detail the moment the camera moves in. The decoded
        RGBA is cached by file identity (a national 10 m raster costs ~9 s to
        read), and only the layer — and its opacity — is rebuilt per request.
        Deliberately no reprojection: the overlay is stretched across the
        terrain footprint, which is what the gallery examples do.
        (Reprojecting into a 0..N destination grid put a georeferenced raster at
        coordinates far from its own extent and produced a flat grey layer.)
        """
        key = self.overlay_key(overlay)
        entry = self._overlay_rgba_cache.get(key)
        if entry is None:
            entry = self.read_overlay_rgba(overlay)
            self._overlay_rgba_cache[key] = entry
            while len(self._overlay_rgba_cache) > self._overlay_rgba_limit:
                self._overlay_rgba_cache.pop(next(iter(self._overlay_rgba_cache)))
        rgba = entry[0]
        return self.f3d.OverlayLayer.from_raster_rgba(
            np.ascontiguousarray(rgba),
            strength=float(overlay.get("opacity", 0.82)),
        )

    def read_overlay_rgba(self, overlay: Dict[str, Any]):
        """Decode a raster into the RGBA the drape texture uploads.

        Returns the pixels and the class IDs the raster holds (empty for an RGB
        raster): the plate's categorical legend needs to know which classes
        the map actually shows.
        """
        import rasterio
        from rasterio.enums import Resampling as RasterioResampling

        # A national 10 m raster is ~2.6 GB of float32 across its bands; GDAL
        # reads the strips in parallel, which takes the three-band masked read
        # from 17.2 s to 3.5 s (measured on the Swiss land cover, 10055x22069).
        with rasterio.Env(GDAL_NUM_THREADS="ALL_CPUS"), rasterio.open(overlay["path"]) as src:
            bands = min(3, src.count)
            height, width = self.overlay_texture_size(src.height, src.width)
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

            # A single-band raster with whole-number values is a classification
            # (a land-cover map, say), not a picture: reading band 1 as the red
            # channel paints it grey. Map the classes through the land-cover
            # palette instead, with the engine's own thematic mapper.
            if bands == 1 and valid.any():
                values = np.asarray(planes[0].filled(0.0), dtype=np.float32)
                classes = np.rint(values).astype(np.int32)
                whole = np.abs(values[valid] - classes[valid]) < 1e-3
                distinct = np.unique(classes[valid & (classes > 0)])
                if whole.all() and 0 < distinct.size <= len(CLASSIFICATION_PALETTE):
                    from forge3d.thematic import apply_palette

                    if bool(overlay.get("despeckle", False)):
                        classes = despeckle_classes(classes)
                    rgba = apply_palette(classes, CLASSIFICATION_PALETTE)
                    rgba[~valid] = (0, 0, 0, 0)
                    present = [
                        int(value)
                        for value in np.unique(classes[valid])
                        if int(value) >= 1
                    ]
                    return np.ascontiguousarray(rgba), present

            rgb = np.zeros((height, width, 3), dtype=np.float32)
            for index, plane in enumerate(planes):
                values = np.asarray(plane.filled(0.0), dtype=np.float32)
                rgb[:, :, index] = np.clip(values, 0.0, None)
            if bands == 1:
                rgb[:, :, 1] = rgb[:, :, 0]
                rgb[:, :, 2] = rgb[:, :, 0]

            # A class map exported as an image: snap it back to the palette,
            # so the drape wears the palette's exact colours and the plate can
            # name the classes. An orthophoto is not a class map and is left
            # alone (the share test decides).
            snapped, class_ids = snap_to_classes(rgb)
            if snapped is not None:
                from forge3d.thematic import apply_palette

                rgba = apply_palette(snapped, CLASSIFICATION_PALETTE)
                rgba[~valid] = (0, 0, 0, 0)
                return np.ascontiguousarray(rgba), class_ids

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
                return (
                    np.concatenate(
                        [np.clip(rgb, 0.0, 255.0).astype(np.uint8), np.where(valid, 255, 0).astype(np.uint8)[..., None]],
                        axis=2,
                    ),
                    [],
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
        return (
            np.concatenate(
                [(scaled * 255.0).astype(np.uint8), alpha[..., None]], axis=2
            ),
            [],
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
        # The plate's cut-out is the renderer's own depth: background pixels
        # carry zero depth, so a dark valley cannot be mistaken for the page.
        # A plate is a layer now, so the depth pass follows the stack.
        layers = request.get("cartography") or []
        has_plate = any(
            str(layer.get("type", "")) == "plate" and bool(layer.get("visible", True))
            for layer in layers
        )
        if has_plate:
            aov_flags["depth"] = True
        want_aov = any(aov_flags.values())
        want_denoise = bool(quality.get("denoise"))
        environment = request.get("environment") or None

        # The map's layer stack, back to front. The engine binds exactly one
        # terrain overlay per frame (the first overlay that has a LUT, then
        # stops), so the elevation ramp and the draped imagery compete for
        # that slot: the uppermost visible one wins and the other is
        # reported, never dropped silently.
        visible = [layer for layer in layers if bool(layer.get("visible", True))]
        imagery_layer = None
        ramp_layer = None
        for layer in visible:
            kind = str(layer.get("type", ""))
            if kind == "imagery":
                imagery_layer = layer
            elif kind == "elevationRamp":
                ramp_layer = layer
        if imagery_layer is not None and ramp_layer is not None:
            print(
                json.dumps(
                    {
                        "event": "warning",
                        "message": "the elevation ramp is hidden under the imagery: "
                        "the engine composites one terrain overlay per frame",
                    }
                ),
                flush=True,
            )
            ramp_layer = None
        overlays = []
        if imagery_layer is not None:
            try:
                overlays = [self.build_overlay(imagery_layer)]
            except Exception as exc:  # noqa: BLE001
                print(json.dumps({"event": "warning", "message": "overlay skipped: %s" % (exc,)}), flush=True)
        else:
            ramp_overlay = self.colormap_overlay(ramp_layer)
            if ramp_overlay is not None:
                overlays.append(ramp_overlay)

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
        # The elevation ramp tints the terrain; the imagery drapes over it.
        ramp_overlay = self.colormap_overlay(ramp_layer)
        sun_visibility = self.sun_visibility_settings(quality)
        water_mask, water_settings = self.water(request)
        fog_settings = self.fog_settings(quality)

        # The engine's quality surface (CONCORDIA) carries the screen-space
        # effects and the GPU cartography; it is equal-resolution, so the
        # render scale is 1 there (the engine refuses to supersample on that
        # path and reports `taau_available: false`).
        output_path = request.get("output")
        quality_path = "plain" if (want_aov or want_denoise) else "concordia"
        screen_space = None
        if quality_path == "concordia":
            from forge3d.terrain_params import ScreenSpaceSettings

            screen_space = ScreenSpaceSettings(enabled=True, taa_enabled=True)

        settings = make_terrain_params_config(
            size_px=(width, height),
            render_scale=(
                1.0
                if quality_path == "concordia"
                else float((request.get("quality") or {}).get("renderScale", 1.0))
            ),
            screen_space=screen_space,
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
            domain=self.scaled_range,
            overlays=overlays,
            # The colormap only reaches the pixels in `mix` mode: the shader's
            # `material` branch assigns `material_albedo` last, so a colormap
            # overlay measured zero changed samples under it. `mix` blends
            # material -> colormap by `colormap_strength`, so 0 is the
            # material set and 1 is the ramp. With no colormap selected the
            # mode stays `material` (a `mix` mode with no overlay applies the
            # engine's fallback LUT to the whole terrain).
            albedo_mode="mix" if ramp_overlay is not None else "material",
            colormap_strength=float(ramp_layer.get("strength", 1.0))
            if ramp_overlay is not None
            else 0.0,
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
            light_azimuth_deg=float(sun.get("azimuthDeg", 135.0)),
            light_elevation_deg=float(sun.get("elevationDeg", 35.0)),
            sun_intensity=float(sun.get("intensity", 3.0)),
            # The starter sun's colour; the shell sends the scene's light
            # colour so Rendered matches the Solid view's lighting hue.
            sun_color=tuple(float(c) for c in sun.get("color", (1.0, 0.97, 0.92))),
            ibl_enabled=True,
            ibl_intensity=float(request.get("iblIntensity", 1.0)),
            exposure=float(request.get("exposure", 1.35)),
            detail=DetailSettings(enabled=False),
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
            fog=fog_settings,
            sun_visibility=sun_visibility,
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
            # The engine's AOV renderer is the plain path: it has no quality
            # surface, so the AOV/denoise finals cannot carry the screen-space
            # effects. The path is reported in the reply.
            frame, aov_frame = self.renderer.render_with_aov(
                self.materials, ibl, params, self.heights, water_mask=water_mask
            )
            image = np.asarray(frame.to_numpy())
            render_path = "plain"
        else:
            image = self.quality_frame(params, ibl, water_mask, quality, output_path)
            aov_frame = None
            render_path = "concordia"
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

        # The map's vector layers, in draw order: each one's lines are
        # draped on the terrain and rasterized by the engine.
        vector_layers = [
            layer for layer in visible if str(layer.get("type", "")) == "vector"
        ]
        label_layers = [
            layer for layer in visible if str(layer.get("type", "")) == "labels"
        ]
        dem_code = crs_code(self.dem_crs)
        for vector in vector_layers:
            lines, _points, vector_crs = read_vector_features(str(vector["path"]), "")
            file_code = crs_code(vector_crs)
            if dem_code and file_code and dem_code != file_code:
                raise RuntimeError(
                    "the vector file is in EPSG:%s and the DEM in EPSG:%s; "
                    "reproject the file onto the DEM's CRS first" % (file_code, dem_code)
                )
            # The width is a cartographic symbol width in pixels: a real rail
            # is sub-pixel at a country-scale framing (62 m per pixel on a
            # 65 km DEM at 900 px), so a physical width could not change the
            # picture at all. The rasterizer's own floor is one pixel: 0.5 and
            # 1.0 px render identically (0 changed samples, measured), so the
            # layer's range starts at 1 rather than offering an inert half.
            stroke_px = max(1.0, float(vector.get("widthPx", 2.0) or 2.0))
            vector_overlay = vector_overlay_rgba(
                params,
                self.heights,
                self.dem_bounds,
                lines,
                width,
                height,
                colour=tuple(
                    float(c) for c in vector.get("colour", (1.0, 0.29, 0.1, 1.0))
                ),
                stroke_px=stroke_px,
            )
            if vector_overlay is not None:
                rgb = composite_overlay(rgb, vector_overlay)

        # The label layers: the engine's own label plan places them and its
        # text compositor draws them (the packaged MSDF atlas, the halo, the
        # rasterizer).
        for label_layer in label_layers:
            field = str(label_layer.get("field", "") or "")
            _lines, points, label_crs = read_vector_features(
                str(label_layer["path"]), field
            )
            file_code = crs_code(label_crs)
            if dem_code and file_code and dem_code != file_code:
                raise RuntimeError(
                    "the label file is in EPSG:%s and the DEM in EPSG:%s; "
                    "reproject the file onto the DEM's CRS first" % (file_code, dem_code)
                )
            if not points:
                continue
            label_colour = tuple(
                float(c) for c in label_layer.get("colour", (0.08, 0.08, 0.1, 1.0))
            )
            # A halo in the opposite tone keeps the text readable on any
            # ground: light text gets a dark halo and the other way round.
            dark_text = sum(label_colour[:3]) / 3.0 < 0.5
            halo = (0.0, 0.0, 0.0, 0.8) if dark_text else (1.0, 1.0, 1.0, 0.8)
            rgb, label_summary = compose_vector_labels(
                rgb,
                params,
                self.heights,
                self.dem_bounds,
                points,
                width,
                height,
                colour=label_colour,
                size_px=float(label_layer.get("sizePx", 14.0) or 14.0),
                halo_colour=halo,
            )
            if not label_summary["accepted"]:
                # Say why the labels drew nothing: the engine rejected them
                # (a name outside the packaged atlas, say), and a silent
                # empty layer looks like a broken control.
                reasons = ", ".join(
                    "%s %d" % (reason, count)
                    for reason, count in sorted(label_summary["rejected"].items())
                )
                print(
                    json.dumps(
                        {
                            "event": "warning",
                            "message": "the label layer drew nothing: %d features named, "
                            "none accepted (%s)"
                            % (label_summary["candidates"], reasons or "no reasons given"),
                        }
                    ),
                    flush=True,
                )

        rgb = apply_post_effects(rgb, quality)
        legend_name = str(ramp_layer.get("colormap", "") or "").strip() if ramp_layer else ""
        if ramp_layer is not None and bool(ramp_layer.get("legend", False)) and legend_name:
            rgb = compose_legend(rgb, legend_name, self.range)

        # The plate: the uppermost visible plate layer is the map's page.
        plate_layer = None
        for layer in visible:
            if str(layer.get("type", "")) == "plate":
                plate_layer = layer
        plate_rgb = None
        if (
            plate_layer is not None
            and aov_frame is not None
            and getattr(aov_frame, "has_depth", False)
        ):
            depth = np.asarray(aov_frame.depth())
            title = str(plate_layer.get("title", "") or "").strip()
            if not title:
                title = Path(self.dem_path).stem.replace("_", " ").title()
            legends = []
            class_ids = self.overlay_classes(imagery_layer) if imagery_layer else []
            if class_ids:
                # A class raster: the legend names the classes the palette
                # already paints.
                legends.append(plate_class_legend(class_ids))
            elif legend_name:
                try:
                    legends.append(plate_legend(legend_name, self.range))
                except Exception:  # noqa: BLE001 - the plate still has its title
                    pass
            for vector in vector_layers:
                # The vector layer gets its own entry: its swatch and its file.
                legends.append(
                    plate_vector_legend(
                        vector.get("colour", (0.9, 0.29, 0.1, 1.0)),
                        Path(str(vector.get("path", "vector"))).name,
                    )
                )
            # The furniture is the plate layer's business: each piece is
            # drawn only when the layer asks for it, and only where it is
            # true (the scale bar needs a known ground scale, the north arrow
            # a north-up view).
            caption = str(plate_layer.get("caption", "") or "")
            if not caption:
                caption = "Forge3D Studio - %s" % Path(self.dem_path).stem
            legend = legends or None
            if not bool(plate_layer.get("legend", True)):
                legend = None
            fov = math.radians(float(camera.get("fovDeg", 45.0) or 45.0))
            visible_metres = 2.0 * radius * math.tan(fov * 0.5) / max(self.scale, 1e-9)
            metres_per_pixel = visible_metres / max(width, 1)
            theta = float(camera.get("thetaDeg", 45.0) or 45.0)
            phi = float(camera.get("phiDeg", 135.0) or 135.0)
            north_up = theta <= 45.0 and min(abs(phi - 90.0), abs(phi - 450.0)) <= 3.0
            if not bool(plate_layer.get("scaleBar", True)):
                metres_per_pixel = None
            plate_rgb = compose_plate(
                rgb,
                depth,
                title=title,
                legend=legend,
                caption=caption,
                metres_per_pixel=metres_per_pixel if theta <= 45.0 else None,
                north_up=north_up and bool(plate_layer.get("northArrow", True)),
            )

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
            if plate_rgb is not None:
                stem = Path(output).with_suffix("")
                Path(str(stem) + "_plate.png").write_bytes(encode_png(plate_rgb))
            Path(output).write_bytes(png)

        return {
            "width": width,
            "height": height,
            # Which engine path made the frame: the quality surface carries the
            # screen-space effects, the plain path is the AOV renderer's.
            "renderPath": render_path,
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


#: The engine's packaged label font (its own asset, used by labels/export).
PLATE_FONT_CANDIDATES = (
    ("data", "fonts", "NotoSansLatin-subset.ttf"),
    ("assets", "fonts", "NotoSansLatin-subset.ttf"),
)


def plate_font(size: int):
    """The engine's packaged Latin font at `size`, or PIL's bitmap fallback."""
    from PIL import ImageFont

    import forge3d as f3d

    root = Path(getattr(f3d, "__file__", "")).resolve().parent
    for parts in PLATE_FONT_CANDIDATES:
        candidate = root.joinpath(*parts)
        if candidate.exists():
            return ImageFont.truetype(str(candidate), size)
    fallback = Path("assets/fonts/NotoSansLatin-subset.ttf")
    if fallback.exists():
        return ImageFont.truetype(str(fallback), size)
    return ImageFont.load_default()


def plate_legend(colormap_name: str, domain):
    """The elevation legend drawn for a plate's white page."""
    from forge3d import get_colormap
    from forge3d.legend import Legend, LegendConfig

    token = colormap_name if ":" in colormap_name else f"forge3d:{colormap_name}"
    low, high = domain
    span = max(1.0, float(high) - float(low))
    scale = max(0.5, min(3.0, 900.0 / span * 0.35))  # keep the card a sensible size
    config = LegendConfig(
        orientation="vertical",
        bar_width=max(16, int(round(22 * scale))),
        bar_height=max(120, int(round(220 * scale))),
        tick_count=5,
        label_suffix=" m",
        label_format="{:.0f}",
        font_size=max(12, int(round(14 * scale))),
        title="Elevation",
        title_font_size=max(13, int(round(16 * scale))),
        background=(255, 255, 255, 0),
    )
    return np.asarray(Legend.from_colormap(get_colormap(token), (float(low), float(high)), config).render())


def _nice_length(metres: float) -> float:
    """The largest 1, 2 or 5 times a power of ten that fits in `metres`.

    A scale bar reads in round distances, so the bar's length is snapped to
    one of those rather than to the exact pixel count.
    """
    if metres <= 0.0 or not math.isfinite(metres):
        return 0.0
    value = 1.0
    while value * 10.0 <= metres:
        value *= 10.0
    for step in (5.0, 2.0):
        while value * step <= metres:
            value *= step
    return value


def _scale_label(metres: float) -> str:
    """A scale bar's label: metres under a kilometre, kilometres above."""
    if metres >= 1000.0:
        value = metres / 1000.0
        return ("%g km" % value) if value % 1 else ("%d km" % value)
    return ("%g m" % metres) if metres % 1 else ("%d m" % metres)


def plate_class_legend(class_ids) -> np.ndarray:
    """The categorical legend for a class raster: the palette's swatches and
    the class names that palette implies."""
    from PIL import Image, ImageDraw

    ids = sorted({int(value) for value in class_ids if int(value) >= 1})
    if not ids:
        return np.zeros((1, 1, 4), dtype=np.uint8)
    scale = 1.0
    font = plate_font(13)
    title_font = plate_font(15)
    swatch = 16
    row = 22
    width = 150
    height = 26 + row * len(ids)
    canvas = Image.new("RGBA", (width, height), (255, 255, 255, 0))
    draw = ImageDraw.Draw(canvas)
    draw.text((2, 2), "Land cover", fill=(54, 54, 60), font=title_font)
    for index, class_id in enumerate(ids):
        y = 26 + index * row
        colour = CLASSIFICATION_PALETTE[min(class_id, len(CLASSIFICATION_PALETTE)) - 1]
        draw.rectangle([2, y, 2 + swatch, y + swatch], fill=tuple(colour), outline=(150, 150, 150))
        label = CLASSIFICATION_LABELS[
            min(class_id, len(CLASSIFICATION_LABELS)) - 1
        ]
        draw.text((2 + swatch + 8, y + 1), label, fill=(72, 72, 78), font=font)
    return np.asarray(canvas)


def plate_vector_legend(colour, label: str) -> np.ndarray:
    """A legend entry for the vector layer: its swatch and its file name."""
    from PIL import Image, ImageDraw

    font = plate_font(13)
    title_font = plate_font(15)
    canvas = Image.new("RGBA", (190, 52), (255, 255, 255, 0))
    draw = ImageDraw.Draw(canvas)
    draw.text((2, 2), "Vector layer", fill=(54, 54, 60), font=title_font)
    rgba = [max(0, min(255, int(round(float(value) * 255.0)))) for value in colour[:4]]
    draw.rectangle([2, 26, 18, 42], fill=(rgba[0], rgba[1], rgba[2], 255), outline=(150, 150, 150))
    draw.text((26, 27), label[:24], fill=(72, 72, 78), font=font)
    return np.asarray(canvas)


def compose_plate(
    rgb: np.ndarray,
    mask: np.ndarray,
    *,
    title: str,
    legend: "np.ndarray | list | None" = None,
    caption: str = "",
    metres_per_pixel: "float | None" = None,
    north_up: bool = False,
) -> np.ndarray:
    """A map plate: the rendered terrain cut out and laid on a white page.

    The cut-out uses the frame's **depth** buffer (the renderer's own data:
    background pixels carry zero depth), never a colour threshold, so a dark
    valley is not mistaken for background. The layout follows the gallery's
    land-cover snapshot: title centred on top, legend top-left, caption along
    the bottom, the subject cropped to its silhouette and scaled to fit.
    """
    from PIL import Image, ImageDraw

    depth = np.asarray(mask)
    if depth.ndim == 3:
        depth = depth[..., 0]
    subject_mask = depth > 0
    rows = np.flatnonzero(subject_mask.any(axis=1))
    cols = np.flatnonzero(subject_mask.any(axis=0))
    if rows.size == 0 or cols.size == 0:
        return rgb
    top, bottom = int(rows[0]), int(rows[-1]) + 1
    left, right = int(cols[0]), int(cols[-1]) + 1
    subject = rgb[top:bottom, left:right]
    alpha = (subject_mask[top:bottom, left:right].astype(np.uint8)) * 255
    subject_rgba = np.dstack([subject, alpha])

    width = subject.shape[1]
    height = subject.shape[0]
    margin = max(24, width // 28)
    title_size = max(22, width // 24)
    caption_size = max(11, width // 80)
    font_title = plate_font(title_size)
    font_caption = plate_font(caption_size)

    # One or more legends, stacked in the page's top-left corner.
    legends = []
    if legend is not None:
        legends = list(legend) if isinstance(legend, (list, tuple)) else [legend]
        legends = [item for item in legends if getattr(item, "size", 0) > 4]
    legend_w = max((item.shape[1] for item in legends), default=0)
    legend_h = sum(item.shape[0] for item in legends) + 8 * max(0, len(legends) - 1)
    page_w = width + margin * 2 + legend_w
    header = int(title_size * 2.1) + margin
    if legends:
        header = max(header, legend_h + int(title_size * 1.6) + margin)
    footer = int(caption_size * 2.4) + margin // 2
    page_h = header + height + footer

    page = Image.new("RGBA", (page_w, page_h), (255, 255, 255, 255))
    draw = ImageDraw.Draw(page)
    draw.text((page_w // 2, margin // 2), title, fill=(38, 38, 42), font=font_title, anchor="ma")
    legend_y = int(title_size * 1.5) + margin // 2
    for item in legends:
        legend_img = Image.fromarray(np.ascontiguousarray(item), "RGBA")
        page.alpha_composite(legend_img, dest=(margin, legend_y))
        legend_y += item.shape[0] + 8
    if north_up:
        # A north arrow only where it is true: the arrow is drawn for a
        # north-up view, not rotated by a guess.
        arrow_x = page_w - margin - int(title_size * 0.7)
        arrow_y = int(title_size * 0.9) + margin // 2
        arrow_len = max(18, int(title_size * 0.9))
        draw.polygon(
            [
                (arrow_x, arrow_y - arrow_len),
                (arrow_x - arrow_len // 4, arrow_y),
                (arrow_x + arrow_len // 4, arrow_y),
            ],
            fill=(38, 38, 42),
        )
        draw.text(
            (arrow_x, arrow_y + 4),
            "N",
            fill=(38, 38, 42),
            font=font_caption,
            anchor="ma",
        )
    page.alpha_composite(Image.fromarray(np.ascontiguousarray(subject_rgba), "RGBA"), dest=(margin, header))
    if metres_per_pixel and metres_per_pixel > 0:
        # A scale bar in round distances, as long as it stays between 15% and
        # 40% of the page's width.
        target = width * 0.28
        metres = target * metres_per_pixel
        nice = _nice_length(metres)
        bar_pixels = int(round(nice / metres_per_pixel))
        if bar_pixels > 8:
            bar_x = margin
            bar_y = header + height - margin // 2
            bar_h = max(3, int(title_size * 0.18))
            draw.rectangle(
                [
                    bar_x,
                    bar_y,
                    bar_x + bar_pixels,
                    bar_y + bar_h,
                ],
                fill=(38, 38, 42),
            )
            label = _scale_label(nice)
            draw.text(
                (bar_x, bar_y - int(caption_size * 1.4)),
                label,
                fill=(54, 54, 60),
                font=font_caption,
                anchor="ls",
            )
    if caption:
        draw.text(
            (page_w // 2, page_h - footer + margin // 4),
            caption,
            fill=(82, 82, 88),
            font=font_caption,
            anchor="ma",
        )
    return np.asarray(page.convert("RGB"))


def compose_legend(
    rgb: np.ndarray,
    colormap_name: str,
    domain: Tuple[float, float],
) -> np.ndarray:
    """Draw the engine's elevation legend in the frame's bottom-right corner.

    `forge3d.legend.Legend` renders the gradient bar, ticks and labels with the
    engine's own packaged text — the same object the engine's cartography uses.
    The domain is the DEM's metres, so the ticks read as elevation even though
    the shader works in scaled heights.

    Sizes follow the frame, and the whole legend shrinks until it fits: a
    viewport preview gets a compact bar, a 4K still a full one with a title.
    A frame too small for any readable legend gets none rather than a smudge.
    """
    from forge3d import get_colormap
    from forge3d.legend import Legend, LegendConfig

    token = colormap_name if ":" in colormap_name else f"forge3d:{colormap_name}"
    colormap = get_colormap(token)
    height, width = rgb.shape[:2]
    base = max(0.25, min(width, height) / 600.0)
    legend = None
    scale = base
    for step in range(5):
        scale = base * (0.72**step)
        compact = scale < 0.6
        config = LegendConfig(
            orientation="vertical",
            bar_width=max(10, int(round(22 * scale))),
            bar_height=max(48, int(round(220 * scale))),
            tick_count=5,
            label_suffix=" m",
            label_format="{:.0f}",
            font_size=max(9, int(round(14 * scale))),
            title=None if compact else "Elevation",
            title_font_size=max(11, int(round(16 * scale))),
            # A card, not a floating label: a legend has to read on dark
            # terrain and on snow.
            background=(255, 255, 255, 235),
        )
        candidate = np.asarray(Legend.from_colormap(colormap, domain, config).render())
        margin = max(6, int(round(16 * scale)))
        if candidate.ndim != 3 or candidate.shape[2] != 4 or candidate.size == 0:
            continue
        if candidate.shape[0] + 2 * margin <= height and candidate.shape[1] + 2 * margin <= width:
            legend = candidate
            break
    if legend is None:
        return rgb
    legend_h, legend_w = legend.shape[:2]
    margin = max(6, int(round(16 * scale)))
    y0 = height - margin - legend_h
    x0 = width - margin - legend_w
    patch = rgb[y0 : y0 + legend_h, x0 : x0 + legend_w].astype(np.float32)
    alpha = legend[..., 3:4].astype(np.float32) / 255.0
    out = patch * (1.0 - alpha) + legend[..., :3].astype(np.float32) * alpha
    frame = rgb.copy()
    frame[y0 : y0 + legend_h, x0 : x0 + legend_w] = np.clip(np.rint(out), 0, 255).astype(
        np.uint8
    )
    return frame


#: The vector reader. GeoPackage is a SQLite database with GeoPackage binary
#: geometry; GeoJSON is plain JSON. Neither needs a third-party package: the
#: studio's environment has no geopandas, fiona or pyproj (verified), and a
#: GeoPackage is a SQLite file.

_GPKG_MAGIC = b"GP"


def _wkb_line_strings(data: bytes) -> list[list[tuple[float, float]]]:
    """The lines inside a WKB geometry (Point/LineString/Polygon/Multi*)."""
    import struct

    lines: list[list[tuple[float, float]]] = []

    def parse(offset: int, depth: int = 0):
        if offset + 5 > len(data) or depth > 4:
            return offset
        order = data[offset]
        prefix = "<" if order == 1 else ">"
        (geometry_type,) = struct.unpack_from(prefix + "I", data, offset + 1)
        offset += 5
        base = geometry_type % 1000
        if base == 1:  # Point
            offset += 16
        elif base == 2:  # LineString
            (count,) = struct.unpack_from(prefix + "I", data, offset)
            offset += 4
            points = []
            for _ in range(count):
                x, y = struct.unpack_from(prefix + "dd", data, offset)
                points.append((x, y))
                offset += 16
            lines.append(points)
        elif base == 3:  # Polygon
            (rings,) = struct.unpack_from(prefix + "I", data, offset)
            offset += 4
            for _ in range(rings):
                (count,) = struct.unpack_from(prefix + "I", data, offset)
                offset += 4 + count * 16
        elif base in (4, 5, 6, 7):  # Multi* / GeometryCollection
            (parts,) = struct.unpack_from(prefix + "I", data, offset)
            offset += 4
            for _ in range(parts):
                offset = parse(offset, depth + 1)
        return offset

    parse(0)
    return lines


def _wkb_points(data: bytes) -> list[tuple[float, float]]:
    """The points inside a WKB geometry (Point/MultiPoint/Polygon/LineString)."""
    import struct

    points: list[tuple[float, float]] = []

    def parse(offset: int, depth: int = 0):
        if offset + 5 > len(data) or depth > 4:
            return offset
        order = data[offset]
        prefix = "<" if order == 1 else ">"
        (geometry_type,) = struct.unpack_from(prefix + "I", data, offset + 1)
        offset += 5
        base = geometry_type % 1000
        if base == 1:  # Point
            x, y = struct.unpack_from(prefix + "dd", data, offset)
            points.append((x, y))
            offset += 16
        elif base == 2:  # LineString
            (count,) = struct.unpack_from(prefix + "I", data, offset)
            offset += 4 + count * 16
        elif base == 3:  # Polygon
            (rings,) = struct.unpack_from(prefix + "I", data, offset)
            offset += 4
            for _ in range(rings):
                (count,) = struct.unpack_from(prefix + "I", data, offset)
                offset += 4 + count * 16
        elif base == 4:  # MultiPoint
            (parts,) = struct.unpack_from(prefix + "I", data, offset)
            offset += 4
            for _ in range(parts):
                offset = parse(offset, depth + 1)
        elif base in (5, 6, 7):  # Multi* / GeometryCollection
            (parts,) = struct.unpack_from(prefix + "I", data, offset)
            offset += 4
            for _ in range(parts):
                offset = parse(offset, depth + 1)
        return offset

    parse(0)
    return points


def _gpkg_geometry_points(blob: bytes) -> list[tuple[float, float]]:
    """The points inside a GeoPackage binary geometry blob."""
    import struct

    if len(blob) < 8 or blob[:2] != _GPKG_MAGIC:
        return []
    flags = blob[3]
    offset = 8
    if flags & 0x20:  # the srs id is present
        offset += 4
    envelope = (flags >> 1) & 0x07
    offset += {0: 0, 1: 32, 2: 48, 3: 48, 4: 64}.get(envelope, 0)
    if offset >= len(blob):
        return []
    return _wkb_points(blob[offset:])


def _gpkg_geometry_lines(blob: bytes) -> list[list[tuple[float, float]]]:
    """The lines inside a GeoPackage binary geometry blob."""
    import struct

    if len(blob) < 8 or blob[:2] != _GPKG_MAGIC:
        return []
    flags = blob[3]
    offset = 8
    if flags & 0x20:  # the srs id is present
        offset += 4
    envelope = (flags >> 1) & 0x07
    offset += {0: 0, 1: 32, 2: 48, 3: 48, 4: 64}.get(envelope, 0)
    if offset >= len(blob):
        return []
    return _wkb_line_strings(blob[offset:])


def read_vector_features(path: str, label_field: str = ""):
    """The features of a vector file, in the file's own coordinates.

    Returns `(lines, points, crs)`:

    * `lines` — the polylines, in the file's own coordinates;
    * `points` — `(id, x, y, text)` for the point features that carry a
      non-empty value in `label_field`; the text is what a label would say,
      and a file without that attribute simply has no points;
    * `crs` — the file's CRS (an EPSG string for a GeoPackage, whatever the
      GeoJSON says) or `None` when the file declares none. A GeoPackage's CRS
      comes from `gpkg_geometry_columns`; a GeoJSON's is only trusted when it
      names one.
    """
    import json
    import os
    import sqlite3

    field = str(label_field or "").strip()
    lowered = path.lower()
    if lowered.endswith((".json", ".geojson")):
        with open(path, "r", encoding="utf-8") as handle:
            document = json.load(handle)
        lines: list[list[tuple[float, float]]] = []
        points: list[tuple[str, float, float, str]] = []
        crs = geojson_crs(document)
        for index, feature in enumerate(document.get("features", []) or []):
            geometry = feature.get("geometry") or {}
            kind = str(geometry.get("type", "")).lower()
            coordinates = geometry.get("coordinates") or []
            if kind == "linestring":
                lines.append([(float(x), float(y)) for x, y in coordinates])
            elif kind == "multilinestring":
                for part in coordinates:
                    lines.append([(float(x), float(y)) for x, y in part])
            elif kind == "point":
                text = _feature_text(feature, field)
                if text:
                    points.append(
                        (
                            str(feature.get("id", "feature-%d" % index)),
                            float(coordinates[0]),
                            float(coordinates[1]),
                            text,
                        )
                    )
        return lines, points, crs

    if not lowered.endswith((".gpkg", ".sqlite", ".db")):
        raise RuntimeError(
            "unsupported vector file %r: expected GeoPackage (.gpkg) or GeoJSON" % (path,)
        )
    connection = sqlite3.connect("file:%s?mode=ro" % path, uri=True)
    try:
        cursor = connection.cursor()
        table = None
        geometry_column = "geom"
        srs_id = None
        try:
            row = cursor.execute(
                "SELECT table_name FROM gpkg_contents WHERE data_type = 'features' LIMIT 1"
            ).fetchone()
            table = row[0] if row else None
        except sqlite3.Error:
            table = None
        if table is None:
            raise RuntimeError("the GeoPackage has no feature table")
        try:
            row = cursor.execute(
                "SELECT column_name, srs_id FROM gpkg_geometry_columns WHERE table_name = ?",
                (table,),
            ).fetchone()
            if row is not None:
                geometry_column, srs_id = row[0], row[1]
        except sqlite3.Error:
            pass
        quoted_table = table.replace('"', '""')
        quoted_geometry = geometry_column.replace('"', '""')
        # The label attribute is read only when it exists: a file without it
        # has no labels, and the column list says so before the query runs.
        columns = set()
        try:
            for row in cursor.execute('PRAGMA table_info("%s")' % quoted_table):
                columns.add(str(row[1]))
        except sqlite3.Error:
            columns = set()
        selected = [quoted_geometry]
        if field and field in columns:
            selected.append('"%s"' % field.replace('"', '""'))
        lines = []
        points = []
        for row in cursor.execute(
            'SELECT %s FROM "%s"' % (", ".join(selected), quoted_table)
        ):
            blob = row[0]
            if not isinstance(blob, (bytes, bytearray)):
                continue
            lines.extend(_gpkg_geometry_lines(bytes(blob)))
            if len(row) > 1 and field:
                text = str(row[1] or "").strip()
                if text:
                    for x, y in _gpkg_geometry_points(bytes(blob)):
                        points.append(("feature-%s" % len(points), x, y, text))
        crs = "EPSG:%d" % int(srs_id) if srs_id is not None else None
        return lines, points, crs
    finally:
        connection.close()


def geojson_crs(document) -> Optional[str]:
    """The CRS a GeoJSON declares, as an EPSG string.

    RFC 7946 says a GeoJSON's coordinates are WGS 84 longitude/latitude, so a
    document with no `crs` member is EPSG:4326. The old `crs` member (GeoJSON
    2008) may name another system; the engine's own parser reads it, so the
    studio never grows a second CRS grammar.
    """
    declared = document.get("crs")
    name = None
    if isinstance(declared, str):
        name = declared
    elif isinstance(declared, dict):
        properties = declared.get("properties")
        if isinstance(properties, dict):
            name = properties.get("name")
        elif declared.get("name"):
            name = declared.get("name")
    if not name:
        # RFC 7946: no member means WGS 84 longitude/latitude.
        return "EPSG:4326"
    try:
        from forge3d import gis

        parsed = gis.parse_crs(str(name))
    except Exception:  # noqa: BLE001 - an unreadable CRS is an unknown CRS
        return None
    authority = parsed.get("authority") if isinstance(parsed, dict) else None
    code = authority.get("code") if isinstance(authority, dict) else None
    if not code:
        return None
    return "EPSG:%s" % code


def _feature_text(feature, field: str) -> str:
    """A feature's value in `field`, or "" when it has none."""
    if not field:
        return ""
    properties = feature.get("properties")
    if isinstance(properties, dict):
        value = properties.get(field)
    else:
        value = feature.get(field)
    if value is None:
        return ""
    return str(value).strip()


def crs_code(crs) -> str:
    """The EPSG code of a CRS string ("EPSG:4326" -> "4326"), or "" when the
    CRS does not name one."""
    text = str(crs or "").strip()
    if ":" in text:
        text = text.rsplit(":", 1)[-1]
    return text if text.isdigit() else ""


def _sample_height(heights, u: float, v: float) -> float:
    """The height at a terrain UV, bilinearly (the engine's own sampling)."""
    rows, cols = heights.shape[:2]
    x = min(max(u, 0.0), 1.0) * (cols - 1)
    y = min(max(v, 0.0), 1.0) * (rows - 1)
    x0 = int(x)
    y0 = int(y)
    x1 = min(x0 + 1, cols - 1)
    y1 = min(y0 + 1, rows - 1)
    tx = x - x0
    ty = y - y0
    top = float(heights[y0, x0]) * (1.0 - tx) + float(heights[y0, x1]) * tx
    bottom = float(heights[y1, x0]) * (1.0 - tx) + float(heights[y1, x1]) * tx
    return top * (1.0 - ty) + bottom * ty


def vector_label_anchors(params, heights, bounds, points, width: int, height: int):
    """The screen anchors of the labelled points, through the engine's camera.

    Each point is placed on the terrain (its own coordinates become the
    terrain's UV, the height is sampled there) and projected with the engine's
    own camera (`project_terrain_points`), so a label sits on its feature
    rather than near it. A point whose anchor lands over the sky — where the
    engine's terrain depth says there is no ground — is dropped: a label must
    not float in the air.
    """
    import numpy as np

    if not points or bounds is None:
        return []
    west, south, east, north = (float(value) for value in bounds)
    if not east > west or not north > south:
        return []

    placed = []
    for identifier, x, y, text in points:
        u = (float(x) - west) / (east - west)
        v = (north - float(y)) / (north - south)
        if not (0.0 <= u <= 1.0 and 0.0 <= v <= 1.0):
            continue
        placed.append((identifier, u, v, text))
    if not placed:
        return []

    batch = [[u, v, _sample_height(heights, u, v), 0.0] for _id, u, v, _text in placed]
    projected = params.project_terrain_points(batch, shared_frame=True)
    if not projected:
        return []

    # Where is the terrain visible? The engine rasterizes the DEM's depth with
    # the same camera, so an anchor over the sky has no ground under it.
    terrain_depth = None
    try:
        terrain_depth = np.asarray(
            params.project_terrain_depth(
                heights, (int(width), int(height)), None, None, True
            )
        )
    except Exception:  # noqa: BLE001 - the depth test is optional
        terrain_depth = None

    anchors = []
    for (identifier, _u, _v, text), value in zip(placed, projected):
        if value is None:
            continue
        px, py, depth = float(value[0]), float(value[1]), float(value[2])
        if terrain_depth is not None:
            column = int(px)
            row = int(py)
            if 0 <= row < terrain_depth.shape[0] and 0 <= column < terrain_depth.shape[1]:
                # The raster's background is exactly 1.0 (the far plane); the
                # terrain's depth is just below it, so the test is "is this
                # pixel the background", not "is it far away".
                if float(terrain_depth[row, column]) >= 1.0 - 1e-6:
                    continue
        anchors.append({"id": identifier, "text": text, "x": px, "y": py, "depth": depth})
    return anchors


def vector_label_records(anchors):
    """The engine's label records for the projected anchors.

    The records are the engine's screen-space label records: the geometry is
    the anchor in pixels and the projected anchor carries the engine's own
    depth, so the plan can reason about the label's place in the frame.
    """
    records = []
    for anchor in anchors:
        records.append(
            {
                "id": anchor["id"],
                "geometry": {
                    "type": "Point",
                    "coordinates": [float(anchor["x"]), float(anchor["y"])],
                },
                "properties": {"name": anchor["text"]},
                "projected_anchor": [
                    float(anchor["x"]),
                    float(anchor["y"]),
                    float(anchor["depth"]),
                ],
            }
        )
    return records


def vector_label_plan(
    records,
    width: int,
    height: int,
    *,
    colour=(0.08, 0.08, 0.1, 1.0),
    size_px: float = 14.0,
    halo_colour=(1.0, 1.0, 1.0, 0.8),
    halo_width: float = 1.5,
):
    """The engine's own label plan for the layer's named features.

    The engine places the labels, resolves their collisions and shapes their
    glyphs (the packaged Latin atlas); the studio supplies the records and the
    typography, and nothing else.
    """
    import forge3d as f3d

    features = [
        {
            "id": record["id"],
            "geometry": record["geometry"],
            "properties": record["properties"],
        }
        for record in records
    ]
    layer = f3d.LabelLayer.from_features(
        features,
        text="name",
        crs=None,
        target_crs=None,
        occlusion="none",
        layer_id="vector-labels",
        atlas=f3d.FontAtlas.default_latin(),
    )
    for record, source in zip(layer.labels, records):
        record["projected_anchor"] = list(source["projected_anchor"])
    plan = f3d.LabelPlan.compile(
        labels=layer.labels,
        camera=None,
        viewport=(int(width), int(height)),
        glyph_atlas=layer.glyph_atlas,
        typography={
            "font_size": float(size_px),
            "color": [float(channel) for channel in colour],
            "halo_color": [float(channel) for channel in halo_colour],
            "halo_width_px": float(halo_width),
        },
    )
    return layer, plan


def cloud_settings(payload) -> Optional[Any]:
    """The engine's cloud settings from the request, or None when off.

    The clouds are the engine's own (`CloudSettings`): a coverage, a density
    and a shadow strength that the quality path composites into the terrain's
    direct sun.
    """
    if not isinstance(payload, dict):
        return None
    from forge3d.terrain_params import CloudSettings

    enabled = bool(payload.get("enabled", False))
    shadows = bool(payload.get("shadowsEnabled", enabled))
    if not enabled and not shadows:
        return None
    return CloudSettings(
        enabled=enabled,
        shadows_enabled=shadows,
        coverage=float(payload.get("coverage", 0.5)),
        density=float(payload.get("density", 0.5)),
        shadow_strength=float(payload.get("shadowStrength", 0.35)),
        quality=str(payload.get("quality", "medium")),
    )


def compose_vector_labels(
    rgb,
    params,
    heights,
    bounds,
    points,
    width: int,
    height: int,
    *,
    colour=(0.08, 0.08, 0.1, 1.0),
    size_px: float = 14.0,
    halo_colour=(1.0, 1.0, 1.0, 0.8),
    halo_width: float = 1.5,
):
    """The engine's own label compositor over the frame.

    Nothing here lays out glyphs: the engine's `LabelPlan.compile` places the
    labels, resolves their collisions and shapes their glyphs, and the engine's
    `composite_label_layers` draws them with the packaged MSDF atlas, the halo
    and the rasterizer — the same stream `MapScene.render` uses. The studio
    only supplies the features and the anchors its own camera projected.

    The anchors are screen positions (the studio owns the camera), so the
    records are the engine's screen-space label records.
    """
    import numpy as np

    anchors = vector_label_anchors(params, heights, bounds, points, width, height)
    if not anchors:
        return rgb, {"candidates": 0, "accepted": 0, "rejected": {}}

    import forge3d as f3d

    layer, plan = vector_label_plan(
        vector_label_records(anchors),
        width,
        height,
        colour=colour,
        size_px=size_px,
        halo_colour=halo_colour,
        halo_width=halo_width,
    )
    rejected: Dict[str, int] = {}
    for label in plan.rejected:
        reason = str(getattr(label, "reason", "rejected"))
        rejected[reason] = rejected.get(reason, 0) + 1
    summary = {
        "candidates": len(anchors),
        "accepted": len(plan.accepted),
        "rejected": rejected,
    }
    if not plan.accepted:
        return rgb, summary

    composited, rendered = f3d.composite_label_layers(
        np.ascontiguousarray(rgb, dtype=np.uint8),
        [layer],
        {layer.layer_id: plan},
    )
    if not rendered:
        return rgb, summary
    pixels = np.asarray(composited)
    if pixels.ndim != 3 or pixels.shape[2] < 3:
        return rgb, summary
    return np.ascontiguousarray(pixels[:, :, :3], dtype=np.uint8), summary


def vector_overlay_rgba(
    params,
    heights,
    bounds,
    lines,
    width: int,
    height: int,
    colour=(1.0, 0.29, 0.1, 1.0),
    stroke_px: float = 3.0,
    densify: int = 256,
):
    """The engine's projection and the engine's own vector rasterizer.

    Nothing here is a second implementation: the terrain points go through
    `TerrainRenderParams.project_terrain_points` (the engine's exact camera
    and height transform), the lines are rasterized by
    `forge3d.vector_render_oit_py` (the engine's anti-aliased OIT bridge), and
    the result is composited over the frame.

    The lines arrive in the raster's own coordinates. They are placed on the
    terrain by mapping them to the terrain's UV, sampling the height there
    (so the line follows the ground) and projecting through the engine's
    camera. A point that lands where the terrain is not present is dropped, so
    the lines do not float over the sky.
    """
    import numpy as np

    if not lines or bounds is None:
        return None
    west, south, east, north = (float(value) for value in bounds)
    if not east > west or not north > south:
        raise RuntimeError("the terrain has no measurable bounds to place vectors on")

    # 1. the geographic lines become terrain UVs, densified along the ground.
    polylines_uv = []
    for line in lines:
        if len(line) < 2:
            continue
        points = []
        for index in range(len(line) - 1):
            x0, y0 = line[index]
            x1, y1 = line[index + 1]
            u0 = (x0 - west) / (east - west)
            u1 = (x1 - west) / (east - west)
            v0 = (north - y0) / (north - south)
            v1 = (north - y1) / (north - south)
            steps = max(1, int(densify * max(abs(u1 - u0), abs(v1 - v0))))
            for step in range(steps):
                t = step / steps
                points.append((u0 + (u1 - u0) * t, v0 + (v1 - v0) * t))
        u, v = line[-1]
        points.append(((u - west) / (east - west), (north - v) / (north - south)))
        if len(points) >= 2:
            polylines_uv.append(points)
    if not polylines_uv:
        return None

    # 2. the engine's projection: terrain UV + height + offset -> screen pixels.
    projected = []
    for points in polylines_uv:
        batch = [[u, v, _sample_height(heights, u, v), 0.0] for u, v in points]
        # The shared frame is the production mesh's own sampling (a 512-vertex
        # grid over the DEM) and the same camera the mesh path renders with, so
        # the line lands where the ground is.
        values = params.project_terrain_points(batch, shared_frame=True)
        projected.append(values)
    if not any(projected):
        return None

    # 3. where is the terrain visible? The engine rasterizes the DEM's depth
    #    with the same camera, so a line over the sky is dropped.
    terrain_depth = None
    try:
        terrain_depth = np.asarray(
            params.project_terrain_depth(
                heights, (int(width), int(height)), None, None, True
            )
        )
    except Exception:  # noqa: BLE001 - the overlay is optional
        terrain_depth = None

    # 4. the rasterizer works in NDC; the engine's projection returns pixels.
    polylines = []
    for values in projected:
        current = []
        for value in values:
            if value is None:
                if len(current) >= 2:
                    polylines.append(current)
                current = []
                continue
            px, py, depth = float(value[0]), float(value[1]), float(value[2])
            if terrain_depth is not None:
                column = int(px)
                row = int(py)
                if 0 <= row < terrain_depth.shape[0] and 0 <= column < terrain_depth.shape[1]:
                    # The raster's background is exactly 1.0 (the far plane);
                    # the terrain's depth is just below it, so the test is
                    # "is this pixel the background", not "is it far away".
                    if float(terrain_depth[row, column]) >= 1.0 - 1e-6:
                        # No terrain at this pixel: the line is over the sky.
                        if len(current) >= 2:
                            polylines.append(current)
                        current = []
                        continue
            current.append((2.0 * px / max(width, 1) - 1.0, 1.0 - 2.0 * py / max(height, 1)))
        if len(current) >= 2:
            polylines.append(current)
    if not polylines:
        return None

    # One polyline per segment, the same shape MapScene's own payload uses
    # ("a projected path is densified along the DEM, so give each segment its
    # own instance"). The OIT bridge draws one instance per polyline, so a
    # densified path handed over as one polyline would draw only its first
    # segment.
    segments = []
    for path in polylines:
        for index in range(len(path) - 1):
            segments.append((path[index], path[index + 1]))
    if not segments:
        return None

    import forge3d as f3d

    overlay = f3d.vector_render_oit_py(
        int(width),
        int(height),
        points_xy=None,
        point_rgba=None,
        point_size=None,
        polylines=[[a, b] for a, b in segments],
        polyline_rgba=[colour for _ in segments],
        stroke_width=[float(stroke_px) for _ in segments],
    )
    return np.asarray(overlay, dtype=np.uint8)


def composite_overlay(rgb: np.ndarray, overlay) -> np.ndarray:
    """The overlay over the frame (the engine's own compositing rule)."""
    if overlay is None:
        return rgb
    source = np.asarray(overlay, dtype=np.uint8)
    if source.ndim != 3 or source.shape[:2] != rgb.shape[:2] or source.shape[2] != 4:
        raise RuntimeError("the vector overlay does not match the frame")
    alpha = source[..., 3:4].astype(np.float32) / 255.0
    bottom = rgb.astype(np.float32)
    out = bottom * (1.0 - alpha) + source[..., :3].astype(np.float32) * alpha
    return np.clip(np.rint(out), 0, 255).astype(np.uint8)



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


#: The land-cover palette the engine's own gallery uses: water, trees, flooded
#: vegetation, crops, built area, bare ground, snow, rangeland, plus four spare
#: categories for a classification with more classes.
CLASSIFICATION_PALETTE = [
    (65, 155, 223),   # water
    (57, 125, 73),    # trees
    (122, 135, 198),  # flooded vegetation
    (228, 150, 53),   # crops
    (196, 40, 27),    # built area
    (165, 155, 143),  # bare ground
    (168, 235, 255),  # snow
    (227, 226, 195),  # rangeland
    (150, 100, 200),
    (90, 190, 170),
    (210, 120, 160),
    (120, 120, 120),
]

#: A raster pixel darker than this in every channel is nodata — the gallery
#: land-cover recipe's own rule (LANDCOVER_INVALID_CHANNEL_MAX).
INVALID_CHANNEL_MAX = 15.0
#: How close (in 0-255 units) a pixel's colour has to be to a palette entry
#: for the raster to count as a class map rather than an image.
CLASS_SNAP_TOLERANCE = 48.0
#: The share of valid pixels that have to snap before the raster is treated as
#: a class map.
CLASS_SNAP_SHARE = 0.8


def snap_to_classes(rgb: np.ndarray):
    """Snap a class map's colours to the land-cover palette.

    A land-cover export is often an *image* of the classes rather than class
    IDs: the gallery's own recipe snaps each pixel to the nearest palette
    entry (`snap_overlay_to_legend`). This does the same, and returns the class
    IDs so the plate can name them.

    It only applies when the raster really is a class map: at least
    CLASS_SNAP_SHARE of the valid pixels have to be close to a palette colour.
    An orthophoto is not a class map and is left alone.
    """
    valid = rgb.max(axis=2) > INVALID_CHANNEL_MAX
    if not valid.any():
        return None, []
    palette = np.asarray(CLASSIFICATION_PALETTE, dtype=np.float32)
    flat = rgb[valid]
    distances = np.sqrt(((flat[:, None, :] - palette[None, :, :]) ** 2).sum(axis=2))
    nearest = distances.argmin(axis=1)
    closest = distances[np.arange(len(nearest)), nearest]
    # Only the *named* classes count towards the share: the spare palette
    # entries are greys and violets, and an orthophoto is full of those. A
    # class map is mostly made of the named classes.
    named = nearest < len(CLASSIFICATION_LABELS) - 4
    share = float(((closest <= CLASS_SNAP_TOLERANCE) & named).mean())
    if share < CLASS_SNAP_SHARE:
        return None, []
    classes = np.zeros(rgb.shape[:2], dtype=np.int32)
    classes[valid] = nearest + 1
    return classes, sorted({int(value) for value in np.unique(classes[valid])})


#: The class names the land-cover palette implies. The drape paints class 1 as
#: water, 2 as trees and so on, so the legend names the same classes the
#: colours already stand for.
CLASSIFICATION_LABELS = [
    "Water",
    "Trees",
    "Flooded vegetation",
    "Crops",
    "Built area",
    "Bare ground",
    "Snow",
    "Rangeland",
    "Class 9",
    "Class 10",
    "Class 11",
    "Class 12",
]

#: A single-band classification's salt-and-pepper, with the gallery land-cover
#: recipe's own numbers: a cell whose 3x3 block holds at most this many cells of
#: its own class (the cell included) is a speck ...
DESPECKLE_MAX_SUPPORT = 2
#: ... and it takes the block's majority class when that class holds at least
#: this many cells. Two passes.
DESPECKLE_MIN_MAJORITY = 4
DESPECKLE_PASSES = 2


def despeckle_classes(classes, passes=DESPECKLE_PASSES):
    """Absorb isolated class cells into their neighbours' majority.

    The same 3x3 block rule the Swiss land-cover gallery applies to its
    classification before colouring (`examples/swiss_terrain_landcover_viewer.py`),
    counting the cell itself: a class with at most 2 cells in the block (the
    cell alone, or with one neighbour) is a speck, and it takes the block's
    majority when that majority holds at least 4 cells. Pixels below class 1
    (nodata) are left alone, so the raster's mask survives.
    """
    cleaned = np.array(classes, copy=True)
    height, width = cleaned.shape
    present = [value for value in np.unique(cleaned) if value >= 1]
    for _ in range(max(0, int(passes))):
        same_count = np.zeros((height, width), dtype=np.uint8)
        best_count = np.zeros((height, width), dtype=np.uint8)
        best_class = np.full((height, width), -1, dtype=np.int32)
        for value in present:
            mask = cleaned == value
            padded = np.pad(mask, 1, mode="constant", constant_values=False).astype(np.uint8)
            count = (
                padded[0:height, 0:width]
                + padded[0:height, 1 : width + 1]
                + padded[0:height, 2 : width + 2]
                + padded[1 : height + 1, 0:width]
                + padded[1 : height + 1, 1 : width + 1]
                + padded[1 : height + 1, 2 : width + 2]
                + padded[2 : height + 2, 0:width]
                + padded[2 : height + 2, 1 : width + 1]
                + padded[2 : height + 2, 2 : width + 2]
            )
            # `count` includes the cell itself, exactly like the gallery.
            same_count[mask] = count[mask]
            replace = count > best_count
            best_count[replace] = count[replace]
            best_class[replace] = value
        replace = (
            (cleaned >= 1)
            & (same_count <= DESPECKLE_MAX_SUPPORT)
            & (best_count >= DESPECKLE_MIN_MAJORITY)
            & (best_class != cleaned)
        )
        if not np.any(replace):
            break
        cleaned[replace] = best_class[replace]
    return cleaned

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
