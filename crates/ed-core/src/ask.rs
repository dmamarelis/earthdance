//! "Ask EarthDance" (port of `js/ask.js`): answers built from the forecast and the measurements, sentence by
//! sentence. No language model: each answer is a fixed template filled with numbers the app computed, so it
//! cannot make up a figure. A typed question is matched to an answer by keywords; anything else gets
//! "I can't answer that".

use serde::Serialize;
use unicode_normalization::UnicodeNormalization;

use crate::forecast::{self, Options, Run};
use crate::insights::{Finding, Level};
use crate::model::{Backtest, ForecastModel, Reservoir, Thresholds, Weather};
use crate::stats::argmax;
use crate::text::{fd, fmt_date, js_number, js_round, n0, n1, DateStyle, Lang, Text};
use crate::Date;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Question {
    Now,
    When,
    Rain,
    Irr,
    Act,
    Trust,
    /// Not understood.
    None,
}

impl Question {
    pub const LISTED: [Question; 6] = [Question::Now, Question::When, Question::Rain, Question::Irr, Question::Act, Question::Trust];

    pub const fn id(self) -> &'static str {
        match self {
            Question::Now => "now",
            Question::When => "when",
            Question::Rain => "rain",
            Question::Irr => "irr",
            Question::Act => "act",
            Question::Trust => "trust",
            Question::None => "none",
        }
    }

    pub fn from_id(id: &str) -> Self {
        Self::LISTED.into_iter().find(|q| q.id() == id).unwrap_or(Question::None)
    }

    /// The suggested wording of the question.
    pub fn label(self, l: Lang) -> &'static str {
        let (el, en) = match self {
            Question::Now => ("Πώς είναι η λίμνη σήμερα;", "How is the lake today?"),
            Question::When => ("Πότε πέφτει ξανά κάτω από το όριο;", "When does it fall below the limit again?"),
            Question::Rain => ("Κι αν βρέξει 20% λιγότερο;", "What if it rains 20% less?"),
            Question::Irr => ("Κι αν μειωθεί η άρδευση 20%;", "What if irrigation drops 20%?"),
            Question::Act => ("Τι πρέπει να κάνει η Περιφέρεια;", "What should the Region do?"),
            Question::Trust => ("Πόσο σίγουρη είναι η πρόβλεψη;", "How sure is the forecast?"),
            Question::None => ("", ""),
        };
        if l == Lang::El { el } else { en }
    }
}

// keyword → answer (accents and case removed before matching); the first matching route wins
const ROUTES: [(Question, &[&str]); 6] = [
    (Question::Rain, &["βροχ", "βρεξ", "ξηρασ", "rain", "drought", "dry winter"]),
    (Question::Irr, &["αρδευ", "ποτισ", "χωραφ", "αγροτ", "irrigat", "farm", "crop"]),
    (Question::Trust, &["σιγουρ", "αξιοπιστ", "λαθος", "σφαλμα", "ακριβ", "εμπιστ", "sure", "trust", "accura", "error", "reliab", "confiden"]),
    (Question::Act, &["κανει", "κανουμε", "μετρα", "ενεργει", "περιφερει", "προστιμ", "should", "action", "do ", "region", "fine", "penalt"]),
    (Question::When, &["ποτε", "οριο", "καλοκαιρ", "επομεν", "στερεψ", "when", "limit", "summer", "next"]),
    (Question::Now, &["σημερα", "τωρα", "ποσο νερο", "κατασταση", "today", "now", "how much water", "status"]),
];

/// Lower case, accents stripped (NFD minus the combining marks U+0300–U+036F), final sigma folded.
fn norm(s: &str) -> String {
    s.to_lowercase().nfd().filter(|c| !('\u{0300}'..='\u{036f}').contains(c)).map(|c| if c == 'ς' { 'σ' } else { c }).collect()
}

