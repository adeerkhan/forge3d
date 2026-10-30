"""Generate the gallery thumbnails that actually exist.

The gallery declares 50 examples; 48 had no thumbnail, so the carousel showed
grey boxes. Only tier A can run today, and a thumbnail must not pretend an
example works, so this renders *only* the tier A entries — through the same
bridge ops the app uses — and writes `studio/assets/thumbs/<id>.png` at
320x200. The index is patched to point at those files.

Run from `studio/` with the repo venv:

    ../.venv/bin/python scripts/make_thumbnails.py [--write]
"""

import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
STUDIO = os.path.dirname(HERE)
REPO = os.path.dirname(STUDIO)
THUMBS = os.path.join(STUDIO, "assets", "thumbs")
INDEX = os.path.join(STUDIO, "examples", "index.json")
WIDTH, HEIGHT = 320, 200

SOURCE = open(
    os.path.join(STUDIO, "crates", "studio-engine", "src", "lib.rs"), encoding="utf-8"
).read()
match = re.search(r'const BRIDGE_SOURCE: &str = r##"(.*?)"##;', SOURCE, re.S)
assert match, "could not find BRIDGE_SOURCE"
ns = {"__name__": "_bridge", "_studio_host_python": None}
exec(compile(match.group(1), "<bridge>", "exec"), ns)

index = json.load(open(INDEX, encoding="utf-8"))
tier_a = [entry for entry in index["entries"] if entry.get("tier") == "A"]
print(f"{len(tier_a)} tier A entries out of {len(index['entries'])}")

os.makedirs(THUMBS, exist_ok=True)
written = []

for entry in tier_a:
    scene = entry.get("scene") or {}
    kind = scene.get("kind")
    dem = scene.get("dem")
    dem_path = dem if dem and os.path.isabs(dem) else os.path.join(REPO, dem or "")
    out = os.path.join(THUMBS, f"{entry['id']}.png")
    try:
        if kind == "mapscene":
            raster = scene.get("raster")
            template = ns["_op_mapscene_template"](
                {
                    "template": scene.get("template"),
                    "dem_path": dem_path,
                    "label_text": scene.get("label_text"),
                    "raster_path": os.path.join(REPO, raster) if raster else None,
                    "width": WIDTH,
                    "height": HEIGHT,
                }
            )
            recipe = template["recipe"]
            if isinstance(recipe, str):
                recipe = json.loads(recipe)
            recipe["output"]["width"] = WIDTH
            recipe["output"]["height"] = HEIGHT
            ns["_op_mapscene_render"](
                {
                    "recipe_json": json.dumps(recipe, default=str),
                    "out_png": out,
                    "certificate": False,
                }
            )
        else:
            # Terrain entries: load the DEM, then render one still with the
            # example's own camera.
            loaded = ns["_op_terrain_load"]({"path": dem_path})
            span = float(loaded.get("terrain_span") or 1000.0)
            camera = dict(scene.get("camera") or {})
            camera.setdefault("phi_deg", 135.0)
            camera.setdefault("theta_deg", 45.0)
            camera.setdefault("fov_deg", 54.0)
            camera["radius"] = float(camera.get("radius_rel", 0.4)) * span
            camera["target"] = (0.0, 0.0, 0.0)
            ns["_op_terrain_render"](
                {
                    "out_png": out,
                    "width": WIDTH,
                    "height": HEIGHT,
                    "camera": camera,
                    "sun": scene.get("sun") or {},
                    "params": scene.get("params") or {},
                }
            )
        written.append((entry["id"], os.path.getsize(out)))
        print(f"  {entry['id']}: {os.path.getsize(out) // 1024} KiB")
    except Exception as error:  # noqa: BLE001 - report, do not fake a thumbnail
        print(f"  {entry['id']}: FAILED — {type(error).__name__}: {str(error)[:160]}")

if "--write" in sys.argv:
    for entry in index["entries"]:
        if entry.get("tier") == "A" and any(w[0] == entry["id"] for w in written):
            entry["thumbnail"] = f"studio/assets/thumbs/{entry['id']}.png"
    json.dump(index, open(INDEX, "w", encoding="utf-8"), indent=2, ensure_ascii=False)
    open(INDEX, "a", encoding="utf-8").write("\n")
    print(f"index updated: {len(written)} thumbnail paths")
else:
    print(f"{len(written)} thumbnails written (index not updated; pass --write)")
