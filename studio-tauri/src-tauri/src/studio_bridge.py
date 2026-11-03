"""Forge3D Studio ↔ Forge3D viewer bridge.

Spawned by the Studio shell with the engine's Python interpreter. Reads a
one-line JSON config on stdin, opens the viewer through the maintained Python
API, then relays one JSON request per stdin line to the viewer's IPC socket.

This file is embedded in the Rust shell with `include_str!` and written to the
app-data directory at launch, so it can be inspected by the user.

Protocol (newline-delimited JSON):

  stdin  line 1   {"width":1280,"height":720,"fovDeg":45,"gltfPath":null}
  stdin  line n   {"id":1,"cmd":"call","payload":{"cmd":"cam_lookat", ...}}
                  {"id":2,"cmd":"close"}
  stdout          {"event":"ready"}
                  {"event":"error","message":"..."}
                  {"id":1,"ok":true,"result":{...}}
"""

from __future__ import annotations

import json
import os
import sys
import sysconfig
import traceback
from typing import Any, Dict, Optional


def emit(payload: Dict[str, Any]) -> None:
    print(json.dumps(payload), flush=True)


def fail(message: str) -> None:
    emit({"event": "error", "message": message})


def ensure_native_runtime() -> None:
    """Make the interpreter's libpython loadable by the viewer binary.

    The viewer is linked against the Python that built it. uv/pyenv-style
    interpreters keep `libpython3.x.so` outside the virtualenv, so the dynamic
    loader needs `LIBDIR` on `LD_LIBRARY_PATH` — without this the viewer exits
    immediately with "cannot open shared object file".
    """
    if not sys.platform.startswith("linux"):
        return
    libdir = sysconfig.get_config_var("LIBDIR")
    if not libdir or not os.path.isdir(libdir):
        return
    existing = [part for part in os.environ.get("LD_LIBRARY_PATH", "").split(os.pathsep) if part]
    if libdir not in existing:
        os.environ["LD_LIBRARY_PATH"] = os.pathsep.join([libdir, *existing])


def main() -> int:
    ensure_native_runtime()
    try:
        config_line = sys.stdin.readline()
        if not config_line.strip():
            fail("the Studio shell sent no viewer configuration")
            return 2
        config = json.loads(config_line)
    except Exception as exc:  # noqa: BLE001 - report, never traceback to the user
        fail(f"could not read the viewer configuration: {exc}")
        return 2

    try:
        import forge3d  # noqa: F401 - proves the package imports
        from forge3d.viewer import open_viewer_async
    except Exception as exc:  # noqa: BLE001
        fail(
            "the forge3d Python package is not importable with this interpreter: "
            f"{exc}"
        )
        return 3

    gltf_path: Optional[str] = config.get("gltfPath") or None
    terrain_path: Optional[str] = config.get("terrainPath") or None
    try:
        if terrain_path:
            handle = open_viewer_async(
                width=int(config.get("width", 1280)),
                height=int(config.get("height", 720)),
                fov_deg=float(config.get("fovDeg", 60.0)),
                terrain_path=terrain_path,
                timeout=float(config.get("timeout", 45.0)),
            )
        else:
            handle = open_viewer_async(
                width=int(config.get("width", 1280)),
                height=int(config.get("height", 720)),
                fov_deg=float(config.get("fovDeg", 60.0)),
                gltf_path=gltf_path,
                timeout=float(config.get("timeout", 45.0)),
            )
    except Exception as exc:  # noqa: BLE001
        fail(f"the Forge3D viewer did not start: {exc}")
        return 4

    emit({"event": "ready", "gltf": gltf_path, "terrain": terrain_path})

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
        command = request.get("cmd")

        try:
            if command == "call":
                result = handle.send_ipc(request.get("payload") or {})
                emit({"id": request_id, "ok": True, "result": result})
            elif command == "close":
                handle.close()
                emit({"id": request_id, "ok": True})
                return 0
            else:
                emit({"id": request_id, "ok": False, "error": f"unknown command {command!r}"})
        except Exception as exc:  # noqa: BLE001
            emit({"id": request_id, "ok": False, "error": str(exc)})

    try:
        handle.close()
    except Exception:  # noqa: BLE001
        traceback.print_exc()
    return 0


if __name__ == "__main__":
    sys.exit(main())
