//! Chunked source drains (MEDULLA-INGEST-WORKER-R605.2, R605.4, R605.5).
//!
//! A drain is a chain of bounded page tasks on the one ingest task queue. Its aggregate (the next
//! page checkpoint, the sums of the acks, the state) is the graph node `source_drain:<id>`; this
//! module holds the pure decisions and the node mapping, and the served handlers read and write
//! the node through the graph client. Nothing here reads a clock or a store.

use medulla_ingestion_types::ingest_drain::{
    drain_node_id, DrainAck, DrainAckError, DrainHandle, DrainRecord, DrainState, DrainStatus,
    IngestDrainError, PageCheckpoint,
};
use medulla_source_types::ingest_job::{IngestJobEnvelope, IngestJobKind};
use medulla_types::contract::Digest256;
use serde_json::{json, Map, Value};

/// The domain tag under which a drain id is framed.
pub const DRAIN_ID_DOMAIN: &[u8] = b"eg/source-drain-id";
/// Node type of a drain record.
pub const DRAIN_NODE_TYPE: &str = "SourceDrain";
/// The node property that holds the canonical JSON of the [`DrainRecord`].
pub const DRAIN_RECORD_PROPERTY: &str = "drain_record";
/// The modes a drain may run in.
pub const DRAIN_MODES: [&str; 3] = ["full", "delta", "reconcile"];

/// A drain that was started: its handle, its record and the envelope of page 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartedDrain {
    pub handle: DrainHandle,
    pub record: DrainRecord,
    pub first_page: IngestJobEnvelope,
}

/// What an ack does to its drain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DrainAckOutcome {
    /// The ack is below the stored checkpoint or the drain is closed: nothing is enqueued or written.
    Stale,
    /// The drain moved to the next page: write `record`, enqueue `next`.
    Advanced {
        record: DrainRecord,
        next: IngestJobEnvelope,
    },
    /// The drain closed: write `record`.
    Closed { record: DrainRecord },
}

/// The drain id of one start: framed over tenant, connector, stream, mode and start time, so a
/// repeat of the same start in the same millisecond is the same drain.
pub fn drain_id(tenant: &str, connector: &str, stream: &str, mode: &str, now_ms: i64) -> String {
    Digest256::framed(
        DRAIN_ID_DOMAIN,
        &[
            tenant.as_bytes(),
            connector.as_bytes(),
            stream.as_bytes(),
            mode.as_bytes(),
            &now_ms.to_be_bytes(),
        ],
    )
    .expect("string lengths always fit the digest framing limit")
    .to_hex()
}

fn blank(value: &str) -> bool {
    value.trim().is_empty() || value.contains('/')
}

/// The envelope of the page `checkpoint` of the drain `record` describes.
pub fn page_envelope(
    record: &DrainRecord,
    checkpoint: PageCheckpoint,
) -> IngestJobEnvelope {
    IngestJobEnvelope {
        job_id: format!("drain:{}:{}", record.drain_id, checkpoint.page_index()),
        tenant: record.tenant.clone(),
        corpus_key: record.connector.clone(),
        kind: IngestJobKind::Document,
        target: format!("drain:{}/{}/{}", record.connector, record.stream, record.mode),
        attempt: 1,
        page: Some(checkpoint),
    }
}

/// Start a drain: the handle, the record at page 0 and the envelope of page 0. `graph` is the
/// graph the drained records go to (the routed graph). A blank connector or stream, a name with
/// a `/` and a mode outside [`DRAIN_MODES`] refuse.
pub fn start_drain(
    tenant: &str,
    connector: &str,
    stream: &str,
    mode: &str,
    graph: &str,
    now_ms: i64,
) -> Result<StartedDrain, String> {
    if blank(connector) || blank(stream) || tenant.trim().is_empty() {
        return Err("connector and stream must be non-blank and hold no '/'".to_string());
    }
    if !DRAIN_MODES.contains(&mode) {
        return Err(format!("mode must be one of {DRAIN_MODES:?}"));
    }
    let id = drain_id(tenant, connector, stream, mode, now_ms);
    let handle = DrainHandle::new(id.clone()).map_err(|error| error.to_string())?;
    let record = DrainRecord {
        drain_id: id,
        tenant: tenant.to_string(),
        connector: connector.to_string(),
        stream: stream.to_string(),
        mode: mode.to_string(),
        graph: graph.to_string(),
        page_index: 0,
        cursor: String::new(),
        pages: 0,
        items: 0,
        state: DrainState::Running,
        profile: Vec::new(),
        started_at_ms: now_ms,
        updated_at_ms: now_ms,
    };
    let checkpoint = record.checkpoint().map_err(|error| error.to_string())?;
    let first_page = page_envelope(&record, checkpoint);
    Ok(StartedDrain {
        handle,
        record,
        first_page,
    })
}

/// Decide the ack of page `page` of `record`. A completed ack with `next_cursor` advances to page
/// `page_index + 1` with that cursor, one without closes the drain. A page below the stored
/// checkpoint is `Stale` (MEDULLA-INGEST-WORKER-R605.4). `max_items` is `ingest.drain_page_records`.
pub fn on_ingest_drain_ack(
    record: &DrainRecord,
    page: &PageCheckpoint,
    ack: &DrainAck,
    max_items: u64,
    now_ms: i64,
) -> Result<DrainAckOutcome, IngestDrainError> {
    match record.after_ack(page.page_index(), ack, max_items, now_ms) {
        Err(DrainAckError::Stale { .. }) => Ok(DrainAckOutcome::Stale),
        Err(DrainAckError::Invalid(error)) => Err(error),
        Err(too_large @ DrainAckError::PageTooLarge { .. }) => {
            // Reported as an invalid ack by the handler; the model error has no cursor/graph fault.
            Err(IngestDrainError::PageTooLarge(too_large.to_string()))
        }
        Ok(next) if next.state == DrainState::Running => {
            let checkpoint = next.checkpoint()?;
            let envelope = page_envelope(&next, checkpoint);
            Ok(DrainAckOutcome::Advanced {
                record: next,
                next: envelope,
            })
        }
        Ok(next) => Ok(DrainAckOutcome::Closed { record: next }),
    }
}

