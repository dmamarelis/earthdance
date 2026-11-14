"""Generates CLEARLY SYNTHETIC placeholder data so the UI can be built before real NISAR data is ready.

Nothing produced here is a measurement. The manifest it writes has "demo": true, which makes the
app show a permanent "DEMO DATA" badge. Replace these files with real exports (see tools/add_layer.py
and README.md) and set "demo": false.

Run:  python tools/make_demo_data.py
"""

from __future__ import annotations

import datetime as dt
import json
import math
from pathlib import Path

import numpy as np

from colormap import colorize, save_image

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / "data"
LAYERS_DIR = DATA / "layers"

W, H = 640, 420
BOUNDS = [21.85, 39.15, 23.05, 39.85]  # west, south, east, north
DATES = [dt.date(2025, 11, 20) + dt.timedelta(days=36 * k) for k in range(10)]

lon = np.linspace(BOUNDS[0], BOUNDS[2], W)
lat = np.linspace(BOUNDS[3], BOUNDS[1], H)  # row 0 = north
LON, LAT = np.meshgrid(lon, lat)
PX, PY = np.meshgrid(np.arange(W), np.arange(H))

# ---------------------------------------------------------------- layer definitions (go to manifest)
LAYER_DEFS = [
    {
        "id": "hh", "band": "L", "sensor": "NISAR", "product": "L2 GCOV", "unit": "dB",
        "name": {"el": "Οπισθοσκέδαση HH", "en": "Backscatter HH"},
        "short": {"el": "Υφή εδάφους & νερό", "en": "Surface texture & water"},
        "description": {
            "el": "Πόση ενέργεια ραντάρ επιστρέφει στον δορυφόρο. Σκούρο = λείο (νερό). Φωτεινό = τραχύ (πόλεις, βουνά).",
            "en": "How much radar energy returns to the satellite. Dark = smooth (water). Bright = rough (cities, mountains).",
        },
        "legend": {"stops": [[-26, "#05070d"], [-18, "#2a3550"], [-11, "#8a97ad"], [-4, "#eef2f7"]],
                   "low": {"el": "λείο / νερό", "en": "smooth / water"}, "high": {"el": "τραχύ", "en": "rough"}},
    },
    {
        "id": "hv", "band": "L", "sensor": "NISAR", "product": "L2 GCOV", "unit": "dB",
        "name": {"el": "Οπισθοσκέδαση HV", "en": "Backscatter HV"},
        "short": {"el": "Βλάστηση & καλλιέργειες", "en": "Vegetation & crops"},
        "description": {
            "el": "Η διασταυρούμενη πόλωση «ανάβει» όπου υπάρχει όγκος βλάστησης: δάση και ώριμες καλλιέργειες.",
            "en": "Cross-polarisation lights up where there is vegetation volume: forests and mature crops.",
        },
        "legend": {"stops": [[-32, "#04070a"], [-25, "#0f3d3a"], [-19, "#2f9e6e"], [-12, "#e9f26b"]],
                   "low": {"el": "γυμνό / νερό", "en": "bare / water"}, "high": {"el": "πυκνή βλάστηση", "en": "dense vegetation"}},
    },
    {
        "id": "change", "band": "L", "sensor": "NISAR", "product": "L2 GCOV (Δ)", "unit": "dB",
        "name": {"el": "Αλλαγή από την αρχή", "en": "Change since start"},
        "short": {"el": "Τι άλλαξε, πού", "en": "What changed, where"},
        "description": {
            "el": "Διαφορά HH από την πρώτη ημερομηνία. Μπλε = πιο σκούρο (συνήθως νερό/πλημμύρα). Πορτοκαλί = πιο φωτεινό (ανάπτυξη, υγρασία).",
            "en": "HH difference from the first date. Blue = darker (usually water/flooding). Orange = brighter (growth, moisture).",
        },
        "legend": {"stops": [[-8, "#2f6df6"], [-3, "#7fb2ff"], [0, "#e8e6df"], [3, "#ffb454"], [8, "#f2622e"]],
                   "low": {"el": "πιο σκούρο", "en": "darker"}, "high": {"el": "πιο φωτεινό", "en": "brighter"}},
    },
    {
        "id": "disp", "band": "L", "sensor": "NISAR", "product": "L2 GUNW", "unit": "mm",
        "name": {"el": "Μετατόπιση εδάφους", "en": "Ground displacement"},
        "short": {"el": "Πού βυθίζεται η γη", "en": "Where the land sinks"},
        "description": {
            "el": "Σωρευτική κίνηση του εδάφους στη γραμμή όρασης του δορυφόρου. Κόκκινο = το έδαφος απομακρύνεται (βυθίζεται).",
            "en": "Cumulative ground motion along the satellite line of sight. Red = ground moving away (sinking).",
        },
        "legend": {"stops": [[-90, "#7a1663"], [-60, "#e5383b"], [-30, "#f9a03f"], [-8, "#f6e7a9"], [0, "#e9eef2"], [15, "#3d8bfd"]],
                   "low": {"el": "βύθιση", "en": "sinking"}, "high": {"el": "ανύψωση", "en": "uplift"}},
    },
    {
        "id": "s1vv", "band": "C", "sensor": "Sentinel-1", "product": "RTC", "unit": "dB",
        "name": {"el": "Sentinel-1 VV (C-band)", "en": "Sentinel-1 VV (C-band)"},
        "short": {"el": "Το ίδιο τοπίο, μικρότερο μήκος κύματος", "en": "Same landscape, shorter wavelength"},
        "description": {
            "el": "Για σύγκριση: το C-band (5,6 cm) σκεδάζεται στην κορυφή της βλάστησης, ενώ το L-band του NISAR (24 cm) τη διαπερνά.",
            "en": "For comparison: C-band (5.6 cm) scatters at the top of vegetation, while NISAR's L-band (24 cm) penetrates it.",
        },
        "legend": {"stops": [[-24, "#05070d"], [-16, "#3a3350"], [-10, "#a898ad"], [-3, "#f7eef2"]],
                   "low": {"el": "λείο / νερό", "en": "smooth / water"}, "high": {"el": "τραχύ", "en": "rough"}},
    },
]
LEGEND = {d["id"]: d["legend"] for d in LAYER_DEFS}

