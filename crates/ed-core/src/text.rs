//! Bilingual text and the exact number/date formatting the generated sentences are built from.
//!
//! The legacy app formatted with `Number.prototype.toFixed`, `Math.round`, template-literal `${n}` and
//! `Intl.DateTimeFormat('el-GR' | 'en-GB')`. Rust's `format!` differs on exact ties (it rounds half to even),
//! so this module re-implements the JS rules; `tests/fixtures/{numbers,dates}.json` pin them.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::{Deserialize, Deserializer, Serialize};

use crate::Date;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    #[default]
    El,
    En,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::El, Lang::En];
    pub const fn code(self) -> &'static str {
        match self {
            Lang::El => "el",
            Lang::En => "en",
        }
    }
    pub const fn other(self) -> Lang {
        match self {
            Lang::El => Lang::En,
            Lang::En => Lang::El,
        }
    }
}

/// A piece of text in both languages. Deserializes from `{"el": …, "en": …}` or from a plain string
/// (used for both), like the legacy `L()` helper.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Text {
    pub el: String,
    pub en: String,
}

impl Text {
    pub fn new(el: impl Into<String>, en: impl Into<String>) -> Self {
        Self { el: el.into(), en: en.into() }
    }
    pub fn same(s: impl Into<String>) -> Self {
        let s = s.into();
        Self { el: s.clone(), en: s }
    }
    /// Builds both languages from one function, as `both()` in the legacy rules.
    pub fn both(mut f: impl FnMut(Lang) -> String) -> Self {
        Self { el: f(Lang::El), en: f(Lang::En) }
    }
    pub fn get(&self, l: Lang) -> &str {
        match l {
            Lang::El if !self.el.is_empty() => &self.el,
            Lang::En if !self.en.is_empty() => &self.en,
            _ if !self.en.is_empty() => &self.en,
            _ => &self.el,
        }
    }
}

impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Plain(String),
            Both {
                #[serde(default)]
                el: Option<String>,
                #[serde(default)]
                en: Option<String>,
            },
        }
        Ok(match Raw::deserialize(d)? {
            Raw::Plain(s) => Text::same(s),
            Raw::Both { el, en } => Text { el: el.unwrap_or_default(), en: en.unwrap_or_default() },
        })
    }
}

// ───────────────────────────────────────────────────────── numbers

