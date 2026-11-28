//! Retention of logged decision inputs (EH-060, DECIDE-LAYER-DESIGN §4.5).
//!
//! A committed record is digest-pinned evidence, so its digest is never
//! recomputed. Compaction moves the bulky feature matrix into one engine-owned
//! Blob CAS body keyed by its content digest; the record keeps its original
//! digest and its matrix field becomes a blob pin, and the entry records
//! `Compacted`. Verification restores the matrix from CAS, re-checks the
//! record digest and replays the decision. Past `drop_blob_after_ms` the blob
//! holder is released and verification answers `INPUTS_RETIRED` -- the digest
//! is still the attested one, but nothing can be re-derived. A graph-sourced
//! summary (the record minus its matrix) is kept until its retirement age
//! reaches `retention.graph_summary_expire_after` (default 90 days); then the
//! entry and every row joined to it are deleted (R066.4). Library-sourced
//! entries are kept for ever (R066.1).

use super::stat_executor::ExecutionContext;
use super::stat_log_reader::MAX_LOG_ROWS;
use super::stat_resolve::resolution_prefix;
use super::stat_support::{refusal, resolve_policy};
use medulla_agent_types::agent_component::DecisionPolicyRef;
use medulla_decision_policy_types::decision::knob_families::retention;
use medulla_decision_statistical_types::decision::knobs::KnobValue;
use medulla_decision_statistical_types::decision::StatisticalPolicy;
use medulla_persistence_records::decision_jobs::decode_artifact;
use medulla_persistence_records::decision_jobs::encode_artifact;
use medulla_persistence_records::decision_jobs::{evaluation_key, record_key};
use medulla_server_blob::blob::BlobStateExt;
use {
    medulla_decision_program_types::decision::statistical::body::canonical_body_bytes,
    medulla_decision_program_types::decision::statistical::body::content_digest_of,
};
use {
    medulla_decision_statistical_types::decision::statistical::errors::StatisticalErrorCode,
    medulla_decision_types::decision::statistical::FeatureMatrixRef,
    medulla_decision_types::decision::statistical::StatisticalDecisionRecord,
};
use {
    medulla_decision_types::decision::statistical::log::DecisionLogCompacted,
    medulla_decision_types::decision::statistical::log::DecisionLogEntry,
    medulla_decision_types::decision::statistical::log::EntryInputs,
    medulla_decision_types::decision::statistical::log::InputsBlob,
    medulla_decision_types::decision::statistical::log::MAX_COMPACT_PER_CALL,
};

/// The blob store and the authority its holder changes are made under.
pub struct Retention {
    #[cfg(feature = "blob")]
    pub(super) blob: Option<std::sync::Arc<medulla_server_blob::blob::BlobCursors>>,
    #[cfg(feature = "blob")]
    pub(super) authority: Option<medulla_server_api::carrier::CarrierAuthority>,
}

impl Retention {
    /// No blob access: compacted inputs cannot be restored through it.
    pub fn none() -> Self {
        Self {
            #[cfg(feature = "blob")]
            blob: None,
            #[cfg(feature = "blob")]
            authority: None,
        }
    }

    /// The served blob store and the caller's carrier authority.
    pub fn of(
        state: &medulla_server_api::state::ServerState,
        verified: &medulla_server_api::authority_context::VerifiedRequestContext,
    ) -> Self {
        #[cfg(not(feature = "blob"))]
        let _ = (state, verified);
        Self {
            #[cfg(feature = "blob")]
            blob: state.blob_cursors().cloned(),
            #[cfg(feature = "blob")]
            authority: medulla_server_api::carrier::CarrierAuthority::from_verified(verified).ok(),
        }
    }
}

/// A logged record with its full inputs: inline as stored, restored from CAS
/// when compacted, or `None` when retention retired them.
pub fn full_record(
    retention: &Retention,
    tenant_id: &str,
    entry: &DecisionLogEntry,
) -> Result<Option<StatisticalDecisionRecord>, String> {
    match &entry.inputs {
        EntryInputs::Inline => Ok(Some(entry.record.as_ref().clone())),
        EntryInputs::Compacted { blob, .. } => {
            restored(retention, tenant_id, &entry.record, blob).map(Some)
        }
        EntryInputs::Retired { .. } => Ok(None),
    }
}

fn unavailable() -> String {
    refusal(
        StatisticalErrorCode::ParameterInvalid,
        "decision-log retention needs the engine's Blob CAS",
    )
}

