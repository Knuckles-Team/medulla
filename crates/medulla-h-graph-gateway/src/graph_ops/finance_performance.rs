//! `Method::FinancePerformance` (MEDULLA-FINANCE-PRIMITIVES-R005.3): one account's return over a
//! range by time-weighted, money-weighted or annualized method, with an optional benchmark over
//! the same window.
//!
//! The stored records are `Activity` nodes of four kinds (`valuation`, `deposit`, `withdrawal`
//! and `benchmark_level`, R005.3.2); the window is resolved by [`window_start_ns`] (R005.3.3);
//! the figure comes from `medulla_finance::returns` (R005.3.1) and is admitted by
//! `check_figure_provenance`. A figure that cannot be computed is null with a reason, never
//! zero. The method reads only.

use std::ops::ControlFlow;

use super::finance_accounting::{
    account_of, datetime_ns, invalid, literal, refuse, tenant_events, Event,
};
use super::ops_context::GraphOpsContext;
use super::*;

use medulla_finance::golden_fixture::{
    check_figure_provenance, ticks_to_decimal, GoldenFixtureFigure,
};
use medulla_finance::lot_accounting::SCALE;
use medulla_finance::returns::{
    mwr_ticks, twr_ticks, xirr_ticks, ReturnError, ReturnFlow, ValuationPoint,
};
use medulla_finance_market::market::calendar::{civil_from_days, days_from_civil};
use medulla_types::finance_accounting::{
    BenchmarkFigure, FinancePerformanceCard, ACCOUNT_BINDING_UNVERIFIED, BENCHMARK_UNKNOWN,
    FIGURE_PROVENANCE_MISSING, RANGE_UNSUPPORTED,
};
use sha2::{Digest, Sha256};

const NS_PER_DAY: i64 = 86_400 * 1_000_000_000;
/// A figure whose end valuation is older than this is stale.
pub const PERFORMANCE_STALE_NS: i64 = 5 * NS_PER_DAY;
const YEAR_NS: f64 = 365.0 * 86_400.0 * 1e9;

/// Whether `kind` is one of the four record kinds the return engine reads.
pub fn is_performance_kind(kind: &str) -> bool {
    matches!(
        kind,
        "valuation" | "deposit" | "withdrawal" | "benchmark_level"
    )
}

/// The eligible records of `account`: those with `ns <= as_of` and `known_ns <= known_at`
/// (the cut `project` applies), of the verified tenant, ascending.
fn eligible_events(
    core: &GraphCore,
    scope: &str,
    account: &str,
    as_ns: i64,
    known_ns: i64,
) -> Vec<Event> {
    tenant_events(core, scope)
        .into_iter()
        .filter(|e| account_of(e).as_deref() == Some(account))
        .filter(|e| e.ns <= as_ns && e.known_ns <= known_ns)
        .collect()
}

/// The `valuation` records as points, ascending by `ns`. A record in another currency than
/// `currency` refuses.
fn valuation_series(events: &[Event], currency: &str) -> Result<Vec<ValuationPoint>, String> {
    let mut points = Vec::new();
    for event in events.iter().filter(|e| e.kind == "valuation") {
        check_currency(event, currency, "valuation")?;
        let value_ticks = event.ticks("activityMarketValue").ok_or_else(|| {
            format!(
                "INVALID_ARGUMENT: valuation {} has no activityMarketValue decimal",
                event.record_id
            )
        })?;
        points.push(ValuationPoint {
            ns: event.ns,
            value_ticks,
        });
    }
    points.sort_by_key(|p| p.ns);
    Ok(points)
}

