//! `EVALUATE RULE id [ASOF ms] [COUNT n] [TEXT s [LOCALE l]]` (MEDULLA-BUSINESS-OPS-R002.7):
//! evaluate one stored `:RuleSpec` node over the rows the stage is given.
//!
//! The rule node's `:ruleKind`, `:ruleVersion` and `:ruleParameters` are the only
//! configuration. The input rows are nodes of the plan's snapshot; the per-kind gather table
//! of the business-operations plan says which classes each kind reads and from which
//! properties. A row of a class the kind does not take is ignored; a missing required
//! property refuses `RULE_INPUT_INVALID` naming the node and the property. The stage reads no
//! clock, calls no model and writes nothing: the as-of time is the explicit `ASOF`.

use std::collections::BTreeMap;

use medulla_graph_core::graph::props_carry_label;
use medulla_numeric::business::phrase_policy::{self, Phrase, Policy};
use medulla_numeric::business::reconcile::{Fee, Node, Refund, Sets};
use medulla_numeric::business::recurrence::HolidayCalendar;
use medulla_numeric::business::registry::{self, RuleInputs};
use medulla_numeric::business::reorder::{OpenLine, Sale, Stock};
use medulla_numeric::business::rule_spec::{
    self, InputRef, RuleKind, RuleRefusal, RuleSpec, RULE_DIGEST_MISMATCH, RULE_INPUT_INVALID,
    RULE_NOT_FOUND, RULE_PARAMETERS_INVALID,
};
use medulla_numeric::business::threshold::DataPoint;
use medulla_query_types::rule_expr::EvaluateRuleOp;
use serde_json::{Map, Value};

use super::propagate::node_props;
use super::PlanCtx;
use crate::rowset::RowSet;

/// Seconds in one day.
const DAY_S: i64 = 86_400;

/// A refusal as the stage's `<CODE>: <detail>` error.
fn refused(refusal: RuleRefusal) -> String {
    refusal.to_string()
}

fn invalid_input(id: &str, property: &str) -> RuleRefusal {
    RuleRefusal::new(
        RULE_INPUT_INVALID,
        format!("node `{id}` has no usable `{property}`"),
    )
}

/// One input row: its node id, its decoded properties and the revision read.
struct Row {
    id: String,
    props: Map<String, Value>,
    revision: u64,
}

impl Row {
    fn is(&self, class: &str) -> bool {
        props_carry_label(&self.props, class)
    }

    fn int(&self, property: &str) -> Result<i64, RuleRefusal> {
        self.props
            .get(property)
            .and_then(Value::as_i64)
            .ok_or_else(|| invalid_input(&self.id, property))
    }

    fn text(&self, property: &str) -> Result<String, RuleRefusal> {
        self.props
            .get(property)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| invalid_input(&self.id, property))
    }

    /// The node's day: `valid_from` in epoch seconds, `div_euclid(86_400)`.
    fn day(&self) -> Result<i64, RuleRefusal> {
        self.int("valid_from").map(|seconds| seconds.div_euclid(DAY_S))
    }

    fn input_ref(&self) -> InputRef {
        InputRef::new(self.id.clone(), self.revision)
    }
}

fn read_row(ctx: &PlanCtx, id: &str) -> Row {
    let props = node_props(ctx.view, id);
    let revision = props.get("tx_from").and_then(Value::as_u64).unwrap_or(0);
    Row { id: id.to_string(), props, revision }
}

/// The live edges of relationship `rel`, as `(source, target)` pairs in key order.
fn links(ctx: &PlanCtx, rel: &str) -> Vec<(String, String)> {
    let mut keys: Vec<&(String, String)> = ctx.view.edge_properties.keys().collect();
    keys.sort_unstable();
    let mut found = Vec::new();
    for key in keys {
        let live = ctx.view.edge_properties[key].iter().any(|blob| {
            medulla_types::msgpack::decode_property_value(blob.as_slice())
                .ok()
                .and_then(|value| value.as_object().cloned())
                .is_some_and(|props| {
                    super::pin_admits(ctx.as_of_pin, &props)
                        && props.get("relationship").and_then(Value::as_str) == Some(rel)
                })
        });
        if live {
            found.push(key.clone());
        }
    }
    found
}

