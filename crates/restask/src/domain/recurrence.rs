//! Recurrence (§3.6): a repeat rule with two spellings — the vault's `🔁 every …` text
//! and iCalendar's `RRULE` — and the arithmetic to find the occurrence that follows a
//! completed one. Pure — no I/O.
//!
//! Only rules that both spellings can express exactly are *managed* (shown and editable
//! in the vault). Anything richer stays the server's: it travels as unmanaged content
//! and is still honoured when an occurrence is completed (§11.6).

use chrono::{Datelike, Duration, NaiveDate, Weekday};

use crate::domain::dates::{LocalDate, LocalDateTime, When};

/// How often a rule repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Freq {
    /// Every n minutes.
    Minutely,
    /// Every n hours.
    Hourly,
    /// Every n days.
    Daily,
    /// Every n weeks.
    Weekly,
    /// Every n months.
    Monthly,
    /// Every n years.
    Yearly,
}

/// A repeat rule (§3.6).
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Recurrence {
    freq: Freq,
    interval: u32,
    /// `BYDAY`: weekday with an optional ordinal (`2TU`, `-1FR`).
    by_day: Vec<(Option<i32>, Weekday)>,
    /// `BYMONTHDAY`: 1..=31, or negative from the end of the month.
    by_month_day: Vec<i32>,
    count: Option<u32>,
    until: Option<NaiveDate>,
}

/// Longest search for a next occurrence, in candidate steps.
const MAX_STEPS: u32 = 100_000;

/// Weekdays in rule order, with their `RRULE` code and their name in the vault.
const WEEKDAYS: [(Weekday, &str, &str); 7] = [
    (Weekday::Mon, "MO", "Monday"),
    (Weekday::Tue, "TU", "Tuesday"),
    (Weekday::Wed, "WE", "Wednesday"),
    (Weekday::Thu, "TH", "Thursday"),
    (Weekday::Fri, "FR", "Friday"),
    (Weekday::Sat, "SA", "Saturday"),
    (Weekday::Sun, "SU", "Sunday"),
];

/// Units in the vault spelling: singular, plural, frequency.
const UNITS: [(&str, &str, Freq); 6] = [
    ("minute", "minutes", Freq::Minutely),
    ("hour", "hours", Freq::Hourly),
    ("day", "days", Freq::Daily),
    ("week", "weeks", Freq::Weekly),
    ("month", "months", Freq::Monthly),
    ("year", "years", Freq::Yearly),
];

impl Recurrence {
    // ── RRULE spelling ────────────────────────────────────────────────────────────────

    /// Parses an `RRULE` value (`FREQ=WEEKLY;INTERVAL=2;BYDAY=MO`). The flag says whether
    /// the rule was understood **exactly** — every part known and expressible in the
    /// vault spelling; only then may it be managed. An inexact rule is still usable for
    /// computing occurrences from its known parts. `None` without a usable `FREQ`.
    pub fn from_rrule(value: &str) -> Option<(Recurrence, bool)> {
        let mut exact = true;
        let mut freq = None;
        let mut rule = Recurrence {
            freq: Freq::Daily,
            interval: 1,
            by_day: Vec::new(),
            by_month_day: Vec::new(),
            count: None,
            until: None,
        };
        for part in value.split(';').filter(|part| !part.trim().is_empty()) {
            let Some((key, value)) = part.split_once('=') else {
                exact = false;
                continue;
            };
            let value = value.trim();
            match key.trim().to_ascii_uppercase().as_str() {
                "FREQ" => {
                    freq = Some(match value.to_ascii_uppercase().as_str() {
                        "MINUTELY" => Freq::Minutely,
                        "HOURLY" => Freq::Hourly,
                        "DAILY" => Freq::Daily,
                        "WEEKLY" => Freq::Weekly,
                        "MONTHLY" => Freq::Monthly,
                        "YEARLY" => Freq::Yearly,
                        _ => return None,
                    });
                }
                "INTERVAL" => match value.parse::<u32>() {
                    Ok(n) if n > 0 => rule.interval = n,
                    _ => exact = false,
                },
                "BYDAY" => {
                    for item in value.split(',') {
                        match parse_by_day(item) {
                            Some(day) => rule.by_day.push(day),
                            None => exact = false,
                        }
                    }
                }
                "BYMONTHDAY" => {
                    for item in value.split(',') {
                        match item.trim().parse::<i32>() {
                            Ok(n) if n != 0 && n.abs() <= 31 => rule.by_month_day.push(n),
                            _ => exact = false,
                        }
                    }
                }
                "COUNT" => match value.parse::<u32>() {
                    Ok(n) => rule.count = Some(n),
                    Err(_) => exact = false,
                },
                "UNTIL" => {
                    rule.until = value
                        .get(..8)
                        .and_then(|day| NaiveDate::parse_from_str(day, "%Y%m%d").ok());
                    exact &= rule.until.is_some();
                }
                // Monday is the week start the arithmetic assumes.
                "WKST" => exact &= value.eq_ignore_ascii_case("MO"),
                _ => exact = false,
            }
        }
        rule.freq = freq?;
        let rule = rule.normalized();
        let exact = exact && rule.representable();
        Some((rule, exact))
    }

