//! Timestamp search input: periods at the supplied precision, resolved in the local timezone.

use chrono::{DateTime, Duration, Local, LocalResult, Months, NaiveDate, NaiveDateTime, TimeZone};

use crate::{Error, Result, schema};

#[derive(Debug, Eq, PartialEq)]
pub(super) struct Bounds {
    pub start: i64,
    pub end: i64,
}

pub(super) fn parse(raw: &str, field: &str) -> Result<Bounds> {
    let invalid = || {
        Error::SearchQuery(format!(
            "Invalid value {raw:?} for {field}. Use a timestamp such as 2026, 2026-11, or \
              2026-02-12T05, optionally followed by Z or an offset such as -05:00, \
              or use ms: followed by integer epoch milliseconds"
        ))
    };
    let epoch = |value: &str| {
        value
            .parse::<i64>()
            .ok()
            .map(|milliseconds| Bounds { start: milliseconds, end: milliseconds })
    };
    if let Some(value) = raw.strip_prefix("ms:") {
        return epoch(value).ok_or_else(invalid);
    }
    // Four unsigned digits denote a calendar year; other integers keep their epoch interpretation.
    if !(raw.len() == 4 && raw.bytes().all(|b| b.is_ascii_digit()))
        && let Some(bounds) = epoch(raw)
    {
        return Ok(bounds);
    }
    let (civil, zone) = split_zone(raw);
    let (start, precision) = civil_time(civil).ok_or_else(invalid)?;
    let end =
        start.checked_add_signed(precision - Duration::milliseconds(1)).ok_or_else(invalid)?;
    Ok(Bounds { start: resolve(start, zone, raw, field)?, end: resolve(end, zone, raw, field)? })
}

fn split_zone(raw: &str) -> (&str, &str) {
    if let Some(civil) = raw.strip_suffix('Z') {
        return (civil, "Z");
    }
    if let Some(at) = raw.len().checked_sub(6)
        && let Some(zone) = raw.get(at..)
        && zone.starts_with(['+', '-'])
        && zone.get(3..4) == Some(":")
    {
        return raw.split_at(at);
    }
    (raw, "")
}

fn resolve(datetime: NaiveDateTime, zone: &str, raw: &str, field: &str) -> Result<i64> {
    if !zone.is_empty() {
        let complete = format!("{}{zone}", datetime.format("%Y-%m-%dT%H:%M:%S%.3f"));
        return DateTime::parse_from_rfc3339(&complete)
            .map(|time| time.timestamp_millis())
            .map_err(|_| {
                Error::SearchQuery(format!("Invalid timezone offset in {raw:?} for {field}"))
            });
    }
    match Local.from_local_datetime(&datetime) {
        LocalResult::Single(time) => Ok(time.timestamp_millis()),
        LocalResult::Ambiguous(..) => Err(Error::SearchQuery(format!(
            "The local time {raw:?} for {field} occurs twice when clocks change. \
              Add Z or an explicit offset such as -04:00"
        ))),
        LocalResult::None => Err(Error::SearchQuery(format!(
            "The local time {raw:?} for {field} doesn't exist when clocks change. \
              Choose another time or add Z or an explicit offset such as -05:00"
        ))),
    }
}

