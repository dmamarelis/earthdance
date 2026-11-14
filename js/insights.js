// Conclusions engine for the Status view.
// Plain, readable rules: every sentence the app shows about an area is produced here from the numbers in
// data/regions.json and the weather series. No machine learning, nothing hidden: change a threshold in
// regions.json ("thresholds") and the conclusions change with it.

const day = (iso) => Date.parse(`${iso}T00:00:00Z`) / 864e5;
const addDays = (iso, n) => new Date((day(iso) + n) * 864e5).toISOString().slice(0, 10);
const sum = (a) => a.reduce((s, v) => s + (v ?? 0), 0);
const mean = (a) => sum(a) / (a.length || 1);
const median = (a) => { const s = [...a].sort((x, y) => x - y); return s[Math.floor(s.length / 2)]; };
const month = (iso) => +iso.slice(5, 7);
const RANK = { ok: 0, watch: 1, alert: 2 };

const num = (v, d = 0) => { const s = v.toFixed(d); return +s === 0 ? s.replace('-', '') : `${v > 0 ? '+' : ''}${s}`.replace('-', '−'); };
const fd = (iso, lang, opts = { day: 'numeric', month: 'short', year: 'numeric' }) =>
  new Intl.DateTimeFormat(lang === 'el' ? 'el-GR' : 'en-GB', opts).format(new Date(`${iso}T00:00:00`));
const both = (fn) => ({ el: fn('el'), en: fn('en') });

export function slopePerYear(dates, vals) {
  const x = dates.map((d) => (day(d) - day(dates[0])) / 365.25);
  const [mx, my] = [mean(x), mean(vals)];
  const den = sum(x.map((xi) => (xi - mx) ** 2));
  return den ? sum(x.map((xi, i) => (xi - mx) * (vals[i] - my))) / den : 0;
}

export function corr(a, b) {
  const [ma, mb] = [mean(a), mean(b)];
  const den = Math.sqrt(sum(a.map((v) => (v - ma) ** 2)) * sum(b.map((v) => (v - mb) ** 2)));
  return den ? sum(a.map((v, i) => (v - ma) * (b[i] - mb))) / den : 0;
}

/** Sum of a weather variable over [from, to], both inclusive. */
export function wxSum(wx, key, from, to) {
  let s = 0;
  wx.time.forEach((d, i) => { if (d >= from && d <= to) s += wx[key][i] ?? 0; });
  return s;
}

// ───────────────────────────────────────────────────────── individual rules
function ground(s, th) {
  const rate = slopePerYear(s.dates, s.disp_mm);
  const total = s.disp_mm.at(-1) - s.disp_mm[0];
  const level = rate <= th.sink_alert_mm_yr ? 'alert' : rate <= th.sink_watch_mm_yr ? 'watch' : 'ok';

  // does it sink faster while the fields are irrigated (May–September)?
  let [dIrr, tIrr, dRest, tRest] = [0, 0, 0, 0];
  for (let i = 1; i < s.dates.length; i++) {
    const span = day(s.dates[i]) - day(s.dates[i - 1]);
    const mid = month(addDays(s.dates[i - 1], span / 2));
    if (mid >= 5 && mid <= 9) { dIrr += s.disp_mm[i] - s.disp_mm[i - 1]; tIrr += span; } else { dRest += s.disp_mm[i] - s.disp_mm[i - 1]; tRest += span; }
  }
  const ratio = tIrr && tRest && dRest < 0 ? (dIrr / tIrr) / (dRest / tRest) : null;
  const seasonal = level !== 'ok' && ratio != null && ratio >= 1.3;

  return {
    rate, total, level, ratio: seasonal ? ratio : null,
    finding: {
      key: 'ground', level,
      title: level === 'ok' ? { el: 'Έδαφος σταθερό', en: 'Ground is stable' }
        : level === 'watch' ? { el: 'Το έδαφος υποχωρεί', en: 'The ground is sinking' }
          : { el: 'Έντονη καθίζηση', en: 'Strong subsidence' },
      text: both((l) => {
        const since = fd(s.dates[0], l);
        const base = l === 'el'
          ? (level === 'ok' ? `Καμία αξιόλογη μετακίνηση: ${num(rate)} mm/έτος.` : `Υποχωρεί με ${num(rate)} mm/έτος (σύνολο ${num(total)} mm από ${since}).`)
          : (level === 'ok' ? `No significant motion: ${num(rate)} mm/yr.` : `Sinking at ${num(rate)} mm/yr (${num(total)} mm in total since ${since}).`);
        if (!seasonal) return base;
        return base + (l === 'el'
          ? ` Την αρδευτική περίοδο (Μάιος–Σεπτέμβριος) βυθίζεται ${ratio.toFixed(1)}× πιο γρήγορα: ένδειξη ότι φταίει η άντληση υπόγειου νερού.`
          : ` During the irrigation season (May–September) it sinks ${ratio.toFixed(1)}× faster: a sign that groundwater pumping is the cause.`);
      }),
      basis: { el: 'NISAR L2 GUNW · κλίση ευθείας στη χρονοσειρά μετατόπισης', en: 'NISAR L2 GUNW · linear trend of the displacement series' },
    },
  };
}