/// The `upsert_node` operation that stores `record` under `source_drain:<id>`, stamped with the
/// tenant and the writing agent.
pub fn drain_node_operation(record: &DrainRecord, agent: &str) -> Result<Value, String> {
    let body = serde_json::to_string(record).map_err(|error| error.to_string())?;
    let mut properties = Map::new();
    properties.insert("type".into(), DRAIN_NODE_TYPE.into());
    properties.insert(DRAIN_RECORD_PROPERTY.into(), body.into());
    properties.insert("state".into(), json!(record.state));
    properties.insert("connector".into(), record.connector.clone().into());
    properties.insert("stream".into(), record.stream.clone().into());
    properties.insert("pages".into(), record.pages.into());
    properties.insert("items".into(), record.items.into());
    properties.insert("tenant_id".into(), record.tenant.clone().into());
    properties.insert("_owner".into(), agent.into());
    Ok(json!({
        "op": "upsert_node",
        "id": drain_node_id(&record.drain_id),
        "properties": properties,
    }))
}

/// The drain a stored node holds, or `None` when the node is not a drain record of `tenant`.
/// Another tenant's drain reads as absent, so there is no existence oracle.
pub fn drain_from_node(properties: &Map<String, Value>, tenant: &str) -> Option<DrainRecord> {
    let body = properties.get(DRAIN_RECORD_PROPERTY)?.as_str()?;
    let record: DrainRecord = serde_json::from_str(body).ok()?;
    (record.tenant == tenant).then_some(record)
}

/// The status of `record` for `tenant`; `None` for another tenant's drain.
pub fn drain_status(record: &DrainRecord, tenant: &str) -> Option<DrainStatus> {
    (record.tenant == tenant).then(|| record.status())
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_ingestion_types::ingest_drain::StageProfile;
    use medulla_types::ingest_throughput::IngestStage;

    fn started() -> StartedDrain {
        start_drain("acme", "crm", "accounts", "full", "main", 1_000).unwrap()
    }

    // spec: MEDULLA-INGEST-WORKER-R605.2
    /// Verifies MEDULLA-INGEST-WORKER-R605.2: a drain is a chain of bounded page tasks.
    #[test]
    fn ack_with_cursor_enqueues_next_page() {
        let drain = started();
        assert_eq!(drain.first_page.page.as_ref().unwrap().page_index(), 0);
        assert_eq!(drain.first_page.tenant, "acme");
        // The same start is the same drain.
        assert_eq!(
            started().handle,
            start_drain("acme", "crm", "accounts", "full", "main", 1_000).unwrap().handle
        );
        let page0 = drain.first_page.page.clone().unwrap();
        let ack = DrainAck {
            items: 10,
            next_cursor: Some("c1".into()),
            profile: vec![StageProfile {
                stage: IngestStage::Fetch,
                wall_ms: 4,
                tokens: 0,
                embeddings: 0,
            }],
        };
        let DrainAckOutcome::Advanced { record, next } =
            on_ingest_drain_ack(&drain.record, &page0, &ack, 500, 2_000).unwrap()
        else {
            panic!("a cursor advances the drain");
        };
        let next_page = next.page.clone().unwrap();
        assert_eq!((next_page.page_index(), next_page.cursor()), (1, "c1"));
        assert_ne!(
            medulla_source_types::ingest_job::task_id(&next),
            medulla_source_types::ingest_job::task_id(&drain.first_page)
        );
        // The same ack again is stale and changes nothing.
        assert_eq!(
            on_ingest_drain_ack(&record, &page0, &ack, 500, 2_001).unwrap(),
            DrainAckOutcome::Stale
        );
        // No cursor closes the drain.
        let close = DrainAck {
            items: 1,
            next_cursor: None,
            profile: Vec::new(),
        };
        let DrainAckOutcome::Closed { record } =
            on_ingest_drain_ack(&record, &next_page, &close, 500, 3_000).unwrap()
        else {
            panic!("no cursor closes the drain");
        };
        assert_eq!(record.state, DrainState::Complete);
        assert_eq!((record.pages, record.items), (2, 11));
    }

    // spec: MEDULLA-INGEST-WORKER-R605.2
    #[test]
    fn a_bad_start_refuses_and_the_node_round_trips_for_its_tenant_only() {
        assert!(start_drain("acme", "", "s", "full", "main", 1).is_err());
        assert!(start_drain("acme", "a/b", "s", "full", "main", 1).is_err());
        assert!(start_drain("acme", "c", "s", "sideways", "main", 1).is_err());
        let drain = started();
        let op = drain_node_operation(&drain.record, "agent").unwrap();
        assert_eq!(op["id"], format!("source_drain:{}", drain.record.drain_id));
        let properties = op["properties"].as_object().unwrap();
        assert_eq!(drain_from_node(properties, "acme"), Some(drain.record.clone()));
        assert_eq!(drain_from_node(properties, "other"), None);
        assert_eq!(drain_status(&drain.record, "other"), None);
        assert_eq!(drain_status(&drain.record, "acme").unwrap().pages, 0);
    }
}
