//! Private implementation module for the analytics job handler: the `QualityEvaluate`
//! claim (MEDULLA-UNIFIED-DATA-PLANE-R762.3).
//!
//! The claim reads the approved `QualityRule` nodes of the job's verified tenant that
//! target the job's dataset, compiles each with `compile_rule`, measures it over the
//! target in the tenant's own table store, scores it with `evaluate_rule`, and counts
//! consecutive failures with `track_failure`. A streak of `knob
//! quality.incident_after_failures` failures records an anomaly with the incident
//! authority, which opens one incident per signature or joins the open one. The score
//! nodes, the `scoreOf` edges, the rules' updated failure counters and the incident
//! nodes commit as ONE `MutationBatch`. A rule in the `proposed` state is skipped and
//! counted in the job result as `skipped_proposed`.

use super::data_plane_write::{
    commit_job_methods, edge_method, node_method, upsert_operations_to_methods,
};
use super::prelude_jobs::*;
use super::prelude_std::*;
use super::*;
use crate::quality_rule::{
    tenant_node_id, QUALITY_RULE_LABEL, RULE_BODY_PROPERTY, RULE_FAILURES_PROPERTY,
    RULE_STATE_PROPERTY,
};
use medulla_core::quality::{
    compile_rule, evaluate_rule, track_failure, CompiledRule, Evaluation, IncidentSink,
    Measurement, QualityAnomaly,
};
use medulla_stream::telemetry::incident_authority::IncidentAuthority;
use medulla_stream::telemetry::incident_nodes::{
    incident_operations, incident_record, INCIDENT_TYPE,
};
use medulla_types::incident::{AnomalyReading, IncidentStatus};
use medulla_types::quality::{QualityRule, RuleState};
use serde_json::{Map, Value};

/// The node type of one quality score.
pub(super) const QUALITY_SCORE_LABEL: &str = "QualityScore";
/// Score → rule edge relation.
pub(super) const SCORE_OF: &str = "scoreOf";
/// The checker of an engine-measured score.
pub(super) const NATIVE_CHECKER: &str = "native";

/// The verified target of one quality claim.
pub(super) struct QualityTarget {
    pub target: String,
    pub tenant_scope: String,
    pub tenant: String,
    pub agent: String,
}

/// One stored rule of the tenant, with its node and failure counter.
struct StoredRule {
    node_id: String,
    properties: Map<String, Value>,
    rule: QualityRule,
    failures: u32,
}

/// The rules one claim evaluates, and how many proposed rules it skipped.
struct LoadedRules {
    approved: Vec<StoredRule>,
    skipped_proposed: u64,
}

fn stored_rule(
    node_id: String,
    properties: &[u8],
    target: &QualityTarget,
) -> Option<(StoredRule, bool)> {
    let node = medulla_types::msgpack::decode_property_value(properties).ok()?;
    let properties = node.as_object()?.clone();
    if properties.get("tenant_id").and_then(Value::as_str) != Some(target.tenant.as_str()) {
        return None;
    }
    let rule: QualityRule =
        serde_json::from_str(properties.get(RULE_BODY_PROPERTY)?.as_str()?).ok()?;
    if rule.target != target.target {
        return None;
    }
    let state: RuleState =
        serde_json::from_value(properties.get(RULE_STATE_PROPERTY)?.clone()).ok()?;
    let failures = properties
        .get(RULE_FAILURES_PROPERTY)
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(u64::from(u32::MAX)) as u32;
    let approved = matches!(state, RuleState::Approved { .. });
    Some((
        StoredRule {
            node_id,
            properties,
            rule,
            failures,
        },
        approved,
    ))
}

/// The tenant's rules on the claim's target, approved first-class, proposed counted.
fn load_rules(core: &medulla_graph_core::graph::GraphCore, target: &QualityTarget) -> LoadedRules {
    let limit = medulla_server_api::knob_config::knobs().quality_max_rules as usize;
    let mut loaded = LoadedRules {
        approved: Vec::new(),
        skipped_proposed: 0,
    };
    for (node_id, properties) in core.get_nodes_by_label(QUALITY_RULE_LABEL, limit) {
        match stored_rule(node_id, &properties, target) {
            Some((rule, true)) => loaded.approved.push(rule),
            Some((_, false)) => loaded.skipped_proposed += 1,
            None => {}
        }
    }
    loaded.approved.sort_by(|a, b| a.rule.id.cmp(&b.rule.id));
    loaded
}

/// Why one rule was not scored.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Unscored {
    rule: String,
    code: String,
}

