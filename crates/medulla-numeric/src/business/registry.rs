//! The business rule registry (MEDULLA-BUSINESS-OPS-R002.7).
//!
//! [`evaluate_rule`] is the one entry the `EVALUATE RULE` stage calls. It is one exhaustive
//! `match` on [`RuleKind`]: the match is the registry, so a sixth kind is a compile error
//! until it has an arm, and no kind can be registered twice. Each arm parses the rule
//! parameters of its kind, checks the input arm the executor gathered, calls the kind's
//! function and wraps the answer in the one [`RuleResult`] type, whose
//! [`RuleResult::canonical_json`] is the same bytes for the same result on every host.

use std::collections::BTreeMap;

use super::phrase_policy::{self, PhraseVerdict, Policy};
use super::reconcile::{self, CurrencyRule, Params, Reconciliation, Sets};
use super::recurrence::{
    self, Adjustment, Date, DaySelector, Frequency, HolidayCalendar, NextDue, RecurrenceRule,
    Weekday,
};
use super::reorder::{self, OpenLine, ReorderResult, Sale, Stock};
use super::rule_spec::{
    InputRef, RuleKind, RuleRefusal, RuleSpec, RuleTrace, ASOF_REQUIRED, RULE_INPUT_INVALID,
    RULE_PARAMETERS_INVALID,
};
use super::threshold::{self, Aggregate, Applicability, Applies, Comparison, DataPoint, Expr, MetricDef};

/// Milliseconds in one day.
const DAY_MS: i64 = 86_400_000;

/// What the executor gathered from the graph for one evaluation, one arm per rule kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleInputs {
    /// A recurrence rule: its holiday calendar and how many due dates to return.
    Recurrence {
        /// The holiday calendar read; empty when the rows held none.
        calendar: HolidayCalendar,
        /// The calendar node and its revision, when the rows held one.
        calendar_node: Option<InputRef>,
        /// How many due dates (`next_due` refuses outside 1 to 60).
        count: usize,
    },
    /// A threshold rule: the data points of its window.
    Threshold {
        /// The ledger, order and payment points.
        points: Vec<DataPoint>,
    },
    /// A reconcile rule: the orders, payments, refunds, payouts and fees.
    Reconcile {
        /// The node sets to match.
        sets: Sets,
    },
    /// A reorder rule: the sales, the stock level and the open purchase lines.
    Reorder {
        /// Sale lines.
        sales: Vec<Sale>,
        /// The stock level.
        stock: Stock,
        /// Open purchase order lines.
        open_lines: Vec<OpenLine>,
    },
    /// A phrase policy rule: the text to check and the policy to check it against.
    PhrasePolicy {
        /// The claim text.
        text: String,
        /// The locale of the text; empty when none was given.
        locale: String,
        /// The claim policy read.
        policy: Policy,
        /// The policy node and its revision.
        policy_node: InputRef,
    },
}

/// The result of one evaluation, one arm per rule kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleResult {
    /// Next due dates.
    Recurrence(NextDue),
    /// Applicability.
    Threshold(Applicability),
    /// Matches.
    Reconcile(Reconciliation),
    /// A reorder decision.
    Reorder(ReorderResult),
    /// A claim-text verdict.
    PhrasePolicy(PhraseVerdict),
}

impl RuleResult {
    /// The kind that produced this result.
    pub fn kind(&self) -> RuleKind {
        match self {
            Self::Recurrence(_) => RuleKind::Recurrence,
            Self::Threshold(_) => RuleKind::Threshold,
            Self::Reconcile(_) => RuleKind::Reconcile,
            Self::Reorder(_) => RuleKind::Reorder,
            Self::PhrasePolicy(_) => RuleKind::PhrasePolicy,
        }
    }

    /// The rule digest, the input nodes with their revisions and the steps taken.
    pub fn trace(&self) -> &RuleTrace {
        match self {
            Self::Recurrence(result) => &result.trace,
            Self::Threshold(result) => &result.trace,
            Self::Reconcile(result) => &result.trace,
            Self::Reorder(result) => &result.trace,
            Self::PhrasePolicy(result) => &result.trace,
        }
    }

