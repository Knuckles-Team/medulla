//! `Method::FinanceAccountProject` (MEDULLA-FINANCE-PRIMITIVES-R013.2.6): the served
//! accounting projection of one account at `(as_of, known_at)`.
//!
//! A terminal read over the RLS-projected core. The handler reads the account's
//! `Activity` records of the verified tenant, resolves what was known at
//! `known_at`, replays buys and sells through `lot_accounting` (FIFO or LIFO),
//! applies the resolved corporate actions (`corporate_actions::resolve`; splits
//! through `apply_split`, dividends as cash on the open quantity), joins the last
//! traded price as valuation, and rolls futures through `futures_roll`. Every
//! output is wrapped in a `GoldenFixtureFigure` and admitted by
//! `check_figure_provenance` before it is returned. Nothing is written.
//!
//! Figure provenance: `source` is the `recordSource` of the last contributing
//! record; `session` is its `activitySession` or, when the account carries an
//! `accountCalendar` (an `ExchangeCalendar` as JSON), the `session_at` label of
//! that record's instant; `as_of` is the request's. At one instant a corporate
//! action applies before a trade.

use super::terminal::GraphOpsContext;
use super::*;

use std::collections::BTreeMap;

use medulla_finance::futures_roll::{roll_position, ContractSpec};
use medulla_finance::golden_fixture::{check_figure_provenance, GoldenFixtureFigure};
use medulla_finance::lot_accounting::{
    apply_activities_from, apply_split, basis_numerator, total_quantity_ticks, LotActivity,
    LotLedger, LotMethod, SCALE,
};
use medulla_finance_market::market::calendar::{days_from_civil, session_at};
use medulla_finance_market::market::corporate_actions::resolve;
use medulla_finance_market::market::{
    CorporateAction, CorporateActionKind, ExchangeCalendar, TradingCalendar,
};
use medulla_types::finance_accounting::{
    FinanceAccountProjection, ProjectedFigure, BASIS_POLICY_UNSUPPORTED, FIGURE_PROVENANCE_MISSING,
};
use sha2::{Digest, Sha256};

const TENANT_SCOPE_KEY: &str = "finance_tenant_scope";
const ACTIVITY_LABEL: &str = "Activity";
const PAGE: usize = 1024;
const NS_PER_SECOND: i64 = 1_000_000_000;

fn refuse(req_id: u64, code: &str, detail: impl std::fmt::Display) -> Response {
    Response::err(req_id, format!("{code}: {detail}"))
}

fn invalid(req_id: u64, detail: impl std::fmt::Display) -> Response {
    refuse(req_id, "INVALID_ARGUMENT", detail)
}

/// The text of a Turtle string or typed literal (`"x"` or `"x"^^xsd:t`), with
/// `\"` and `\\` unescaped.
fn literal(term: &str) -> Option<String> {
    let body = term.strip_prefix('"')?;
    let end = body.rfind('"')?;
    Some(body[..end].replace("\\\"", "\"").replace("\\\\", "\\"))
}

