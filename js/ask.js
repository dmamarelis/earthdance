// "Ask EarthDance": answers built from the forecast (js/forecast.js) and the measurements, sentence by sentence.
// No language model: each answer is a fixed template filled with numbers the app computed, so it cannot make
// up a figure. A typed question is matched to one of the answers by keywords; anything else gets "I can't answer that".

import { runForecast } from './forecast.js';

const day = (iso) => Date.parse(`${iso}T00:00:00Z`) / 864e5;
const fd = (iso, l, opts = { day: 'numeric', month: 'short', year: 'numeric' }) =>
  iso ? new Intl.DateTimeFormat(l === 'el' ? 'el-GR' : 'en-GB', opts).format(new Date(`${iso}T00:00:00`)) : '–';
const n0 = (v) => (v == null ? '–' : Math.round(v).toString());
const n1 = (v, l) => (v == null ? '–' : v.toFixed(1).replace('.', l === 'el' ? ',' : '.'));
const both = (fn) => ({ el: fn('el'), en: fn('en') });
const days = (n, l) => (l === 'el' ? `${n} ${n === 1 ? 'ημέρα' : 'ημέρες'}` : `${n} ${n === 1 ? 'day' : 'days'}`);

export const QUESTIONS = [
  { id: 'now', el: 'Πώς είναι η λίμνη σήμερα;', en: 'How is the lake today?' },
  { id: 'when', el: 'Πότε πέφτει ξανά κάτω από το όριο;', en: 'When does it fall below the limit again?' },
  { id: 'rain', el: 'Κι αν βρέξει 20% λιγότερο;', en: 'What if it rains 20% less?' },
  { id: 'irr', el: 'Κι αν μειωθεί η άρδευση 20%;', en: 'What if irrigation drops 20%?' },
  { id: 'act', el: 'Τι πρέπει να κάνει η Περιφέρεια;', en: 'What should the Region do?' },
  { id: 'trust', el: 'Πόσο σίγουρη είναι η πρόβλεψη;', en: 'How sure is the forecast?' },
];

// keyword → answer (accents and case removed before matching)
const ROUTES = [
  ['rain', ['βροχ', 'βρεξ', 'ξηρασ', 'rain', 'drought', 'dry winter']],
  ['irr', ['αρδευ', 'ποτισ', 'χωραφ', 'αγροτ', 'irrigat', 'farm', 'crop']],
  ['trust', ['σιγουρ', 'αξιοπιστ', 'λαθος', 'σφαλμα', 'ακριβ', 'εμπιστ', 'sure', 'trust', 'accura', 'error', 'reliab', 'confiden']],
  ['act', ['κανει', 'κανουμε', 'μετρα', 'ενεργει', 'περιφερει', 'προστιμ', 'should', 'action', 'do ', 'region', 'fine', 'penalt']],
  ['when', ['ποτε', 'οριο', 'καλοκαιρ', 'επομεν', 'στερεψ', 'when', 'limit', 'summer', 'next']],
  ['now', ['σημερα', 'τωρα', 'ποσο νερο', 'κατασταση', 'today', 'now', 'how much water', 'status']],
];
const norm = (s) => s.toLowerCase().normalize('NFD').replace(/[̀-ͯ]/g, '').replace(/ς/g, 'σ');
export function route(text) {
  const s = ` ${norm(text)} `;
  for (const [id, keys] of ROUTES) if (keys.some((k) => s.includes(norm(k).replace(/ς/g, 'σ')))) return id;
  return null;
}

/** Everything the answers and the alert need, computed once per weather load. */
export function brief(fc, res, wx, today, lakeName) {
  const base = runForecast(fc, res, wx, today);
  const lessRain = runForecast(fc, res, wx, today, { rain: -20 });
  const lessIrr = runForecast(fc, res, wx, today, { irrigation: -20 });
  const fit = fc.fit;
  const peakI = fit.measured.indexOf(Math.max(...fit.measured));
  return {
    base, lessRain, lessIrr, today, lakeName, limit: res.limit,
    last: { date: res.dates.at(-1), area: res.area_km2.at(-1) },
    peak: { date: fit.dates[peakI], area: fit.measured[peakI] },
    passes: fit.dates.length, rmse: fc.rmse, bt: fc.backtest,
    years: [fc.climate.years[0], fc.climate.years.at(-2)],
  };
}

const shift = (a, b) => (a && b ? Math.round(day(b) - day(a)) : null);

