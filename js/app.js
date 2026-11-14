// EarthDance — main application.
// Everything shown comes from data/manifest.json (+ the files it points to). No data is hard-coded here.

import { STR } from './i18n.js';
import * as sound from './sound.js';
import { createStatus } from './status.js';
import { slopePerYear } from './insights.js';

const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const app = $('#app');

const state = {
  lang: 'el', mode: 'explore',
  layerId: null, frame: 0,
  opacity: 0,                    // radar overlay hidden by default: raise this (0..1) to show the layers on the map
  compare: false, compareFrame: 0, swipeX: null,
  pointId: null, tab: 'ground',
  regionId: null, rtab: 'wx',
  tilt: false,
};

let M, C, validation;            // manifest, content, validation data
let points = [];
let mapA, mapB = null;
const markers = {};
let chart, valChart;
let status;                      // the Status view (js/status.js)

// ───────────────────────────────────────────────────────── helpers
const t = (k) => STR[k]?.[state.lang] ?? k;
const L = (o) => (o && typeof o === 'object' ? o[state.lang] ?? o.en ?? o.el ?? '' : o ?? '');
const layer = () => M.layers.find((l) => l.id === state.layerId);
const frames = () => layer().frames;
const point = () => points.find((p) => p.id === state.pointId);
const getJSON = async (url) => {
  const r = await fetch(url);
  if (!r.ok) throw new Error(`${url}: ${r.status}`);
  return r.json();
};
const fmtDate = (iso, opts = { day: 'numeric', month: 'short', year: 'numeric' }) =>
  new Intl.DateTimeFormat(state.lang === 'el' ? 'el-GR' : 'en-GB', opts).format(new Date(`${iso}T00:00:00`));
const days = (iso) => Date.parse(iso) / 864e5;
const nearestIndex = (dates, iso) => {
  let best = 0;
  dates.forEach((d, i) => { if (Math.abs(days(d) - days(iso)) < Math.abs(days(dates[best]) - days(iso))) best = i; });
  return best;
};
const fmtNum = (v, d = 0) => `${v > 0 ? '+' : ''}${v.toFixed(d)}`.replace('-', '−');
const isMobile = () => innerWidth <= 820;

function pad() {
  if (isMobile()) return { top: 66, bottom: 140, left: 10, right: 10 };
  return { top: 76, bottom: 150, left: state.mode === 'explore' ? 348 : 432, right: state.pointId || state.regionId ? 408 : 16 };
}

// ───────────────────────────────────────────────────────── map
function baseStyle() {
  // Keyless Esri satellite imagery, at full quality. Swap these URLs if you prefer another provider.
  const esri = (name) => [`https://server.arcgisonline.com/ArcGIS/rest/services/${name}/MapServer/tile/{z}/{y}/{x}`];
  const hidpi = Math.min(2, Math.max(1, Math.round(devicePixelRatio || 1)));   // sharper tiles on retina screens
  return {
    version: 8,
    sources: {
      sat: { type: 'raster', tiles: esri('World_Imagery'), tileSize: 256 / hidpi, maxzoom: 19, attribution: 'Imagery © Esri, Maxar, Earthstar Geographics' },
      labels: { type: 'raster', tiles: esri('Reference/World_Boundaries_and_Places'), tileSize: 256, maxzoom: 19 },
    },
    layers: [
      { id: 'bg', type: 'background', paint: { 'background-color': '#070a10' } },
      { id: 'sat', type: 'raster', source: 'sat' },
      { id: 'labels', type: 'raster', source: 'labels', paint: { 'raster-opacity': 0.9 } },
    ],
  };
}

function makeMap(container, interactive) {
  return new maplibregl.Map({
    container, style: baseStyle(), center: M.site.center, zoom: M.site.zoom,
    interactive, maxPitch: 65, fadeDuration: 0,
    attributionControl: interactive ? { compact: true } : false,
  });
}

