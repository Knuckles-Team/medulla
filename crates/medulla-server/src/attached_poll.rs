//! Watermark capture poll for attached SQLite file sources
//! (MEDULLA-UNIFIED-DATA-PLANE-R015.2.2.2).
//!
//! The periodic sweep (`server_startup::attached_poll_tick`) calls [`run`], which walks every
//! registered `ForeignSourceSpec::SqliteFile` that declares a watermark column, polls it
//! through `medulla_plan::sqlite_capture::poll_watermark` and commits what it saw as one
//! governed change envelope per poll, as the fixed service principal
//! `service:attached-poll`, through the same dispatch path as any caller's write
//! (`ApplyChangeEnvelopeDraft`): the same authorization, durability, replay and audit.
//!
//! The stored position is the committed envelope's own [`ChangeCursor`], read back with
//! `GetChangeCursor`, so a restart resumes from the last committed position and the commit's
//! `expected_previous` refuses a concurrent double write: nothing is emitted twice. A
//! source whose poll is refused (a missing or unindexed watermark column, a file that
//! cannot be read) is reported by name and skipped; the other sources still run.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use medulla_durable::change_envelope::ChangeEnvelope as CaptureEnvelope;
use medulla_durable::idempotent_consumer::ConsumerCheckpoint;
use medulla_plan::sqlite_capture::poll_watermark;
use medulla_protocol_types::change_envelope::{
    ChangeCursor, ChangeEnvelopeDraft, ChangeMutationDraft, ContentVersion, ContentVersionPosition,
    CursorPosition, MaterialClass, MaterialOperation, PolicyRecord, PrivacyAttestation,
    CHANGE_ENVELOPE_VERSION,
};
use medulla_protocol_types::mutation_batch::MutationOperation;
use medulla_protocol_types::protocol::{Method, Request, ResultPayload};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_server_ops::api::query_txn::foreign_catalog::SqliteCaptureSource;
use medulla_server_ops::api::query_txn::ForeignStateExt;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

/// The service principal the poll commits as (`service:attached-poll`).
pub const POLL_SERVICE: &str = "attached-poll";

/// The graph the captured rows land in.
pub const CAPTURE_GRAPH: &str = "attached-capture";

/// The cursor type of a watermark position.
const CURSOR_TYPE: &str = "sqlite_watermark";

/// What one poll of one source did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PollReport {
    /// The source name.
    pub source: String,
    /// The number of changed rows committed this poll.
    pub captured: usize,
    /// The committed position, when anything was captured.
    pub position: Option<u64>,
}

/// The cursor source and partition one registered source's position is stored under.
pub fn cursor_key(source: &SqliteCaptureSource) -> (String, String) {
    let partition = hex::encode(Sha256::digest(source.owner_scope.as_bytes()));
    (
        format!("sqlite-file:{}:{}", source.name, source.table),
        partition,
    )
}

/// The graph node a captured row addresses.
pub fn node_id(source: &SqliteCaptureSource, rowid: &str) -> String {
    format!("sqlite:{}:{}:{rowid}", source.name, source.table)
}