# name, lon, lat, kind
POINTS = [
    ("stefanovikeio", {"el": "Στεφανοβίκειο", "en": "Stefanovikeio"}, 22.74, 39.46, "subsidence"),
    ("nikaia", {"el": "Νίκαια", "en": "Nikaia"}, 22.47, 39.56, "subsidence"),
    ("farsala", {"el": "Φάρσαλα", "en": "Farsala"}, 22.38, 39.30, "subsidence"),
    ("palamas", {"el": "Παλαμάς", "en": "Palamas"}, 22.08, 39.46, "subsidence"),
    ("karla", {"el": "Λίμνη Κάρλα", "en": "Lake Karla"}, 22.845, 39.49, "water"),
    ("flood", {"el": "Πεδιάδα Πηνειού", "en": "Pineios lowland"}, 22.61, 39.535, "flood"),
    ("kileler", {"el": "Κιλελέρ", "en": "Kileler"}, 22.63, 39.48, "crop"),
    ("platykampos", {"el": "Πλατύκαμπος", "en": "Platykampos"}, 22.53, 39.62, "crop"),
    ("larissa", {"el": "Λάρισα", "en": "Larissa"}, 22.42, 39.64, "urban"),
    ("tyrnavos", {"el": "Τύρναβος", "en": "Tyrnavos"}, 22.29, 39.74, "stable"),
]

# Monitoring areas for the "Status" view: id, name, lon, lat, radius km, ids of the points inside
REGIONS = [
    ("larissa", {"el": "Δήμος Λαρισαίων", "en": "Larissa"}, 22.42, 39.64, 6.0, ["larissa"]),
    ("kileler", {"el": "Δήμος Κιλελέρ", "en": "Kileler"}, 22.57, 39.54, 10.0, ["nikaia", "platykampos", "kileler", "flood"]),
    ("rigas", {"el": "Δήμος Ρήγα Φεραίου", "en": "Rigas Feraios"}, 22.78, 39.47, 9.0, ["stefanovikeio", "karla"]),
    ("farsala", {"el": "Δήμος Φαρσάλων", "en": "Farsala"}, 22.38, 39.30, 9.0, ["farsala"]),
    ("palamas", {"el": "Δήμος Παλαμά", "en": "Palamas"}, 22.08, 39.46, 9.0, ["palamas"]),
    ("tyrnavos", {"el": "Δήμος Τυρνάβου", "en": "Tyrnavos"}, 22.29, 39.74, 7.0, ["tyrnavos"]),
]
KM_LON, KM_LAT = 85.9, 110.9  # km per degree at 39.5°N
PX_KM2 = ((BOUNDS[2] - BOUNDS[0]) / W * KM_LON) * ((BOUNDS[3] - BOUNDS[1]) / H * KM_LAT)