    /// The canonical `RRULE` value of this rule.
    pub fn to_rrule(&self) -> String {
        let freq = match self.freq {
            Freq::Minutely => "MINUTELY",
            Freq::Hourly => "HOURLY",
            Freq::Daily => "DAILY",
            Freq::Weekly => "WEEKLY",
            Freq::Monthly => "MONTHLY",
            Freq::Yearly => "YEARLY",
        };
        let mut parts = vec![format!("FREQ={freq}")];
        if self.interval != 1 {
            parts.push(format!("INTERVAL={}", self.interval));
        }
        if !self.by_day.is_empty() {
            let days: Vec<String> = self
                .by_day
                .iter()
                .map(|(ordinal, day)| {
                    let code = weekday_entry(*day).1;
                    match ordinal {
                        Some(n) => format!("{n}{code}"),
                        None => code.to_string(),
                    }
                })
                .collect();
            parts.push(format!("BYDAY={}", days.join(",")));
        }
        if !self.by_month_day.is_empty() {
            let days: Vec<String> = self.by_month_day.iter().map(i32::to_string).collect();
            parts.push(format!("BYMONTHDAY={}", days.join(",")));
        }
        if let Some(count) = self.count {
            parts.push(format!("COUNT={count}"));
        }
        if let Some(until) = self.until {
            parts.push(format!("UNTIL={}", until.format("%Y%m%d")));
        }
        parts.join(";")
    }

    // ── vault spelling ────────────────────────────────────────────────────────────────

    /// Parses the vault spelling at the start of `text` and returns the rule with the
    /// number of bytes it occupies; what follows is not part of the rule.
    ///
    /// ```text
    /// every [N] minute|hour|day|week|month|year[s]      every weekday
    ///   [on Monday, Thursday]                           (weeks)
    ///   [on the 15th | the 1st, 15th | the last day]    (months)
    ///   [on the 2nd Tuesday | the last Friday]          (months)
    ///   [for N times] [until YYYY-MM-DD]
    /// ```
    pub fn from_text(text: &str) -> Option<(Recurrence, usize)> {
        let words = words(text);
        let word = |i: usize| words.get(i).map(|w| w.text.to_ascii_lowercase());
        if word(0)? != "every" {
            return None;
        }
        let mut rule = Recurrence {
            freq: Freq::Daily,
            interval: 1,
            by_day: Vec::new(),
            by_month_day: Vec::new(),
            count: None,
            until: None,
        };
        let mut at = 1;
        if word(at)? == "weekday" || word(at)? == "weekdays" {
            rule.freq = Freq::Weekly;
            rule.by_day = WEEKDAYS[..5]
                .iter()
                .map(|(day, _, _)| (None, *day))
                .collect();
            at += 1;
        } else {
            if let Ok(n) = word(at)?.parse::<u32>() {
                if n == 0 {
                    return None;
                }
                rule.interval = n;
                at += 1;
            }
            let unit = word(at)?;
            rule.freq = UNITS
                .iter()
                .find(|(one, many, _)| unit == *one || unit == *many)?
                .2;
            at += 1;
            if word(at).as_deref() == Some("on") {
                let mut next = at + 1;
                let mut any = false;
                loop {
                    while matches!(word(next).as_deref(), Some("the" | "and")) {
                        next += 1;
                    }
                    let Some(first) = word(next) else { break };
                    let second = word(next + 1);
                    let second_day = second.as_deref().and_then(weekday_named);
                    match rule.freq {
                        Freq::Weekly => match weekday_named(&first) {
                            Some(day) => {
                                rule.by_day.push((None, day));
                                next += 1;
                            }
                            None => break,
                        },
                        Freq::Monthly => {
                            let ordinal = if first == "last" {
                                Some(-1)
                            } else {
                                parse_ordinal(&first)
                            };
                            match (ordinal, second_day, second.as_deref()) {
                                (Some(n), Some(day), _) => {
                                    rule.by_day.push((Some(n), day));
                                    next += 2;
                                }
                                (Some(-1), None, Some("day")) => {
                                    rule.by_month_day.push(-1);
                                    next += 2;
                                }
                                (Some(n), None, _) if n > 0 => {
                                    rule.by_month_day.push(n);
                                    next += 1;
                                }
                                _ => break,
                            }
                        }
                        _ => break,
                    }
                    any = true;
                    at = next;
                }
                if !any {
                    // "on" introduced nothing this rule understands: it is task text.
                    rule.by_day.clear();
                    rule.by_month_day.clear();
                }
            }
        }
        if word(at).as_deref() == Some("for") {
            if let (Some(Ok(n)), Some("time" | "times")) = (
                word(at + 1).map(|w| w.parse::<u32>()),
                word(at + 2).as_deref(),
            ) {
                rule.count = Some(n);
                at += 3;
            }
        }
        if word(at).as_deref() == Some("until") {
            if let Some(day) = word(at + 1).and_then(|w| LocalDate::parse(&w).ok()) {
                rule.until = Some(day.0);
                at += 2;
            }
        }
        let rule = rule.normalized();
        if !rule.representable() {
            return None;
        }
        let end = words.get(at - 1)?.end;
        Some((rule, end))
    }