/** The lake finding shown in the Status view (replaces the straight-line projection when a forecast exists). */
export function finding(B, th) {
  const F = B.base;
  const belowNow = F.now.p50 < B.limit;
  const toNext = F.next ? day(F.next.mid) - day(B.today) : Infinity;
  const level = belowNow || toNext <= th.reservoir_warn_days ? 'alert' : F.next ? 'watch' : 'ok';
  return {
    key: 'reservoir', level,
    title: belowNow
      ? { el: 'Η λίμνη είναι κάτω από το όριο (εκτίμηση)', en: 'The lake is below the limit (estimate)' }
      : F.next ? both((l) => (l === 'el' ? `Η λίμνη ξαναπέφτει κάτω από το όριο γύρω στις ${fd(F.next.mid, l)}` : `The lake falls below the limit again around ${fd(F.next.mid, l)}`))
        : { el: 'Η λίμνη μένει πάνω από το όριο', en: 'The lake stays above the limit' },
    text: both((l) => lines(B, l).slice(0, 2).join(' ')),
    action: both((l) => lines(B, l)[2]),
    basis: both((l) => (l === 'el'
      ? `NISAR (${B.passes} μετρήσεις) + καιρός Open-Meteo + μοντέλο ισοζυγίου νερού · σφάλμα ±${n1(B.rmse, l)} km²`
      : `NISAR (${B.passes} measurements) + Open-Meteo weather + water-balance model · error ±${n1(B.rmse, l)} km²`)),
  };
}

/** The three lines of the alert: what we measured, what is coming, what to do. */
export function lines(B, l) {
  const F = B.base;
  const pct = Math.round((B.last.area / B.peak.area) * 100);
  const el = l === 'el';
  const measured = el
    ? `Τελευταία λήψη NISAR, ${fd(B.last.date, l)}: ${n1(B.last.area, l)} km² νερού, ${pct}% της έκτασης του ${fd(B.peak.date, l, { month: 'long' })} (${n1(B.peak.area, l)} km²). Όριο: ${n1(B.limit, l)} km².`
    : `Last NISAR pass, ${fd(B.last.date, l)}: ${n1(B.last.area, l)} km² of water, ${pct}% of the ${fd(B.peak.date, l, { month: 'long' })} extent (${n1(B.peak.area, l)} km²). Limit: ${n1(B.limit, l)} km².`;
  let coming = '';
  if (F.cross && F.cross.mid <= B.today) {
    coming += el ? `Το μοντέλο εκτιμά ότι πέρασε κάτω από το όριο γύρω στις ${fd(F.cross.mid, l)}. `
      : `The model estimates it went below the limit around ${fd(F.cross.mid, l)}. `;
  }
  coming += el ? `Σήμερα: ~${n0(F.now.p50)} km² (εύρος ${n0(F.now.p10)}–${n0(F.now.p90)}). `
    : `Today: ~${n0(F.now.p50)} km² (range ${n0(F.now.p10)}–${n0(F.now.p90)}). `;
  if (F.next) {
    coming += el
      ? `Με καιρό σαν των ετών ${B.years[0]}–${B.years[1]}, ξαναπέφτει κάτω από το όριο γύρω στις ${fd(F.next.mid, l)} (πιθανότερα ${fd(F.next.early, l)} – ${fd(F.next.late, l)}) και μένει από κάτω ~${F.daysBelowNext} ημέρες.`
      : `With weather like ${B.years[0]}–${B.years[1]}, it falls below the limit again around ${fd(F.next.mid, l)} (most likely ${fd(F.next.early, l)} – ${fd(F.next.late, l)}) and stays below for ~${F.daysBelowNext} days.`;
  }
  const gain = shift(F.next?.mid, B.lessIrr.next?.mid);
  const fewer = F.daysBelowNext - B.lessIrr.daysBelowNext;
  const by = F.next?.early || F.next?.mid;
  const effect = el
    ? [gain > 0 ? `η πτώση καθυστερεί ~${days(gain, l)}` : '', fewer > 0 ? `οι ημέρες κάτω από το όριο μειώνονται κατά ~${fewer}` : ''].filter(Boolean).join(' και ')
    : [gain > 0 ? `the fall comes ~${days(gain, l)} later` : '', fewer > 0 ? `the days below the limit drop by ~${fewer}` : ''].filter(Boolean).join(' and ');
  const todo = el
    ? `Σχέδιο άρδευσης για τα χωράφια γύρω από τη λίμνη${by ? ` πριν από τις ${fd(by, l)}` : ''}.${effect ? ` Με 20% λιγότερο νερό άρδευσης ${effect}.` : ''} Ενημέρωση Διεύθυνσης Υδάτων και ΤΟΕΒ τώρα, όχι όταν περάσει το όριο.`
    : `An irrigation plan for the fields around the lake${by ? ` before ${fd(by, l)}` : ''}.${effect ? ` With 20% less irrigation water ${effect}.` : ''} Notify the Water Directorate and irrigation boards now, not once the limit is crossed.`;
  return [measured, coming, todo];
}