BOWLS = [  # lon, lat, sigma_lon, sigma_lat, mm/yr at centre
    (22.74, 39.46, 0.060, 0.040, 78.0),
    (22.47, 39.56, 0.070, 0.045, 46.0),
    (22.38, 39.30, 0.065, 0.040, 58.0),
    (22.08, 39.46, 0.080, 0.050, 31.0),
]


# ---------------------------------------------------------------- building blocks
def fbm(seed: int, beta: float = 3.0) -> np.ndarray:
    """Fractal noise with zero mean, unit variance."""
    rng = np.random.default_rng(seed)
    f = np.fft.fft2(rng.standard_normal((H, W)))
    ky = np.fft.fftfreq(H)[:, None]
    kx = np.fft.fftfreq(W)[None, :]
    k = np.sqrt(kx**2 + ky**2)
    k[0, 0] = 1.0
    out = np.fft.ifft2(f / k ** (beta / 2)).real
    out -= out.mean()
    return out / out.std()


def gauss(lon0, lat0, slon, slat):
    return np.exp(-(((LON - lon0) / slon) ** 2 + ((LAT - lat0) / slat) ** 2))


def smoothstep(x):
    x = np.clip(x, 0, 1)
    return x * x * (3 - 2 * x)


def doy(d: dt.date) -> int:
    return d.timetuple().tm_yday


# static landscape
rot = math.radians(-14)
u = (LON - 22.38) * math.cos(rot) + (LAT - 39.50) * math.sin(rot)
v = -(LON - 22.38) * math.sin(rot) + (LAT - 39.50) * math.cos(rot)
PLAIN = smoothstep((1.0 - (np.sqrt((u / 0.56) ** 2 + (v / 0.27) ** 2) + 0.16 * fbm(1))) / 0.14)
RELIEF = fbm(3, 2.4)
FOREST = smoothstep((fbm(4, 2.8) + 0.3) / 1.2) * (1 - PLAIN)
URBAN = gauss(22.42, 39.64, 0.030, 0.022)
for _, _, plon, plat, kind in POINTS:
    if kind in ("subsidence", "crop", "stable"):
        URBAN = np.maximum(URBAN, 0.7 * gauss(plon, plat, 0.009, 0.007))

# crop field patchwork
a = math.radians(21)
xr = PX * math.cos(a) + PY * math.sin(a)
yr = -PX * math.sin(a) + PY * math.cos(a)
fx = np.floor(xr / 9).astype(np.int64)
fy = np.floor(yr / 6).astype(np.int64)
fx -= fx.min()
fy -= fy.min()
_rng = np.random.default_rng(5)
R1 = _rng.random((fx.max() + 1, fy.max() + 1))[fx, fy]  # one random number per field: crop type
R2 = _rng.random((fx.max() + 1, fy.max() + 1))[fx, fy]  # and one for its brightness
PEAK = np.where(R1 < 0.45, 125.0, np.where(R1 < 0.86, 222.0, -999.0))  # winter cereals / summer crops / fallow
LOW = fbm(7)
EDGE = fbm(9, 2.6)
COH_HOLES = fbm(11, 2.2)


def growth(d: dt.date) -> np.ndarray:
    dd = np.abs(doy(d) - PEAK)
    dd = np.minimum(dd, 365 - dd)
    g = np.exp(-((dd / 48.0) ** 2))
    return np.where(PEAK < 0, 0.05, g)


def wetness(d: dt.date) -> float:
    return 0.5 * (1 + math.cos(2 * math.pi * (doy(d) - 45) / 365))


