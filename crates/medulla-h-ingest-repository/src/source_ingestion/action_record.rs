//! Admission of `action_records` batches (MEDULLA-DECISION-ENGINE-R869.2).
//!
//! An action record is append-only provenance. Every check runs before any write, and a
//! refusal refuses the whole batch. A fact is pinned by node, graph version and the digest of
//! the node's property blob at that version, so a record can never name a fact image the graph
//! did not hold.

use std::collections::BTreeMap;

use medulla_core::approval_machine::ApprovalStatus;
use medulla_graph_core::graph::{GraphCore, NodeHistoryError};
use medulla_protocol_types::protocol::Method;
use medulla_source_types::source_action_record::{ActionRecordItem, FactUseItem};
use medulla_source_types::source_ingestion::SourceJson;
use medulla_types::contract::{Digest256, ServerErrorCode};
use medulla_types::transition_refused::MachineState;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// The digest domain of an action record identity.
const ACTION_DOMAIN: &[u8] = b"eg/action-record/v1";

/// The nodes `admit_actions` reads: the held nodes of the batch's records, the records they
/// are `caused_by`, the gate decisions that approve them and the approval requests those name.
pub struct ActionReads {
    /// Held node id to its properties.
    pub held: BTreeMap<String, Value>,
    /// The request graph, which answers `node_properties_at`.
    pub core: std::sync::Arc<GraphCore>,
}

/// The methods of one admitted `action_records` batch, in write order.
#[derive(Debug, Default)]
pub struct ActionAdmission {
    /// Nodes first, then edges.
    pub methods: Vec<Method>,
    /// The nodes written: each takes one policy record.
    pub node_ids: Vec<String>,
    /// The record node id of every item, in batch order; a repeat names the held node.
    pub record_ids: Vec<String>,
}

fn refuse(code: ServerErrorCode, detail: impl std::fmt::Display) -> String {
    format!("{}: {detail}", code.as_str())
}

fn invalid(detail: impl std::fmt::Display) -> String {
    refuse(ServerErrorCode::ActionRecordInvalid, detail)
}

/// Decode the elements of an `action_records` extension.
pub fn decode_action_items(items: &[SourceJson]) -> Result<Vec<ActionRecordItem>, String> {
    items
        .iter()
        .map(|item| serde_json::from_value::<ActionRecordItem>(item.value()?).map_err(invalid))
        .collect()
}

/// The node id of the record `key` names under `tenant`: `action:` plus the framed digest.
pub fn action_node_id(tenant: &str, key: &str) -> Result<String, String> {
    Ok(format!(
        "action:{}",
        Digest256::framed(ACTION_DOMAIN, &[tenant.as_bytes(), key.as_bytes()])?.to_hex()
    ))
}

/// SHA-256 over the item's JSON in struct field order.
pub fn item_digest(item: &ActionRecordItem) -> Result<Digest256, String> {
    let bytes = serde_json::to_vec(item).map_err(invalid)?;
    Ok(Digest256::from_bytes(Sha256::digest(bytes).into()))
}

fn prefixed(digest: &Digest256) -> String {
    format!("sha256:{}", digest.to_hex())
}

