// Speed of the legacy JS on the Status computation: every rule on every area, the lake forecast brief
// (3 ensemble runs) and all Ask answers in both languages. Same inputs as the Rust benchmark.
//   node oracle/bench.mjs
import { readFileSync } from 'node:fs';
import { assess } from '../js/insights.js';
import { brief, finding, answer, QUESTIONS } from '../js/ask.js';

const ROOT = new URL('..', import.meta.url).pathname;
const J = (p) => JSON.parse(readFileSync(ROOT + p, 'utf8'));
const regions = J('data/real/regions.json');
const wx = J('data/real/weather.json').byRegion;
const fc = J('data/real/forecast.json');
const TODAY = '2026-11-14';

function once() {
  const A = {};
  for (const r of regions.regions) A[r.id] = assess(r, wx[r.id], regions.thresholds, { today: TODAY, refName: 'EGMS', soilRef: null });
  const lake = regions.regions.find((r) => r.id === fc.region);
  const B = brief(fc, lake.reservoir, wx[lake.id], TODAY, lake.name);
  const f = finding(B, regions.thresholds);
  let n = f.text.en.length;
  for (const l of ['el', 'en']) for (const q of [...QUESTIONS.map((x) => x.id), 'none']) n += answer(q, B, l).text.length;
  return n + Object.keys(A).length;
}

for (let i = 0; i < 20; i++) once();           // warm-up (JIT)
const t = [];
for (let i = 0; i < 200; i++) { const t0 = process.hrtime.bigint(); once(); t.push(Number(process.hrtime.bigint() - t0) / 1e6); }
t.sort((a, b) => a - b);
console.log(JSON.stringify({ impl: 'JS (Node ' + process.version + ')', median_ms: t[100], p10_ms: t[20], p90_ms: t[180] }));
