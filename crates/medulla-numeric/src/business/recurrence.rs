//! Recurrence and compliance calendar (MEDULLA-BUSINESS-OPS-R002.2).
//!
//! Kind `recurrence`. Input: a [`RecurrenceRule`], a [`HolidayCalendar`] revision, an
//! as-of date and a count up to [`MAX_COUNT`]. Output: the next `count` due dates after
//! the as-of date. Supported: daily, weekly, monthly, quarterly and yearly; an interval;
//! a day of month, the last day, or the nth weekday; a month list; and the adjustment
//! `none`, `next_business_day` or `previous_business_day`. A date that does not exist
//! (day 31 in a short month) clamps to the last day of the month. The rule names its IANA
//! zone and the result states it; every date is a calendar date in that zone, so no clock
//! and no time zone table is read.

use std::collections::BTreeSet;

use super::rule_spec::{RuleRefusal, RuleTrace};

/// The most due dates one evaluation returns.
pub const MAX_COUNT: usize = 60;

/// The most periods one evaluation steps over looking for the first due date after the
/// as-of date.
pub const MAX_PERIODS: usize = 100_000;

/// The refusal code for a count outside `1..=MAX_COUNT`.
pub const RECURRENCE_COUNT_INVALID: &str = "RECURRENCE_COUNT_INVALID";
/// The refusal code for a rule whose fields are out of range.
pub const RECURRENCE_RULE_INVALID: &str = "RECURRENCE_RULE_INVALID";
/// The refusal code for a rule with no due date within [`MAX_PERIODS`] periods.
pub const RECURRENCE_HORIZON_EXCEEDED: &str = "RECURRENCE_HORIZON_EXCEEDED";

/// A calendar date, held as days since 1970-01-01.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date(i64);

/// A day of the week, Monday first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Weekday {
    /// Monday.
    Monday,
    /// Tuesday.
    Tuesday,
    /// Wednesday.
    Wednesday,
    /// Thursday.
    Thursday,
    /// Friday.
    Friday,
    /// Saturday.
    Saturday,
    /// Sunday.
    Sunday,
}

impl Weekday {
    fn index(self) -> i64 {
        match self {
            Self::Monday => 0,
            Self::Tuesday => 1,
            Self::Wednesday => 2,
            Self::Thursday => 3,
            Self::Friday => 4,
            Self::Saturday => 5,
            Self::Sunday => 6,
        }
    }
}

impl Date {
    /// The date `year`-`month`-`day`, or `None` when it does not exist.
    pub fn from_ymd(year: i32, month: u8, day: u8) -> Option<Self> {
        if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
            return None;
        }
        Some(Self(days_from_civil(year, month, day)))
    }

    /// The date `days` days after 1970-01-01.
    pub fn from_epoch_days(days: i64) -> Self {
        Self(days)
    }

    /// Days since 1970-01-01.
    pub fn epoch_days(self) -> i64 {
        self.0
    }

    /// The year, month and day.
    pub fn ymd(self) -> (i32, u8, u8) {
        civil_from_days(self.0)
    }

    /// Monday is 0 and Sunday is 6.
    fn weekday_index(self) -> i64 {
        // 1970-01-01 was a Thursday.
        (self.0 + 3).rem_euclid(7)
    }

    /// Whether the date falls on a Saturday or a Sunday.
    pub fn is_weekend(self) -> bool {
        self.weekday_index() >= 5
    }

    /// The date `days` days later (earlier when negative).
    pub fn plus_days(self, days: i64) -> Self {
        Self(self.0 + days)
    }
}

impl std::fmt::Display for Date {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (year, month, day) = self.ymd();
        write!(formatter, "{year:04}-{month:02}-{day:02}")
    }
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap(year) => 29,
        _ => 28,
    }
}

fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let shifted = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = if month_part < 10 {
        month_part + 3
    } else {
        month_part - 9
    };
    let year = year + i64::from(month <= 2);
    (year as i32, month as u8, day as u8)
}

