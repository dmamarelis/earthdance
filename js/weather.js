// Daily weather for each monitoring area, from Open-Meteo (free, no key, CC BY 4.0).
//   history : archive-api.open-meteo.com  (reanalysis: a model constrained by observations, not a station)
//   today + 7-day forecast : api.open-meteo.com
// If the network is unavailable the app falls back to a saved snapshot (tools/fetch_weather.py).

const DAILY = 'precipitation_sum,temperature_2m_max,temperature_2m_min,et0_fao_evapotranspiration,wind_speed_10m_max';
const FIELDS = { rain: 'precipitation_sum', tmax: 'temperature_2m_max', tmin: 'temperature_2m_min', et0: 'et0_fao_evapotranspiration', wind: 'wind_speed_10m_max' };

export const isoDay = (d) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
const addDays = (iso, n) => { const d = new Date(`${iso}T12:00:00`); d.setDate(d.getDate() + n); return isoDay(d); };

async function call(base, regions, extra) {
  const q = new URLSearchParams({
    latitude: regions.map((r) => r.center[1]).join(','),
    longitude: regions.map((r) => r.center[0]).join(','),
    daily: DAILY, timezone: 'auto', ...extra,
  });
  const r = await fetch(`${base}?${q}`);
  if (!r.ok) throw new Error(`weather ${r.status}`);
  const j = await r.json();
  return Array.isArray(j) ? j : [j];
}

/** Merges history and forecast into one daily series per area: { time[], rain[], tmax[], tmin[], et0[], wind[] }. */
function merge(parts) {
  const byDay = new Map();
  for (const p of parts) {
    if (!p?.daily) continue;
    p.daily.time.forEach((day, i) => {
      const row = byDay.get(day) || {};
      for (const [k, f] of Object.entries(FIELDS)) if (p.daily[f]?.[i] != null) row[k] = p.daily[f][i];
      byDay.set(day, row);
    });
  }
  const time = [...byDay.keys()].sort();
  const out = { time };
  for (const k of Object.keys(FIELDS)) out[k] = time.map((d) => byDay.get(d)[k] ?? null);
  return out;
}

/**
 * @param regions  [{ id, center:[lon,lat] }]
 * @param start    first day needed (YYYY-MM-DD)
 * @param snapshot optional URL of a saved copy
 * @returns { source:'live'|'snapshot', today, byRegion:{ id: series } }  or null when nothing could be loaded
 */
export async function loadWeather(regions, start, snapshot) {
  const today = isoDay(new Date());
  const key = `earthdance-wx:${today}:${start}:${regions.map((r) => r.id).join()}`;
  try {
    const hit = sessionStorage.getItem(key);
    if (hit) return JSON.parse(hit);
  } catch { /* storage may be blocked */ }

  try {
    const [hist, fc] = await Promise.all([
      call('https://archive-api.open-meteo.com/v1/archive', regions, { start_date: start, end_date: addDays(today, -1) }),
      call('https://api.open-meteo.com/v1/forecast', regions, { past_days: 14, forecast_days: 7 }),
    ]);
    const byRegion = {};
    // forecast first, history second: where both exist, the reanalysis value wins
    regions.forEach((r, i) => { byRegion[r.id] = merge([fc[i], hist[i]]); });
    const out = { source: 'live', today, byRegion };
    try { sessionStorage.setItem(key, JSON.stringify(out)); } catch { /* ignore */ }
    return out;
  } catch (err) {
    console.warn('Live weather unavailable, trying snapshot.', err);
  }
  if (!snapshot) return null;
  try {
    const r = await fetch(snapshot);
    if (!r.ok) return null;
    const j = await r.json();
    return { source: 'snapshot', today, saved: j.saved, byRegion: j.byRegion };
  } catch { return null; }
}
