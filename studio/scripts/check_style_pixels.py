"""Does the Studio's style patch actually change rendered pixels?

P3's "Style" section writes `line-color` / `line-width` into the recipe's
Mapbox line paint. That is only honest if the engine's renderer reads those
values at render time, so this renders the same recipe twice at 320x200 with
nothing changed but the paint, and compares the images.

Run from `studio/` with the repo's venv:  ../.venv/bin/python scripts/check_style_pixels.py
"""

import hashlib
import json
import os
import re
import sys
import tempfile

import numpy as np

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
recipe = ns["_op_mapscene_template"](
    {
        "template": "terrain_vector_labels",
        "dem_path": dem,
        "label_text": "Mount Fuji",
        "width": 320,
        "height": 200,
    }
)["recipe"]
data = json.loads(json.dumps(recipe, default=str)) if isinstance(recipe, str) else recipe
vector = next(
    layer for layer in data["layers"] if layer.get("kind") == "vector_overlay"
)
print("vector layer style:", json.dumps(vector["style"]))
print("top-level keys:", sorted(data))

data["output"]["width"] = 320
data["output"]["height"] = 200
plain = json.dumps(data)

styled = json.loads(plain)
paint = styled["layers"][[layer.get("kind") for layer in styled["layers"]].index("vector_overlay")][
    "style"
]["layers"][0]["paint"]
print("paint before:", paint)
paint["line-color"] = "#ff8800"
paint["line-width"] = 8.0
print("paint after: ", paint)
styled = json.dumps(styled)

shots = {}
for name, recipe_text in (("plain", plain), ("styled", styled)):
    out = os.path.join(tempfile.gettempdir(), f"style-{name}.png")
    report = ns["_op_mapscene_render"](
        {"recipe_json": recipe_text, "out_png": out, "certificate": False}
    )
    print(
        f"  {name}: status {report['status']} · backend {report['backend']} · "
        f"vector_backend {report['metadata'].get('vector_backend')} · "
        f"diagnostics {report['diagnostics']}"
    )
    raw = open(out, "rb").read()
    shots[name] = raw
    print(f"{name}: {len(raw)} bytes, sha256 {hashlib.sha256(raw).hexdigest()[:16]}")

if shots["plain"] == shots["styled"]:
    print("RESULT: identical — the style patch does NOT reach the renderer")
    sys.exit(1)

from PIL import Image

a = np.asarray(Image.open(os.path.join(tempfile.gettempdir(), "style-plain.png")).convert("RGB"), dtype=np.int16)
b = np.asarray(Image.open(os.path.join(tempfile.gettempdir(), "style-styled.png")).convert("RGB"), dtype=np.int16)
diff = np.abs(a - b)
changed = int((diff.sum(axis=2) > 0).sum())
print(
    f"RESULT: {changed} px differ ({100.0 * changed / diff.shape[0] / diff.shape[1]:.1f}%), "
    f"max channel delta {int(diff.max())}"
)
if changed == 0:
    sys.exit(1)

# The orange line should be present in the styled render and absent in the plain one.
orange = lambda img: int(((img[:, :, 0] > 200) & (img[:, :, 1] > 100) & (img[:, :, 1] < 200) & (img[:, :, 2] < 80)).sum())
print("orange px: plain", orange(a), "styled", orange(b))
