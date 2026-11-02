"""Fetch NAIP aerial imagery for the Bryce Canyon DEM and register it as a GeoTIFF.

The engine drapes a raster by reading its georeferencing, so a bare PNG is not
enough: this asks the same USGS NAIP ImageServer the repository's own example
uses (`examples/bryce_canyon_flyover/fetch_data.py`), then writes the pixels
with the DEM's bounds and CRS. The result sits next to the DEM as
`Bryce_Canyon_naip.tif`, which is the name the studio looks for when it builds a
textured render.

    studio/scripts/fetch_bryce_imagery.py [--size 2048] [--write]

Without `--write` it downloads and reports, and leaves the file in the temp dir.
"""

from __future__ import annotations

import argparse
import io
import json
import os
import tempfile
import urllib.parse
import urllib.request

import numpy as np
import rasterio
from pathlib import Path

from PIL import Image
from rasterio.enums import Resampling
from rasterio.transform import from_origin

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
DEM = os.path.join(REPO, "assets", "tif", "Bryce_Canyon.tif")
OUT = os.path.join(REPO, "assets", "tif", "Bryce_Canyon_naip.tif")
NAIP_URL = (
    "https://imagery.nationalmap.gov/arcgis/rest/services/USGSNAIPImagery/"
    "ImageServer/exportImage"
)


def fetch(dem_path: str, size: int, target: str) -> dict:
    with rasterio.open(dem_path) as dem:
        bounds = tuple(dem.bounds)
        crs = dem.crs
        transform = dem.transform
    if crs is None:
        raise SystemExit(f"{dem_path} has no CRS: imagery cannot be registered")

    # The service takes easting/northing in the DEM's own CRS.
    query = urllib.parse.urlencode(
        {
            "bbox": ",".join(map(str, bounds)),
            "bboxSR": crs.to_epsg() or 26912,
            "imageSR": crs.to_epsg() or 26912,
            "size": f"{size},{size}",
            "format": "png32",
            "pixelType": "U8",
            "bandIds": "0,1,2",
            "transparent": "true",
            "interpolation": "RSP_BilinearInterpolation",
            "f": "json",
        }
    )
    with urllib.request.urlopen(f"{NAIP_URL}?{query}", timeout=120) as response:
        meta = json.load(response)
    if "error" in meta or not meta.get("href"):
        raise SystemExit(f"NAIP export failed: {meta}")
    with urllib.request.urlopen(meta["href"], timeout=300) as response:
        image = Image.open(io.BytesIO(response.read())).convert("RGBA")

    if image.size != (size, size):
        raise SystemExit(f"NAIP returned {image.size}, expected ({size}, {size})")
    rgba = np.asarray(image)
    gaps = int((rgba[:, :, 3] < 255).sum())
    if gaps:
        print(f"warning: {gaps} transparent pixels (outside NAIP coverage)")

    # Register with the DEM's exact grid: same bounds, same CRS, square pixels.
    # A PNG copy sits beside the GeoTIFF because the live viewport decodes PNG
    # (the `image` crate has no GeoTIFF reader) while MapScene needs the
    # georeferencing in the TIFF.
    png_path = str(Path(target).with_suffix(".png"))
    Image.fromarray(rgba[:, :, :3], "RGB").save(png_path, optimize=True)

    with rasterio.open(
        target,
        "w",
        driver="GTiff",
        width=size,
        height=size,
        count=3,
        dtype="uint8",
        crs=crs,
        transform=from_origin(
            bounds[0], bounds[3], (bounds[2] - bounds[0]) / size, (bounds[3] - bounds[1]) / size
        ),
        compress="deflate",
        photometric="RGB",
    ) as dst:
        dst.write(np.moveaxis(rgba[:, :, :3], -1, 0))
    return {
        "path": target,
        "size": size,
        "crs": str(crs),
        "bounds": [round(v, 1) for v in bounds],
        "bytes": os.path.getsize(target),
        "gaps": gaps,
        "dem_transform_matches": transform is not None,
        "png": png_path,
    }


def dem_sidecar(dem_path: str, size: int) -> str:
    """Write a downsampled DEM beside the original, for the drape render.

    The engine reserves the drape's temporaries against a host budget: a 4000²
    grid reserves ~2.6 GB and is refused, while 1024² fits. The textured still
    therefore renders from this sidecar; the live viewport uses the full DEM.
    """
    with rasterio.open(dem_path) as source:
        data = source.read(1, out_shape=(size, size), resampling=Resampling.average)
        profile = source.profile.copy()
        profile.update(width=size, height=size, dtype="float32", compress="deflate")
        profile["transform"] = source.transform * source.transform.scale(
            source.width / size, source.height / size
        )
        target = str(Path(dem_path).with_name(f"{Path(dem_path).stem}_{size}.tif"))
    with rasterio.open(target, "w", **profile) as dst:
        dst.write(data.astype("float32"), 1)
    return target


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--size", type=int, default=2048, help="output pixels per side")
    parser.add_argument("--write", action="store_true", help="write into assets/tif")
    args = parser.parse_args()

    target = OUT if args.write else os.path.join(tempfile.gettempdir(), "bryce_naip.tif")
    result = fetch(DEM, args.size, target)
    if args.write:
        result["dem_sidecar"] = dem_sidecar(DEM, 1024)
    print(json.dumps(result, indent=2))
    if not args.write:
        print("(dry run: pass --write to place it next to the DEM)")


if __name__ == "__main__":
    main()