function reference(s, th, rate, refName) {
  if (!s.ref_mm) return null;
  const refRate = slopePerYear(s.dates, s.ref_mm);
  const diff = rate - refRate;
  const rmse = Math.sqrt(mean(s.disp_mm.map((v, i) => (v - s.ref_mm[i]) ** 2)));
  const agrees = Math.abs(diff) <= th.egms_tolerance_mm_yr;
  return {
    refRate, diff, rmse, agrees,
    finding: {
      key: 'egms', level: agrees ? 'ok' : 'watch',
      title: agrees ? { el: `Συμφωνεί με ${refName}`, en: `Agrees with ${refName}` } : { el: `Απόκλιση από ${refName}`, en: `Differs from ${refName}` },
      text: both((l) => {
        const head = l === 'el'
          ? `NISAR ${num(rate)} έναντι ${refName} ${num(refRate)} mm/έτος (διαφορά ${num(diff, 1)}, RMSE ${rmse.toFixed(1)} mm).`
          : `NISAR ${num(rate)} vs ${refName} ${num(refRate)} mm/yr (difference ${num(diff, 1)}, RMSE ${rmse.toFixed(1)} mm).`;
        if (agrees) return head + (l === 'el' ? ' Δύο ανεξάρτητα συστήματα δίνουν την ίδια εικόνα.' : ' Two independent systems give the same picture.');
        return head + (l === 'el'
          ? ` Ξεπερνά την ανοχή των ±${th.egms_tolerance_mm_yr} mm/έτος: το συμπέρασμα για το έδαφος θέλει έλεγχο πριν χρησιμοποιηθεί.`
          : ` Beyond the ±${th.egms_tolerance_mm_yr} mm/yr tolerance: check the ground conclusion before acting on it.`);
      }),
      basis: { el: `${refName} · σύγκριση ρυθμών στην ίδια περιοχή`, en: `${refName} · rates compared over the same area` },
    },
  };
}

function sensors(s) {
  if (!s.hh_db || !s.vv_db) return null;
  const r = corr(s.hh_db, s.vv_db);
  const hvRange = s.hv_db ? Math.max(...s.hv_db) - Math.min(...s.hv_db) : 0;
  const peak = s.hv_db ? s.dates[s.hv_db.indexOf(Math.max(...s.hv_db))] : null;
  return {
    r, hvRange,
    finding: {
      key: 'sensors', level: 'ok',
      title: r >= 0.6 ? { el: 'NISAR και Sentinel-1 συμφωνούν', en: 'NISAR and Sentinel-1 agree' } : { el: 'Οι δύο δορυφόροι βλέπουν διαφορετικά', en: 'The two satellites see differently' },
      text: both((l) => {
        let out = l === 'el'
          ? `Οπισθοσκέδαση L-band (HH) και C-band (VV): συσχέτιση r = ${r.toFixed(2)}.`
          : `L-band (HH) and C-band (VV) backscatter: correlation r = ${r.toFixed(2)}.`;
        if (r < 0.6) out += l === 'el' ? ' Αναμενόμενο όπου υπάρχει βλάστηση: το L-band τη διαπερνά, το C-band όχι.' : ' Expected over vegetation: L-band penetrates it, C-band does not.';
        if (hvRange >= 4) {
          const m = fd(peak, l, { month: 'long' });
          out += l === 'el' ? ` Ο κύκλος της καλλιέργειας φαίνεται στο HV: κορύφωση τον ${m} (εύρος ${hvRange.toFixed(1)} dB).` : ` The crop cycle shows in HV: peak in ${m} (range ${hvRange.toFixed(1)} dB).`;
        }
        return out;
      }),
      basis: { el: 'NISAR L2 GCOV + Sentinel-1 RTC · μέσος όρος περιοχής', en: 'NISAR L2 GCOV + Sentinel-1 RTC · area mean' },
    },
  };
}