    /// The canonical vault spelling (`every 2 weeks on Monday, Thursday`).
    pub fn to_text(&self) -> String {
        let weekdays = self.freq == Freq::Weekly
            && self.interval == 1
            && self.by_day.len() == 5
            && self
                .by_day
                .iter()
                .zip(&WEEKDAYS[..5])
                .all(|((ordinal, day), (expected, _, _))| ordinal.is_none() && day == expected);
        let mut out = if weekdays {
            "every weekday".to_string()
        } else {
            let (one, many, _) = UNITS
                .iter()
                .find(|(_, _, freq)| *freq == self.freq)
                .copied()
                .unwrap_or(UNITS[2]);
            let mut out = match self.interval {
                1 => format!("every {one}"),
                n => format!("every {n} {many}"),
            };
            let mut items: Vec<String> = Vec::new();
            for (ordinal, day) in &self.by_day {
                let name = weekday_entry(*day).2;
                items.push(match ordinal {
                    None => name.to_string(),
                    Some(-1) => format!("last {name}"),
                    Some(n) => format!("{} {name}", ordinal_text(*n)),
                });
            }
            for n in &self.by_month_day {
                items.push(match n {
                    -1 => "last day".to_string(),
                    n => ordinal_text(*n),
                });
            }
            if !items.is_empty() {
                let the = if self.freq == Freq::Monthly {
                    "the "
                } else {
                    ""
                };
                out.push_str(&format!(" on {the}{}", items.join(", ")));
            }
            out
        };
        if let Some(count) = self.count {
            let times = if count == 1 { "time" } else { "times" };
            out.push_str(&format!(" for {count} {times}"));
        }
        if let Some(until) = self.until {
            out.push_str(&format!(" until {}", until.format("%Y-%m-%d")));
        }
        out
    }

    /// Whether the vault spelling can express this rule exactly.
    fn representable(&self) -> bool {
        match self.freq {
            Freq::Minutely | Freq::Hourly | Freq::Daily | Freq::Yearly => {
                self.by_day.is_empty() && self.by_month_day.is_empty()
            }
            Freq::Weekly => {
                self.by_month_day.is_empty() && self.by_day.iter().all(|(n, _)| n.is_none())
            }
            Freq::Monthly => {
                (self.by_day.is_empty() || self.by_month_day.is_empty())
                    && self
                        .by_day
                        .iter()
                        .all(|(n, _)| matches!(n, Some(-1 | 1..=5)))
                    && self.by_month_day.iter().all(|n| *n == -1 || *n > 0)
            }
        }
    }