/// The `deposit` and `withdrawal` records as flows (a withdrawal is negative), ascending.
fn flow_series(events: &[Event], currency: &str) -> Result<Vec<ReturnFlow>, String> {
    let mut flows = Vec::new();
    for event in events
        .iter()
        .filter(|e| e.kind == "deposit" || e.kind == "withdrawal")
    {
        check_currency(event, currency, &event.kind)?;
        let amount = event.ticks("activityAmount").ok_or_else(|| {
            format!(
                "INVALID_ARGUMENT: {} {} has no activityAmount decimal",
                event.kind, event.record_id
            )
        })?;
        flows.push(ReturnFlow {
            ns: event.ns,
            amount_ticks: if event.kind == "withdrawal" {
                -amount
            } else {
                amount
            },
        });
    }
    flows.sort_by_key(|f| f.ns);
    Ok(flows)
}

fn check_currency(event: &Event, currency: &str, kind: &str) -> Result<(), String> {
    match event.text("activityCurrency") {
        Some(found) if found == currency => Ok(()),
        found => Err(format!(
            "INVALID_ARGUMENT: {kind} {} is in {}, not {currency}",
            event.record_id,
            found.unwrap_or_default()
        )),
    }
}

/// The `benchmark_level` records named `name` across the tenant that were known at `known_ns`
/// (the cut every other record reader applies), ascending.
fn benchmark_events(core: &GraphCore, scope: &str, name: &str, known_ns: i64) -> Vec<Event> {
    tenant_events(core, scope)
        .into_iter()
        .filter(|e| e.kind == "benchmark_level" && e.position == name && e.known_ns <= known_ns)
        .collect()
}

/// `(ns, level_ticks)` of the benchmark `name` known at `known_ns`, ascending.
#[cfg(test)]
fn benchmark_levels(core: &GraphCore, scope: &str, name: &str, known_ns: i64) -> Vec<(i64, i64)> {
    levels_of(&benchmark_events(core, scope, name, known_ns))
}

fn levels_of(events: &[Event]) -> Vec<(i64, i64)> {
    let mut levels: Vec<(i64, i64)> = events
        .iter()
        .filter_map(|e| Some((e.ns, e.ticks("activityPrice")?)))
        .collect();
    levels.sort();
    levels
}

/// The window's start instant for `range` at `as_of_ns`. `1M`, `1Y` and `5Y` step back on the
/// UTC calendar date, clamping the day to the target month and keeping the time of day.
fn window_start_ns(
    range: &str,
    as_of_ns: i64,
    first_valuation_ns: Option<i64>,
) -> Result<i64, String> {
    let days = as_of_ns.div_euclid(NS_PER_DAY);
    let time_of_day = as_of_ns.rem_euclid(NS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    let months_back = |back: i64| -> i64 {
        let index = year * 12 + (month - 1) - back;
        let (target_year, target_month) = (index.div_euclid(12), index.rem_euclid(12) + 1);
        let last_day = civil_from_days(
            days_from_civil(
                target_year + i64::from(target_month == 12),
                target_month % 12 + 1,
                1,
            ) - 1,
        )
        .2;
        days_from_civil(target_year, target_month, day.min(last_day)) * NS_PER_DAY + time_of_day
    };
    match range {
        "1D" => Ok(as_of_ns - NS_PER_DAY),
        "1M" => Ok(months_back(1)),
        "1Y" => Ok(months_back(12)),
        "5Y" => Ok(months_back(60)),
        "YTD" => Ok(days_from_civil(year, 1, 1) * NS_PER_DAY),
        "ALL" => first_valuation_ns.ok_or_else(|| "NO_VALUATION: the series is empty".to_string()),
        other => Err(format!("{RANGE_UNSUPPORTED}: {other}")),
    }
}

/// The benchmark's return over the same window: its latest level at or before the start and at
/// or before the end.
fn benchmark_figure(
    name: &str,
    method: &str,
    levels: &[(i64, i64)],
    start_ns: i64,
    end_ns: i64,
) -> BenchmarkFigure {
    let level_at = |ns: i64| levels.iter().rev().find(|(at, _)| *at <= ns).map(|l| l.1);
    let unavailable = |reason: &str| BenchmarkFigure {
        name: name.to_string(),
        value_ticks: None,
        value: None,
        reason: Some(reason.to_string()),
    };
    let (Some(start), Some(end)) = (level_at(start_ns), level_at(end_ns)) else {
        return unavailable("NO_BENCHMARK_LEVEL");
    };
    if start <= 0 {
        return unavailable("NO_BENCHMARK_LEVEL");
    }
    let ticks = if method == "xirr" {
        let years = (end_ns - start_ns) as f64 / YEAR_NS;
        let rate = (end as f64 / start as f64).powf(1.0 / years) - 1.0;
        let scaled = (rate * SCALE as f64).round();
        if !scaled.is_finite() || scaled.abs() >= i64::MAX as f64 {
            return unavailable("NO_SOLUTION");
        }
        scaled as i64
    } else {
        let scale = i128::from(SCALE);
        let growth =
            (2 * i128::from(end) * scale + i128::from(start)).div_euclid(2 * i128::from(start));
        match i64::try_from(growth - scale) {
            Ok(ticks) => ticks,
            Err(_) => return unavailable("NO_SOLUTION"),
        }
    };
    BenchmarkFigure {
        name: name.to_string(),
        value_ticks: Some(ticks),
        value: Some(ticks_to_decimal(ticks)),
        reason: None,
    }
}

/// The request of one performance figure.
struct Request<'a> {
    account: &'a str,
    as_of: &'a str,
    known_at: &'a str,
    base_currency: &'a str,
    method: &'a str,
    range: &'a str,
    as_ns: i64,
}