function surfaceWater(s, wx) {
  const w = s.water_km2;
  if (!w) return null;
  const base = median(w);
  const peak = Math.max(...w);
  if (peak - base < Math.max(5, base)) return null;   // a lake's normal seasonal swing is not a flood
  const i = w.indexOf(peak);
  const recent = i >= w.length - 2;
  const rain = wx ? wxSum(wx, 'rain', addDays(s.dates[i], -14), s.dates[i]) : null;
  return {
    date: s.dates[i], extra: peak - base, rain,
    finding: {
      key: 'flood', level: recent ? 'alert' : 'ok',
      title: recent ? { el: 'Πλημμύρα σε εξέλιξη', en: 'Flooding in progress' } : { el: 'Καταγράφηκε πλημμύρα', en: 'A flood was recorded' },
      text: both((l) => {
        let out = l === 'el'
          ? `Στις ${fd(s.dates[i], l)} το ραντάρ είδε ${peak.toFixed(0)} km² νερού (συνήθως ${base.toFixed(0)}).`
          : `On ${fd(s.dates[i], l)} the radar saw ${peak.toFixed(0)} km² of water (normally ${base.toFixed(0)}).`;
        if (rain != null) out += l === 'el' ? ` Τις 14 προηγούμενες ημέρες είχαν πέσει ${rain.toFixed(0)} mm βροχής.` : ` ${rain.toFixed(0)} mm of rain had fallen in the 14 days before.`;
        if (i + 1 < w.length) out += l === 'el' ? ` Στην επόμενη λήψη (${fd(s.dates[i + 1], l, { day: 'numeric', month: 'short' })}) είχαν μείνει ${w[i + 1].toFixed(0)} km².` : ` By the next pass (${fd(s.dates[i + 1], l, { day: 'numeric', month: 'short' })}) ${w[i + 1].toFixed(0)} km² remained.`;
        return out;
      }),
      basis: { el: 'NISAR HH: το ήρεμο νερό είναι σκοτεινό στο ραντάρ, ακόμη και με σύννεφα', en: 'NISAR HH: calm water is dark to radar, even through cloud' },
    },
  };
}

/** Days until the reservoir reaches its limit, using the drop between the last two measurements up to index i. */
function project(res, i) {
  const perDay = (res.volume[i] - res.volume[i - 1]) / (day(res.dates[i]) - day(res.dates[i - 1]));
  const room = res.volume[i] - res.limit;
  return { perDay, daysLeft: room <= 0 ? 0 : perDay < 0 ? room / -perDay : Infinity };
}

