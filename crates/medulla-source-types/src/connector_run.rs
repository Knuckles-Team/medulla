//! Connector runs (MEDULLA-UNIFIED-DATA-PLANE-R790): the durable, tenant-keyed record of each
//! source-sync run of a connector connection, and the run list over them.
//!
//! The connector control plane reports a run as it starts and as it ends; the engine stamps the
//! start and end with its own clock, never with a value the caller sends. [`CONNECTOR_RUN_MACHINE`]
//! is the one edge table: a run starts running and ends succeeded, failed or cancelled, and a
//! finished run is sealed. The list answers the runs of one verified tenant, newest first. A
//! connection no run of the tenant names refuses the one `CONNECTION_NOT_FOUND`, whether it is
//! absent or another tenant's.
//!
//! Pure data and decisions; the served handler reads the stored nodes the caller can see, calls
//! [`handle_connector_run_record`] or [`handle_connector_runs`] and writes the one node.

use std::collections::BTreeMap;

use medulla_types::refusal_code::RefusalCode;
use medulla_types::tenant_cursor::CursorFamily;
use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::lifecycle::sha256_hex;

/// The graph `type` of a connector run node.
pub const CONNECTOR_RUN_TYPE: &str = "ConnectorRun";
/// The node property holding the canonical JSON of the run counts.
pub const CONNECTOR_RUN_COUNTS_PROPERTY: &str = "counts";

/// No run of the verified tenant names the connection. An absent connection and a foreign one
/// answer alike.
pub const CONNECTION_NOT_FOUND: &str = "CONNECTION_NOT_FOUND";
/// No run of the verified tenant has this id. An absent run and a foreign one answer alike.
pub const CONNECTOR_RUN_NOT_FOUND: &str = "CONNECTOR_RUN_NOT_FOUND";
/// A run field is out of bounds, a run changed its connection, or a cursor is unusable.
pub const CONNECTOR_RUN_INVALID: &str = "CONNECTOR_RUN_INVALID";

/// The stable refusal codes of the connector run methods (a refused transition is
/// `TRANSITION_REFUSED`).
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new(CONNECTION_NOT_FOUND),
    RefusalCode::new(CONNECTOR_RUN_INVALID),
    RefusalCode::new(CONNECTOR_RUN_NOT_FOUND),
];

/// Longest run id or connection id, in bytes.
pub const MAX_RUN_ID_BYTES: usize = 128;
/// Most counters one run carries.
pub const MAX_RUN_COUNTERS: usize = 16;
/// Longest counter name, in bytes.
pub const MAX_RUN_COUNTER_BYTES: usize = 64;
/// Most runs one `ConnectorRuns` page carries, and the page size when none is asked for.
pub const MAX_RUN_LIST_LIMIT: u32 = 100;
/// Longest opaque list cursor a caller may hand back.
pub const MAX_RUN_CURSOR_BYTES: usize = 1024;
/// Domain-separation prefix of the `ConnectorRuns` cursor family.
pub const RUN_CURSOR_DOMAIN: &[u8] = b"medulla/connector-run-list-cursor/v1";

const LIST_CURSOR: CursorFamily = CursorFamily {
    domain: RUN_CURSOR_DOMAIN,
    max_bytes: MAX_RUN_CURSOR_BYTES,
    noun: "connector run list",
};

/// A typed refusal of a connector run method.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RunRefusal {
    /// A field is out of bounds, the connection changed, or a cursor is not usable.
    #[error("CONNECTOR_RUN_INVALID: {0}")]
    Invalid(String),
    /// No run of the verified tenant names the connection.
    #[error("CONNECTION_NOT_FOUND: {0}")]
    ConnectionNotFound(String),
    /// No run of the verified tenant has the id.
    #[error("CONNECTOR_RUN_NOT_FOUND: {0}")]
    RunNotFound(String),
    /// The run machine refuses the event in the run's current state.
    #[error("{0}")]
    Transition(TransitionRefused),
}