fn civil_time(raw: &str) -> Option<(NaiveDateTime, Duration)> {
    let (date, clock) = raw.split_once('T').map_or((raw, None), |(d, t)| (d, Some(t)));
    if clock.is_none() && matches!(date.len(), 4 | 7) {
        let (suffix, months) = if date.len() == 4 { ("-01-01", 12) } else { ("-01", 1) };
        let complete = format!("{date}{suffix}");
        if !schema::is_date(&complete) {
            return None;
        }
        let start = NaiveDate::parse_from_str(&complete, "%Y-%m-%d").ok()?.and_hms_opt(0, 0, 0)?;
        let next = start.checked_add_months(Months::new(months))?;
        return Some((start, next - start));
    }
    if !schema::is_date(date) {
        return None;
    }
    let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let Some(clock) = clock else { return Some((date.and_hms_opt(0, 0, 0)?, Duration::days(1))) };
    let (clock, fraction) = clock.split_once('.').map_or((clock, None), |(c, f)| (c, Some(f)));
    let milliseconds = if let Some(fraction) = fraction {
        if clock.len() != 8
            || !(1..=3).contains(&fraction.len())
            || !fraction.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        format!("{fraction:0<3}").parse().ok()?
    } else {
        0
    };
    let clock = if clock.len() == 2 && clock.bytes().all(|b| b.is_ascii_digit()) {
        format!("{clock}:00:00")
    } else if schema::is_time(clock) {
        super::query::time_key(clock)
    } else {
        return None;
    };
    let start = date.and_hms_milli_opt(
        clock[0..2].parse().ok()?,
        clock[3..5].parse().ok()?,
        clock[6..8].parse().ok()?,
        milliseconds,
    )?;
    let precision = if let Some(fraction) = fraction {
        Duration::milliseconds(match fraction.len() {
            1 => 100,
            2 => 10,
            _ => 1,
        })
    } else {
        match raw.split_once('T')?.1.len() {
            2 => Duration::hours(1),
            5 => Duration::minutes(1),
            _ => Duration::seconds(1),
        }
    };
    Some((start, precision))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Checks the search ranges specification's timestamp inputs section.
    #[test]
    fn timestamp_inputs_default_omitted_time_components_to_zero() {
        let midnight = Local.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).single().unwrap();
        let morning = Local.with_ymd_and_hms(2026, 2, 12, 5, 0, 0).single().unwrap();
        for input in ["2026-01-01", "2026-01-01T00", "2026-01-01T00:00:00.000"] {
            assert_eq!(parse(input, "captured-at").unwrap().start, midnight.timestamp_millis());
        }
        for input in ["2026-02-12T05", "2026-02-12T05:00", "2026-02-12T05:00:00"] {
            assert_eq!(parse(input, "captured-at").unwrap().start, morning.timestamp_millis());
        }
        assert_eq!(parse("1970-01-01Z", "captured-at").unwrap().start, 0);
        assert_eq!(parse("1969-12-31T19-05:00", "captured-at").unwrap().start, 0);
        assert_eq!(parse("1970-01-01T05:30+05:30", "captured-at").unwrap().start, 0);
        for (input, expected) in [
            ("-1", -1),
            ("1970-01-01T00:00:00.001Z", 1),
            ("1970-01-01T00:00:00.1Z", 100),
            ("1970-01-01T00:00:00.12Z", 120),
            ("1969-12-31T23:59:59.999Z", -1),
        ] {
            assert_eq!(parse(input, "captured-at").unwrap().start, expected, "{input}");
        }
    }

    #[test]
    fn timestamp_periods_expand_to_the_supplied_precision() {
        for (input, start, end) in [
            ("1970-01-01Z", 0, 86_399_999),
            ("1970-01-01T00Z", 0, 3_599_999),
            ("1970-01-01T00:00Z", 0, 59_999),
            ("1970-01-01T00:00:00Z", 0, 999),
            ("1970-01-01T00:00:00.1Z", 100, 199),
            ("1970-01-01T00:00:00.12Z", 120, 129),
            ("1970-01-01T00:00:00.123Z", 123, 123),
            ("1969-12-31T23:59:59.9Z", -100, -1),
            ("1969-12-31T19-05:00", 0, 3_599_999),
            ("-1", -1, -1),
        ] {
            assert_eq!(parse(input, "captured-at").unwrap(), Bounds { start, end }, "{input}");
        }
    }

    // Checks the search ranges specification's calendar years and months section.
    #[test]
    fn timestamp_years_and_months_follow_calendar_boundaries() {
        for (input, start, end) in [
            ("2026Z", "2026-01-01T00:00:00.000Z", "2026-12-31T23:59:59.999Z"),
            ("2024-02Z", "2024-02-01T00:00:00.000Z", "2024-02-29T23:59:59.999Z"),
            ("2026-02Z", "2026-02-01T00:00:00.000Z", "2026-02-28T23:59:59.999Z"),
            ("2026-11Z", "2026-11-01T00:00:00.000Z", "2026-11-30T23:59:59.999Z"),
            ("2026-12Z", "2026-12-01T00:00:00.000Z", "2026-12-31T23:59:59.999Z"),
            ("2026-11-05:00", "2026-11-01T00:00:00.000-05:00", "2026-11-30T23:59:59.999-05:00"),
            ("2026+05:30", "2026-01-01T00:00:00.000+05:30", "2026-12-31T23:59:59.999+05:30"),
        ] {
            let expected = Bounds {
                start: parse(start, "captured-at").unwrap().start,
                end: parse(end, "captured-at").unwrap().end,
            };
            assert_eq!(parse(input, "captured-at").unwrap(), expected, "{input}");
        }
        let local = |year, month, day, hour, minute, second| {
            Local
                .with_ymd_and_hms(year, month, day, hour, minute, second)
                .single()
                .unwrap()
                .timestamp_millis()
        };
        assert_eq!(
            parse("2026", "captured-at").unwrap(),
            Bounds {
                start: local(2026, 1, 1, 0, 0, 0),
                end: local(2026, 12, 31, 23, 59, 59) + 999
            }
        );
        assert_eq!(
            parse("2026-11", "captured-at").unwrap(),
            Bounds {
                start: local(2026, 11, 1, 0, 0, 0),
                end: local(2026, 11, 30, 23, 59, 59) + 999
            }
        );
        for (input, milliseconds) in [
            ("ms:2026", 2026),
            ("ms:-2026", -2026),
            ("ms:0", 0),
            ("1700000000000", 1_700_000_000_000),
        ] {
            assert_eq!(
                parse(input, "captured-at").unwrap(),
                Bounds { start: milliseconds, end: milliseconds }
            );
        }
    }

    #[test]
    fn timestamp_inputs_reject_invalid_components() {
        for input in [
            "2026-02-29",
            "2026-01-01T",
            "2026-01-01T5",
            "2026-01-01T24",
            "2026-01-01T05:60",
            "2026-01-01T05:00:60",
            "2026-01-01T05:00.1",
            "2026-01-01T05:00:00.",
            "2026-01-01T05:00:00.0001",
            "2026-01-01T05-25:00",
            "2026-01-01T05+05:60",
            "2026-01-01ZZ",
            "2026-01-01T０５",
            "2026-00",
            "2026-13",
            "2026-1",
            "2026-11T05",
            "2026T05",
            "ms:2026-11",
            "ms:",
        ] {
            assert!(matches!(parse(input, "captured-at"), Err(Error::SearchQuery(_))), "{input}");
        }
        assert!(parse("2024-02-29Z", "captured-at").is_ok());
    }

    #[test]
    fn timestamp_inputs_resolve_local_clock_changes() {
        // A child process sets the timezone before Chrono caches it, without changing the test
        // runner's environment or depending on the machine's timezone database.
        const CHILD: &str = "ZIB_TIMESTAMP_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "search::timestamp::tests::timestamp_inputs_resolve_local_clock_changes",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env("TZ", "EST5EDT,M3.2.0,M11.1.0")
                .output()
                .unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
            return;
        }
        assert_eq!(
            parse("2026-01-01", "captured-at").unwrap().start,
            parse("2026-01-01T05Z", "captured-at").unwrap().start
        );
        assert_eq!(
            parse("2026-07-01", "captured-at").unwrap().start,
            parse("2026-07-01T04Z", "captured-at").unwrap().start
        );
        for (input, message) in
            [("2026-03-08T02:30", "doesn't exist"), ("2026-11-01T01:30", "occurs twice")]
        {
            let error = parse(input, "captured-at").unwrap_err().to_string();
            assert!(error.contains(message), "{error}");
            assert!(error.contains("offset"), "{error}");
        }
        let first = parse("2026-11-01T01:30-04:00", "captured-at").unwrap().start;
        let second = parse("2026-11-01T01:30-05:00", "captured-at").unwrap().start;
        assert_eq!(second - first, 3_600_000);
        for (input, hours) in [("2026-03-08", 23), ("2026-11-01", 25)] {
            let bounds = parse(input, "captured-at").unwrap();
            assert_eq!(bounds.end - bounds.start + 1, hours * 3_600_000);
        }
        for (input, hours) in [("2026-03", 31 * 24 - 1), ("2026-11", 30 * 24 + 1)] {
            let bounds = parse(input, "captured-at").unwrap();
            assert_eq!(bounds.end - bounds.start + 1, hours * 3_600_000);
        }
        assert_eq!(
            parse("2026-02-19", "captured-at").unwrap().start,
            parse("2026-02-19T00:00:00.000-05:00", "captured-at").unwrap().start
        );
        assert_eq!(
            parse("2026-03-15", "captured-at").unwrap().end,
            parse("2026-03-15T23:59:59.999-04:00", "captured-at").unwrap().start
        );
    }
}