function reservoir(res, th, wx, today) {
  if (!res) return null;
  const n = res.volume.length - 1;
  const now = res.volume[n];
  const { perDay, daysLeft } = project(res, n);
  const below = now <= res.limit;
  const level = below || daysLeft <= th.reservoir_warn_days ? 'alert' : daysLeft <= 2 * th.reservoir_warn_days ? 'watch' : 'ok';
  const crossDate = Number.isFinite(daysLeft) && !below ? addDays(res.dates[n], Math.round(daysLeft)) : null;
  const overdue = crossDate != null && crossDate <= today;        // the projected date has passed with no new measurement
  const daysFromToday = crossDate ? Math.round(day(crossDate) - day(today)) : null;
  // the first measurement at which this warning would already have fired
  let warnedOn = null;
  for (let i = 1; i <= n && !warnedOn; i++) if (project(res, i).daysLeft <= th.reservoir_warn_days) warnedOn = res.dates[i];
  const rainSince = wx && today > res.dates[n] ? wxSum(wx, 'rain', addDays(res.dates[n], 1), today) : null;
  const pct = (now / res.capacity) * 100;

  return {
    now, pct, perDay, daysLeft, crossDate, overdue, warnedOn, below, level, rainSince,
    finding: {
      key: 'reservoir', level,
      title: below ? { el: 'Νερό κάτω από το ελάχιστο όριο', en: 'Water below the minimum limit' }
        : overdue ? { el: 'Το νερό ίσως πέρασε ήδη το όριο', en: 'Water may already be past the limit' }
          : level === 'alert' ? { el: `Το νερό φτάνει στο όριο σε ~${daysFromToday} ημέρες`, en: `Water reaches the limit in ~${daysFromToday} days` }
          : level === 'watch' ? { el: 'Το απόθεμα νερού πέφτει', en: 'Water storage is falling' }
            : { el: 'Απόθεμα νερού εντός ορίων', en: 'Water storage within limits' },
      text: both((l) => {
        let out = l === 'el'
          ? `${res.name.el}: ${now.toFixed(0)} ${res.unit} (${pct.toFixed(0)}% της χωρητικότητας), όριο ${res.limit.toFixed(0)} ${res.unit}.`
          : `${res.name.en}: ${now.toFixed(0)} ${res.unit} (${pct.toFixed(0)}% of capacity), limit ${res.limit.toFixed(0)} ${res.unit}.`;
        if (crossDate && level !== 'ok') {
          out += l === 'el'
            ? ` Στην τελευταία λήψη έχανε ${Math.abs(perDay).toFixed(2)} ${res.unit} την ημέρα· με αυτόν τον ρυθμό ${overdue ? 'πέρασε' : 'περνά'} το όριο γύρω στις ${fd(crossDate, l)}.`
            : ` At the last pass it was losing ${Math.abs(perDay).toFixed(2)} ${res.unit} a day; at that pace it ${overdue ? 'crossed' : 'crosses'} the limit around ${fd(crossDate, l)}.`;
        }
        if (rainSince != null && level !== 'ok') {
          out += l === 'el'
            ? ` Από τότε έχουν πέσει ${rainSince.toFixed(0)} mm βροχής· μόνο η επόμενη λήψη θα δείξει αν ανέκοψαν την πτώση.`
            : ` ${rainSince.toFixed(0)} mm of rain have fallen since; only the next pass will show whether that stopped the drop.`;
        }
        return out;
      }),
      basis: res.method,
    },
  };
}

function aquifer(s) {
  const v = s.vol_loss_hm3;
  if (!v) return null;
  const loss = v.at(-1) - v[0];
  if (loss < 0.5) return null;
  return {
    loss,
    finding: {
      key: 'aquifer', level: 'ok',   // explains the ground finding; it does not raise a second alarm
      title: { el: 'Χάνεται υπόγειο νερό', en: 'Groundwater is being lost' },
      text: both((l) => (l === 'el'
        ? `Η καθίζηση αντιστοιχεί σε όγκο συμπίεσης ${loss.toFixed(1)} hm³ από ${fd(s.dates[0], l)}: τόσο νερό, τουλάχιστον, βγήκε από τα στρώματα που συμπιέστηκαν και δεν αναπληρώθηκε.`
        : `The subsidence amounts to a compaction volume of ${loss.toFixed(1)} hm³ since ${fd(s.dates[0], l)}: at least that much water left the compacted layers and was not replaced.`)),
      basis: { el: 'Βύθιση × έκταση. Κατώτατη ένδειξη, όχι μέτρηση στάθμης γεώτρησης.', en: 'Subsidence × area. A lower bound, not a borehole level measurement.' },
    },
  };
}

