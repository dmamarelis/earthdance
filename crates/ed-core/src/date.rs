//! Civil (calendar) dates with no time zone: what every series in the app is indexed by.
//!
//! Stored as days since 1970-01-01 so that date arithmetic is integer arithmetic. The legacy JS mixed
//! `Date.parse(iso)` (UTC) with `new Date(iso + 'T00:00:00')` (local); for whole days both reduce to this.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Date(i32);

/// Days from 1970-01-01 to y-m-d in the proleptic Gregorian calendar (H. Hinnant, "chrono-compatible
/// low-level date algorithms"). Works by shifting the year to start in March, so the leap day is last.
const fn days_from_civil(y: i32, m: u32, d: u32) -> i32 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32; // [0, 399]
    let mp = (m + 9) % 12; // March = 0
    let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe as i32 - 719_468
}

const fn civil_from_days(z: i32) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i32 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub const fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

pub const fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
    }
}

impl Date {
    /// Panics on an invalid date: use [`Date::try_ymd`] for untrusted input.
    pub const fn ymd(y: i32, m: u32, d: u32) -> Self {
        assert!(m >= 1 && m <= 12 && d >= 1 && d <= days_in_month(y, m), "invalid date");
        Self(days_from_civil(y, m, d))
    }

    pub const fn try_ymd(y: i32, m: u32, d: u32) -> Option<Self> {
        if m >= 1 && m <= 12 && d >= 1 && d <= days_in_month(y, m) { Some(Self(days_from_civil(y, m, d))) } else { None }
    }

    pub const fn from_days(n: i32) -> Self {
        Self(n)
    }

    /// Days since 1970-01-01.
    pub const fn days(self) -> i32 {
        self.0
    }

    pub const fn year(self) -> i32 {
        civil_from_days(self.0).0
    }
    pub const fn month(self) -> u32 {
        civil_from_days(self.0).1
    }
    pub const fn day(self) -> u32 {
        civil_from_days(self.0).2
    }

    /// (month, day): compares like the legacy `iso.slice(5)` string ("04-15" <= md <= "09-30").
    pub const fn md(self) -> (u32, u32) {
        let (_, m, d) = civil_from_days(self.0);
        (m, d)
    }

    pub const fn add_days(self, n: i32) -> Self {
        Self(self.0 + n)
    }

    /// Signed number of days from `other` to `self`.
    pub const fn since(self, other: Self) -> i32 {
        self.0 - other.0
    }

    /// 0 = Monday … 6 = Sunday. 1970-01-01 was a Thursday.
    pub const fn weekday(self) -> u32 {
        (self.0 + 3).rem_euclid(7) as u32
    }

    /// Index in a 366-slot climate year (Feb 29 has its own slot), as `doy()` in js/forecast.js.
    pub const fn climate_slot(self) -> usize {
        const START: [u32; 12] = [0, 31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335];
        let (_, m, d) = civil_from_days(self.0);
        (START[(m - 1) as usize] + d - 1) as usize
    }

    pub fn parse(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
            return None;
        }
        let y: i32 = s.get(0..4)?.parse().ok()?;
        let m: u32 = s.get(5..7)?.parse().ok()?;
        let d: u32 = s.get(8..10)?.parse().ok()?;
        Self::try_ymd(y, m, d)
    }

    pub fn iso(self) -> String {
        self.to_string()
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (y, m, d) = civil_from_days(self.0);
        write!(f, "{y:04}-{m:02}-{d:02}")
    }
}

impl fmt::Debug for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl std::str::FromStr for Date {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| format!("not a YYYY-MM-DD date: {s:?}"))
    }
}

impl Serialize for Date {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Date {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        Self::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("not a YYYY-MM-DD date: {s:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_day_of_four_centuries() {
        let start = Date::ymd(1900, 1, 1).days();
        let end = Date::ymd(2300, 12, 31).days();
        for n in start..=end {
            let d = Date::from_days(n);
            assert_eq!(Date::ymd(d.year(), d.month(), d.day()), d);
            assert_eq!(Date::parse(&d.iso()), Some(d));
        }
    }

    #[test]
    fn epoch_weekday_and_slots() {
        assert_eq!(Date::ymd(1970, 1, 1).days(), 0);
        assert_eq!(Date::ymd(2026, 11, 14).weekday(), 5); // Saturday
        assert_eq!(Date::ymd(2026, 3, 1).climate_slot(), 60);
        assert_eq!(Date::ymd(2024, 2, 29).climate_slot(), 59);
        assert_eq!(Date::ymd(2026, 12, 31).climate_slot(), 365);
        assert_eq!(Date::parse("2026-02-30"), None);
    }
}