/// `Number.prototype.toFixed(d)`: the exact binary value rounded to `d` decimals, exact ties away from zero.
/// A negative number keeps its sign even when it rounds to zero (`(-0.04).toFixed(1) == "-0.0"`).
pub fn to_fixed(v: f64, d: usize) -> String {
    if !v.is_finite() {
        return js_number(v);
    }
    let neg = v < 0.0;
    // Printing 40 extra digits is exact enough to tell a tie from a near-tie for every magnitude the app
    // formats: a double that is not a tie lies at least 2^e / 5^d away from it (e = exponent of its ulp).
    let s = format!("{:.*}", d + 40, v.abs());
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    let mut digits: Vec<u8> = int.bytes().chain(frac.bytes().take(d)).collect();
    if frac.as_bytes().get(d).is_some_and(|&c| c >= b'5') {
        let mut i = digits.len();
        loop {
            if i == 0 {
                digits.insert(0, b'1');
                break;
            }
            i -= 1;
            if digits[i] == b'9' {
                digits[i] = b'0';
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    let int_len = digits.len() - d;
    let mut out = String::with_capacity(digits.len() + 2);
    if neg {
        out.push('-');
    }
    out.push_str(std::str::from_utf8(&digits[..int_len]).expect("ascii"));
    if d > 0 {
        out.push('.');
        out.push_str(std::str::from_utf8(&digits[int_len..]).expect("ascii"));
    }
    out
}

/// `Math.round`: nearest integer, exact halves towards +∞ (`Math.round(-2.5) == -2`).
pub fn js_round(v: f64) -> f64 {
    let f = v.floor();
    if v - f >= 0.5 { f + 1.0 } else { f }
}

/// `String(n)` / `${n}` for the numbers the app prints (shortest round-trip form, `-0` → `0`).
pub fn js_number(v: f64) -> String {
    if v.is_nan() {
        "NaN".into()
    } else if v.is_infinite() {
        if v > 0.0 { "Infinity".into() } else { "-Infinity".into() }
    } else if v == 0.0 {
        "0".into()
    } else {
        format!("{v}")
    }
}

/// The signed number of the rules (`num()` in insights.js, `signed()` in status.js): `+12`, `−3.5`, `0`.
/// The ASCII minus becomes U+2212 and a value that rounds to zero loses its sign.
pub fn signed(v: f64, d: usize) -> String {
    let s = to_fixed(v, d);
    if s.parse::<f64>().is_ok_and(|x| x == 0.0) {
        return s.replacen('-', "", 1);
    }
    format!("{}{s}", if v > 0.0 { "+" } else { "" }).replacen('-', "\u{2212}", 1)
}

/// `fmtNum()` in app.js: like [`signed`] but keeps `−0`.
pub fn fmt_num(v: f64, d: usize) -> String {
    format!("{}{}", if v > 0.0 { "+" } else { "" }, to_fixed(v, d)).replacen('-', "\u{2212}", 1)
}

/// One decimal with the language's decimal separator (`n1()` in ask.js); `–` for no value.
pub fn n1(v: Option<f64>, l: Lang) -> String {
    match v {
        None => "–".into(),
        Some(v) => {
            let s = to_fixed(v, 1);
            if l == Lang::El { s.replacen('.', ",", 1) } else { s }
        }
    }
}

/// Rounded integer (`n0()` in ask.js); `–` for no value.
pub fn n0(v: Option<f64>) -> String {
    v.map_or_else(|| "–".into(), |v| js_number(js_round(v)))
}

// ───────────────────────────────────────────────────────── dates

/// The `Intl.DateTimeFormat` option sets the app uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateStyle {
    /// `{day, month: short, year}` — "18 Ιουν 2026" / "18 Jun 2026" (the default).
    DayMonYear,
    /// `{day, month: short}` — "18 Ιουν".
    DayMon,
    /// `{month: long}` — "Ιουνίου" (genitive, as ICU gives it) / "June".
    MonthLong,
    /// `{weekday: long, day, month: long}` — "Πέμπτη 18 Ιουνίου" / "Thursday 18 June".
    WeekdayDayMonth,
    /// `{day, month: long, year}` — "18 Ιουνίου 2026".
    DayMonthYear,
    /// `{weekday: short}` — "Πέμ" / "Thu".
    WeekdayShort,
    /// `{day, month: short, year: 2-digit}` — "18 Ιουν 26".
    DayMonYY,
    /// `{weekday: long, day, month: long, year}` — "Πέμπτη 18 Ιουνίου 2026" / "Thursday, 18 June 2026".
    WeekdayDayMonthYear,
}

const MON_EL: [&str; 12] = ["Ιαν", "Φεβ", "Μαρ", "Απρ", "Μαΐ", "Ιουν", "Ιουλ", "Αυγ", "Σεπ", "Οκτ", "Νοε", "Δεκ"];
const MON_EN: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sept", "Oct", "Nov", "Dec"];
const MONTH_EL: [&str; 12] = [
    "Ιανουαρίου", "Φεβρουαρίου", "Μαρτίου", "Απριλίου", "Μαΐου", "Ιουνίου", "Ιουλίου", "Αυγούστου", "Σεπτεμβρίου",
    "Οκτωβρίου", "Νοεμβρίου", "Δεκεμβρίου",
];
const MONTH_EN: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November",
    "December",
];
const WD_EL: [&str; 7] = ["Δευτέρα", "Τρίτη", "Τετάρτη", "Πέμπτη", "Παρασκευή", "Σάββατο", "Κυριακή"];
const WD_EN: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const WDS_EL: [&str; 7] = ["Δευ", "Τρί", "Τετ", "Πέμ", "Παρ", "Σάβ", "Κυρ"];
const WDS_EN: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

pub fn fmt_date(d: Date, l: Lang, style: DateStyle) -> String {
    let (y, m, day, wd) = (d.year(), d.month() as usize - 1, d.day(), d.weekday() as usize);
    let el = l == Lang::El;
    let mon = if el { MON_EL[m] } else { MON_EN[m] };
    let month = if el { MONTH_EL[m] } else { MONTH_EN[m] };
    let wday = if el { WD_EL[wd] } else { WD_EN[wd] };
    match style {
        DateStyle::DayMonYear => format!("{day} {mon} {y}"),
        DateStyle::DayMon => format!("{day} {mon}"),
        DateStyle::MonthLong => month.to_string(),
        DateStyle::WeekdayDayMonth => format!("{wday} {day} {month}"),
        DateStyle::DayMonthYear => format!("{day} {month} {y}"),
        DateStyle::WeekdayShort => (if el { WDS_EL[wd] } else { WDS_EN[wd] }).to_string(),
        DateStyle::DayMonYY => format!("{day} {mon} {:02}", y.rem_euclid(100)),
        DateStyle::WeekdayDayMonthYear => {
            if el {
                format!("{wday} {day} {month} {y}")
            } else {
                format!("{wday}, {day} {month} {y}")
            }
        }
    }
}

/// Short month name (1–12).
pub fn month_short(m: u32, l: Lang) -> &'static str {
    let i = (m.clamp(1, 12) - 1) as usize;
    if l == Lang::El { MON_EL[i] } else { MON_EN[i] }
}

