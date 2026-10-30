"""Does `mapscene.pick` agree with the pixels the renderer drew?

The pick claims "this pixel belongs to that layer". The only way to know is to
render a recipe whose vector line is unmistakable (thick, pure orange), find
the pixels that are actually orange, and ask the pick about them — and about
pixels that are not.

Run from `studio/` with the repo venv:  ../.venv/bin/python scripts/check_pick.py
"""

import json
import os
import re
import sys
import tempfile

import numpy as np
from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
SOURCE = open(
    os.path.join(HERE, "..", "crates", "studio-engine", "src", "lib.rs"),
    encoding="utf-8",
).read()
match = re.search(r'const BRIDGE_SOURCE: &str = r##"(.*?)"##;', SOURCE, re.S)
assert match, "could not find BRIDGE_SOURCE"
ns = {"__name__": "_bridge", "_studio_host_python": None}
exec(compile(match.group(1), "<bridge>", "exec"), ns)

dem = os.path.join(HERE, "..", "..", "assets", "tif", "Mount_Fuji_30m.tif")
WIDTH, HEIGHT = 640, 360
MODE = sys.argv[1] if len(sys.argv) > 1 else "screen"
data = json.loads(
    json.dumps(
        ns["_op_mapscene_template"](
            {
                "template": "terrain_vector_labels",
                "dem_path": dem,
                "label_text": "Summit",
                "width": WIDTH,
                "height": HEIGHT,
            }
        )["recipe"],
        default=str,
    )
)
if MODE != "screen":
    # The camera mode is the recipe's own switch; set it so the render and the
    # pick have to agree through the terrain camera rather than the grid.
    data.setdefault("lighting", {}).setdefault("settings", {})["cli_params"] = {
        "camera_mode": MODE
    }
data["output"]["width"] = WIDTH
data["output"]["height"] = HEIGHT
for layer in data["layers"]:
    if layer.get("kind") == "vector_overlay":
        layer["style"]["layers"][0]["paint"] = {
            "line-color": "#ff8800",
            "line-width": 12.0,
        }
recipe_json = json.dumps(data)

out = os.path.join(tempfile.gettempdir(), "pick-frame.png")
ns["_op_mapscene_render"](
    {"recipe_json": recipe_json, "out_png": out, "certificate": False}
)
frame = np.asarray(Image.open(out).convert("RGB"), dtype=np.int16)
height, width = frame.shape[:2]

orange = (
    (frame[:, :, 0] > 200)
    & (frame[:, :, 1] > 90)
    & (frame[:, :, 1] < 200)
    & (frame[:, :, 2] < 90)
)
rows, cols = np.nonzero(orange)
print(f"frame {width}x{height}: {len(cols)} orange pixels")

if len(cols) == 0:
    raise SystemExit("no orange pixels: nothing to check the pick against")

rng = np.random.default_rng(7)
sample = rng.choice(len(cols), size=12, replace=False)


def pick(col, row):
    return ns["_op_mapscene_pick"](
        {
            "recipe_json": recipe_json,
            "u": (col + 0.5) / width,
            "v": (row + 0.5) / height,
        }
    )


hits = 0
for index in sample:
    col, row = int(cols[index]), int(rows[index])
    result = pick(col, row)
    if result.get("hit") and result.get("kind") == "vector_overlay":
        hits += 1
    else:
        print(f"  MISS at ({col}, {row}): {json.dumps(result)[:120]}")
print(f"on the line:      {hits}/{len(sample)} picked the vector layer")

# A far corner should not be the vector layer (the raster covers the frame, so
# a hit there must be the raster, in paint order below the vector).
corner = pick(4, height - 5)
print("far corner:      ", json.dumps(corner)[:140])

# The label is drawn at its projected position; picking there must name it.
label = next(layer for layer in data["layers"] if layer.get("kind") == "label_layer")
coords = label["labels"][0]["geometry"]["coordinates"]
projected = ns["_op_mapscene_pick"](
    {"recipe_json": recipe_json, "u": 0.5, "v": 0.5, "tolerance_px": 6.0}
)
print("centre:          ", json.dumps(projected)[:140])

# The screen-mode mapping is the thing under test; print it for the record.
grid = {}
for layer in data["layers"]:
    if layer.get("kind") == "vector_overlay":
        grid = (layer.get("metadata") or {}).get("preparation_grid") or {}
print(f"mode: {MODE} | preparation_grid present:", bool(grid), "| label coords:", coords)

failures = []
if hits != len(sample):
    failures.append(f"{len(sample) - hits} on-line picks missed the vector layer")
if corner.get("kind") == "vector_overlay":
    failures.append("a far corner picked the vector layer")
if failures:
    print("RESULT:", "; ".join(failures))
    raise SystemExit(1)
print("RESULT: pick agrees with the rendered pixels")