/// Measure one compiled SQL rule over the tenant's native target: the two counts of the
/// compiled statement.
#[cfg(feature = "query")]
fn measure_sql(
    view: &medulla_graph_core::graph::GraphView,
    store: &medulla_table_store::tables::TableStore,
    statement: &str,
) -> Result<Measurement, String> {
    let result = medulla_sql::sql::exec_sql_typed_with_tables(view, store, statement)?;
    let row = result
        .rows
        .first()
        .ok_or_else(|| "the quality statement returned no row".to_string())?;
    let count = |index: usize| -> Result<u64, String> {
        row.get(index)
            .and_then(|value| {
                value
                    .as_u64()
                    .or_else(|| value.as_i64().map(|v| v.max(0) as u64))
                    .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
            })
            .ok_or_else(|| "the quality statement returned no count".to_string())
    };
    let checked = count(0)?;
    let passed = count(1)?.min(checked);
    Ok(Measurement::Rows {
        checked,
        failed: checked - passed,
    })
}

/// Measure one compiled rule. A SQL rule runs over the tenant's native target; a shape or
/// freshness rule has no measurement on this worker yet and is reported unscored with
/// `QUALITY_MEASURE_UNAVAILABLE` (MEDULLA-UNIFIED-DATA-PLANE-R762.9).
#[cfg(feature = "query")]
fn measure(
    view: &medulla_graph_core::graph::GraphView,
    store: &medulla_table_store::tables::TableStore,
    compiled: &CompiledRule,
) -> Result<Measurement, String> {
    match compiled {
        CompiledRule::Sql { statement } => measure_sql(view, store, statement)
            .map_err(|error| format!("QUALITY_MEASURE_UNAVAILABLE: {error}")),
        CompiledRule::Shape { .. } | CompiledRule::Freshness { .. } => {
            Err("QUALITY_MEASURE_UNAVAILABLE: this rule kind is not measured here".to_string())
        }
    }
}

/// Every approved rule measured and scored, in rule-id order, plus the rules that could
/// not be scored.
#[cfg(feature = "query")]
fn evaluate_rules(
    persist_dir: &Path,
    core: &medulla_graph_core::graph::GraphCore,
    target: &QualityTarget,
    rules: &[StoredRule],
    at: i64,
) -> Result<(Vec<Evaluation>, Vec<Unscored>), String> {
    let store =
        medulla_server_auth::sql_tables::tenant_table_store(&target.tenant_scope, persist_dir)
            .map_err(|_| {
                "QUALITY_MEASURE_UNAVAILABLE: the tenant table store cannot be opened".to_string()
            })?;
    let view = core.analysis_snapshot();
    let mut scored = Vec::with_capacity(rules.len());
    let mut unscored = Vec::new();
    for stored in rules {
        let outcome = compile_rule(&stored.rule)
            .map_err(|refusal| refusal.to_string())
            .and_then(|compiled| {
                measure(&view, &store, &compiled)
                    .map(|measurement| evaluate_rule(&stored.rule, &compiled, measurement, at))
            });
        match outcome {
            Ok(evaluation) => scored.push(evaluation),
            Err(refusal) => unscored.push(Unscored {
                rule: stored.rule.id.clone(),
                code: refusal.split(':').next().unwrap_or_default().to_string(),
            }),
        }
    }
    Ok((scored, unscored))
}

#[cfg(not(feature = "query"))]
fn evaluate_rules(
    _persist_dir: &Path,
    _core: &medulla_graph_core::graph::GraphCore,
    _target: &QualityTarget,
    _rules: &[StoredRule],
    _at: i64,
) -> Result<(Vec<Evaluation>, Vec<Unscored>), String> {
    Err("QUALITY_MEASURE_UNAVAILABLE: this worker has no native table store".to_string())
}

/// The anomalies a failing streak reports, stamped with the evaluation time.
struct AnomalySink {
    at_ms: u64,
    readings: Vec<AnomalyReading>,
}

impl IncidentSink for AnomalySink {
    fn record_anomaly(&mut self, anomaly: &QualityAnomaly) {
        self.readings.push(AnomalyReading {
            entity_ref: anomaly.entity_ref.clone(),
            signal: anomaly.signal.clone(),
            observed: anomaly.observed,
            baseline: anomaly.baseline,
            at_ms: self.at_ms,
        });
    }
}

/// What one claim decided: the graph methods to commit and the result rows.
struct QualityOutcome {
    methods: Vec<Method>,
    scores: Vec<(Evaluation, u32)>,
    incident_ids: Vec<String>,
}

