"""Raster source identity and terrain-UV preparation for MapScene."""
from __future__ import annotations
import hashlib
import io
import json
import struct
import tempfile
from contextlib import contextmanager
from pathlib import Path


def raster_content_hashes(recipe):
    """Identify source bytes before cache lookup, including in-place edits."""
    from .map_scene import MapSceneNativeUnavailable, RasterOverlay
    from ._map_scene_validation import diagnostic_block
    from .raster_style import BivariateRasterStyle, raster_values_to_dict
    identities = []
    for layer in recipe.layers:
        if not isinstance(layer, RasterOverlay):
            continue
        sources = [("primary", layer.path, layer.data)]
        if isinstance(layer.style, BivariateRasterStyle):
            sources.append(("secondary", layer.secondary_path, layer.secondary_data))
        for role, source_path, data in sources:
            try:
                digest = hashlib.sha256()
                if data is not None:
                    payload = raster_values_to_dict(data)
                    digest.update(json.dumps(payload, sort_keys=True, allow_nan=False,
                                             separators=(",", ":")).encode("utf-8"))
                else:
                    path = Path(source_path or "")
                    if layer.style is None and path.suffix.lower() not in {".png", ".tif", ".tiff"}:
                        raise ValueError("supported unstyled raster sources are PNG and GeoTIFF")
                    with path.open("rb") as source:
                        while chunk := source.read(io.DEFAULT_BUFFER_SIZE):
                            digest.update(chunk)
                identity = {"layer_id": layer.layer_id, "sha256": digest.hexdigest()}
                if layer.style is not None:
                    identity["source"] = role
                identities.append(identity)
            except (OSError, ValueError, TypeError) as exc:
                source_name = f" {role}" if layer.style is not None else ""
                raise MapSceneNativeUnavailable([diagnostic_block(
                    layer=str(layer.layer_id), reason=f"RasterOverlay{source_name} source is unavailable: {exc}",
                    required_native="readable RasterOverlay source",
                )]) from exc
    return identities


def raster_route_blocks(recipe):
    """Reject raster stacks whose order the single thematic pass cannot preserve."""
    from .map_scene import RasterOverlay
    from ._map_scene_validation import diagnostic_block
    layers = [layer for layer in recipe.layers if isinstance(layer, RasterOverlay)]
    styled = [layer for layer in layers if layer.style is not None]
    if len(styled) > 1 or (styled and len(styled) != len(layers)):
        return [diagnostic_block(
            layer=str(layer.layer_id),
            reason="native terrain supports one styled RasterOverlay or an ordered unstyled raster stack; "
                   "mixed styled/unstyled overlays cannot preserve recipe order",
            required_native="separate thematic and terrain-UV raster passes",
        ) for layer in layers]
    return []


