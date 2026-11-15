"""Fits the lake forecast model to the real measurements and writes data/real/forecast.json.

    python tools/fit_forecast.py            (after tools/build_real_data.py)

The model (the app runs it in js/forecast.js, this script only finds its parameters):

    S  = how full the lake is, 0 (dry) .. 1 (fullest extent seen), one number per day
    dS = a * rain  -  k * ET0                      every day, S kept within 0..1
         a = a_wet  from November to March  (rain also runs off the catchment into the lake)
         a = a_dry  from April to October   (summer rain mostly soaks into dry soil)
    area = A_max * S ** q                          (shallow, flat lake: a little water still covers a large area)

    rain : daily precipitation, mm        ET0 : daily reference evapotranspiration, mm (FAO-56)
    Both from Open-Meteo (ERA5 reanalysis); ET0 stands for evaporation from the lake and the water
    the crops around it take, since both follow the same heat, sun and wind.

Fitting, in two steps:
  1. a_dry, k, q: least squares between the simulated area and the 14 measured areas (Feb–Sep 2026),
     driven by the real weather between the passes.
  2. a_wet: the measurements have no winter yet, so a_wet is set so that a lake that was at this
     year's lowest measured area on 1 October 2025 refills, with the real weather of winter 2025–26,
     to the area measured on 16 February 2026. Assumption: last autumn's low was like this year's.

Back-test: the same fit with only the passes up to 17 August 2026, then a forecast for 17 September,
compared with what NISAR measured that day.
"""
from __future__ import annotations

import json
import urllib.request
from datetime import date, timedelta
from pathlib import Path

import numpy as np
from scipy.optimize import brentq, least_squares

ROOT = Path(__file__).resolve().parents[1]
REAL = ROOT / "data" / "real"
LAKE = (40.682, 23.145)                       # lat, lon of the lake centre
CLIMATE_FROM = 2015                           # weather years used as the forecast scenarios
BACKTEST_CUTOFF = "2026-08-17"
REFILL_FROM = date(2025, 10, 1)


def fetch_weather(end: date) -> dict[str, tuple[float, float]]:
    q = (f"https://archive-api.open-meteo.com/v1/archive?latitude={LAKE[0]}&longitude={LAKE[1]}"
         f"&start_date={CLIMATE_FROM}-01-01&end_date={end}"
         "&daily=precipitation_sum,et0_fao_evapotranspiration&timezone=auto")
    with urllib.request.urlopen(q, timeout=60) as r:
        d = json.load(r)["daily"]
    return {t: (p, e) for t, p, e in zip(d["time"], d["precipitation_sum"], d["et0_fao_evapotranspiration"])
            if p is not None and e is not None}


def wet(d: date) -> bool:
    return d.month >= 11 or d.month <= 3


def simulate(p, start: date, area0: float, days: int, amax: float, wx) -> np.ndarray:
    """Daily lake area from `start` for `days` days. p = (a_wet, a_dry, k, q). wx(date) -> (rain, et0)."""
    a_wet, a_dry, k, q = p
    s = (min(area0, amax) / amax) ** (1 / q)
    out = np.empty(days + 1)
    for i in range(days + 1):
        d = start + timedelta(i)
        out[i] = amax * s ** q
        rain, et0 = wx(d)
        s = min(1.0, max(0.0, s + (a_wet if wet(d) else a_dry) * rain - k * et0))
    return out


def fit(dates: list[str], area: np.ndarray, amax: float, wx) -> dict:
    d0 = date.fromisoformat(dates[0])
    idx = [(date.fromisoformat(x) - d0).days for x in dates]

    # step 1: dry-season parameters (one rain coefficient for the whole measured period)
    def res(p):
        a, k, q = p
        return simulate((a, a, k, q), d0, area[0], idx[-1], amax, wx)[idx] - area

    f = least_squares(res, [0.001, 0.001, 0.3], bounds=([0, 1e-5, 0.05], [0.05, 0.05, 2.0]))
    a_dry, k, q = f.x
    n = len(area)
    rmse = float(np.sqrt(np.mean(f.fun ** 2)))
    cov = np.linalg.pinv(f.jac.T @ f.jac) * float(np.sum(f.fun ** 2)) / max(n - 3, 1)
    sd = np.sqrt(np.clip(np.diag(cov), 0, None))

    # step 2: winter rain coefficient from the refill assumption
    low = float(area.min())
    days = (d0 - REFILL_FROM).days

    def miss(a_wet):
        return simulate((a_wet, a_dry, k, q), REFILL_FROM, low, days, amax, wx)[-1] - area[0]

    a_wet = brentq(miss, a_dry, 0.05) if miss(a_dry) < 0 < miss(0.05) else a_dry
    p = (float(a_wet), float(a_dry), float(k), float(q))
    sim = simulate(p, d0, area[0], idx[-1], amax, wx)[idx]
    return {"p": p, "sd": {"a_dry": float(sd[0]), "k": float(sd[1]), "q": float(sd[2])},
            "rmse": float(np.sqrt(np.mean((sim - area) ** 2))), "rmse_step1": rmse,
            "fitted": [round(float(v), 2) for v in sim], "refill_low": low}