    /// The canonical JSON of the result: object keys sorted by bytes, dates as
    /// `YYYY-MM-DD`, amounts as integers. The same result is the same bytes on every host.
    pub fn canonical_json(&self) -> String {
        let value = match self {
            Self::Recurrence(result) => recurrence_json(result),
            Self::Threshold(result) => threshold_json(result),
            Self::Reconcile(result) => reconcile_json(result),
            Self::Reorder(result) => reorder_json(result),
            Self::PhrasePolicy(result) => phrase_json(result),
        };
        let mut out = String::new();
        value.render(&mut out);
        out
    }
}

/// A JSON value whose objects are `BTreeMap`s, so they print in byte order whatever
/// features the build unifies on any JSON library.
enum Json {
    Null,
    Bool(bool),
    Int(String),
    Text(String),
    List(Vec<Json>),
    Object(BTreeMap<&'static str, Json>),
}

impl Json {
    fn int(value: impl std::fmt::Display) -> Self {
        Self::Int(value.to_string())
    }

    fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }

    fn strings(values: &[String]) -> Self {
        Self::List(values.iter().map(Self::text).collect())
    }

    fn object<const N: usize>(entries: [(&'static str, Json); N]) -> Self {
        Self::Object(entries.into_iter().collect())
    }

    fn render(&self, out: &mut String) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
            Self::Int(digits) => out.push_str(digits),
            Self::Text(text) => render_text(text, out),
            Self::List(items) => {
                out.push('[');
                for (at, item) in items.iter().enumerate() {
                    if at > 0 {
                        out.push(',');
                    }
                    item.render(out);
                }
                out.push(']');
            }
            Self::Object(entries) => {
                out.push('{');
                for (at, (name, value)) in entries.iter().enumerate() {
                    if at > 0 {
                        out.push(',');
                    }
                    render_text(name, out);
                    out.push(':');
                    value.render(out);
                }
                out.push('}');
            }
        }
    }
}

/// A JSON string literal, escaped the way every JSON writer escapes one.
fn render_text(text: &str, out: &mut String) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            control if (control as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

fn optional_int(value: Option<i64>) -> Json {
    value.map_or(Json::Null, Json::int)
}

fn recurrence_json(result: &NextDue) -> Json {
    let dates = result
        .dates
        .iter()
        .map(|due| {
            Json::object([
                ("scheduled", Json::text(due.scheduled.to_string())),
                ("due", Json::text(due.due.to_string())),
            ])
        })
        .collect();
    Json::object([
        ("zone", Json::text(result.zone.clone())),
        ("calendar_revision", Json::text(result.calendar_revision.clone())),
        ("dates", Json::List(dates)),
    ])
}

fn threshold_json(result: &Applicability) -> Json {
    let applies = match result.applies {
        Applies::Yes => "yes",
        Applies::No => "no",
        Applies::Unknown => "unknown",
    };
    let metrics = result
        .metrics
        .iter()
        .map(|metric| {
            Json::object([
                ("name", Json::text(metric.name.clone())),
                ("value", optional_int(metric.value)),
                ("nodes", Json::strings(&metric.nodes)),
            ])
        })
        .collect();
    Json::object([("applies", Json::text(applies)), ("metrics", Json::List(metrics))])
}

fn match_kind(kind: reconcile::MatchKind) -> &'static str {
    match kind {
        reconcile::MatchKind::OrderPayment => "order_payment",
        reconcile::MatchKind::PaymentPayout => "payment_payout",
    }
}

fn unmatched_reason(reason: reconcile::Reason) -> &'static str {
    match reason {
        reconcile::Reason::NoCounterpart => "no_counterpart",
        reconcile::Reason::OutsideWindow => "outside_window",
        reconcile::Reason::CurrencyMismatch => "currency_mismatch",
        reconcile::Reason::NoMatchWithinTolerance => "no_match_within_tolerance",
    }
}

fn reconcile_json(result: &Reconciliation) -> Json {
    let matches = result
        .matches
        .iter()
        .map(|found| {
            Json::object([
                ("kind", Json::text(match_kind(found.kind))),
                ("left", Json::strings(&found.left)),
                ("right", Json::strings(&found.right)),
                ("residual_minor", Json::int(found.residual.minor)),
                ("date_gap", Json::int(found.date_gap)),
            ])
        })
        .collect();
    let unmatched = result
        .unmatched
        .iter()
        .map(|node| {
            Json::object([
                ("kind", Json::text(match_kind(node.kind))),
                ("id", Json::text(node.id.clone())),
                ("reason", Json::text(unmatched_reason(node.reason))),
            ])
        })
        .collect();
    let ties = result
        .ties
        .iter()
        .map(|tie| {
            Json::object([
                ("kind", Json::text(match_kind(tie.kind))),
                ("anchor", Json::text(tie.anchor.clone())),
                ("chosen", Json::strings(&tie.chosen)),
                ("other", Json::strings(&tie.other)),
            ])
        })
        .collect();
    Json::object([
        ("matches", Json::List(matches)),
        ("unmatched", Json::List(unmatched)),
        ("ties", Json::List(ties)),
    ])
}

