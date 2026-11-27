//! The `decision_table_eval` and `branch_select` question families
//! (MEDULLA-DECISION-ENGINE-R180.4, R180.5, R180.6).
//!
//! Both read the graph named by the `graph` param under the caller's access and
//! row-level filter, and carry one JSON param, `process_decision`: a
//! `DecisionTableRequest` or a `BranchSelectRequest`. A table question reads the
//! `DmnDecisionTable` node of the decision (the `ProcessDocumentImport` mapping:
//! its clauses and its rules in `dmnRuleIndex` order) and evaluates it. A branch
//! question finds the sealed task plan by its digest in the caller's decision
//! log, takes the named branch step and selects its arms, evaluating the stored
//! table of every `{ decision_iri }` condition. Each outcome is an `Advisory`; a
//! refusal abstains with its registered code and writes nothing.

use std::collections::BTreeMap;

use medulla_decision_stats::decision::branch_select::select_branch;
use medulla_decision_stats::decision::dmn_eval::{
    evaluate_table, DmnError, DmnOutput, DmnRule, DmnTable,
};
use medulla_decision_types::decision::process_decision::{
    BranchSelectRequest, BranchVerdict, DecisionTableRequest, DmnValue, TableVerdict,
    BRANCH_NO_ARM, DMN_ANY_CONFLICT, DMN_EXPRESSION_UNSUPPORTED, DMN_INPUT_MISSING,
    DMN_NO_RULE_MATCHED, DMN_UNIQUE_VIOLATED, PLAN_UNKNOWN,
};
use medulla_decision_types::decision::record::outcome::DecisionRefusal;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, StatisticalOutcome,
};
use medulla_decision_types::decision::task_plan::{BranchCondition, BranchKind, BranchSpec};
use medulla_decision_types::decision::{DecisionPolicy, PremiseClass, PremiseRef};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;
use medulla_types::evidence_class::EvidenceClass;
use medulla_types::refusal_code::RefusalCode;

use crate::decide::stat_executor::{Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource};
use crate::decide::stat_log::visible_entries;
use crate::decide::SharedState;

use super::support::{abstained, advisory, invalid, json_param, single_option, text_param};
use super::DecisionFamily;

const PARAM_GRAPH: &str = "graph";
const PARAM_REQUEST: &str = "process_decision";

/// The family of the `decision_table_eval` and `branch_select` question kinds.
pub struct ProcessDecisionFamily;

/// The question ids a host pins a feature schema for, one per control step
/// (MEDULLA-DECISION-ENGINE-R180.8). The schema component of each is
/// `decide.schema.<question id>`.
pub const PROCESS_CONTROL_QUESTION_IDS: [&str; 2] =
    ["process.branch_select", "process.decision_table_eval"];

/// The one feature schema both control steps pin (MEDULLA-DECISION-ENGINE-R180.8).
///
/// A process decision scores nothing: it evaluates a stored table or a sealed plan
/// branch and answers the single declared option `decision`. A request still names a
/// pinned schema, and a schema declares at least one feature, so the body is the one
/// inert `declared_cost_micros` feature imputing 0, which no process decision reads.
/// `medulla/generated/_process_control_feature_schemas.py` mirrors it.
pub fn process_control_feature_schema(
) -> medulla_decision_types::decision::statistical::features::FeatureSchemaBody {
    use medulla_decision_types::decision::numeric::{QuantScaleTag, QuantisedValue};
    use medulla_decision_types::decision::statistical::features::{
        FeatureKind, FeatureSchemaBody, FeatureSpec, MissingValue, FEATURE_SCHEMA_VERSION,
    };
    FeatureSchemaBody {
        schema_version: FEATURE_SCHEMA_VERSION,
        features: BoundedVec::new(vec![FeatureSpec {
            name: "declared_cost_micros".to_string(),
            kind: FeatureKind::DeclaredCostMicros,
            missing: MissingValue::Impute {
                value: QuantisedValue {
                    scale: QuantScaleTag::Q32,
                    value: 0,
                },
            },
        }])
        .expect("one feature is within the schema bound"),
    }
}

struct Abstention(RefusalCode, String);