/// Convert the capture records one poll returned into the governed draft that commits them
/// as one envelope: one node write per captured row, one cursor at the poll's position.
/// `previous` is the position the stored cursor holds, bound as `expected_previous`.
pub fn draft_for_poll(
    source: &SqliteCaptureSource,
    captured: &[CaptureEnvelope],
    previous: Option<u64>,
) -> Result<ChangeEnvelopeDraft, String> {
    let Some(first) = captured.first() else {
        return Err("a poll that captured nothing commits nothing".to_string());
    };
    for record in captured {
        record.validate().map_err(|error| error.to_string())?;
    }
    let position: u64 = first.source_position.parse().map_err(|_| {
        format!(
            "source position `{}` is not a watermark",
            first.source_position
        )
    })?;
    let (cursor_source, partition) = cursor_key(source);
    let envelope_id = format!("{cursor_source}:{partition}:{position}");
    let mut operations = Vec::with_capacity(captured.len());
    let mut digest = Sha256::new();
    for (ordinal, record) in captured.iter().enumerate() {
        let rowid = record
            .key
            .as_ref()
            .and_then(|key| key.get(medulla_plan::sqlite_capture::ROWID))
            .ok_or_else(|| "a captured row carries no rowid".to_string())?;
        let node = node_id(source, rowid);
        let properties = serde_json::json!({
            "source": source.name,
            "table": record.table,
            "rowid": rowid,
            "watermark": source.watermark,
            "position": position,
        });
        digest.update(node.as_bytes());
        digest.update(properties.to_string().as_bytes());
        operations.push(MutationOperation {
            ordinal: ordinal as u32,
            surface: medulla_mutation_types::mutation_scope::MutationSurface::Graph,
            domain: medulla_mutation_types::mutation_scope::DurabilityDomain::GraphRows,
            method: Method::AddNode {
                node_id: node,
                properties_msgpack: rmp_serde::to_vec_named(&properties)
                    .map_err(|error| error.to_string())?,
            },
        });
    }
    let payload_digest = hex::encode(digest.finalize());
    let object_id = format!("sqlite:{}:{}", source.name, source.table);
    Ok(ChangeEnvelopeDraft {
        schema_version: CHANGE_ENVELOPE_VERSION,
        envelope_id: envelope_id.clone(),
        mutation: ChangeMutationDraft {
            batch_id: envelope_id,
            placement_epoch: 0,
            expected_graph_version: None,
            fencing_token: None,
            operations,
            outbox: Vec::new(),
        },
        privacy: PrivacyAttestation {
            policy_version: "attached-poll-v1".to_string(),
            sanitizer_version: "none".to_string(),
            sanitized_payload_digest: payload_digest.clone(),
        },
        material_class: MaterialClass::Attested,
        content_version: ContentVersion {
            object_id: object_id.clone(),
            digest_algorithm: "sha256".to_string(),
            digest: payload_digest,
            previous_digest: None,
            source_version: ContentVersionPosition::Sequence(position),
        },
        cursor: Some(ChangeCursor {
            source: cursor_source,
            partition,
            position: CursorPosition::Opaque {
                cursor_type: CURSOR_TYPE.to_string(),
                value: position.to_string(),
            },
            expected_previous: previous.map(|previous| CursorPosition::Opaque {
                cursor_type: CURSOR_TYPE.to_string(),
                value: previous.to_string(),
            }),
        }),
        lineage: Vec::new(),
        policies: vec![PolicyRecord {
            policy_id: format!("attached-poll:{}", source.name),
            operation: MaterialOperation::Upsert,
            object_id,
            tenant: source.owner_scope.clone(),
            classification: "internal".to_string(),
            policy_version: "attached-poll-v1".to_string(),
            subject_set_digest: "0".repeat(64),
            retention_policy: "default".to_string(),
            legal_hold: false,
        }],
        evidence: Vec::new(),
        features: Vec::new(),
        blobs: Vec::new(),
    })
}

/// The service context one source's poll commits under; its idempotency key names the
/// source and position, so a retried commit replays instead of writing twice.
fn service_context(
    state_anonymous: bool,
    server: &medulla_server_api::server_context::ServerContext,
    key: String,
) -> Result<VerifiedRequestContext, String> {
    if state_anonymous {
        return Ok(VerifiedRequestContext::anonymous_local(Some(key)));
    }
    let service = VerifiedRequestContext::authenticated_fixed_service_actor(
        server,
        POLL_SERVICE,
        &["ingest:write", "ingest:read"],
    )?;
    Ok(VerifiedRequestContext::from_verified_claims_with_nonce(
        service.claims().clone(),
        key,
        Some(medulla_types::contract::Nonce::minted()),
    ))
}

/// The position stored for `source`, from the cursor its last committed envelope carried.
async fn stored_position(
    state: &Arc<RwLock<ServerState>>,
    context: &VerifiedRequestContext,
    source: &SqliteCaptureSource,
) -> Result<Option<u64>, String> {
    let (cursor_source, partition) = cursor_key(source);
    let request = Request {
        id: 0,
        graph: CAPTURE_GRAPH.to_string(),
        auth_token: String::new(),
        agent_id: Some(context.agent_id().to_string()),
        method: Method::GetChangeCursor {
            source: cursor_source,
            partition,
            tenant: context.tenant().to_string(),
        },
        trace_id: None,
    };
    let response = medulla_server_api::api::reentry::dispatch_verified_request(
        state,
        request,
        context.clone(),
    )
    .await;
    if let Some(refusal) = response.refusal_text() {
        return Err(refusal);
    }
    let cursor: Option<ChangeCursor> = match response.result {
        Some(ResultPayload::Raw(bytes)) => medulla_types::msgpack::decode_bounded(
            &bytes,
            medulla_types::msgpack::MsgpackLimits::new(64 * 1024, 1_000, 16),
        )
        .map_err(|_| "the stored change cursor is corrupt".to_string())?,
        Some(ResultPayload::Json(value)) => serde_json::from_value(value)
            .map_err(|_| "the stored change cursor is corrupt".to_string())?,
        None => None,
        Some(_) => return Err("the stored change cursor has an invalid payload shape".into()),
    };
    Ok(match cursor.map(|cursor| cursor.position) {
        Some(CursorPosition::Opaque { value, .. }) => Some(
            value
                .parse()
                .map_err(|_| "the stored change cursor is not a watermark".to_string())?,
        ),
        Some(_) | None => None,
    })
}

