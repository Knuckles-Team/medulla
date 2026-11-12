//! The operation-audit row of one committed uncertain-effect write
//! (MEDULLA-DURABLE-KERNEL-R820.5).
//!
//! One reservation+outcome pair per start mark and per receipt: the job ID, the item revision, the
//! attempt, the outcome and the time. Never the request parameters, never a
//! credential; the revision, outcome and time are bound by `params_sha256`.

use super::*;
use medulla_work_types::work_item_read::EffectWriteResult;

/// Who did what, for one committed effect write.
pub struct EffectAudit<'a> {
    pub tenant: &'a str,
    pub principal: &'a str,
    /// `MarkWorkItemEffectStarted` or `RecordEffectReceipt`.
    pub op: &'static str,
    pub result: &'a EffectWriteResult,
    pub now_ms: u64,
}

#[cfg(feature = "security")]
use sha2::Digest;

/// The audit `request_id` is a bounded token: the readable
/// `job:attempt:outcome` when it fits the token alphabet, else its digest.
#[cfg(feature = "security")]
fn request_id(result: &EffectWriteResult) -> String {
    let readable = format!(
        "{}:{}:{}",
        result.work_item_id,
        result.attempt,
        result.outcome_label()
    );
    let fits = readable.len() <= 128
        && readable
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-/".contains(&b));
    if fits {
        readable
    } else {
        hex::encode(sha2::Sha256::digest(readable.as_bytes()))
    }
}

#[cfg(feature = "security")]
fn effect_audit_event(entry: &EffectAudit<'_>) -> medulla_store::redb_store::OperationAuditEvent {
    let result = entry.result;
    let bound = format!(
        "{}\0{}\0{}\0{}\0{}",
        result.work_item_id,
        result.attempt,
        result.revision,
        result.outcome_label(),
        entry.now_ms
    );
    medulla_store::redb_store::OperationAuditEvent {
        tenant: entry.tenant.to_string(),
        principal: entry.principal.to_string(),
        op: entry.op.to_string(),
        surface: "engine".to_string(),
        params_sha256: hex::encode(sha2::Sha256::digest(bound.as_bytes())),
        status: "ok".to_string(),
        request_id: request_id(result),
        audit_class: "event".to_string(),
        subject_iri: None,
        policy_iri: None,
    }
}

impl RedbBackend {
    /// Append the audit row of a committed effect write. The refusal of the
    /// writer is returned as it is (`AUDIT_WRITER_UNAVAILABLE` when none
    /// answered). A build without the `security` feature keeps no audit chain,
    /// so there is nothing to append.
    pub async fn audit_effect_write(
        &self,
        graph_fname: &str,
        entry: EffectAudit<'_>,
    ) -> Result<(), String> {
        // The operation audit is two-phase: an outcome closes a reservation
        // made under the same request identity, so each committed write is
        // exactly one reservation+outcome pair (MEDULLA-DURABLE-KERNEL-R820.5).
        #[cfg(feature = "security")]
        {
            let outcome = effect_audit_event(&entry);
            let reservation = medulla_store::redb_store::OperationAuditEvent {
                status: "reserved".to_string(),
                ..outcome.clone()
            };
            self.audit_append(graph_fname, reservation).await?;
            self.audit_append(graph_fname, outcome).await?;
        }
        #[cfg(not(feature = "security"))]
        let _ = (graph_fname, entry);
        Ok(())
    }
}
