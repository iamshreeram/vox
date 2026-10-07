#!/usr/bin/env python3
"""Generate the Vox 'Soundwave Orb' icon set -- card #3 from the design
gallery, picked as-is (plain gold radial-gradient sphere + two faint
concentric ripple rings, NO rainbow). Mirrors the approach in
~/ram/projects/python/vox/scripts/generate_icons.py: plain PIL primitives,
numpy-vectorized gradient math, no design tool needed, fully reproducible.

Usage:
    uv run --with pillow --with numpy \\
      --index-url https://pypi.ci.artifacts.walmart.com/artifactory/api/pypi/external-pypi/simple \\
      --allow-insecure-host pypi.ci.artifacts.walmart.com \\
      python3 design/generate_icon.py
"""
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw

REPO = Path(__file__).resolve().parent.parent
ICONS_DIR = REPO / "src-tauri" / "icons"
RESOURCES_DIR = REPO / "src-tauri" / "resources"

# Exact stops from gallery card #3's radialGradient #g3.
HIGHLIGHT = np.array([255, 233, 168])  # #ffe9a8
MID = np.array([244, 196, 48])  # #f4c430
EDGE = np.array([185, 130, 10])  # #b9820a


def draw_gold_sphere(size: int) -> Image.Image:
    """Radial-gradient gold sphere, highlight offset top-left (matches g3's
    cx=35% cy=30% r=75%). Vectorized with numpy -- a pure-Python per-pixel
    loop at supersampled app-icon resolution would take minutes."""
    y, x = np.mgrid[0:size, 0:size].astype(float)
    cx, cy = size * 0.35, size * 0.30
    r_max = size * 0.75
    d = np.clip(np.hypot(x - cx, y - cy) / r_max, 0, 1)

    t_inner = np.clip(d / 0.5, 0, 1)[..., None]
    t_outer = np.clip((d - 0.5) / 0.5, 0, 1)[..., None]
    inner_color = HIGHLIGHT + (MID - HIGHLIGHT) * t_inner
    outer_color = MID + (EDGE - MID) * t_outer
    rgb = np.where(d[..., None] < 0.5, inner_color, outer_color)

    dist_center = np.hypot(x - size / 2, y - size / 2)
    alpha = np.where(dist_center <= size * 0.5, 255, 0)

    rgba = np.dstack([rgb, alpha]).astype(np.uint8)
    return Image.fromarray(rgba, mode="RGBA")


def draw_orb(size: int) -> Image.Image:
    """Full glyph: gold sphere (58.6% of canvas, matching gallery's r=150 of
    a 512 canvas) + two faint concentric gold ripple rings outside it."""
    scale = 4
    s = size * scale
    canvas = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    draw = ImageDraw.Draw(canvas)
    cx = cy = s / 2

    # Ripple rings first (behind the sphere), matching gallery's r=190/222
    # of a 256 half-canvas -> ~74%/87% of the half-width, opacity 0.35/0.15.
    for radius_ratio, opacity, width_ratio in (
        (0.74, 0.35, 0.020),
        (0.87, 0.15, 0.016),
    ):
        r = s / 2 * radius_ratio
        draw.ellipse(
            [cx - r, cy - r, cx + r, cy + r],
            outline=(244, 196, 48, round(255 * opacity)),
            width=max(1, round(s * width_ratio)),
        )

    sphere_d = int(s * 0.586)
    sphere = draw_gold_sphere(sphere_d)
    offset = (round(cx - sphere_d / 2), round(cy - sphere_d / 2))
    canvas.alpha_composite(sphere, offset)
    return canvas.resize((size, size), Image.LANCZOS)


def draw_app_icon(size: int = 1024) -> Image.Image:
    """Dock/Finder/installer icon: dark rounded-square background (same
    convention as the Python Vox app icon) with the orb glyph centered."""
    bg = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(bg)
    margin = int(size * 0.06)
    radius = int(size * 0.22)
    draw.rounded_rectangle(
        [margin, margin, size - margin, size - margin],
        radius=radius,
        fill=(21, 22, 26, 255),  # #15161a, matches gallery card #3's bg
    )
    orb = draw_orb(int(size * 0.72))
    offset = ((size - orb.width) // 2, (size - orb.height) // 2)
    bg.alpha_composite(orb, offset)
    return bg


def add_badge(base: Image.Image, kind: str) -> Image.Image:
    """Small corner badge so recording/transcribing/warning stay visually
    distinct at a glance in the menu bar, same as the old per-state icons."""
    img = base.copy()
    draw = ImageDraw.Draw(img)
    s = img.width
    bx, by, br = s * 0.80, s * 0.80, s * 0.15
    if kind == "recording":
        draw.ellipse([bx - br, by - br, bx + br, by + br], fill=(235, 50, 50, 255))
    elif kind == "transcribing":
        draw.ellipse(
            [bx - br, by - br, bx + br, by + br],
            outline=(255, 255, 255, 255),
            width=max(2, int(s * 0.03)),
        )
    elif kind == "warning":
        tri = [
            (bx, by - br),
            (bx - br, by + br * 0.8),
            (bx + br, by + br * 0.8),
        ]
        draw.polygon(tri, fill=(255, 181, 71, 255))
    return img


def main() -> None:
    ICONS_DIR.mkdir(parents=True, exist_ok=True)
    RESOURCES_DIR.mkdir(parents=True, exist_ok=True)

    # 1. App/Dock/installer master -> feed into `bun run tauri icon`.
    app_icon = draw_app_icon(1024)
    app_icon.save(ICONS_DIR / "icon-source-orb.png")

    # 2. Tray glyph -- self-contained (gold against transparent), no
    # light/dark duplication needed, unlike the old monochrome hand
    # silhouette.
    tray_size = 64
    orb = draw_orb(tray_size)
    orb_warning = add_badge(orb, "warning")
    orb_recording = add_badge(orb, "recording")
    orb_transcribing = add_badge(orb, "transcribing")

    pairs = {
        "tray_idle.png": orb,
        "tray_idle_dark.png": orb,
        "tray_idle_warning.png": orb_warning,
        "tray_idle_warning_dark.png": orb_warning,
        "tray_recording.png": orb_recording,
        "tray_recording_dark.png": orb_recording,
        "tray_transcribing.png": orb_transcribing,
        "tray_transcribing_dark.png": orb_transcribing,
        # Linux "Colored" theme fallback set (see tray.rs get_icon_path).
        "handy.png": orb,
        "handy_warning.png": orb_warning,
        "recording.png": orb_recording,
        "transcribing.png": orb_transcribing,
    }
    for name, image in pairs.items():
        image.save(RESOURCES_DIR / name)

    print(f"Wrote icon-source-orb.png to {ICONS_DIR}")
    print(f"Wrote {len(pairs)} tray/resource PNGs to {RESOURCES_DIR}")


if __name__ == "__main__":
    main()
