"""Exercise the studio bridge's SVG export op without a window.

The bridge dispatcher is Rust-embedded Python, so this extracts it verbatim
from the source, executes it, and calls the op directly. The Pro gate on
`export_svg` is monkeypatched because this machine has no licence; the point of
the exercise is the geometry, bounds and paint the op hands to the engine.
"""

import json
import os
import re
import sys
import tempfile

import forge3d
import forge3d.export as f3d_export

SOURCE = open(
    os.path.join(
        os.path.dirname(os.path.abspath(__file__)),
        "..",
        "crates",
        "studio-engine",
        "src",
        "lib.rs",
    ),
    encoding="utf-8",
).read()
match = re.search(r'const BRIDGE_SOURCE: &str = r##"(.*?)"##;', SOURCE, re.S)
assert match, "could not find BRIDGE_SOURCE"
namespace = {"__name__": "_bridge", "_studio_host_python": None}
exec(compile(match.group(1), "<bridge>", "exec"), namespace)
print("ops:", len(namespace["_OPS"]))

# The licence gate would stop the export here; the geometry is what we test.
f3d_export._check_pro_access = lambda *a, **k: None

recipe = {
    "kind": "scene_recipe",
    "terrain": {"kind": "terrain"},
    "camera": {"kind": "camera"},
    "lighting": {"kind": "lighting", "preset": "daylight"},
    "output": {"kind": "output_spec", "width": 1200, "height": 800},
    "layers": [
        {
            "kind": "raster_overlay",
            "layer_id": "ortho",
            "opacity": 0.9,
            "path": "a.tif",
        },
        {
            "kind": "vector_overlay",
            "layer_id": "lines",
            "style": {
                "version": 8,
                "layers": [
                    {
                        "id": "lines",
                        "type": "line",
                        "paint": {"line-color": "#ff8800", "line-width": 3.0},
                    }
                ],
            },
            "features": [
                {
                    "id": "line-1",
                    "geometry": {
                        "type": "LineString",
                        "coordinates": [
                            [-121.8, 46.7],
                            [-121.6, 46.86],
                        ],
                    },
                }
            ],
        },
        {
            "kind": "vector_overlay",
            "layer_id": "hidden",
            "studio_visible": False,
            "style": {
                "version": 8,
                "layers": [
                    {"id": "hidden", "type": "line", "paint": {"line-color": "#000000"}}
                ],
            },
            "features": [
                {
                    "id": "line-2",
                    "geometry": {
                        "type": "LineString",
                        "coordinates": [[-121.7, 46.7], [-121.7, 46.9]],
                    },
                }
            ],
        },
        {
            "kind": "label_layer",
            "layer_id": "labels",
            "labels": [
                {
                    "id": "label-1",
                    "text": "Summit",
                    "geometry": {"type": "Point", "coordinates": [-121.7, 46.8, 0.0]},
                }
            ],
        },
    ],
}

out = os.path.join(tempfile.gettempdir(), "studio-export-test.svg")
if os.path.exists(out):
    os.remove(out)
result = namespace["_op_mapscene_export_svg"](
    {"recipe_json": json.dumps(recipe), "out_svg": out}
)
print("result:", json.dumps(result, indent=2, default=str))
text = open(out, encoding="utf-8").read()
print("svg chars:", len(text))
# Labels are glyph outlines (halo + fill), not <text>, so count paths instead.
paths = text.count("<path")
print("has polyline:", "<polyline" in text, "| label paths:", paths)
assert paths == 2 * result["counts"]["labels"], "one halo + one fill path per label"
print("orange:", "#ff8800" in text.lower())
print("hidden layer drawn:", "-121.7" in text and text.count("polyline") > 1)

# Now the no-licence path: the gate must surface, not produce an empty file.
f3d_export._check_pro_access = forge3d._license._check_pro_access
try:
    namespace["_op_mapscene_export_svg"](
        {"recipe_json": json.dumps(recipe), "out_svg": out + ".pro.svg"}
    )
    print("licence path: UNEXPECTED PASS")
except Exception as error:  # noqa: BLE001 - the point is the message
    print("licence path:", type(error).__name__, str(error)[:90])