/// The form `sha256:` plus 64 lowercase hex.
fn is_digest_text(text: &str) -> bool {
    text.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// The node ids the first read of [`admit_actions`] needs: each record's own node, the record
/// it is `caused_by` and the gate decision it names.
pub fn first_read_ids(
    tenant: &str,
    items: &[ActionRecordItem],
) -> Result<std::collections::BTreeSet<String>, String> {
    let mut ids = std::collections::BTreeSet::new();
    for item in items {
        if item.idempotency_key.is_empty() {
            continue;
        }
        ids.insert(action_node_id(tenant, &item.idempotency_key)?);
        ids.extend(item.caused_by.iter().cloned());
        ids.extend(item.approved_by.iter().cloned());
    }
    Ok(ids)
}

/// The approval request a gate decision decides: the target of its `decisionRequest` edge.
/// The graph is the authority; the decision node carries no request id property.
pub fn decision_request(core: &GraphCore, gate: &str) -> Option<String> {
    core.get_successors(gate).ok()?.into_iter().find(|target| {
        core.get_edge_properties(gate, target).iter().any(|bytes| {
            rmp_serde::from_slice::<Value>(bytes)
                .ok()
                .is_some_and(|edge| {
                    edge.get("relationship").and_then(Value::as_str) == Some("decisionRequest")
                })
        })
    })
}

/// The approval requests the gate decisions name by `decisionRequest` edge: the second read.
pub fn second_read_ids(
    core: &GraphCore,
    items: &[ActionRecordItem],
) -> std::collections::BTreeSet<String> {
    items
        .iter()
        .filter_map(|item| item.approved_by.as_ref())
        .filter_map(|gate| decision_request(core, gate))
        .collect()
}

fn type_of(node: &Value) -> Option<&str> {
    node.get("type").and_then(Value::as_str)
}

fn check_shape(item: &ActionRecordItem, held: &BTreeMap<String, Value>) -> Result<(), String> {
    if item.idempotency_key.is_empty() || item.actor.is_empty() {
        return Err(invalid(
            "an action record needs an idempotency key and an actor",
        ));
    }
    let no_inputs = item
        .no_inputs
        .as_deref()
        .is_some_and(|reason| !reason.is_empty());
    if item.used_facts.is_empty() && !no_inputs {
        return Err(invalid("a record with no used fact states no_inputs"));
    }
    for fact in &item.used_facts {
        if !is_digest_text(&fact.content_digest) {
            return Err(invalid(format!(
                "fact {} digest is not sha256: plus 64 lowercase hex",
                fact.node_id
            )));
        }
    }
    for rule in &item.used_rules {
        if !is_digest_text(&rule.digest) {
            return Err(invalid(format!(
                "rule {} digest is not sha256: plus 64 lowercase hex",
                rule.rule_id
            )));
        }
    }
    if let Some(cause) = &item.caused_by {
        if held.get(cause).and_then(type_of) != Some("ActionRecord") {
            return Err(invalid(format!(
                "caused_by {cause} names no held ActionRecord"
            )));
        }
    }
    Ok(())
}

fn check_facts(core: &GraphCore, facts: &[FactUseItem]) -> Result<(), String> {
    for fact in facts {
        let unknown = |why: &str| {
            refuse(
                ServerErrorCode::ActionFactRevisionUnknown,
                format!("{} at revision {}: {why}", fact.node_id, fact.revision),
            )
        };
        match core.node_properties_at(&fact.node_id, fact.revision) {
            Ok(Some(blob)) => {
                let digest = format!("sha256:{}", hex::encode(Sha256::digest(blob.as_slice())));
                if digest != fact.content_digest {
                    return Err(unknown("the digest differs"));
                }
            }
            Ok(None) => return Err(unknown("the node did not exist")),
            Err(NodeHistoryError::Future { .. }) => {
                return Err(unknown("the revision is in the future"))
            }
            Err(NodeHistoryError::Retired { .. }) => {
                return Err(unknown("the revision is retired"))
            }
        }
    }
    Ok(())
}

fn check_approval(
    item: &ActionRecordItem,
    held: &BTreeMap<String, Value>,
    core: &GraphCore,
) -> Result<(), String> {
    if !item.action_kind.needs_approval() {
        return Ok(());
    }
    let required = |why: &str| {
        refuse(
            ServerErrorCode::ActionApprovalRequired,
            format!(
                "{} needs an approved gate decision: {why}",
                item.action_kind.as_str()
            ),
        )
    };
    let gate = item
        .approved_by
        .as_ref()
        .and_then(|id| held.get(id))
        .filter(|node| type_of(node) == Some("GateDecision"))
        .ok_or_else(|| required("approved_by names no held GateDecision"))?;
    if gate.get("gateOutcome").and_then(Value::as_str) != Some("approve") {
        return Err(required("the gate outcome is not approve"));
    }
    let gate_id = item.approved_by.as_deref().unwrap_or_default();
    let request = decision_request(core, gate_id)
        .and_then(|id| held.get(&id))
        .filter(|node| type_of(node) == Some("ApprovalRequest"))
        .ok_or_else(|| required("the decision names no held ApprovalRequest"))?;
    if request.get("requestStatus").and_then(Value::as_str)
        != Some(ApprovalStatus::Approved.stored())
    {
        return Err(required("the request is not approved"));
    }
    if request.get("requestActionKind").and_then(Value::as_str) != Some(item.action_kind.as_str()) {
        return Err(required("the request is for another action kind"));
    }
    Ok(())
}

/// The held properties of each node in `ids`; an absent node has no entry.
pub async fn read_held_nodes(
    persistence: &dyn medulla_server_api::api::persistence::PersistenceBackend,
    graph_fname: &str,
    ids: &std::collections::BTreeSet<String>,
) -> Result<BTreeMap<String, Value>, String> {
    let mut held = BTreeMap::new();
    for id in ids {
        let Some(bytes) = persistence.read_node(graph_fname, id).await? else {
            continue;
        };
        let properties: Value = rmp_serde::from_slice(&bytes)
            .map_err(|error| invalid(format!("held node {id}: {error}")))?;
        held.insert(id.clone(), properties);
    }
    Ok(held)
}

fn encode(value: &Value) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(value).map_err(invalid)
}