def water_mask(d: dt.date, k: int) -> np.ndarray:
    s = 1.0 + 0.30 * wetness(d)
    lake = np.sqrt(((LON - 22.845) / (0.060 * s)) ** 2 + ((LAT - 39.49) / (0.048 * s)) ** 2) + 0.10 * EDGE < 1.0
    flood_strength = {2: 1.0, 3: 0.45}.get(k, 0.0)  # a winter flood that then recedes
    if flood_strength:
        zone = gauss(22.61, 39.535, 0.11, 0.045)
        lake |= (LOW * 0.55 + 2.6 * zone * flood_strength) > 1.55
    return lake & (PLAIN > 0.4)


def pumping(d: dt.date) -> float:
    """Cumulative 'years of pumping': steady trend plus faster sinking in the irrigation season."""
    t = (d - DATES[0]).days / 365.25
    seas = lambda x: -math.cos(2 * math.pi * (doy(x) - 120) / 365)  # noqa: E731
    return t + 0.16 * (seas(d) - seas(DATES[0]))


def footprint(skew: float) -> np.ndarray:
    """Soft-edged slanted swath, like a real SAR frame."""
    left = 0.015 * W + skew * W * (PY / H)
    right = 0.94 * W + skew * W * (PY / H)
    return smoothstep((PX - left) / 6.0) * smoothstep((right - PX) / 6.0)


def fields(d: dt.date, k: int, speckle: bool = True) -> dict[str, np.ndarray]:
    rng = np.random.default_rng(100 + k)
    g, wet, water = growth(d), wetness(d), water_mask(d, k)
    summer = PEAK > 200

    hh_plain = -14.5 + 2.4 * (R2 - 0.5) + g * np.where(summer, 3.2, 2.0) + 1.6 * wet
    hh = PLAIN * hh_plain + (1 - PLAIN) * (-8.5 + 2.6 * RELIEF)
    hh = hh + 9.0 * URBAN
    hh = np.where(water, -24.5, hh)

    hv_plain = -24.5 + 7.5 * g + 1.6 * (R2 - 0.5)
    hv = PLAIN * hv_plain + (1 - PLAIN) * (-17.5 + 4.5 * FOREST + 1.0 * RELIEF)
    hv = hv + 6.0 * URBAN
    hv = np.where(water, -31.0, hv)

    vv_plain = -12.0 + 1.8 * (R2 - 0.5) + 1.1 * g + 2.6 * wet
    vv = PLAIN * vv_plain + (1 - PLAIN) * (-9.0 + 2.2 * RELIEF)
    vv = vv + 8.0 * URBAN
    vv = np.where(water, -21.5, vv)

    bowl = sum(amp * gauss(lo, la, sl, sa) for lo, la, sl, sa, amp in BOWLS)
    disp = -bowl * pumping(d) * PLAIN + (1.6 * fbm(200 + k, 2.2) if k else 0.0)

    out = {"hh_clean": hh, "hv_clean": hv, "vv_clean": vv, "disp": disp, "water": water}
    if speckle:
        out["hh"] = hh + rng.normal(0, 1.1, hh.shape)
        out["hv"] = hv + rng.normal(0, 1.1, hv.shape)
        out["vv"] = vv + rng.normal(0, 1.5, vv.shape)
    return out


def sample(arr: np.ndarray, plon: float, plat: float) -> float:
    x = int(round((plon - BOUNDS[0]) / (BOUNDS[2] - BOUNDS[0]) * (W - 1)))
    y = int(round((BOUNDS[3] - plat) / (BOUNDS[3] - BOUNDS[1]) * (H - 1)))
    return float(np.mean(arr[max(y - 1, 0):y + 2, max(x - 1, 0):x + 2]))