const cornersOf = ([w, s, e, n]) => [[w, n], [e, n], [e, s], [w, s]];

/** Shows one frame of a layer on a map. A frame is either {image} (one picture + bounds) or {tiles} (XYZ URL). */
function applyOverlay(map, ly, frame) {
  if (!map?.__ready || !frame) return;
  const kind = frame.tiles ? 'tiles' : 'image';
  const coordinates = cornersOf(ly.bounds || M.site.bounds);
  const src = map.getSource('ov');
  if (src && kind === 'image' && map.__ovKind === 'image') {
    src.updateImage({ url: frame.image, coordinates });
  } else {
    if (map.getLayer('ov')) map.removeLayer('ov');
    if (src) map.removeSource('ov');
    map.addSource('ov', kind === 'image'
      ? { type: 'image', url: frame.image, coordinates }
      : { type: 'raster', tiles: [frame.tiles], tileSize: 256 });
    map.addLayer({ id: 'ov', type: 'raster', source: 'ov', paint: { 'raster-fade-duration': 0 } }, 'labels');
    map.__ovKind = kind;
  }
  map.setPaintProperty('ov', 'raster-opacity', state.opacity);
}

function syncB() {
  if (!mapB) return;
  mapB.jumpTo({
    center: mapA.getCenter(), zoom: mapA.getZoom(), bearing: mapA.getBearing(),
    pitch: mapA.getPitch(), padding: mapA.getPadding(),
  });
}

function addMarkers() {
  for (const p of points) {
    const el = document.createElement('div');
    el.className = `pt k-${p.kind}`;
    el.innerHTML = '<span class="pt-dot"></span><span class="pt-label"></span>';
    el.addEventListener('click', (e) => { e.stopPropagation(); selectPoint(p.id); });
    new maplibregl.Marker({ element: el, anchor: 'left', offset: [-7, 0] }).setLngLat([p.lon, p.lat]).addTo(mapA);
    markers[p.id] = el;
  }
  renderMarkers();
}

function renderMarkers() {
  for (const p of points) {
    const el = markers[p.id];
    if (!el) continue;
    $('.pt-label', el).textContent = L(p.name);
    el.classList.toggle('is-sel', p.id === state.pointId);
  }
}

// ───────────────────────────────────────────────────────── layers, legend, source
function setLayer(id) {
  const date = state.layerId ? frames()[state.frame]?.date : null;
  state.layerId = id;
  state.frame = date ? nearestIndex(frames().map((f) => f.date), date) : 0;
  state.compareFrame = Math.min(state.compareFrame, frames().length - 1);
  frames().forEach((f) => { if (f.image) new Image().src = f.image; });   // warm the cache for smooth playback
  renderLayers();
  renderTimeline();
  setFrame(state.frame);
  if (state.compare) applyOverlay(mapB, layer(), frames()[state.compareFrame]);
}

function gradient(ly, dir = '90deg') {
  const s = [...ly.legend.stops].sort((a, b) => a[0] - b[0]);
  const [min, max] = [s[0][0], s.at(-1)[0]];
  return `linear-gradient(${dir}, ${s.map(([v, c]) => `${c} ${(((v - min) / (max - min)) * 100).toFixed(1)}%`).join(', ')})`;
}

function legendHTML(ly) {
  const vals = ly.legend.stops.map((s) => s[0]);
  const [min, max] = [Math.min(...vals), Math.max(...vals)];
  return `
    <p class="legend-desc">${L(ly.description)}</p>
    <div class="legend-bar" style="background:${gradient(ly)}"></div>
    <div class="legend-scale"><span>${fmtNum(min)}</span><span>${ly.unit}</span><span>${fmtNum(max)}</span></div>
    <div class="legend-ends"><span>${L(ly.legend.low)}</span><span>${L(ly.legend.high)}</span></div>`;
}

