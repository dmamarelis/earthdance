// "Status" view: the daily briefing for whoever runs the region.
// Left: what needs action today, area by area. Right: one area's conclusions, weather, water and alerts.
// The numbers come from data/regions.json + live weather; the sentences come from js/insights.js.

import { loadWeather, isoDay } from './weather.js';
import { assess } from './insights.js';

const COLORS = { alert: '#ff6b5e', watch: '#f4b94a', ok: '#7bd88f' };
const RANK = { ok: 0, watch: 1, alert: 2 };
const sum = (a) => a.reduce((s, v) => s + (v ?? 0), 0);
const dayNum = (iso) => Date.parse(`${iso}T00:00:00Z`) / 864e5;

export function createStatus(ctx) {
  const { $, $$, t, L, fmtDate, state, manifest } = ctx;
  const today = isoDay(new Date());
  const tf = (k, v) => t(k).replace(/\{(\w+)\}/g, (_, n) => v[n]);
  const short = (iso) => fmtDate(iso, { day: 'numeric', month: 'short' });
  const signed = (v, d = 0) => { const s = v.toFixed(d); return +s === 0 ? s.replace('-', '') : `${v > 0 ? '+' : ''}${s}`.replace('-', '−'); };

  let data = { regions: [], thresholds: {} };
  let wx = null;
  let wxState = 'loading';            // loading | live | snapshot | none
  let A = {};                         // assessments by area id
  let map = null;
  let chart = null;
  const labels = {};

  const region = (id) => data.regions.find((r) => r.id === id);
  const sorted = () => [...data.regions].sort((a, b) => RANK[A[b.id].status] - RANK[A[a.id].status]);

  function compute() {
    A = {};
    for (const r of data.regions) {
      A[r.id] = assess(r, wx?.byRegion?.[r.id] ?? null, data.thresholds, { today, refName: manifest.reference?.name });
    }
  }

  async function init(regionsData) {
    data = regionsData;
    compute();
    const first = data.regions.map((r) => r.series.dates[0]).sort()[0];
    const start = new Date(Date.parse(`${first}T12:00:00`) - 30 * 864e5);
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
    for (const id of ['regions-fill', 'regions-line']) map.setLayoutProperty(id, 'visibility', on ? 'visible' : 'none');
  }

  // ───────────────────────────────────────────────────────── left: daily briefing
  function renderBrief() {
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
      <div class="brief-kicker">${t('brief.kicker')} · ${fmtDate(today, { weekday: 'long', day: 'numeric', month: 'long' })}</div>
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
              <span title="${t('rg.tile.rate')}">${signed(a.ground.rate)} ${t('unit.mmyr')}</span>
              ${ref ? `<span class="${ref.agrees ? 'yes' : 'no'}" title="${L(ref.finding.title)}">${manifest.reference?.name || 'Ref'} ${ref.agrees ? '✓' : '✗'}</span>` : ''}
              ${a.weather ? `<span title="${t('rg.rain7')}">${a.weather.rain7.toFixed(0)} mm</span>` : ''}
            </span>
          </button>`;
  }).join('')}
      </div>
      <p class="brief-foot">${t('brief.foot')}</p>`;
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
        if (Math.abs(e.dDisp) >= 1) after.push(`${t('ev.ground')} ${signed(e.dDisp)} mm`);
      }
      return `<li class="ev ev-${e.type}">
        <time>${fmtDate(e.date)}</time><b>${what}</b>
        ${e.pass ? `<span>${tf('ev.after', { d: short(e.pass) })} ${after.join(' · ') || '—'}</span>` : ''}
      </li>`;
    }).join('');
    return `<h3 class="sec">${t('ev.title')}</h3>${rows ? `<ul class="events">${rows}</ul>` : `<p class="empty">${t('ev.none')}</p>`}`;
  }

  function alertHTML(r, a) {
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

    box.innerHTML = `
      <div class="insp-head">
        <div>
          <span class="st st-${a.status}">${t(`st.${a.status}`)}</span>
          <h2>${L(r.name)}</h2>
          <div class="coords">${tf('rg.pass', { d: fmtDate(a.lastPass), n: ago })}</div>
        </div>
        <button class="x" data-close-right aria-label="Close"><svg viewBox="0 0 24 24"><path d="M6 6l12 12M18 6L6 18"/></svg></button>
      </div>

      ${w ? forecastHTML(w) : `<p class="empty">${t(wxState === 'loading' ? 'brief.wx.loading' : 'rnote.nowx')}</p>`}

      <div class="tiles">
        <div class="tile ${a.ground.level !== 'ok' ? 'neg' : ''}"><div class="t-label">${t('rg.tile.rate')}</div><div class="t-val">${signed(a.ground.rate)}<small>${t('unit.mmyr')}</small></div></div>
        <div class="tile ${ref && !ref.agrees ? 'warn' : ''}"><div class="t-label">${tf('rg.tile.egms', { ref: manifest.reference?.name || 'Ref' })}</div><div class="t-val">${ref ? signed(ref.diff, 1) : '–'}<small>${t('unit.mmyr')}</small></div></div>
        <div class="tile"><div class="t-label">${t('rg.tile.rain')}</div><div class="t-val">${a.weather ? a.weather.rain30.toFixed(0) : '–'}<small>mm</small></div></div>
      </div>

      <h3 class="sec">${t('rg.findings')}</h3>
      <div class="findings">
        ${a.findings.map((f) => `
          <article class="finding lv-${f.level}">
            <h4>${L(f.title)}</h4>
            <p>${L(f.text)}</p>
            <small>${L(f.basis)}</small>
          </article>`).join('')}
      </div>

      ${alertHTML(r, a)}

      <div class="tabs rtabs">
        ${['wx', 'fx', 'h2o'].map((k) => `<button class="tab ${k === state.rtab ? 'is-active' : ''}" data-rtab="${k}">${t(`rtab.${k}`)}</button>`).join('')}
      </div>
      <div class="chart-wrap"><canvas id="rChart"></canvas></div>
      <div class="chart-note" id="rNote"></div>

      ${eventsHTML(a)}

      <details class="card unknowns">
        <summary>${t('how.title')}</summary>
        <ul>
          <li>${t('how.text')}</li>
          <li>${tf('how.sink', { a: th.sink_watch_mm_yr, b: th.sink_alert_mm_yr })}</li>
          <li>${tf('how.egms', { a: th.egms_tolerance_mm_yr, ref: manifest.reference?.name || 'Ref' })}</li>
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
          line('HH · NISAR (dB)', s.hh_db, '#e9edf3', { pointRadius: 2.5 }),
          line('VV · Sentinel-1 (dB)', s.vv_db, '#c9a7ff', { borderDash: [5, 4], borderWidth: 1.5 }),
          ...(a.effects ? [{ type: 'bar', label: t('rc.rainPass'), data: a.effects.passes.map((p) => p.rain), backgroundColor: 'rgba(92,200,240,.55)', borderRadius: 3, yAxisID: 'y1' }] : []),
        ],
        scales: { y: axis('dB'), ...(a.effects ? { y1: right('mm') } : {}) },
      };
      note = t(a.effects ? 'rnote.fx' : 'rnote.nowx');
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
      options: {
        responsive: true, maintainAspectRatio: false, animation: { duration: 350 },
        interaction: { mode: 'index', intersect: false },
        plugins: { legend: { position: 'bottom', labels: { padding: 12 } } },
        scales: { x: { grid: { display: false }, border: { color: 'rgba(255,255,255,.12)' }, ticks: { maxRotation: 0, autoSkipPadding: 16 } }, ...cfg.scales },
      },
    });
  }

  function render() {
    renderBrief();
    renderRegion();
    refreshMap();
  }

  function bind() {
    $('#brief').addEventListener('click', (e) => {
      const b = e.target.closest('[data-region]');
      if (b) ctx.selectRegion(b.dataset.region);
    });
    $('#regionView').addEventListener('click', (e) => {
      const tab = e.target.closest('[data-rtab]');
      if (tab) { state.rtab = tab.dataset.rtab; renderChart(); }
    });
  }

  return {
    init, bind, render, addToMap, setVisible, region,
    thresholds: () => data.thresholds,
    refName: () => manifest.reference?.name || 'Reference',
  };
}