impl DecisionRefusal for Abstention {
    fn code(&self) -> RefusalCode {
        self.0
    }
    fn detail(&self) -> String {
        self.1.clone()
    }
}

/// The graph a process decision reads its stored tables from, under the caller's access.
pub(in crate::decide) async fn evidence_view(
    state: &SharedState,
    agent_id: &str,
    request: &DecideRequest,
) -> Result<Option<GraphView>, String> {
    match text_param(request, PARAM_GRAPH)? {
        Some(graph) => crate::decide::stat_support::filtered_view(state, agent_id, graph)
            .await
            .map(Some),
        None => Err(invalid(format!(
            "a {} question requires the {PARAM_GRAPH} parameter",
            if request.question.kind == QuestionKind::BranchSelect {
                "branch_select"
            } else {
                "decision_table_eval"
            }
        ))),
    }
}

fn refuse(kind: QuestionKind, error: DmnError) -> Result<Executed, String> {
    let (code, detail) = match error {
        DmnError::NoRuleMatched => (DMN_NO_RULE_MATCHED, "no rule matched".to_string()),
        DmnError::UniqueViolated { rule_ids } => (
            DMN_UNIQUE_VIOLATED,
            format!("more than one rule matched: {}", rule_ids.join(",")),
        ),
        DmnError::AnyConflict { rule_ids } => (
            DMN_ANY_CONFLICT,
            format!("rules with different outputs matched: {}", rule_ids.join(",")),
        ),
        DmnError::ExpressionUnsupported { entry } => (
            DMN_EXPRESSION_UNSUPPORTED,
            format!("`{entry}` is outside the supported subset"),
        ),
        DmnError::InputMissing { name } => {
            (DMN_INPUT_MISSING, format!("input `{name}` was not supplied"))
        }
        DmnError::MalformedTable { rule_id } => {
            return Err(invalid(format!(
                "the stored rule `{rule_id}` does not match its table's clauses"
            )))
        }
        DmnError::Overflow => {
            return Err(invalid("a COLLECT aggregation overflows 64-bit fixed point"))
        }
    };
    Ok(abstained(kind, &Abstention(code, detail)))
}