fn edge(source: &str, target: &str, label: &str) -> Result<Method, String> {
    Ok(Method::AddEdge {
        source_id: source.to_string(),
        target_id: target.to_string(),
        properties_msgpack: encode(&json!({ "relationship": label }))?,
    })
}

fn record_methods(
    tenant: &str,
    id: &str,
    item: &ActionRecordItem,
    digest: &str,
    admission: &mut ActionAdmission,
) -> Result<(), String> {
    let mut properties = json!({
        "type": "ActionRecord",
        "actionKind": item.action_kind.as_str(),
        "actor": item.actor,
        "idempotencyKey": item.idempotency_key,
        "content_digest": digest,
        "tenant_id": tenant,
    });
    if let Some(outcome) = &item.outcome {
        properties["actionOutcome"] = json!(outcome);
    }
    if let Some(reason) = &item.no_inputs {
        properties["noInputs"] = json!(reason);
    }
    admission.methods.push(Method::AddNode {
        node_id: id.to_string(),
        properties_msgpack: encode(&properties)?,
    });
    admission.node_ids.push(id.to_string());
    let mut edges = Vec::new();
    for (n, fact) in item.used_facts.iter().enumerate() {
        let fact_id = format!("{id}:fact:{n}");
        admission.methods.push(Method::AddNode {
            node_id: fact_id.clone(),
            properties_msgpack: encode(&json!({
                "type": "FactUse",
                "factNode": fact.node_id,
                "factRevision": fact.revision,
                "factDigest": fact.content_digest,
                "tenant_id": tenant,
            }))?,
        });
        admission.node_ids.push(fact_id.clone());
        edges.push(edge(id, &fact_id, "usedFact")?);
    }
    for (n, rule) in item.used_rules.iter().enumerate() {
        let rule_id = format!("{id}:rule:{n}");
        admission.methods.push(Method::AddNode {
            node_id: rule_id.clone(),
            properties_msgpack: encode(&json!({
                "type": "RuleUse",
                "ruleNode": rule.rule_id,
                "ruleVersion": rule.version,
                "ruleDigest": rule.digest,
                "tenant_id": tenant,
            }))?,
        });
        admission.node_ids.push(rule_id.clone());
        edges.push(edge(id, &rule_id, "usedRule")?);
    }
    if let Some(gate) = &item.approved_by {
        edges.push(edge(id, gate, "approvedBy")?);
    }
    if let Some(cause) = &item.caused_by {
        edges.push(edge(id, cause, "causedBy")?);
    }
    admission.methods.extend(edges);
    Ok(())
}

/// Admit one `action_records` batch. A refusal admits nothing; an item whose held node has the
/// same digest is an idempotent repeat and emits no method.
pub fn admit_actions(
    tenant: &str,
    items: &[ActionRecordItem],
    reads: &ActionReads,
) -> Result<ActionAdmission, String> {
    let mut admission = ActionAdmission::default();
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for item in items {
        check_shape(item, &reads.held)?;
        let id = action_node_id(tenant, &item.idempotency_key)?;
        let digest = prefixed(&item_digest(item)?);
        let held_digest = reads
            .held
            .get(&id)
            .map(|node| {
                node.get("content_digest")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .or_else(|| seen.get(&id).cloned());
        admission.record_ids.push(id.clone());
        match held_digest {
            Some(held) if held == digest => continue,
            Some(_) => {
                return Err(refuse(
                    ServerErrorCode::ActionIdempotencyConflict,
                    format!("key {} is held with another digest", item.idempotency_key),
                ))
            }
            None => {}
        }
        check_facts(&reads.core, item.used_facts.as_slice())?;
        check_approval(item, &reads.held, &reads.core)?;
        seen.insert(id.clone(), digest.clone());
        record_methods(tenant, &id, item, &digest, &mut admission)?;
    }
    Ok(admission)
}