function renderLayers() {
  $('#layerCount').textContent = String(M.layers.length).padStart(2, '0');
  $('#layerList').innerHTML = M.layers.map((ly) => `
    <button class="layer ${ly.id === state.layerId ? 'is-active' : ''}" data-layer="${ly.id}">
      <span class="swatch" style="background:${gradient(ly, '0deg')}"></span>
      <span><span class="l-name">${L(ly.name)}</span><br><span class="l-short">${L(ly.short)}</span></span>
      <span class="band ${ly.band}">${ly.band}</span>
    </button>`).join('');
  $('#legend').innerHTML = legendHTML(layer());
}

// ───────────────────────────────────────────────────────── timeline
const tickPos = (i, n) => (n > 1 ? (i / (n - 1)) * 100 : 50);

function renderTimeline() {
  const fr = frames();
  const every = Math.ceil(fr.length / 10);
  $('#ticks').innerHTML = fr.map((f, i) => `
    <button class="tick" data-i="${i}" style="left:${tickPos(i, fr.length)}%" title="${fmtDate(f.date)}">
      <i></i><span class="${i % every ? 'nolabel' : ''}">${fmtDate(f.date, { day: 'numeric', month: 'short' })}</span>
    </button>`).join('');
  $('#compareSel').innerHTML = fr.map((f, i) => `<option value="${i}">${fmtDate(f.date)}</option>`).join('');
  $('#compareSel').value = state.compareFrame;
  updateTimeline();
}

function updateTimeline() {
  const fr = frames();
  const f = fr[state.frame];
  $$('.tick').forEach((el, i) => {
    el.classList.toggle('past', i < state.frame);
    el.classList.toggle('now', i === state.frame);
    el.classList.toggle('cmp', state.compare && i === state.compareFrame && i !== state.frame);
  });
  $('#trackFill').style.width = `${tickPos(state.frame, fr.length)}%`;
  $('#tlDate').textContent = fmtDate(f.date, { day: 'numeric', month: 'long', year: 'numeric' });
  $('#tlMeta').textContent = `${t('tl.frame')} ${state.frame + 1} ${t('tl.of')} ${fr.length} · ${L(layer().name)}`;
  $('#swipeTagL').textContent = fmtDate(fr[state.compareFrame].date);
  $('#swipeTagR').textContent = fmtDate(f.date);
}

function setFrame(i) {
  state.frame = Math.max(0, Math.min(frames().length - 1, i));
  applyOverlay(mapA, layer(), frames()[state.frame]);
  updateTimeline();
  if (chart && point()) {
    chart.$cursor = nearestIndex(point().series.dates, frames()[state.frame].date);
    chart.update('none');
  }
}

// ───────────────────────────────────────────────────────── swipe compare
function setSwipe(x) {
  state.swipeX = Math.max(30, Math.min(innerWidth - 30, x));
  $('#swipe').style.setProperty('--x', `${state.swipeX}px`);
  $('#mapB').style.clipPath = `inset(0 ${innerWidth - state.swipeX}px 0 0)`;
}

function setCompare(on) {
  state.compare = on;
  $('#btnCompare').classList.toggle('is-on', on);
  for (const id of ['#compareSel', '#swipe', '#mapB']) $(id).hidden = !on;
  if (on) {
    if (state.compareFrame === state.frame) {
      state.compareFrame = 0;
      if (state.frame === 0) setFrame(frames().length - 1);
    }
    $('#compareSel').value = state.compareFrame;
    const p = pad();
    setSwipe(state.swipeX ?? (p.left + innerWidth - p.right) / 2);
    if (!mapB) {
      mapB = makeMap('mapB', false);
      mapB.on('load', () => {
        mapB.__ready = true;
        syncB();
        applyOverlay(mapB, layer(), frames()[state.compareFrame]);
      });
    } else {
      mapB.resize();
      syncB();
      applyOverlay(mapB, layer(), frames()[state.compareFrame]);
    }
  }
  updateTimeline();
}