/// How often a rule recurs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frequency {
    /// Every `interval` days.
    Daily,
    /// Every `interval` weeks, on the weekday of the anchor.
    Weekly,
    /// Every `interval` months.
    Monthly,
    /// Every `interval` quarters.
    Quarterly,
    /// Every `interval` years.
    Yearly,
}

/// Which day of the month a monthly, quarterly or yearly rule falls on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DaySelector {
    /// A day of the month, 1 to 31; a month with fewer days clamps to its last day.
    DayOfMonth(u8),
    /// The last day of the month.
    LastDay,
    /// The nth weekday of the month, `n` from 1 to 5; a fifth that does not exist clamps
    /// to the last such weekday.
    NthWeekday {
        /// Which occurrence, 1 to 5.
        n: u8,
        /// The weekday.
        weekday: Weekday,
    },
}

/// The adjustment of a due date that is not a business day.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adjustment {
    /// Keep the date.
    None,
    /// Move to the next business day.
    NextBusinessDay,
    /// Move to the previous business day.
    PreviousBusinessDay,
}

/// A `:RecurrenceRule`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecurrenceRule {
    /// How often the rule recurs.
    pub frequency: Frequency,
    /// Every n-th period of the frequency, at least 1.
    pub interval: u32,
    /// Which day of the month.
    pub day: DaySelector,
    /// The months (1 to 12) the rule falls in; empty means every month of its periods.
    pub months: Vec<u8>,
    /// The business-day adjustment.
    pub adjustment: Adjustment,
    /// The IANA zone the rule is stated in; echoed in the result.
    pub zone: String,
    /// The first date of the first period; fixes the phase of the interval.
    pub anchor: Date,
}

/// A `:HolidayCalendar` revision: the non-business days besides weekends.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HolidayCalendar {
    /// The revision of the calendar that was read.
    pub revision: String,
    /// The holidays.
    pub holidays: BTreeSet<Date>,
}

impl HolidayCalendar {
    /// Whether `date` is a business day: not a weekend and not a holiday.
    pub fn is_business_day(&self, date: Date) -> bool {
        !date.is_weekend() && !self.holidays.contains(&date)
    }
}

/// One due date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Due {
    /// The date the rule falls on, before adjustment.
    pub scheduled: Date,
    /// The due date after the business-day adjustment.
    pub due: Date,
}

/// The result of one evaluation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NextDue {
    /// The zone the rule is stated in.
    pub zone: String,
    /// The calendar revision the adjustment read.
    pub calendar_revision: String,
    /// The next due dates, in order.
    pub dates: Vec<Due>,
    /// The rule digest, the input nodes and the steps.
    pub trace: RuleTrace,
}

fn invalid(detail: impl Into<String>) -> RuleRefusal {
    RuleRefusal::new(RECURRENCE_RULE_INVALID, detail)
}

fn validate(rule: &RecurrenceRule) -> Result<(), RuleRefusal> {
    if rule.interval == 0 {
        return Err(invalid("the interval is at least 1"));
    }
    if rule.zone.is_empty() {
        return Err(invalid("the rule names no zone"));
    }
    if rule.months.iter().any(|month| !(1..=12).contains(month)) {
        return Err(invalid("a month is outside 1 to 12"));
    }
    match rule.day {
        DaySelector::DayOfMonth(day) if !(1..=31).contains(&day) => {
            Err(invalid("the day of month is outside 1 to 31"))
        }
        DaySelector::NthWeekday { n, .. } if !(1..=5).contains(&n) => {
            Err(invalid("the weekday occurrence is outside 1 to 5"))
        }
        _ => Ok(()),
    }
}