/// Which answer a typed question gets, if any.
pub fn route(text: &str) -> Option<Question> {
    let s = format!(" {} ", norm(text));
    ROUTES.iter().find(|(_, keys)| keys.iter().any(|k| s.contains(&norm(k)))).map(|(q, _)| *q)
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Measured {
    pub date: Date,
    pub area: f64,
}

/// Everything the answers and the alert need, computed once per weather load.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Brief {
    pub base: Run,
    pub less_rain: Run,
    pub less_irr: Run,
    pub today: Date,
    pub lake_name: Text,
    pub limit: f64,
    pub last: Measured,
    pub peak: Measured,
    pub passes: usize,
    pub rmse: f64,
    pub bt: Option<Backtest>,
    /// First and last complete scenario year (replayed years).
    pub years: (i32, i32),
    /// With a weather generator: the ERA5 years it was fitted to.
    pub generator: Option<(i32, i32)>,
}

pub fn brief(fc: &ForecastModel, res: &Reservoir, wx: Option<&Weather>, today: Date, lake_name: &Text) -> Brief {
    let base = forecast::run(fc, res, wx, today, Options::what_if(0.0, 0.0));
    let less_rain = forecast::run(fc, res, wx, today, Options::what_if(-20.0, 0.0));
    let less_irr = forecast::run(fc, res, wx, today, Options::what_if(0.0, -20.0));
    let peak_i = argmax(&fc.fit.measured);
    let ys = &fc.climate.years;
    let area = res.area_km2.as_ref().unwrap_or(&res.volume);
    Brief {
        base,
        less_rain,
        less_irr,
        today,
        lake_name: lake_name.clone(),
        limit: res.limit,
        last: Measured { date: res.dates[res.dates.len() - 1], area: area[area.len() - 1] },
        peak: Measured { date: fc.fit.dates[peak_i], area: fc.fit.measured[peak_i] },
        passes: fc.fit.dates.len(),
        rmse: fc.rmse,
        bt: fc.backtest.clone(),
        years: (ys[0], ys[ys.len().saturating_sub(2)]),
        generator: fc.generator.as_ref().map(|g| g.years),
    }
}

fn shift(a: Option<Date>, b: Option<Date>) -> Option<i32> {
    Some(b?.since(a?))
}

fn days(n: i32, l: Lang) -> String {
    match (l, n == 1) {
        (Lang::El, true) => format!("{n} ημέρα"),
        (Lang::El, false) => format!("{n} ημέρες"),
        (Lang::En, true) => format!("{n} day"),
        (Lang::En, false) => format!("{n} days"),
    }
}

/// The lake finding (replaces the straight-line projection when a forecast exists).
pub fn finding(b: &Brief, th: &Thresholds) -> Finding {
    let f = &b.base;
    let below_now = f.now.p50 < b.limit;
    let to_next = f.next.map_or(f64::INFINITY, |n| f64::from(n.mid.since(b.today)));
    let level = if below_now || to_next <= th.reservoir_warn_days {
        Level::Alert
    } else if f.next.is_some() {
        Level::Watch
    } else {
        Level::Ok
    };
    Finding {
        key: "reservoir",
        level,
        sub: None,
        title: if below_now {
            Text::new("Η λίμνη είναι κάτω από το όριο (εκτίμηση)", "The lake is below the limit (estimate)")
        } else if let Some(n) = f.next {
            Text::both(|l| match l {
                Lang::El => format!("Η λίμνη ξαναπέφτει κάτω από το όριο γύρω στις {}", fd(Some(n.mid), l)),
                Lang::En => format!("The lake falls below the limit again around {}", fd(Some(n.mid), l)),
            })
        } else {
            Text::new("Η λίμνη μένει πάνω από το όριο", "The lake stays above the limit")
        },
        text: Text::both(|l| lines(b, l)[..2].join(" ")),
        action: Some(Text::both(|l| lines(b, l)[2].clone())),
        basis: Text::both(|l| match l {
            Lang::El => format!("NISAR ({} μετρήσεις) + καιρός Open-Meteo + μοντέλο ισοζυγίου νερού · σφάλμα ±{} km²", b.passes, n1(Some(b.rmse), l)),
            Lang::En => format!("NISAR ({} measurements) + Open-Meteo weather + water-balance model · error ±{} km²", b.passes, n1(Some(b.rmse), l)),
        }),
    }
}

