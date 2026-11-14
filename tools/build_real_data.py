"""Builds the "Real data" set of the app (data/real/) from the Lake Koronia GeoTIFFs.

Inputs (kept in the repo):
  Koroneia_NISAR/koroneia_nisar_<date>.tif           NISAR L2 GCOV, HHHH covariance term (gamma0 power), 20 m, UTM 34N/35N
  Koroneia_Sentinel/koroneia_optical_<date>_B04.tif   HLS S30 (Sentinel-2) red, reflectance x 10000, 30 m
  Koroneia_Sentinel/koroneia_optical_<date>_B08.tif   HLS S30 (Sentinel-2) near infrared

Outputs (the demo set in data/ is not touched):
  data/real/manifest.json      layers hh + change, one frame per NISAR pass (empty passes kept as "no data" frames)
  data/real/regions.json       lake area series (Sentinel-2 until the first NISAR pass, then NISAR) for the Status view
  data/real/points.geojson     empty: no point time series yet (needs GUNW)
  data/real/layers/ values/ optical/

Method (same rules as tools/add_series.py in the original data repo):
  * Optical water: (B04 - B08) / (B04 + B08) > 0.1 and B08 < 1000. Optical dates are used up to the first NISAR pass;
    a date more than 20% below the previous accepted one is dropped (turbid water / cloud).
  * Radar water: HH <= -18 dB, counted inside the fullest optical lake. An orbit is used only if its first pass sees
    at least half of the lake as water (on steep orbits calm water is not dark enough). A pass more than 40% below both
    neighbouring passes is dropped (wind-roughened water).
  * Change = HH minus the first pass of the same orbit (12-day repeat), so different viewing geometries are not
    mistaken for change on the ground.

    python tools/build_real_data.py

Needs: numpy, pillow, rasterio, scipy.
"""

from __future__ import annotations

import copy
import json
import math
import re
from datetime import date
from pathlib import Path

import numpy as np
import rasterio
from PIL import Image
from rasterio.transform import from_bounds
from rasterio.warp import Resampling, reproject
from scipy.ndimage import binary_dilation, gaussian_filter, label, median_filter

from colormap import colorize, save_image

ROOT = Path(__file__).resolve().parents[1]
NISAR_DIR = ROOT / "Koroneia_NISAR"
S2_DIR = ROOT / "Koroneia_Sentinel"
OUT = ROOT / "data" / "real"
DEMO_MANIFEST = ROOT / "data" / "manifest.json"
DEMO_REGIONS = ROOT / "data" / "regions.json"

BOUNDS = [23.03, 40.62, 23.27, 40.74]           # west, south, east, north (EPSG:4326)
RES = 0.0002
W = round((BOUNDS[2] - BOUNDS[0]) / RES)         # 1200
H = round((BOUNDS[3] - BOUNDS[1]) / RES)         # 600
TRANSFORM = from_bounds(*BOUNDS, W, H)
KM_LON = 111.13 * math.cos(math.radians(40.68))
KM_LAT = 110.57
PX_KM2 = (RES * KM_LON) * (RES * KM_LAT)
GRID = 4                                         # hover grid = image / 4

SAR_WATER_DB = -18.0
NDWI_MIN, NIR_MAX = 0.1, 1000
OPTICAL_DROP = 0.8
WIND_DIP = 0.6
ORBIT_MIN_LAKE = 0.5
LIMIT_SHARE = 0.25                               # INDICATIVE limit: 25% of the fullest lake (replace with the plan's value)

ZONES = [
    {"id": "koroneia_core", "name": {"el": "Λίμνη Κορώνεια", "en": "Lake Koronia"},
     "center": [23.145, 40.682], "radius_km": 4.0},
    {"id": "koroneia_shore", "name": {"el": "Δυτική όχθη & καλαμιώνας", "en": "West shore & reeds"},
     "center": [23.11, 40.66], "radius_km": 2.5},
]

LON = BOUNDS[0] + (np.arange(W) + 0.5) * RES
LAT = BOUNDS[3] - (np.arange(H) + 0.5) * RES


