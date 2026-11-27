//! Branch selection (MEDULLA-DECISION-ENGINE-R180.3).
//!
//! A pure function over a task-plan branch, the bound inputs and the decision tables the branch
//! names. An exclusive branch takes the first arm, in declared order, whose condition holds, else
//! the default arm. An inclusive branch takes every arm whose condition holds, else the default
//! arm. A parallel branch takes every arm. A `{ decision_iri }` condition holds when the first
//! output of that table's verdict is true. Nothing here reads a clock or a store.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use medulla_decision_types::decision::process_decision::{BranchVerdict, DmnValue};
use medulla_decision_types::decision::task_plan::{
    BranchArm, BranchCondition, BranchKind, BranchSpec, CompareOp,
};

use super::dmn_eval::{evaluate_table, DmnError, DmnTable};

fn unsupported(what: impl Into<String>) -> DmnError {
    DmnError::ExpressionUnsupported { entry: what.into() }
}

/// The typed value a comparison literal stands for. A float is not a value of the closed set.
fn literal(value: &serde_json::Value) -> Result<DmnValue, DmnError> {
    match value {
        serde_json::Value::String(text) => Ok(DmnValue::String(text.clone())),
        serde_json::Value::Bool(flag) => Ok(DmnValue::Boolean(*flag)),
        serde_json::Value::Number(number) => number
            .as_i64()
            .map(DmnValue::Integer)
            .ok_or_else(|| unsupported(number.to_string())),
        other => Err(unsupported(other.to_string())),
    }
}

/// The value as `units / 10^scale`, for the numeric kinds.
fn numeric(value: &DmnValue) -> Option<(i128, u32)> {
    match value {
        DmnValue::Integer(units) => Some((i128::from(*units), 0)),
        DmnValue::Decimal(decimal) => Some((i128::from(decimal.units), u32::from(decimal.scale))),
        _ => None,
    }
}

/// The order of two values of one kind; `None` when the kinds have no order.
fn order(left: &DmnValue, right: &DmnValue) -> Option<Ordering> {
    if let (Some((a, a_scale)), Some((b, b_scale))) = (numeric(left), numeric(right)) {
        let scale = a_scale.max(b_scale);
        let lift = |units: i128, from: u32| units.checked_mul(10_i128.checked_pow(scale - from)?);
        return Some(lift(a, a_scale)?.cmp(&lift(b, b_scale)?));
    }
    match (left, right) {
        (DmnValue::String(a), DmnValue::String(b)) | (DmnValue::Date(a), DmnValue::Date(b)) => {
            Some(a.cmp(b))
        }
        _ => None,
    }
}

fn equal(left: &DmnValue, right: &DmnValue) -> bool {
    order(left, right).map_or(left == right, |found| found == Ordering::Equal)
}

fn compares(
    op: CompareOp,
    bound: &DmnValue,
    expected: &serde_json::Value,
) -> Result<bool, DmnError> {
    if op == CompareOp::In {
        let serde_json::Value::Array(items) = expected else {
            return Err(unsupported("in needs an array"));
        };
        let mut found = false;
        for item in items {
            found |= equal(bound, &literal(item)?);
        }
        return Ok(found);
    }
    let expected = literal(expected)?;
    Ok(match op {
        CompareOp::Eq => equal(bound, &expected),
        CompareOp::Ne => !equal(bound, &expected),
        CompareOp::Lt | CompareOp::Le | CompareOp::Gt | CompareOp::Ge => {
            let found =
                order(bound, &expected).ok_or_else(|| unsupported("an order over unlike kinds"))?;
            match op {
                CompareOp::Lt => found == Ordering::Less,
                CompareOp::Le => found != Ordering::Greater,
                CompareOp::Gt => found == Ordering::Greater,
                _ => found != Ordering::Less,
            }
        }
        CompareOp::In => unreachable!("handled above"),
    })
}

/// Whether one arm's condition holds. An arm with no condition never holds on its own.
fn holds(
    arm: &BranchArm,
    inputs: &BTreeMap<String, DmnValue>,
    tables: &BTreeMap<String, DmnTable>,
) -> Result<bool, DmnError> {
    match &arm.condition {
        None => Ok(false),
        Some(BranchCondition::Compare(comparison)) => {
            let bound =
                inputs
                    .get(&comparison.reference)
                    .ok_or_else(|| DmnError::InputMissing {
                        name: comparison.reference.clone(),
                    })?;
            compares(comparison.op, bound, &comparison.value)
        }
        Some(BranchCondition::Decision(decision)) => {
            let table =
                tables
                    .get(&decision.decision_iri)
                    .ok_or_else(|| DmnError::InputMissing {
                        name: decision.decision_iri.clone(),
                    })?;
            let verdict = evaluate_table(table, inputs)?;
            let first = table.outputs.first().map(|output| output.name.as_str());
            Ok(verdict
                .outputs
                .first()
                .zip(first)
                .and_then(|(row, name)| row.get(name))
                .is_some_and(|value| *value == DmnValue::Boolean(true)))
        }
    }
}