/// The local name of an IRI term (`<http://x#name>` gives `name`).
fn iri_local(term: &str) -> Option<String> {
    let inner = term.strip_prefix('<')?.strip_suffix('>')?;
    inner
        .rsplit(['#', '/'])
        .next()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// A decimal literal as fixed-point ticks at `SCALE` (at most 8 fraction digits).
fn decimal_ticks(text: &str) -> Option<i64> {
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    if whole.is_empty() || fraction.len() > 8 {
        return None;
    }
    if !whole
        .chars()
        .chain(fraction.chars())
        .all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let whole: i128 = whole.parse().ok()?;
    let padded = format!("{fraction:0<8}");
    let fraction: i128 = padded.parse().ok()?;
    let ticks = whole.checked_mul(SCALE as i128)?.checked_add(fraction)?;
    i64::try_from(if negative { -ticks } else { ticks }).ok()
}

/// `YYYY-MM-DDTHH:MM:SS[.fraction](Z|+HH:MM|-HH:MM)` as UTC nanoseconds.
fn datetime_ns(text: &str) -> Option<i64> {
    let (date, time) = text.split_once('T')?;
    let mut d = date.split('-');
    let (year, month, day): (i64, i64, i64) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    let (clock, offset_minutes) = if let Some(clock) = time.strip_suffix('Z') {
        (clock, 0)
    } else {
        let at = time.rfind(['+', '-'])?;
        let (clock, zone) = time.split_at(at);
        let sign = if zone.starts_with('-') { -1 } else { 1 };
        let (h, m) = zone[1..].split_once(':')?;
        (
            clock,
            sign * (h.parse::<i64>().ok()? * 60 + m.parse::<i64>().ok()?),
        )
    };
    let (hms, fraction) = clock.split_once('.').unwrap_or((clock, ""));
    let mut t = hms.split(':');
    let (hour, minute, second): (i64, i64, i64) = (
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
    );
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let nanos: i64 = if fraction.is_empty() {
        0
    } else if fraction.len() <= 9 && fraction.chars().all(|c| c.is_ascii_digit()) {
        format!("{fraction:0<9}").parse().ok()?
    } else {
        return None;
    };
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second
        - offset_minutes * 60;
    seconds.checked_mul(NS_PER_SECOND)?.checked_add(nanos)
}

/// One stored `Activity` record, decoded.
struct Event {
    node_id: String,
    record_id: String,
    fields: BTreeMap<String, String>,
    position: String,
    kind: String,
    ns: i64,
    known_ns: i64,
}

impl Event {
    fn text(&self, name: &str) -> Option<String> {
        self.fields.get(name).and_then(|term| literal(term))
    }

    fn ticks(&self, name: &str) -> Option<i64> {
        self.text(name).and_then(|text| decimal_ticks(&text))
    }

    fn whole(&self, name: &str) -> Option<i64> {
        self.text(name).and_then(|text| text.parse().ok())
    }

    fn instant(&self, name: &str) -> Option<i64> {
        self.text(name).and_then(|text| datetime_ns(&text))
    }
}

fn decode(node_id: &str, blob: &[u8], scope: &str) -> Option<Event> {
    let props = medulla_types::msgpack::decode_property_object(blob).ok()?;
    if props.get(TENANT_SCOPE_KEY).and_then(|v| v.as_str()) != Some(scope) {
        return None;
    }
    let fields: BTreeMap<String, String> = props
        .get("fields")?
        .as_object()?
        .iter()
        .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_owned())))
        .collect();
    let mut event = Event {
        node_id: node_id.to_owned(),
        record_id: props.get("record_id")?.as_str()?.to_owned(),
        position: iri_local(fields.get("activityPosition")?)?,
        kind: literal(fields.get("activityKind")?)?,
        ns: 0,
        known_ns: 0,
        fields,
    };
    event.ns = event.instant("activityTradeDate")?;
    event.known_ns = event.instant("recordKnownAt").unwrap_or(event.ns);
    Some(event)
}

fn account_of(event: &Event) -> Option<String> {
    event
        .fields
        .get("activityAccount")
        .and_then(|term| iri_local(term))
}

/// Every `Activity` of the verified tenant belonging to `account`, by record id.
fn activities(core: &GraphCore, scope: &str, account: &str) -> Vec<Event> {
    let mut out = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let page = core.get_nodes_by_label_page(ACTIVITY_LABEL, after.as_deref(), PAGE);
        let full = page.len() == PAGE;
        after = page.last().map(|(id, _)| id.clone());
        out.extend(page.iter().filter_map(|(id, blob)| {
            decode(id, blob, scope).filter(|e| account_of(e).as_deref() == Some(account))
        }));
        if !full || after.is_none() {
            break;
        }
    }
    out.sort_by(|a, b| (a.ns, &a.record_id).cmp(&(b.ns, &b.record_id)));
    out
}

/// The account's exchange calendar, when the account record carries one.
fn account_calendar(core: &GraphCore, account: &str) -> Result<Option<TradingCalendar>, String> {
    let node_id = format!("finance:Account:{account}");
    let Some(props) = core
        .get_node_properties(&node_id)
        .and_then(|blob| medulla_types::msgpack::decode_property_object(&blob).ok())
    else {
        return Err(format!("account {account} not found"));
    };
    let Some(text) = props
        .get("fields")
        .and_then(|fields| fields.get("accountCalendar"))
        .and_then(|term| term.as_str())
        .and_then(literal)
    else {
        return Ok(None);
    };
    let calendar: ExchangeCalendar = serde_json::from_str(&text)
        .map_err(|error| format!("accountCalendar is not an exchange calendar: {error}"))?;
    Ok(Some(TradingCalendar::Exchange { calendar }))
}

