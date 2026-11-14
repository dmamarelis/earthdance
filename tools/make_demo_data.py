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
import urllib.request
import warnings
from pathlib import Path

import numpy as np

from colormap import colorize, save_image

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / "data"
LAYERS_DIR = DATA / "layers"
VALUES_DIR = DATA / "values"
CLOUDS_DIR = DATA / "clouds"
GRID = 4  # value grid for hover = image downsampled by this factor

W, H = 640, 380
BOUNDS = [22.85, 40.50, 23.65, 40.86]  # west, south, east, north
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
        "classes": [
            [-19, {"el": "Ήρεμο νερό: λεία επιφάνεια, το σήμα ανακλάται μακριά σαν από καθρέφτη", "en": "Calm water: a smooth surface reflects the signal away like a mirror"}],
            [-14, {"el": "Λείο ή γυμνό έδαφος: λίγη τραχύτητα, λίγο σήμα επιστρέφει", "en": "Smooth or bare soil: little roughness, little signal returns"}],
            [-9, {"el": "Χωράφι ή υγρό έδαφος: τραχύτητα και νερό (υψηλή διηλεκτρική σταθερά) δυναμώνουν την επιστροφή", "en": "Field or wet soil: roughness and water (high dielectric constant) strengthen the return"}],
            [-3, {"el": "Καλαμιώνας ή βλάστηση μέσα σε νερό: διπλή ανάκλαση νερό → μίσχος → δορυφόρος", "en": "Reeds or vegetation standing in water: double bounce water → stem → satellite"}],
            [99, {"el": "Κτίρια ή κατασκευές: γωνίες που στέλνουν το σήμα πίσω (διπλή ανάκλαση)", "en": "Buildings or structures: corners send the signal straight back (double bounce)"}],
        ],
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
        "classes": [
            [-27, {"el": "Νερό: σχεδόν καμία αποπόλωση", "en": "Water: almost no depolarisation"}],
            [-21, {"el": "Γυμνό ή οργωμένο έδαφος: λίγος όγκος βλάστησης", "en": "Bare or ploughed soil: little vegetation volume"}],
            [-16, {"el": "Καλλιέργεια σε ανάπτυξη: ο όγκος των φυτών αποπολώνει το σήμα", "en": "Growing crop: plant volume depolarises the signal"}],
            [99, {"el": "Πυκνή βλάστηση, καλαμιώνας ή δάσος: πολλαπλές σκεδάσεις μέσα στον όγκο", "en": "Dense vegetation, reeds or forest: multiple scattering inside the volume"}],
        ],
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
        "classes": [
            [-3, {"el": "Πιο σκούρο από την αρχή: συνήθως νερό που ήρθε ή βλάστηση που χάθηκε", "en": "Darker than at the start: usually new water or lost vegetation"}],
            [3, {"el": "Μικρή ή καμία αλλαγή", "en": "Little or no change"}],
            [99, {"el": "Πιο φωτεινό: ανάπτυξη, όργωμα (τραχύτητα) ή υγρασία", "en": "Brighter: growth, ploughing (roughness) or moisture"}],
        ],
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
        "classes": [
            [-40, {"el": "Έντονη βύθιση: συμπίεση υδροφορέα από άντληση", "en": "Strong sinking: aquifer compaction from pumping"}],
            [-10, {"el": "Βύθιση", "en": "Sinking"}],
            [10, {"el": "Σταθερό, μέσα στον θόρυβο της μέτρησης", "en": "Stable, within measurement noise"}],
            [99, {"el": "Ανύψωση", "en": "Uplift"}],
        ],
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
        "classes": [
            [-17, {"el": "Νερό (C-band)", "en": "Water (C-band)"}],
            [-10, {"el": "Έδαφος ή χαμηλή βλάστηση: το C-band σκεδάζεται στην κορυφή των φυτών", "en": "Soil or low vegetation: C-band scatters at the top of the plants"}],
            [99, {"el": "Τραχιά επιφάνεια ή κτίρια", "en": "Rough surface or buildings"}],
        ],
    },
    {
        "id": "sm", "band": "L", "sensor": "NISAR", "product": "L3 Soil Moisture", "unit": "%",
        "name": {"el": "Υγρασία εδάφους", "en": "Soil moisture"},
        "short": {"el": "Πόσο νερό κρατά το χώμα", "en": "How much water the soil holds"},
        "description": {
            "el": "Το νερό έχει πολύ μεγαλύτερη διηλεκτρική σταθερά από το ξερό χώμα, άρα το υγρό έδαφος επιστρέφει περισσότερο σήμα. Από αυτή τη διαφορά εκτιμάται η υγρασία (όγκος νερού / όγκος εδάφους).",
            "en": "Water has a far higher dielectric constant than dry soil, so wet ground returns more signal. Moisture (water volume / soil volume) is estimated from that difference.",
        },
        "legend": {"stops": [[4, "#7a4b1e"], [14, "#c9a46a"], [24, "#ece6cf"], [34, "#5fb3d9"], [45, "#1d4e89"]],
                   "low": {"el": "ξερό", "en": "dry"}, "high": {"el": "κορεσμένο", "en": "saturated"}},
        "classes": [
            [12, {"el": "Ξερό έδαφος: χαμηλή διηλεκτρική σταθερά", "en": "Dry soil: low dielectric constant"}],
            [25, {"el": "Μέτρια υγρασία", "en": "Moderate moisture"}],
            [99, {"el": "Υγρό έδαφος ή αρδευόμενο χωράφι", "en": "Wet soil or irrigated field"}],
        ],
    },
]
LEGEND = {d["id"]: d["legend"] for d in LAYER_DEFS}

