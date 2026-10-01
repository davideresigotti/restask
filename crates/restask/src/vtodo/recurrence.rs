//! Recurrence rules (`RRULE`, RFC 5545 §3.3.10) — just enough to roll a recurring task
//! forward when one occurrence is completed in the vault (§11.6). Pure — no I/O.
//!
//! restask does not author recurrence: the rule is set in another client and travels as
//! unmanaged content ([`crate::vtodo::RemoteTask::extras`]). What the engine needs is the
//! next occurrence after the one just done, and the rule to hand back.

use chrono::{Datelike, Duration, NaiveDate, Weekday};

use crate::domain::dates::{LocalDate, LocalDateTime, When};

/// How often a rule repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Freq {
    Minutely,
    Hourly,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

/// A parsed `RRULE`, with its place among a resource's extras.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recurrence {
    /// Index of the `RRULE` line in the extras it was found in.
    pub index: usize,
    freq: Freq,
    interval: u32,
    /// `BYDAY`: weekday with an optional ordinal (`2TU`, `-1FR`).
    by_day: Vec<(Option<i32>, Weekday)>,
    /// `BYMONTHDAY`: 1..=31, or negative from the end of the month.
    by_month_day: Vec<i32>,
    count: Option<u32>,
    until: Option<NaiveDate>,
    /// The rule's parts as written (`KEY=value`), to rewrite it faithfully.
    parts: Vec<(String, String)>,
}

/// Longest search for a next occurrence, in candidate steps.
const MAX_STEPS: u32 = 100_000;

impl Recurrence {
    /// Finds the task's own `RRULE` among `extras` (lines nested in a sub-component such
    /// as `VALARM` are not the task's). `None` when there is none or it has no usable
    /// `FREQ`.
    pub fn find(extras: &[String]) -> Option<Recurrence> {
        let mut depth = 0usize;
        for (index, line) in extras.iter().enumerate() {
            let upper = line.to_ascii_uppercase();
            if upper.starts_with("BEGIN:") {
                depth += 1;
            } else if upper.starts_with("END:") {
                depth = depth.saturating_sub(1);
            } else if depth == 0 && (upper.starts_with("RRULE:") || upper.starts_with("RRULE;")) {
                let value = line.split_once(':')?.1;
                return Self::parse(index, value);
            }
        }
        None
    }

    fn parse(index: usize, value: &str) -> Option<Recurrence> {
        let parts: Vec<(String, String)> = value
            .split(';')
            .filter_map(|part| part.split_once('='))
            .map(|(key, value)| (key.trim().to_ascii_uppercase(), value.trim().to_string()))
            .collect();
        let get = |key: &str| {
            parts
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
        };
        let freq = match get("FREQ")?.to_ascii_uppercase().as_str() {
            "MINUTELY" => Freq::Minutely,
            "HOURLY" => Freq::Hourly,
            "DAILY" => Freq::Daily,
            "WEEKLY" => Freq::Weekly,
            "MONTHLY" => Freq::Monthly,
            "YEARLY" => Freq::Yearly,
            _ => return None,
        };
        let interval = get("INTERVAL")
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(1);
        let by_day = get("BYDAY")
            .map(|list| list.split(',').filter_map(parse_by_day).collect())
            .unwrap_or_default();
        let by_month_day = get("BYMONTHDAY")
            .map(|list| {
                list.split(',')
                    .filter_map(|v| v.trim().parse::<i32>().ok())
                    .filter(|v| *v != 0 && v.abs() <= 31)
                    .collect()
            })
            .unwrap_or_default();
        let count = get("COUNT").and_then(|v| v.parse::<u32>().ok());
        let until = get("UNTIL")
            .and_then(|v| v.get(..8))
            .and_then(|day| NaiveDate::parse_from_str(day, "%Y%m%d").ok());
        Some(Recurrence {
            index,
            freq,
            interval,
            by_day,
            by_month_day,
            count,
            until,
            parts,
        })
    }

    /// `true` when the occurrence being completed is the rule's last one (`COUNT=1`):
    /// completing it completes the task.
    pub fn is_last(&self) -> bool {
        self.count.is_some_and(|count| count <= 1)
    }

    /// The `RRULE` line to store after one occurrence was completed: identical, except
    /// that a `COUNT` is decremented.
    pub fn after_one_occurrence(&self) -> String {
        let parts: Vec<String> = self
            .parts
            .iter()
            .map(|(key, value)| match (key.as_str(), self.count) {
                ("COUNT", Some(count)) => format!("COUNT={}", count.saturating_sub(1)),
                _ => format!("{key}={value}"),
            })
            .collect();
        format!("RRULE:{}", parts.join(";"))
    }

    /// The first occurrence after `anchor` (the occurrence just completed) that also lies
    /// after `done_on` — so completing a long-overdue task yields the next upcoming
    /// occurrence, not another overdue one. A timed anchor keeps its time of day.
    /// `None` when the rule has ended (`UNTIL`) or nothing is found.
    pub fn next_after(&self, anchor: When, done_on: LocalDate) -> Option<When> {
        let next = match (self.freq, anchor) {
            (Freq::Minutely | Freq::Hourly, When::DateTime(LocalDateTime(start))) => {
                let step = match self.freq {
                    Freq::Minutely => Duration::minutes(i64::from(self.interval)),
                    _ => Duration::hours(i64::from(self.interval)),
                };
                let mut at = start;
                let mut found = None;
                for _ in 0..MAX_STEPS {
                    at += step;
                    if at.date() >= done_on.0 {
                        found = Some(When::DateTime(LocalDateTime(at)));
                        break;
                    }
                }
                found?
            }
            (_, anchor) => {
                let (day, time) = match anchor {
                    When::Date(LocalDate(day)) => (day, None),
                    When::DateTime(LocalDateTime(at)) => (at.date(), Some(at.time())),
                };
                let after = day.max(done_on.0);
                let next = self.next_day(day, after)?;
                match time {
                    Some(time) => When::DateTime(LocalDateTime(next.and_time(time))),
                    None => When::Date(LocalDate(next)),
                }
            }
        };
        let day = match next {
            When::Date(LocalDate(day)) => day,
            When::DateTime(LocalDateTime(at)) => at.date(),
        };
        self.until.is_none_or(|until| day <= until).then_some(next)
    }