function weatherNow(wx, th, today) {
  const i = wx.time.indexOf(today);
  if (i < 0) return null;
  const back = (n, key) => sum(wx[key].slice(Math.max(0, i - n + 1), i + 1));
  const rain7 = back(7, 'rain');
  const rain30 = back(30, 'rain');
  const balance90 = back(90, 'rain') - back(90, 'et0');
  const ahead = wx.time.slice(i + 1).map((d, k) => ({ date: d, rain: wx.rain[i + 1 + k] ?? 0, tmax: wx.tmax[i + 1 + k] }));
  const forecast = sum(ahead.map((a) => a.rain));
  const heavy = ahead.find((a) => a.rain >= th.heavy_rain_mm_day);
  const hot = ahead.find((a) => a.tmax >= th.heat_c);
  let dry = 0;
  for (let k = i; k >= 0 && (wx.rain[k] ?? 0) < 1; k--) dry++;
  const level = heavy || hot || dry >= th.dry_spell_days ? 'watch' : 'ok';

  return {
    rain7, rain30, balance90, forecast, dry, tmax: wx.tmax[i], rainToday: wx.rain[i] ?? 0, level,
    finding: {
      key: 'weather', level, sub: heavy ? 'heavy' : hot ? 'heat' : dry >= th.dry_spell_days ? 'dry' : null,
      title: heavy ? { el: 'Έρχεται ισχυρή βροχή', en: 'Heavy rain is coming' }
        : hot ? { el: 'Έρχεται καύσωνας', en: 'A heatwave is coming' }
          : dry >= th.dry_spell_days ? { el: `${dry} ημέρες χωρίς βροχή`, en: `${dry} days without rain` }
            : { el: 'Καιρός χωρίς ακραία φαινόμενα', en: 'No extreme weather' },
      text: both((l) => {
        let out = l === 'el'
          ? `Σήμερα έως ${wx.tmax[i]?.toFixed(0)}°C, βροχή ${(wx.rain[i] ?? 0).toFixed(1)} mm. Τελευταίες 30 ημέρες: ${rain30.toFixed(0)} mm. Ισοζύγιο 90 ημερών (βροχή − εξατμισοδιαπνοή): ${num(balance90)} mm. Πρόγνωση 7 ημερών: ${forecast.toFixed(0)} mm.`
          : `Today up to ${wx.tmax[i]?.toFixed(0)}°C, rain ${(wx.rain[i] ?? 0).toFixed(1)} mm. Last 30 days: ${rain30.toFixed(0)} mm. 90-day balance (rain − evapotranspiration): ${num(balance90)} mm. 7-day forecast: ${forecast.toFixed(0)} mm.`;
        if (heavy) out += l === 'el' ? ` Στις ${fd(heavy.date, l, { day: 'numeric', month: 'short' })} αναμένονται ${heavy.rain.toFixed(0)} mm σε μία ημέρα.` : ` ${heavy.rain.toFixed(0)} mm expected in one day on ${fd(heavy.date, l, { day: 'numeric', month: 'short' })}.`;
        return out;
      }),
      basis: { el: 'Open-Meteo · ημερήσια δεδομένα επανανάλυσης και πρόγνωση', en: 'Open-Meteo · daily reanalysis and forecast' },
    },
  };
}

