// Lake forecast: the water balance fitted by tools/fit_forecast.py, run here day by day.
//
//   S    = how full the lake is, 0 (dry) .. 1 (fullest seen)
//   dS   = a * rain - k * ET0 * irrigation factor      a = a_wet (Nov–Mar) or a_dry (Apr–Oct)
//   area = A_max * S^q
//
// From the last radar pass to today (+7 days) the real weather is used (Open-Meteo, live or snapshot).
// After that, one scenario per past year: "what if the coming months have the weather of 2015, 2016, …".
// Each weather scenario runs three times, with the rain coefficient at 50%, 100% and 150% of the fitted value:
// the summer passes say little about how fast rain refills the lake, so that stays wide on purpose.
// The band is the 10th–90th percentile of all those runs, widened by the model's typical error (rmse).
// Dates (below the limit, refilled, next fall) are where the band's edges and its middle cross the limit.
// Nothing here is guessed by a language model: every number comes from the measurements and the weather.

const DAY = 864e5;
const dnum = (iso) => Date.parse(`${iso}T12:00:00Z`) / DAY;
const iso = (n) => new Date(n * DAY).toISOString().slice(0, 10);
const doy = (isoDate) => {                      // index in the 366-day climate arrays (Feb 29 = its own slot)
  const m = +isoDate.slice(5, 7);
  const d = +isoDate.slice(8, 10);
  const start = [0, 31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335];
  return start[m - 1] + d - 1;
};
const IRRIGATION = (isoDate) => { const md = isoDate.slice(5); return md >= '04-15' && md <= '09-30'; };
const WET = (isoDate) => { const m = +isoDate.slice(5, 7); return m >= 11 || m <= 3; };
export const RAIN_FACTORS = [0.5, 1, 1.5];
const pct = (arr, p) => { const s = [...arr].sort((a, b) => a - b); const i = (s.length - 1) * p; const lo = Math.floor(i); return s[lo] + (s[Math.ceil(i)] - s[lo]) * (i - lo); };

/**
 * @param fc     data/real/forecast.json
 * @param res    the lake's reservoir block from regions.json (dates, area_km2, limit)
 * @param wx     daily weather for the lake area { time[], rain[], et0[] } or null
 * @param today  'YYYY-MM-DD'
 * @param opt    { rain: % change of future rain, irrigation: % change of irrigation water, horizon: days after today }
 */