impl RunRefusal {
    /// The stable refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => CONNECTOR_RUN_INVALID,
            Self::ConnectionNotFound(_) => CONNECTION_NOT_FOUND,
            Self::RunNotFound(_) => CONNECTOR_RUN_NOT_FOUND,
            Self::Transition(_) => medulla_types::transition_refused::TRANSITION_REFUSED,
        }
    }

    /// The one refusal of a read that cannot find its run.
    pub fn run_not_found() -> Self {
        Self::RunNotFound("no run with this id".to_owned())
    }

    /// The one refusal of a list that names a connection with no run.
    pub fn connection_not_found() -> Self {
        Self::ConnectionNotFound("no connection with this id".to_owned())
    }
}

/// The state of one run: the closed state set of `ConnectorRunMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// The run has started and not ended.
    Running,
    /// The run ended having done its work.
    Succeeded,
    /// The run ended with an error.
    Failed,
    /// The run was stopped.
    Cancelled,
}

/// The closed event set of `ConnectorRunMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RunEvent {
    /// The run ended having done its work.
    Succeed,
    /// The run ended with an error.
    Fail,
    /// The run was stopped.
    Cancel,
}

impl MachineState for RunState {
    fn stored(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl MachineEvent for RunEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Succeed => "Succeed",
            Self::Fail => "Fail",
            Self::Cancel => "Cancel",
        }
    }
}

impl RunState {
    /// The event that ends a running run in this state; `None` for `Running`.
    pub fn ending_event(self) -> Option<RunEvent> {
        match self {
            Self::Running => None,
            Self::Succeeded => Some(RunEvent::Succeed),
            Self::Failed => Some(RunEvent::Fail),
            Self::Cancelled => Some(RunEvent::Cancel),
        }
    }

    /// The state whose stored text is `stored`.
    pub fn from_stored(stored: &str) -> Option<Self> {
        CONNECTOR_RUN_MACHINE.state_from_stored(stored)
    }

    /// The one transition function of `ConnectorRunMachine`.
    pub fn apply(self, event: RunEvent) -> Result<Self, TransitionRefused> {
        transition(&CONNECTOR_RUN_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }
}

/// `ConnectorRunMachine`: the closed edge table. Every other pair of state and event is refused.
pub static CONNECTOR_RUN_MACHINE: MachineDef<RunState, RunEvent, &'static str> = MachineDef {
    name: "ConnectorRunMachine",
    states: &[
        RunState::Running,
        RunState::Succeeded,
        RunState::Failed,
        RunState::Cancelled,
    ],
    initial: RunState::Running,
    terminals: &[RunState::Succeeded, RunState::Failed, RunState::Cancelled],
    events: &[RunEvent::Succeed, RunEvent::Fail, RunEvent::Cancel],
    guards: &[],
    edges: &[
        Edge {
            id: "C01",
            from: RunState::Running,
            event: RunEvent::Succeed,
            guard: None,
            to: RunState::Succeeded,
        },
        Edge {
            id: "C02",
            from: RunState::Running,
            event: RunEvent::Fail,
            guard: None,
            to: RunState::Failed,
        },
        Edge {
            id: "C03",
            from: RunState::Running,
            event: RunEvent::Cancel,
            guard: None,
            to: RunState::Cancelled,
        },
    ],
};

/// What the control plane reports of a run: its id, its connection, where it stands and its
/// counters. The times are the engine's, never the caller's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ConnectorRunInput {
    /// The id of the run, chosen by the control plane.
    pub run_id: String,
    /// The connection the run syncs.
    pub connection_id: String,
    /// Where the run stands.
    pub state: RunState,
    /// What the run has done so far, by counter name (`records`, `relationships`, `errors`, ...).
    #[serde(default)]
    pub counts: BTreeMap<String, u64>,
}

/// One stored connector run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ConnectorRun {
    /// The id of the run.
    pub run_id: String,
    /// The connection the run syncs.
    pub connection_id: String,
    /// Where the run stands.
    pub state: RunState,
    /// When the run was first reported, by the engine clock, in milliseconds.
    pub started_ms: u64,
    /// When the run ended, by the engine clock; absent while it runs.
    #[serde(default)]
    pub finished_ms: Option<u64>,
    /// What the run has done, by counter name.
    #[serde(default)]
    pub counts: BTreeMap<String, u64>,
}