/// Build the card from the account's eligible records and the benchmark's records.
fn build_card(
    request: &Request<'_>,
    events: &[Event],
    benchmark: Option<(&str, &[Event])>,
) -> Result<FinancePerformanceCard, String> {
    let valuations = valuation_series(events, request.base_currency)?;
    let flows = flow_series(events, request.base_currency)?;
    let mut card = FinancePerformanceCard {
        account: request.account.to_string(),
        method: request.method.to_string(),
        currency: request.base_currency.to_string(),
        range: request.range.to_string(),
        as_of: request.as_of.to_string(),
        known_at: request.known_at.to_string(),
        value_ticks: None,
        value: None,
        reason: None,
        benchmark: None,
        source: String::new(),
        session: String::new(),
        stale: false,
        scale: SCALE,
        input_digest: String::new(),
    };
    let mut digest = Sha256::new();
    digest.update(format!(
        "{}|{}|{}|{}|{}|{}\n",
        request.account,
        request.as_of,
        request.known_at,
        request.base_currency,
        request.method,
        request.range
    ));
    let unavailable = |mut card: FinancePerformanceCard, reason: &str| {
        card.reason = Some(reason.to_string());
        card
    };
    let Some(end) = valuations.last().copied() else {
        card.input_digest = hex::encode(digest.finalize());
        return Ok(unavailable(card, "NO_VALUATION"));
    };
    let start_ns = window_start_ns(
        request.range,
        request.as_ns,
        valuations.first().map(|p| p.ns),
    )?;
    let start = valuations.iter().rev().find(|p| p.ns <= start_ns).copied();
    let Some(start) = start else {
        card.input_digest = hex::encode(digest.finalize());
        return Ok(unavailable(card, "NO_VALUATION_AT_START"));
    };

    let mut absorb = |event: &Event| {
        digest.update(event.node_id.as_bytes());
        for (name, value) in &event.fields {
            digest.update(format!("|{name}={value}"));
        }
        digest.update(b"\n");
    };

    let window_flows: Vec<ReturnFlow> = flows
        .iter()
        .filter(|f| f.ns > start.ns && f.ns <= end.ns)
        .copied()
        .collect();
    let end_record = events
        .iter()
        .rev()
        .find(|e| e.kind == "valuation" && e.ns == end.ns);
    for event in events.iter().filter(|e| is_performance_kind(&e.kind)) {
        absorb(event);
    }
    if let Some((_, levels)) = benchmark {
        for event in levels {
            absorb(event);
        }
    }
    card.source = end_record
        .and_then(|e| e.text("recordSource"))
        .unwrap_or_default();
    card.session = end_record
        .and_then(|e| e.text("activitySession"))
        .unwrap_or_default();
    card.stale = request.as_ns - end.ns > PERFORMANCE_STALE_NS;

    let result = match request.method {
        "twr" => {
            let mut points = vec![start];
            points.extend(
                valuations
                    .iter()
                    .filter(|p| p.ns > start.ns && p.ns < end.ns)
                    .copied(),
            );
            points.push(end);
            twr_ticks(&points, &window_flows)
        }
        "mwr" => mwr_ticks(start, end, &window_flows),
        _ => xirr_ticks(start, end, &window_flows),
    };
    match result {
        Ok(ticks) => {
            let golden = GoldenFixtureFigure {
                value_ticks: ticks,
                source: card.source.clone(),
                as_of: card.as_of.clone(),
                session: card.session.clone(),
            };
            if let Err(refusal) = check_figure_provenance(&golden) {
                return Err(format!("{FIGURE_PROVENANCE_MISSING}: {}", refusal.reason));
            }
            card.value_ticks = Some(ticks);
            card.value = Some(ticks_to_decimal(ticks));
        }
        Err(error) => {
            card.reason = Some(
                match error {
                    ReturnError::NoPeriod => "NO_VALUATION_AT_START",
                    ReturnError::NonPositiveStartValue => "NON_POSITIVE_START_VALUE",
                    ReturnError::NoSolution | ReturnError::Overflow => "NO_SOLUTION",
                }
                .to_string(),
            );
        }
    }
    if let Some((name, events)) = benchmark {
        card.benchmark = Some(benchmark_figure(
            name,
            request.method,
            &levels_of(events),
            start.ns,
            end.ns,
        ));
    }
    card.input_digest = hex::encode(digest.finalize());
    Ok(card)
}