/** Weather set against what the radar saw: rain before each pass, notable events, and what followed them. */
function weatherEffects(s, wx, th) {
  const passes = s.dates.map((d, i) => ({
    date: d,
    rain: wxSum(wx, 'rain', i ? addDays(s.dates[i - 1], 1) : addDays(d, -30), d),   // since the previous pass
    rain10: wxSum(wx, 'rain', addDays(d, -9), d),
  }));
  const r = s.hh_db ? corr(passes.map((p) => p.rain10), s.hh_db) : 0;
  const deficit = -(wxSum(wx, 'rain', `${s.dates.at(-1).slice(0, 4)}-05-01`, `${s.dates.at(-1).slice(0, 4)}-09-30`)
    - wxSum(wx, 'et0', `${s.dates.at(-1).slice(0, 4)}-05-01`, `${s.dates.at(-1).slice(0, 4)}-09-30`));

  // notable days inside the satellite record, each paired with the first pass after it
  const events = [];
  const [first, last] = [s.dates[0], s.dates.at(-1)];
  wx.time.forEach((d, i) => {
    if (d < first || d > last) return;
    if ((wx.rain[i] ?? 0) >= th.heavy_rain_mm_day) events.push({ type: 'rain', date: d, value: wx.rain[i] });
    if ((wx.tmax[i] ?? -99) >= th.heat_c) {
      const prev = events.at(-1);
      if (prev?.type === 'heat' && day(d) - day(prev.end) <= 1) { prev.end = d; prev.value = Math.max(prev.value, wx.tmax[i]); prev.days++; } else events.push({ type: 'heat', date: d, end: d, value: wx.tmax[i], days: 1 });
    }
  });
  for (const e of events) {
    const k = s.dates.findIndex((d) => d >= (e.end || e.date));
    if (k > 0) {
      e.pass = s.dates[k];
      e.dHH = s.hh_db ? s.hh_db[k] - s.hh_db[k - 1] : null;
      e.dWater = s.water_km2 ? s.water_km2[k] - s.water_km2[k - 1] : null;
      e.dDisp = s.disp_mm[k] - s.disp_mm[k - 1];
    }
  }
  events.sort((a, b) => (a.date < b.date ? 1 : -1));

  const clear = Math.abs(r) >= 0.5;
  return {
    passes, r, deficit, events,
    finding: {
      key: 'effects', level: 'ok',
      title: clear ? { el: 'Η βροχή φαίνεται στο ραντάρ', en: 'Rain shows up in the radar' } : { el: 'Βροχή και ραντάρ: καμία καθαρή σχέση', en: 'Rain and radar: no clear link' },
      text: both((l) => {
        let out = l === 'el'
          ? `Βροχή 10 ημερών πριν από κάθε λήψη έναντι οπισθοσκέδασης HH: r = ${r.toFixed(2)} σε ${passes.length} λήψεις.`
          : `10-day rain before each pass against HH backscatter: r = ${r.toFixed(2)} over ${passes.length} passes.`;
        if (clear && r > 0) out += l === 'el' ? ' Το βρεγμένο έδαφος επιστρέφει περισσότερο σήμα.' : ' Wet soil returns more signal.';
        if (deficit > 50) out += l === 'el' ? ` Μάιος–Σεπτέμβριος: έλλειμμα ${deficit.toFixed(0)} mm, που καλύφθηκε με άρδευση.` : ` May–September: a ${deficit.toFixed(0)} mm deficit, made up by irrigation.`;
        return out;
      }),
      basis: { el: 'Open-Meteo + NISAR L2 GCOV', en: 'Open-Meteo + NISAR L2 GCOV' },
    },
  };
}

// Compared with the other areas on the same date, not with this area's own year: moisture is seasonal.
function soil(s, th, ref) {
  const v = s.soil_moist_pct;
  if (!v) return null;
  const now = v.at(-1);
  const avg = ref ?? mean(v);
  const drop = ((avg - now) / avg) * 100;
  const level = ref != null && drop >= th.soil_dry_drop_pct ? 'watch' : 'ok';
  return {
    now, avg, drop,
    finding: {
      key: 'soil', level,
      title: level === 'watch' ? { el: 'Το έδαφος ξεραίνεται', en: 'The soil is drying out' } : { el: 'Υγρασία εδάφους κανονική', en: 'Soil moisture normal' },
      text: both((l) => (l === 'el'
        ? `Υγρασία εδάφους ${now.toFixed(0)}% στην τελευταία λήψη· ο μέσος όρος όλων των περιοχών την ίδια μέρα είναι ${avg.toFixed(0)}% (${drop > 0 ? `${drop.toFixed(0)}% πιο ξερό εδώ` : 'εδώ πιο υγρό'}).`
        : `Soil moisture ${now.toFixed(0)}% at the last pass; the average of all areas that day is ${avg.toFixed(0)}% (${drop > 0 ? `${drop.toFixed(0)}% drier here` : 'wetter here'}).`)),
      basis: { el: 'NISAR L-band · το νερό αυξάνει τη διηλεκτρική σταθερά του εδάφους και άρα το σήμα', en: 'NISAR L-band · water raises the soil dielectric constant and therefore the signal' },
    },
  };
}