/** The back-test sentence: what the model would have said with the passes up to the cut-off. */
export function backtestLine(B, l) {
  const bt = B.bt;
  if (!bt) return '';
  const lead = bt.cross ? Math.round(day(bt.cross.mid) - day(bt.cutoff)) : null;
  return l === 'el'
    ? `Έλεγχος στο παρελθόν: με λήψεις μόνο ως τις ${fd(bt.cutoff, l)}, το μοντέλο θα έλεγε ότι η λίμνη πέφτει κάτω από το όριο γύρω στις ${fd(bt.cross?.mid, l)} (${fd(bt.cross?.early, l)} – ${fd(bt.cross?.late, l)}), δηλαδή ${lead} ημέρες πριν. Για τις ${fd(bt.target, l)} προέβλεψε ${n1(bt.predicted, l)} km²· το NISAR μέτρησε ${n1(bt.observed, l)} km².`
    : `Back-test: with passes only up to ${fd(bt.cutoff, l)}, the model would have said the lake falls below the limit around ${fd(bt.cross?.mid, l)} (${fd(bt.cross?.early, l)} – ${fd(bt.cross?.late, l)}), ${lead} days ahead. For ${fd(bt.target, l)} it predicted ${n1(bt.predicted, l)} km²; NISAR measured ${n1(bt.observed, l)} km².`;
}