# name, lon, lat, kind  (coordinates are approximate: check them before using real data)
POINTS = [
    ("agvasileios", {"el": "Άγιος Βασίλειος", "en": "Agios Vasileios"}, 23.12, 40.645, "subsidence"),
    ("gerakarou", {"el": "Γερακαρού", "en": "Gerakarou"}, 23.21, 40.63, "subsidence"),
    ("langadikia", {"el": "Λαγκαδίκια", "en": "Langadikia"}, 23.26, 40.64, "subsidence"),
    ("lagyna", {"el": "Λαγυνά", "en": "Lagyna"}, 23.00, 40.725, "subsidence"),
    ("koronia", {"el": "Λίμνη Κορώνεια", "en": "Lake Koronia"}, 23.15, 40.683, "water"),
    ("volvi", {"el": "Λίμνη Βόλβη", "en": "Lake Volvi"}, 23.47, 40.68, "water"),
    ("flood", {"el": "Κάμπος Λαγκαδά", "en": "Langadas lowland"}, 23.05, 40.70, "flood"),
    ("kolchiko", {"el": "Κολχικό", "en": "Kolchiko"}, 23.13, 40.745, "crop"),
    ("evangelismos", {"el": "Ευαγγελισμός", "en": "Evangelismos"}, 23.215, 40.725, "crop"),
    ("langadas", {"el": "Λαγκαδάς", "en": "Langadas"}, 23.07, 40.75, "urban"),
    ("neaapollonia", {"el": "Νέα Απολλωνία", "en": "Nea Apollonia"}, 23.44, 40.63, "stable"),
]

# Monitoring areas for the "Status" view: id, name, lon, lat, radius km, ids of the points inside
REGIONS = [
    ("south", {"el": "Νότια όχθη Κορώνειας", "en": "Koronia south shore"}, 23.16, 40.635, 4.5, ["agvasileios", "gerakarou", "koronia"]),
    ("east", {"el": "Λαγκαδίκια", "en": "Langadikia"}, 23.29, 40.645, 4.0, ["langadikia"]),
    ("langadas", {"el": "Λαγκαδάς", "en": "Langadas"}, 23.04, 40.725, 5.0, ["langadas", "lagyna", "flood"]),
    ("north", {"el": "Βόρεια όχθη Κορώνειας", "en": "Koronia north shore"}, 23.17, 40.74, 4.0, ["kolchiko", "evangelismos"]),
    ("apollonia", {"el": "Νέα Απολλωνία", "en": "Nea Apollonia"}, 23.44, 40.63, 4.0, ["neaapollonia", "volvi"]),
]
KM_LON, KM_LAT = 84.4, 111.0  # km per degree at 40.7°N
PX_KM2 = ((BOUNDS[2] - BOUNDS[0]) / W * KM_LON) * ((BOUNDS[3] - BOUNDS[1]) / H * KM_LAT)