/// A set of months (bit m−1) as ranges, wrapping over the new year: "Οκτ–Μαρ", "Apr–Sept".
pub fn month_span(mask: u16, l: Lang) -> String {
    let on = |m: i32| mask & (1 << m.rem_euclid(12)) != 0;
    if mask & 0xfff == 0xfff {
        return if l == Lang::El { "όλο το έτος".into() } else { "all year".into() };
    }
    let mut parts = Vec::new();
    for m in 0..12 {
        if on(m) && !on(m - 1) {
            let mut e = m;
            while on(e + 1) {
                e += 1;
            }
            let (a, b) = (month_short(m as u32 + 1, l), month_short((e.rem_euclid(12)) as u32 + 1, l));
            parts.push(if a == b { a.to_string() } else { format!("{a}–{b}") });
        }
    }
    parts.join(", ")
}

/// Default-style date, or `–` when there is none (`fd()` in ask.js).
pub fn fd(d: Option<Date>, l: Lang) -> String {
    d.map_or_else(|| "–".into(), |d| fmt_date(d, l, DateStyle::DayMonYear))
}

// ───────────────────────────────────────────────────────── UI strings

static STRINGS: OnceLock<HashMap<String, Text>> = OnceLock::new();

fn strings() -> &'static HashMap<String, Text> {
    STRINGS.get_or_init(|| {
        serde_json::from_str(include_str!("../../../assets/strings.json")).expect("assets/strings.json is valid")
    })
}

/// UI string by key (`t()` in the legacy app): the key itself when it is missing.
pub fn t(key: &str, l: Lang) -> &str {
    strings().get(key).map_or(key, |s| s.get(l))
}

/// UI string with `{name}` placeholders filled (`tf()` in status.js).
pub fn tf(key: &str, l: Lang, vars: &[(&str, &str)]) -> String {
    fill(t(key, l), vars)
}

pub fn fill(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('}') {
            Some(j) if after[..j].chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && j > 0 => {
                let name = &after[..j];
                match vars.iter().find(|(k, _)| *k == name) {
                    Some((_, v)) => out.push_str(v),
                    None => out.push_str("undefined"), // what `v[n]` gave in the legacy template
                }
                rest = &after[j + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_rules() {
        assert_eq!(to_fixed(2.5, 0), "3");
        assert_eq!(to_fixed(-2.5, 0), "-3");
        assert_eq!(to_fixed(0.125, 2), "0.13");
        assert_eq!(to_fixed(1.005, 2), "1.00");
        assert_eq!(to_fixed(-0.04, 1), "-0.0");
        assert_eq!(to_fixed(99.95, 1), "100.0");
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(signed(-0.04, 1), "0.0");
        assert_eq!(signed(-12.345, 1), "\u{2212}12.3");
        assert_eq!(signed(3.0, 0), "+3");
        assert_eq!(fill("a {n} b {x}", &[("n", "1")]), "a 1 b undefined");
        assert_eq!(month_span(crate::model::LEGACY_WET_MONTHS, Lang::En), "Nov–Mar");
        assert_eq!(month_span(0b0000_0011_1111_0000 >> 1, Lang::En), "Apr–Sept");
    }
}