fn text_of(row: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    row.get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn index_of(row: &serde_json::Map<String, serde_json::Value>, key: &str) -> usize {
    text_of(row, key)
        .and_then(|text| text.parse().ok())
        .unwrap_or(usize::MAX)
}

/// The rows of `view` whose `type` is `label` and whose `reference` property is `table_id`.
fn rows_of(
    view: &GraphView,
    label: &str,
    reference: &str,
    table_id: &str,
) -> Vec<(String, serde_json::Map<String, serde_json::Value>)> {
    view.node_properties
        .keys()
        .filter_map(|id| Some((id.clone(), view.node_row_object(id)?)))
        .filter(|(_, row)| {
            text_of(row, "type").as_deref() == Some(label)
                && text_of(row, reference).as_deref() == Some(table_id)
        })
        .collect()
}

/// A stored entry map's positional entries `<prefix><n>`, in position order.
fn entries(row: &serde_json::Map<String, serde_json::Value>, prefix: &str) -> Vec<String> {
    let mut found: Vec<(usize, String)> = row
        .iter()
        .filter_map(|(key, value)| {
            let position = key.strip_prefix(prefix)?.parse::<usize>().ok()?;
            Some((position, value.as_str()?.to_string()))
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, entry)| entry).collect()
}

/// The decision table of `decision_iri` as the import mapped it into `view`.
fn load_table(view: &GraphView, decision_iri: &str) -> Option<DmnTable> {
    let (table_id, table_row) = view
        .node_properties
        .keys()
        .filter_map(|id| Some((id.clone(), view.node_row_object(id)?)))
        .find(|(_, row)| {
            text_of(row, "type").as_deref() == Some("DmnDecisionTable")
                && text_of(row, "dmnDecision").as_deref() == Some(decision_iri)
        })?;
    let mut inputs = rows_of(view, "DmnInputClause", "dmnTable", &table_id);
    inputs.sort_by_key(|(id, row)| (index_of(row, "dmnClauseIndex"), id.clone()));
    let mut outputs = rows_of(view, "DmnOutputClause", "dmnTable", &table_id);
    outputs.sort_by_key(|(id, row)| (index_of(row, "dmnClauseIndex"), id.clone()));
    let mut rules = rows_of(view, "DmnRule", "dmnTable", &table_id);
    rules.sort_by_key(|(id, row)| (index_of(row, "dmnRuleIndex"), id.clone()));
    Some(DmnTable {
        hit_policy: text_of(&table_row, "dmnHitPolicy").unwrap_or_default(),
        aggregation: text_of(&table_row, "dmnAggregation"),
        input_names: inputs
            .iter()
            .map(|(id, row)| {
                text_of(row, "dmnExpression")
                    .filter(|expression| !expression.trim().is_empty())
                    .or_else(|| text_of(row, "name"))
                    .unwrap_or_else(|| id.clone())
            })
            .collect(),
        outputs: outputs
            .iter()
            .map(|(id, row)| DmnOutput {
                name: text_of(row, "name").unwrap_or_else(|| id.clone()),
                priority: Vec::new(),
            })
            .collect(),
        rules: rules
            .iter()
            .map(|(id, row)| DmnRule {
                id: id.clone(),
                input_entries: entries(row, "dmnInputEntry:"),
                output_entries: entries(row, "dmnOutputEntry:"),
            })
            .collect(),
    })
}

fn render(value: &DmnValue) -> String {
    match value {
        DmnValue::String(text) | DmnValue::Date(text) => text.clone(),
        DmnValue::Integer(number) => number.to_string(),
        DmnValue::Boolean(flag) => flag.to_string(),
        DmnValue::Decimal(decimal) => {
            let scale = usize::from(decimal.scale);
            let digits = decimal.units.unsigned_abs().to_string();
            let padded = format!("{digits:0>width$}", width = scale + 1);
            let split = padded.len() - scale;
            let sign = if decimal.units < 0 { "-" } else { "" };
            if scale == 0 {
                format!("{sign}{padded}")
            } else {
                format!("{sign}{}.{}", &padded[..split], &padded[split..])
            }
        }
    }
}

fn input_premises(inputs: &BTreeMap<String, DmnValue>) -> Vec<FamilyPremise> {
    inputs
        .iter()
        .map(|(name, value)| FamilyPremise {
            subject: "process_decision.input".into(),
            fact: format!("input:{name}={}", render(value)),
            class: PremiseClass::Claim,
            source: FamilyPremiseSource::Request("params.process_decision"),
        })
        .collect()
}

fn table_answer(
    view: Option<&GraphView>,
    request: &DecideRequest,
) -> Result<Executed, String> {
    let ask: DecisionTableRequest = json_param(request, PARAM_REQUEST)?;
    let kind = QuestionKind::DecisionTableEval;
    let view = view.ok_or_else(|| invalid("the decision graph was not read"))?;
    let table = load_table(view, &ask.decision_iri).ok_or_else(|| {
        invalid(format!(
            "the graph holds no decision table of `{}`",
            ask.decision_iri
        ))
    })?;
    let verdict: TableVerdict = match evaluate_table(&table, &ask.inputs) {
        Ok(verdict) => verdict,
        Err(error) => return refuse(kind, error),
    };
    let mut options: Vec<String> = verdict
        .matched_rule_ids
        .iter()
        .map(|id| format!("rule:{id}"))
        .collect();
    options.push(format!(
        "outputs:{}",
        serde_json::to_string(&verdict.outputs).map_err(invalid)?
    ));
    let mut executed = advisory(options)?;
    let hit_policy = table.hit_policy.trim().to_uppercase();
    executed.family_premises = verdict
        .matched_rule_ids
        .iter()
        .map(|id| FamilyPremise {
            subject: "process_decision.rule".into(),
            fact: format!(
                "dmn:rule={id};table={};hit_policy={hit_policy}",
                ask.decision_iri
            ),
            class: PremiseClass::Observation,
            source: FamilyPremiseSource::Observation(id.clone()),
        })
        .chain(input_premises(&ask.inputs))
        .collect();
    Ok(executed)
}

/// The branch step `branch_task_id` of the sealed plan whose digest is `plan_digest`.
fn sealed_branch(
    ctx: &ExecutionContext,
    plan_digest: &str,
    branch_task_id: &str,
) -> Result<Option<BranchSpec>, String> {
    let Some(reader) = ctx.reader else {
        return Ok(None);
    };
    for entry in visible_entries(ctx.store, reader)? {
        let Some(plan) = entry.record.task_plan() else {
            continue;
        };
        if plan.plan_digest != plan_digest {
            continue;
        }
        return Ok(plan
            .tasks
            .iter()
            .find(|task| task.task_id == branch_task_id)
            .and_then(|task| task.branch.clone()));
    }
    Ok(None)
}

fn branch_answer(
    ctx: &ExecutionContext,
    view: Option<&GraphView>,
    request: &DecideRequest,
) -> Result<Executed, String> {
    let ask: BranchSelectRequest = json_param(request, PARAM_REQUEST)?;
    let kind = QuestionKind::BranchSelect;
    let Some(branch) = sealed_branch(ctx, &ask.plan_digest, &ask.branch_task_id)? else {
        return Ok(abstained(
            kind,
            &Abstention(
                PLAN_UNKNOWN,
                format!(
                    "no sealed plan `{}` holds the branch step `{}`",
                    ask.plan_digest, ask.branch_task_id
                ),
            ),
        ));
    };
    let mut tables = BTreeMap::new();
    for arm in branch.arms.iter() {
        if let Some(BranchCondition::Decision(decision)) = &arm.condition {
            let table = view
                .and_then(|view| load_table(view, &decision.decision_iri))
                .ok_or_else(|| {
                    invalid(format!(
                        "the graph holds no decision table of `{}`",
                        decision.decision_iri
                    ))
                })?;
            tables.insert(decision.decision_iri.clone(), table);
        }
    }
    let verdict: BranchVerdict = match select_branch(&branch, &ask.inputs, &tables) {
        Ok(verdict) => verdict,
        Err(error) => return refuse(kind, error),
    };
    if verdict.taken_task_ids.is_empty() && branch.kind != BranchKind::Parallel {
        return Ok(abstained(
            kind,
            &Abstention(
                BRANCH_NO_ARM,
                format!(
                    "no arm of `{}` held and the branch has no default arm",
                    ask.branch_task_id
                ),
            ),
        ));
    }
    let mut options: Vec<String> = verdict
        .taken_task_ids
        .iter()
        .map(|id| format!("taken:{id}"))
        .collect();
    options.push(format!("default:{}", verdict.used_default));
    let mut executed = advisory(options)?;
    let mut premises = vec![FamilyPremise {
        subject: "process_decision.branch".into(),
        fact: format!(
            "branch:task={};taken={};default={}",
            ask.branch_task_id,
            verdict.taken_task_ids.join(","),
            verdict.used_default
        ),
        class: PremiseClass::Observation,
        source: FamilyPremiseSource::Observation(ask.plan_digest.clone()),
    }];
    premises.extend(input_premises(&ask.inputs));
    executed.family_premises = premises;
    Ok(executed)
}

impl DecisionFamily for ProcessDecisionFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, "decision")
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(match request.question.kind {
            QuestionKind::BranchSelect => branch_answer(ctx, graph_view, request),
            _ => table_answer(graph_view, request),
        })
    }

    fn premises(
        &self,
        _request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        executed
            .family_premises
            .iter()
            .map(|premise| premise.seal(policy_digest))
            .collect()
    }

    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        // The table is stored graph content, so the record is an observation.
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(EvidenceClass::Observation)
    }
}

#[cfg(test)]
mod control_schema_tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R180.8
    #[test]
    fn the_control_schema_is_valid_and_encodes_one_inert_feature() {
        let body = process_control_feature_schema().checked().unwrap();
        assert_eq!(body.names(), ["declared_cost_micros"]);
        let encoded =
            medulla_decision_types::decision::statistical::body::encode_body(&body).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&encoded.bytes).unwrap();
        assert_eq!(json["features"][0]["kind"]["feature"], "declared_cost_micros");
        assert_eq!(json["features"][0]["missing"]["missing"], "impute");
        assert_eq!(PROCESS_CONTROL_QUESTION_IDS.len(), 2);
    }
}
