//! Quality scores from outside checkers (MEDULLA-UNIFIED-DATA-PLANE-R762.8).
//!
//! A `Records` batch whose resolved mapping class is `QualityScore` carries one score per
//! record, measured by a named outside `checker` against an approved `QualityRule` of the
//! verified tenant. [`normalize_quality_score`] reads one record; [`score_methods`] scores
//! it against its rule with the same `QualityRule::score` the native evaluator uses, counts
//! consecutive failures with the same `track_failure` (a passing score resets the counter;
//! the third consecutive failing score, from any checker, reports an anomaly) and lowers the
//! score nodes, the updated rule counters and the incident nodes the streaks opened or
//! joined into graph methods that commit in the batch's own envelope.

use std::collections::{BTreeMap, BTreeSet};

use medulla_catalog_types::incident::{AnomalyReading, IncidentStatus};
use medulla_core::quality::{track_failure, Evaluation, IncidentSink, QualityAnomaly};
use medulla_protocol_types::protocol::Method;
use medulla_source_types::source_ingestion::{SourceIngestionBatch, SourceRecord};
use medulla_stream::telemetry::incident_authority::IncidentAuthority;
use medulla_stream::telemetry::incident_nodes::{
    incident_operations, incident_record, INCIDENT_TYPE,
};
use medulla_types::contract::Digest256;
use serde_json::{json, Map, Value};
use {
    medulla_catalog_source_types::quality::QualityRule,
    medulla_catalog_source_types::quality::QualityScore,
    medulla_catalog_source_types::quality::RuleState,
    medulla_catalog_source_types::quality::ScoreEvidence,
    medulla_catalog_source_types::quality::NATIVE_CHECKER,
};

use super::ResolvedIngestionMappings;

/// The mapping class whose records are outside quality scores.
pub(super) const QUALITY_SCORE_CLASS: &str = "QualityScore";
/// `QUALITY_RULE_UNKNOWN`: the score names no approved rule of the tenant.
pub const QUALITY_RULE_UNKNOWN: &str = "QUALITY_RULE_UNKNOWN";
/// `QUALITY_THRESHOLD_INVALID`: the score value lies outside 0 to 1.
pub const QUALITY_THRESHOLD_INVALID: &str = "QUALITY_THRESHOLD_INVALID";
/// `QUALITY_RULE_INVALID`: the score names no checker, or claims the native one.
const QUALITY_SCORE_INVALID: &str = "QUALITY_RULE_INVALID";

/// The stored node types and properties the native `QualityEvaluate` job writes; an
/// outside score is stored in the same shape.
const RULE_BODY_PROPERTY: &str = "rule";
const RULE_STATE_PROPERTY: &str = "state";
const RULE_FAILURES_PROPERTY: &str = "consecutive_failures";
const QUALITY_SCORE_LABEL: &str = "QualityScore";
const SCORE_OF: &str = "scoreOf";

/// Whether `ontology_class` names the outside quality score class.
pub(super) fn is_quality_class(ontology_class: &str) -> bool {
    ontology_class.trim().trim_start_matches(':') == QUALITY_SCORE_CLASS
}

/// A node id private to one tenant, the same function the rule and score writers use
/// (`medulla_h_jobs::quality_rule::tenant_node_id`).
fn tenant_node_id(prefix: &str, tenant: &str, parts: &[&str]) -> String {
    let mut key = String::from(tenant);
    for part in parts {
        key.push('\0');
        key.push_str(part);
    }
    let digest = Digest256::sha256(key.as_bytes()).to_hex();
    format!("{prefix}:{}", &digest[..32])
}