def write_regions(stack: list[dict], rng) -> None:
    """Area statistics for the Status view. With real data, compute the same numbers over each area."""
    iso = [d.isoformat() for d in DATES]
    regions = []
    for rid, name, rlon, rlat, radius, pts in REGIONS:
        inside = np.hypot((LON - rlon) * KM_LON, (LAT - rlat) * KM_LAT) < radius
        land = [inside & (PLAIN > 0.4) & ~s["water"] for s in stack]
        disp = [float(np.percentile(s["disp"][m], 10)) for s, m in zip(stack, land)]
        disp = [round(x - disp[0], 1) for x in disp]
        egms = 0.5 if rid == "palamas" else 0.94  # one area disagrees on purpose, to show the check working
        mean = lambda key, sd: [round(float(s[key][inside].mean()) + float(rng.normal(0, sd)), 2) for s in stack]  # noqa: E731
        region = {
            "id": rid, "name": name, "center": [rlon, rlat], "radius_km": radius, "points": pts,
            "series": {
                "dates": iso,
                "disp_mm": disp,
                "ref_mm": [0.0] + [round(x * egms + float(rng.normal(0, 1.8)), 1) for x in disp[1:]],
                "hh_db": mean("hh_clean", 0.15),
                "hv_db": mean("hv_clean", 0.15),
                "vv_db": mean("vv_clean", 0.2),
                "water_km2": [round(float((s["water"] & inside).sum()) * PX_KM2, 1) for s in stack],
                "vol_loss_hm3": [round(float(-np.minimum(s["disp"], 0)[m].sum()) * PX_KM2 * 1e-3, 2) for s, m in zip(stack, land)],
            },
        }
        if rid == "rigas":
            cap, vol = 180.0, []
            for d in DATES:  # a reservoir that fills until April and is drawn down by irrigation until October
                fill = 0.5 * (1 + math.cos(2 * math.pi * (doy(d) - 105) / 365))
                trend = 1 - 0.10 * (d - DATES[0]).days / 365.25
                vol.append(round(cap * (0.30 + 0.50 * fill) * trend + float(rng.normal(0, 0.8)), 1))
            region["reservoir"] = {
                "name": {"el": "Ταμιευτήρας Κάρλας", "en": "Lake Karla reservoir"},
                "unit": "hm³", "capacity": cap, "limit": 45.0,
                "dates": iso, "volume": vol,
                "method": {"el": "Έκταση νερού από το ραντάρ × καμπύλη στάθμης–όγκου του ταμιευτήρα.",
                           "en": "Radar water extent × the reservoir's level–volume curve."},
                "rule": {"el": "[Συμπληρώστε: ελάχιστο επιτρεπόμενο απόθεμα και κύρωση, από το Σχέδιο Διαχείρισης Λεκανών Απορροής Θεσσαλίας (EL08) / Οδηγία 2000/60/ΕΚ]",
                         "en": "[Fill in: minimum permitted storage and penalty, from the Thessaly River Basin Management Plan (EL08) / Directive 2000/60/EC]"},
            }
        regions.append(region)
    (DATA / "regions.json").write_text(json.dumps({
        "thresholds": {
            "sink_watch_mm_yr": -15, "sink_alert_mm_yr": -40, "egms_tolerance_mm_yr": 6,
            "reservoir_warn_days": 45, "heavy_rain_mm_day": 40, "heat_c": 38, "dry_spell_days": 30,
        },
        "regions": regions,
    }, ensure_ascii=False, indent=1), encoding="utf-8")


