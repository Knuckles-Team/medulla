//! The audit record of one shape-policy evaluation (MEDULLA-FEDERATED-QUERY-R706.11), the one
//! writer every query language shares (MEDULLA-FEDERATED-QUERY-R706.12).
//!
//! The UQL and SQL reads of `medulla-h-query` and the SPARQL read of this crate each hand
//! their evaluation here, so a record has one shape wherever the statement came from.

use medulla_exec::policy_mask::PolicyEvaluation;
use medulla_types::contract::ServerErrorCode;

/// Queue the audit record of one evaluation on the read-audit chain, once per evaluation and
/// settled at once: the digest of the policy set, the principal, and the counts of `allowed`,
/// `filtered` and `violation` results, with no node id and no node value.
/// `AUDIT_UNAVAILABLE` when the audit queue is full.
pub fn append_policy_evaluation(
    persistence: Option<
        std::sync::Arc<dyn medulla_server_api::api::persistence::PersistenceBackend>,
    >,
    caller: &medulla_server_api::read_scope::ReadCaller,
    graph_name: &str,
    req_id: u64,
    evaluation: &PolicyEvaluation,
) -> Result<(), ServerErrorCode> {
    use sha2::{Digest, Sha256};

    let Some(persistence) = persistence else {
        return Ok(());
    };
    let counts = evaluation.counts;
    let line = serde_json::json!({
        "policy_digest": evaluation.policy_digest,
        "principal": evaluation.principal,
        "tenant": caller.principal.tenant,
        "allowed": counts.allowed,
        "filtered": counts.filtered,
        "violation": counts.violation,
    });
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let event = medulla_store::redb_store::OperationAuditEvent {
        tenant: caller.principal.tenant.clone(),
        principal: caller.principal.id.clone(),
        op: format!(
            "policy_evaluation.allowed-{}.filtered-{}.violation-{}",
            counts.allowed, counts.filtered, counts.violation
        ),
        surface: "read".to_owned(),
        params_sha256: hex::encode(Sha256::digest(line.to_string().as_bytes())),
        status: medulla_store::redb_store::audit_status::AuditStatus::settled(
            medulla_store::redb_store::audit_machine::AuditEvent::Succeed,
        ),
        request_id: format!("policy-{req_id}-{nanos}"),
        audit_class: "event".to_owned(),
        subject_iri: None,
        policy_iri: None,
        evidence: Default::default(),
    };
    let chain = medulla_store::redb_store::read_audit_chain_scope(
        &event.tenant,
        &medulla_kernel::persist::sanitize(graph_name),
    );
    medulla_server_api::read_audit::ReadAuditSink::global().enqueue(Box::pin(async move {
        use medulla_persistence::AsRedb;
        match persistence.as_redb() {
            Some(redb) => redb.audit_append(&chain, event).await.map(|_| ()),
            None => Ok(()),
        }
    }))
}