function wetland(s, th) {
  const r = s.reed_km2;
  if (!r || Math.max(...r) < 0.05) return null;
  const loss = ((r[0] - r.at(-1)) / r[0]) * 100;
  const ha = s.cleared_ha ? s.cleared_ha.at(-1) - s.cleared_ha[0] : 0;
  const level = ha >= th.cleared_alert_ha ? 'alert' : ha >= th.cleared_watch_ha || loss >= th.reed_loss_pct ? 'watch' : 'ok';
  const first = s.cleared_ha ? s.dates[s.cleared_ha.findIndex((x) => x > 0)] : null;
  return {
    loss, ha, level,
    finding: {
      key: 'wetland', level,
      title: level === 'ok' ? { el: 'Καλαμιώνες σταθεροί', en: 'Reed beds stable' } : { el: 'Ο υγρότοπος γίνεται χωράφι', en: 'Wetland is turning into fields' },
      text: both((l) => {
        if (level === 'ok') return l === 'el' ? `Καλαμιώνες ${r.at(-1).toFixed(1)} km², χωρίς αξιόλογη αλλαγή.` : `Reed beds ${r.at(-1).toFixed(1)} km², no notable change.`;
        return l === 'el'
          ? `${ha.toFixed(0)} ha καλαμιώνα άλλαξαν σε γυμνό, οργωμένο έδαφος${first ? ` από ${fd(first, l)}` : ''} (−${loss.toFixed(0)}% έκτασης). Το HV έπεσε (χάθηκε ο όγκος της βλάστησης) και το HH άλλαξε από διπλή ανάκλαση σε τραχιά επιφάνεια: η υπογραφή του οργώματος.`
          : `${ha.toFixed(0)} ha of reeds turned into bare, ploughed ground${first ? ` since ${fd(first, l)}` : ''} (−${loss.toFixed(0)}% of the area). HV dropped (the vegetation volume is gone) and HH went from double bounce to a rough surface: the signature of ploughing.`;
      }),
      basis: { el: 'NISAR HV (όγκος βλάστησης) + HH (τραχύτητα, διπλή ανάκλαση)', en: 'NISAR HV (vegetation volume) + HH (roughness, double bounce)' },
    },
  };
}

/** What the region could do about each finding. Suggestions for the competent authority, not decisions. */
const ACTIONS = {
  reservoir: { el: 'Ενημέρωση Διεύθυνσης Υδάτων και ΤΟΕΒ· σταδιακός περιορισμός των αρδευτικών απολήψεων από τη λίμνη πριν από το όριο, όχι μετά.', en: 'Notify the Water Directorate and irrigation boards; reduce irrigation withdrawals from the lake gradually before the limit, not after.' },
  wetland: { el: 'Αυτοψία από τον Φορέα Διαχείρισης (ΟΦΥΠΕΚΑ) στα σημεία που άλλαξαν· έλεγχος χρήσεων γης στη ζώνη προστασίας.', en: 'Field inspection by the protected-area authority at the changed spots; check land use inside the protection zone.' },
  ground: { el: 'Έλεγχος αδειών και μετρητών στις γεωτρήσεις της ζώνης, περιορισμός αντλήσεων· σταθμός GNSS για επιβεβαίωση.', en: 'Check borehole permits and meters in the zone, limit pumping; a GNSS station to confirm.' },
  soil: { el: 'Άρδευση με βάση τη μετρημένη υγρασία αντί για σταθερό ωράριο: λιγότερο νερό από λίμνη και γεωτρήσεις.', en: 'Irrigate by measured moisture instead of a fixed schedule: less water from the lake and the boreholes.' },
  egms: { el: 'Να μη ληφθεί απόφαση μόνο με αυτό: επιβεβαίωση με επόμενες λήψεις ή επιτόπια μέτρηση.', en: 'Do not decide on this alone: confirm with the next passes or a field measurement.' },
  flood: { el: 'Ενημέρωση Πολιτικής Προστασίας· έλεγχος αναχωμάτων και αγροτικών δρόμων στη ζώνη που πλημμύρισε.', en: 'Notify Civil Protection; check embankments and farm roads in the flooded zone.' },
  'weather.heavy': { el: 'Προειδοποίηση Πολιτικής Προστασίας, καθαρισμός τάφρων και φρεατίων πριν από τη βροχή.', en: 'Civil Protection warning; clear ditches and drains before the rain.' },
  'weather.heat': { el: 'Ενημέρωση αγροτών και ΤΟΕΒ για αυξημένη ζήτηση νερού· αυστηρότερη τήρηση ωραρίων.', en: 'Warn farmers and irrigation boards about higher water demand; enforce schedules.' },
  'weather.dry': { el: 'Ενημέρωση αγροτών και ΤΟΕΒ για αυξημένη ζήτηση νερού· αυστηρότερη τήρηση ωραρίων.', en: 'Warn farmers and irrigation boards about higher water demand; enforce schedules.' },
};