#[allow(clippy::too_many_arguments)]
fn performance(
    ctx: GraphOpsContext<'_>,
    account: &str,
    as_of: &str,
    known_at: &str,
    base_currency: &str,
    method: &str,
    range: &str,
    benchmark: Option<&str>,
) -> Response {
    let (Some(as_ns), Some(known_ns)) = (datetime_ns(as_of), datetime_ns(known_at)) else {
        return invalid(ctx.req_id, "as_of and known_at must be RFC 3339 date-times");
    };
    if !matches!(method, "twr" | "mwr" | "xirr") {
        return invalid(
            ctx.req_id,
            format!("method '{method}' is not twr, mwr or xirr"),
        );
    }
    let Some(carrier) = ctx.read_authority.carrier() else {
        return refuse(ctx.req_id, "ACCESS_DENIED", "no verified tenant");
    };
    let scope = carrier.tenant_scope().to_owned();
    if let Err(refusal) = window_start_ns(range, as_ns, Some(as_ns)) {
        return Response::err(ctx.req_id, refusal);
    }
    let node_id = format!("finance:Account:{account}");
    let Some(props) = ctx
        .core
        .get_node_properties(&node_id)
        .and_then(|blob| medulla_types::msgpack::decode_property_object(&blob).ok())
    else {
        return invalid(ctx.req_id, format!("account {account} not found"));
    };
    let bound = props
        .get("fields")
        .and_then(|fields| fields.get("accountBindingState"))
        .and_then(|term| term.as_str())
        .and_then(literal)
        .is_some_and(|state| state == "verified");
    if !bound {
        return refuse(
            ctx.req_id,
            ACCOUNT_BINDING_UNVERIFIED,
            format!("account {account} is not bound to the verified principal"),
        );
    }
    let benchmark_records = match benchmark {
        Some(name) => {
            let records = benchmark_events(ctx.core, &scope, name, known_ns);
            if levels_of(&records).is_empty() {
                return refuse(
                    ctx.req_id,
                    BENCHMARK_UNKNOWN,
                    format!("no benchmark_level record names {name}"),
                );
            }
            Some((name, records))
        }
        None => None,
    };
    let events = eligible_events(ctx.core, &scope, account, as_ns, known_ns);
    let request = Request {
        account,
        as_of,
        known_at,
        base_currency,
        method,
        range,
        as_ns,
    };
    let benchmark_input = benchmark_records
        .as_ref()
        .map(|(name, records)| (*name, records.as_slice()));
    match build_card(&request, &events, benchmark_input) {
        Ok(card) => Response::ok(
            ctx.req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::graph::FinancePerformance,
            >(&card),
        ),
        Err(refusal) => Response::err(ctx.req_id, refusal),
    }
}