// ───────────────────────────────────────────────────────── point inspector
function openRight(kind) {
  app.dataset.right = kind;
  app.classList.add('has-point');
  $('#right').setAttribute('aria-hidden', 'false');
  $('#right').scrollTop = 0;
}

function selectRegion(id, fly = true) {
  if (state.pointId) { state.pointId = null; sound.stop(); listenUI(false); renderMarkers(); }
  state.regionId = id;
  openRight('region');
  status.render();
  const r = status.region(id);
  if (fly) mapA.easeTo({ center: r.center, zoom: Math.max(mapA.getZoom(), 9.8), padding: pad(), duration: 900 });
}

function selectPoint(id, fly = true) {
  const changed = state.pointId !== id;
  state.pointId = id;
  if (state.regionId) { state.regionId = null; status.render(); }
  openRight('point');
  renderMarkers();
  renderInspector();
  if (changed) { sound.stop(); listenUI(false); }
  const p = point();
  if (fly) mapA.easeTo({ center: [p.lon, p.lat], padding: pad(), duration: 900 });
}

function closePoint(ease = true) {
  if (!state.pointId && !state.regionId) return;
  state.pointId = null;
  if (state.regionId) { state.regionId = null; status.render(); }
  sound.stop(); listenUI(false);
  app.classList.remove('has-point');
  $('#right').setAttribute('aria-hidden', 'true');
  renderMarkers();
  if (ease) mapA.easeTo({ padding: pad(), duration: 600 });
}

const tile = (label, val, unit, neg = false) =>
  `<div class="tile ${neg ? 'neg' : ''}"><div class="t-label">${label}</div><div class="t-val">${val}<small>${unit}</small></div></div>`;

function renderInspector() {
  const p = point();
  if (!p) return;
  const s = p.series;
  $('#pKind').textContent = t(`kind.${p.kind}`);
  $('#pKind').className = `kind k-${p.kind}`;
  $('#pName').textContent = L(p.name);
  $('#pCoords').textContent = `${p.lat.toFixed(3)}°N  ${p.lon.toFixed(3)}°E`;

  const total = s.disp_mm.at(-1) - s.disp_mm[0];
  const rate = slopePerYear(s.dates, s.disp_mm);
  const hvRange = Math.max(...s.hv_db) - Math.min(...s.hv_db);
  $('#pTiles').innerHTML =
    tile(t('tile.total'), fmtNum(total), 'mm', total < -10) +
    tile(t('tile.rate'), fmtNum(rate), t('unit.mmyr'), rate < -10) +
    tile(t('tile.hv'), hvRange.toFixed(1), 'dB');

  // does the independent reference tell the same story at this point?
  const agree = $('#pAgree');
  agree.hidden = !s.ref_mm;
  if (s.ref_mm) {
    const diff = rate - slopePerYear(s.dates, s.ref_mm);
    const ok = Math.abs(diff) <= (status.thresholds().egms_tolerance_mm_yr ?? 6);
    agree.className = `agree ${ok ? 'yes' : 'no'}`;
    agree.innerHTML = `<b>${ok ? '✓' : '!'}</b> ${t(ok ? 'agree.yes' : 'agree.no').replace('{ref}', status.refName())} <span>Δ ${fmtNum(diff, 1)} ${t('unit.mmyr')}</span>`;
  }

  const why = C.explanations?.[p.kind];
  $('#pWhy').hidden = !why;
  if (why) $('#pWhy').innerHTML = `<h3>${L(why.title)}</h3><p>${L(why.text)}</p>`;

  const voice = C.voices?.[p.id];
  $('#pVoice').hidden = !voice;
  if (voice) {
    $('#pVoice').innerHTML = `<div class="voice-tag">${t('voice.tag')}</div>
      <blockquote>${L(voice.quote)}<cite>— ${L(voice.who)}</cite></blockquote>
      ${voice.media ? `<audio controls src="${voice.media}" style="width:100%;margin-top:10px"></audio>` : ''}`;
  }
  $('#unknownList').innerHTML = (C.unknowns || []).map((u) => `<li>${L(u)}</li>`).join('');
  renderChart();
}

