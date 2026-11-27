// One-off oracle: runs the legacy pure JS modules (js/insights.js, js/forecast.js, js/ask.js) on fixed inputs and
// writes what they produce to tests/fixtures/*.json. The Rust port is diffed against these files.
// Not shipped. Run:  node oracle/gen.mjs
import { readFileSync, writeFileSync } from 'node:fs';
import { assess, scenario, slopePerYear, corr, wxSum } from '../js/insights.js';
import { runForecast } from '../js/forecast.js';
import { brief, finding, lines, backtestLine, answer, route, QUESTIONS } from '../js/ask.js';
import { STR } from '../js/i18n.js';

const ROOT = new URL('..', import.meta.url).pathname;
const J = (p) => JSON.parse(readFileSync(ROOT + p, 'utf8'));
const out = (name, v) => writeFileSync(`${ROOT}tests/fixtures/${name}.json`, JSON.stringify(v, (k, x) => (x === Infinity ? 'Infinity' : x === -Infinity ? '-Infinity' : x), 1));
const TODAY = '2026-11-14';
const RANK = { ok: 0, watch: 1, alert: 2 };

writeFileSync(`${ROOT}tests/fixtures/strings.json`, JSON.stringify(STR, null, 1));

function statusCompute(regions, wx, manifest, fc) {
  const A = {};
  const sm = regions.regions.map((r) => r.series.soil_moist_pct?.at(-1)).filter((v) => v != null);
  const soilRef = sm.length > 1 ? sm.reduce((a, b) => a + b, 0) / sm.length : null;
  for (const r of regions.regions) A[r.id] = assess(r, wx?.[r.id] ?? null, regions.thresholds, { today: TODAY, refName: manifest.reference?.name, soilRef });
  const lake = fc ? regions.regions.find((r) => r.id === fc.region && r.reservoir) : null;
  const B = lake ? brief(fc, lake.reservoir, wx?.[lake.id] ?? null, TODAY, lake.name) : null;
  if (B) {
    const a = A[lake.id];
    const f = finding(B, regions.thresholds);
    const i = a.findings.findIndex((x) => x.key === 'reservoir');
    if (i >= 0) a.findings[i] = f; else a.findings.push(f);
    a.reservoir.finding = f;
    a.reservoir.level = f.level;
    a.findings.sort((x, y) => RANK[y.level] - RANK[x.level]);
    a.status = a.findings[0]?.level ?? 'ok';
    a.headline = a.findings[0];
  }
  return { A, B };
}

const fcOut = (F) => ({ ...F, params: undefined, backtest: undefined });

for (const [name, dir] of [['real', 'data/real/'], ['demo', 'data/']]) {
  const manifest = J(`${dir}manifest.json`);
  const regions = J(`${dir}regions.json`);
  const wx = J(`${dir}weather.json`).byRegion;
  const fc = manifest.forecast ? J(manifest.forecast) : null;
  for (const [tag, w] of [['wx', wx], ['nowx', null]]) {
    const { A, B } = statusCompute(regions, w, manifest, fc);
    out(`assess_${name}_${tag}`, { today: TODAY, A });
    if (!B) continue;
    const lake = regions.regions.find((r) => r.id === fc.region);
    const res = lake.reservoir;
    const lw = w?.[lake.id] ?? null;
    const runs = {};
    for (const s of [{}, { rain: -20 }, { irrigation: -20 }, { rain: -50, irrigation: 50 }, { rain: 30, irrigation: -50 }, { rain: 15, irrigation: 5 }]) {
      runs[JSON.stringify(s)] = fcOut(runForecast(fc, res, lw, TODAY, s));
    }
    const ans = {};
    for (const l of ['el', 'en']) {
      ans[l] = { lines: lines(B, l), backtest: backtestLine(B, l), answers: {} };
      for (const q of [...QUESTIONS.map((x) => x.id), 'none']) ans[l].answers[q] = answer(q, B, l);
    }
    out(`forecast_${name}_${tag}`, { today: TODAY, runs });
    out(`ask_${name}_${tag}`, { today: TODAY, ans, finding: finding(B, regions.thresholds), questions: QUESTIONS });
  }
  // what-if without a model (straight water balance), only where a reservoir exists
  const { A } = statusCompute(regions, wx, manifest, null);
  const sc = {};
  for (const r of regions.regions.filter((x) => x.reservoir?.volume?.length)) {
    for (const rain of [-50, -20, 0, 15, 30]) for (const irrigation of [-50, -10, 0, 25, 50]) {
      sc[`${r.id}|${rain}|${irrigation}`] = scenario(r, A[r.id], { rain, irrigation });
    }
  }
  out(`scenario_${name}`, sc);
}