/// Count each rule's streak, lower the scores, the updated rules and the incidents the
/// streaks opened or joined into graph methods.
fn quality_outcome(
    core: &medulla_graph_core::graph::GraphCore,
    target: &QualityTarget,
    rules: &[StoredRule],
    evaluations: Vec<Evaluation>,
    at_ms: u64,
) -> Result<QualityOutcome, String> {
    let knobs = medulla_server_api::knob_config::knobs();
    let incident_after = knobs.quality_incident_after_failures as u32;
    let mut sink = AnomalySink {
        at_ms,
        readings: Vec::new(),
    };
    let mut methods = Vec::with_capacity(evaluations.len() * 3);
    let mut scores = Vec::with_capacity(evaluations.len());
    for evaluation in evaluations {
        let Some(stored) = rules
            .iter()
            .find(|rule| rule.rule.id == evaluation.score.rule)
        else {
            continue;
        };
        let failures = track_failure(
            stored.failures,
            &stored.rule,
            &evaluation,
            incident_after,
            &mut sink,
        );
        let score_id = tenant_node_id(
            "quality_score",
            &target.tenant,
            &[&stored.rule.id, &evaluation.score.at.to_string()],
        );
        methods.push(node_method(
            score_id.clone(),
            &serde_json::json!({
                "type": QUALITY_SCORE_LABEL,
                "rule": stored.rule.id,
                "target": stored.rule.target,
                "value": evaluation.score.value,
                "passed": evaluation.score.passed,
                "at": evaluation.score.at,
                "query_digest": evaluation.query_digest,
                "rows_checked": evaluation.rows_checked,
                "rows_failed": evaluation.rows_failed,
                "checker": NATIVE_CHECKER,
                "tenant_id": target.tenant,
                "_owner": target.agent,
            }),
        )?);
        methods.push(edge_method(
            score_id,
            stored.node_id.clone(),
            &serde_json::json!({
                "relationship": SCORE_OF,
                "tenant_id": target.tenant,
                "_owner": target.agent,
            }),
        )?);
        let mut properties = stored.properties.clone();
        properties.insert(RULE_FAILURES_PROPERTY.to_string(), failures.into());
        methods.push(node_method(
            stored.node_id.clone(),
            &Value::Object(properties),
        )?);
        scores.push((evaluation, failures));
    }
    let incident_ids = if sink.readings.is_empty() {
        Vec::new()
    } else {
        let limit = knobs.quality_max_incidents as usize;
        let records = core
            .get_nodes_by_label(INCIDENT_TYPE, limit)
            .into_iter()
            .filter_map(|(_, properties)| incident_record(&properties))
            .filter(|record| {
                record.tenant == target.tenant && record.status != IncidentStatus::Resolved
            });
        let mut authority = IncidentAuthority::load(records);
        let touched = authority
            .correlate(
                Some(&target.tenant),
                sink.readings,
                knobs.quality_incident_window_ms,
            )
            .map_err(|refusal| refusal.code().to_string())?;
        let (operations, _) =
            incident_operations(authority.changes(), &target.tenant, &target.agent)?;
        methods.extend(upsert_operations_to_methods(&operations)?);
        touched
    };
    Ok(QualityOutcome {
        methods,
        scores,
        incident_ids,
    })
}

