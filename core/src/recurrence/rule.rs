//! Date-only recurrence patterns shared by the three task modes.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use chrono::{Datelike, Days, Months, NaiveDate, Weekday};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Keep,
    Jump,
    Interval,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Frequency {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Rule {
    pub mode: Mode,
    pub start: NaiveDate,
    pub scheduled: Option<i32>,
    pub due: Option<i32>,
    pub pattern: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pattern {
    pub frequency: Frequency,
    pub interval: u32,
    pub count: Option<u32>,
    pub until: Option<NaiveDate>,
    pub week_start: Weekday,
    pub weekdays: Vec<(Option<i8>, Weekday)>,
    pub month_day: Option<u32>,
    pub month: Option<u32>,
}

impl Rule {
    /// Offset-only edits keep the schedule and its existing occurrence identities.
    pub fn same_schedule(&self, other: &Self) -> bool {
        self.mode == other.mode && self.start == other.start && self.pattern == other.pattern
    }

    /// Validates a date-only rule and returns its first matching occurrence.
    pub fn first(&self) -> Result<NaiveDate> {
        let pattern = self.compile()?;
        if self.mode == Mode::Interval && pattern.admits(1, self.start) {
            return Ok(self.start);
        }
        pattern.dates(self.start).next().ok_or_else(|| {
            Error::Space("No matching recurrence date within the rule's bounds".into())
        })
    }

    /// Returns up to five initial calendar dates. Completion intervals have only a first date.
    pub fn preview(&self) -> Result<Vec<NaiveDate>> {
        self.first()?;
        if self.mode == Mode::Interval {
            return Ok(vec![self.start]);
        }
        Ok(self.compile()?.dates(self.start).take(5).collect())
    }

    pub(super) fn compile(&self) -> Result<Pattern> {
        let pattern: Pattern = self.pattern.parse()?;
        if self.mode == Mode::Interval
            && (!pattern.weekdays.is_empty()
                || pattern.month.is_some()
                || pattern.month_day.is_some())
        {
            return Err(Error::Space(
                "Completion intervals use only frequency and interval".into(),
            ));
        }
        if pattern.until.is_some_and(|date| date < self.start) {
            return Err(Error::Space("The end date precedes the start date".into()));
        }
        Ok(pattern)
    }
}

impl FromStr for Pattern {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        let invalid = || Error::invalid("recurrence rule", text);
        let mut parts = BTreeMap::new();
        let upper = text.trim().to_ascii_uppercase();
        for part in upper.trim_start_matches("RRULE:").split(';') {
            let (name, value) = part.split_once('=').ok_or_else(invalid)?;
            if parts.insert(name.trim(), value.trim()).is_some() {
                return Err(invalid());
            }
        }
        let frequency = match parts.remove("FREQ") {
            Some("DAILY") => Frequency::Daily,
            Some("WEEKLY") => Frequency::Weekly,
            Some("MONTHLY") => Frequency::Monthly,
            Some("YEARLY") => Frequency::Yearly,
            _ => return Err(invalid()),
        };
        let interval = parts.remove("INTERVAL").unwrap_or("1").parse().map_err(|_| invalid())?;
        let mut number = |name| -> Result<Option<u32>> {
            parts.remove(name).map(|s| s.parse().map_err(|_| invalid())).transpose()
        };
        let count = number("COUNT")?;
        let month = number("BYMONTH")?;
        let month_day = number("BYMONTHDAY")?;
        let until = parts
            .remove("UNTIL")
            .map(|s| {
                NaiveDate::parse_from_str(s, "%Y%m%d").or_else(|_| s.parse()).map_err(|_| invalid())
            })
            .transpose()?;
        let week_start = weekday(parts.remove("WKST").unwrap_or("MO")).ok_or_else(invalid)?;
        let mut weekdays = Vec::new();
        if let Some(days) = parts.remove("BYDAY") {
            for day in days.split(',') {
                let (position, day) =
                    day.split_at_checked(day.len().saturating_sub(2)).ok_or_else(invalid)?;
                let position = if position.is_empty() {
                    None
                } else {
                    let n = position.parse::<i8>().map_err(|_| invalid())?;
                    if !matches!(n, -1 | 1..=5)
                        || !matches!(frequency, Frequency::Monthly | Frequency::Yearly)
                        || frequency == Frequency::Yearly && month.is_none()
                    {
                        return Err(invalid());
                    }
                    Some(n)
                };
                weekdays.push((position, weekday(day).ok_or_else(invalid)?));
            }
        }
        if interval == 0
            || count == Some(0)
            || count.is_some() && until.is_some()
            || month.is_some_and(|n| !(1..=12).contains(&n))
            || month_day.is_some_and(|n| !(1..=31).contains(&n))
            || !parts.is_empty()
        {
            return Err(invalid());
        }
        Ok(Self { frequency, interval, count, until, week_start, weekdays, month_day, month })
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let frequency = match self.frequency {
            Frequency::Daily => "DAILY",
            Frequency::Weekly => "WEEKLY",
            Frequency::Monthly => "MONTHLY",
            Frequency::Yearly => "YEARLY",
        };
        write!(formatter, "FREQ={frequency}")?;
        if self.interval != 1 {
            write!(formatter, ";INTERVAL={}", self.interval)?;
        }
        if !self.weekdays.is_empty() {
            let mut weekdays = self.weekdays.clone();
            weekdays.sort_by_key(|(ordinal, day)| (*ordinal, day.num_days_from_monday()));
            weekdays.dedup();
            formatter.write_str(";BYDAY=")?;
            for (index, (ordinal, day)) in weekdays.into_iter().enumerate() {
                if index != 0 {
                    formatter.write_str(",")?;
                }
                if let Some(ordinal) = ordinal {
                    write!(formatter, "{ordinal}")?;
                }
                formatter.write_str(weekday_name(day))?;
            }
        }
        if let Some(month) = self.month {
            write!(formatter, ";BYMONTH={month}")?;
        }
        if let Some(day) = self.month_day {
            write!(formatter, ";BYMONTHDAY={day}")?;
        }
        if let Some(count) = self.count {
            write!(formatter, ";COUNT={count}")?;
        }
        if let Some(until) = self.until {
            write!(formatter, ";UNTIL={}", until.format("%Y%m%d"))?;
        }
        if self.week_start != Weekday::Mon {
            write!(formatter, ";WKST={}", weekday_name(self.week_start))?;
        }
        Ok(())
    }
}

impl Pattern {
    pub(super) fn admits(&self, position: u32, day: NaiveDate) -> bool {
        self.count.is_none_or(|n| position <= n) && self.until.is_none_or(|end| day <= end)
    }

    pub(super) fn dates(&self, start: NaiveDate) -> impl Iterator<Item = NaiveDate> + '_ {
        // Zib's date fields use four-digit years.
        let end = self.until.unwrap_or(NaiveDate::from_ymd_opt(9999, 12, 31).unwrap());
        let mut day = Some(start);
        let mut position = 0;
        std::iter::from_fn(move || {
            while let Some(date) = day.filter(|d| *d <= end) {
                day = date.succ_opt();
                if self.matches(start, date) {
                    position += 1;
                    return self.admits(position, date).then_some(date);
                }
            }
            None
        })
        .fuse()
    }

    fn matches(&self, start: NaiveDate, date: NaiveDate) -> bool {
        let days = date.signed_duration_since(start).num_days();
        let years = i64::from(date.year() - start.year());
        let period = match self.frequency {
            Frequency::Daily => days,
            Frequency::Weekly => {
                let offset = (start.weekday().num_days_from_monday() + 7
                    - self.week_start.num_days_from_monday())
                    % 7;
                (days + i64::from(offset)) / 7
            }
            Frequency::Monthly => years * 12 + i64::from(date.month()) - i64::from(start.month()),
            Frequency::Yearly => years,
        };
        if period % i64::from(self.interval) != 0 {
            return false;
        }
        let default_month = (self.frequency == Frequency::Yearly
            && self.weekdays.is_empty()
            && self.month_day.is_none())
        .then_some(start.month());
        if self.month.or(default_month).is_some_and(|month| month != date.month()) {
            return false;
        }
        let default_day = (matches!(self.frequency, Frequency::Monthly | Frequency::Yearly)
            && self.weekdays.is_empty())
        .then_some(start.day());
        if self.month_day.or(default_day).is_some_and(|day| day != date.day()) {
            return false;
        }
        if self.weekdays.is_empty() {
            return self.frequency != Frequency::Weekly || date.weekday() == start.weekday();
        }
        self.weekdays.iter().any(|(ordinal, day)| {
            *day == date.weekday()
                && match ordinal {
                    None => true,
                    Some(-1) => date
                        .checked_add_days(Days::new(7))
                        .is_none_or(|next| next.month() != date.month()),
                    Some(n) => (date.day() - 1) / 7 + 1 == u32::try_from(*n).unwrap_or(0),
                }
        })
    }

    pub(super) fn after(&self, day: NaiveDate) -> Option<NaiveDate> {
        match self.frequency {
            Frequency::Daily => day.checked_add_days(Days::new(u64::from(self.interval))),
            Frequency::Weekly => day.checked_add_days(Days::new(7 * u64::from(self.interval))),
            Frequency::Monthly => day.checked_add_months(Months::new(self.interval)),
            Frequency::Yearly => {
                day.checked_add_months(Months::new(self.interval.checked_mul(12)?))
            }
        }
        .filter(|d| d.year() <= 9999)
    }
}

fn weekday(text: &str) -> Option<Weekday> {
    Some(match text {
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        "SU" => Weekday::Sun,
        _ => return None,
    })
}

fn weekday_name(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "MO",
        Weekday::Tue => "TU",
        Weekday::Wed => "WE",
        Weekday::Thu => "TH",
        Weekday::Fri => "FR",
        Weekday::Sat => "SA",
        Weekday::Sun => "SU",
    }
}