/// What `ConnectorRunRecord` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ConnectorRunReceipt {
    /// The id of the run.
    pub run_id: String,
    /// The graph node id of the run.
    pub node_id: String,
    /// Where the run stands now.
    pub state: RunState,
    /// When the run was first reported, by the engine clock.
    pub started_ms: u64,
    /// When the run ended, by the engine clock.
    #[serde(default)]
    pub finished_ms: Option<u64>,
    /// True when the same report was already stored and nothing was written.
    pub replayed: bool,
}

/// One cursor-paged page of runs, newest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ConnectorRunPage {
    /// The runs of this page.
    pub runs: Vec<ConnectorRun>,
    /// The opaque cursor of the next page, absent on the last page.
    #[serde(default)]
    pub next_cursor: Option<String>,
}

impl ConnectorRun {
    /// Read the run a graph node holds. `None` when the node is not a run of `tenant` or holds a
    /// value the run model does not allow.
    pub fn from_node(node: &Value, tenant: &str) -> Option<Self> {
        if node.get("type").and_then(Value::as_str) != Some(CONNECTOR_RUN_TYPE)
            || node.get("tenant_id").and_then(Value::as_str) != Some(tenant)
        {
            return None;
        }
        let text = |key: &str| node.get(key).and_then(Value::as_str).map(str::to_owned);
        Some(Self {
            run_id: text("run_id")?,
            connection_id: text("connection_id")?,
            state: RunState::from_stored(&text("state")?)?,
            started_ms: node.get("started_ms")?.as_u64()?,
            finished_ms: node.get("finished_ms").and_then(Value::as_u64),
            counts: serde_json::from_str(node.get(CONNECTOR_RUN_COUNTS_PROPERTY)?.as_str()?)
                .ok()?,
        })
    }
}

/// The graph node id of `run_id` in `tenant`: the tenant is part of the key as a short digest, so
/// no tenant name appears in a node id.
pub fn connector_run_node_id(tenant: &str, run_id: &str) -> String {
    format!(
        "connector_run:{}:{run_id}",
        &sha256_hex(tenant.as_bytes())[..16]
    )
}

fn invalid<T>(what: &str) -> Result<T, RunRefusal> {
    Err(RunRefusal::Invalid(format!("{what} is out of bounds")))
}

fn id_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_RUN_ID_BYTES
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b':'))
}

fn counter_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_RUN_COUNTER_BYTES
        && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}

impl ConnectorRunInput {
    /// Check every bound.
    pub fn validate(&self) -> Result<(), RunRefusal> {
        if !id_ok(&self.run_id) {
            return invalid("run_id");
        }
        if !id_ok(&self.connection_id) {
            return invalid("connection_id");
        }
        if self.counts.len() > MAX_RUN_COUNTERS || !self.counts.keys().all(|name| counter_ok(name))
        {
            return invalid("counts");
        }
        Ok(())
    }
}

/// What [`handle_connector_run_record`] decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectorRunWrite {
    /// The run to store, or the stored one when the report replays.
    pub run: ConnectorRun,
    /// True when the same report was already stored, so nothing is written.
    pub replayed: bool,
}

impl ConnectorRunWrite {
    /// The receipt of this decision; `node_id` is the node of the run.
    pub fn receipt(&self, node_id: String) -> ConnectorRunReceipt {
        ConnectorRunReceipt {
            run_id: self.run.run_id.clone(),
            node_id,
            state: self.run.state,
            started_ms: self.run.started_ms,
            finished_ms: self.run.finished_ms,
            replayed: self.replayed,
        }
    }
}