#[cfg(feature = "blob")]
fn store_inputs(
    retention: &Retention,
    tenant_id: &str,
    body: Vec<u8>,
    now_ms: u64,
) -> Result<InputsBlob, String> {
    use sha2::{Digest, Sha256};
    let blob = retention.blob.as_ref().ok_or_else(unavailable)?;
    let sha256 = medulla_types::contract::Digest256::from_bytes(Sha256::digest(&body).into());
    let engine_body = medulla_server_blob::blob::engine_bodies::EngineBody { sha256, body };
    let stored = blob
        .store
        .put_engine_bodies(tenant_id, &[engine_body], now_ms)?;
    let stored = stored.into_iter().next().ok_or_else(unavailable)?;
    Ok(InputsBlob {
        sha256: format!("sha256:{}", stored.sha256.to_hex()),
        manifest_digest: stored.manifest_digest,
        holder_id: stored.holder_id,
        length: stored.length,
    })
}

#[cfg(feature = "blob")]
fn load_inputs(
    retention: &Retention,
    tenant_id: &str,
    pin: &InputsBlob,
) -> Result<Vec<u8>, String> {
    let blob = retention.blob.as_ref().ok_or_else(unavailable)?;
    let sha256 =
        medulla_types::contract::Digest256::parse(pin.sha256.trim_start_matches("sha256:"))?;
    medulla_server_blob::blob::engine_bodies::read_engine_body(
        blob.store.as_ref(),
        tenant_id,
        &pin.manifest_digest,
        sha256,
        pin.length,
    )
}

#[cfg(feature = "blob")]
fn release_inputs(retention: &Retention, pin: &InputsBlob, request_id: u64) -> Result<(), String> {
    use medulla_server_blob::blob::store::HolderChange;
    use medulla_server_blob::blob::store::HolderId;
    let (Some(blob), Some(authority)) = (retention.blob.as_ref(), retention.authority.as_ref())
    else {
        return Err(unavailable());
    };
    let method = medulla_protocol_types::protocol::Method::BlobUnref {
        digest: pin.manifest_digest.clone(),
    };
    let (batch, now) = medulla_server_ops::api::ingest::compile_blob_batch(
        blob.store.as_ref(),
        request_id,
        authority,
        &method,
    )?;
    let change = HolderChange::release(&pin.manifest_digest, HolderId::new(&pin.holder_id)?)?;
    blob.store.holder_batch(&change, &batch, now).map(|_| ())
}

#[cfg(not(feature = "blob"))]
fn store_inputs(_: &Retention, _: &str, _: Vec<u8>, _: u64) -> Result<InputsBlob, String> {
    Err(unavailable())
}

#[cfg(not(feature = "blob"))]
fn load_inputs(_: &Retention, _: &str, _: &InputsBlob) -> Result<Vec<u8>, String> {
    Err(unavailable())
}

#[cfg(not(feature = "blob"))]
fn release_inputs(_: &Retention, _: &InputsBlob, _: u64) -> Result<(), String> {
    Err(unavailable())
}

/// Which retention step one entry is due for, if any.
enum Step {
    Compact,
    Retire(InputsBlob, u64),
    /// Delete the entry and every row joined to it.
    Expire,
}

/// The retirement age at which a graph summary expires: the pinned knob, or
/// its default (R066.4).
fn expire_after_ms(policy: &medulla_decision_statistical_types::decision::DecisionPolicy) -> u64 {
    let spec = &retention::KNOBS[retention::GRAPH_SUMMARY_EXPIRE_AFTER];
    match policy.knob(spec) {
        KnobValue::DurationMs(ms) => ms,
        _ => match spec.default {
            KnobValue::DurationMs(ms) => ms,
            _ => u64::MAX,
        },
    }
}

fn due(
    entry: &DecisionLogEntry,
    policy: &StatisticalPolicy,
    expire_after_ms: u64,
    now_ms: u64,
) -> Option<Step> {
    // Library evidence remains reproducible indefinitely, regardless of policy.
    if matches!(
        entry.record.candidate_source,
        medulla_decision_types::decision::record::CandidateSourceRecord::AgentLibrary { .. }
    ) {
        return None;
    }
    let inline = matches!(
        entry.record.inputs.feature_matrix,
        FeatureMatrixRef::Inline { .. }
    );
    match &entry.inputs {
        EntryInputs::Inline => {
            let age = now_ms.saturating_sub(entry.committed_at_ms);
            (inline && policy.compact_after_ms.is_some_and(|after| age >= after))
                .then_some(Step::Compact)
        }
        EntryInputs::Compacted {
            blob,
            compacted_at_ms,
        } => {
            let age = now_ms.saturating_sub(*compacted_at_ms);
            policy
                .drop_blob_after_ms
                .is_some_and(|after| age >= after)
                .then(|| Step::Retire(blob.clone(), *compacted_at_ms))
        }
        EntryInputs::Retired { retired_at_ms, .. } => {
            let age = now_ms.saturating_sub(*retired_at_ms);
            (age >= expire_after_ms).then_some(Step::Expire)
        }
    }
}