def to_grid(path: Path) -> np.ndarray | None:
    """Band 1 on the common grid (bilinear), NaN = no data. None if the file is empty."""
    if path.stat().st_size == 0:
        return None
    with rasterio.open(path) as ds:
        src = ds.read(1).astype("float32")
        if ds.nodata is not None and not np.isnan(ds.nodata):
            src[src == ds.nodata] = np.nan
        if not np.isfinite(src).any():
            return None
        dst = np.full((H, W), np.nan, dtype="float32")
        reproject(src, dst, src_transform=ds.transform, src_crs=ds.crs, src_nodata=np.nan,
                  dst_transform=TRANSFORM, dst_crs="EPSG:4326", dst_nodata=np.nan, resampling=Resampling.bilinear)
        return dst


def circle(center, radius_km) -> np.ndarray:
    dx = (LON[None, :] - center[0]) * KM_LON
    dy = (LAT[:, None] - center[1]) * KM_LAT
    return dx * dx + dy * dy <= radius_km ** 2


def day_of(path: Path) -> str:
    return re.search(r"\d{4}-\d{2}-\d{2}", path.name).group(0)


def nearest(dates: list[str], d: str) -> int:
    return min(range(len(dates)), key=lambda i: abs((date.fromisoformat(dates[i]) - date.fromisoformat(d)).days))


def edge_alpha(valid: np.ndarray) -> np.ndarray:
    """Soft edge along the border of the radar footprint."""
    return np.clip(gaussian_filter(valid.astype(float), sigma=3.0) * 1.6 - 0.3, 0.0, 1.0)


def save_values(path: Path, vals: np.ndarray) -> list[int]:
    """Block-averaged values as little-endian int16 (value x 10, -32768 = no data). Used by the map hover."""
    h, w = vals.shape[0] // GRID, vals.shape[1] // GRID
    blk = vals[:h * GRID, :w * GRID].reshape(h, GRID, w, GRID)
    cnt = np.isfinite(blk).sum(axis=(1, 3))
    tot = np.nansum(blk, axis=(1, 3))
    mean = np.where(cnt > 0, tot / np.maximum(cnt, 1), np.nan)
    out = np.where(np.isfinite(mean), np.clip(np.round(mean * 10), -32767, 32767), -32768).astype("<i2")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(out.tobytes())
    return [w, h]


def optical_image(red: np.ndarray, nir: np.ndarray) -> np.ndarray:
    """Pseudo-natural colour from red + NIR only (no green/blue band in the files): vegetation green, water dark."""
    r = np.clip(red / 2200.0, 0, 1) ** 0.75
    n = np.clip(nir / 4200.0, 0, 1) ** 0.75
    rgb = np.stack([r, 0.55 * r + 0.45 * n, 0.85 * r + 0.08 * n], axis=-1)
    valid = np.isfinite(red) & np.isfinite(nir)
    rgba = np.zeros((H, W, 4), np.uint8)
    rgba[..., :3] = (np.nan_to_num(rgb) * 255).astype(np.uint8)
    rgba[..., 3] = (edge_alpha(valid) * 255).astype(np.uint8)
    return rgba


def outline(mask: np.ndarray) -> list[list[float]]:
    """Lake edge as a 72-point polygon: per direction, the farthest pixel of the largest water body."""
    lab, n = label(mask)
    if not n:
        return []
    big = lab == (np.bincount(lab.ravel())[1:].argmax() + 1)
    ys, xs = np.nonzero(big)
    cx, cy = LON[xs].mean(), LAT[ys].mean()
    px, py = (LON[xs] - cx) * KM_LON, (LAT[ys] - cy) * KM_LAT
    ang, dist = np.arctan2(py, px), np.hypot(px, py)
    ring = []
    for i in range(72):
        a = -math.pi + i * 2 * math.pi / 72
        sel = np.abs(np.angle(np.exp(1j * (ang - a)))) < math.pi / 72
        if not sel.any():
            continue
        k = np.argmax(np.where(sel, dist, -1))
        ring.append([round(float(LON[xs[k]]), 5), round(float(LAT[ys[k]]), 5)])
    return ring + ring[:1]