/// The three lines of the alert: what we measured, what is coming, what to do.
pub fn lines(b: &Brief, l: Lang) -> [String; 3] {
    let f = &b.base;
    let pct = js_number(js_round(b.last.area / b.peak.area * 100.0));
    let el = l == Lang::El;
    let month = fmt_date(b.peak.date, l, DateStyle::MonthLong);
    let measured = if el {
        format!(
            "Τελευταία λήψη NISAR, {}: {} km² νερού, {pct}% της έκτασης του {month} ({} km²). Όριο: {} km².",
            fd(Some(b.last.date), l),
            n1(Some(b.last.area), l),
            n1(Some(b.peak.area), l),
            n1(Some(b.limit), l)
        )
    } else {
        format!(
            "Last NISAR pass, {}: {} km² of water, {pct}% of the {month} extent ({} km²). Limit: {} km².",
            fd(Some(b.last.date), l),
            n1(Some(b.last.area), l),
            n1(Some(b.peak.area), l),
            n1(Some(b.limit), l)
        )
    };
    let mut coming = String::new();
    if let Some(m) = f.cross.and_then(|c| c.mid).filter(|&m| m <= b.today) {
        coming += &if el {
            format!("Το μοντέλο εκτιμά ότι πέρασε κάτω από το όριο γύρω στις {}. ", fd(Some(m), l))
        } else {
            format!("The model estimates it went below the limit around {}. ", fd(Some(m), l))
        };
    }
    let (a, lo, hi) = (n0(Some(f.now.p50)), n0(Some(f.now.p10)), n0(Some(f.now.p90)));
    coming += &if el { format!("Σήμερα: ~{a} km² (εύρος {lo}–{hi}). ") } else { format!("Today: ~{a} km² (range {lo}–{hi}). ") };
    if let Some(n) = f.next {
        let (y0, y1) = b.years;
        let (mid, early, late) = (fd(Some(n.mid), l), fd(Some(n.early), l), fd(Some(n.late), l));
        let d = f.days_below_next;
        coming += &if let Some((g0, g1)) = b.generator {
            let ny = crate::forecast::SYNTHETIC_YEARS;
            if el {
                format!("Σε {ny} προσομοιωμένες χρονιές καιρού (αλυσίδα Markov από το ERA5 {g0}–{g1}), ξαναπέφτει κάτω από το όριο γύρω στις {mid} (πιθανότερα {early} – {late}) και μένει από κάτω ~{d} ημέρες.")
            } else {
                format!("Over {ny} simulated years of weather (a Markov chain fitted to ERA5 {g0}–{g1}), it falls below the limit again around {mid} (most likely {early} – {late}) and stays below for ~{d} days.")
            }
        } else if el {
            format!("Με καιρό σαν των ετών {y0}–{y1}, ξαναπέφτει κάτω από το όριο γύρω στις {mid} (πιθανότερα {early} – {late}) και μένει από κάτω ~{d} ημέρες.")
        } else {
            format!("With weather like {y0}–{y1}, it falls below the limit again around {mid} (most likely {early} – {late}) and stays below for ~{d} days.")
        };
    }
    let gain = shift(f.next.map(|n| n.mid), b.less_irr.next.map(|n| n.mid));
    let fewer = f.days_below_next as i64 - b.less_irr.days_below_next as i64;
    let by = f.next.map(|n| n.early);
    let mut parts = Vec::new();
    if let Some(g) = gain.filter(|&g| g > 0) {
        parts.push(if el { format!("η πτώση καθυστερεί ~{}", days(g, l)) } else { format!("the fall comes ~{} later", days(g, l)) });
    }
    if fewer > 0 {
        parts.push(if el { format!("οι ημέρες κάτω από το όριο μειώνονται κατά ~{fewer}") } else { format!("the days below the limit drop by ~{fewer}") });
    }
    let effect = parts.join(if el { " και " } else { " and " });
    let before = by.map_or_else(String::new, |d| if el { format!(" πριν από τις {}", fd(Some(d), l)) } else { format!(" before {}", fd(Some(d), l)) });
    let todo = if el {
        format!(
            "Σχέδιο άρδευσης για τα χωράφια γύρω από τη λίμνη{before}.{} Ενημέρωση Διεύθυνσης Υδάτων και ΤΟΕΒ τώρα, όχι όταν περάσει το όριο.",
            if effect.is_empty() { String::new() } else { format!(" Με 20% λιγότερο νερό άρδευσης {effect}.") }
        )
    } else {
        format!(
            "An irrigation plan for the fields around the lake{before}.{} Notify the Water Directorate and irrigation boards now, not once the limit is crossed.",
            if effect.is_empty() { String::new() } else { format!(" With 20% less irrigation water {effect}.") }
        )
    };
    [measured, coming, todo]
}