def _overlay_in_terrain_uv(layer, recipe, shape, row_range=None):
    import numpy as np
    from .map_scene import (_load_native_raster_overlay, _terrain_alignment_grid,
                            _terrain_scene_diagonal, _raster_to_rgba8,
                            _resize_nearest_rgba)
    from ._map_scene_common import _metadata_dict, _same_crs
    path = Path(layer.path or '')
    metadata = _metadata_dict(layer.metadata)
    grid = _terrain_alignment_grid(recipe.terrain, target_crs=recipe.target_crs or recipe.terrain.crs,
                                   fallback_shape=shape)
    if path.suffix.lower() in {'.tif', '.tiff'}:
        if grid is None:
            raise ValueError('GeoTIFF draping requires a georeferenced terrain grid')
        from .alignment import _require_rasterio, _affine_from_metadata, _resampling_method
        from rasterio.windows import Window
        from rasterio._warp import _do_reproject
        rasterio = _require_rasterio()
        from rasterio.warp import reproject
        row_start, row_stop = (0, shape[0]) if row_range is None else (int(row_range[0]), int(row_range[1]))
        window_height = row_stop - row_start
        with rasterio.open(path) as source:
            if source.crs is None:
                raise ValueError('GeoTIFF draping requires source CRS metadata')
            raw = source.read()
            valid = np.min(source.read_masks(), axis=0) > 0
            if not np.isfinite(raw[:, valid]).all():
                raise ValueError('raster source contains non-finite valid samples')
            rgba = _raster_to_rgba8(np.moveaxis(raw, 0, -1), valid)
            # Preserve both nodata and source footprint even for opaque RGB input.
            rgba[..., 3] = np.minimum(rgba[..., 3], source.dataset_mask())
            output = np.zeros((window_height, shape[1], 4), dtype=np.uint8)
            # GDAL can move a valid zero byte to 1 when it coincides with the
            # destination nodata value. Warp into a floating-point band so its
            # nodata adjustment stays below one byte, then quantize explicitly.
            band_output = np.zeros((window_height, shape[1]), dtype=np.float32)
            base_transform = _affine_from_metadata(grid['transform'])
            dst_transform = base_transform if row_start == 0 else base_transform * type(base_transform)(1, 0, 0, 0, 1, row_start)
            for band in range(4):
                band_output.fill(0)
                reproject(rgba[..., band], band_output, src_transform=source.transform,
                          src_crs=source.crs, dst_transform=dst_transform,
                          dst_crs=grid['crs'], src_nodata=None, dst_nodata=0,
                          window=Window(0, row_start, shape[1], window_height) if row_range else None,
                          resampling=_resampling_method(str(metadata.get('alignment_resampling') or metadata.get('resampling') or 'nearest')))
                output[..., band] = np.clip(np.rint(band_output), 0, 255).astype(np.uint8)
            return output
    if not _same_crs(layer.crs, recipe.target_crs or recipe.terrain.crs):
        raise ValueError('PNG draping requires the terrain CRS; use a georeferenced GeoTIFF for reprojection')
    rgba = _load_native_raster_overlay(layer)
    if rgba is None:
        raise ValueError('raster source is missing, unreadable, or unsupported')
    bounds = metadata.get('bounds')
    row_start, row_stop = (0, shape[0]) if row_range is None else (int(row_range[0]), int(row_range[1]))
    if bounds is None:
        if metadata.get('transform') is not None or metadata.get('geotransform') is not None:
            raise ValueError('PNG transform requires explicit bounds; use GeoTIFF for affine reprojection')
        from .map_scene import _resize_nearest_rgba
        if row_range is None:
            return _resize_nearest_rgba(rgba, shape)
        target_h, target_w = int(shape[0]), int(shape[1])
        src_h, src_w = rgba.shape[:2]
        sample_y = np.clip(
            np.arange(row_start, row_stop, dtype=np.int64) * src_h // target_h, 0, src_h - 1
        )
        sample_x = np.clip(np.arange(target_w, dtype=np.int64) * src_w // target_w, 0, src_w - 1)
        return np.ascontiguousarray(rgba[sample_y[:, None], sample_x[None, :]])
    bounds = np.asarray(bounds, dtype=np.float64)
    if bounds.shape != (4,) or not np.isfinite(bounds).all() or np.any(bounds[2:] <= bounds[:2]):
        raise ValueError('raster bounds must be finite [west, south, east, north] with positive extent')
    yy, xx = np.mgrid[row_start:row_stop, :shape[1]]
    if grid is not None:
        from .alignment import _affine_from_metadata
        transform = _affine_from_metadata(grid['transform'])
        east, north = transform * (xx + 0.5, yy + 0.5)
    else:
        span = max(1.0, _terrain_scene_diagonal(recipe.terrain))
        east = (xx / max(shape[1] - 1, 1) - 0.5) * span
        north = (0.5 - yy / max(shape[0] - 1, 1)) * span
    u = (east - bounds[0]) / (bounds[2] - bounds[0])
    v = (bounds[3] - north) / (bounds[3] - bounds[1])
    inside = (u >= 0.0) & (u < 1.0) & (v >= 0.0) & (v < 1.0)
    col = np.clip(np.floor(u * rgba.shape[1]).astype(np.int64), 0, rgba.shape[1] - 1)
    row = np.clip(np.floor(v * rgba.shape[0]).astype(np.int64), 0, rgba.shape[0] - 1)
    output = rgba[row, col].copy()
    output[~inside] = 0
    return output


@contextmanager
def terrain_raster_drape(recipe, heightmap):
    """Keep one ordered straight-alpha UV albedo map alive through native draw."""
    import numpy as np
    from .map_scene import MapSceneNativeUnavailable, RasterOverlay
    from ._map_scene_validation import diagnostic_block
    from ._native import get_native_module
    from .helpers.offscreen import save_png_deterministic
    blocks = raster_route_blocks(recipe)
    if blocks:
        raise MapSceneNativeUnavailable(blocks)
    layers = [layer for layer in recipe.layers if isinstance(layer, RasterOverlay) and layer.style is None]
    if not layers:
        yield None, 0
        return
    raster_content_hashes(recipe)
    shape = heightmap.shape
    native = get_native_module()
    reserve = getattr(native, '_reserve_label_depth_host_allocation', None)
    if not callable(reserve):
        raise MapSceneNativeUnavailable([diagnostic_block(
            layer=str(layers[0].layer_id), reason='native host allocation tracking is unavailable',
            required_native='terrain-UV raster drape host allocation tracking')])
    # The drape is composed in row bands, so the peak is one full-width
    # RGBA8 canvas (4 B/px) plus a handful of float32 band buffers — no
    # full-size float grid is ever alive. The band height targets ~48 MiB
    # of band buffers each (source 16 B/px, accumulator 16 B/px, two
    # rounding temporaries each).
    band_rows = max(1, min(int(shape[0]), int(48 * 1024 * 1024 / (shape[1] * 16 * 4))))
    output = np.empty((*shape, 4), dtype=np.uint8)
    band_bytes = band_rows * shape[1] * (
        2 * 4 * np.dtype(np.float32).itemsize +  # accumulator + source
        4 * 3 * np.dtype(np.float32).itemsize    # rounding/arithmetic temporaries
    ) + band_rows * shape[1] * 2 * np.dtype(np.float64).itemsize  # mgrid east/north
    # Final canvas + one band of accumulators/source/temps + the mgrid.
    grid_bytes = int(np.prod(shape)) * 4 + int(band_bytes * 1.5)
    reservation = reserve(grid_bytes, 'mapscene.terrain_uv_raster')
    try:
        for row_start in range(0, int(shape[0]), band_rows):
            row_stop = min(int(shape[0]), row_start + band_rows)
            accum = np.zeros((row_stop - row_start, int(shape[1]), 4), dtype=np.float32)
            for layer in layers:
                try:
                    # Read dimensions without decoding; account for the source
                    # pixels as well as the destination grid before a large decode.
                    path = Path(layer.path)
                    if path.suffix.lower() == '.png':
                        # PNG's signature and IHDR dimensions occupy 24 bytes;
                        # use the format header without adding a Pillow dependency.
                        with path.open('rb') as header_file:
                            header = header_file.read(24)
                        if len(header) != 24 or header[:8] != b'\x89PNG\r\n\x1a\n' or header[12:16] != b'IHDR':
                            raise ValueError('invalid PNG header')
                        width, height = struct.unpack('>II', header[16:24])
                        if not width or not height:
                            raise ValueError('PNG dimensions must be positive')
                        source_bytes = width * height * 4 * (
                            2 * np.dtype(np.uint16).itemsize + 2 * np.dtype(np.uint8).itemsize) + height
                    else:
                        from .alignment import _require_rasterio
                        with _require_rasterio().open(path) as header:
                            source_bytes = header.width * header.height * (
                                sum(np.dtype(dtype).itemsize for dtype in header.dtypes) +
                                4 * header.count * np.dtype(np.float32).itemsize + 2 * 4)
                    source_reservation = reserve(int(source_bytes), 'mapscene.terrain_uv_raster.source')
                    try:
                        source = _overlay_in_terrain_uv(
                            layer, recipe, shape, row_range=(row_start, row_stop)
                        ).astype(np.float32) / 255.0
                    finally:
                        source_reservation.close()
                    opacity = float(layer.opacity)
                    if not np.isfinite(opacity):
                        raise ValueError('raster opacity must be finite')
                    alpha = source[..., 3:4] * np.clip(opacity, 0.0, 1.0)
                    accum[..., :3] = source[..., :3] * alpha + accum[..., :3] * (1.0 - alpha)
                    accum[..., 3:4] = alpha + accum[..., 3:4] * (1.0 - alpha)
                except Exception as exc:
                    raise MapSceneNativeUnavailable([diagnostic_block(
                        layer=str(layer.layer_id), reason=f'RasterOverlay cannot be aligned to terrain UV: {exc}',
                        required_native='readable, aligned RasterOverlay source')]) from exc
            np.divide(accum[..., :3], accum[..., 3:4], out=accum[..., :3], where=accum[..., 3:4] > 0.0)
            output[row_start:row_stop] = np.clip(np.rint(accum * 255.0), 0, 255).astype(np.uint8)
        rgba = output
        with tempfile.TemporaryDirectory(prefix='forge3d-terrain-drape-') as directory:
            path = Path(directory) / 'albedo.png'
            save_png_deterministic(path, rgba)
            yield str(path), len(layers)
    finally:
        reservation.close()