const cursorPlugin = {
  id: 'cursor',
  afterDatasetsDraw(c) {
    if (c.$cursor == null || !c.chartArea) return;
    const x = c.scales.x.getPixelForValue(c.$cursor);
    const { top, bottom } = c.chartArea;
    c.ctx.save();
    c.ctx.strokeStyle = '#f4b94a';
    c.ctx.lineWidth = 1.5;
    c.ctx.setLineDash([3, 3]);
    c.ctx.beginPath(); c.ctx.moveTo(x, top); c.ctx.lineTo(x, bottom); c.ctx.stroke();
    c.ctx.setLineDash([]);
    c.ctx.fillStyle = '#f4b94a';
    c.ctx.beginPath(); c.ctx.arc(x, top, 3.5, 0, Math.PI * 2); c.ctx.fill();
    c.ctx.restore();
  },
};

function chartDefaults() {
  Chart.defaults.font.family = '"Iosevka UI", ui-monospace, monospace';
  Chart.defaults.font.size = 11;
  Chart.defaults.color = '#8b96a8';
  Chart.defaults.plugins.legend.labels.usePointStyle = true;
  Chart.defaults.plugins.legend.labels.boxWidth = 7;
  Chart.defaults.plugins.legend.labels.boxHeight = 7;
  Object.assign(Chart.defaults.plugins.tooltip, {
    backgroundColor: '#0d121b', borderColor: 'rgba(255,255,255,.16)', borderWidth: 1,
    padding: 10, titleColor: '#e9edf3', bodyColor: '#b6c0cf', cornerRadius: 10, boxPadding: 4,
  });
}

function renderChart() {
  const p = point();
  if (!p) return;
  const s = p.series;
  const line = (label, data, color, extra = {}) => ({
    label, data, borderColor: color, backgroundColor: color, borderWidth: 2.2, tension: 0.35,
    pointRadius: 2.5, pointHoverRadius: 5, pointBackgroundColor: color, ...extra,
  });
  const fillDown = (c) => {
    const a = c.chart.chartArea;
    if (!a) return 'transparent';
    const g = c.chart.ctx.createLinearGradient(0, a.top, 0, a.bottom);
    g.addColorStop(0, 'rgba(255,107,94,0)');
    g.addColorStop(1, 'rgba(255,107,94,.38)');
    return g;
  };
  const datasets = state.tab === 'ground'
    ? [
      line(t('chart.disp'), s.disp_mm, '#ff6b5e', { fill: 'origin', backgroundColor: fillDown }),
      s.ref_mm && line(`${M.reference?.name || 'Reference'} (mm)`, s.ref_mm, '#9aa6b8', { borderDash: [5, 4], borderWidth: 1.5, pointRadius: 0 }),
    ].filter(Boolean)
    : [
      line('HH · L (dB)', s.hh_db, '#e9edf3'),
      line('HV · L (dB)', s.hv_db, '#7bd88f'),
      s.vv_db && line('VV · C (dB)', s.vv_db, '#c9a7ff', { borderDash: [5, 4], borderWidth: 1.5, pointRadius: 0 }),
    ].filter(Boolean);
  const labels = s.dates.map((d) => fmtDate(d, { day: 'numeric', month: 'short' }));

  if (!chart) {
    chart = new Chart($('#chart'), {
      type: 'line',
      data: { labels, datasets },
      plugins: [cursorPlugin],
      options: {
        responsive: true, maintainAspectRatio: false, animation: { duration: 450 },
        interaction: { mode: 'index', intersect: false },
        plugins: { legend: { position: 'bottom', labels: { padding: 14 } } },
        scales: {
          x: { grid: { display: false }, border: { color: 'rgba(255,255,255,.12)' }, ticks: { maxRotation: 0, autoSkipPadding: 14 } },
          y: { grid: { color: 'rgba(255,255,255,.06)' }, border: { display: false }, ticks: { maxTicksLimit: 6 } },
        },
        onClick: (e, _els, c) => {
          const i = c.scales.x.getValueForPixel(e.x);
          const pt = point();
          if (pt && i >= 0 && i < pt.series.dates.length) {
            setFrame(nearestIndex(frames().map((f) => f.date), pt.series.dates[i]));
          }
        },
      },
    });
  } else {
    chart.data.labels = labels;
    chart.data.datasets = datasets;
  }
  chart.$cursor = nearestIndex(s.dates, frames()[state.frame].date);
  chart.update();
  $('#chartNote').textContent = t(`chart.note.${state.tab}`);
  $$('.tab').forEach((b) => b.classList.toggle('is-active', b.dataset.tab === state.tab));
}

