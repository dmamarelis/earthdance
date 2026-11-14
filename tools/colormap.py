"""Shared helpers: turn a 2D array of physical values into a coloured, transparent image.

The colour ramp always comes from the layer's `legend` in data/manifest.json, so the map
overlay and the legend shown in the app can never disagree.
"""

from __future__ import annotations

import numpy as np
from PIL import Image


def _hex_to_rgb(h: str) -> tuple[int, int, int]:
    h = h.lstrip("#")
    return int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16)


def colorize(values: np.ndarray, legend: dict, alpha: np.ndarray | None = None) -> np.ndarray:
    """values: 2D float array (NaN = no data). legend: {"stops": [[value, "#hex"], ...]}.

    Returns an (H, W, 4) uint8 RGBA array. `alpha` (0..1) is optional per-pixel opacity.
    """
    stops = sorted(legend["stops"], key=lambda s: s[0])
    xs = np.array([s[0] for s in stops], dtype=float)
    rgb = np.array([_hex_to_rgb(s[1]) for s in stops], dtype=float)
    nodata = ~np.isfinite(values)
    v = np.where(nodata, xs[0], values)
    out = np.empty(values.shape + (4,), dtype=np.uint8)
    for c in range(3):
        out[..., c] = np.interp(v, xs, rgb[:, c]).astype(np.uint8)
    a = np.ones(values.shape) if alpha is None else np.clip(alpha, 0.0, 1.0)
    a = np.where(nodata, 0.0, a)
    out[..., 3] = (a * 255).astype(np.uint8)
    return out


def save_image(path, rgba: np.ndarray) -> None:
    """Saves RGBA as WebP (small files, transparency kept). Use a .png path for lossless PNG."""
    img = Image.fromarray(rgba, mode="RGBA")
    if str(path).lower().endswith(".webp"):
        img.save(path, "WEBP", quality=82, method=4)
    else:
        img.save(path, optimize=True)