fn reorder_json(result: &ReorderResult) -> Json {
    Json::object([
        ("status", Json::text(result.status.as_str())),
        ("reorder", Json::Bool(result.reorder)),
        ("point", optional_int(result.point)),
        ("position", Json::int(result.position)),
        ("quantity", Json::int(result.quantity)),
        ("samples", Json::int(result.samples)),
    ])
}

fn phrase_json(result: &PhraseVerdict) -> Json {
    let forbidden = result
        .forbidden
        .iter()
        .map(|span| {
            Json::object([
                ("sentence", Json::int(span.sentence)),
                ("start", Json::int(span.start)),
                ("end", Json::int(span.end)),
                ("phrase", Json::text(span.phrase.clone())),
            ])
        })
        .collect();
    Json::object([
        ("verdict", Json::text(result.verdict.as_str())),
        ("forbidden", Json::List(forbidden)),
        ("unmatched", Json::strings(&result.unmatched)),
        ("policy_revision", Json::text(result.policy_revision.clone())),
    ])
}

fn invalid_parameters(detail: impl Into<String>) -> RuleRefusal {
    RuleRefusal::new(RULE_PARAMETERS_INVALID, detail)
}

fn wrong_inputs(kind: RuleKind) -> RuleRefusal {
    RuleRefusal::new(
        RULE_INPUT_INVALID,
        format!("the gathered rows are not the inputs of a `{}` rule", kind.as_str()),
    )
}

fn required<'a>(
    parameters: &'a BTreeMap<String, String>,
    name: &str,
) -> Result<&'a str, RuleRefusal> {
    parameters
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| invalid_parameters(format!("parameter `{name}` is required")))
}

fn whole<T: std::str::FromStr>(
    parameters: &BTreeMap<String, String>,
    name: &str,
) -> Result<T, RuleRefusal> {
    let text = required(parameters, name)?;
    text.parse::<T>()
        .map_err(|_| invalid_parameters(format!("parameter `{name}` is not an integer: `{text}`")))
}

/// The date written `YYYY-MM-DD`, or `None` when it is not one.
pub fn parse_date(text: &str) -> Option<Date> {
    let mut parts = text.split('-');
    let (year, month, day) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return None;
    }
    if !(year.bytes().chain(month.bytes()).chain(day.bytes())).all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Date::from_ymd(year.parse().ok()?, month.parse().ok()?, day.parse().ok()?)
}

fn parse_weekday(text: &str) -> Option<Weekday> {
    Some(match text {
        "mon" => Weekday::Monday,
        "tue" => Weekday::Tuesday,
        "wed" => Weekday::Wednesday,
        "thu" => Weekday::Thursday,
        "fri" => Weekday::Friday,
        "sat" => Weekday::Saturday,
        "sun" => Weekday::Sunday,
        _ => return None,
    })
}

fn parse_day(text: &str) -> Result<DaySelector, RuleRefusal> {
    if text == "last" {
        return Ok(DaySelector::LastDay);
    }
    if let Some((n, weekday)) = text.split_once(':') {
        let n = n
            .parse::<u8>()
            .ok()
            .filter(|n| (1..=5).contains(n))
            .ok_or_else(|| invalid_parameters(format!("day `{text}`: n is 1 to 5")))?;
        let weekday = parse_weekday(weekday)
            .ok_or_else(|| invalid_parameters(format!("day `{text}`: weekday is mon to sun")))?;
        return Ok(DaySelector::NthWeekday { n, weekday });
    }
    text.parse::<u8>()
        .ok()
        .filter(|day| (1..=31).contains(day))
        .map(DaySelector::DayOfMonth)
        .ok_or_else(|| invalid_parameters(format!("day `{text}` is 1 to 31, last or n:weekday")))
}