    /// Sorted, de-duplicated lists: one rule, one representation.
    fn normalized(mut self) -> Self {
        self.by_day
            .sort_by_key(|(n, day)| (n.is_some_and(|n| n < 0), *n, day.num_days_from_monday()));
        self.by_day.dedup();
        self.by_month_day.sort_by_key(|n| (*n < 0, *n));
        self.by_month_day.dedup();
        self
    }

    // ── occurrences ───────────────────────────────────────────────────────────────────

    /// `true` when the occurrence being completed is the rule's last one (`COUNT` ≤ 1):
    /// completing it completes the task.
    pub fn is_last(&self) -> bool {
        self.count.is_some_and(|count| count <= 1)
    }

    /// The rule after one occurrence was completed: identical, a `COUNT` decremented.
    pub fn consumed(&self) -> Recurrence {
        let mut rule = self.clone();
        rule.count = rule.count.map(|count| count.saturating_sub(1));
        rule
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
        let on_listed_day = |day: NaiveDate| self.by_day.iter().any(|(_, wd)| *wd == day.weekday());
        match self.freq {
            // A sub-daily rule on a date-only task repeats, at the earliest, the next day.
            Freq::Minutely | Freq::Hourly => after.succ_opt(),
            Freq::Daily => {
                let first = (after - anchor).num_days().div_euclid(interval) + 1;
                // `FREQ=DAILY;BYDAY=…` (e.g. working days): skip the unlisted days.
                (first..first + i64::from(MAX_STEPS))
                    .filter_map(|steps| {
                        anchor.checked_add_signed(Duration::days(steps.checked_mul(interval)?))
                    })
                    .find(|day| self.by_day.is_empty() || on_listed_day(*day))
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
                    if on_listed_day(day) && week.rem_euclid(interval) == 0 {
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

/// One whitespace-separated word of the vault spelling, without a trailing comma.
struct Word<'a> {
    text: &'a str,
    /// Byte offset just past the word (comma excluded).
    end: usize,
}

/// Splits `text` into words, remembering where each ends.
fn words(text: &str) -> Vec<Word<'_>> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut push = |from: usize, to: usize| {
        let raw = &text[from..to];
        let word = raw.trim_end_matches(',');
        if !word.is_empty() {
            out.push(Word {
                text: word,
                end: from + word.len(),
            });
        }
    };
    for (index, ch) in text.char_indices() {
        if ch == ' ' || ch == '\t' {
            if let Some(from) = start.take() {
                push(from, index);
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(from) = start {
        push(from, text.len());
    }
    out
}

fn weekday_entry(day: Weekday) -> (Weekday, &'static str, &'static str) {
    WEEKDAYS
        .iter()
        .copied()
        .find(|(candidate, _, _)| *candidate == day)
        .unwrap_or(WEEKDAYS[0])
}

/// A weekday by its (lower-cased) name or three-letter abbreviation.
fn weekday_named(word: &str) -> Option<Weekday> {
    WEEKDAYS
        .iter()
        .find(|(_, _, name)| {
            let name = name.to_ascii_lowercase();
            word == name || (word.len() == 3 && name.starts_with(word))
        })
        .map(|(day, _, _)| *day)
}

/// `15th` / `1st` / `2nd` / `3rd` / `15` → 15.
fn parse_ordinal(word: &str) -> Option<i32> {
    let digits = word
        .strip_suffix("st")
        .or_else(|| word.strip_suffix("nd"))
        .or_else(|| word.strip_suffix("rd"))
        .or_else(|| word.strip_suffix("th"))
        .unwrap_or(word);
    digits.parse::<i32>().ok().filter(|n| (1..=31).contains(n))
}

/// 1 → `1st`, 2 → `2nd`, 11 → `11th`, 23 → `23rd`.
fn ordinal_text(n: i32) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// Parses one `BYDAY` element: `MO`, `2TU`, `-1FR`.
fn parse_by_day(raw: &str) -> Option<(Option<i32>, Weekday)> {
    let raw = raw.trim();
    let split = raw.len().checked_sub(2)?;
    if !raw.is_char_boundary(split) {
        return None;
    }
    let (ordinal, code) = raw.split_at(split);
    let weekday = WEEKDAYS
        .iter()
        .find(|(_, candidate, _)| candidate.eq_ignore_ascii_case(code))?
        .0;
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