/**
 * "What if" for the lake: rain and irrigation changed by a percentage next season.
 * A deliberately simple water balance built only from the measured series:
 *   winter gain (last low → high) scales with rain; summer loss (high → latest low) is 70 % irrigation, 30 % evaporation.
 *   lake area follows storage through a power law fitted to the radar areas; ground sinking scales with pumping.
 */
export function scenario(region, a, { rain = 0, irrigation = 0 } = {}) {
  const res = region.reservoir;
  if (!res?.volume?.length) return null;
  const v = res.volume;
  const hi = v.indexOf(Math.max(...v));
  const lowBefore = Math.min(...v.slice(0, hi + 1));
  const gain = v[hi] - lowBefore;
  const loss = v[hi] - v.at(-1);
  const r = rain / 100;
  const i = irrigation / 100;
  const peak = v.at(-1) + gain * (1 + r);
  const low = Math.max(0, peak - loss * (0.7 * (1 + i) + 0.3));
  const base = Math.max(0, v.at(-1) + gain - loss);

  let areaOf = () => null;
  if (res.area_km2?.length === v.length) {
    const xs = v.map(Math.log);
    const ys = res.area_km2.map((x) => Math.log(Math.max(x, 0.01)));
    const b = corr(xs, ys) * Math.sqrt(sum(ys.map((y) => (y - mean(ys)) ** 2)) / sum(xs.map((x) => (x - mean(xs)) ** 2)));
    const c = Math.exp(mean(ys) - b * mean(xs));
    areaOf = (vol) => c * Math.max(vol, 0.01) ** b;
  }
  const below = Math.max(0, res.limit - low);
  const daysBelow = below > 0 ? Math.round((150 * below) / Math.max(loss * (0.7 * (1 + i) + 0.3), 1)) : 0;
  return {
    peak, low, base, limit: res.limit, unit: res.unit,
    area: areaOf(low), areaNow: res.area_km2?.at(-1) ?? null, areaBase: areaOf(base),
    scale: res.area_km2 ? Math.sqrt(areaOf(low) / areaOf(v.at(-1))) : 1,
    daysBelow,
    sink: a.ground.rate * (1 + i),
  };
}

// ───────────────────────────────────────────────────────── one area
/**
 * @param region  one entry of regions.json
 * @param wx      weather series for this area, or null
 * @param th      thresholds from regions.json
 * @param opts    { today:'YYYY-MM-DD', refName }
 */
export function assess(region, wx, th, { today, refName = 'EGMS', soilRef = null }) {
  const s = region.series;
  const g = ground(s, th);
  const out = {
    id: region.id, lastPass: s.dates.at(-1),
    ground: g,
    reference: reference(s, th, g.rate, refName),
    sensors: sensors(s),
    flood: surfaceWater(s, wx),
    reservoir: reservoir(region.reservoir, th, wx, today),
    aquifer: aquifer(s),
    soil: soil(s, th, soilRef),
    wetland: wetland(s, th),
    weather: wx ? weatherNow(wx, th, today) : null,
    effects: wx ? weatherEffects(s, wx, th) : null,
  };
  // order = the order the findings are shown in
  out.findings = ['reservoir', 'wetland', 'ground', 'soil', 'aquifer', 'reference', 'flood', 'weather', 'effects', 'sensors']
    .map((k) => out[k]?.finding).filter(Boolean)
    .sort((a, b) => RANK[b.level] - RANK[a.level]);
  for (const f of out.findings) {
    if (f.level !== 'ok') f.action = ACTIONS[f.sub ? `${f.key}.${f.sub}` : f.key] ?? null;
  }
  out.status = out.findings[0]?.level ?? 'ok';
  out.headline = out.findings[0];
  return out;
}