fn is_digest(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// One outside score from the mapped properties of `source` (mapped property -> source
/// field in `fields`). `passed` is decided later against the rule; `now_ms` stamps a score
/// that carries no `at`.
pub fn normalize_payload(
    source: &Map<String, Value>,
    fields: &BTreeMap<String, String>,
    now_ms: i64,
) -> Result<QualityScore, String> {
    let value_of = |target: &str| {
        fields
            .iter()
            .find(|(_, mapped)| mapped.as_str() == target)
            .and_then(|(field, _)| source.get(field))
            .filter(|value| !value.is_null())
    };
    let text_of = |target: &str| value_of(target).and_then(Value::as_str).map(str::trim);
    let checker = text_of("checker")
        .filter(|checker| !checker.is_empty())
        .ok_or_else(|| format!("{QUALITY_SCORE_INVALID}: the score names no checker"))?;
    if checker == NATIVE_CHECKER {
        return Err(format!(
            "{QUALITY_SCORE_INVALID}: `{NATIVE_CHECKER}` is the engine's own checker"
        ));
    }
    let rule = text_of("rule")
        .filter(|rule| !rule.is_empty())
        .ok_or_else(|| format!("{QUALITY_RULE_UNKNOWN}: the score names no rule"))?;
    let value = value_of("value")
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
        .ok_or_else(|| format!("{QUALITY_THRESHOLD_INVALID}: the score value is outside 0 to 1"))?;
    let count = |target: &str| value_of(target).and_then(Value::as_u64).unwrap_or(0);
    let rows_checked = count("rows_checked");
    let query_digest = match text_of("query_digest") {
        Some(digest) if is_digest(digest) => digest.to_string(),
        _ => Digest256::sha256(format!("{checker}\0{rule}").as_bytes()).to_hex(),
    };
    Ok(QualityScore {
        rule: rule.to_string(),
        value,
        passed: false,
        at: value_of("at").and_then(Value::as_i64).unwrap_or(now_ms),
        query_digest,
        rows_checked,
        rows_failed: count("rows_failed").min(rows_checked),
        checker: checker.to_string(),
    })
}

/// One outside score from one record of a `QualityScore`-class mapping.
pub fn normalize_quality_score(
    record: &SourceRecord,
    fields: &BTreeMap<String, String>,
    now_ms: i64,
) -> Result<QualityScore, String> {
    let payload = record.payload.value()?;
    let source = payload.as_object().ok_or_else(|| {
        format!("{QUALITY_SCORE_INVALID}: a quality score record is not an object")
    })?;
    normalize_payload(source, fields, now_ms)
}

/// One stored rule of the tenant with its running failure counter.
#[derive(Debug, Clone)]
pub struct StoredRule {
    pub node_id: String,
    pub properties: Map<String, Value>,
    pub rule: QualityRule,
    pub failures: u32,
}

/// The approved rule `rule_id` of `tenant` stored in `core`, or `None` when there is none
/// or it is still proposed.
fn load_rule(
    core: &medulla_graph_core::graph::GraphCore,
    tenant: &str,
    rule_id: &str,
) -> Option<StoredRule> {
    let node_id = tenant_node_id("quality_rule", tenant, &[rule_id]);
    let blob = core.get_node_properties(&node_id)?;
    let node = medulla_types::msgpack::decode_property_value(&blob).ok()?;
    let properties = node.as_object()?.clone();
    if properties.get("tenant_id").and_then(Value::as_str) != Some(tenant) {
        return None;
    }
    let rule: QualityRule =
        serde_json::from_str(properties.get(RULE_BODY_PROPERTY)?.as_str()?).ok()?;
    let state: RuleState =
        serde_json::from_value(properties.get(RULE_STATE_PROPERTY)?.clone()).ok()?;
    if state.kind() != medulla_types::quality_rule_machine::RuleStateKind::Approved {
        return None;
    }
    let failures = properties
        .get(RULE_FAILURES_PROPERTY)
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(u64::from(u32::MAX)) as u32;
    Some(StoredRule {
        node_id,
        properties,
        rule,
        failures,
    })
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

/// What scoring a batch decided: the graph methods and the anomalies its streaks reported.
#[derive(Debug, Default)]
pub struct ScoreOutcome {
    pub methods: Vec<Method>,
    pub readings: Vec<AnomalyReading>,
}

fn node_method(id: String, properties: &Value) -> Result<Method, String> {
    Ok(Method::AddNode {
        node_id: id,
        properties_msgpack: rmp_serde::to_vec_named(properties)
            .map_err(|_| "quality node encoding failed".to_string())?,
    })
}

fn edge_method(source: String, target: String, properties: &Value) -> Result<Method, String> {
    Ok(Method::AddEdge {
        source_id: source,
        target_id: target,
        properties_msgpack: rmp_serde::to_vec_named(properties)
            .map_err(|_| "quality edge encoding failed".to_string())?,
    })
}

/// Score `scores` in order against `rules` (rule id -> stored rule; a rule absent from the
/// map refuses `QUALITY_RULE_UNKNOWN`), counting each rule's consecutive failures across the
/// whole batch. Lowers one score node and one `scoreOf` edge per score and one updated rule
/// node per touched rule.
pub fn score_methods(
    tenant: &str,
    agent: &str,
    rules: &mut BTreeMap<String, StoredRule>,
    scores: Vec<QualityScore>,
    incident_after: u32,
    at_ms: u64,
) -> Result<ScoreOutcome, String> {
    let mut sink = AnomalySink {
        at_ms,
        readings: Vec::new(),
    };
    let mut methods = Vec::with_capacity(scores.len() * 2 + 1);
    let mut touched = BTreeSet::new();
    for outside in scores {
        let stored = rules.get_mut(&outside.rule).ok_or_else(|| {
            format!(
                "{QUALITY_RULE_UNKNOWN}: no approved rule `{}` for this tenant",
                outside.rule
            )
        })?;
        let evaluation = Evaluation {
            score: stored.rule.score(
                outside.value,
                outside.at,
                ScoreEvidence {
                    query_digest: outside.query_digest,
                    rows_checked: outside.rows_checked,
                    rows_failed: outside.rows_failed,
                    checker: outside.checker,
                },
            ),
        };
        stored.failures = track_failure(
            stored.failures,
            &stored.rule,
            &evaluation,
            incident_after,
            &mut sink,
        );
        let score = &evaluation.score;
        let score_id = tenant_node_id(
            "quality_score",
            tenant,
            &[&stored.rule.id, &score.checker, &score.at.to_string()],
        );
        methods.push(node_method(
            score_id.clone(),
            &json!({
                "type": QUALITY_SCORE_LABEL,
                "rule": stored.rule.id,
                "target": stored.rule.target,
                "value": score.value,
                "passed": score.passed,
                "at": score.at,
                "query_digest": score.query_digest,
                "rows_checked": score.rows_checked,
                "rows_failed": score.rows_failed,
                "checker": score.checker,
                "tenant_id": tenant,
                "_owner": agent,
            }),
        )?);
        methods.push(edge_method(
            score_id,
            stored.node_id.clone(),
            &json!({"relationship": SCORE_OF, "tenant_id": tenant, "_owner": agent}),
        )?);
        touched.insert(stored.rule.id.clone());
    }
    for rule_id in touched {
        let stored = &rules[&rule_id];
        let mut properties = stored.properties.clone();
        properties.insert(RULE_FAILURES_PROPERTY.to_string(), stored.failures.into());
        methods.push(node_method(
            stored.node_id.clone(),
            &Value::Object(properties),
        )?);
    }
    Ok(ScoreOutcome {
        methods,
        readings: sink.readings,
    })
}

/// The incident upserts the failing streaks open or join, over the tenant's unresolved
/// incidents in `core`; returns the methods and the ids of every node they write.
pub fn incident_methods(
    core: &medulla_graph_core::graph::GraphCore,
    tenant: &str,
    agent: &str,
    readings: Vec<AnomalyReading>,
) -> Result<(Vec<Method>, Vec<String>), String> {
    if readings.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let knobs = medulla_server_api::knob_config::knobs();
    let records = core
        .get_nodes_by_label(INCIDENT_TYPE, knobs.quality_max_incidents as usize)
        .into_iter()
        .filter_map(|(_, properties)| incident_record(&properties))
        .filter(|record| record.tenant == tenant && record.status != IncidentStatus::Resolved);
    let mut authority = IncidentAuthority::load(records);
    authority
        .correlate(Some(tenant), readings, knobs.quality_incident_window_ms)
        .map_err(|refusal| refusal.code().to_string())?;
    let (operations, _) = incident_operations(authority.changes(), tenant, agent)?;
    let mut methods = Vec::with_capacity(operations.len());
    let mut node_ids = Vec::new();
    for operation in &operations {
        let properties = operation.get("properties").cloned().unwrap_or(Value::Null);
        let field = |name: &str| {
            operation
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("incident operation has no `{name}`"))
        };
        match operation.get("op").and_then(Value::as_str) {
            Some("upsert_node") => {
                let id = field("id")?;
                node_ids.push(id.clone());
                methods.push(node_method(id, &properties)?);
            }
            Some("upsert_edge") => methods.push(edge_method(
                field("source")?,
                field("target")?,
                &properties,
            )?),
            _ => return Err("incident operation has an unknown op".to_string()),
        }
    }
    Ok((methods, node_ids))
}

/// The graph methods and the ids of the nodes they write that carry a policy, for every
/// outside quality score of `batch`: empty when the batch holds none.
pub(super) fn admit_outside_scores(
    core: &medulla_graph_core::graph::GraphCore,
    tenant: &str,
    agent: &str,
    batch: &SourceIngestionBatch,
    resolved: &ResolvedIngestionMappings,
    now_ms: u64,
) -> Result<(Vec<Method>, Vec<String>), String> {
    let mut scores = Vec::new();
    for record in &batch.records {
        let Some(mapping) = resolved.entities.get(&record.mapping_reference) else {
            continue;
        };
        if !is_quality_class(&mapping.schema_mapping.ontology_class) {
            continue;
        }
        scores.push(normalize_quality_score(
            record,
            &mapping.schema_mapping.fields,
            now_ms as i64,
        )?);
    }
    if scores.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut rules = BTreeMap::new();
    for score in &scores {
        if let std::collections::btree_map::Entry::Vacant(slot) = rules.entry(score.rule.clone()) {
            let stored = load_rule(core, tenant, &score.rule).ok_or_else(|| {
                format!(
                    "{QUALITY_RULE_UNKNOWN}: no approved rule `{}` for this tenant",
                    score.rule
                )
            })?;
            slot.insert(stored);
        }
    }
    let after = medulla_server_api::knob_config::knobs().quality_incident_after_failures as u32;
    let mut outcome = score_methods(tenant, agent, &mut rules, scores, after, now_ms)?;
    let (incidents, incident_ids) = incident_methods(core, tenant, agent, outcome.readings)?;
    outcome.methods.extend(incidents);
    let mut policy_ids: Vec<String> = outcome
        .methods
        .iter()
        .filter_map(|method| match method {
            Method::AddNode { node_id, .. } if !incident_ids.contains(node_id) => {
                Some(node_id.clone())
            }
            _ => None,
        })
        .collect();
    policy_ids.extend(incident_ids);
    Ok((outcome.methods, policy_ids))
}

#[cfg(test)]
mod tests {
    use super::*;
    use {
        medulla_catalog_source_types::quality::QualityDimension,
        medulla_catalog_source_types::quality::QualitySeverity,
    };

    fn fields() -> BTreeMap<String, String> {
        [("checker", "checker"), ("rule", "rule"), ("score", "value")]
            .into_iter()
            .map(|(field, property)| (field.to_string(), property.to_string()))
            .collect()
    }

    fn payload(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    fn stored(failures: u32) -> StoredRule {
        StoredRule {
            node_id: "quality_rule:1".into(),
            properties: Map::new(),
            rule: QualityRule {
                id: "r1".into(),
                target: "orders".into(),
                dimension: QualityDimension::Completeness,
                expression: "status IS NOT NULL".into(),
                threshold: 0.9,
                severity: QualitySeverity::Critical,
            },
            failures,
        }
    }

    fn outside(value: f64, at: i64) -> QualityScore {
        normalize_payload(
            &payload(json!({"checker": "dq-tool", "rule": "r1", "score": value})),
            &fields(),
            at,
        )
        .unwrap()
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R762.8
    #[test]
    fn normalize_reads_checker_rule_and_value_and_refuses_bad_ones() {
        let score = outside(0.4, 7);
        assert_eq!(
            (score.checker.as_str(), score.rule.as_str()),
            ("dq-tool", "r1")
        );
        assert_eq!((score.value, score.at), (0.4, 7));
        assert_eq!(score.query_digest.len(), 64);

        let refused = |value: Value| normalize_payload(&payload(value), &fields(), 0).unwrap_err();
        assert!(refused(json!({"checker": "t", "rule": "r1", "score": 1.5}))
            .starts_with(QUALITY_THRESHOLD_INVALID));
        assert!(
            refused(json!({"checker": "t", "rule": "r1", "score": -0.1}))
                .starts_with(QUALITY_THRESHOLD_INVALID)
        );
        assert!(refused(json!({"checker": "t", "score": 0.5})).starts_with(QUALITY_RULE_UNKNOWN));
        assert!(refused(json!({"rule": "r1", "score": 0.5})).starts_with("QUALITY_RULE_INVALID"));
        assert!(
            refused(json!({"checker": "native", "rule": "r1", "score": 0.5}))
                .starts_with("QUALITY_RULE_INVALID")
        );
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R762.8
    #[test]
    fn third_failing_outside_score_reports_one_anomaly_and_a_pass_resets() {
        let mut rules = BTreeMap::from([("r1".to_string(), stored(0))]);
        let scores = vec![outside(0.1, 1), outside(0.2, 2), outside(0.3, 3)];
        let outcome = score_methods("acme", "agent", &mut rules, scores, 3, 10).unwrap();
        assert_eq!(outcome.readings.len(), 1);
        assert_eq!(outcome.readings[0].entity_ref, "orders");
        assert_eq!(rules["r1"].failures, 3);
        // Three score nodes, three scoreOf edges and the updated rule node.
        assert_eq!(outcome.methods.len(), 7);

        let reset =
            score_methods("acme", "agent", &mut rules, vec![outside(0.95, 4)], 3, 11).unwrap();
        assert!(reset.readings.is_empty());
        assert_eq!(rules["r1"].failures, 0);

        let unknown = score_methods(
            "acme",
            "agent",
            &mut BTreeMap::new(),
            vec![outside(0.5, 5)],
            3,
            12,
        )
        .unwrap_err();
        assert!(unknown.starts_with(QUALITY_RULE_UNKNOWN));
    }
}