/// The date the day selector names in `year`-`month`.
fn select_day(year: i32, month: u8, selector: DaySelector) -> Date {
    let last = days_in_month(year, month);
    match selector {
        DaySelector::DayOfMonth(day) => Date(days_from_civil(year, month, day.min(last))),
        DaySelector::LastDay => Date(days_from_civil(year, month, last)),
        DaySelector::NthWeekday { n, weekday } => {
            let first = Date(days_from_civil(year, month, 1));
            let offset = (weekday.index() - first.weekday_index()).rem_euclid(7);
            let mut day = 1 + offset + 7 * (i64::from(n) - 1);
            while day > i64::from(last) {
                day -= 7;
            }
            Date(days_from_civil(year, month, day as u8))
        }
    }
}

fn adjust(date: Date, adjustment: Adjustment, calendar: &HolidayCalendar) -> Date {
    let step = match adjustment {
        Adjustment::None => return date,
        Adjustment::NextBusinessDay => 1,
        Adjustment::PreviousBusinessDay => -1,
    };
    let mut adjusted = date;
    while !calendar.is_business_day(adjusted) {
        adjusted = adjusted.plus_days(step);
    }
    adjusted
}

/// The scheduled date of period `index` (0 is the anchor's period), or `None` when the
/// period holds no date (its month is not in the month list).
fn scheduled(rule: &RecurrenceRule, index: i64) -> Option<Date> {
    let interval = i64::from(rule.interval);
    let (year, month, _) = rule.anchor.ymd();
    match rule.frequency {
        Frequency::Daily => Some(rule.anchor.plus_days(index * interval)),
        Frequency::Weekly => Some(rule.anchor.plus_days(index * interval * 7)),
        Frequency::Monthly | Frequency::Quarterly | Frequency::Yearly => {
            let months_per_period = match rule.frequency {
                Frequency::Monthly => 1,
                Frequency::Quarterly => 3,
                _ => 12,
            };
            let total =
                i64::from(year) * 12 + i64::from(month) - 1 + index * interval * months_per_period;
            let target_year = total.div_euclid(12) as i32;
            let target_month = (total.rem_euclid(12) + 1) as u8;
            if !rule.months.is_empty() && !rule.months.contains(&target_month) {
                return None;
            }
            Some(select_day(target_year, target_month, rule.day))
        }
    }
}

