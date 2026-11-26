//! Chunked source drains (MEDULLA-INGEST-WORKER-R605.2, R605.4, R605.5).
//!
//! A drain is a chain of bounded page tasks on the one ingest task queue. Its aggregate (the next
//! page checkpoint, the sums of the acks, the state) is the graph node `source_drain:<id>`; this
//! module holds the pure decisions and the node mapping, and the served handlers read and write
//! the node through the graph client. Nothing here reads a clock or a store.
//!
//! The ack order is node, then task ack, then next-page enqueue (the node write is idempotent on
//! the page cursor). A crash between two steps leaves a state the next step or the recovery pass
//! completes: the worker's retry of the ack is a [`DrainAckOutcome::Replay`], and the scheduler
//! tick re-enqueues the page of a Running drain that has no task (`medulla_source_types::ingest_job::recovery_page`).

use medulla_ingestion_types::ingest_drain::{
    drain_node_id, DrainAck, DrainAckError, DrainHandle, DrainRecord, DrainState, DrainStatus,
    IngestDrainError, PageCheckpoint,
};
use medulla_source_types::ingest_job::{drain_page_envelope, IngestJobEnvelope};
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
    /// The node already holds this ack (the node was written, the task ack or the enqueue did not
    /// finish): write nothing, ack the task and enqueue `next` when the drain is still Running.
    Replay { next: Option<IngestJobEnvelope> },
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
    let first_page = drain_page_envelope(&record, checkpoint);
    Ok(StartedDrain {
        handle,
        record,
        first_page,
    })
}

/// Decide the ack of page `page` of `record`. A completed ack with `next_cursor` advances to page
/// `page_index + 1` with that cursor, one without closes the drain. A page below the stored
/// checkpoint is `Stale` (MEDULLA-INGEST-WORKER-R605.4), unless the caller still holds the live
/// lease of the page task (`lease_held`) and the ack is the one the node already holds: that is a
/// retry after a crash between the node write and the task ack, and it is a `Replay`.
/// `max_items` is `ingest.drain_page_records`.
pub fn on_ingest_drain_ack(
    record: &DrainRecord,
    page: &PageCheckpoint,
    ack: &DrainAck,
    max_items: u64,
    now_ms: i64,
    lease_held: bool,
) -> Result<DrainAckOutcome, IngestDrainError> {
    match record.after_ack(page.page_index(), ack, max_items, now_ms) {
        Err(DrainAckError::Stale { .. }) => replay_of(record, page, ack, lease_held),
        Err(DrainAckError::Invalid(error)) => Err(error),
        Err(too_large @ DrainAckError::PageTooLarge { .. }) => {
            // Reported as an invalid ack by the handler; the model error has no cursor/graph fault.
            Err(IngestDrainError::PageTooLarge(too_large.to_string()))
        }
        Ok(next) if next.state == DrainState::Running => {
            let checkpoint = next.checkpoint()?;
            let envelope = drain_page_envelope(&next, checkpoint);
            Ok(DrainAckOutcome::Advanced {
                record: next,
                next: envelope,
            })
        }
        Ok(next) => Ok(DrainAckOutcome::Closed { record: next }),
    }
}

/// One effect of a drain ack, in the order the handler runs them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DrainAckStep {
    /// Store the record as the drain node (idempotent on the page cursor).
    WriteNode(DrainRecord),
    /// Ack the page task through the task-queue handler (its lease check refuses a foreign or
    /// lapsed ack).
    AckTask,
    /// Enqueue the next page under its deterministic task id.
    EnqueuePage(IngestJobEnvelope),
}

/// The ordered effects of an ack decision (MEDULLA-INGEST-WORKER-R605.2): node, then task ack,
/// then next page. A caller that does not hold the lease acks the task first, so the task ack
/// refuses it before any write. A `Stale` decision has no effect. A handler runs the steps in
/// order and stops at the first failure; every prefix is a state the retry or the recovery pass
/// completes.
pub fn drain_ack_plan(outcome: &DrainAckOutcome, lease_held: bool) -> Vec<DrainAckStep> {
    let ordered = |record: &DrainRecord, next: Option<&IngestJobEnvelope>| {
        let mut steps = if lease_held {
            vec![DrainAckStep::WriteNode(record.clone()), DrainAckStep::AckTask]
        } else {
            vec![DrainAckStep::AckTask, DrainAckStep::WriteNode(record.clone())]
        };
        if let Some(next) = next {
            steps.push(DrainAckStep::EnqueuePage(next.clone()));
        }
        steps
    };
    match outcome {
        DrainAckOutcome::Stale => Vec::new(),
        DrainAckOutcome::Advanced { record, next } => ordered(record, Some(next)),
        DrainAckOutcome::Closed { record } => ordered(record, None),
        DrainAckOutcome::Replay { next } => {
            let mut steps = vec![DrainAckStep::AckTask];
            if let Some(next) = next {
                steps.push(DrainAckStep::EnqueuePage(next.clone()));
            }
            steps
        }
    }
}