BOWLS = [  # lon, lat, sigma_lon, sigma_lat, mm/yr at centre
    (23.12, 40.645, 0.045, 0.018, 74.0),
    (23.21, 40.630, 0.050, 0.020, 50.0),
    (23.27, 40.640, 0.040, 0.018, 40.0),
    (23.01, 40.725, 0.050, 0.022, 30.0),
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
rot = math.radians(-4)   # the Mygdonia basin runs east-west
u = (LON - 23.22) * math.cos(rot) + (LAT - 40.69) * math.sin(rot)
v = -(LON - 23.22) * math.sin(rot) + (LAT - 40.69) * math.cos(rot)
PLAIN = smoothstep((1.0 - (np.sqrt((u / 0.39) ** 2 + (v / 0.115) ** 2) + 0.16 * fbm(1))) / 0.14)
RELIEF = fbm(3, 2.4)
FOREST = smoothstep((fbm(4, 2.8) + 0.3) / 1.2) * (1 - PLAIN)
URBAN = gauss(23.07, 40.75, 0.018, 0.012)
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

# Koronia: lake centre and semi-axes at "normal" level; a reed belt around it
LAKE_C, LAKE_RX, LAKE_RY = (23.15, 40.683), 0.062, 0.021
R_LAKE = np.sqrt(((LON - LAKE_C[0]) / LAKE_RX) ** 2 + ((LAT - LAKE_C[1]) / LAKE_RY) ** 2)
REED = (R_LAKE + 0.08 * EDGE > 1.12) & (R_LAKE + 0.08 * EDGE < 1.40) & (fbm(13, 2.4) > -0.6)
# part of the north-shore belt is cleared for fields, a little more every month (an illegal-encroachment scenario)
ENC_ZONE = REED & (LAT > LAKE_C[1] + 0.012) & (LON > 23.10) & (LON < 23.21) & (fbm(17, 2.2) > -0.2)
ENC_T = 0.35 + 1.20 * np.clip((fbm(19, 2.0) + 2.0) / 4.0, 0, 1)  # years after the first date when each pixel is cleared


def cleared(d: dt.date) -> np.ndarray:
    return ENC_ZONE & ((d - DATES[0]).days / 365.25 >= ENC_T)


def growth(d: dt.date) -> np.ndarray:
    dd = np.abs(doy(d) - PEAK)
    dd = np.minimum(dd, 365 - dd)
    g = np.exp(-((dd / 48.0) ** 2))
    return np.where(PEAK < 0, 0.05, g)


def wetness(d: dt.date) -> float:
    return 0.5 * (1 + math.cos(2 * math.pi * (doy(d) - 45) / 365))


def lake_fill(d: dt.date) -> float:
    """0..1: the lake fills with winter inflow until April and is drawn down by irrigation until October."""
    return 0.5 * (1 + math.cos(2 * math.pi * (doy(d) - 105) / 365))


def water_mask(d: dt.date, k: int) -> np.ndarray:
    s = (0.80 + 0.45 * lake_fill(d)) * (1 - 0.10 * (d - DATES[0]).days / 365.25)  # seasonal, and slowly drying
    lake = R_LAKE / s + 0.10 * EDGE < 1.0
    lake |= np.sqrt(((LON - 23.47) / 0.115) ** 2 + ((LAT - 40.68) / 0.027) ** 2) + 0.06 * EDGE < 1.0  # Volvi: stable
    flood_strength = {2: 1.0, 3: 0.45}.get(k, 0.0)  # a winter flood that then recedes
    if flood_strength:
        zone = gauss(23.05, 40.70, 0.06, 0.022)
        lake |= (LOW * 0.55 + 2.6 * zone * flood_strength) > 1.55
    return lake & (PLAIN > 0.4) & ~REED  # flooded reeds stay bright: they are not open water


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
    clr = cleared(d)
    reeds = REED & ~clr
    reed_g = 0.5 * (1 + math.cos(2 * math.pi * (doy(d) - 200) / 365))  # reeds peak in July

    hh_plain = -14.5 + 2.4 * (R2 - 0.5) + g * np.where(summer, 3.2, 2.0) + 1.6 * wet
    hh = PLAIN * hh_plain + (1 - PLAIN) * (-8.5 + 2.6 * RELIEF)
    hh = hh + 9.0 * URBAN
    hh = np.where(reeds, -7.5 + 1.5 * reed_g + 1.5 * wet, hh)   # double bounce: water → stem → satellite
    hh = np.where(clr, -11.0 + 1.0 * (R2 - 0.5), hh)               # freshly ploughed: rough, bare
    hh = np.where(water, -24.5, hh)

    hv_plain = -24.5 + 7.5 * g + 1.6 * (R2 - 0.5)
    hv = PLAIN * hv_plain + (1 - PLAIN) * (-17.5 + 4.5 * FOREST + 1.0 * RELIEF)
    hv = hv + 6.0 * URBAN
    hv = np.where(reeds, -14.0 + 2.5 * reed_g, hv)                 # volume scattering in the reed canopy
    hv = np.where(clr, -22.5 + 1.0 * (R2 - 0.5), hv)
    hv = np.where(water, -31.0, hv)

    vv_plain = -12.0 + 1.8 * (R2 - 0.5) + 1.1 * g + 2.6 * wet
    vv = PLAIN * vv_plain + (1 - PLAIN) * (-9.0 + 2.2 * RELIEF)
    vv = vv + 8.0 * URBAN
    vv = np.where(reeds, -10.0 + 1.0 * reed_g, vv)
    vv = np.where(clr, -11.5, vv)
    vv = np.where(water, -21.5, vv)

    irrigated = summer & (doy(d) > 150) & (doy(d) < 260)
    sm = 9 + 24 * wet + 4 * LOW + 10 * irrigated * g + 14 * REED + 3 * (R2 - 0.5)
    sm = np.clip(np.where(PLAIN > 0.4, sm, 8 + 14 * wet + 3 * RELIEF), 3, 48)
    sm = np.where(water, np.nan, sm)

    bowl = sum(amp * gauss(lo, la, sl, sa) for lo, la, sl, sa, amp in BOWLS)
    disp = -bowl * pumping(d) * PLAIN + (1.6 * fbm(200 + k, 2.2) if k else 0.0)

    out = {"hh_clean": hh, "hv_clean": hv, "vv_clean": vv, "disp": disp, "water": water,
           "sm": sm, "reeds": reeds, "cleared": clr}
    if speckle:
        out["hh"] = hh + rng.normal(0, 1.1, hh.shape)
        out["hv"] = hv + rng.normal(0, 1.1, hv.shape)
        out["vv"] = vv + rng.normal(0, 1.5, vv.shape)
    return out


def cloud_cover() -> dict[str, float]:
    """Real daily cloud cover (%) at the lake for each date, from Open-Meteo. Falls back to a seasonal guess offline."""
    try:
        url = ("https://archive-api.open-meteo.com/v1/archive?latitude={:.3f}&longitude={:.3f}&start_date={}&end_date={}"
               "&daily=cloud_cover_mean&timezone=auto").format(LAKE_C[1], LAKE_C[0], DATES[0], DATES[-1])
        with urllib.request.urlopen(url, timeout=60) as r:
            d = json.load(r)["daily"]
        return {t: float(c) for t, c in zip(d["time"], d["cloud_cover_mean"]) if c is not None}
    except Exception as err:  # noqa: BLE001
        print("cloud cover offline, using a seasonal guess:", err)
        return {x.isoformat(): 40 + 40 * wetness(x) for x in DATES}


def cloud_image(k: int, cover: float) -> np.ndarray:
    """RGBA clouds covering roughly `cover` % of the frame."""
    n = fbm(300 + k, 3.8)
    thr = np.quantile(n, 1 - cover / 100) if cover > 0 else n.max() + 1
    a = smoothstep((n - thr) / 0.3) * 0.97
    a *= smoothstep(np.minimum.reduce([PX, W - 1 - PX, PY, H - 1 - PY]) / 40.0)  # soft frame edge
    shade = 228 + 24 * smoothstep((n - thr) / 2.2)
    rgba = np.zeros((H, W, 4), np.uint8)
    rgba[..., 0] = rgba[..., 1] = np.clip(shade, 0, 255).astype(np.uint8)
    rgba[..., 2] = np.clip(shade + 4, 0, 255).astype(np.uint8)
    rgba[..., 3] = (a * 255).astype(np.uint8)
    return rgba


def save_values(path: Path, vals: np.ndarray) -> list[int]:
    """Block-averaged values as little-endian int16 (value × 10, -32768 = no data). Used by the map hover."""
    h, w = vals.shape[0] // GRID, vals.shape[1] // GRID
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", RuntimeWarning)  # all-NaN blocks are expected (no data)
        blocks = np.nanmean(vals[:h * GRID, :w * GRID].reshape(h, GRID, w, GRID), axis=(1, 3))
    out = np.where(np.isfinite(blocks), np.clip(np.round(blocks * 10), -32767, 32767), -32768).astype("<i2")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(out.tobytes())
    return [w, h]


def sample(arr: np.ndarray, plon: float, plat: float) -> float:
    x = int(round((plon - BOUNDS[0]) / (BOUNDS[2] - BOUNDS[0]) * (W - 1)))
    y = int(round((BOUNDS[3] - plat) / (BOUNDS[3] - BOUNDS[1]) * (H - 1)))
    return float(np.mean(arr[max(y - 1, 0):y + 2, max(x - 1, 0):x + 2]))


def lake_outline(water: np.ndarray) -> list[list[float]]:
    """Lake edge as a polygon (72 vertices): for each direction, the farthest water pixel from the centre."""
    lake = water & (R_LAKE < 1.6)
    ang = np.arctan2((LAT - LAKE_C[1]) / LAKE_RY, (LON - LAKE_C[0]) / LAKE_RX)
    ring = []
    for i in range(72):
        a = -math.pi + i * 2 * math.pi / 72
        sel = lake & (np.abs(np.angle(np.exp(1j * (ang - a)))) < math.pi / 72)
        r = float(R_LAKE[sel].max()) if sel.any() else 0.0
        ring.append([round(LAKE_C[0] + r * LAKE_RX * math.cos(a), 5), round(LAKE_C[1] + r * LAKE_RY * math.sin(a), 5)])
    return ring + [ring[0]]


def write_regions(stack: list[dict], rng) -> None:
    """Area statistics for the Status view. With real data, compute the same numbers over each area."""
    iso = [d.isoformat() for d in DATES]
    regions = []
    for rid, name, rlon, rlat, radius, pts in REGIONS:
        inside = np.hypot((LON - rlon) * KM_LON, (LAT - rlat) * KM_LAT) < radius
        land = [inside & (PLAIN > 0.4) & ~s["water"] for s in stack]
        assert all(m.any() for m in land), f"area {rid} has no land pixels"
        disp = [float(np.percentile(s["disp"][m], 10)) for s, m in zip(stack, land)]
        disp = [round(x - disp[0], 1) for x in disp]
        egms = 0.5 if rid == "east" else 0.94  # one area disagrees on purpose, to show the check working
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
                "soil_moist_pct": [round(float(np.nanmean(s["sm"][m])), 1) for s, m in zip(stack, land)],
                "reed_km2": [round(float((s["reeds"] & inside).sum()) * PX_KM2, 2) for s in stack],
                "cleared_ha": [round(float((s["cleared"] & inside).sum()) * PX_KM2 * 100, 1) for s in stack],
            },
        }
        if rid == "south":
            cap, vol = 120.0, []
            for d in DATES:  # a reservoir that fills until April and is drawn down by irrigation until October
                fill = lake_fill(d)
                trend = 1 - 0.10 * (d - DATES[0]).days / 365.25
                vol.append(round(cap * (0.30 + 0.50 * fill) * trend + float(rng.normal(0, 0.8)), 1))
            region["reservoir"] = {
                "name": {"el": "Λίμνη Κορώνεια", "en": "Lake Koronia"},
                "unit": "hm³", "capacity": cap, "limit": 30.0,
                "dates": iso, "volume": vol,
                "area_km2": [round(float((s["water"] & (R_LAKE < 1.6)).sum()) * PX_KM2, 2) for s in stack],
                "outline": lake_outline(stack[-1]["water"]),
                "method": {"el": "Έκταση νερού από το ραντάρ × καμπύλη στάθμης–όγκου της λίμνης.",
                           "en": "Radar water extent × the lake's level–volume curve."},
                "rule": {"el": "[Συμπληρώστε: ελάχιστο επιτρεπόμενο απόθεμα και κύρωση, από το Σχέδιο Διαχείρισης Λεκανών Απορροής Κεντρικής Μακεδονίας (EL10) / Οδηγία 2000/60/ΕΚ]",
                         "en": "[Fill in: minimum permitted storage and penalty, from the Central Macedonia River Basin Management Plan (EL10) / Directive 2000/60/EC]"},
            }
        regions.append(region)
    (DATA / "regions.json").write_text(json.dumps({
        "thresholds": {
            "sink_watch_mm_yr": -15, "sink_alert_mm_yr": -40, "egms_tolerance_mm_yr": 8,
            "reservoir_warn_days": 45, "heavy_rain_mm_day": 40, "heat_c": 38, "dry_spell_days": 30,
            "soil_dry_drop_pct": 15, "reed_loss_pct": 10, "cleared_watch_ha": 5, "cleared_alert_ha": 25,
        },
        "regions": regions,
    }, ensure_ascii=False, indent=1), encoding="utf-8")