/// The back-test sentence: what the model would have said with the passes up to the cut-off.
pub fn backtest_line(b: &Brief, l: Lang) -> String {
    let Some(bt) = &b.bt else { return String::new() };
    let c = bt.cross;
    let lead = c.and_then(|c| c.mid).map_or_else(|| "null".into(), |m| m.since(bt.cutoff).to_string());
    let (mid, early, late) = (fd(c.and_then(|c| c.mid), l), fd(c.and_then(|c| c.early), l), fd(c.and_then(|c| c.late), l));
    let (cut, tgt, pred, obs) = (fd(Some(bt.cutoff), l), fd(Some(bt.target), l), n1(Some(bt.predicted), l), n1(Some(bt.observed), l));
    match l {
        Lang::El => format!("Έλεγχος στο παρελθόν: με λήψεις μόνο ως τις {cut}, το μοντέλο θα έλεγε ότι η λίμνη πέφτει κάτω από το όριο γύρω στις {mid} ({early} – {late}), δηλαδή {lead} ημέρες πριν. Για τις {tgt} προέβλεψε {pred} km²· το NISAR μέτρησε {obs} km²."),
        Lang::En => format!("Back-test: with passes only up to {cut}, the model would have said the lake falls below the limit around {mid} ({early} – {late}), {lead} days ahead. For {tgt} it predicted {pred} km²; NISAR measured {obs} km²."),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Answer {
    /// One paragraph per entry.
    pub text: Vec<String>,
    pub sources: String,
}

fn ensure_period(s: String) -> String {
    match s.strip_suffix('.') {
        Some(_) => s,
        None => s + ".",
    }
}

pub fn answer(q: Question, b: &Brief, l: Lang) -> Answer {
    let f = &b.base;
    let el = l == Lang::El;
    let l3 = lines(b, l);
    let src = if el { "Πηγές" } else { "Sources" };
    let (y0, y1) = b.years;
    let s_nisar = format!("NISAR L2 GCOV · {} {}", b.passes, if el { "μετρήσεις" } else { "measurements" });
    let s_wx = match (b.generator, el) {
        (Some((g0, g1)), true) => format!("Open-Meteo: καιρός ως {} · ERA5 {g0}–{g1} για τη γεννήτρια καιρού", fd(f.weather_until, l)),
        (Some((g0, g1)), false) => format!("Open-Meteo: weather up to {} · ERA5 {g0}–{g1} for the weather generator", fd(f.weather_until, l)),
        (None, true) => format!("Open-Meteo: καιρός ως {} και {y0}–{y1}", fd(f.weather_until, l)),
        (None, false) => format!("Open-Meteo: weather up to {} and {y0}–{y1}", fd(f.weather_until, l)),
    };
    let s_model = if el { "μοντέλο ισοζυγίου νερού (tools/fit_forecast.py)" } else { "water-balance model (tools/fit_forecast.py)" }.to_string();
    let out = |text: Vec<String>, sources: &[&String]| Answer {
        text,
        sources: format!("{src}: {}", sources.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" · ")),
    };
    match q {
        Question::Now => {
            let gap = b.today.since(b.last.date);
            let crossed = f.cross.and_then(|c| c.mid).is_some_and(|m| m <= b.today);
            let coming = l3[1].split(". ").take(if crossed { 2 } else { 1 }).collect::<Vec<_>>().join(". ");
            out(
                vec![
                    l3[0].clone(),
                    if el {
                        format!("Από τότε ({gap} ημέρες) δεν υπάρχει νέα λήψη, οπότε το σήμερα είναι εκτίμηση του μοντέλου με τον πραγματικό καιρό ως τις {}.", fd(f.weather_until, l))
                    } else {
                        format!("There has been no pass since ({gap} days), so today is the model's estimate with the real weather up to {}.", fd(f.weather_until, l))
                    },
                    ensure_period(coming),
                    if el { "Η επόμενη λήψη NISAR θα το επιβεβαιώσει ή θα το διορθώσει." } else { "The next NISAR pass will confirm or correct it." }.into(),
                ],
                &[&s_nisar, &s_wx, &s_model],
            )
        }
        Question::When => {
            let Some(n) = f.next else {
                return out(
                    vec![if el { "Σε κανένα σενάριο καιρού δεν πέφτει ξανά κάτω από το όριο μέσα στον επόμενο χρόνο." } else { "In no weather scenario does it fall below the limit again within a year." }.into()],
                    &[&s_model],
                );
            };
            let miss = js_round((1.0 - n.share) * 100.0) as i64;
            let (mid, early, late, lim) = (fd(Some(n.mid), l), fd(Some(n.early), l), fd(Some(n.late), l), n1(Some(b.limit), l));
            let (members, d, yr) = (f.members, f.days_below_next, n.mid.year());
            let mut text = vec![
                match (b.generator, el) {
                    (Some((g0, g1)), true) => format!(
                        "Έτρεξα το μοντέλο σε {} προσομοιωμένες χρονιές καιρού, τρεις φορές την καθεμία ({members} σενάρια). Τον καιρό τον παράγει μια αλυσίδα Markov: μία για το αν κάθε μέρα βρέχει, και μία πιο αργή για το αν ο μήνας είναι ξερός, κανονικός ή βροχερός, προσαρμοσμένες στο ERA5 {g0}–{g1}.",
                        crate::forecast::SYNTHETIC_YEARS
                    ),
                    (Some((g0, g1)), false) => format!(
                        "I ran the model on {} simulated years of weather, three times each ({members} scenarios). The weather comes from Markov chains: one for whether each day is wet, and a slower one for whether a month is dry, normal or wet, fitted to ERA5 {g0}–{g1}.",
                        crate::forecast::SYNTHETIC_YEARS
                    ),
                    (None, true) => format!("Έτρεξα το μοντέλο με τον καιρό καθενός από τα έτη {y0}–{y1}, τρεις φορές το καθένα ({members} σενάρια)."),
                    (None, false) => format!("I ran the model with the weather of each year {y0}–{y1}, three times each ({members} scenarios)."),
                },
                if el {
                    format!("Μεσαίο σενάριο: η λίμνη πέφτει κάτω από το όριο ({lim} km²) γύρω στις {mid}. Στα μισά σενάρια αυτό γίνεται από {early} έως {late}.")
                } else {
                    format!("Middle scenario: the lake falls below the limit ({lim} km²) around {mid}. In half the scenarios this happens between {early} and {late}.")
                },
                if el { format!("Το καλοκαίρι του {yr} μένει κάτω από το όριο ~{d} ημέρες.") } else { format!("In summer {yr} it stays below the limit for ~{d} days.") },
            ];
            if miss >= 5 {
                text.push(if el { format!("Σε {miss}% των σεναρίων ο χειμώνας δεν την ανεβάζει καν πάνω από το όριο.") } else { format!("In {miss}% of the scenarios winter does not even lift it above the limit.") });
            }
            out(text, &[&s_nisar, &s_wx, &s_model])
        }
        Question::Rain | Question::Irr => {
            let alt = if q == Question::Rain { &b.less_rain } else { &b.less_irr };
            let d = shift(f.next.map(|n| n.mid), alt.next.map(|n| n.mid));
            let what = match (q == Question::Rain, el) {
                (true, true) => "Με 20% λιγότερη βροχή από σήμερα και μετά",
                (true, false) => "With 20% less rain from today on",
                (false, true) => "Με 20% λιγότερο νερό άρδευσης (15 Απρ – 30 Σεπ)",
                (false, false) => "With 20% less irrigation water (15 Apr – 30 Sep)",
            };
            let mv = match d {
                None => String::new(),
                Some(d) if d < 0 => format!("{} {}", days(-d, l), if el { "νωρίτερα" } else { "earlier" }),
                Some(d) if d > 0 => format!("{} {}", days(d, l), if el { "αργότερα" } else { "later" }),
                Some(_) => if el { "την ίδια περίπου μέρα" } else { "about the same day" }.into(),
            };
            let (am, fm) = (fd(alt.next.map(|n| n.mid), l), fd(f.next.map(|n| n.mid), l));
            let (ad, fdn) = (alt.days_below_next, f.days_below_next);
            out(
                vec![
                    if el {
                        format!("{what}, η λίμνη πέφτει κάτω από το όριο γύρω στις {am}, {mv} από το μεσαίο σενάριο ({fm}).")
                    } else {
                        format!("{what}, the lake falls below the limit around {am}, {mv} than the middle scenario ({fm}).")
                    },
                    if el { format!("Ημέρες κάτω από το όριο το επόμενο καλοκαίρι: {ad} αντί για {fdn}.") } else { format!("Days below the limit next summer: {ad} instead of {fdn}.") },
                    match (q == Question::Irr, el) {
                        (true, true) => "Υπόθεση: το καλοκαίρι το 70% των απωλειών είναι άρδευση και το 30% εξάτμιση. Δεν έχουμε μετρημένες απολήψεις, οπότε ο αριθμός δείχνει κατεύθυνση.",
                        (true, false) => "Assumption: in summer 70% of the loss is irrigation and 30% evaporation. We have no metered withdrawals, so the figure shows direction.",
                        (false, true) => "Η αλλαγή εφαρμόζεται σε κάθε σενάριο καιρού· τα ίδια σενάρια, απλώς με λιγότερη βροχή.",
                        (false, false) => "The change is applied to every weather scenario: the same scenarios, with less rain.",
                    }
                    .into(),
                ],
                &[&s_wx, &s_model],
            )
        }
        Question::Act => out(
            vec![
                if el { "Προτάσεις προς τις αρμόδιες υπηρεσίες, όχι αποφάσεις:" } else { "Suggestions for the competent authorities, not decisions:" }.into(),
                format!("1. {}", l3[2]),
                if el { "2. Νέα λήψη NISAR κάθε 12 ημέρες: η εφαρμογή ξαναϋπολογίζει την πρόβλεψη με κάθε μέτρηση." } else { "2. A new NISAR pass every 12 days: the app recomputes the forecast with each measurement." }.into(),
                if el { "3. Να αντικατασταθεί το ενδεικτικό όριο με το επίσημο του Σχεδίου Διαχείρισης (EL10)." } else { "3. Replace the indicative limit with the official one from the River Basin Management Plan (EL10)." }.into(),
            ],
            &[&s_nisar, &s_model],
        ),
        Question::Trust => {
            let (p, e) = (b.passes, n1(Some(b.rmse), l));
            out(
                vec![
                    if el {
                        format!("Το μοντέλο έχει 4 παραμέτρους, προσαρμοσμένες στις {p} μετρήσεις έκτασης του 2026. Τυπικό σφάλμα: ±{e} km².")
                    } else {
                        format!("The model has 4 parameters, fitted to the {p} area measurements of 2026. Typical error: ±{e} km².")
                    },
                    backtest_line(b, l),
                    if el {
                        "Αδυναμίες: δεν έχουμε ακόμα μέτρηση χειμώνα, οπότε το πόσο γρήγορα γεμίζει η λίμνη με τη βροχή είναι το πιο αβέβαιο κομμάτι (γι' αυτό το εύρος είναι φαρδύ). Το όριο είναι ενδεικτικό (25% της μέγιστης έκτασης)."
                    } else {
                        "Weak points: we have no winter measurement yet, so how fast rain refills the lake is the most uncertain part (hence the wide range). The limit is indicative (25% of the largest extent)."
                    }
                    .into(),
                ],
                &[&s_nisar, &s_wx, &s_model],
            )
        }
        Question::None => out(
            vec![if el { "Δεν μπορώ να απαντήσω σε αυτό με τα δεδομένα που έχω. Δοκίμασε μία από τις ερωτήσεις παραπάνω." } else { "I cannot answer that from the data I have. Try one of the questions above." }.into()],
            &[],
        ),
    }
}