/// The next `count` due dates after `as_of` (strictly after), each adjusted against
/// `calendar`. `trace` carries the rule digest and the input nodes; the steps taken are
/// appended to it.
pub fn next_due(
    rule: &RecurrenceRule,
    calendar: &HolidayCalendar,
    as_of: Date,
    count: usize,
    mut trace: RuleTrace,
) -> Result<NextDue, RuleRefusal> {
    if count == 0 || count > MAX_COUNT {
        return Err(RuleRefusal::new(
            RECURRENCE_COUNT_INVALID,
            format!("the count is 1 to {MAX_COUNT}, not {count}"),
        ));
    }
    validate(rule)?;
    trace.step(format!(
        "rule {:?} every {} in zone {} from {}",
        rule.frequency, rule.interval, rule.zone, rule.anchor
    ));
    trace.step(format!(
        "adjustment {:?} against calendar revision {}",
        rule.adjustment, calendar.revision
    ));
    let mut dates: Vec<Due> = Vec::with_capacity(count);
    let mut index = 0_i64;
    let mut periods = 0_usize;
    while dates.len() < count {
        periods += 1;
        if periods > MAX_PERIODS {
            return Err(RuleRefusal::new(
                RECURRENCE_HORIZON_EXCEEDED,
                format!("no {count} due dates after {as_of} within {MAX_PERIODS} periods"),
            ));
        }
        let candidate = scheduled(rule, index);
        index += 1;
        let Some(scheduled_date) = candidate else {
            continue;
        };
        let due = adjust(scheduled_date, rule.adjustment, calendar);
        if due <= as_of || dates.last().is_some_and(|last| last.due >= due) {
            continue;
        }
        trace.step(format!("scheduled {scheduled_date} due {due}"));
        dates.push(Due {
            scheduled: scheduled_date,
            due,
        });
    }
    Ok(NextDue {
        zone: rule.zone.clone(),
        calendar_revision: calendar.revision.clone(),
        dates,
        trace,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::business::rule_spec::{load, InputRef, RuleSpec};
    use std::collections::BTreeMap;

    fn date(year: i32, month: u8, day: u8) -> Date {
        Date::from_ymd(year, month, day).expect("a real date")
    }

    fn spec() -> RuleSpec {
        load("recurrence", 1, BTreeMap::new()).unwrap()
    }

    fn trace() -> RuleTrace {
        RuleTrace::new(
            &spec(),
            vec![InputRef::new("rule:r1", 3), InputRef::new("calendar:c1", 7)],
        )
    }

    fn monthly(day: DaySelector, adjustment: Adjustment) -> RecurrenceRule {
        RecurrenceRule {
            frequency: Frequency::Monthly,
            interval: 1,
            day,
            months: Vec::new(),
            adjustment,
            zone: "America/New_York".to_string(),
            anchor: date(2027, 1, 1),
        }
    }

    fn calendar(holidays: &[Date]) -> HolidayCalendar {
        HolidayCalendar {
            revision: "2027-r1".to_string(),
            holidays: holidays.iter().copied().collect(),
        }
    }

    fn dues(result: &NextDue) -> Vec<String> {
        result.dates.iter().map(|d| d.due.to_string()).collect()
    }

    #[test]
    fn civil_dates_round_trip_and_weekdays_are_right() {
        assert_eq!(date(1970, 1, 1).epoch_days(), 0);
        assert_eq!(date(2000, 3, 1).epoch_days(), 11_017);
        for days in [
            -800_000_i64,
            -1,
            0,
            59,
            60,
            365,
            11_016,
            11_017,
            20_000,
            400_000,
        ] {
            let (y, m, d) = Date::from_epoch_days(days).ymd();
            assert_eq!(Date::from_ymd(y, m, d).unwrap().epoch_days(), days);
        }
        assert!(date(2027, 1, 31).is_weekend());
        assert!(!date(2027, 2, 26).is_weekend());
        assert!(Date::from_ymd(2027, 2, 29).is_none());
        assert!(Date::from_ymd(2028, 2, 29).is_some());
    }

    // spec: MEDULLA-BUSINESS-OPS-R002.2
    #[test]
    fn monthly_last_day_clamps_and_adjusts_for_holidays() {
        // Day 31 clamps to the last day of a short month; a weekend or holiday moves back
        // to the previous business day.
        let holidays = calendar(&[date(2027, 2, 26)]);
        let rule = monthly(DaySelector::DayOfMonth(31), Adjustment::PreviousBusinessDay);
        let result = next_due(&rule, &holidays, date(2027, 1, 15), 4, trace()).unwrap();
        assert_eq!(
            dues(&result),
            ["2027-01-29", "2027-02-25", "2027-03-31", "2027-04-30"]
        );
        let scheduled: Vec<String> = result
            .dates
            .iter()
            .map(|d| d.scheduled.to_string())
            .collect();
        assert_eq!(
            scheduled,
            ["2027-01-31", "2027-02-28", "2027-03-31", "2027-04-30"]
        );
        assert_eq!(result.zone, "America/New_York");
        assert_eq!(result.calendar_revision, "2027-r1");
        assert_eq!(result.trace.rule_digest, spec().digest);
        assert_eq!(result.trace.inputs.len(), 2);
        assert!(!result.trace.steps.is_empty());

        // The last-day selector names the same dates; a leap February has 29 days.
        let last = monthly(DaySelector::LastDay, Adjustment::None);
        let leap = next_due(&last, &calendar(&[]), date(2028, 1, 31), 2, trace()).unwrap();
        assert_eq!(dues(&leap), ["2028-02-29", "2028-03-31"]);

        // The next business day moves a weekend date forward.
        let next = monthly(DaySelector::DayOfMonth(31), Adjustment::NextBusinessDay);
        let forward = next_due(&next, &holidays, date(2027, 1, 15), 2, trace()).unwrap();
        assert_eq!(dues(&forward), ["2027-02-01", "2027-03-01"]);

        // The as-of date is excluded: a due date on the as-of date is not "after" it.
        let after = next_due(&rule, &holidays, date(2027, 1, 29), 1, trace()).unwrap();
        assert_eq!(dues(&after), ["2027-02-25"]);

        // The same inputs give the same result.
        let again = next_due(&rule, &holidays, date(2027, 1, 15), 4, trace()).unwrap();
        assert_eq!(again, result);
    }

    #[test]
    fn other_frequencies_selectors_and_refusals() {
        let none = calendar(&[]);
        // Quarterly on the second Tuesday, from January.
        let mut rule = monthly(
            DaySelector::NthWeekday {
                n: 2,
                weekday: Weekday::Tuesday,
            },
            Adjustment::None,
        );
        rule.frequency = Frequency::Quarterly;
        let result = next_due(&rule, &none, date(2027, 1, 1), 3, trace()).unwrap();
        assert_eq!(dues(&result), ["2027-01-12", "2027-04-13", "2027-07-13"]);

        // Yearly with a month list and an interval of two years.
        let mut yearly = monthly(DaySelector::DayOfMonth(15), Adjustment::None);
        yearly.frequency = Frequency::Yearly;
        yearly.interval = 2;
        let result = next_due(&yearly, &none, date(2027, 1, 1), 2, trace()).unwrap();
        assert_eq!(dues(&result), ["2027-01-15", "2029-01-15"]);

        // A month list keeps only its months.
        let mut listed = monthly(DaySelector::DayOfMonth(10), Adjustment::None);
        listed.months = vec![3, 9];
        let result = next_due(&listed, &none, date(2027, 1, 1), 3, trace()).unwrap();
        assert_eq!(dues(&result), ["2027-03-10", "2027-09-10", "2028-03-10"]);

        // Weekly every two weeks, and daily.
        let mut weekly = monthly(DaySelector::LastDay, Adjustment::None);
        weekly.frequency = Frequency::Weekly;
        weekly.interval = 2;
        weekly.anchor = date(2027, 1, 4);
        let result = next_due(&weekly, &none, date(2027, 1, 4), 2, trace()).unwrap();
        assert_eq!(dues(&result), ["2027-01-18", "2027-02-01"]);
        let mut daily = weekly.clone();
        daily.frequency = Frequency::Daily;
        daily.interval = 1;
        let result = next_due(&daily, &none, date(2027, 1, 4), 2, trace()).unwrap();
        assert_eq!(dues(&result), ["2027-01-05", "2027-01-06"]);

        // Refusals: count, interval, day and zone.
        for count in [0, MAX_COUNT + 1] {
            let refused = next_due(&rule, &none, date(2027, 1, 1), count, trace()).unwrap_err();
            assert_eq!(refused.code, RECURRENCE_COUNT_INVALID);
        }
        let mut bad = rule.clone();
        bad.interval = 0;
        assert_eq!(
            next_due(&bad, &none, date(2027, 1, 1), 1, trace())
                .unwrap_err()
                .code,
            RECURRENCE_RULE_INVALID
        );
        let mut bad = rule.clone();
        bad.day = DaySelector::DayOfMonth(32);
        assert_eq!(
            next_due(&bad, &none, date(2027, 1, 1), 1, trace())
                .unwrap_err()
                .code,
            RECURRENCE_RULE_INVALID
        );
        let mut bad = rule;
        bad.zone = String::new();
        assert_eq!(
            next_due(&bad, &none, date(2027, 1, 1), 1, trace())
                .unwrap_err()
                .code,
            RECURRENCE_RULE_INVALID
        );
    }
}