export function runForecast(fc, res, wx, today, { rain = 0, irrigation = 0, horizon = 365 } = {}) {
  const { a_wet, a_dry, k, q, amax } = fc.params;
  const limit = res.limit;
  const start = res.dates.at(-1);
  const area0 = res.area_km2.at(-1);
  const end = iso(dnum(today) + horizon);
  const known = new Map();                      // real weather by day (history + 7-day forecast)
  if (wx) wx.time.forEach((d, i) => { if (wx.rain[i] != null && wx.et0[i] != null) known.set(d, [wx.rain[i], wx.et0[i]]); });
  const lastKnown = wx ? wx.time.filter((d) => known.has(d)).at(-1) : null;

  // a scenario year y gives Oct–Dec from y and Jan–Sep from y+1 (one hydrological year)
  const years = fc.climate.years.filter((y) => fc.climate.days[y + 1]);
  const days = [];
  for (let n = dnum(start) + 1; n <= dnum(end); n++) days.push(iso(n));
  const fr = 1 + rain / 100;
  const fi = 1 + irrigation / 100;
  const futureFrom = today;                     // the what-if changes only apply from today on

  const runs = years.flatMap((y) => RAIN_FACTORS.map((f) => [y, f]));
  const members = runs.map(([y, af]) => {
    let s = (Math.min(area0, amax) / amax) ** (1 / q);
    const out = [];
    for (const d of days) {
      let w = d <= lastKnown ? known.get(d) : null;
      if (!w) {
        const yy = +d.slice(5, 7) >= 10 ? y : y + 1;
        const c = fc.climate.days[yy] || fc.climate.days[y];
        const i = doy(d);
        w = [c.rain[i] ?? 0, c.et0[i] ?? 0];
      }
      const future = d >= futureFrom;
      const r = w[0] * (future ? fr : 1);
      // the fitted loss k*ET0 is taken as 30% evaporation + 70% irrigation in the irrigation season (15 Apr – 30 Sep);
      // only the what-if uses that split: with no change the factor is 1, the fitted model as it is
      const kf = future && IRRIGATION(d) ? 0.3 + 0.7 * fi : 1;
      s = Math.min(1, Math.max(0, s + (WET(d) ? a_wet : a_dry) * af * r - k * w[1] * kf));
      out.push(amax * s ** q);
    }
    return { year: y, factor: af, area: out };
  });

  const err = fc.rmse;
  const p10 = [], p50 = [], p90 = [];
  days.forEach((_, i) => {
    const v = members.map((m) => m.area[i]);
    p50.push(pct(v, 0.5));
    p10.push(Math.max(0, pct(v, 0.1) - err));
    p90.push(Math.min(amax, pct(v, 0.9) + err));
  });

  // dates where a curve crosses the limit: down (first day below), up (first day back above)
  const ti = Math.max(0, days.indexOf(today));
  const down = (arr, from = 0) => { for (let i = from; i < arr.length; i++) if (arr[i] < limit) return i; return -1; };
  const up = (arr, from) => { for (let i = from; i < arr.length; i++) if (arr[i] >= limit) return i; return -1; };
  const at = (i) => (i >= 0 ? days[i] : null);
  // band edges: p10 is the dry side (falls first, refills last), p90 the wet side
  const span = (f) => { const e = f(p10, 'dry'); const m = f(p50, 'mid'); const l = f(p90, 'wet'); return m < 0 && e < 0 && l < 0 ? null : { early: at(e), mid: at(m), late: at(l) }; };
  const cross = (() => { const r = span((a) => down(a)); return r && { early: r.early, mid: r.mid, late: r.late }; })();
  const refill = p50[ti] < limit ? (() => { const r = span((a) => (a[ti] < limit ? up(a, ti) : ti)); return r && { early: r.late, mid: r.mid, late: r.early }; })() : null;
  // next summer's fall below the limit: the range is the middle half (25th–75th) of the runs' own dates
  const next = (() => {
    const ds = members.map((m) => { const b = m.area[ti] >= limit ? ti : up(m.area, ti); return b < 0 ? -1 : down(m.area, b); })
      .filter((i) => i >= 0).sort((x, y) => x - y);
    if (!ds.length) return null;
    const q = (f) => days[ds[Math.min(ds.length - 1, Math.floor(f * ds.length))]];
    // the middle date is where the median curve crosses (stable when a what-if moves some runs out of the set)
    const b50 = p50[ti] >= limit ? ti : up(p50, ti);
    const m50 = b50 < 0 ? -1 : down(p50, b50);
    return { early: q(0.25), mid: m50 >= 0 ? days[m50] : q(0.5), late: q(0.75), share: ds.length / members.length };
  })();
  const belowToday = members.filter((m) => m.area[ti] < limit).length / members.length;
  const june = days.findIndex((d) => d > today && d.slice(5) === '06-01');
  const nov = june < 0 ? -1 : days.findIndex((d, i) => i > june && d.slice(5) === '11-01');
  const lowNext = june < 0 ? null : Math.min(...p50.slice(june, nov < 0 ? undefined : nov));
  const daysBelowNext = june < 0 ? 0 : p50.slice(june, nov < 0 ? undefined : nov).filter((v) => v < limit).length;

  return {
    start, today, dates: days, p10, p50, p90, members: members.length, limit, amax,
    now: { p50: p50[ti], p10: p10[ti], p90: p90[ti] }, belowToday,
    cross, refill, next, lowNext, daysBelowNext,
    weatherUntil: lastKnown, rmse: err, params: fc.params, backtest: fc.backtest,
  };
}