/// The arms `branch` takes over `inputs`; `tables` holds the decision tables by decision IRI.
pub fn select_branch(
    branch: &BranchSpec,
    inputs: &BTreeMap<String, DmnValue>,
    tables: &BTreeMap<String, DmnTable>,
) -> Result<BranchVerdict, DmnError> {
    let arms = branch.arms.as_slice();
    if branch.kind == BranchKind::Parallel {
        return Ok(BranchVerdict {
            taken_task_ids: arms.iter().map(|arm| arm.target_task_id.clone()).collect(),
            matched_conditions: Vec::new(),
            used_default: false,
        });
    }
    let mut matched = Vec::new();
    for arm in arms {
        if holds(arm, inputs, tables)? {
            matched.push(arm.target_task_id.clone());
            if branch.kind == BranchKind::Exclusive {
                break;
            }
        }
    }
    if matched.is_empty() {
        return Ok(BranchVerdict {
            taken_task_ids: branch.default_arm.iter().cloned().collect(),
            matched_conditions: Vec::new(),
            used_default: branch.default_arm.is_some(),
        });
    }
    Ok(BranchVerdict {
        taken_task_ids: matched.clone(),
        matched_conditions: matched,
        used_default: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::dmn_eval::{DmnOutput, DmnRule};
    use medulla_decision_types::decision::task_plan::{BranchComparison, BranchDecisionCondition};
    use medulla_types::contract::BoundedVec;

    fn arm(target: &str, condition: Option<BranchCondition>) -> BranchArm {
        BranchArm {
            target_task_id: target.into(),
            condition,
        }
    }

    fn above(reference: &str, limit: i64) -> Option<BranchCondition> {
        Some(BranchCondition::Compare(BranchComparison {
            reference: reference.into(),
            op: CompareOp::Gt,
            value: serde_json::json!(limit),
        }))
    }

    fn branch(kind: BranchKind, arms: Vec<BranchArm>, default_arm: Option<&str>) -> BranchSpec {
        BranchSpec {
            kind,
            arms: BoundedVec::new(arms).unwrap(),
            default_arm: default_arm.map(str::to_string),
            exhaustive: false,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R180.3
    /// Each branch kind takes its own arms: first match, every match, the default and all arms,
    /// and a missing input is refused.
    #[test]
    fn branch_kinds_take_the_right_arms() {
        let inputs = BTreeMap::from([("age".to_string(), DmnValue::Integer(42))]);
        let table = DmnTable {
            hit_policy: "UNIQUE".into(),
            aggregation: None,
            input_names: vec!["age".into()],
            outputs: vec![DmnOutput {
                name: "ok".into(),
                priority: Vec::new(),
            }],
            rules: vec![
                DmnRule {
                    id: "r1".into(),
                    input_entries: vec![">= 18".into()],
                    output_entries: vec!["true".into()],
                },
                DmnRule {
                    id: "r2".into(),
                    input_entries: vec!["< 18".into()],
                    output_entries: vec!["false".into()],
                },
            ],
        };
        let tables = BTreeMap::from([("urn:adult".to_string(), table)]);
        let adult = Some(BranchCondition::Decision(BranchDecisionCondition {
            decision_iri: "urn:adult".into(),
        }));
        let arms = || {
            vec![
                arm("a", above("age", 50)),
                arm("b", above("age", 40)),
                arm("c", adult.clone()),
            ]
        };

        // Exclusive: the first holding arm in declared order, and only it.
        let exclusive = select_branch(
            &branch(BranchKind::Exclusive, arms(), Some("d")),
            &inputs,
            &tables,
        )
        .unwrap();
        assert_eq!(exclusive.taken_task_ids, ["b"]);
        assert!(!exclusive.used_default);
        // Inclusive: every holding arm, a table condition holding when its first output is true.
        let inclusive = select_branch(
            &branch(BranchKind::Inclusive, arms(), Some("d")),
            &inputs,
            &tables,
        )
        .unwrap();
        assert_eq!(inclusive.taken_task_ids, ["b", "c"]);
        assert_eq!(inclusive.matched_conditions, ["b", "c"]);
        // No condition holds: the default arm.
        let young = BTreeMap::from([("age".to_string(), DmnValue::Integer(10))]);
        let fallback = select_branch(
            &branch(BranchKind::Exclusive, arms(), Some("d")),
            &young,
            &tables,
        )
        .unwrap();
        assert_eq!(fallback.taken_task_ids, ["d"]);
        assert!(fallback.used_default);
        // Parallel: every arm.
        let parallel = select_branch(
            &branch(BranchKind::Parallel, arms(), None),
            &inputs,
            &tables,
        )
        .unwrap();
        assert_eq!(parallel.taken_task_ids, ["a", "b", "c"]);
        // An unbound input is named, not guessed.
        let missing = select_branch(
            &branch(BranchKind::Exclusive, arms(), None),
            &BTreeMap::new(),
            &tables,
        );
        assert_eq!(missing, Err(DmnError::InputMissing { name: "age".into() }));
    }
}
