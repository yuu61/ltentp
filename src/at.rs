//! Parsing of the time-related AT lines (`+CTZEU`, `+CTZE`, `+CTZV`, `+CCLK`).
//!
//! Formats follow 3GPP TS 27.007 and the Fibocom L860 AT manual §5.1 (see
//! docs/at-commands.md). Parsing is lenient where real modems are known to
//! differ (quoted or bare zone, 2- or 4-digit year, missing time).

use std::fmt;

use crate::civil::DateTime;

/// A time-related line from the modem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeLine {
    /// `+CTZEU: <tz>,<dst>[,<utime>]` — zone and universal time (`AT+CTZR=3`).
    Ctzeu {
        tz_quarters: i32,
        dst: u8,
        utc: Option<DateTime>,
    },
    /// `+CTZE: <tz>,<dst>[,<time>]` — zone and local time (`AT+CTZR=2`).
    Ctze {
        tz_quarters: i32,
        dst: u8,
        local: Option<DateTime>,
    },
    /// `+CTZV: <tz>` — zone only (`AT+CTZR=1`).
    Ctzv { tz_quarters: i32 },
    /// `+CCLK: "<time>[±zz]"` — the modem clock. `time` is the field as written,
    /// with no zone applied: on the L860-GL it was observed to already be UTC.
    Cclk {
        time: DateTime,
        tz_quarters: Option<i32>,
    },
}

impl TimeLine {
    /// Short tag used in the CSV `kind` column.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Ctzeu { .. } => "CTZEU",
            Self::Ctze { .. } => "CTZE",
            Self::Ctzv { .. } => "CTZV",
            Self::Cclk { .. } => "CCLK",
        }
    }
}

/// A line that has a known time prefix but could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

/// Classifies one line (without CR/LF).
///
/// Returns `None` when the line is not a time line (echo, `OK`, other URCs),
/// `Some(Err)` when it has a time prefix but a malformed body.
#[must_use]
pub fn parse_time_line(line: &str) -> Option<Result<TimeLine, ParseError>> {
    let line = line.trim();
    // Longest prefix first: "+CTZEU:" also starts with "+CTZE".
    if let Some(body) = line.strip_prefix("+CTZEU:") {
        return Some(
            parse_ctze(body).map(|(tz_quarters, dst, utc)| TimeLine::Ctzeu {
                tz_quarters,
                dst,
                utc,
            }),
        );
    }
    if let Some(body) = line.strip_prefix("+CTZE:") {
        return Some(
            parse_ctze(body).map(|(tz_quarters, dst, local)| TimeLine::Ctze {
                tz_quarters,
                dst,
                local,
            }),
        );
    }
    if let Some(body) = line.strip_prefix("+CTZV:") {
        return Some(parse_tz(body.trim()).map(|tz_quarters| TimeLine::Ctzv { tz_quarters }));
    }
    if let Some(body) = line.strip_prefix("+CCLK:") {
        return Some(parse_cclk(body));
    }
    None
}

fn parse_ctze(body: &str) -> Result<(i32, u8, Option<DateTime>), ParseError> {
    let fields = split_fields(body);
    if fields.len() < 2 || fields.len() > 3 {
        return Err(ParseError(format!(
            "expected 2 or 3 fields, got {}: {body:?}",
            fields.len()
        )));
    }
    let tz = parse_tz(fields[0])?;
    let dst = match unquote(fields[1]) {
        "0" => 0,
        "1" => 1,
        "2" => 2,
        other => return Err(ParseError(format!("bad dst {other:?}"))),
    };
    let time = match fields.get(2).map(|f| unquote(f)) {
        None | Some("") => None,
        Some(t) => Some(DateTime::parse(t).map_err(|e| ParseError(e.to_string()))?),
    };
    Ok((tz, dst, time))
}

fn parse_cclk(body: &str) -> Result<TimeLine, ParseError> {
    let text = unquote(body.trim());
    // A zone suffix is "+zz"/"-zz" after the seconds; dates themselves use '/' and ':' only.
    let (time_text, tz_quarters) = match text.rfind(['+', '-']) {
        Some(i) => (&text[..i], Some(parse_tz(&text[i..])?)),
        None => (text, None),
    };
    let time = DateTime::parse(time_text).map_err(|e| ParseError(e.to_string()))?;
    Ok(TimeLine::Cclk { time, tz_quarters })
}

/// Zone in quarter-hours: `+36`, `-08`, `36`, optionally quoted. Range per TS 27.007: -48..=+56.
fn parse_tz(field: &str) -> Result<i32, ParseError> {
    let s = unquote(field.trim());
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || digits.len() > 2 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ParseError(format!("bad time zone {s:?}")));
    }
    let v: i32 = s
        .parse()
        .map_err(|_| ParseError(format!("bad time zone {s:?}")))?;
    if !(-48..=56).contains(&v) {
        return Err(ParseError(format!("time zone out of range {v}")));
    }
    Ok(v)
}