/// The node an edge of `rel` leaves `source` for, if there is one.
fn target_of(links: &[(String, String)], source: &str) -> Option<String> {
    links
        .iter()
        .find(|(from, _)| from == source)
        .map(|(_, to)| to.clone())
}

/// The rule record: kind, version and parameters from the rule node alone.
fn load_rule(ctx: &PlanCtx, rule: &str) -> Result<RuleSpec, RuleRefusal> {
    let node = read_row(ctx, rule);
    let Some(kind) = node.props.get("ruleKind").and_then(Value::as_str) else {
        return Err(RuleRefusal::new(
            RULE_NOT_FOUND,
            format!("no rule record `{rule}`"),
        ));
    };
    let version = node
        .props
        .get("ruleVersion")
        .and_then(Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())
        .ok_or_else(|| {
            RuleRefusal::new(
                RULE_PARAMETERS_INVALID,
                format!("rule `{rule}` has no usable ruleVersion"),
            )
        })?;
    let text = node
        .props
        .get("ruleParameters")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let spec = rule_spec::load(kind, version, rule_spec::parse_parameters(text)?)?;
    if let Some(stored) = node.props.get("ruleDigest").and_then(Value::as_str) {
        if stored != spec.digest {
            return Err(RuleRefusal::new(
                RULE_DIGEST_MISMATCH,
                format!("rule `{rule}` stores digest {stored}, computed {}", spec.digest),
            ));
        }
    }
    Ok(spec)
}

fn recurrence_inputs(rows: &[Row], count: Option<u32>) -> Result<RuleInputs, RuleRefusal> {
    let mut calendars = rows.iter().filter(|row| row.is("HolidayCalendar"));
    let (calendar, calendar_node) = match (calendars.next(), calendars.next()) {
        (None, _) => (HolidayCalendar::default(), None),
        (Some(_), Some(extra)) => {
            return Err(RuleRefusal::new(
                RULE_INPUT_INVALID,
                format!("more than one HolidayCalendar row, `{}` is one too many", extra.id),
            ))
        }
        (Some(row), None) => {
            let dates = match row.props.get("holidayDate") {
                None => Vec::new(),
                Some(Value::String(one)) => vec![one.as_str()],
                Some(Value::Array(many)) => many
                    .iter()
                    .map(|date| date.as_str().ok_or_else(|| invalid_input(&row.id, "holidayDate")))
                    .collect::<Result<Vec<_>, _>>()?,
                Some(_) => return Err(invalid_input(&row.id, "holidayDate")),
            };
            let holidays = dates
                .into_iter()
                .map(|date| {
                    registry::parse_date(date).ok_or_else(|| invalid_input(&row.id, "holidayDate"))
                })
                .collect::<Result<std::collections::BTreeSet<_>, RuleRefusal>>()?;
            (
                HolidayCalendar { revision: row.revision.to_string(), holidays },
                Some(row.input_ref()),
            )
        }
    };
    Ok(RuleInputs::Recurrence {
        calendar,
        calendar_node,
        count: count.map_or(1, |count| count as usize),
    })
}

/// The amount of an order: its lines' `lineQuantity * unitPriceMinor + taxMinor`.
fn order_amount(ctx: &PlanCtx, order: &Row, of_order: &[(String, String)]) -> Result<i64, RuleRefusal> {
    let mut total = 0i64;
    for (line, _) in of_order.iter().filter(|(_, to)| *to == order.id) {
        let line = read_row(ctx, line);
        if !line.is("OrderLine") {
            continue;
        }
        let amount = line
            .int("lineQuantity")?
            .checked_mul(line.int("unitPriceMinor")?)
            .and_then(|gross| gross.checked_add(line.int("taxMinor").ok()?))
            .ok_or_else(|| invalid_input(&line.id, "taxMinor"))?;
        total = total
            .checked_add(amount)
            .ok_or_else(|| invalid_input(&order.id, "lineQuantity"))?;
    }
    Ok(total)
}