// ───────────────────────────────────────────────────────── sonification
function listenUI(on) {
  $('#btnListen').classList.toggle('is-on', on);
  $('#listenLabel').textContent = t(on ? 'listen.stop' : 'listen.play');
}

function toggleListen() {
  if (sound.isPlaying()) { sound.stop(); listenUI(false); return; }
  const p = point();
  if (!p) return;
  const s = p.series;
  const [lo, hi] = [Math.min(...s.hv_db), Math.max(...s.hv_db)];
  const frameDates = frames().map((f) => f.date);
  listenUI(true);
  sound.play(state.tab === 'ground' ? s.disp_mm : s.hv_db, {
    bright: s.hv_db.map((v) => (v - lo) / (hi - lo || 1)),
    onStep: (i) => {
      setFrame(nearestIndex(frameDates, s.dates[i]));
      const el = markers[p.id];
      el.classList.remove('beat');
      void el.offsetWidth;
      el.classList.add('beat');
    },
    onEnd: () => listenUI(false),
  });
}

// ───────────────────────────────────────────────────────── modes
function setMode(mode) {
  state.mode = mode;
  app.dataset.mode = mode;
  app.classList.remove('drawer');
  $$('.mode').forEach((b) => b.classList.toggle('is-active', b.dataset.mode === mode));
  status.setVisible(mode === 'status');
  if (mode === 'status') {
    closePoint(false);
    mapA.easeTo({ center: M.site.center, zoom: M.site.zoom, pitch: 0, bearing: 0, padding: pad(), duration: 900 });
  } else {
    if (state.regionId) closePoint(false);
    mapA.easeTo({ padding: pad(), duration: 600 });
  }
}