fn recurrence_rule(parameters: &BTreeMap<String, String>) -> Result<RecurrenceRule, RuleRefusal> {
    let frequency = match required(parameters, "frequency")? {
        "daily" => Frequency::Daily,
        "weekly" => Frequency::Weekly,
        "monthly" => Frequency::Monthly,
        "quarterly" => Frequency::Quarterly,
        "yearly" => Frequency::Yearly,
        other => {
            return Err(invalid_parameters(format!(
                "frequency `{other}` is not daily, weekly, monthly, quarterly or yearly"
            )))
        }
    };
    let interval = if parameters.contains_key("interval") {
        whole::<u32>(parameters, "interval")?
    } else {
        1
    };
    if interval == 0 {
        return Err(invalid_parameters("interval is at least 1"));
    }
    let adjustment = match required(parameters, "adjustment")? {
        "none" => Adjustment::None,
        "next_business_day" => Adjustment::NextBusinessDay,
        "previous_business_day" => Adjustment::PreviousBusinessDay,
        other => {
            return Err(invalid_parameters(format!(
                "adjustment `{other}` is not none, next_business_day or previous_business_day"
            )))
        }
    };
    let mut months = Vec::new();
    if let Some(list) = parameters.get("months") {
        for month in list.split(',') {
            let month = month
                .parse::<u8>()
                .ok()
                .filter(|month| (1..=12).contains(month))
                .ok_or_else(|| invalid_parameters(format!("month `{month}` is 1 to 12")))?;
            months.push(month);
        }
    }
    let anchor_text = required(parameters, "anchor")?;
    let anchor = parse_date(anchor_text)
        .ok_or_else(|| invalid_parameters(format!("anchor `{anchor_text}` is not YYYY-MM-DD")))?;
    Ok(RecurrenceRule {
        frequency,
        interval,
        day: parse_day(required(parameters, "day")?)?,
        months,
        adjustment,
        zone: required(parameters, "zone")?.to_string(),
        anchor,
    })
}

fn parse_comparison(text: &str) -> Option<Comparison> {
    Some(match text {
        "<" => Comparison::Less,
        "<=" => Comparison::LessOrEqual,
        ">" => Comparison::Greater,
        ">=" => Comparison::GreaterOrEqual,
        _ => return None,
    })
}

fn parse_atom(text: &str) -> Result<Expr, RuleRefusal> {
    let bad = || invalid_parameters(format!("expression atom `{text}` is not a comparison"));
    let words: Vec<&str> = text.split(' ').collect();
    let int = |word: &str| word.parse::<i64>().map_err(|_| bad());
    match words.as_slice() {
        [metric, "between", low, high] if !metric.is_empty() => Ok(Expr::Between {
            metric: (*metric).to_string(),
            low: int(low)?,
            high: int(high)?,
        }),
        [metric, op, value] if !metric.is_empty() => Ok(Expr::Compare {
            metric: (*metric).to_string(),
            op: parse_comparison(op).ok_or_else(bad)?,
            value: int(value)?,
        }),
        _ => Err(bad()),
    }
}

fn fold_left(
    parts: impl Iterator<Item = Result<Expr, RuleRefusal>>,
    join: fn(Box<Expr>, Box<Expr>) -> Expr,
) -> Result<Expr, RuleRefusal> {
    let mut parts = parts;
    let mut folded = parts
        .next()
        .ok_or_else(|| invalid_parameters("the expression is empty"))??;
    for next in parts {
        folded = join(Box::new(folded), Box::new(next?));
    }
    Ok(folded)
}

/// `expr := conj { " or " conj }`, `conj := atom { " and " atom }`, folded left.
fn parse_expression(text: &str) -> Result<Expr, RuleRefusal> {
    fold_left(
        text.split(" or ").map(|conj| {
            fold_left(conj.split(" and ").map(parse_atom), Expr::And)
        }),
        Expr::Or,
    )
}

fn parse_metrics(text: &str) -> Result<Vec<MetricDef>, RuleRefusal> {
    text.split(',')
        .map(|metric| {
            let parts: Vec<&str> = metric.split(':').collect();
            let [name, series, aggregate] = parts.as_slice() else {
                return Err(invalid_parameters(format!(
                    "metric `{metric}` is not name:series:agg"
                )));
            };
            let aggregate = match *aggregate {
                "sum" => Aggregate::Sum,
                "count" => Aggregate::Count,
                "max" => Aggregate::Max,
                other => {
                    return Err(invalid_parameters(format!(
                        "aggregate `{other}` is not sum, count or max"
                    )))
                }
            };
            Ok(MetricDef {
                name: (*name).to_string(),
                series: (*series).to_string(),
                aggregate,
            })
        })
        .collect()
}