/// Provenance of the last record that contributed to a position's figures.
#[derive(Clone, Default)]
struct Provenance {
    source: String,
    session: String,
}

fn provenance(event: &Event, calendar: Option<&TradingCalendar>) -> Provenance {
    let session = event.text("activitySession").unwrap_or_else(|| {
        calendar
            .and_then(|c| serde_json::to_value(session_at(c, event.ns)).ok())
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    });
    Provenance {
        source: event.text("recordSource").unwrap_or_default(),
        session,
    }
}

fn half_up(numerator: i128, divisor: i128) -> i128 {
    if numerator >= 0 {
        (numerator + divisor / 2) / divisor
    } else {
        -((-numerator + divisor / 2) / divisor)
    }
}

fn to_i64(value: i128) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| "figure overflowed fixed-point ticks".to_owned())
}

struct Projector<'a> {
    as_of: &'a str,
    base_currency: &'a str,
    method: LotMethod,
    calendar: Option<TradingCalendar>,
    as_ns: i64,
    known_ns: i64,
    figures: Vec<ProjectedFigure>,
}

impl Projector<'_> {
    fn push(&mut self, position: &str, name: &str, value_ticks: i64, prov: &Provenance) {
        self.figures.push(ProjectedFigure {
            position: position.to_owned(),
            name: name.to_owned(),
            value_ticks,
            source: prov.source.clone(),
            as_of: self.as_of.to_owned(),
            session: prov.session.clone(),
        });
    }

    fn position(&mut self, position: &str, events: &[&Event]) -> Result<(), String> {
        let mut actions = Vec::new();
        for event in events {
            let kind = match event.kind.as_str() {
                "split" => CorporateActionKind::Split {
                    from: u32::try_from(
                        event
                            .whole("activitySplitFrom")
                            .ok_or("split without activitySplitFrom")?,
                    )
                    .map_err(|_| "activitySplitFrom out of range")?,
                    to: u32::try_from(
                        event
                            .whole("activitySplitTo")
                            .ok_or("split without activitySplitTo")?,
                    )
                    .map_err(|_| "activitySplitTo out of range")?,
                },
                "dividend" => CorporateActionKind::Dividend {
                    amount_ticks: event
                        .ticks("activityDividendPerShare")
                        .ok_or("dividend without activityDividendPerShare")?,
                },
                _ => continue,
            };
            actions.push(CorporateAction {
                listing_id: position.to_owned(),
                action: kind,
                effective_time: event.ns,
                known_at: event.known_ns,
                source: event.text("recordSource").unwrap_or_default(),
                revision: u32::try_from(event.whole("activityRevision").unwrap_or(0))
                    .map_err(|_| "activityRevision out of range")?,
            });
        }
        let actions: Vec<CorporateAction> = resolve(&actions, Some(self.known_ns))
            .map_err(|error| format!("{}: {}", error.code, error.detail))?
            .into_iter()
            .filter(|action| action.effective_time <= self.as_ns)
            .collect();
        let source_of = |ns: i64| {
            events
                .iter()
                .rev()
                .find(|e| e.ns == ns && matches!(e.kind.as_str(), "split" | "dividend"))
        };

        let mut ledger = LotLedger::default();
        let mut pending: Vec<LotActivity> = Vec::new();
        let mut last_price: Option<i64> = None;
        let mut dividend_cash: i128 = 0;
        let mut last: Option<&Event> = None;
        let mut sequence = 0u64;
        let mut trades: Vec<&Event> = events
            .iter()
            .copied()
            .filter(|e| matches!(e.kind.as_str(), "buy" | "sell"))
            .collect();
        trades.reverse();
        let mut action_iter = actions.iter().peekable();
        loop {
            let next_trade_ns = trades.last().map(|e| e.ns);
            let next_action_ns = action_iter.peek().map(|a| a.effective_time);
            let take_action = match (next_action_ns, next_trade_ns) {
                (Some(a), Some(t)) => a <= t,
                (Some(_), None) => true,
                (None, _) => false,
            };
            if take_action {
                let action = action_iter.next().expect("peeked");
                ledger = apply_activities_from(ledger, &std::mem::take(&mut pending), self.method)
                    .map_err(|e| e.to_string())?;
                match &action.action {
                    CorporateActionKind::Split { from, to } => {
                        let outcome = apply_split(&ledger.open, u64::from(*to), u64::from(*from))
                            .map_err(|e| e.to_string())?;
                        ledger.open = outcome.lots;
                        // The last traded price is quoted per pre-split share.
                        last_price = last_price.map(|price| {
                            half_up(i128::from(price) * i128::from(*from), i128::from(*to)) as i64
                        });
                    }
                    CorporateActionKind::Dividend { amount_ticks } => {
                        dividend_cash += half_up(
                            total_quantity_ticks(&ledger.open) * i128::from(*amount_ticks),
                            i128::from(SCALE),
                        );
                    }
                    _ => {}
                }
                last = source_of(action.effective_time).copied().or(last);
                continue;
            }
            let Some(trade) = trades.pop() else { break };
            if trade.ns > self.as_ns || trade.known_ns > self.known_ns {
                continue;
            }
            let currency = trade
                .text("activityCurrency")
                .ok_or_else(|| format!("activity {} has no activityCurrency", trade.record_id))?;
            if currency != self.base_currency {
                return Err(format!(
                    "activity {} is in {currency}, not the base currency {}",
                    trade.record_id, self.base_currency
                ));
            }
            let quantity = trade
                .ticks("activityQuantity")
                .ok_or_else(|| format!("activity {} has no quantity", trade.record_id))?;
            let price = trade
                .ticks("activityPrice")
                .ok_or_else(|| format!("activity {} has no activityPrice", trade.record_id))?;
            sequence += 1;
            pending.push(LotActivity {
                sequence,
                quantity_ticks: if trade.kind == "sell" {
                    -quantity
                } else {
                    quantity
                },
                price_ticks: price,
            });
            last_price = Some(price);
            last = Some(trade);
        }
        let ledger =
            apply_activities_from(ledger, &pending, self.method).map_err(|e| e.to_string())?;
        let Some(last) = last else { return Ok(()) };
        let prov = provenance(last, self.calendar.as_ref());
        let open_quantity = total_quantity_ticks(&ledger.open);
        let cost_basis = half_up(basis_numerator(&ledger.open), i128::from(SCALE));
        let realized: i64 = ledger.closed.iter().map(|c| c.realized_gain_ticks).sum();
        self.push(position, "open_quantity", to_i64(open_quantity)?, &prov);
        self.push(position, "cost_basis", to_i64(cost_basis)?, &prov);
        self.push(position, "realized_gain", realized, &prov);
        if let Some(price) = last_price {
            let value = half_up(open_quantity * i128::from(price), i128::from(SCALE));
            self.push(position, "market_value", to_i64(value)?, &prov);
        }
        self.push(position, "dividend_cash", to_i64(dividend_cash)?, &prov);
        Ok(())
    }

    /// A futures roll marker: a `transfer_out` naming its successor contract.
    fn roll(&mut self, event: &Event) -> Result<(), String> {
        let need = |name: &str| {
            event
                .ticks(name)
                .ok_or_else(|| format!("roll {} has no {name}", event.record_id))
        };
        let need_whole = |name: &str| {
            event
                .whole(name)
                .ok_or_else(|| format!("roll {} has no {name}", event.record_id))
        };
        let need_time = |name: &str| {
            event
                .instant(name)
                .ok_or_else(|| format!("roll {} has no {name}", event.record_id))
        };
        let expiring = ContractSpec {
            settlement_ticks: need("activitySettlement")?,
            expiry_ns: need_time("activityExpiry")?,
            tick_size_ticks: need("activityTickSize")?,
            multiplier: need_whole("activityMultiplier")?,
        };
        let successor = ContractSpec {
            settlement_ticks: need("activitySuccessorSettlement")?,
            expiry_ns: need_time("activitySuccessorExpiry")?,
            tick_size_ticks: need("activitySuccessorTickSize")?,
            multiplier: need_whole("activitySuccessorMultiplier")?,
        };
        let outcome = roll_position(&expiring, &successor, need("activityQuantity")?)
            .map_err(|e| e.to_string())?;
        let prov = provenance(event, self.calendar.as_ref());
        self.push(&event.position, "roll_cash", outcome.roll_cash_ticks, &prov);
        self.push(
            &event.position,
            "roll_position_value",
            outcome.position_value_ticks,
            &prov,
        );
        Ok(())
    }
}