fn matrix_pin(matrix: &FeatureMatrixRef, blob: &InputsBlob) -> FeatureMatrixRef {
    let (rows, columns) = match matrix {
        FeatureMatrixRef::Inline {
            candidate_ids,
            feature_names,
            ..
        } => (candidate_ids.len(), feature_names.len()),
        FeatureMatrixRef::Blob { rows, columns, .. } => (*rows as usize, *columns as usize),
    };
    FeatureMatrixRef::Blob {
        sha256: blob.sha256.clone(),
        length: blob.length,
        rows: rows as u32,
        columns: columns as u32,
    }
}

fn apply(
    ctx: &ExecutionContext,
    retention: &Retention,
    mut entry: DecisionLogEntry,
    step: Step,
) -> Result<Option<DecisionLogEntry>, String> {
    match step {
        Step::Compact => {
            let body = canonical_body_bytes(&entry.record.inputs.feature_matrix)?;
            let blob = store_inputs(retention, ctx.tenant_id, body, ctx.now_ms)?;
            entry.record.inputs.feature_matrix =
                matrix_pin(&entry.record.inputs.feature_matrix, &blob);
            entry.inputs = EntryInputs::Compacted {
                blob,
                compacted_at_ms: ctx.now_ms,
            };
        }
        Step::Expire => {
            expire(ctx, &entry.record.record_id)?;
            return Ok(None);
        }
        Step::Retire(blob, compacted_at_ms) => {
            release_inputs(retention, &blob, ctx.now_ms)?;
            entry.inputs = EntryInputs::Retired {
                blob,
                compacted_at_ms,
                retired_at_ms: ctx.now_ms,
            };
        }
    }
    Ok(Some(entry))
}

/// Apply the policy's retention to at most `limit` due entries of the tenant.
pub fn compact(
    ctx: &ExecutionContext,
    retention: &Retention,
    policy: &DecisionPolicyRef,
    limit: u32,
) -> Result<DecisionLogCompacted, String> {
    let resolved = resolve_policy(ctx.store, ctx.tenant_id, policy)?;
    let expire_after = expire_after_ms(&resolved.policy);
    let policy = resolved.statistical;
    let limit = limit.clamp(1, MAX_COMPACT_PER_CALL);
    let mut done = DecisionLogCompacted {
        compacted: 0,
        retired: 0,
        expired: 0,
        more: false,
    };
    for (key, bytes) in
        ctx.store
            .decision_artifacts_with_prefix(ctx.tenant_id, "record:", MAX_LOG_ROWS)?
    {
        let entry: DecisionLogEntry = decode_artifact(&bytes, "decision log entry")?;
        let Some(step) = due(&entry, &policy, expire_after, ctx.now_ms) else {
            continue;
        };
        if done.compacted + done.retired + done.expired == limit {
            done.more = true;
            break;
        }
        match step {
            Step::Compact => done.compacted += 1,
            Step::Retire(..) => done.retired += 1,
            Step::Expire => done.expired += 1,
        }
        if let Some(updated) = apply(ctx, retention, entry, step)? {
            ctx.store
                .replace_decision_artifact(ctx.tenant_id, &key, encode_artifact(&updated)?)?;
        }
    }
    Ok(done)
}

/// Delete one expired entry with its evaluations, abstention resolutions and
/// evaluator grant, in one owner transaction.
fn expire(ctx: &ExecutionContext, record_id: &str) -> Result<(), String> {
    let keys = [
        record_key(record_id),
        super::stat_evaluator::lease_key(record_id),
    ];
    let prefixes = [evaluation_key(record_id, ""), resolution_prefix(record_id)];
    ctx.store
        .remove_decision_artifacts(ctx.tenant_id, &keys, &prefixes)
        .map(|_| ())
}

fn restored(
    retention: &Retention,
    tenant_id: &str,
    record: &StatisticalDecisionRecord,
    pin: &InputsBlob,
) -> Result<StatisticalDecisionRecord, String> {
    let bytes = load_inputs(retention, tenant_id, pin)?;
    if content_digest_of(&bytes) != pin.sha256 {
        return Err(refusal(
            StatisticalErrorCode::DatasetInvalid,
            "the inputs blob does not match its pin",
        ));
    }
    let matrix: FeatureMatrixRef = serde_json::from_slice(&bytes)
        .map_err(|error| refusal(StatisticalErrorCode::DatasetInvalid, error))?;
    let mut full = record.clone();
    full.inputs.feature_matrix = matrix;
    Ok(full)
}