/// `ConnectorRunRecord`: decide what one report of a run writes, or refuse before anything is
/// written. `stored` is the run already held for the id of the tenant; `now_ms` is the engine
/// clock.
///
/// The first report of a run stamps its start; a report that ends it stamps its end. A running
/// run may report fresh counters. The same report again replays the stored run. A finished run is
/// sealed: any other report refuses (`TRANSITION_REFUSED` for an ending event, `CONNECTOR_RUN_INVALID`
/// for a return to running). A report may not move a run to another connection.
pub fn handle_connector_run_record(
    stored: Option<&ConnectorRun>,
    input: ConnectorRunInput,
    now_ms: u64,
) -> Result<ConnectorRunWrite, RunRefusal> {
    input.validate()?;
    let Some(held) = stored else {
        let finished_ms = match input.state.ending_event() {
            None => None,
            Some(event) => {
                CONNECTOR_RUN_MACHINE
                    .initial
                    .apply(event)
                    .map_err(RunRefusal::Transition)?;
                Some(now_ms)
            }
        };
        return Ok(ConnectorRunWrite {
            run: ConnectorRun {
                run_id: input.run_id,
                connection_id: input.connection_id,
                state: input.state,
                started_ms: now_ms,
                finished_ms,
                counts: input.counts,
            },
            replayed: false,
        });
    };
    if held.connection_id != input.connection_id {
        return invalid("connection_id (a run stays on its connection)");
    }
    if held.state == input.state && held.counts == input.counts {
        return Ok(ConnectorRunWrite {
            run: held.clone(),
            replayed: true,
        });
    }
    let mut run = held.clone();
    match input.state.ending_event() {
        // The same state again with fresh counters: only a running run reports progress.
        None if held.state == RunState::Running => {}
        None => return invalid("state (a finished run does not return to running)"),
        Some(event) => {
            run.state = held.state.apply(event).map_err(RunRefusal::Transition)?;
            run.finished_ms = Some(now_ms);
        }
    }
    run.counts = input.counts;
    Ok(ConnectorRunWrite {
        run,
        replayed: false,
    })
}

/// `ConnectorRunGet`: the stored run, or the one not-found refusal for an absent, foreign,
/// invisible or malformed run id.
pub fn handle_connector_run_get(
    stored: Option<ConnectorRun>,
    run_id: &str,
) -> Result<ConnectorRun, RunRefusal> {
    match stored {
        Some(run) if id_ok(run_id) => Ok(run),
        _ => Err(RunRefusal::run_not_found()),
    }
}

/// The resume key of a run in the newest-first order: the inverted start time, then the id.
fn resume_key(run: &ConnectorRun) -> String {
    format!("{:020}|{}", u64::MAX - run.started_ms, run.run_id)
}

