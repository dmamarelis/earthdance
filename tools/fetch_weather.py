"""Saves a weather snapshot (data/weather.json) so the Status view also works offline, e.g. during the demo.

The app always tries the live Open-Meteo API first and only falls back to this file.

Run:  python tools/fetch_weather.py            # demo set (data/)
      python tools/fetch_weather.py --real     # real set (data/real/)
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DAILY = "precipitation_sum,temperature_2m_max,temperature_2m_min,et0_fao_evapotranspiration,wind_speed_10m_max"
FIELDS = {"rain": "precipitation_sum", "tmax": "temperature_2m_max", "tmin": "temperature_2m_min",
          "et0": "et0_fao_evapotranspiration", "wind": "wind_speed_10m_max"}


def call(base: str, regions: list[dict], **extra) -> list[dict]:
    q = urllib.parse.urlencode({
        "latitude": ",".join(str(r["center"][1]) for r in regions),
        "longitude": ",".join(str(r["center"][0]) for r in regions),
        "daily": DAILY, "timezone": "auto", **extra,
    })
    with urllib.request.urlopen(f"{base}?{q}", timeout=60) as resp:
        data = json.load(resp)
    return data if isinstance(data, list) else [data]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--real", action="store_true", help="the real data set in data/real/")
    folder = ROOT / "data" / ("real" if ap.parse_args().real else "")
    regions = json.loads((folder / "regions.json").read_text(encoding="utf-8"))["regions"]
    today = dt.date.today()
    first = min(r["series"]["dates"][0] for r in regions)
    start = (dt.date.fromisoformat(first) - dt.timedelta(days=30)).isoformat()

    hist = call("https://archive-api.open-meteo.com/v1/archive", regions,
                start_date=start, end_date=(today - dt.timedelta(days=1)).isoformat())
    fc = call("https://api.open-meteo.com/v1/forecast", regions, past_days=14, forecast_days=7)

    by_region = {}
    for r, h, f in zip(regions, hist, fc):
        days: dict[str, dict] = {}
        for part in (f, h):  # history last: reanalysis wins where both exist
            for i, day in enumerate(part["daily"]["time"]):
                row = days.setdefault(day, {})
                for key, field in FIELDS.items():
                    v = part["daily"].get(field, [None] * (i + 1))[i]
                    if v is not None:
                        row[key] = v
        time = sorted(days)
        by_region[r["id"]] = {"time": time, **{k: [days[d].get(k) for d in time] for k in FIELDS}}

    out = folder / "weather.json"
    out.write_text(json.dumps({"saved": today.isoformat(), "source": "Open-Meteo", "byRegion": by_region},
                              separators=(",", ":")), encoding="utf-8")
    print(f"saved {out.relative_to(ROOT)}: {len(by_region)} areas, {start} .. {max(by_region[regions[0]['id']]['time'])}")


if __name__ == "__main__":
    main()