def main() -> None:
    for sub in ("layers", "values", "optical"):
        (OUT / sub).mkdir(parents=True, exist_ok=True)
        for old in (OUT / sub).glob("*"):
            old.unlink()
    demo = json.loads(DEMO_MANIFEST.read_text(encoding="utf-8"))
    demo_layers = {l["id"]: l for l in demo["layers"]}
    masks = {z["id"]: circle(z["center"], z["radius_km"]) for z in ZONES}

    # ── Sentinel-2 (HLS): optical water + the images of the "Optical" view
    s2, optical = [], []
    for red_path in sorted(S2_DIR.glob("koroneia_optical_*_B04.tif")):
        d = day_of(red_path)
        nir_path = red_path.with_name(red_path.name.replace("_B04", "_B08"))
        if not nir_path.exists():
            print(f"S2    {d}  skipped: no B08")
            continue
        red, nir = to_grid(red_path), to_grid(nir_path)
        if red is None or nir is None:
            print(f"S2    {d}  skipped: empty band")
            continue
        save_image(OUT / "optical" / f"{d}.webp", optical_image(red, nir))
        optical.append({"date": d, "image": f"data/real/optical/{d}.webp", "source": f"HLS S30 · {red_path.name[:-8]}"})
        with np.errstate(invalid="ignore", divide="ignore"):
            water = ((red - nir) / (red + nir) > NDWI_MIN) & (nir < NIR_MAX)
        s2.append({"date": d, "water_km2": float(water.sum() * PX_KM2), "mask": water})
    first_sar = min(day_of(p) for p in NISAR_DIR.glob("koroneia_nisar_*.tif"))
    last_ok = None
    for p in s2:
        p["used"] = p["date"] < first_sar and (last_ok is None or p["water_km2"] >= OPTICAL_DROP * last_ok)
        if p["used"]:
            last_ok = p["water_km2"]
        print(f"S2    {p['date']}  water {p['water_km2']:6.2f} km2  {'used' if p['used'] else 'not used'}")
    lake = np.any([p["mask"] for p in s2 if p["used"]], axis=0)
    lake_km2 = float(lake.sum() * PX_KM2)

    # ── NISAR: one frame per pass, empty passes kept as "no data"
    hh_ly = copy.deepcopy(demo_layers["hh"])
    ch_ly = copy.deepcopy(demo_layers["change"])
    ch_ly["description"] = {
        "el": "Διαφορά HH από την πρώτη λήψη της ίδιας τροχιάς. Μπλε = πιο σκοτεινό (συνήθως νερό). Πορτοκαλί = πιο φωτεινό (τραχύτητα, υγρασία, βλάστηση).",
        "en": "HH difference from the first pass of the same orbit. Blue = darker (usually water). Orange = brighter (roughness, moisture, vegetation).",
    }
    for ly in (hh_ly, ch_ly):
        ly["bounds"], ly["frames"] = BOUNDS, []
        ly["grid"] = [W // GRID, H // GRID]
    sar, first_of_orbit = [], {}
    for path in sorted(NISAR_DIR.glob("koroneia_nisar_*.tif")):
        d = day_of(path)
        orbit = date.fromisoformat(d).toordinal() % 12
        src = f"NISAR L2 GCOV HHHH · {path.name}"
        power = to_grid(path)
        if power is None:
            print(f"NISAR {d}  empty file: kept as a 'no data' frame")
            for ly in (hh_ly, ch_ly):
                ly["frames"].append({"date": d, "empty": True, "source": src, "url": ""})
            continue
        valid = np.isfinite(power)
        db = 10.0 * np.log10(np.maximum(np.where(valid, power, 1.0), 1e-6))
        db = np.where(valid, median_filter(np.where(valid, db, np.median(db[valid])), size=3), np.nan)
        base = first_of_orbit.setdefault(orbit, db)
        change = db - base
        alpha = edge_alpha(valid)
        for lid, vals, ly in (("hh", db, hh_ly), ("change", change, ch_ly)):
            save_image(OUT / "layers" / f"{lid}_{d}.webp", colorize(vals, ly["legend"], alpha))
            save_values(OUT / "values" / f"{lid}_{d}.bin", vals)
            ly["frames"].append({"date": d, "image": f"data/real/layers/{lid}_{d}.webp",
                                 "values": f"data/real/values/{lid}_{d}.bin", "source": src, "url": ""})
        with np.errstate(invalid="ignore"):
            water = (db <= SAR_WATER_DB) & lake
        sar.append({"date": d, "orbit": orbit, "water_km2": float(water.sum() * PX_KM2), "mask": water,
                    "hh_db": {k: float(np.nanmean(db[m & valid])) for k, m in masks.items()}})

    good = {o for o in {p["orbit"] for p in sar}
            if next(p for p in sar if p["orbit"] == o)["water_km2"] >= ORBIT_MIN_LAKE * lake_km2}
    for p in sar:
        p["used"], p["why"] = p["orbit"] in good, "steep orbit: calm water not dark enough"
    kept = [p for p in sar if p["used"]]
    for prev, p, nxt in zip(kept, kept[1:], kept[2:]):
        if p["water_km2"] < WIND_DIP * min(prev["water_km2"], nxt["water_km2"]):
            p["used"], p["why"] = False, "far below the passes before and after: wind-roughened water"
    for p in sar:
        print(f"NISAR {p['date']}  orbit {p['orbit']:2d}  lake water {p['water_km2']:6.2f} km2  "
              f"{'used' if p['used'] else 'not used (' + p['why'] + ')'}")

    # ── regions: lake area series for the Status view (no displacement: that needs GUNW)
    rows = [{**p, "sensor": "Sentinel-2"} for p in s2 if p["used"]] + [{**p, "sensor": "NISAR"} for p in sar if p["used"]]
    dates = [p["date"] for p in rows]
    r2 = lambda v: round(v, 2)
    regions = []
    for z in ZONES:
        core = z["id"] == "koroneia_core"
        water = [r2(p["water_km2"] if core else float((p["mask"] & lake & masks[z["id"]]).sum() * PX_KM2)) for p in rows]
        region = {**z, "points": [], "series": {
            "dates": dates,
            "hh_db": [r2(p["hh_db"][z["id"]]) if p["sensor"] == "NISAR" else None for p in rows],
            "water_km2": water,
            "water_source": [p["sensor"] for p in rows],
        }}
        if core:
            cap = max(water)
            region["reservoir"] = {
                "name": {"el": "Επιφάνεια Λίμνης Κορώνειας", "en": "Lake Koronia surface"},
                "unit": "km²", "capacity": r2(cap), "limit": r2(LIMIT_SHARE * cap),
                "dates": dates, "volume": water, "area_km2": water,
                "outline": outline(rows[-1]["mask"]),
                "method": {
                    "el": f"Επιφάνεια νερού μετρημένη από δορυφόρο: Sentinel-2 (HLS) έως τον Μάιο, NISAR HH ≤ {SAR_WATER_DB:g} dB από τον Ιούνιο. Το όριο ({LIMIT_SHARE:.0%} της μέγιστης επιφάνειας) είναι ενδεικτικό.",
                    "en": f"Water surface measured from satellite: Sentinel-2 (HLS) until May, NISAR HH ≤ {SAR_WATER_DB:g} dB from June. The limit ({LIMIT_SHARE:.0%} of the largest surface) is indicative.",
                },
                "rule": {
                    "el": "Ενδεικτικό όριο. Να αντικατασταθεί με την ελάχιστη στάθμη/έκταση του Σχεδίου Διαχείρισης Λεκανών Απορροής Κεντρικής Μακεδονίας (EL10) και του σχεδίου αποκατάστασης της Κορώνειας.",
                    "en": "Indicative limit. Replace with the minimum level/extent in the Central Macedonia River Basin Management Plan (EL10) and the Koronia restoration plan.",
                },
            }
        regions.append(region)
    thresholds = json.loads(DEMO_REGIONS.read_text(encoding="utf-8"))["thresholds"]
    (OUT / "regions.json").write_text(json.dumps({"thresholds": thresholds, "regions": regions}, ensure_ascii=False, indent=1), encoding="utf-8")
    (OUT / "points.geojson").write_text(json.dumps({"type": "FeatureCollection", "features": []}), encoding="utf-8")

    # ── manifest
    manifest = {
        "demo": False,
        "source": "real",
        "site": {**demo["site"], "bounds": BOUNDS, "center": [23.15, 40.68], "zoom": 11.1},
        "reference": demo.get("reference"),
        "layers": [hh_ly, ch_ly],
        "optical": {"kind": "image", "bounds": BOUNDS, "clouds": optical},
        "points": "data/real/points.geojson",
        "regions": "data/real/regions.json",
        "weather": {"provider": "open-meteo", "snapshot": "data/real/weather.json"},
        "content": demo["content"],
        "missing": [{"id": i, "name": demo_layers[i]["name"]} for i in ("hv", "disp", "s1vv", "sm") if i in demo_layers],
    }
    (OUT / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=1), encoding="utf-8")

    print(f"\n{len(hh_ly['frames'])} NISAR frames ({len(sar)} with data) · {len(optical)} optical images · lake at its fullest {lake_km2:.2f} km2")
    print("lake area: " + "  ".join(f"{p['date'][5:]} {p['water_km2']:.1f}" for p in rows))


if __name__ == "__main__":
    main()