/// A stale ack is a replay when the page task is still leased to the caller and the node holds
/// exactly this ack: the next cursor of a Running drain, the close of a Complete one.
fn replay_of(
    record: &DrainRecord,
    page: &PageCheckpoint,
    ack: &DrainAck,
    lease_held: bool,
) -> Result<DrainAckOutcome, IngestDrainError> {
    if !lease_held || ack.validate().is_err() {
        return Ok(DrainAckOutcome::Stale);
    }
    let index = page.page_index();
    let recorded = match record.state {
        DrainState::Running => {
            index.saturating_add(1) == record.page_index
                && ack.next_cursor.as_deref() == Some(record.cursor.as_str())
        }
        DrainState::Complete => {
            index == record.page_index
                && index.saturating_add(1) == record.pages
                && ack.next_cursor.is_none()
        }
        _ => false,
    };
    if !recorded {
        return Ok(DrainAckOutcome::Stale);
    }
    let next = match record.state {
        DrainState::Running => Some(drain_page_envelope(record, record.checkpoint()?)),
        _ => None,
    };
    Ok(DrainAckOutcome::Replay { next })
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
    use medulla_source_types::ingest_job::recovery_page;
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
            on_ingest_drain_ack(&drain.record, &page0, &ack, 500, 2_000, true).unwrap()
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
            on_ingest_drain_ack(&record, &page0, &ack, 500, 2_001, false).unwrap(),
            DrainAckOutcome::Stale
        );
        // No cursor closes the drain.
        let close = DrainAck {
            items: 1,
            next_cursor: None,
            profile: Vec::new(),
        };
        let DrainAckOutcome::Closed { record } =
            on_ingest_drain_ack(&record, &next_page, &close, 500, 3_000, true).unwrap()
        else {
            panic!("no cursor closes the drain");
        };
        assert_eq!(record.state, DrainState::Complete);
        assert_eq!((record.pages, record.items), (2, 11));
    }

    // spec: MEDULLA-INGEST-WORKER-R605.2
    /// Verifies MEDULLA-INGEST-WORKER-R605.2: a retry after a crash between the node write and the
    /// task ack replays, and the recovery pass names the page a Running drain waits on.
    #[test]
    fn a_retry_of_a_recorded_ack_replays_and_a_running_drain_names_its_page() {
        let drain = started();
        let page0 = drain.first_page.page.clone().unwrap();
        let ack = DrainAck {
            items: 10,
            next_cursor: Some("c1".into()),
            profile: Vec::new(),
        };
        let DrainAckOutcome::Advanced { record, next } =
            on_ingest_drain_ack(&drain.record, &page0, &ack, 500, 2_000, true).unwrap()
        else {
            panic!("a cursor advances the drain");
        };
        // The node holds the ack; the task ack never happened. The worker retries while it still
        // holds the lease: replay, and the next page is the one the first attempt would enqueue.
        assert_eq!(
            on_ingest_drain_ack(&record, &page0, &ack, 500, 2_100, true).unwrap(),
            DrainAckOutcome::Replay { next: Some(next.clone()) }
        );
        // Without the lease, or with a different cursor, the ack is stale.
        assert_eq!(
            on_ingest_drain_ack(&record, &page0, &ack, 500, 2_100, false).unwrap(),
            DrainAckOutcome::Stale
        );
        let other = DrainAck { next_cursor: Some("c9".into()), ..ack.clone() };
        assert_eq!(
            on_ingest_drain_ack(&record, &page0, &other, 500, 2_100, true).unwrap(),
            DrainAckOutcome::Stale
        );
        // The recovery pass names the same page task.
        assert_eq!(recovery_page(&record).unwrap(), Some(next.clone()));
        // A closed drain replays its closing ack with nothing to enqueue and has no recovery page.
        let page1 = next.page.clone().unwrap();
        let close = DrainAck { items: 1, next_cursor: None, profile: Vec::new() };
        let DrainAckOutcome::Closed { record: closed } =
            on_ingest_drain_ack(&record, &page1, &close, 500, 3_000, true).unwrap()
        else {
            panic!("no cursor closes the drain");
        };
        assert_eq!(
            on_ingest_drain_ack(&closed, &page1, &close, 500, 3_100, true).unwrap(),
            DrainAckOutcome::Replay { next: None }
        );
        assert_eq!(recovery_page(&closed).unwrap(), None);
    }

    /// The failable substrate of a drain: the node, the page tasks and the queue as the handler
    /// sees them, with one step that can be made to fail (the crash).
    struct Substrate {
        node: DrainRecord,
        /// Task ids with their state: `true` is acked, `false` is leased to the worker.
        tasks: std::collections::BTreeMap<String, bool>,
        steps_run: usize,
        crash_at: Option<usize>,
    }

    impl Substrate {
        fn new(drain: &StartedDrain) -> Self {
            let mut tasks = std::collections::BTreeMap::new();
            tasks.insert(medulla_source_types::ingest_job::task_id(&drain.first_page), false);
            Self { node: drain.record.clone(), tasks, steps_run: 0, crash_at: None }
        }

        fn lease_held(&self, page: &IngestJobEnvelope) -> bool {
            self.tasks.get(&medulla_source_types::ingest_job::task_id(page)) == Some(&false)
        }

        /// Run the plan of one ack; `Err` is the injected crash, after which nothing else runs.
        fn run(&mut self, page: &IngestJobEnvelope, plan: &[DrainAckStep]) -> Result<(), ()> {
            for step in plan {
                if self.crash_at == Some(self.steps_run) {
                    self.crash_at = None;
                    return Err(());
                }
                self.steps_run += 1;
                match step {
                    DrainAckStep::WriteNode(record) => self.node = record.clone(),
                    DrainAckStep::AckTask => {
                        self.tasks.insert(medulla_source_types::ingest_job::task_id(page), true);
                    }
                    DrainAckStep::EnqueuePage(next) => {
                        self.tasks
                            .entry(medulla_source_types::ingest_job::task_id(next))
                            .or_insert(false);
                    }
                }
            }
            Ok(())
        }

        /// The scheduler tick's recovery pass: enqueue the page a Running drain waits on.
        fn recover(&mut self) {
            if let Some(page) = recovery_page(&self.node).unwrap() {
                self.tasks
                    .entry(medulla_source_types::ingest_job::task_id(&page))
                    .or_insert(false);
            }
        }
    }

    // spec: MEDULLA-INGEST-WORKER-R605.2
    /// Verifies MEDULLA-INGEST-WORKER-R605.2 (the ack order and the recovery pass): a crash
    /// injected before each step of a page ack never stalls the drain. Either the worker's retry
    /// replays the ack or the recovery pass enqueues the page, and the drain ends Complete with
    /// each page counted once.
    #[test]
    fn a_crash_between_ack_steps_never_stalls_the_drain() {
        for crash_at in 0..3 {
            for recover_before_retry in [false, true] {
                let drain = started();
                let mut world = Substrate::new(&drain);
                let acks = [
                    DrainAck { items: 10, next_cursor: Some("c1".into()), profile: Vec::new() },
                    DrainAck { items: 5, next_cursor: None, profile: Vec::new() },
                ];
                let mut page = drain.first_page.clone();
                for (index, ack) in acks.iter().enumerate() {
                    let now_ms = 2_000 + index as i64 * 100;
                    if index == 0 {
                        world.crash_at = Some(crash_at);
                    }
                    let attempt = |world: &mut Substrate, now: i64| {
                        let held = world.lease_held(&page);
                        let outcome = on_ingest_drain_ack(
                            &world.node,
                            page.page.as_ref().unwrap(),
                            ack,
                            500,
                            now,
                            held,
                        )
                        .unwrap();
                        let plan = drain_ack_plan(&outcome, held);
                        world.run(&page, &plan)
                    };
                    if attempt(&mut world, now_ms).is_err() {
                        // The crash: the engine restarts, the tick may run its recovery pass, and
                        // the worker retries the ack while it still holds the lease.
                        if recover_before_retry {
                            world.recover();
                        }
                        attempt(&mut world, now_ms + 1).expect("the retry has no crash left");
                    }
                    // Whatever the crash point, the page the drain waits on has a task now.
                    world.recover();
                    if let Some(waiting) = recovery_page(&world.node).unwrap() {
                        page = waiting;
                        assert!(world.tasks.contains_key(&medulla_source_types::ingest_job::task_id(&page)));
                    }
                }
                assert_eq!(world.node.state, DrainState::Complete, "crash {crash_at}");
                assert_eq!((world.node.pages, world.node.items), (2, 15), "crash {crash_at}");
                assert!(world.tasks.values().all(|acked| *acked), "every page task acked");
                assert_eq!(world.tasks.len(), 2, "no duplicate page task");
            }
        }
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