// ───────────────────────────────────────────────────────── validation + about
function renderValidation() {
  const pairs = validation.pairs;
  const x = pairs.map((p) => p.ref);
  const y = pairs.map((p) => p.nisar);
  const n = pairs.length;
  const mean = (a) => a.reduce((s, v) => s + v, 0) / a.length;
  const [mx, my] = [mean(x), mean(y)];
  const sxy = x.reduce((s, xi, i) => s + (xi - mx) * (y[i] - my), 0);
  const r2 = sxy ** 2 / (x.reduce((s, xi) => s + (xi - mx) ** 2, 0) * y.reduce((s, yi) => s + (yi - my) ** 2, 0));
  const rmse = Math.sqrt(mean(x.map((xi, i) => (y[i] - xi) ** 2)));
  const ref = M.reference?.name || 'Reference';
  const u = validation.unit;

  $('#valTiles').innerHTML =
    tile(t('val.n'), n, '') + tile(t('val.r2'), r2.toFixed(2), '') +
    tile(t('val.rmse'), rmse.toFixed(1), u) + tile(t('val.bias'), fmtNum(my - mx, 1), u);
  $('#valText').textContent = t('val.text');
  $('#valNote').textContent = `${L(validation.note)} ${ref}: ${L(M.reference?.label)}.${M.demo ? ` ${t('demo.warn')}` : ''}`;

  const lo = Math.floor((Math.min(...x, ...y) - 2) / 10) * 10;
  const hi = Math.ceil((Math.max(...x, ...y, 0) + 2) / 10) * 10;
  valChart?.destroy();
  valChart = new Chart($('#valChart'), {
    type: 'scatter',
    data: {
      datasets: [
        { label: t('val.line'), type: 'line', data: [{ x: lo, y: lo }, { x: hi, y: hi }], borderColor: 'rgba(255,255,255,.4)', borderDash: [6, 5], borderWidth: 1.5, pointRadius: 0 },
        { label: `NISAR vs ${ref}`, data: pairs.map((p) => ({ x: p.ref, y: p.nisar })), backgroundColor: 'rgba(244,185,74,.75)', borderColor: '#f4b94a', pointRadius: 4.5, pointHoverRadius: 7 },
      ],
    },
    options: {
      responsive: true, maintainAspectRatio: false,
      plugins: { legend: { position: 'bottom', labels: { padding: 14 } } },
      scales: {
        x: { min: lo, max: hi, title: { display: true, text: `${ref} (${u})` }, grid: { color: 'rgba(255,255,255,.06)' } },
        y: { min: lo, max: hi, title: { display: true, text: `NISAR (${u})` }, grid: { color: 'rgba(255,255,255,.06)' } },
      },
    },
  });
}

function renderAbout() {
  const a = C.about;
  $('#aboutTagline').textContent = L(a.tagline);
  $('#aboutSources').innerHTML = a.sources.map((s) =>
    `<li><a href="${s.url}" target="_blank" rel="noopener">${s.name} ↗</a><span>${s.by}</span></li>`).join('');
  $('#aboutTeam').textContent = L(a.team);
  $('#aboutAi').textContent = L(a.ai);
}

// ───────────────────────────────────────────────────────── render everything (also on language change)
function renderAll() {
  document.documentElement.lang = state.lang;
  $$('[data-i18n]').forEach((el) => { el.textContent = t(el.dataset.i18n); });
  $('#btnLang').classList.toggle('en', state.lang === 'en');
  $('#siteName').textContent = `${L(M.site.name)} · ${L(M.site.country)}`;
  $('#demoBadge').hidden = !M.demo;
  listenUI(sound.isPlaying());
  renderLayers();
  renderTimeline();
  renderMarkers();
  renderInspector();
  renderAbout();
  status.render();
  if (!$('#validation').hidden) renderValidation();
}

