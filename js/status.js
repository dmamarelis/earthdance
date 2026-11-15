// "Status" view: the daily briefing for whoever runs the region.
// Left: what needs action today, area by area. Right: one area's conclusions, weather, water and alerts.
// The numbers come from data/regions.json + live weather; the sentences come from js/insights.js.

import { loadWeather, isoDay } from './weather.js';
import { assess, scenario } from './insights.js';
import { runForecast } from './forecast.js';
import { brief, finding as lakeFinding, lines as alertLines, backtestLine, answer, route, QUESTIONS } from './ask.js';

const COLORS = { alert: '#ff6b5e', watch: '#f4b94a', ok: '#7bd88f' };
const RANK = { ok: 0, watch: 1, alert: 2 };
const sum = (a) => a.reduce((s, v) => s + (v ?? 0), 0);
const dayNum = (iso) => Date.parse(`${iso}T00:00:00Z`) / 864e5;

export function createStatus(ctx) {
  const { $, $$, t, L, fmtDate, state, manifest } = ctx;
  const today = isoDay(new Date());
  const tf = (k, v) => t(k).replace(/\{(\w+)\}/g, (_, n) => v[n]);
  const short = (iso) => fmtDate(iso, { day: 'numeric', month: 'short' });
  const rate = (a) => (a.ground.rate == null ? '–' : signed(a.ground.rate));
  const signed = (v, d = 0) => { const s = v.toFixed(d); return +s === 0 ? s.replace('-', '') : `${v > 0 ? '+' : ''}${s}`.replace('-', '−'); };

  let data = { regions: [], thresholds: {} };
  let wx = null;
  let wxState = 'loading';            // loading | live | snapshot | none
  let A = {};                         // assessments by area id
  let map = null;
  let chart = null;
  const labels = {};
  const scn = { rain: 0, irrigation: 0 };   // what-if, in % change for next season
  const fc = ctx.forecast || null;          // fitted lake model (real data only): js/forecast.js
  let B = null;                             // forecast + what-ifs for the lake, the source of the alert and of "Ask"
  let asked = null;                         // last question asked

  const region = (id) => data.regions.find((r) => r.id === id);
  const sorted = () => [...data.regions].sort((a, b) => RANK[A[b.id].status] - RANK[A[a.id].status]);

  function compute() {
    A = {};
    const sm = data.regions.map((r) => r.series.soil_moist_pct?.at(-1)).filter((v) => v != null);
    const soilRef = sm.length > 1 ? sm.reduce((a, b) => a + b, 0) / sm.length : null;
    for (const r of data.regions) {
      A[r.id] = assess(r, wx?.byRegion?.[r.id] ?? null, data.thresholds, { today, refName: manifest.reference?.name, soilRef });
    }
    // with a fitted model, the lake finding comes from the forecast instead of the straight-line projection
    const lake = fc && wxState !== 'loading' ? data.regions.find((r) => r.id === fc.region && r.reservoir) : null;
    B = lake ? brief(fc, lake.reservoir, wx?.byRegion?.[lake.id] ?? null, today, lake.name) : null;
    if (B) {
      const a = A[lake.id];
      const f = lakeFinding(B, data.thresholds);
      const i = a.findings.findIndex((x) => x.key === 'reservoir');
      if (i >= 0) a.findings[i] = f; else a.findings.push(f);
      a.reservoir.finding = f;
      a.reservoir.level = f.level;
      a.findings.sort((x, y) => RANK[y.level] - RANK[x.level]);
      a.status = a.findings[0]?.level ?? 'ok';
      a.headline = a.findings[0];
    }
  }

  async function init(regionsData) {
    data = regionsData;
    compute();
    const first = data.regions.map((r) => r.series.dates[0]).sort()[0];
    const start = new Date(Date.parse(`${first}T12:00:00`) - 30 * 864e5);
    if (fc) state.rtab = 'h2o';
    loadWeather(data.regions, isoDay(start), manifest.weather?.snapshot).then((w) => {
      wx = w;
      wxState = w ? w.source : 'none';
      compute();
      render();
      refreshMap();
    });
  }

  // ───────────────────────────────────────────────────────── map
  function ring([lon, lat], km) {
    const pts = [];
    for (let i = 0; i <= 72; i++) {
      const a = (i / 72) * 2 * Math.PI;
      pts.push([lon + (km / (111.32 * Math.cos((lat * Math.PI) / 180))) * Math.cos(a), lat + (km / 110.57) * Math.sin(a)]);
    }
    return pts;
  }

  const geojson = () => ({
    type: 'FeatureCollection',
    features: data.regions.map((r) => ({
      type: 'Feature',
      properties: { id: r.id, status: A[r.id].status, sel: r.id === state.regionId },
      geometry: { type: 'Polygon', coordinates: [ring(r.center, r.radius_km)] },
    })),
  });

  function addToMap(m) {
    map = m;
    const color = ['match', ['get', 'status'], 'alert', COLORS.alert, 'watch', COLORS.watch, COLORS.ok];
    m.addSource('regions', { type: 'geojson', data: geojson() });
    m.addLayer({ id: 'regions-fill', type: 'fill', source: 'regions', layout: { visibility: 'none' }, paint: { 'fill-color': color, 'fill-opacity': ['case', ['get', 'sel'], 0.3, 0.13] } });
    m.addLayer({ id: 'regions-line', type: 'line', source: 'regions', layout: { visibility: 'none' }, paint: { 'line-color': color, 'line-width': ['case', ['get', 'sel'], 3, 1.6] } });
    m.on('click', 'regions-fill', (e) => ctx.selectRegion(e.features[0].properties.id));
    m.on('mouseenter', 'regions-fill', () => { m.getCanvas().style.cursor = 'pointer'; });
    m.on('mouseleave', 'regions-fill', () => { m.getCanvas().style.cursor = ''; });
    for (const r of data.regions) {
      const el = document.createElement('button');
      el.className = 'rg';
      el.addEventListener('click', (e) => { e.stopPropagation(); ctx.selectRegion(r.id); });
      const north = r.center[1] + r.radius_km / 110.57;
      new maplibregl.Marker({ element: el, anchor: 'bottom' }).setLngLat([r.center[0], north]).addTo(m);
      labels[r.id] = el;
    }
    m.addSource('scenario', { type: 'geojson', data: { type: 'FeatureCollection', features: [] } });
    m.addLayer({ id: 'scn-fill', type: 'fill', source: 'scenario', filter: ['==', ['get', 'k'], 'scn'], paint: { 'fill-color': '#5cc8f0', 'fill-opacity': 0.28 } });
    m.addLayer({ id: 'scn-line', type: 'line', source: 'scenario', filter: ['==', ['get', 'k'], 'scn'], paint: { 'line-color': '#5cc8f0', 'line-width': 2.5 } });
    m.addLayer({ id: 'scn-now', type: 'line', source: 'scenario', filter: ['==', ['get', 'k'], 'now'], paint: { 'line-color': '#ffffff', 'line-width': 1.5, 'line-dasharray': [3, 2] } });
    setVisible(state.mode === 'status');
    refreshMap();
  }

  function refreshMap() {
    if (!map) return;
    map.getSource('regions')?.setData(geojson());
    for (const r of data.regions) {
      const el = labels[r.id];
      // classList, not className: MapLibre keeps its own positioning classes on the marker element
      for (const s of ['alert', 'watch', 'ok']) el.classList.toggle(`st-${s}`, A[r.id].status === s);
      el.classList.toggle('is-sel', r.id === state.regionId);
      el.textContent = L(r.name);
    }
  }

  function setVisible(on) {
    if (!map) return;
    for (const id of ['regions-fill', 'regions-line', 'scn-fill', 'scn-line', 'scn-now']) map.setLayoutProperty(id, 'visibility', on ? 'visible' : 'none');
  }

  // ───────────────────────────────────────────────────────── left: daily briefing
  function renderBrief() {
    const ask = $('#ask');                  // "Ask" sits inside the briefing: take it out before the briefing is redrawn
    if (ask && $('#brief').contains(ask)) $('#brief').after(ask);
    const count = { alert: 0, watch: 0, ok: 0 };
    for (const r of data.regions) count[A[r.id].status]++;
    const badge = $('#alertCount');
    badge.hidden = !count.alert;
    badge.textContent = count.alert;

    const title = count.alert ? tf(count.alert === 1 ? 'brief.h.alert1' : 'brief.h.alertN', { n: count.alert })
      : count.watch ? tf(count.watch === 1 ? 'brief.h.watch1' : 'brief.h.watchN', { n: count.watch })
        : t('brief.h.ok');
    const lastPass = data.regions.map((r) => r.series.dates.at(-1)).sort().at(-1);
    const ago = Math.round(dayNum(today) - dayNum(lastPass));
    const wxLine = wxState === 'snapshot' ? tf('brief.wx.snapshot', { d: fmtDate(wx.saved) }) : t(`brief.wx.${wxState}`);

    const todo = [];
    for (const r of sorted()) for (const f of A[r.id].findings) if (f.level !== 'ok') todo.push({ r, f });
    todo.sort((a, b) => RANK[b.f.level] - RANK[a.f.level]);

    $('#brief').innerHTML = `
      <div class="brief-top">
        <div class="brief-kicker">${t('brief.kicker')} · ${fmtDate(today, { weekday: 'long', day: 'numeric', month: 'long' })}</div>
        <button class="chip-btn report-btn" data-report><svg viewBox="0 0 24 24"><path d="M12 4v11M7 10l5 5 5-5M5 20h14"/></svg><span>${t('report.btn')}</span></button>
      </div>
      <h1 class="brief-title">${title}</h1>
      <div class="brief-counts">
        ${['alert', 'watch', 'ok'].map((s) => `<span class="st st-${s}">${count[s]} ${t(`st.${s}`)}</span>`).join('')}
      </div>
      <dl class="fresh">
        <dt>${t('brief.radar')}</dt><dd>${tf('brief.radarText', { d: fmtDate(lastPass), n: ago })}${manifest.demo ? ` · <em>${t('demo.short')}</em>` : ''}</dd>
        <dt>${t('brief.wx')}</dt><dd>${wxLine}</dd>
      </dl>

      <h2 class="sec">${t('brief.todo')}</h2>
      <div class="todo">
        ${todo.length ? todo.map(({ r, f }) => `
          <button class="todo-row" data-region="${r.id}">
            <i class="dot st-${f.level}"></i>
            <span><b>${L(f.title)}</b><small>${L(r.name)}</small></span>
            <svg viewBox="0 0 24 24"><path d="M9 5l7 7-7 7"/></svg>
          </button>`).join('') : `<p class="empty">${t('brief.none')}</p>`}
      </div>

      <div id="askSlot"></div>
      <h2 class="sec">${t('brief.areas')}</h2>
      <div class="areas">
        ${sorted().map((r) => {
    const a = A[r.id];
    const ref = a.reference;
    return `
          <button class="area ${r.id === state.regionId ? 'is-active' : ''}" data-region="${r.id}">
            <i class="dot st-${a.status}"></i>
            <span class="a-name">${L(r.name)}</span>
            <span class="a-metrics">
              ${a.ground.rate != null ? `<span title="${t('rg.tile.rate')}">${signed(a.ground.rate)} ${t('unit.mmyr')}</span>` : ''}
              ${ref ? `<span class="${ref.agrees ? 'yes' : 'no'}" title="${L(ref.finding.title)}">${manifest.reference?.name || 'Ref'} ${ref.agrees ? '✓' : '✗'}</span>` : ''}
              ${a.weather ? `<span title="${t('rg.rain7')}">${a.weather.rain7.toFixed(0)} mm</span>` : ''}
            </span>
          </button>`;
  }).join('')}
      </div>
      <p class="brief-foot">${t('brief.foot')}</p>`;
    if (ask && $('#askSlot')) $('#askSlot').replaceWith(ask);
  }

  // ───────────────────────────────────────────────────────── right: one area
  function forecastHTML(w) {
    const i = w.time.indexOf(today);
    if (i < 0) return '';
    const days = w.time.slice(i, i + 7);
    const max = Math.max(5, ...days.map((_, k) => w.rain[i + k] ?? 0));
    return `<div class="forecast">${days.map((d, k) => {
      const rain = w.rain[i + k] ?? 0;
      return `<div class="fc ${k === 0 ? 'now' : ''}">
        <span class="fc-day">${k === 0 ? t('fc.today') : fmtDate(d, { weekday: 'short' })}</span>
        <span class="fc-t">${w.tmax[i + k] != null ? `${Math.round(w.tmax[i + k])}°` : '–'}</span>
        <span class="fc-bar"><i style="height:${(rain / max) * 100}%"></i></span>
        <span class="fc-r">${rain >= 0.1 ? rain.toFixed(rain < 10 ? 1 : 0) : '0'}</span>
      </div>`;
    }).join('')}</div>`;
  }

  function eventsHTML(a) {
    if (!a.effects) return '';
    const ev = a.effects.events.slice(0, 5);
    const rows = ev.map((e) => {
      const what = e.type === 'rain' ? tf('ev.rain', { v: e.value.toFixed(0) }) : tf('ev.heat', { n: e.days, v: e.value.toFixed(0) });
      const after = [];
      if (e.pass) {
        if (e.dHH != null) after.push(`HH ${signed(e.dHH, 1)} dB`);
        if (e.dWater) after.push(`${t('ev.water')} ${signed(e.dWater)} km²`);
        if (e.dDisp != null && Math.abs(e.dDisp) >= 1) after.push(`${t('ev.ground')} ${signed(e.dDisp)} mm`);
      }
      return `<li class="ev ev-${e.type}">
        <time>${fmtDate(e.date)}</time><b>${what}</b>
        ${e.pass ? `<span>${tf('ev.after', { d: short(e.pass) })} ${after.join(' · ') || '—'}</span>` : ''}
      </li>`;
    }).join('');
    return `<h3 class="sec">${t('ev.title')}</h3>${rows ? `<ul class="events">${rows}</ul>` : `<p class="empty">${t('ev.none')}</p>`}`;
  }

  function alertHTML(r, a) {
    if (B && r.id === fc.region) {
      const [m, c, d] = alertLines(B, state.lang);
      const lv = a.reservoir.level === 'ok' ? 'watch' : a.reservoir.level;
      return `<section class="card alertcard lv-${lv}">
        <div class="voice-tag">${t('al.title')}</div>
        <h3>${L(a.reservoir.finding.title)}</h3>
        <dl class="al3">
          <dt>${t('al3.seen')}</dt><dd>${m}</dd>
          <dt>${t('al3.coming')}</dt><dd>${c}</dd>
          <dt>${t('al3.do')}</dt><dd>${d}</dd>
        </dl>
        <p class="al-bt">${backtestLine(B, state.lang)}</p>
        <p class="rule"><b>${t('al.rule')}:</b> ${L(r.reservoir.rule)}</p>
      </section>`;
    }
    const res = a.reservoir;
    if (!res || res.level === 'ok') return '';
    return `<section class="card alertcard lv-${res.level}">
      <div class="voice-tag">${t('al.title')}</div>
      <h3>${L(res.finding.title)}</h3>
      <div class="gauge"><i style="width:${Math.min(100, res.pct)}%"></i><b style="left:${(r.reservoir.limit / r.reservoir.capacity) * 100}%"></b></div>
      <div class="gauge-scale"><span>0</span><span>${t('rc.limit')} ${r.reservoir.limit} ${r.reservoir.unit}</span><span>${r.reservoir.capacity}</span></div>
      ${res.warnedOn ? `<p>${tf('al.warned', { w: data.thresholds.reservoir_warn_days, d: fmtDate(res.warnedOn) })}</p>` : ''}
      <p class="rule"><b>${t('al.rule')}:</b> ${L(r.reservoir.rule)}</p>
    </section>`;
  }

  function renderRegion() {
    const r = region(state.regionId);
    const box = $('#regionView');
    if (!r) { box.innerHTML = ''; return; }
    const a = A[r.id];
    const w = wx?.byRegion?.[r.id];
    const th = data.thresholds;
    const ago = Math.round(dayNum(today) - dayNum(a.lastPass));
    const ref = a.reference;
    const lead = !!(B && r.id === fc.region);      // the lake with a forecast: alert and chart first
    const charts = `
      <div class="tabs rtabs">
        ${['wx', 'fx', 'h2o'].map((k) => `<button class="tab ${k === state.rtab ? 'is-active' : ''}" data-rtab="${k}">${t(`rtab.${k}`)}</button>`).join('')}
      </div>
      <div class="chart-wrap"><canvas id="rChart"></canvas></div>
      <div class="chart-note" id="rNote"></div>`;

    box.innerHTML = `
      <div class="insp-head">
        <div>
          <span class="st st-${a.status}">${t(`st.${a.status}`)}</span>
          <h2>${L(r.name)}</h2>
          <div class="coords">${tf('rg.pass', { d: fmtDate(a.lastPass), n: ago })}</div>
        </div>
        <button class="x" data-close-right aria-label="Close"><svg viewBox="0 0 24 24"><path d="M6 6l12 12M18 6L6 18"/></svg></button>
      </div>

      ${lead ? alertHTML(r, a) + charts : ''}

      ${w ? forecastHTML(w) : `<p class="empty">${t(wxState === 'loading' ? 'brief.wx.loading' : 'rnote.nowx')}</p>`}

      <div class="tiles">
        ${a.soil ? `<div class="tile ${a.soil.finding.level !== 'ok' ? 'warn' : ''}"><div class="t-label">${t('rg.tile.soil')}</div><div class="t-val">${a.soil.now.toFixed(0)}<small>%</small></div></div>` : ''}
        ${a.wetland ? `<div class="tile ${a.wetland.level !== 'ok' ? 'neg' : ''}"><div class="t-label">${t('rg.tile.cleared')}</div><div class="t-val">${a.wetland.ha.toFixed(0)}<small>ha</small></div></div>` : ''}
        ${a.reservoir ? `<div class="tile ${a.reservoir.level !== 'ok' ? 'neg' : ''}"><div class="t-label">${t('rg.tile.lake')}</div><div class="t-val">${(r.reservoir.area_km2?.at(-1) ?? a.reservoir.now).toFixed(0)}<small>${r.reservoir.area_km2 ? 'km²' : r.reservoir.unit}</small></div></div>` : ''}
        ${a.ground.rate != null ? `<div class="tile ${a.ground.level !== 'ok' ? 'neg' : ''}"><div class="t-label">${t('rg.tile.rate')}</div><div class="t-val">${signed(a.ground.rate)}<small>${t('unit.mmyr')}</small></div></div>` : ''}
        ${a.ground.rate != null ? `<div class="tile ${ref && !ref.agrees ? 'warn' : ''}"><div class="t-label">${tf('rg.tile.egms', { ref: manifest.reference?.name || 'Ref' })}</div><div class="t-val">${ref ? signed(ref.diff, 1) : '–'}<small>${t('unit.mmyr')}</small></div></div>` : ''}
        <div class="tile"><div class="t-label">${t('rg.tile.rain')}</div><div class="t-val">${a.weather ? a.weather.rain30.toFixed(0) : '–'}<small>mm</small></div></div>
      </div>

      <h3 class="sec">${t('rg.findings')}</h3>
      <div class="findings">
        ${a.findings.filter((f) => !(lead && f.key === 'reservoir')).map((f) => `
          <article class="finding lv-${f.level}">
            <h4>${L(f.title)}</h4>
            <p>${L(f.text)}</p>
            ${f.action ? `<div class="action"><b>${t('action.title')}</b>${L(f.action)}</div>` : ''}
            <small>${L(f.basis)}</small>
          </article>`).join('')}
      </div>

      ${lead ? '' : alertHTML(r, a) + charts}

      ${eventsHTML(a)}

      <details class="card unknowns">
        <summary>${t('how.title')}</summary>
        <ul>
          <li>${t('how.text')}</li>
          ${a.ground.rate != null ? `<li>${tf('how.sink', { a: th.sink_watch_mm_yr, b: th.sink_alert_mm_yr })}</li>
          <li>${tf('how.egms', { a: th.egms_tolerance_mm_yr, ref: manifest.reference?.name || 'Ref' })}</li>` : ''}
          <li>${tf('how.res', { a: th.reservoir_warn_days })}</li>
          <li>${tf('how.wx', { a: th.heavy_rain_mm_day, b: th.heat_c, c: th.dry_spell_days })}</li>
          ${manifest.demo ? `<li class="demo-li">${t('how.demo')}</li>` : ''}
        </ul>
      </details>`;
    chart?.destroy();
    chart = null;
    renderChart();
  }

  // ───────────────────────────────────────────────────────── charts
  function renderChart() {
    const r = region(state.regionId);
    const canvas = $('#rChart');
    if (!r || !canvas) return;
    const s = r.series;
    const w = wx?.byRegion?.[r.id];
    const a = A[r.id];
    let cfg = null;
    let note = '';
    const grid = { color: 'rgba(255,255,255,.06)' };
    const axis = (title, extra = {}) => ({ grid, border: { display: false }, ticks: { maxTicksLimit: 6 }, title: { display: true, text: title, font: { size: 10 } }, ...extra });
    const right = (title) => axis(title, { position: 'right', grid: { display: false } });
    const line = (label, d, color, extra = {}) => ({ type: 'line', label, data: d, borderColor: color, backgroundColor: color, borderWidth: 2, tension: 0.3, pointRadius: 0, pointHoverRadius: 4, ...extra });

    if (state.rtab === 'wx') {
      if (w) {
        const weeks = [];
        let bal = 0;
        for (let i = 0; i < w.time.length; i += 7) {
          bal += sum(w.rain.slice(i, i + 7)) - sum(w.et0.slice(i, i + 7));
          weeks.push({ date: w.time[i], rain: sum(w.rain.slice(i, i + 7)), bal, future: w.time[i] > today });
        }
        cfg = {
          labels: weeks.map((k) => short(k.date)),
          datasets: [
            line(t('rc.balance'), weeks.map((k) => k.bal), COLORS.watch, { yAxisID: 'y1' }),
            { type: 'bar', label: t('rc.rainWeek'), data: weeks.map((k) => k.rain), backgroundColor: weeks.map((k) => (k.future ? 'rgba(92,200,240,.3)' : 'rgba(92,200,240,.85)')), borderRadius: 2, yAxisID: 'y' },
          ],
          scales: { y: axis('mm'), y1: right('mm') },
        };
        note = t('rnote.wx');
      } else note = t('rnote.nowx');
    } else if (state.rtab === 'fx') {
      cfg = {
        labels: s.dates.map(short),
        datasets: [
          line('HH · NISAR (dB)', s.hh_db, '#e9edf3', { pointRadius: 2.5, spanGaps: true }),
          ...(s.vv_db ? [line('VV · Sentinel-1 (dB)', s.vv_db, '#c9a7ff', { borderDash: [5, 4], borderWidth: 1.5 })] : []),
          ...(a.effects ? [{ type: 'bar', label: t('rc.rainPass'), data: a.effects.passes.map((p) => p.rain), backgroundColor: 'rgba(92,200,240,.55)', borderRadius: 3, yAxisID: 'y1' }] : []),
        ],
        scales: { y: axis('dB'), ...(a.effects ? { y1: right('mm') } : {}) },
      };
      note = t(a.effects ? 'rnote.fx' : 'rnote.nowx');
    } else if (r.reservoir && B && r.id === fc.region) {
      const F = B.base;
      const res = r.reservoir;
      const first = fc.fit.dates[0];
      const all = [];
      // shown up to 1 October after next summer: far enough to see the next fall below the limit
      const until = F.dates.find((d) => d > today && d.slice(5) === '10-01' && d.slice(0, 4) > today.slice(0, 4)) || F.dates.at(-1);
      for (let n = dayNum(first); n <= dayNum(until); n++) all.push(new Date(n * 864e5).toISOString().slice(0, 10));
      const off = all.indexOf(F.dates[0]);
      const fcast = (arr) => all.map((d, i) => (i >= off ? arr[i - off] : null));
      const meas = all.map((d) => { const i = res.dates.indexOf(d); return i >= 0 ? res.area_km2[i] : null; });
      const ti = all.indexOf(today);
      cfg = {
        labels: all.map((d) => fmtDate(d, { day: 'numeric', month: 'short', year: '2-digit' })),
        datasets: [
          line(t('rc.band'), fcast(F.p90), 'rgba(92,200,240,0)', { fill: false, pointHoverRadius: 0 }),
          line(t('rc.band'), fcast(F.p10), 'rgba(92,200,240,.35)', { fill: '-1', backgroundColor: 'rgba(92,200,240,.2)', borderWidth: 0, pointHoverRadius: 0 }),
          line(t('rc.forecast'), fcast(F.p50), '#5cc8f0', { borderDash: [5, 4], borderWidth: 1.8 }),
          line(t('rc.measured'), meas, '#ffffff', { showLine: false, pointRadius: 3.5, pointHoverRadius: 5 }),
          line(`${t('rc.limit')} (km²)`, all.map(() => res.limit), COLORS.alert, { borderDash: [6, 4], borderWidth: 1.5 }),
        ],
        scales: { y: axis('km²', { beginAtZero: true, max: Math.ceil(F.amax / 5) * 5 }) },
        plugins: [{
          id: 'today',
          afterDatasetsDraw(c) {
            if (ti < 0) return;
            const x = c.scales.x.getPixelForValue(ti);
            const { top, bottom } = c.chartArea;
            const g = c.ctx;
            g.save();
            g.strokeStyle = 'rgba(255,255,255,.7)'; g.setLineDash([2, 3]); g.lineWidth = 1;
            g.beginPath(); g.moveTo(x, top); g.lineTo(x, bottom); g.stroke();
            g.setLineDash([]); g.fillStyle = '#fff'; g.font = '600 10px system-ui'; g.textAlign = 'center';
            g.fillText(t('rc.today'), x, top + 9);
            g.restore();
          },
        }],
        legendFilter: (it) => it.datasetIndex !== 0,
      };
      note = t('rnote.h2o.fc');
    } else if (r.reservoir) {
      const res = r.reservoir;
      const proj = a.reservoir.crossDate;
      const dates = proj ? [...res.dates, proj] : res.dates;
      const n = res.volume.length;
      cfg = {
        labels: dates.map(short),
        datasets: [
          line(`${t('rc.volume')} (${res.unit})`, res.volume, '#5cc8f0', { fill: 'origin', backgroundColor: 'rgba(92,200,240,.16)', pointRadius: 2.5 }),
          ...(proj ? [line(t('rc.projection'), dates.map((_, i) => (i === n - 1 ? res.volume[n - 1] : i === n ? res.limit : null)), '#5cc8f0', { borderDash: [4, 4], pointRadius: 3 })] : []),
          line(`${t('rc.limit')} (${res.unit})`, dates.map(() => res.limit), COLORS.alert, { borderDash: [6, 4], borderWidth: 1.5 }),
        ],
        scales: { y: axis(res.unit, { beginAtZero: true }) },
      };
      note = t('rnote.h2o.res');
    } else {
      const wet = s.water_km2?.some((v) => v > 0);
      cfg = {
        labels: s.dates.map(short),
        datasets: [
          line(t('rc.loss'), s.vol_loss_hm3, COLORS.alert, { fill: 'origin', backgroundColor: 'rgba(255,107,94,.16)', pointRadius: 2.5 }),
          ...(wet ? [{ type: 'bar', label: t('rc.water'), data: s.water_km2, backgroundColor: 'rgba(92,200,240,.55)', borderRadius: 3, yAxisID: 'y1' }] : []),
        ],
        scales: { y: axis('hm³', { beginAtZero: true }), ...(wet ? { y1: right('km²') } : {}) },
      };
      note = t('rnote.h2o.aq');
    }

    $('#rNote').textContent = note + (manifest.demo && state.rtab !== 'wx' ? ` ${t('demo.warn')}` : '');
    $$('[data-rtab]').forEach((b) => b.classList.toggle('is-active', b.dataset.rtab === state.rtab));
    chart?.destroy();
    chart = null;
    canvas.parentElement.hidden = !cfg;
    if (!cfg) return;
    chart = new Chart(canvas, {
      data: { labels: cfg.labels, datasets: cfg.datasets },
      plugins: cfg.plugins || [],
      options: {
        responsive: true, maintainAspectRatio: false, animation: { duration: 350 },
        interaction: { mode: 'index', intersect: false },
        plugins: {
          legend: { position: 'bottom', labels: { padding: 12, ...(cfg.legendFilter ? { filter: cfg.legendFilter } : {}) } },
          ...(cfg.legendFilter ? { tooltip: { filter: (it) => it.raw != null && it.datasetIndex > 1 } } : {}),
        },
        scales: { x: { grid: { display: false }, border: { color: 'rgba(255,255,255,.12)' }, ticks: { maxRotation: 0, autoSkipPadding: 16, ...(cfg.legendFilter ? { maxTicksLimit: 8 } : {}) } }, ...cfg.scales },
      },
    });
  }

  // ───────────────────────────────────────────────────────── ask
  function renderAsk() {
    const box = $('#ask');
    if (!box) return;
    box.hidden = !B;
    if (!B) { box.innerHTML = ''; return; }
    box.innerHTML = `
      <h2 class="sec">${t('ask.title')}</h2>
      <div class="ask card">
        <div class="ask-chips">${QUESTIONS.map((q) => `<button class="ask-chip ${asked?.id === q.id ? 'is-on' : ''}" data-ask="${q.id}">${q[state.lang] || q.el}</button>`).join('')}</div>
        <form class="ask-form" id="askForm"><input id="askInput" type="text" autocomplete="off" placeholder="${t('ask.ph')}"><button type="submit" aria-label="${t('ask.send')}"><svg viewBox="0 0 24 24"><path d="M5 12h14M13 6l6 6-6 6"/></svg></button></form>
        <div class="ask-out" id="askOut" aria-live="polite"></div>
        <p class="scn-note">${t('ask.note')}</p>
      </div>`;
    if (asked) showAnswer(asked, false);
  }

  let typing = 0;
  function showAnswer(q, animate = true) {
    asked = q;
    const out = $('#askOut');
    if (!out || !B) return;
    $$('[data-ask]').forEach((b) => b.classList.toggle('is-on', b.dataset.ask === q.id));
    const a = answer(q.id, B, state.lang);
    const html = `<p class="ask-q">${q.text}</p>${a.text.map((p) => `<p>${p}</p>`).join('')}${a.sources ? `<small>${a.sources}</small>` : ''}`;
    const run = ++typing;
    if (!animate || matchMedia('(prefers-reduced-motion: reduce)').matches) { out.innerHTML = html; return; }
    // reveal word by word: the answer is ready at once, this only makes it readable on video
    out.innerHTML = `<p class="ask-q">${q.text}</p><p class="ask-wait">${t('ask.thinking')}</p>`;
    const words = a.text.map((p) => p.split(' '));
    let pi = 0;
    let wi = 0;
    setTimeout(function step() {
      if (run !== typing) return;
      if (pi >= words.length) { out.innerHTML = html; return; }
      wi = Math.min(wi + 3, words[pi].length);
      out.innerHTML = `<p class="ask-q">${q.text}</p>${words.slice(0, pi).map((w) => `<p>${w.join(' ')}</p>`).join('')}<p>${words[pi].slice(0, wi).join(' ')}</p>`;
      if (wi >= words[pi].length) { pi++; wi = 0; }
      setTimeout(step, 28);
    }, 450);
  }

  // ───────────────────────────────────────────────────────── what if
  const lakeRegion = () => data.regions.find((r) => r.reservoir?.volume?.length);

  function renderScenario() {
    const r = lakeRegion();
    const box = $('#scenario');
    if (!r) { box.innerHTML = ''; return; }
    box.innerHTML = `
      <h2 class="sec">${t('scn.title')}</h2>
      <div class="scn card">
        <p class="scn-q">${tf(B && r.id === fc?.region ? 'scn.q.model' : 'scn.q', { lake: L(r.reservoir.name) })}</p>
        <label class="scn-row"><span>${t('scn.rain')}</span><output id="scnRainOut"></output>
          <input type="range" id="scnRain" min="-50" max="30" step="5" value="${scn.rain}"></label>
        <label class="scn-row"><span>${t('scn.irr')}</span><output id="scnIrrOut"></output>
          <input type="range" id="scnIrr" min="-50" max="50" step="5" value="${scn.irrigation}"></label>
        <div class="scn-out" id="scnOut"></div>
        <button class="scn-reset" id="scnReset">${t('scn.reset')}</button>
        <p class="scn-note">${t(B && r.id === fc?.region ? 'scn.note.model' : 'scn.note')}${manifest.demo ? ` ${t('demo.warn')}` : ''}</p>
      </div>`;
    updateScenario();
  }

  /** The what-if run through the fitted model: same shape as scenario() so the map and the report can use either. */
  function modelScenario(r) {
    const F = runForecast(fc, r.reservoir, wx?.byRegion?.[r.id] ?? null, today, scn);
    const sep = F.dates.findIndex((d) => d > today && d.slice(5) === '06-01');
    const area = sep >= 0 ? F.p50[sep] : F.lowNext;
    return {
      model: true, F, base: B.base, low: area, limit: r.reservoir.limit, unit: 'km²',
      area: null, areaNow: r.reservoir.area_km2.at(-1), daysBelow: F.daysBelowNext, next: F.next?.mid,
      scale: Math.sqrt(Math.max(area, 0.3) / r.reservoir.area_km2.at(-1)), sink: null,
    };
  }

  function updateScenario() {
    const r = lakeRegion();
    if (!r || !$('#scnOut')) return;
    const s = B && r.id === fc.region ? modelScenario(r) : scenario(r, A[r.id], scn);
    const pct = (v) => `${v > 0 ? '+' : ''}${v}%`.replace('-', '−');
    $('#scnRainOut').textContent = pct(scn.rain);
    $('#scnIrrOut').textContent = pct(scn.irrigation);
    const bad = s.low < s.limit;
    if (s.model) {
      const nd = (iso) => (iso ? fmtDate(iso, { day: 'numeric', month: 'short', year: 'numeric' }) : '–');
      const sepBase = (() => { const i = s.base.dates.findIndex((d) => d > today && d.slice(5) === '06-01'); return i >= 0 ? s.base.p50[i] : null; })();
      $('#scnOut').innerHTML = `
        <div class="scn-kpi ${s.next ? 'bad' : 'good'}"><span>${t('scn.next')}</span><b>${nd(s.next)}</b><em>${t('scn.base')} ${nd(s.base.next?.mid)}</em></div>
        <div class="scn-kpi ${bad ? 'bad' : 'good'}"><span>${t('scn.sep')}</span><b>${s.low.toFixed(0)} <small>km²</small></b><em>${t('rc.limit')} ${s.limit} · ${t('scn.base')} ${sepBase == null ? '–' : sepBase.toFixed(0)}</em></div>
        <div class="scn-kpi ${s.daysBelow ? 'bad' : 'good'}"><span>${t('scn.days')}</span><b>${s.daysBelow}</b><em>${s.daysBelow ? t('scn.fine') : t('scn.nofine')} · ${t('scn.base')} ${s.base.daysBelowNext}</em></div>`;
    } else $('#scnOut').innerHTML = `
      <div class="scn-kpi ${bad ? 'bad' : 'good'}"><span>${t('scn.low')}</span><b>${s.low.toFixed(0)} <small>${s.unit}</small></b><em>${t('rc.limit')} ${s.limit} · ${t('scn.same')} ${s.base.toFixed(0)}</em></div>
      ${s.area != null ? `<div class="scn-kpi"><span>${t('scn.area')}</span><b>${s.area.toFixed(0)} <small>km²</small></b><em>${t('scn.now')} ${s.areaNow.toFixed(0)}</em></div>` : ''}
      <div class="scn-kpi ${s.daysBelow ? 'bad' : 'good'}"><span>${t('scn.days')}</span><b>${s.daysBelow}</b><em>${s.daysBelow ? t('scn.fine') : t('scn.nofine')}</em></div>
      ${s.sink != null ? `<div class="scn-kpi"><span>${tf('scn.sink', { area: L(r.name) })}</span><b>${signed(s.sink)} <small>${t('unit.mmyr')}</small></b></div>` : ''}`;
    // map: today's shoreline (dashed) and the scenario's (filled)
    const ring = r.reservoir.outline;
    const src = map?.getSource('scenario');
    if (src && ring) {
      const n = ring.length - 1;
      const cx = ring.slice(0, n).reduce((a, p) => a + p[0], 0) / n;
      const cy = ring.slice(0, n).reduce((a, p) => a + p[1], 0) / n;
      const scaled = ring.map(([x, y]) => [cx + (x - cx) * s.scale, cy + (y - cy) * s.scale]);
      const changed = scn.rain || scn.irrigation;
      src.setData({ type: 'FeatureCollection', features: changed ? [
        { type: 'Feature', properties: { k: 'now' }, geometry: { type: 'LineString', coordinates: ring } },
        { type: 'Feature', properties: { k: 'scn' }, geometry: { type: 'Polygon', coordinates: [scaled] } },
      ] : [] });
    }
    return s;
  }

  // ───────────────────────────────────────────────────────── printable regional brief
  async function exportReport() {
    const btn = $('[data-report]');
    if (btn) btn.disabled = true;
    const img = await ctx.snapshot?.();
    const lastPass = data.regions.map((r) => r.series.dates.at(-1)).sort().at(-1);
    const ref = manifest.reference?.name || 'Ref';
    const lake = lakeRegion();
    const s = lake ? (B && lake.id === fc.region ? modelScenario(lake) : scenario(lake, A[lake.id], scn)) : null;
    const rows = sorted().map((r) => {
      const a = A[r.id];
      return `<tr><td><i class="dot st-${a.status}"></i> ${L(r.name)}</td><td>${t(`st.${a.status}`)}</td>
        <td>${rate(a)}</td><td>${a.reference ? `${signed(a.reference.diff, 1)} ${a.reference.agrees ? '✓' : '✗'}` : '–'}</td>
        <td>${a.soil ? `${a.soil.now.toFixed(0)}%` : '–'}</td><td>${a.weather ? a.weather.rain30.toFixed(0) : '–'}</td></tr>`;
    }).join('');
    const issues = sorted().flatMap((r) => A[r.id].findings.filter((f) => f.level !== 'ok').map((f) => ({ r, f })));
    $('#report').innerHTML = `
      <header>
        <div><small>EarthDance · ${t('report.kicker')}</small><h1>${L(manifest.site.name)} — ${t('report.title')}</h1></div>
        <div class="r-date">${fmtDate(today, { weekday: 'long', day: 'numeric', month: 'long', year: 'numeric' })}</div>
      </header>
      ${manifest.demo ? `<p class="r-demo">${t('report.demo')}</p>` : ''}
      <p class="r-fresh">${t('brief.radar')}: ${tf('brief.radarText', { d: fmtDate(lastPass), n: Math.round(dayNum(today) - dayNum(lastPass)) })} · ${t('brief.wx')}: ${wxState === 'snapshot' ? tf('brief.wx.snapshot', { d: fmtDate(wx.saved) }) : t(`brief.wx.${wxState}`)}</p>
      ${img ? `<img class="r-map" src="${img}" alt="">` : ''}
      <h2>${t('report.summary')}</h2>
      <table><thead><tr><th>${t('report.area')}</th><th>${t('report.status')}</th><th>${t('rg.tile.rate')} (${t('unit.mmyr')})</th><th>Δ ${ref}</th><th>${t('rg.tile.soil')}</th><th>${t('rg.tile.rain')} (mm)</th></tr></thead><tbody>${rows}</tbody></table>
      <h2>${t('report.actions')}</h2>
      ${issues.length ? issues.map(({ r, f }) => `
        <div class="r-item lv-${f.level}">
          <h3>${L(r.name)} · ${L(f.title)}</h3>
          <p>${L(f.text)}</p>
          ${f.action ? `<p class="r-act"><b>${t('action.title')}:</b> ${L(f.action)}</p>` : ''}
          <p class="r-basis">${L(f.basis)}</p>
        </div>`).join('') : `<p>${t('brief.none')}</p>`}
      ${B ? `<h2>${t('report.forecast')}</h2>${alertLines(B, state.lang).map((x, i) => `<p><b>${t(['al3.seen', 'al3.coming', 'al3.do'][i])}:</b> ${x}</p>`).join('')}<p class="r-basis">${backtestLine(B, state.lang)}</p>` : ''}
      ${s && (scn.rain || scn.irrigation) ? `<h2>${t('scn.title')}</h2><p>${tf(s.model ? 'report.scn.model' : 'report.scn', { rain: scn.rain, irr: scn.irrigation, low: s.low.toFixed(0), unit: s.unit, limit: s.limit, days: s.daysBelow, next: s.next ? fmtDate(s.next) : '–' })}</p>` : ''}
      <footer>${t('report.foot')}</footer>`;
    if (btn) btn.disabled = false;
    setTimeout(() => window.print(), 60);
  }

  function render() {
    renderBrief();
    renderAsk();
    renderScenario();
    renderRegion();
    refreshMap();
  }

  function bind() {
    $('#brief').addEventListener('click', (e) => {
      if (e.target.closest('[data-report]')) { exportReport(); return; }
      const b = e.target.closest('[data-region]');
      if (b) ctx.selectRegion(b.dataset.region);
    });
    $('#ask').addEventListener('click', (e) => {
      const b = e.target.closest('[data-ask]');
      if (!b) return;
      const q = QUESTIONS.find((x) => x.id === b.dataset.ask);
      showAnswer({ id: q.id, text: q[state.lang] || q.el });
    });
    $('#ask').addEventListener('submit', (e) => {
      e.preventDefault();
      const input = $('#askInput');
      const text = input.value.trim();
      if (!text) return;
      showAnswer({ id: route(text) || 'none', text });
      input.value = '';
    });
    $('#scenario').addEventListener('input', (e) => {
      if (e.target.id === 'scnRain') scn.rain = +e.target.value;
      if (e.target.id === 'scnIrr') scn.irrigation = +e.target.value;
      updateScenario();
    });
    $('#scenario').addEventListener('click', (e) => {
      if (e.target.id !== 'scnReset') return;
      scn.rain = 0; scn.irrigation = 0;
      renderScenario();
    });
    $('#regionView').addEventListener('click', (e) => {
      const tab = e.target.closest('[data-rtab]');
      if (tab) { state.rtab = tab.dataset.rtab; renderChart(); }
    });
  }

  return {
    init, bind, render, addToMap, setVisible, region, exportReport,
    lakeId: () => (fc ? fc.region : null),
    thresholds: () => data.thresholds,
    refName: () => manifest.reference?.name || 'Reference',
  };
}