def main() -> None:
    regions = json.loads((REAL / "regions.json").read_text(encoding="utf-8"))
    lake = next(r for r in regions["regions"] if r.get("reservoir"))
    res = lake["reservoir"]
    dates = res["dates"]
    area = np.array(res["area_km2"], float)
    amax = float(max(res["capacity"], area.max()))

    end = date.today() - timedelta(days=2)
    W = fetch_weather(end)
    wx = lambda d: W.get(str(d), (0.0, 0.0))                        # noqa: E731

    full = fit(dates, area, amax, wx)
    a_wet, a_dry, k, q = full["p"]
    print(f"a_wet {a_wet:.5f}  a_dry {a_dry:.5f}  k {k:.5f}  q {q:.3f}  rmse {full['rmse']:.2f} km2")
    print("measured", list(area))
    print("model   ", full["fitted"])

    # back-test: fit on the passes up to the cutoff, forecast the last pass with the real weather
    cut = [i for i, d in enumerate(dates) if d <= BACKTEST_CUTOFF]
    bt = fit([dates[i] for i in cut], area[cut], amax, wx)
    c0 = date.fromisoformat(dates[cut[-1]])
    target = date.fromisoformat(dates[-1])
    pred = float(simulate(bt["p"], c0, area[cut[-1]], (target - c0).days, amax, wx)[-1])
    # the same back-test model, but crossing the limit with every past year's weather (what it could have said that day)
    cross = []
    for y in range(CLIMATE_FROM, end.year):
        def wy(d, y=y):
            try:
                return W.get(str(d.replace(year=y + (d.year - c0.year))), (0.0, 0.0))
            except ValueError:
                return W.get(str(d.replace(year=y + (d.year - c0.year), day=28)), (0.0, 0.0))
        tr = simulate(bt["p"], c0, area[cut[-1]], 120, amax, wy)
        below = np.nonzero(tr < res["limit"])[0]
        if below.size:
            cross.append(str(c0 + timedelta(int(below[0]))))
    cross.sort()
    backtest = {
        "cutoff": dates[cut[-1]], "passes": len(cut), "target": dates[-1],
        "predicted": round(pred, 2), "observed": round(float(area[-1]), 2),
        "cross": {"early": cross[0], "mid": cross[len(cross) // 2], "late": cross[-1]} if cross else None,
        "members": len(cross),
    }
    print("back-test", backtest)

    # weather scenarios: one per past year, day by day (index 0 = 1 Jan, 365 = 31 Dec, Feb 29 copies Feb 28)
    def year_series(y: int):
        rain, et0 = [], []
        for m, dd in [(m, dd) for m in range(1, 13) for dd in range(1, 32)]:
            try:
                d = date(y, m, dd)
            except ValueError:
                if m == 2 and dd == 29:
                    d = date(y, 2, 28)
                else:
                    continue
            r, e = W.get(str(d), (None, None))
            rain.append(None if r is None else round(r, 1))
            et0.append(None if e is None else round(e, 2))
        return rain, et0

    years = list(range(CLIMATE_FROM, end.year + 1))
    climate = {str(y): dict(zip(("rain", "et0"), year_series(y))) for y in years}

    out = {
        "model": "storage water balance: dS = a*rain - k*ET0 ; area = A_max * S^q",
        "region": lake["id"],
        "params": {"a_wet": a_wet, "a_dry": a_dry, "k": k, "q": q, "amax": amax},
        "sd": full["sd"],
        "rmse": round(full["rmse"], 2),
        "fit": {"dates": dates, "measured": [round(float(v), 2) for v in area], "model": full["fitted"]},
        "assumptions": {
            "refill_from": str(REFILL_FROM), "refill_low": full["refill_low"],
            "wet_months": "Nov–Mar", "weather": "Open-Meteo ERA5 reanalysis (rain, FAO-56 ET0) at the lake centre",
        },
        "backtest": backtest,
        "climate": {"years": years, "until": str(end), "days": climate},
    }
    path = REAL / "forecast.json"
    path.write_text(json.dumps(out, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    man = json.loads((REAL / "manifest.json").read_text(encoding="utf-8"))
    man["forecast"] = "data/real/forecast.json"
    (REAL / "manifest.json").write_text(json.dumps(man, ensure_ascii=False, indent=1), encoding="utf-8")
    print(f"wrote {path.relative_to(ROOT)} ({path.stat().st_size // 1024} KB)")


if __name__ == "__main__":
    main()
