//! Calendar date-time as sent by the modem, and its conversion to Unix seconds.
//!
//! No time zone is applied here: the caller decides what the fields mean
//! (`+CTZEU` carries UTC; `+CCLK` carries local time, see `TimeLine::cclk_utc`).

use std::fmt;

/// A calendar date and time with one-second resolution, as written by the modem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// Why a date-time string was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateTimeError(String);

impl fmt::Display for DateTimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DateTimeError {}

impl DateTime {
    /// Parses `yy/MM/dd,hh:mm:ss` or `YYYY/MM/DD,hh:mm:ss` (no quotes, no zone).
    /// A two-digit year is taken as 20yy.
    ///
    /// # Errors
    /// Returns an error when the layout or any field range is invalid.
    pub fn parse(text: &str) -> Result<Self, DateTimeError> {
        let err = |why: &str| DateTimeError(format!("{why}: {text:?}"));
        let (date, time) = text.split_once(',').ok_or_else(|| err("missing ','"))?;
        let d: Vec<&str> = date.split('/').collect();
        let t: Vec<&str> = time.split(':').collect();
        if d.len() != 3 || t.len() != 3 {
            return Err(err("expected Y/M/D,h:m:s"));
        }
        let num = |s: &str, width: &[usize]| -> Result<u32, DateTimeError> {
            if !width.contains(&s.len()) || !s.bytes().all(|b| b.is_ascii_digit()) {
                return Err(err("bad number"));
            }
            s.parse::<u32>().map_err(|_| err("bad number"))
        };
        let year_raw = num(d[0], &[2, 4])?;
        let year = if d[0].len() == 2 {
            2000 + year_raw
        } else {
            year_raw
        };
        let field = |s: &str, max: u32| -> Result<u8, DateTimeError> {
            let v = num(s, &[2])?;
            if v > max {
                return Err(err("field out of range"));
            }
            u8::try_from(v).map_err(|_| err("field out of range"))
        };
        let v = Self {
            year: i32::try_from(year).map_err(|_| err("year out of range"))?,
            month: field(d[1], 12)?,
            day: field(d[2], 31)?,
            hour: field(t[0], 23)?,
            minute: field(t[1], 59)?,
            // 60 is a leap second; NITZ can carry one in principle.
            second: field(t[2], 60)?,
        };
        if v.month == 0 || v.day == 0 || v.day > days_in_month(v.year, v.month) {
            return Err(err("no such date"));
        }
        Ok(v)
    }

    /// Seconds since 1970-01-01T00:00:00 when the fields are read as UTC.
    #[must_use]
    pub fn unix_seconds(&self) -> i64 {
        let days = days_from_civil(self.year, self.month, self.day);
        days * 86_400
            + i64::from(self.hour) * 3_600
            + i64::from(self.minute) * 60
            + i64::from(self.second)
    }
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (H. Hinnant's algorithm).
fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unix(s: &str) -> i64 {
        DateTime::parse(s).unwrap().unix_seconds()
    }

    #[test]
    fn known_instants() {
        // Reference values from Python's calendar.timegm.
        assert_eq!(unix("1970/01/01,00:00:00"), 0);
        assert_eq!(unix("2000/03/01,00:00:00"), 951_868_800);
        assert_eq!(unix("2024/02/29,00:00:00"), 1_709_164_800);
        assert_eq!(unix("2026/11/17,08:51:24"), 1_794_905_484);
        assert_eq!(unix("2099/12/31,23:59:59"), 4_102_444_799);
    }

    #[test]
    fn two_digit_year_is_20yy() {
        assert_eq!(unix("26/11/17,08:51:24"), 1_794_905_484);
        assert_eq!(unix("01/01/01,01:01:01"), 978_310_861);
    }

    #[test]
    fn rejects_malformed() {
        for bad in [
            "",
            "26/11/17",
            "26/11/17 08:51:24",
            "026/11/17,08:51:24",
            "26/13/17,08:51:24",
            "26/02/30,08:51:24",
            "25/02/29,00:00:00",
            "26/11/17,24:00:00",
            "26/11/17,08:5:24",
            "26/11/17,08:51:2x",
            "+6/11/17,08:51:24",
        ] {
            assert!(DateTime::parse(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn accepts_leap_day_and_leap_second() {
        assert!(DateTime::parse("24/02/29,00:00:00").is_ok());
        assert!(DateTime::parse("2016/12/31,23:59:60").is_ok());
    }
}