fn threshold_inputs(ctx: &PlanCtx, rows: &[Row]) -> Result<RuleInputs, RuleRefusal> {
    let of_order = links(ctx, "lineOfOrder");
    let mut points = Vec::new();
    for row in rows {
        for series in ["LedgerEntry", "Order", "Payment"] {
            if !row.is(series) {
                continue;
            }
            let amount = if series == "Order" {
                order_amount(ctx, row, &of_order)?
            } else {
                row.int("amountMinor")?
            };
            points.push(DataPoint {
                node_id: row.id.clone(),
                revision: row.revision,
                series: series.to_string(),
                day: row.day()?,
                amount,
            });
        }
    }
    Ok(RuleInputs::Threshold { points })
}

fn amount_node(row: &Row, amount: i64) -> Result<Node, RuleRefusal> {
    Ok(Node {
        id: row.id.clone(),
        revision: row.revision,
        amount,
        currency: row.text("amountCurrency")?,
        day: row.day()?,
    })
}

fn reconcile_inputs(ctx: &PlanCtx, rows: &[Row]) -> Result<RuleInputs, RuleRefusal> {
    let of_order = links(ctx, "lineOfOrder");
    let refund_of = links(ctx, "refundOf");
    let fee_of = links(ctx, "feeOfPayout");
    let mut sets = Sets::default();
    for row in rows {
        if row.is("Order") {
            sets.orders.push(amount_node(row, order_amount(ctx, row, &of_order)?)?);
        } else if row.is("Payment") {
            sets.payments.push(amount_node(row, row.int("amountMinor")?)?);
        } else if row.is("Payout") {
            sets.payouts.push(amount_node(row, row.int("amountMinor")?)?);
        } else if row.is("Refund") {
            sets.refunds.push(Refund {
                id: row.id.clone(),
                revision: row.revision,
                payment_id: target_of(&refund_of, &row.id)
                    .ok_or_else(|| invalid_input(&row.id, "refundOf"))?,
                amount: row.int("amountMinor")?,
            });
        } else if row.is("Fee") {
            sets.fees.push(Fee {
                id: row.id.clone(),
                revision: row.revision,
                payout_id: target_of(&fee_of, &row.id)
                    .ok_or_else(|| invalid_input(&row.id, "feeOfPayout"))?,
                amount: row.int("amountMinor")?,
            });
        }
    }
    Ok(RuleInputs::Reconcile { sets })
}

fn reorder_inputs(rows: &[Row]) -> Result<RuleInputs, RuleRefusal> {
    let mut sales = Vec::new();
    let mut open_lines = Vec::new();
    let mut levels = Vec::new();
    for row in rows {
        if row.is("OrderLine") {
            sales.push(Sale {
                node_id: row.id.clone(),
                revision: row.revision,
                day: row.day()?,
                quantity: row.int("lineQuantity")?,
            });
        } else if row.is("PurchaseOrderLine") {
            open_lines.push(OpenLine {
                node_id: row.id.clone(),
                revision: row.revision,
                quantity: row.int("purchaseLineQuantity")?,
            });
        } else if row.is("StockLevel") {
            levels.push(Stock {
                node_id: row.id.clone(),
                revision: row.revision,
                on_hand: row.int("onHand")?,
                reserved: row.int("reservedQuantity")?,
            });
        }
    }
    let (Some(stock), None) = (levels.pop(), levels.first()) else {
        return Err(RuleRefusal::new(
            RULE_INPUT_INVALID,
            "a reorder rule takes exactly one StockLevel row",
        ));
    };
    Ok(RuleInputs::Reorder { sales, stock, open_lines })
}

