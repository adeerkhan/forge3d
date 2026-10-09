#!/usr/bin/env python3
"""Render the volumetric-cloud presets to 2K stills and a camera-orbit video.

This drives ``examples/terrain_volumetric_clouds_hdri.py`` in-process (no code
duplication): it renders one still per preset for the hero PNGs, and renders a
genuinely moving camera-orbit animation (real cloud/terrain parallax — the
offline cloud path pins ``time_seconds = 0``, so motion comes from the camera)
and encodes it to MP4 with ffmpeg.

Everything here needs the built native ``forge3d`` extension and a GPU. If
ffmpeg is not on PATH, the driver leaves the PNG frames on disk and prints the
ffmpeg command instead of failing.

Outputs (under ``--output-dir``, default ``out/clouds``)::

    <preset>.png                     # the four 2K hero stills
    video_frames/frame_0000.png ...  # orbit animation frames
    clouds_orbit_<preset>.mp4        # the video (if ffmpeg present)

Examples::

    # The four 2K stills only:
    python tools/render_cloud_showcase.py --stills

    # The four stills AND a ~5 s orbit video of the cumulus preset:
    python tools/render_cloud_showcase.py --stills --video

    # Just a storm orbit at full 2K, 30 frames:
    python tools/render_cloud_showcase.py --video --preset storm \\
        --frames 30 --video-width 2048 --video-height 1152 --video-samples 64
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from pathlib import Path

import importlib.util

REPO_ROOT = Path(__file__).resolve().parents[1]
EXAMPLES_DIR = REPO_ROOT / "examples"
EXAMPLE_MODULE = EXAMPLES_DIR / "terrain_volumetric_clouds_hdri.py"

DEFAULT_PRESETS = ("cumulus", "scattered", "storm", "cirrus")


def _load_example():
    """Import the example module from its file path so we can call main()."""
    python_dir = REPO_ROOT / "python"
    if python_dir.exists():
        sys.path.insert(0, str(python_dir))
    spec = importlib.util.spec_from_file_location("clouds_hdri_example", EXAMPLE_MODULE)
    if spec is None or spec.loader is None:  # pragma: no cover - defensive
        raise SystemExit(f"could not load example module: {EXAMPLE_MODULE}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _lerp(a: float, b: float, t: float) -> float:
    return a + (b - a) * t


def _parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    p.add_argument("--stills", action="store_true",
                   help="render one 2K PNG per preset")
    p.add_argument("--video", action="store_true",
                   help="render a camera-orbit animation and encode it to MP4")
    p.add_argument("--presets", type=str, default=",".join(DEFAULT_PRESETS),
                   help="comma-separated presets for the --stills pass")
    p.add_argument("--preset", type=str, default="cumulus",
                   help="preset used for the --video pass")

    out = p.add_argument_group("output")
    out.add_argument("--output-dir", type=Path, default=REPO_ROOT / "out" / "clouds")
    out.add_argument("--hdr", type=str, default=None, help="override the HDRI path")

    still = p.add_argument_group("stills (the 2K hero images)")
    still.add_argument("--width", type=int, default=2048)
    still.add_argument("--height", type=int, default=1152)
    still.add_argument("--samples", type=int, default=64)
    still.add_argument("--bit-depth", type=int, choices=(8, 16), default=8)

    vid = p.add_argument_group("video (camera orbit)")
    vid.add_argument("--frames", type=int, default=40)
    vid.add_argument("--fps", type=int, default=15)
    vid.add_argument("--orbit-start-deg", type=float, default=118.0,
                     help="start camera azimuth")
    vid.add_argument("--orbit-end-deg", type=float, default=158.0,
                     help="end camera azimuth (a sweep shows cloud parallax)")
    vid.add_argument("--video-width", type=int, default=1280)
    vid.add_argument("--video-height", type=int, default=720)
    vid.add_argument("--video-samples", type=int, default=24,
                     help="lower than stills: a video is many frames")

    args = p.parse_args()
    if not args.stills and not args.video:
        # Default to doing both so a bare run gives you the four images + video.
        args.stills = True
        args.video = True
    if args.frames < 2:
        raise SystemExit("--frames must be >= 2 for an animation")
    return args


def _run_example(example, argv: list[str]) -> None:
    print(f"\n$ render_cloud -> {' '.join(argv)}", flush=True)
    rc = example.main(argv)
    if rc not in (0, None):
        raise SystemExit(f"example returned status {rc}")


def render_stills(example, args, output_dir: Path) -> list[Path]:
    output_dir.mkdir(parents=True, exist_ok=True)
    written: list[Path] = []
    for preset in [name.strip() for name in args.presets.split(",") if name.strip()]:
        argv = [
            "--preset", preset,
            "--output-dir", str(output_dir),
            "--output-name", preset,
            "--width", str(args.width),
            "--height", str(args.height),
            "--samples", str(args.samples),
            "--bit-depth", str(args.bit_depth),
        ]
        if args.hdr is not None:
            argv += ["--hdr", args.hdr]
        _run_example(example, argv)
        written.append(output_dir / f"{preset}.png")
    return written


def _encode_mp4(frames_dir: Path, output_path: Path, fps: int) -> bool:
    ffmpeg = shutil.which("ffmpeg")
    if ffmpeg is None:
        cmd = ["ffmpeg", "-y", "-framerate", str(fps),
               "-i", str(frames_dir / "frame_%04d.png"),
               "-c:v", "libx264", "-preset", "medium", "-crf", "18",
               "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(output_path)]
        print("ffmpeg not found — frames left on disk. Encode with:")
        print("  " + " ".join(str(part) for part in cmd))
        return False
    cmd = [ffmpeg, "-y", "-framerate", str(fps),
           "-i", str(frames_dir / "frame_%04d.png"),
           "-c:v", "libx264", "-preset", "medium", "-crf", "18",
           "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(output_path)]
    result = subprocess.run(cmd, capture_output=True, text=True, check=False)
    if result.returncode != 0:
        raise SystemExit(f"ffmpeg failed:\n{result.stderr[-1200:]}")
    return True


def render_orbit_video(example, args, output_dir: Path) -> Path | None:
    frames_dir = output_dir / "video_frames"
    frames_dir.mkdir(parents=True, exist_ok=True)
    for stale in frames_dir.glob("frame_*.png"):
        stale.unlink()

    for i in range(args.frames):
        t = i / (args.frames - 1)
        phi = _lerp(args.orbit_start_deg, args.orbit_end_deg, t)
        argv = [
            "--preset", args.preset,
            "--output-dir", str(frames_dir),
            "--output-name", f"frame_{i:04d}",
            "--width", str(args.video_width),
            "--height", str(args.video_height),
            "--samples", str(args.video_samples),
            "--cam-phi", f"{phi:.3f}",
        ]
        if args.hdr is not None:
            argv += ["--hdr", args.hdr]
        _run_example(example, argv)
        print(f"  frame {i + 1}/{args.frames} (azimuth {phi:.1f}deg)", flush=True)

    video_path = output_dir / f"clouds_orbit_{args.preset}.mp4"
    if _encode_mp4(frames_dir, video_path, args.fps):
        print(f"\nMP4: {video_path}")
        return video_path
    print(f"\nFrames: {frames_dir}")
    return None


def main() -> int:
    args = _parse_args()
    if not EXAMPLE_MODULE.exists():  # pragma: no cover - defensive
        raise SystemExit(f"missing example: {EXAMPLE_MODULE}")
    output_dir = args.output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)

    example = _load_example()

    if args.stills:
        print(f"[clouds] rendering stills: {args.presets} @ {args.width}x{args.height}, "
              f"samples={args.samples}")
        written = render_stills(example, args, output_dir)
        for path in written:
            print(f"  PNG: {path}")

    if args.video:
        print(f"[clouds] rendering orbit video: {args.preset} @ {args.video_width}x"
              f"{args.video_height}, frames={args.frames}, samples={args.video_samples}")
        print("[clouds] note: this renders one full cloud frame per orbit step, so it "
              "is far slower than a realtime flyover — reduce --frames/--video-samples "
              "for a quick pass.")
        render_orbit_video(example, args, output_dir)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