// ───────────────────────────────────────────────────────── events
function bindUI() {
  $('#layerList').addEventListener('click', (e) => {
    const b = e.target.closest('[data-layer]');
    if (b) setLayer(b.dataset.layer);
  });

  // timeline: click or drag anywhere on the track
  const track = $('#track');
  let dragging = false;
  const indexAt = (e) => {
    const r = $('#ticks').getBoundingClientRect();
    const n = frames().length;
    return Math.max(0, Math.min(n - 1, Math.round(((e.clientX - r.left) / r.width) * (n - 1))));
  };
  track.addEventListener('pointerdown', (e) => { dragging = true; track.setPointerCapture(e.pointerId); setFrame(indexAt(e)); });
  track.addEventListener('pointermove', (e) => { if (dragging && indexAt(e) !== state.frame) setFrame(indexAt(e)); });
  track.addEventListener('pointerup', () => { dragging = false; });

  $('#btnCompare').addEventListener('click', () => setCompare(!state.compare));
  $('#compareSel').addEventListener('change', (e) => {
    state.compareFrame = +e.target.value;
    applyOverlay(mapB, layer(), frames()[state.compareFrame]);
    updateTimeline();
  });
  const handle = $('.swipe-handle');
  let swiping = false;
  handle.addEventListener('pointerdown', (e) => { swiping = true; handle.setPointerCapture(e.pointerId); });
  handle.addEventListener('pointermove', (e) => { if (swiping) setSwipe(e.clientX); });
  handle.addEventListener('pointerup', () => { swiping = false; });

  $$('.mode').forEach((b) => b.addEventListener('click', () => setMode(b.dataset.mode)));

  $('#right').addEventListener('click', (e) => { if (e.target.closest('[data-close-right]')) closePoint(); });
  status.bind();
  $$('.tab').forEach((b) => b.addEventListener('click', () => { state.tab = b.dataset.tab; sound.stop(); listenUI(false); renderChart(); }));
  $('#btnListen').addEventListener('click', toggleListen);

  $('#btnLang').addEventListener('click', () => { state.lang = state.lang === 'el' ? 'en' : 'el'; renderAll(); });
  $('#btnValidation').addEventListener('click', () => { $('#validation').hidden = false; renderValidation(); });
  $('#btnAbout').addEventListener('click', () => { $('#about').hidden = false; });
  $('#btnDrawer').addEventListener('click', () => app.classList.toggle('drawer'));
  $$('.sheet').forEach((s) => s.addEventListener('click', (e) => {
    if (e.target === s || e.target.closest('[data-close]')) s.hidden = true;
  }));

  $('#zoomIn').addEventListener('click', () => mapA.zoomIn());
  $('#zoomOut').addEventListener('click', () => mapA.zoomOut());
  $('#btnHome').addEventListener('click', () => {
    state.tilt = false; $('#btnTilt').classList.remove('is-on');
    mapA.easeTo({ center: M.site.center, zoom: M.site.zoom, pitch: 0, bearing: 0, padding: pad(), duration: 1200 });
  });
  $('#btnTilt').addEventListener('click', (e) => {
    state.tilt = !state.tilt;
    e.currentTarget.classList.toggle('is-on', state.tilt);
    mapA.easeTo({ pitch: state.tilt ? 55 : 0, bearing: state.tilt ? -18 : 0, duration: 1000 });
  });

  addEventListener('keydown', (e) => {
    if (e.target.matches('input, select, textarea')) return;
    if (e.key === 'Escape') { $$('.sheet').forEach((s) => { s.hidden = true; }); return; }
    if (e.key === 'ArrowRight') setFrame(state.frame + 1);
    if (e.key === 'ArrowLeft') setFrame(state.frame - 1);
  });
  addEventListener('resize', () => { if (state.compare) setSwipe(state.swipeX); });
}

// ───────────────────────────────────────────────────────── boot
async function boot() {
  try {
    M = await getJSON('data/manifest.json');
    const [content, pts, val, regions] = await Promise.all([
      getJSON(M.content), getJSON(M.points), getJSON(M.validation),
      M.regions ? getJSON(M.regions) : { regions: [], thresholds: {} },
    ]);
    status = createStatus({ $, $$, t, L, fmtDate, state, manifest: M, selectRegion });
    status.init(regions);
    C = content;
    validation = val;
    points = pts.features.map((f) => ({ ...f.properties, lon: f.geometry.coordinates[0], lat: f.geometry.coordinates[1] }));
  } catch (err) {
    console.error(err);
    $('#loaderMsg').textContent = t('err.load');
    return;
  }
  state.layerId = M.layers[0].id;
  chartDefaults();
  bindUI();
  renderAll();
  $('#loaderMsg').textContent = t('loader.map');

  mapA = makeMap('map', true);
  mapA.on('move', syncB);
  mapA.on('load', () => {
    mapA.__ready = true;
    mapA.jumpTo({ center: M.site.center, zoom: M.site.zoom, padding: pad() });
    addMarkers();
    status.addToMap(mapA);
    setLayer(state.layerId);
    $('#loader').classList.add('done');
  });
}

boot();