export function answer(id, B, l) {
  const F = B.base;
  const el = l === 'el';
  const L3 = lines(B, l);
  const src = el ? 'Πηγές' : 'Sources';
  const S = {
    nisar: `NISAR L2 GCOV · ${B.passes} ${el ? 'μετρήσεις' : 'measurements'}`,
    wx: el ? `Open-Meteo: καιρός ως ${fd(F.weatherUntil, l)} και ${B.years[0]}–${B.years[1]}` : `Open-Meteo: weather up to ${fd(F.weatherUntil, l)} and ${B.years[0]}–${B.years[1]}`,
    model: el ? 'μοντέλο ισοζυγίου νερού (tools/fit_forecast.py)' : 'water-balance model (tools/fit_forecast.py)',
  };
  const out = (text, sources) => ({ text, sources: `${src}: ${sources.join(' · ')}` });
  switch (id) {
    case 'now': {
      const gap = Math.round(day(B.today) - day(B.last.date));
      return out([
        L3[0],
        el ? `Από τότε (${gap} ημέρες) δεν υπάρχει νέα λήψη, οπότε το σήμερα είναι εκτίμηση του μοντέλου με τον πραγματικό καιρό ως τις ${fd(F.weatherUntil, l)}.`
          : `There has been no pass since (${gap} days), so today is the model's estimate with the real weather up to ${fd(F.weatherUntil, l)}.`,
        L3[1].split('. ').slice(0, F.cross?.mid <= B.today ? 2 : 1).join('. ').replace(/\.?$/, '.'),
        el ? 'Η επόμενη λήψη NISAR θα το επιβεβαιώσει ή θα το διορθώσει.' : 'The next NISAR pass will confirm or correct it.',
      ], [S.nisar, S.wx, S.model]);
    }
    case 'when': {
      if (!F.next) return out([el ? 'Σε κανένα σενάριο καιρού δεν πέφτει ξανά κάτω από το όριο μέσα στον επόμενο χρόνο.' : 'In no weather scenario does it fall below the limit again within a year.'], [S.model]);
      const miss = Math.round((1 - F.next.share) * 100);
      return out([
        el ? `Έτρεξα το μοντέλο με τον καιρό καθενός από τα έτη ${B.years[0]}–${B.years[1]}, τρεις φορές το καθένα (${F.members} σενάρια).`
          : `I ran the model with the weather of each year ${B.years[0]}–${B.years[1]}, three times each (${F.members} scenarios).`,
        el ? `Μεσαίο σενάριο: η λίμνη πέφτει κάτω από το όριο (${n1(B.limit, l)} km²) γύρω στις ${fd(F.next.mid, l)}. Στα μισά σενάρια αυτό γίνεται από ${fd(F.next.early, l)} έως ${fd(F.next.late, l)}.`
          : `Middle scenario: the lake falls below the limit (${n1(B.limit, l)} km²) around ${fd(F.next.mid, l)}. In half the scenarios this happens between ${fd(F.next.early, l)} and ${fd(F.next.late, l)}.`,
        el ? `Το καλοκαίρι του ${F.next.mid.slice(0, 4)} μένει κάτω από το όριο ~${F.daysBelowNext} ημέρες.` : `In summer ${F.next.mid.slice(0, 4)} it stays below the limit for ~${F.daysBelowNext} days.`,
        ...(miss >= 5 ? [el ? `Σε ${miss}% των σεναρίων ο χειμώνας δεν την ανεβάζει καν πάνω από το όριο.` : `In ${miss}% of the scenarios winter does not even lift it above the limit.`] : []),
      ], [S.nisar, S.wx, S.model]);
    }
    case 'rain':
    case 'irr': {
      const alt = id === 'rain' ? B.lessRain : B.lessIrr;
      const d = shift(F.next?.mid, alt.next?.mid);
      const what = id === 'rain'
        ? (el ? 'Με 20% λιγότερη βροχή από σήμερα και μετά' : 'With 20% less rain from today on')
        : (el ? 'Με 20% λιγότερο νερό άρδευσης (15 Απρ – 30 Σεπ)' : 'With 20% less irrigation water (15 Apr – 30 Sep)');
      const move = d == null ? '' : d < 0 ? `${days(-d, l)} ${el ? 'νωρίτερα' : 'earlier'}` : d > 0 ? `${days(d, l)} ${el ? 'αργότερα' : 'later'}` : (el ? 'την ίδια περίπου μέρα' : 'about the same day');
      return out([
        el ? `${what}, η λίμνη πέφτει κάτω από το όριο γύρω στις ${fd(alt.next?.mid, l)}, ${move} από το μεσαίο σενάριο (${fd(F.next?.mid, l)}).`
          : `${what}, the lake falls below the limit around ${fd(alt.next?.mid, l)}, ${move} than the middle scenario (${fd(F.next?.mid, l)}).`,
        el ? `Ημέρες κάτω από το όριο το επόμενο καλοκαίρι: ${alt.daysBelowNext} αντί για ${F.daysBelowNext}.` : `Days below the limit next summer: ${alt.daysBelowNext} instead of ${F.daysBelowNext}.`,
        id === 'irr'
          ? (el ? 'Υπόθεση: το καλοκαίρι το 70% των απωλειών είναι άρδευση και το 30% εξάτμιση. Δεν έχουμε μετρημένες απολήψεις, οπότε ο αριθμός δείχνει κατεύθυνση.' : 'Assumption: in summer 70% of the loss is irrigation and 30% evaporation. We have no metered withdrawals, so the figure shows direction.')
          : (el ? 'Η αλλαγή εφαρμόζεται σε κάθε σενάριο καιρού· τα ίδια σενάρια, απλώς με λιγότερη βροχή.' : 'The change is applied to every weather scenario: the same scenarios, with less rain.'),
      ], [S.wx, S.model]);
    }
    case 'act':
      return out([
        el ? 'Προτάσεις προς τις αρμόδιες υπηρεσίες, όχι αποφάσεις:' : 'Suggestions for the competent authorities, not decisions:',
        `1. ${L3[2]}`,
        el ? `2. Νέα λήψη NISAR κάθε 12 ημέρες: η εφαρμογή ξαναϋπολογίζει την πρόβλεψη με κάθε μέτρηση.` : '2. A new NISAR pass every 12 days: the app recomputes the forecast with each measurement.',
        el ? '3. Να αντικατασταθεί το ενδεικτικό όριο με το επίσημο του Σχεδίου Διαχείρισης (EL10).' : '3. Replace the indicative limit with the official one from the River Basin Management Plan (EL10).',
      ], [S.nisar, S.model]);
    case 'trust':
      return out([
        el ? `Το μοντέλο έχει 4 παραμέτρους, προσαρμοσμένες στις ${B.passes} μετρήσεις έκτασης του 2026. Τυπικό σφάλμα: ±${n1(B.rmse, l)} km².`
          : `The model has 4 parameters, fitted to the ${B.passes} area measurements of 2026. Typical error: ±${n1(B.rmse, l)} km².`,
        backtestLine(B, l),
        el ? 'Αδυναμίες: δεν έχουμε ακόμα μέτρηση χειμώνα, οπότε το πόσο γρήγορα γεμίζει η λίμνη με τη βροχή είναι το πιο αβέβαιο κομμάτι (γι\' αυτό το εύρος είναι φαρδύ). Το όριο είναι ενδεικτικό (25% της μέγιστης έκτασης).'
          : 'Weak points: we have no winter measurement yet, so how fast rain refills the lake is the most uncertain part (hence the wide range). The limit is indicative (25% of the largest extent).',
      ], [S.nisar, S.wx, S.model]);
    default:
      return out([el ? 'Δεν μπορώ να απαντήσω σε αυτό με τα δεδομένα που έχω. Δοκίμασε μία από τις ερωτήσεις παραπάνω.' : 'I cannot answer that from the data I have. Try one of the questions above.'], []);
  }
}