/// `ConnectorRuns`: one page of `runs` (every run of `tenant`), newest first, resuming after
/// `cursor`, optionally of one connection and one state.
///
/// A named connection that no run of the tenant carries refuses `CONNECTION_NOT_FOUND`, checked
/// before the state filter. `limit` is 1 to [`MAX_RUN_LIST_LIMIT`] (the maximum when absent). A
/// cursor that is malformed or was minted for another tenant refuses `CONNECTOR_RUN_INVALID`.
pub fn handle_connector_runs(
    runs: Vec<ConnectorRun>,
    tenant: &str,
    connection_id: Option<&str>,
    state: Option<RunState>,
    limit: Option<u32>,
    cursor: Option<&str>,
) -> Result<ConnectorRunPage, RunRefusal> {
    let limit = match limit {
        None => MAX_RUN_LIST_LIMIT as usize,
        Some(0) => return invalid("limit"),
        Some(asked) if asked > MAX_RUN_LIST_LIMIT => return invalid("limit"),
        Some(asked) => asked as usize,
    };
    if let Some(connection) = connection_id {
        if !id_ok(connection) || !runs.iter().any(|run| run.connection_id == connection) {
            return Err(RunRefusal::connection_not_found());
        }
    }
    let after = cursor
        .map(|cursor| {
            LIST_CURSOR
                .decode(tenant, cursor, |key| !key.trim().is_empty())
                .map_err(RunRefusal::Invalid)
        })
        .transpose()?;
    let mut selected: Vec<ConnectorRun> = runs
        .into_iter()
        .filter(|run| connection_id.is_none_or(|connection| run.connection_id == connection))
        .filter(|run| state.is_none_or(|wanted| run.state == wanted))
        .collect();
    selected.sort_by_cached_key(resume_key);
    let rest: Vec<ConnectorRun> = selected
        .into_iter()
        .filter(|run| after.as_ref().is_none_or(|key| resume_key(run) > *key))
        .collect();
    let more = rest.len() > limit;
    let page: Vec<ConnectorRun> = rest.into_iter().take(limit).collect();
    let next_cursor = if more {
        page.last()
            .map(|last| LIST_CURSOR.encode(tenant, &resume_key(last)))
    } else {
        None
    };
    Ok(ConnectorRunPage {
        runs: page,
        next_cursor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn report(run: &str, connection: &str, state: RunState, records: u64) -> ConnectorRunInput {
        ConnectorRunInput {
            run_id: run.to_owned(),
            connection_id: connection.to_owned(),
            state,
            counts: BTreeMap::from([("records".to_owned(), records)]),
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R790
    #[test]
    fn a_run_starts_reports_progress_ends_and_is_then_sealed() {
        let started =
            handle_connector_run_record(None, report("r1", "c1", RunState::Running, 0), 10)
                .expect("start");
        assert_eq!(
            (started.run.started_ms, started.run.finished_ms),
            (10, None)
        );

        let progress = handle_connector_run_record(
            Some(&started.run),
            report("r1", "c1", RunState::Running, 40),
            20,
        )
        .expect("progress");
        assert!(!progress.replayed);
        assert_eq!(progress.run.started_ms, 10, "the start stays");
        assert_eq!(progress.run.counts["records"], 40);

        let ended = handle_connector_run_record(
            Some(&progress.run),
            report("r1", "c1", RunState::Succeeded, 90),
            30,
        )
        .expect("end");
        assert_eq!(
            (ended.run.state, ended.run.finished_ms),
            (RunState::Succeeded, Some(30))
        );

        // The same report replays; any other one refuses.
        let replay = handle_connector_run_record(
            Some(&ended.run),
            report("r1", "c1", RunState::Succeeded, 90),
            99,
        )
        .expect("replay");
        assert!(replay.replayed);
        assert_eq!(replay.run.finished_ms, Some(30), "the first end stays");
        let refused = handle_connector_run_record(
            Some(&ended.run),
            report("r1", "c1", RunState::Failed, 90),
            99,
        )
        .expect_err("sealed");
        assert_eq!(refused.code(), "TRANSITION_REFUSED");
        let back = handle_connector_run_record(
            Some(&ended.run),
            report("r1", "c1", RunState::Running, 91),
            99,
        )
        .expect_err("no return");
        assert_eq!(back.code(), CONNECTOR_RUN_INVALID);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R790
    #[test]
    fn a_run_reported_already_ended_stamps_one_instant_and_a_run_keeps_its_connection() {
        let instant = handle_connector_run_record(None, report("r1", "c1", RunState::Failed, 3), 7)
            .expect("an instant run");
        assert_eq!(
            (instant.run.started_ms, instant.run.finished_ms),
            (7, Some(7))
        );
        let moved = handle_connector_run_record(
            Some(&instant.run),
            report("r1", "c2", RunState::Failed, 3),
            8,
        )
        .expect_err("another connection");
        assert_eq!(moved.code(), CONNECTOR_RUN_INVALID);
        for bad in [
            report("", "c1", RunState::Running, 0),
            report("r 1", "c1", RunState::Running, 0),
            report("r1", &"c".repeat(129), RunState::Running, 0),
        ] {
            assert_eq!(
                handle_connector_run_record(None, bad, 1)
                    .expect_err("bad")
                    .code(),
                CONNECTOR_RUN_INVALID
            );
        }
        let mut loud = report("r1", "c1", RunState::Running, 0);
        loud.counts.insert("Not A Counter".to_owned(), 1);
        assert!(handle_connector_run_record(None, loud, 1).is_err());
    }

    fn run(id: &str, connection: &str, state: RunState, started: u64) -> ConnectorRun {
        ConnectorRun {
            run_id: id.to_owned(),
            connection_id: connection.to_owned(),
            state,
            started_ms: started,
            finished_ms: None,
            counts: BTreeMap::new(),
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R790
    #[test]
    fn the_list_filters_pages_newest_first_and_refuses_an_unknown_connection() {
        let runs = vec![
            run("a", "c1", RunState::Succeeded, 1),
            run("b", "c1", RunState::Failed, 3),
            run("c", "c2", RunState::Running, 2),
        ];
        let all =
            handle_connector_runs(runs.clone(), "acme", None, None, Some(2), None).expect("page");
        assert_eq!(
            all.runs
                .iter()
                .map(|r| r.run_id.as_str())
                .collect::<Vec<_>>(),
            ["b", "c"]
        );
        let cursor = all.next_cursor.expect("one remains");
        let rest = handle_connector_runs(runs.clone(), "acme", None, None, Some(2), Some(&cursor))
            .expect("page");
        assert_eq!(rest.runs.len(), 1);
        assert!(rest.next_cursor.is_none());
        let foreign =
            handle_connector_runs(runs.clone(), "globex", None, None, None, Some(&cursor))
                .expect_err("foreign cursor");
        assert_eq!(foreign.code(), CONNECTOR_RUN_INVALID);

        let one = handle_connector_runs(
            runs.clone(),
            "acme",
            Some("c1"),
            Some(RunState::Failed),
            None,
            None,
        )
        .expect("filtered");
        assert_eq!(one.runs.len(), 1);
        // A state with no match on a known connection is an empty page; an unknown connection
        // refuses, whether it is absent or only another tenant's.
        let empty = handle_connector_runs(
            runs.clone(),
            "acme",
            Some("c2"),
            Some(RunState::Failed),
            None,
            None,
        )
        .expect("empty");
        assert!(empty.runs.is_empty());
        let unknown = handle_connector_runs(runs.clone(), "acme", Some("ghost"), None, None, None)
            .expect_err("unknown");
        assert_eq!(unknown, RunRefusal::connection_not_found());
        assert_eq!(unknown.code(), CONNECTION_NOT_FOUND);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R790
    #[test]
    fn get_answers_one_refusal_for_an_absent_and_a_malformed_run_id() {
        let held = run("r1", "c1", RunState::Running, 1);
        assert_eq!(
            handle_connector_run_get(Some(held.clone()), "r1"),
            Ok(held.clone())
        );
        let absent = handle_connector_run_get(None, "r1").expect_err("absent");
        assert_eq!(absent.code(), CONNECTOR_RUN_NOT_FOUND);
        assert_eq!(handle_connector_run_get(Some(held), "bad id"), Err(absent));
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R790
    #[test]
    fn a_node_reads_as_a_run_only_for_its_tenant() {
        let node = json!({
            "type": "ConnectorRun", "tenant_id": "acme", "run_id": "r1", "connection_id": "c1",
            "state": "running", "started_ms": 5, "counts": "{\"records\":2}",
        });
        let read = ConnectorRun::from_node(&node, "acme").expect("a run");
        assert_eq!((read.state, read.counts["records"]), (RunState::Running, 2));
        assert!(ConnectorRun::from_node(&node, "globex").is_none());
        assert!(!connector_run_node_id("acme", "r1").contains("acme"));
        assert_ne!(
            connector_run_node_id("acme", "r1"),
            connector_run_node_id("globex", "r1")
        );
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R790
    #[test]
    fn every_legal_edge_is_taken_and_every_other_pair_is_refused() {
        for &state in CONNECTOR_RUN_MACHINE.states {
            for &event in CONNECTOR_RUN_MACHINE.events {
                let edge = CONNECTOR_RUN_MACHINE
                    .edges
                    .iter()
                    .find(|edge| edge.from == state && edge.event == event);
                match (edge, state.apply(event)) {
                    (Some(edge), result) => assert_eq!(result, Ok(edge.to)),
                    (None, Err(refused)) => {
                        assert_eq!(refused.machine, "ConnectorRunMachine");
                        assert_eq!(refused.from, state.stored());
                    }
                    (None, Ok(next)) => {
                        panic!("{state:?} {event:?} must be refused, took {next:?}")
                    }
                }
            }
        }
    }
}