pub(super) async fn execute_quality_claim(
    state: &Arc<RwLock<ServerState>>,
    store: &Arc<JobStore>,
    job: &medulla_jobs::AnalyticsJob,
    worker_ref: &str,
    epoch: u64,
    target: QualityTarget,
) -> Result<(), String> {
    store
        .checkpoint_fenced(
            &job.job_id,
            worker_ref,
            epoch,
            Checkpoint {
                progress: 0.1,
                stage: "evaluating".to_string(),
                state_blob: None,
                updated_at_ms: unix_ms(),
            },
            unix_ms(),
        )
        .map_err(|error| error.to_string())?;
    let (_, _, core) = resolve_core_ref(state, &job.input_snapshot.graph)
        .await
        .ok_or_else(|| "analytics target graph is unavailable".to_string())?;
    let persist_dir = state
        .read()
        .await
        .persist_dir
        .clone()
        .map(std::path::PathBuf::from);
    let Some(persist_dir) = persist_dir else {
        let _ = store.fail_attempt_fenced(
            &job.job_id,
            worker_ref,
            epoch,
            "QUALITY_MEASURE_UNAVAILABLE",
            unix_ms(),
        );
        return Ok(());
    };
    let rules = load_rules(&core, &target);
    let at = unix_ms();
    let measured_core = Arc::clone(&core);
    let (measured, target, rules) = tokio::task::spawn_blocking(move || {
        let measured = evaluate_rules(&persist_dir, &measured_core, &target, &rules.approved, at);
        (measured, target, rules)
    })
    .await
    .map_err(|_| "quality kernel failed".to_string())?;
    let (evaluations, unscored) = match measured {
        Ok(measured) => measured,
        Err(refusal) => {
            let code = refusal
                .split(':')
                .next()
                .unwrap_or("QUALITY_MEASURE_UNAVAILABLE");
            let _ = store.fail_attempt_fenced(&job.job_id, worker_ref, epoch, code, unix_ms());
            return Ok(());
        }
    };
    let outcome = quality_outcome(
        &core,
        &target,
        &rules.approved,
        evaluations,
        at.max(0) as u64,
    )?;
    commit_job_methods(state, job, "quality-evaluate", outcome.methods).await?;
    let now = unix_ms();
    store
        .checkpoint_fenced(
            &job.job_id,
            worker_ref,
            epoch,
            Checkpoint {
                progress: 0.9,
                stage: "scored".to_string(),
                state_blob: None,
                updated_at_ms: now,
            },
            now,
        )
        .map_err(|error| error.to_string())?;
    let result = typed_quality_result(
        job,
        &target,
        &outcome.scores,
        &unscored,
        &outcome.incident_ids,
        rules.skipped_proposed,
    )?;
    let staged = store
        .stage_result_fenced(&job.job_id, worker_ref, epoch, result, unix_ms())
        .map_err(|error| error.to_string())?;
    publish_staged_result(state, store, staged, worker_ref, epoch).await
}