# ---------------------------------------------------------------- main
def main() -> None:
    LAYERS_DIR.mkdir(parents=True, exist_ok=True)
    rng = np.random.default_rng(42)
    fp_nisar, fp_s1 = footprint(0.045), footprint(-0.03)
    frames: dict[str, list] = {d["id"]: [] for d in LAYER_DEFS}
    stack: list[dict] = []
    f0 = fields(DATES[0], 0)

    for k, d in enumerate(DATES):
        f = fields(d, k)
        stack.append(f)
        iso = d.isoformat()
        tag = d.strftime("%Y%m%d")

        rasters = {
            "hh": (f["hh"], fp_nisar * 0.96),
            "hv": (f["hv"], fp_nisar * 0.96),
            "s1vv": (f["vv"], fp_s1 * 0.96),
        }
        diff = f["hh_clean"] - f0["hh_clean"] + rng.normal(0, 0.35, (H, W))
        rasters["change"] = (diff, fp_nisar * np.clip(np.abs(diff) / 3.5, 0, 1) ** 0.8 * 0.95)
        valid = ~f["water"] & ~((PLAIN < 0.35) & (COH_HOLES > -0.1))
        dalpha = fp_nisar * (0.50 + 0.45 * np.clip(-f["disp"] / 40.0, 0, 1))
        rasters["disp"] = (np.where(valid, f["disp"], np.nan), dalpha)

        for d_ in LAYER_DEFS:
            lid = d_["id"]
            vals, alpha = rasters[lid]
            rel = f"data/layers/{lid}_{iso}.webp"
            save_image(ROOT / rel, colorize(vals, LEGEND[lid], alpha))
            prefix = "S1" if lid == "s1vv" else "NISAR"
            frames[lid].append({
                "date": iso,
                "image": rel,
                "source": f"DEMO_{prefix}_{d_['product'].split(' (')[0].replace(' ', '_')}_{tag}",
                "url": "",
            })
        print("frame", iso)

    manifest = {
        "demo": True,
        "site": {
            # The map opens on Lake Koroneia. The synthetic layers and points below still cover BOUNDS (Thessaly).
            "name": {"el": "Λίμνη Κορώνεια", "en": "Lake Koroneia"},
            "country": {"el": "Ελλάδα", "en": "Greece"},
            "bounds": [23.03, 40.62, 23.27, 40.74],
            "center": [23.145, 40.682],
            "zoom": 12.2,
        },
        "reference": {"name": "EGMS", "label": {"el": "EGMS (Ευρωπαϊκή Υπηρεσία Κίνησης Εδάφους)", "en": "EGMS (European Ground Motion Service)"}},
        "layers": [{**{k_: v_ for k_, v_ in d_.items()}, "bounds": BOUNDS, "frames": frames[d_["id"]]} for d_ in LAYER_DEFS],
        "points": "data/points.geojson",
        "regions": "data/regions.json",
        "weather": {"provider": "open-meteo", "snapshot": "data/weather.json"},
        "validation": "data/validation.json",
        "content": "data/content.json",
    }
    (DATA / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")

    feats = []
    for pid, name, plon, plat, kind in POINTS:
        disp = [round(sample(s["disp"], plon, plat), 1) for s in stack]
        disp = [round(x - disp[0], 1) for x in disp]
        ref = [0.0] + [round(x * 0.94 + float(rng.normal(0, 2.2)), 1) for x in disp[1:]]
        feats.append({
            "type": "Feature",
            "geometry": {"type": "Point", "coordinates": [plon, plat]},
            "properties": {
                "id": pid, "name": name, "kind": kind,
                "series": {
                    "dates": [d.isoformat() for d in DATES],
                    "disp_mm": disp,
                    "ref_mm": ref,
                    "hh_db": [round(sample(s["hh_clean"], plon, plat) + float(rng.normal(0, 0.3)), 2) for s in stack],
                    "hv_db": [round(sample(s["hv_clean"], plon, plat) + float(rng.normal(0, 0.3)), 2) for s in stack],
                    "vv_db": [round(sample(s["vv_clean"], plon, plat) + float(rng.normal(0, 0.4)), 2) for s in stack],
                },
            },
        })
    (DATA / "points.geojson").write_text(
        json.dumps({"type": "FeatureCollection", "features": feats}, ensure_ascii=False, indent=1), encoding="utf-8")

    write_regions(stack, rng)

    years = (DATES[-1] - DATES[0]).days / 365.25
    final = stack[-1]["disp"]
    pairs = []
    ys, xs = np.where((PLAIN > 0.6) & ~stack[2]["water"])
    for i in rng.choice(len(xs), 90, replace=False):
        rate = float(final[ys[i], xs[i]] / years)
        pairs.append({"nisar": round(rate, 1), "ref": round(rate * 0.94 + float(rng.normal(0, 3.4)), 1)})
    (DATA / "validation.json").write_text(json.dumps({
        "unit": "mm/yr",
        "note": {"el": "Ρυθμός μετατόπισης στη γραμμή όρασης, ίδια σημεία στα δύο σύνολα δεδομένων.",
                 "en": "Line-of-sight displacement rate, same locations in both datasets."},
        "pairs": pairs,
    }, ensure_ascii=False, indent=1), encoding="utf-8")
    print("done:", len(DATES), "dates,", len(LAYER_DEFS), "layers")


if __name__ == "__main__":
    main()