/// Route `FinancePerformance` from the graph terminal read chain.
pub async fn handle_performance(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    if let Method::FinancePerformance {
        account,
        as_of,
        known_at,
        base_currency,
        method: figure_method,
        range,
        benchmark,
    } = &method
    {
        return ControlFlow::Break(performance(
            ctx,
            account,
            as_of,
            known_at,
            base_currency,
            figure_method,
            range,
            benchmark.as_deref(),
        ));
    }
    ControlFlow::Continue(method)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const SCOPE: &str = "tenant-a";

    /// The fixed columns of one seeded activity record.
    struct Activity<'a> {
        scope: &'a str,
        account: &'a str,
        kind: &'a str,
        position: &'a str,
        date: &'a str,
    }

    fn seed(core: &GraphCore, id: &str, activity: &Activity<'_>, extra: &[(&str, &str)]) {
        let Activity {
            scope,
            account,
            kind,
            position,
            date,
        } = *activity;
        let mut fields: BTreeMap<&str, String> = BTreeMap::new();
        fields.insert("activityKind", format!("\"{kind}\""));
        fields.insert(
            "activityPosition",
            format!("<http://example.org/m#{position}>"),
        );
        fields.insert(
            "activityAccount",
            format!("<http://example.org/m#{account}>"),
        );
        fields.insert("activityTradeDate", format!("\"{date}\"^^xsd:dateTime"));
        fields.insert("recordSource", "\"broker\"".to_string());
        fields.insert("activitySession", "\"regular\"".to_string());
        for (name, value) in extra {
            fields.insert(name, (*value).to_string());
        }
        let properties = serde_json::json!({
            "type": "Activity",
            "finance_tenant_scope": scope,
            "record_id": id,
            "fields": fields,
        });
        core.add_node(
            id.to_string(),
            rmp_serde::to_vec_named(&properties).expect("encode record"),
        );
    }

    fn ns(text: &str) -> i64 {
        datetime_ns(text).expect("a date-time")
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R005.3.2
    #[test]
    fn series_read_only_eligible_records_in_one_currency() {
        let core = GraphCore::new();
        let value = |v: &str| ("activityMarketValue", format!("\"{v}\"^^xsd:decimal"));
        let put = |id: &str, scope: &str, kind: &str, date: &str, v: &str, known: Option<&str>| {
            let mut extra: Vec<(&str, String)> = vec![value(v)];
            extra.push(("activityCurrency", "\"USD\"".to_string()));
            if let Some(known) = known {
                extra.push(("recordKnownAt", format!("\"{known}\"^^xsd:dateTime")));
            }
            let extra: Vec<(&str, &str)> = extra.iter().map(|(a, b)| (*a, b.as_str())).collect();
            let activity = Activity {
                scope,
                account: "acct",
                kind,
                position: "account-value",
                date,
            };
            seed(&core, id, &activity, &extra);
        };
        put(
            "v1",
            SCOPE,
            "valuation",
            "2026-01-02T00:00:00Z",
            "100",
            None,
        );
        put(
            "v2",
            SCOPE,
            "valuation",
            "2026-02-01T00:00:00Z",
            "110",
            None,
        );
        put(
            "late",
            SCOPE,
            "valuation",
            "2026-04-01T00:00:00Z",
            "999",
            None,
        );
        put(
            "known-late",
            SCOPE,
            "valuation",
            "2026-01-20T00:00:00Z",
            "998",
            Some("2026-03-01T00:00:00Z"),
        );
        put(
            "other",
            "tenant-b",
            "valuation",
            "2026-01-10T00:00:00Z",
            "997",
            None,
        );
        let as_ns = ns("2026-03-01T00:00:00Z");
        let events = eligible_events(&core, SCOPE, "acct", as_ns, as_ns);
        let series = valuation_series(&events, "USD").expect("one currency");
        assert_eq!(
            series
                .iter()
                .map(|p| p.value_ticks / SCALE)
                .collect::<Vec<_>>(),
            vec![100, 110],
            "after as_of, known after known_at and another tenant's records are excluded"
        );
        let refusal = valuation_series(&events, "EUR").unwrap_err();
        assert_eq!(refusal, "INVALID_ARGUMENT: valuation v1 is in USD, not EUR");
        assert!(flow_series(&events, "USD").unwrap().is_empty());
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R005.3.3
    #[test]
    fn benchmark_levels_apply_the_known_at_cut() {
        let core = GraphCore::new();
        let put = |id: &str, date: &str, level: &str, known: Option<&str>| {
            let mut extra = vec![("activityPrice", format!("\"{level}\"^^xsd:decimal"))];
            if let Some(known) = known {
                extra.push(("recordKnownAt", format!("\"{known}\"^^xsd:dateTime")));
            }
            let extra: Vec<(&str, &str)> = extra.iter().map(|(a, b)| (*a, b.as_str())).collect();
            let activity = Activity {
                scope: SCOPE,
                account: "account-benchmark",
                kind: "benchmark_level",
                position: "spx",
                date,
            };
            seed(&core, id, &activity, &extra);
        };
        put("b1", "2026-01-02T00:00:00Z", "100", None);
        put(
            "b2",
            "2026-02-01T00:00:00Z",
            "110",
            Some("2026-05-01T00:00:00Z"),
        );
        let known = ns("2026-03-01T00:00:00Z");
        assert_eq!(
            benchmark_levels(&core, SCOPE, "spx", known),
            vec![(ns("2026-01-02T00:00:00Z"), 100 * SCALE)],
            "a level recorded after known_at is invisible"
        );
        assert_eq!(
            benchmark_levels(&core, SCOPE, "spx", ns("2026-06-01T00:00:00Z")).len(),
            2
        );
    }

    // spec: MEDULLA-FINANCE-PRIMITIVES-R005.3.3
    #[test]
    fn ranges_window_the_series_and_clamp_month_ends() {
        let as_of = ns("2026-03-31T12:00:00Z");
        assert_eq!(
            window_start_ns("1M", as_of, None),
            Ok(ns("2026-02-28T12:00:00Z"))
        );
        assert_eq!(
            window_start_ns("YTD", as_of, None),
            Ok(ns("2026-01-01T00:00:00Z"))
        );
        assert_eq!(
            window_start_ns("1Y", as_of, None),
            Ok(ns("2025-03-31T12:00:00Z"))
        );
        assert_eq!(
            window_start_ns("5Y", as_of, None),
            Ok(ns("2021-03-31T12:00:00Z"))
        );
        assert_eq!(
            window_start_ns("1D", as_of, None),
            Ok(ns("2026-03-30T12:00:00Z"))
        );
        assert_eq!(window_start_ns("ALL", as_of, Some(7)), Ok(7));
        assert!(window_start_ns("ALL", as_of, None).is_err());
        assert_eq!(
            window_start_ns("2W", as_of, None),
            Err("RANGE_UNSUPPORTED: 2W".to_string())
        );
    }
}