/// Poll one source once and commit what it captured.
pub async fn poll_source(
    state: &Arc<RwLock<ServerState>>,
    source: &SqliteCaptureSource,
    busy_timeout: Duration,
) -> Result<PollReport, String> {
    let (anonymous, server) = {
        let state = state.read().await;
        (state.local_anonymous, state.context.clone())
    };
    let session = format!("{POLL_SERVICE}:{}", source.name);
    let context = service_context(anonymous, &server, session.clone())?;
    #[cfg(feature = "security")]
    if !anonymous {
        medulla_server_api::service_grant::ServiceGraphGrant {
            actor: &format!("service:{POLL_SERVICE}"),
            role_prefix: POLL_SERVICE,
            unbootstrapped: "ATTACHED_POLL_POLICY_UNBOOTSTRAPPED",
        }
        .ensure(
            &mut medulla_server_api::state::timed_write(state)
                .await
                .isolation,
            CAPTURE_GRAPH,
        )?;
    }
    medulla_server_api::api::reentry::ensure_internal_global_graph(
        state,
        0,
        &context,
        CAPTURE_GRAPH,
        &session,
    )
    .await?;

    let previous = stored_position(state, &context, source).await?;
    let mut checkpoint = ConsumerCheckpoint {
        consumer_id: format!("attached-poll:{}", source.name),
        position: previous.unwrap_or(0),
    };
    let path = source.path.clone();
    let (table, watermark) = (source.table.clone(), source.watermark.clone());
    let (captured, checkpoint) = tokio::task::spawn_blocking(move || {
        let captured = poll_watermark(
            Path::new(&path),
            &table,
            &watermark,
            &mut checkpoint,
            busy_timeout,
        );
        (captured, checkpoint)
    })
    .await
    .map_err(|error| format!("poll task failed: {error}"))?;
    let captured = captured.map_err(|refusal| refusal.to_string())?;
    if captured.is_empty() {
        return Ok(PollReport {
            source: source.name.clone(),
            captured: 0,
            position: None,
        });
    }
    let draft = draft_for_poll(source, &captured, previous)?;
    let key = format!("{session}:{}", checkpoint.position);
    let commit_context = service_context(anonymous, &server, key)?;
    let request = Request {
        id: 0,
        graph: CAPTURE_GRAPH.to_string(),
        auth_token: String::new(),
        agent_id: Some(commit_context.agent_id().to_string()),
        method: Method::ApplyChangeEnvelopeDraft {
            draft: Box::new(draft),
        },
        trace_id: None,
    };
    let response =
        medulla_server_api::api::reentry::dispatch_verified_request(state, request, commit_context)
            .await;
    if let Some(refusal) = response.refusal_text() {
        return Err(refusal);
    }
    Ok(PollReport {
        source: source.name.clone(),
        captured: captured.len(),
        position: Some(checkpoint.position),
    })
}

/// One sweep: poll every registered SQLite file source that declares a watermark. A refused
/// source is reported and skipped; the others still run. Returns what each source did.
pub async fn run(state: &Arc<RwLock<ServerState>>) -> Vec<Result<PollReport, (String, String)>> {
    let catalog = state.read().await.foreign_sources();
    let busy = Duration::from_millis(
        medulla_server_api::knob_config::knobs().foreign_sqlite_busy_timeout_ms,
    );
    let mut reports = Vec::new();
    for source in catalog.capture_sources() {
        reports.push(
            poll_source(state, &source, busy)
                .await
                .map_err(|error| (source.name.clone(), error)),
        );
    }
    reports
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn source() -> SqliteCaptureSource {
        SqliteCaptureSource {
            owner_scope: "tenant/agent".into(),
            name: "orders".into(),
            path: "/data/orders.db".into(),
            table: "t".into(),
            watermark: "updated_at".into(),
        }
    }

    fn record(rowid: &str, position: &str) -> CaptureEnvelope {
        CaptureEnvelope {
            source_position: position.into(),
            table: "t".into(),
            op: medulla_durable::change_envelope::ChangeOp::Update,
            key: Some(BTreeMap::from([("rowid".to_string(), rowid.to_string())])),
            table_is_keyless: false,
        }
    }

    #[test]
    fn a_poll_becomes_one_draft_with_a_row_write_each_and_the_cursor_at_the_position() {
        let draft = draft_for_poll(&source(), &[record("2", "14"), record("4", "14")], Some(12))
            .expect("draft");
        assert_eq!(draft.mutation.operations.len(), 2);
        let cursor = draft.cursor.expect("cursor");
        assert_eq!(
            cursor.expected_previous,
            Some(CursorPosition::Opaque {
                cursor_type: CURSOR_TYPE.into(),
                value: "12".into()
            })
        );
        assert!(matches!(
            draft.mutation.operations[0].method,
            Method::AddNode { ref node_id, .. } if node_id == "sqlite:orders:t:2"
        ));
    }

    #[test]
    fn an_empty_poll_commits_nothing() {
        assert!(draft_for_poll(&source(), &[], None).is_err());
    }
}
