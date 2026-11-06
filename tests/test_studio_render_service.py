"""Studio render service: sky encoding and colour-space helpers.

`studio_render.py` is the shell's embedded renderer, not an importable package
module, so it is loaded by path. These tests cover the pieces that are pure
maths — the parts a GPU golden cannot see:

* the studio HDR is a *valid* Radiance file (one broken exponent makes the
  engine decode a black environment and a blown-out sky pass), and
* the denoise path converts the display-referred frame into the linear space
  OIDN and its guidance images live in.
"""

from __future__ import annotations

import importlib.util
from pathlib import Path

import numpy as np
import pytest

SERVICE = (
    Path(__file__).resolve().parents[1]
    / "studio-tauri"
    / "src-tauri"
    / "src"
    / "studio_render.py"
)


def load_service():
    spec = importlib.util.spec_from_file_location("studio_render_service", SERVICE)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def decode_radiance_rgbe(data: bytes):
    """Decode an uncompressed `-Y h +X w` Radiance file to linear RGB."""
    marker = data.index(b"\n-Y ")
    header_end = data.index(b"\n", marker + 1)
    resolution = data[marker + 1 : header_end + 1].decode().strip()
    parts = resolution.split()
    height, width = int(parts[1]), int(parts[3])
    pixels = np.frombuffer(data[header_end + 1 :], dtype=np.uint8)
    assert pixels.size == height * width * 4
    rgbe = pixels.reshape(height, width, 4).astype(np.float32)
    exponent = rgbe[..., 3:4]
    return rgbe[..., :3] / 256.0 * np.exp2(exponent - 128.0)


def test_studio_sky_hdr_is_a_valid_radiance_file(tmp_path):
    service = load_service()
    path = tmp_path / "sky.hdr"
    service.write_sky_hdr(path)

    data = path.read_bytes()
    assert data.startswith(b"#?RADIANCE\n")
    assert b"FORMAT=32-bit_rle_rgbe" in data

    rgb = decode_radiance_rgbe(data)
    assert np.isfinite(rgb).all(), "every pixel must decode (biased exponent, not raw frexp)"
    assert rgb.max() > 0.5, "the sky must carry real radiance, not a black environment"

    # Equirectangular, row 0 = zenith: the upper half is the sky (blue at the
    # zenith), the lower half is the ground. An inverted map lights the terrain
    # from below.
    zenith = rgb[0].mean(axis=0)
    horizon = rgb[rgb.shape[0] // 2 - 1].mean(axis=0)
    nadir = rgb[-1].mean(axis=0)
    assert zenith[2] > zenith[0], "zenith must read blue"
    assert horizon[0] > nadir[0] and horizon[1] > nadir[1], "ground must be darker than the horizon"
    assert nadir.mean() < zenith.mean()


def test_srgb_round_trip_is_lossless_enough_for_8_bit():
    service = load_service()
    values = np.linspace(0.0, 1.0, 256, dtype=np.float32)
    linear = service.srgb_to_linear(values)
    back = service.linear_to_srgb(linear)
    assert np.allclose(back, values, atol=1e-6)
    # And the decode is the real transfer function, not a plain gamma.
    assert service.srgb_to_linear(np.array([0.5], dtype=np.float32))[0] == pytest.approx(
        0.21404114, rel=1e-5
    )


def test_denoise_is_applied_in_linear_space():
    """The frame is sRGB; OIDN and its albedo/normal guidance are linear."""
    service = load_service()
    source = SERVICE.read_text()
    assert "srgb_to_linear(rgb.astype(np.float32) / 255.0)" in source, (
        "the still must be decoded to linear before OIDN and re-encoded after"
    )
    assert "linear_to_srgb(denoised)" in source


def test_still_uses_the_physical_shading_model():
    """Solid/Material previews are three.js PBR; the still must match them."""
    service = load_service()
    source = SERVICE.read_text()
    assert 'terrain_shading_model="lambert_physical"' in source