    /// The first occurrence day strictly after `after`, for a series anchored at `anchor`.
    fn next_day(&self, anchor: NaiveDate, after: NaiveDate) -> Option<NaiveDate> {
        let interval = i64::from(self.interval);
        match self.freq {
            // A sub-daily rule on a date-only task repeats, at the earliest, the next day.
            Freq::Minutely | Freq::Hourly => after.succ_opt(),
            Freq::Daily => {
                let gap = (after - anchor).num_days();
                let steps = gap.div_euclid(interval) + 1;
                anchor.checked_add_signed(Duration::days(steps * interval))
            }
            Freq::Weekly if self.by_day.is_empty() => {
                let period = 7 * interval;
                let gap = (after - anchor).num_days();
                let steps = gap.div_euclid(period) + 1;
                anchor.checked_add_signed(Duration::days(steps * period))
            }
            Freq::Weekly => {
                let week_start =
                    anchor - Duration::days(i64::from(anchor.weekday().num_days_from_monday()));
                let mut day = after;
                for _ in 0..MAX_STEPS {
                    day = day.succ_opt()?;
                    let week = (day - week_start).num_days().div_euclid(7);
                    let on_day = self.by_day.iter().any(|(_, wd)| *wd == day.weekday());
                    if on_day && week.rem_euclid(interval) == 0 {
                        return Some(day);
                    }
                }
                None
            }
            Freq::Monthly => {
                for step in 0..1_200u32 {
                    let months = step.checked_mul(self.interval)?;
                    let (year, month) = add_months(anchor.year(), anchor.month(), months);
                    let mut days = self.month_days(year, month, anchor.day());
                    days.sort_unstable();
                    if let Some(day) = days.into_iter().find(|day| *day > after) {
                        return Some(day);
                    }
                }
                None
            }
            Freq::Yearly => (1..=400i32).find_map(|step| {
                let year = anchor
                    .year()
                    .checked_add(step.checked_mul(i32::try_from(self.interval).ok()?)?)?;
                // 29 February exists only in leap years: other years have no occurrence.
                NaiveDate::from_ymd_opt(year, anchor.month(), anchor.day())
                    .filter(|day| *day > after)
            }),
        }
    }

    /// The occurrence days of a monthly rule within one month.
    fn month_days(&self, year: i32, month: u32, anchor_day: u32) -> Vec<NaiveDate> {
        let last = last_day_of_month(year, month);
        if !self.by_month_day.is_empty() {
            return self
                .by_month_day
                .iter()
                .filter_map(|n| {
                    let day = if *n > 0 {
                        u32::try_from(*n).ok()?
                    } else {
                        (i64::from(last) + 1 + i64::from(*n)).try_into().ok()?
                    };
                    NaiveDate::from_ymd_opt(year, month, day)
                })
                .collect();
        }
        if !self.by_day.is_empty() {
            let in_month: Vec<NaiveDate> = (1..=last)
                .filter_map(|day| NaiveDate::from_ymd_opt(year, month, day))
                .collect();
            let mut days = Vec::new();
            for (ordinal, weekday) in &self.by_day {
                let matching: Vec<NaiveDate> = in_month
                    .iter()
                    .copied()
                    .filter(|day| day.weekday() == *weekday)
                    .collect();
                match ordinal {
                    None => days.extend(matching),
                    Some(n) if *n > 0 => days.extend(
                        usize::try_from(*n - 1)
                            .ok()
                            .and_then(|i| matching.get(i))
                            .copied(),
                    ),
                    Some(n) => days.extend(
                        usize::try_from(-*n)
                            .ok()
                            .and_then(|back| matching.len().checked_sub(back))
                            .and_then(|i| matching.get(i))
                            .copied(),
                    ),
                }
            }
            return days;
        }
        // Same day of the month; months too short for it have no occurrence.
        NaiveDate::from_ymd_opt(year, month, anchor_day)
            .into_iter()
            .collect()
    }
}

/// Parses one `BYDAY` element: `MO`, `2TU`, `-1FR`.
fn parse_by_day(raw: &str) -> Option<(Option<i32>, Weekday)> {
    let raw = raw.trim();
    let split = raw.len().checked_sub(2)?;
    if !raw.is_char_boundary(split) {
        return None;
    }
    let (ordinal, day) = raw.split_at(split);
    let weekday = match day.to_ascii_uppercase().as_str() {
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        "SU" => Weekday::Sun,
        _ => return None,
    };
    let ordinal = match ordinal {
        "" => None,
        text => Some(text.parse::<i32>().ok().filter(|n| *n != 0)?),
    };
    Some((ordinal, weekday))
}

/// `(year, month)` plus `months`.
fn add_months(year: i32, month: u32, months: u32) -> (i32, u32) {
    let total = i64::from(year) * 12 + i64::from(month) - 1 + i64::from(months);
    let year = i32::try_from(total.div_euclid(12)).unwrap_or(i32::MAX);
    let month = u32::try_from(total.rem_euclid(12)).unwrap_or(0) + 1;
    (year, month)
}

/// Number of days in a month.
fn last_day_of_month(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = add_months(year, month, 1);
    NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .and_then(|first| first.pred_opt())
        .map_or(28, |last| last.day())
}