// router
const routeIn = ['Πότε θα στερέψει;', 'how much water is there now', 'Κι αν δεν βρέξει;', 'Τι να κάνουμε;', 'is it accurate?', 'ΑΡΔΕΥΣΗ', 'hello', 'what should the region do', 'Ποσο σιγουρη ειναι;', 'next summer?', 'farm crops', 'Κατάσταση σήμερα'];
out('route', Object.fromEntries(routeIn.map((s) => [s, route(s)])));

// stats
const pts = J('data/points.geojson').features.map((f) => f.properties);
out('stats', {
  slope: pts.map((p) => ({ id: p.id, disp: slopePerYear(p.series.dates, p.series.disp_mm), ref: p.series.ref_mm ? slopePerYear(p.series.dates, p.series.ref_mm) : null })),
  corr: pts.map((p) => corr(p.series.hh_db, p.series.hv_db)),
  wxSum: wxSum(J('data/weather.json').byRegion.south, 'rain', '2026-03-01', '2026-04-15'),
});

// dates: every format the UI uses, every month and weekday
const OPTS = {
  dmy: { day: 'numeric', month: 'short', year: 'numeric' }, dm: { day: 'numeric', month: 'short' }, m: { month: 'long' },
  wdm: { weekday: 'long', day: 'numeric', month: 'long' }, dMy: { day: 'numeric', month: 'long', year: 'numeric' },
  w: { weekday: 'short' }, dmyy: { day: 'numeric', month: 'short', year: '2-digit' }, wdMy: { weekday: 'long', day: 'numeric', month: 'long', year: 'numeric' },
};
const dates = {};
for (let k = 0; k < 400; k += 3) {
  const iso = new Date(Date.UTC(2025, 9, 1 + k)).toISOString().slice(0, 10);
  dates[iso] = {};
  for (const l of ['el', 'en']) for (const [n, o] of Object.entries(OPTS)) dates[iso][`${l}.${n}`] = new Intl.DateTimeFormat(l === 'el' ? 'el-GR' : 'en-GB', o).format(new Date(`${iso}T00:00:00`));
}
out('dates', dates);

// numbers: JS toFixed / Math.round semantics
const nums = [0, 0.5, 1.5, 2.5, -0.5, -1.5, -2.5, 0.125, 1.005, 2.675, 1e-7, -1e-7, 12.345, -12.345, 0.05, 0.15, 0.25, 0.35, 99.95, -0.04, 1234.5678, 8.75, 9.2, -15.69];
out('numbers', nums.map((v) => ({ v, f0: v.toFixed(0), f1: v.toFixed(1), f2: v.toFixed(2), round: Math.round(v) })));

// validation stats (app.js renderValidation)
const val = J('data/validation.json');
{
  const x = val.pairs.map((p) => p.ref), y = val.pairs.map((p) => p.nisar);
  const mean = (a) => a.reduce((s, v) => s + v, 0) / a.length;
  const [mx, my] = [mean(x), mean(y)];
  const sxy = x.reduce((s, xi, i) => s + (xi - mx) * (y[i] - my), 0);
  const r2 = sxy ** 2 / (x.reduce((s, xi) => s + (xi - mx) ** 2, 0) * y.reduce((s, yi) => s + (yi - my) ** 2, 0));
  const rmse = Math.sqrt(mean(x.map((xi, i) => (y[i] - xi) ** 2)));
  out('validation', { n: x.length, r2, rmse, bias: my - mx });
}
console.log('fixtures written');
