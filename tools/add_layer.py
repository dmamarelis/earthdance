"""Adds ONE real acquisition to the app: raster in -> coloured image + manifest entry out.

Examples
--------
GeoTIFF already in lat/lon (EPSG:4326), e.g. after `gdalwarp -t_srs EPSG:4326 in.tif out.tif`:

    python tools/add_layer.py --layer hh --date 2026-03-14 --input hh_20260314.tif \
        --source NISAR_L2_PR_GCOV_..._20260314T... --url https://search.asf.alaska.edu/...

NumPy array (.npy, 2D, row 0 = north) with explicit bounds west south east north:

    python tools/add_layer.py --layer disp --date 2026-03-14 --input disp.npy \
        --bounds 21.85 39.15 23.05 39.85 --source NISAR_L2_PR_GUNW_...

Notes
-----
* The layer id must already exist in data/manifest.json (name, legend, unit are defined there).
* Values must be in the unit of the legend (dB for backscatter: 10*log10(gamma0); mm for displacement).
* NaN / nodata pixels become transparent.
* The first real frame you add to a layer removes that layer's DEMO frames.
  When no DEMO frames are left anywhere, "demo" is switched to false and the badge disappears.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import numpy as np

from colormap import colorize, save_image

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "data" / "manifest.json"


def read_raster(path: Path, bounds):
    if path.suffix.lower() == ".npy":
        if bounds is None:
            raise SystemExit("--bounds west south east north is required for .npy input")
        return np.load(path).astype(float), list(bounds)
    try:
        import rasterio
    except ImportError:
        raise SystemExit("GeoTIFF input needs rasterio:  pip install rasterio")
    with rasterio.open(path) as ds:
        if ds.crs and ds.crs.to_epsg() != 4326:
            raise SystemExit(f"{path.name} is in {ds.crs}; reproject first: gdalwarp -t_srs EPSG:4326 in.tif out.tif")
        arr = ds.read(1, masked=True).astype(float).filled(np.nan)
        b = ds.bounds
        return arr, [b.left, b.bottom, b.right, b.top]


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--layer", required=True, help="layer id from the manifest, e.g. hh, hv, change, disp, s1vv")
    ap.add_argument("--date", required=True, help="acquisition date YYYY-MM-DD")
    ap.add_argument("--input", required=True, type=Path, help=".tif (EPSG:4326) or .npy")
    ap.add_argument("--bounds", nargs=4, type=float, metavar=("W", "S", "E", "N"))
    ap.add_argument("--source", default="", help="granule / product name shown in the app")
    ap.add_argument("--url", default="", help="link to the granule (ASF, Earthdata)")
    ap.add_argument("--max-size", type=int, default=2000, help="longest image side in pixels")
    ap.add_argument("--opacity", type=float, default=0.95)
    args = ap.parse_args()

    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    layer = next((l for l in manifest["layers"] if l["id"] == args.layer), None)
    if layer is None:
        raise SystemExit(f"unknown layer '{args.layer}'. Known: {[l['id'] for l in manifest['layers']]}")

    values, bounds = read_raster(args.input, args.bounds)
    step = max(1, int(np.ceil(max(values.shape) / args.max_size)))
    values = values[::step, ::step]

    rel = f"data/layers/{args.layer}_{args.date}.webp"
    save_image(ROOT / rel, colorize(values, layer["legend"], np.full(values.shape, args.opacity)))

    frames = [f for f in layer["frames"] if not f.get("source", "").startswith("DEMO_") and f["date"] != args.date]
    frames.append({"date": args.date, "image": rel, "source": args.source, "url": args.url})
    layer["frames"] = sorted(frames, key=lambda f: f["date"])
    layer["bounds"] = bounds
    manifest["demo"] = any(f.get("source", "").startswith("DEMO_") for l in manifest["layers"] for f in l["frames"])

    MANIFEST.write_text(json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")
    finite = values[np.isfinite(values)]
    print(f"added {rel}  {values.shape[1]}x{values.shape[0]} px  "
          f"values {finite.min():.1f}..{finite.max():.1f} {layer['unit']}  frames in layer: {len(layer['frames'])}")


if __name__ == "__main__":
    main()