/// The phrases the policy `policy` links to by `rel`, in target id order.
fn phrases(ctx: &PlanCtx, policy: &str, rel: &str) -> Result<Vec<Phrase>, RuleRefusal> {
    links(ctx, rel)
        .into_iter()
        .filter(|(from, _)| from == policy)
        .map(|(_, to)| -> Result<Phrase, RuleRefusal> {
            let row = read_row(ctx, &to);
            Ok(Phrase {
                text: row.text("phraseText")?,
                locale: row.text("phraseLocale").unwrap_or_default(),
                mode: phrase_policy::Mode::parse(&row.text("phraseMatchMode")?)?,
            })
        })
        .collect()
}

fn phrase_inputs(ctx: &PlanCtx, rows: &[Row], op: &EvaluateRuleOp) -> Result<RuleInputs, RuleRefusal> {
    let mut policies = rows.iter().filter(|row| row.is("ClaimPolicy"));
    let (Some(row), None) = (policies.next(), policies.next()) else {
        return Err(RuleRefusal::new(
            RULE_INPUT_INVALID,
            "a phrase policy rule takes exactly one ClaimPolicy row",
        ));
    };
    let text = op.text.clone().ok_or_else(|| {
        RuleRefusal::new(RULE_INPUT_INVALID, "a phrase policy rule needs a TEXT to check")
    })?;
    let policy = Policy {
        revision: row.revision.to_string(),
        forbidden: phrases(ctx, &row.id, "claimForbiddenPhrase")?,
        approved: phrases(ctx, &row.id, "claimApprovedPhrase")?,
    };
    Ok(RuleInputs::PhrasePolicy {
        text,
        locale: op.locale.clone().unwrap_or_default(),
        policy,
        policy_node: row.input_ref(),
    })
}

fn gather(
    ctx: &PlanCtx,
    spec: &RuleSpec,
    input: &RowSet,
    op: &EvaluateRuleOp,
) -> Result<RuleInputs, RuleRefusal> {
    let rows: Vec<Row> = input.ids().iter().map(|id| read_row(ctx, id)).collect();
    match spec.kind {
        RuleKind::Recurrence => recurrence_inputs(&rows, op.count),
        RuleKind::Threshold => threshold_inputs(ctx, &rows),
        RuleKind::Reconcile => reconcile_inputs(ctx, &rows),
        RuleKind::Reorder => reorder_inputs(&rows),
        RuleKind::PhrasePolicy => phrase_inputs(ctx, &rows, op),
    }
}

fn evaluate(ctx: &PlanCtx, input: &RowSet, op: &EvaluateRuleOp) -> Result<RowSet, RuleRefusal> {
    let as_of_ms = op.as_of_ms.ok_or_else(registry::asof_required)?;
    let spec = load_rule(ctx, &op.rule)?;
    let inputs = gather(ctx, &spec, input, op)?;
    let result = registry::evaluate_rule(&spec, inputs, as_of_ms)?;
    let trace = result.trace();
    let mut references: Vec<String> = trace
        .inputs
        .iter()
        .map(|input| format!("{}@{}", input.node_id, input.revision))
        .collect();
    references.sort();
    let cells: BTreeMap<String, String> = BTreeMap::from([
        ("kind".to_string(), result.kind().as_str().to_string()),
        ("rule_digest".to_string(), trace.rule_digest.clone()),
        ("inputs".to_string(), references.join(",")),
        ("steps".to_string(), trace.steps.join("\n")),
        ("result".to_string(), result.canonical_json()),
    ]);
    let texts = BTreeMap::from([(op.rule.clone(), cells)]);
    Ok(RowSet::from_rows([(op.rule.clone(), None)]).with_texts(texts))
}

/// Run one `EVALUATE RULE` stage over `input`: one row, the id of the rule node, with the
/// text cells `kind`, `rule_digest`, `inputs`, `steps` and `result`. A refusal is the
/// stage's `<CODE>: <detail>` error.
pub(super) fn apply(op: &EvaluateRuleOp, input: RowSet, ctx: &PlanCtx) -> Result<RowSet, String> {
    evaluate(ctx, &input, op).map_err(refused)
}