fn reconcile_params(parameters: &BTreeMap<String, String>) -> Result<Params, RuleRefusal> {
    let currency = match required(parameters, "currency")? {
        "same" => CurrencyRule::SameCurrency,
        code if code.len() == 3 && code.bytes().all(|byte| byte.is_ascii_alphabetic()) => {
            CurrencyRule::Only(code.to_string())
        }
        other => {
            return Err(invalid_parameters(format!(
                "currency `{other}` is not `same` or a three-letter code"
            )))
        }
    };
    Ok(Params {
        tolerance_minor: whole(parameters, "tolerance_minor")?,
        window_days: whole(parameters, "window_days")?,
        currency,
    })
}

fn reorder_policy(parameters: &BTreeMap<String, String>) -> Result<reorder::Policy, RuleRefusal> {
    Ok(reorder::Policy {
        lead_time_days: whole(parameters, "lead_time_days")?,
        safety_stock: whole(parameters, "safety_stock")?,
        minimum_order_quantity: whole(parameters, "minimum_order_quantity")?,
        review_window_days: whole(parameters, "review_window_days")?,
        minimum_samples: whole(parameters, "minimum_samples")?,
    })
}

/// Evaluate `spec` over `inputs` at `as_of_ms` (epoch milliseconds). The as-of day is
/// `as_of_ms.div_euclid(86_400_000)`. One exhaustive `match` on the kind: each arm checks
/// that the gathered inputs are its own, parses the parameters of the record, and calls the
/// kind's function with a trace that carries the rule digest.
pub fn evaluate_rule(
    spec: &RuleSpec,
    inputs: RuleInputs,
    as_of_ms: i64,
) -> Result<RuleResult, RuleRefusal> {
    let as_of_day = as_of_ms.div_euclid(DAY_MS);
    let parameters = &spec.parameters;
    match spec.kind {
        RuleKind::Recurrence => {
            let RuleInputs::Recurrence { calendar, calendar_node, count } = inputs else {
                return Err(wrong_inputs(spec.kind));
            };
            let rule = recurrence_rule(parameters)?;
            let trace = RuleTrace::new(spec, calendar_node.into_iter().collect());
            recurrence::next_due(&rule, &calendar, Date::from_epoch_days(as_of_day), count, trace)
                .map(RuleResult::Recurrence)
        }
        RuleKind::Threshold => {
            let RuleInputs::Threshold { points } = inputs else {
                return Err(wrong_inputs(spec.kind));
            };
            let expr = parse_expression(required(parameters, "expr")?)?;
            let metrics = parse_metrics(required(parameters, "metrics")?)?;
            let period_days: u32 = whole(parameters, "period_days")?;
            let trace = RuleTrace::new(spec, Vec::new());
            threshold::applies(&expr, &metrics, &points, period_days, as_of_day, trace)
                .map(RuleResult::Threshold)
        }
        RuleKind::Reconcile => {
            let RuleInputs::Reconcile { sets } = inputs else {
                return Err(wrong_inputs(spec.kind));
            };
            let params = reconcile_params(parameters)?;
            let trace = RuleTrace::new(spec, Vec::new());
            reconcile::match_sets(&sets, &params, trace).map(RuleResult::Reconcile)
        }
        RuleKind::Reorder => {
            let RuleInputs::Reorder { sales, stock, open_lines } = inputs else {
                return Err(wrong_inputs(spec.kind));
            };
            let policy = reorder_policy(parameters)?;
            let trace = RuleTrace::new(spec, Vec::new());
            reorder::reorder_point(&policy, &sales, &stock, &open_lines, as_of_day, trace)
                .map(RuleResult::Reorder)
        }
        RuleKind::PhrasePolicy => {
            let RuleInputs::PhrasePolicy { text, locale, policy, policy_node } = inputs else {
                return Err(wrong_inputs(spec.kind));
            };
            let trace = RuleTrace::new(spec, vec![policy_node]);
            phrase_policy::check_text(&text, &locale, &policy, trace)
                .map(RuleResult::PhrasePolicy)
        }
    }
}

/// The refusal for an evaluation with no as-of time.
pub fn asof_required() -> RuleRefusal {
    RuleRefusal::new(ASOF_REQUIRED, "EVALUATE RULE takes an explicit ASOF in epoch milliseconds")
}