/// Splits on commas that are outside double quotes, trimming each field.
/// `"2026/11/17,08:51:24"` contains a comma and must stay one field.
fn split_fields(body: &str) -> Vec<&str> {
    let mut fields = Vec::new();
    let mut in_quotes = false;
    let mut start = 0;
    for (i, c) in body.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                fields.push(body[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    fields.push(body[start..].trim());
    fields
}

fn unquote(s: &str) -> &str {
    s.strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> DateTime {
        DateTime::parse(s).unwrap()
    }

    fn ok(line: &str) -> TimeLine {
        parse_time_line(line).unwrap().unwrap()
    }

    #[test]
    fn split_keeps_quoted_comma() {
        assert_eq!(
            split_fields(r#" +36,0,"2026/11/17,08:51:24""#),
            vec!["+36", "0", r#""2026/11/17,08:51:24""#]
        );
        assert_eq!(split_fields(r#""+36",0,"#), vec![r#""+36""#, "0", ""]);
    }

    #[test]
    fn ctzeu_manual_format() {
        assert_eq!(
            ok(r#"+CTZEU: +36,0,"2026/11/17,08:51:24""#),
            TimeLine::Ctzeu {
                tz_quarters: 36,
                dst: 0,
                utc: Some(dt("2026/11/17,08:51:24")),
            }
        );
    }

    #[test]
    fn ctzeu_variants() {
        let expected = TimeLine::Ctzeu {
            tz_quarters: 36,
            dst: 0,
            utc: Some(dt("2026/11/17,08:51:24")),
        };
        assert_eq!(ok(r#"+CTZEU: "+36",0,"26/11/17,08:51:24""#), expected);
        assert_eq!(ok(r#"+CTZEU:36,0,"2026/11/17,08:51:24""#), expected);
        assert_eq!(ok("  +CTZEU: +36,0,\"2026/11/17,08:51:24\"  "), expected);
    }

    #[test]
    fn ctzeu_without_time() {
        let expected = TimeLine::Ctzeu {
            tz_quarters: 36,
            dst: 0,
            utc: None,
        };
        assert_eq!(ok("+CTZEU: +36,0"), expected);
        assert_eq!(ok(r#"+CTZEU: "+36",0,"#), expected);
        assert_eq!(ok(r#"+CTZEU: "+36",0,"""#), expected);
    }

    #[test]
    fn ctze_and_ctzv() {
        assert_eq!(
            ok(r#"+CTZE: "-08",1,"01/01/01,01:01:01""#),
            TimeLine::Ctze {
                tz_quarters: -8,
                dst: 1,
                local: Some(dt("01/01/01,01:01:01")),
            }
        );
        assert_eq!(ok("+CTZV: +36"), TimeLine::Ctzv { tz_quarters: 36 });
    }

    #[test]
    fn cclk_observed_on_l860() {
        // Real response captured on 2026-11-17 (Windows, MBIM AT tunnel).
        assert_eq!(
            ok(r#"+CCLK: "26/11/17,08:51:24+36""#),
            TimeLine::Cclk {
                time: dt("26/11/17,08:51:24"),
                tz_quarters: Some(36),
            }
        );
    }

    #[test]
    fn cclk_negative_and_missing_zone() {
        assert_eq!(
            ok(r#"+CCLK: "01/01/01,01:01:01-08""#),
            TimeLine::Cclk {
                time: dt("01/01/01,01:01:01"),
                tz_quarters: Some(-8),
            }
        );
        assert_eq!(
            ok(r#"+CCLK: "01/01/01,01:01:01""#),
            TimeLine::Cclk {
                time: dt("01/01/01,01:01:01"),
                tz_quarters: None,
            }
        );
    }

    #[test]
    fn other_lines_are_not_time_lines() {
        for line in [
            "OK",
            "ERROR",
            "AT+CTZR=3",
            "+CTZR: 3",
            "+CREG: 1",
            "",
            "garbage",
        ] {
            assert_eq!(parse_time_line(line), None, "{line:?}");
        }
    }

    #[test]
    fn malformed_time_lines_are_errors() {
        for line in [
            "+CTZEU:",
            "+CTZEU: +36",
            "+CTZEU: +36,3,\"2026/11/17,08:51:24\"",
            "+CTZEU: +99,0",
            "+CTZEU: +36,0,\"2026/11/17\"",
            "+CTZEU: +36,0,\"2026/11/17,08:51:24\",x",
            "+CTZV: x",
            "+CCLK: \"26/11/17,08:51:24+3x\"",
            "+CCLK: garbage",
        ] {
            assert!(
                matches!(parse_time_line(line), Some(Err(_))),
                "not rejected: {line:?}"
            );
        }
    }

    #[test]
    fn kind_tags() {
        assert_eq!(ok("+CTZEU: +36,0").kind(), "CTZEU");
        assert_eq!(ok("+CTZE: +36,0").kind(), "CTZE");
        assert_eq!(ok("+CTZV: +36").kind(), "CTZV");
        assert_eq!(ok(r#"+CCLK: "26/11/17,08:51:24+36""#).kind(), "CCLK");
    }
}