fn project(
    ctx: GraphOpsContext<'_>,
    account: &str,
    as_of: &str,
    known_at: &str,
    basis_policy: &str,
    base_currency: &str,
) -> Response {
    let method = match basis_policy {
        "fifo" => LotMethod::Fifo,
        "lifo" => LotMethod::Lifo,
        other => {
            return refuse(
                ctx.req_id,
                BASIS_POLICY_UNSUPPORTED,
                format!("basis policy '{other}' is not applied; use fifo or lifo"),
            )
        }
    };
    if as_of.is_empty() {
        return refuse(ctx.req_id, FIGURE_PROVENANCE_MISSING, "as_of is empty");
    }
    let (Some(as_ns), Some(known_ns)) = (datetime_ns(as_of), datetime_ns(known_at)) else {
        return invalid(ctx.req_id, "as_of and known_at must be RFC 3339 date-times");
    };
    let Some(carrier) = ctx.read_authority.carrier() else {
        return refuse(ctx.req_id, "ACCESS_DENIED", "no verified tenant");
    };
    let scope = carrier.tenant_scope().to_owned();
    let calendar = match account_calendar(ctx.core, account) {
        Ok(calendar) => calendar,
        Err(detail) => return invalid(ctx.req_id, detail),
    };
    let events = activities(ctx.core, &scope, account);
    let mut projector = Projector {
        as_of,
        base_currency,
        method,
        calendar,
        as_ns,
        known_ns,
        figures: Vec::new(),
    };
    let mut by_position: BTreeMap<&str, Vec<&Event>> = BTreeMap::new();
    let mut digest = Sha256::new();
    digest.update(format!(
        "{account}|{as_of}|{known_at}|{basis_policy}|{base_currency}\n"
    ));
    for event in &events {
        if event.ns > as_ns || event.known_ns > known_ns {
            continue;
        }
        digest.update(event.node_id.as_bytes());
        for (name, value) in &event.fields {
            digest.update(format!("|{name}={value}"));
        }
        digest.update(b"\n");
        if event.kind == "transfer_out" && event.fields.contains_key("activityRollSuccessor") {
            if let Err(detail) = projector.roll(event) {
                return invalid(ctx.req_id, detail);
            }
        } else {
            by_position.entry(&event.position).or_default().push(event);
        }
    }
    for (position, group) in &by_position {
        if let Err(detail) = projector.position(position, group) {
            return invalid(ctx.req_id, detail);
        }
    }
    for figure in &projector.figures {
        let golden = GoldenFixtureFigure {
            value_ticks: figure.value_ticks,
            source: figure.source.clone(),
            as_of: figure.as_of.clone(),
            session: figure.session.clone(),
        };
        if let Err(refusal) = check_figure_provenance(&golden) {
            return refuse(
                ctx.req_id,
                FIGURE_PROVENANCE_MISSING,
                format!("{} of {}: {}", figure.name, figure.position, refusal.reason),
            );
        }
    }
    let projection = FinanceAccountProjection {
        account: account.to_owned(),
        as_of: as_of.to_owned(),
        known_at: known_at.to_owned(),
        basis_policy: basis_policy.to_owned(),
        base_currency: base_currency.to_owned(),
        scale: SCALE,
        input_digest: hex::encode(digest.finalize()),
        figures: projector.figures,
    };
    Response::ok(
        ctx.req_id,
        ResultPayload::of_ref::<
            medulla_protocol_types::result_contract::graph::FinanceAccountProject,
        >(&projection),
    )
}

/// Route `FinanceAccountProject` from the graph terminal read chain.
pub(super) async fn handle_project(
    ctx: GraphOpsContext<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    if let Method::FinanceAccountProject {
        account,
        as_of,
        known_at,
        basis_policy,
        base_currency,
    } = &method
    {
        return ControlFlow::Break(project(
            ctx,
            account,
            as_of,
            known_at,
            basis_policy,
            base_currency,
        ));
    }
    ControlFlow::Continue(method)
}