# ---------------------------------------------------------------- main
def main() -> None:
    LAYERS_DIR.mkdir(parents=True, exist_ok=True)
    CLOUDS_DIR.mkdir(parents=True, exist_ok=True)
    rng = np.random.default_rng(42)
    covers = cloud_cover()
    clouds = []
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
            "sm": (f["sm"], fp_nisar * 0.92),
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
            vrel = f"data/values/{lid}_{iso}.bin"
            d_["grid"] = save_values(ROOT / vrel, np.where(alpha > 0.05, vals, np.nan) if lid != "change" else np.where(fp_nisar > 0.05, vals, np.nan))
            prefix = "S1" if lid == "s1vv" else "NISAR"
            frames[lid].append({
                "date": iso,
                "image": rel,
                "values": vrel,
                "source": f"DEMO_{prefix}_{d_['product'].split(' (')[0].replace(' ', '_')}_{tag}",
                "url": "",
            })
        cover = round(covers.get(iso, 60.0))
        crel = f"data/clouds/{iso}.webp"
        save_image(ROOT / crel, cloud_image(k, cover))
        clouds.append({"date": iso, "image": crel, "cover": cover})
        print("frame", iso, "clouds", cover, "%")

    manifest = {
        "demo": True,
        "site": {
            "name": {"el": "Λίμνη Κορώνεια", "en": "Lake Koronia"},
            "country": {"el": "Ελλάδα", "en": "Greece"},
            "bounds": BOUNDS,
            "center": [23.20, 40.68],
            "zoom": 10.6,
        },
        "reference": {"name": "EGMS", "label": {"el": "EGMS (Ευρωπαϊκή Υπηρεσία Κίνησης Εδάφους)", "en": "EGMS (European Ground Motion Service)"}},
        "layers": [{**{k_: v_ for k_, v_ in d_.items()}, "bounds": BOUNDS, "frames": frames[d_["id"]]} for d_ in LAYER_DEFS],
        "optical": {
            "name": {"el": "Οπτικός δορυφόρος (π.χ. Sentinel-2)", "en": "Optical satellite (e.g. Sentinel-2)"},
            "note": {"el": "Η νεφοκάλυψη κάθε ημερομηνίας είναι η πραγματική (Open-Meteo)· το σχήμα των σύννεφων είναι σχηματικό.",
                     "en": "Each date's cloud cover is the real one (Open-Meteo); the shape of the clouds is schematic."},
            "clouds": clouds,
        },
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