/// One result row per rule: its score and evidence, or why it was not scored. Every row
/// carries the job's `skipped_proposed` count and the incidents the run opened or joined.
fn typed_quality_result(
    job: &medulla_jobs::AnalyticsJob,
    target: &QualityTarget,
    scores: &[(Evaluation, u32)],
    unscored: &[Unscored],
    incident_ids: &[String],
    skipped_proposed: u64,
) -> Result<TypedJobResult, String> {
    let dataset = job.input_snapshot.dataset_ref.clone();
    let row = |rule: &str, outcome: Vec<(&str, Value)>| {
        let mut row = BTreeMap::from([
            (
                "id".to_string(),
                serde_json::json!(format!("quality_run:{}:{rule}", job.job_id)),
            ),
            ("kind".to_string(), serde_json::json!("quality_score")),
            ("confidence".to_string(), serde_json::json!(1.0)),
            ("evidence_refs".to_string(), serde_json::json!([dataset])),
            ("source_refs".to_string(), serde_json::json!([dataset])),
            ("proof_ids".to_string(), serde_json::json!([])),
            ("contradiction_ids".to_string(), serde_json::json!([])),
            ("target".to_string(), serde_json::json!(target.target)),
            ("rule".to_string(), serde_json::json!(rule)),
            (
                "skipped_proposed".to_string(),
                serde_json::json!(skipped_proposed),
            ),
            ("incident_ids".to_string(), serde_json::json!(incident_ids)),
        ]);
        for (name, value) in outcome {
            row.insert(name.to_string(), value);
        }
        row
    };
    let mut rows: Vec<BTreeMap<String, Value>> = scores
        .iter()
        .map(|(evaluation, failures)| {
            row(
                &evaluation.score.rule,
                vec![
                    ("value", serde_json::json!(evaluation.score.value)),
                    ("passed", serde_json::json!(evaluation.score.passed)),
                    ("consecutive_failures", serde_json::json!(failures)),
                    ("query_digest", serde_json::json!(evaluation.query_digest)),
                    ("rows_checked", serde_json::json!(evaluation.rows_checked)),
                    ("rows_failed", serde_json::json!(evaluation.rows_failed)),
                    ("code", Value::Null),
                ],
            )
        })
        .collect();
    rows.extend(unscored.iter().map(|unscored| {
        row(
            &unscored.rule,
            vec![
                ("value", Value::Null),
                ("passed", Value::Null),
                ("consecutive_failures", Value::Null),
                ("query_digest", Value::Null),
                ("rows_checked", Value::Null),
                ("rows_failed", Value::Null),
                ("code", serde_json::json!(unscored.code)),
            ],
        )
    }));
    if rows.is_empty() {
        rows.push(row(
            "",
            vec![
                ("value", Value::Null),
                ("passed", Value::Null),
                ("consecutive_failures", Value::Null),
                ("query_digest", Value::Null),
                ("rows_checked", Value::Null),
                ("rows_failed", Value::Null),
                ("code", Value::Null),
            ],
        ));
    }
    TypedJobResult::new(
        [
            ("id", "string", false),
            ("kind", "string", false),
            ("confidence", "float64", false),
            ("evidence_refs", "list<string>", false),
            ("source_refs", "list<string>", false),
            ("proof_ids", "list<string>", false),
            ("contradiction_ids", "list<string>", false),
            ("target", "string", false),
            ("rule", "string", false),
            ("skipped_proposed", "int64", false),
            ("incident_ids", "list<string>", false),
            ("value", "float64", true),
            ("passed", "bool", true),
            ("consecutive_failures", "int64", true),
            ("query_digest", "string", true),
            ("rows_checked", "int64", true),
            ("rows_failed", "int64", true),
            ("code", "string", true),
        ]
        .into_iter()
        .map(|(name, logical_type, nullable)| ResultColumn {
            name: name.to_string(),
            logical_type: logical_type.to_string(),
            nullable,
        })
        .collect(),
        rows,
        vec![dataset],
        Vec::new(),
        None,
        None,
        reproducibility_manifest(&job.input_snapshot, &job.algo, &job.policy),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::quality::{QualityDimension, QualitySeverity};

    fn target() -> QualityTarget {
        QualityTarget {
            target: "orders".into(),
            tenant_scope: "scope".into(),
            tenant: "acme".into(),
            agent: "agent-1".into(),
        }
    }

    fn rule_node(tenant: &str, target: &str, state: RuleState, failures: u32) -> Vec<u8> {
        let rule = QualityRule {
            id: "r1".into(),
            target: target.into(),
            dimension: QualityDimension::Completeness,
            expression: "email IS NOT NULL".into(),
            threshold: 0.9,
            severity: QualitySeverity::Critical,
        };
        rmp_serde::to_vec_named(&serde_json::json!({
            "type": QUALITY_RULE_LABEL,
            RULE_BODY_PROPERTY: serde_json::to_string(&rule).unwrap(),
            RULE_STATE_PROPERTY: serde_json::to_value(&state).unwrap(),
            RULE_FAILURES_PROPERTY: failures,
            "tenant_id": tenant,
        }))
        .unwrap()
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R762.3
    #[test]
    fn only_the_tenants_rules_on_the_target_load_and_proposed_ones_are_counted() {
        let approved = RuleState::Approved {
            approver: "steward".into(),
        };
        let (rule, active) = stored_rule(
            "n1".into(),
            &rule_node("acme", "orders", approved.clone(), 2),
            &target(),
        )
        .unwrap();
        assert!(active);
        assert_eq!(rule.failures, 2);
        let (_, active) = stored_rule(
            "n2".into(),
            &rule_node("acme", "orders", RuleState::Proposed, 0),
            &target(),
        )
        .unwrap();
        assert!(!active);
        assert!(stored_rule(
            "n3".into(),
            &rule_node("globex", "orders", approved.clone(), 0),
            &target()
        )
        .is_none());
        assert!(stored_rule(
            "n4".into(),
            &rule_node("acme", "customers", approved, 0),
            &target()
        )
        .is_none());
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R762.3
    #[test]
    fn the_third_failure_opens_one_incident_in_the_same_batch() {
        let core = medulla_graph_core::graph::GraphCore::new();
        let approved = RuleState::Approved {
            approver: "steward".into(),
        };
        let (stored, _) = stored_rule(
            "n1".into(),
            &rule_node("acme", "orders", approved, 2),
            &target(),
        )
        .unwrap();
        let compiled = compile_rule(&stored.rule).unwrap();
        let failing = evaluate_rule(
            &stored.rule,
            &compiled,
            Measurement::Rows {
                checked: 10,
                failed: 5,
            },
            7,
        );
        let outcome = quality_outcome(
            &core,
            &target(),
            std::slice::from_ref(&stored),
            vec![failing],
            7,
        )
        .unwrap();
        assert_eq!(outcome.scores[0].1, 3);
        assert_eq!(outcome.incident_ids.len(), 1);
        let incident_nodes = outcome
            .methods
            .iter()
            .filter(|method| matches!(method, Method::AddNode { node_id, .. } if node_id == &outcome.incident_ids[0]))
            .count();
        assert_eq!(incident_nodes, 1);
    }
}
