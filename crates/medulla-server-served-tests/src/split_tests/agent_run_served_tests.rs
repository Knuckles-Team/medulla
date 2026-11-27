// MEDULLA-DECISION-ENGINE-R901 and MEDULLA-UNIFIED-DATA-PLANE-R790 through the served dispatch:
// the agent goal reads and control, and the connector run list and report. A goal or a connection
// of another tenant refuses exactly like an absent one, every transition is the machine's, and
// every time is the engine clock.

use medulla_kernel::kernel::dispatch_support::with_engine_clock_offset;
use medulla_protocol_types::protocol::Method;
use medulla_source_types::agent_goal::{
    AgentGoal, GoalControlOp, GoalControlReceipt, GoalFinish, GoalIterations, GoalPage, GoalState,
    IterationOutcome,
};
use medulla_source_types::connector_run::{
    ConnectorRun, ConnectorRunInput, ConnectorRunPage, ConnectorRunReceipt, RunState,
};
use medulla_source_types::source_connection::{
    ConnectionState, MappingState, SourceConnection, SourceConnectionInput, SourceConnectionOp,
    SourceConnectionPage, SourceConnectionReceipt, SourceMapping, SourceMappingOp,
    SourceMappingOutcome,
};
use medulla_work_types::action_spec::{ActionList, ActionParameter, ActionSpec, ParamType};

use super::connector_pack_harness::{ok, Served, GRAPH, OTHER_TENANT, TENANT};
use super::deploy_record_served_tests::{admit, refusal_text};

const CONNECTOR: &str = "agent-run-source";
const STREAM: &str = "paper";
const OTHER_PRINCIPAL: &str = "usr:other-tenant-operator";
const CONTROL: &[&str] = &["runs:control"];
const GOAL_READ: &[&str] = &["agent:library-read"];
const INGEST_WRITE: &[&str] = &["ingest:write"];
const INGEST_READ: &[&str] = &["ingest:read"];
const DAY_MS: u64 = 86_400_000;

async fn served() -> Served {
    Served::provisioned(GRAPH, CONNECTOR, &[(STREAM, "ResearchPaper")]).await
}

fn create(id: &str) -> Method {
    Method::GoalControl {
        op: GoalControlOp::Create {
            goal_id: id.to_string(),
            title: format!("goal {id}"),
        },
    }
}

fn run(id: &str) -> Method {
    Method::GoalControl {
        op: GoalControlOp::Run {
            goal_id: id.to_string(),
        },
    }
}

fn cancel(id: &str) -> Method {
    Method::GoalControl {
        op: GoalControlOp::Cancel {
            goal_id: id.to_string(),
        },
    }
}

fn finish(id: &str, outcome: GoalFinish) -> Method {
    Method::GoalControl {
        op: GoalControlOp::Finish {
            goal_id: id.to_string(),
            outcome,
        },
    }
}

fn iterations(id: &str) -> Method {
    Method::GoalIterations {
        goal_id: id.to_string(),
    }
}

fn goal_list(limit: Option<u32>, cursor: Option<String>) -> Method {
    Method::GoalList { limit, cursor }
}

// spec: MEDULLA-DECISION-ENGINE-R901
#[tokio::test]
async fn a_goal_is_created_run_failed_retried_and_finished_through_the_machine() {
    let served = served().await;
    let made: GoalControlReceipt = ok(
        "create",
        served.verified(TENANT, CONTROL, create("g1")).await,
    );
    assert_eq!(
        (made.state, made.replayed, made.run_id.clone()),
        (GoalState::Queued, false, None)
    );
    let again: GoalControlReceipt = ok(
        "create again",
        with_engine_clock_offset(DAY_MS, served.verified(TENANT, CONTROL, create("g1"))).await,
    );
    assert!(again.replayed, "the same id and title replays");

    let first: GoalControlReceipt = ok("run", served.verified(TENANT, CONTROL, run("g1")).await);
    assert_eq!(
        (first.state, first.iteration),
        (GoalState::Running, Some(1))
    );
    let first_run = first.run_id.clone().expect("a run id");
    // A running goal cannot run again, and the refusal writes nothing.
    let twice = served.verified(TENANT, CONTROL, run("g1")).await;
    assert!(
        refusal_text(&twice).contains("TRANSITION_REFUSED"),
        "{twice:?}"
    );

    let failed: GoalControlReceipt = ok(
        "finish failed",
        with_engine_clock_offset(
            DAY_MS,
            served.verified(TENANT, CONTROL, finish("g1", GoalFinish::Failed)),
        )
        .await,
    );
    assert_eq!(failed.state, GoalState::Failed);
    let retry: GoalControlReceipt = ok(
        "retry",
        with_engine_clock_offset(2 * DAY_MS, served.verified(TENANT, CONTROL, run("g1"))).await,
    );
    assert_eq!(
        (retry.state, retry.iteration),
        (GoalState::Running, Some(2))
    );
    assert_ne!(retry.run_id, Some(first_run));
    let done: GoalControlReceipt = ok(
        "finish succeeded",
        served
            .verified(TENANT, CONTROL, finish("g1", GoalFinish::Succeeded))
            .await,
    );
    assert_eq!(done.state, GoalState::Succeeded);
    // A succeeded goal is terminal.
    let after = served.verified(TENANT, CONTROL, cancel("g1")).await;
    assert!(
        refusal_text(&after).contains("TRANSITION_REFUSED"),
        "{after:?}"
    );

    // The iterations read back in order, the first closed as failed by the engine clock.
    let read: GoalIterations = ok(
        "iterations",
        served.verified(TENANT, GOAL_READ, iterations("g1")).await,
    );
    assert_eq!(read.iterations.len(), 2);
    assert_eq!(read.iterations[0].iteration, 1);
    assert_eq!(read.iterations[0].outcome, IterationOutcome::Failed);
    let ended = read.iterations[0].ended_ms.expect("closed");
    assert!(
        ended >= read.iterations[0].started_ms + DAY_MS,
        "the engine clock stamps the end"
    );
    assert_eq!(read.iterations[1].outcome, IterationOutcome::Succeeded);
}

// spec: MEDULLA-DECISION-ENGINE-R901
#[tokio::test]
async fn the_goal_list_pages_newest_first_by_a_tenant_bound_cursor() {
    let served = served().await;
    for (id, offset) in [("a", 0), ("b", DAY_MS), ("c", 2 * DAY_MS)] {
        let _: GoalControlReceipt = ok(
            "create",
            with_engine_clock_offset(offset, served.verified(TENANT, CONTROL, create(id))).await,
        );
    }
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page: GoalPage = ok(
            "list",
            served
                .verified(TENANT, GOAL_READ, goal_list(Some(2), cursor.clone()))
                .await,
        );
        seen.extend(
            page.goals
                .iter()
                .map(|goal: &AgentGoal| goal.goal_id.clone()),
        );
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(seen, ["c", "b", "a"]);

    let minted = ok::<GoalPage>(
        "first page",
        served
            .verified(TENANT, GOAL_READ, goal_list(Some(1), None))
            .await,
    )
    .next_cursor
    .expect("two remain");
    admit(&served, OTHER_PRINCIPAL, false).await;
    let transplanted = served
        .verified_as(
            OTHER_PRINCIPAL,
            OTHER_TENANT,
            GOAL_READ,
            goal_list(None, Some(minted)),
        )
        .await;
    assert!(
        refusal_text(&transplanted).contains("GOAL_INVALID"),
        "{transplanted:?}"
    );
    let zero = served
        .verified(TENANT, GOAL_READ, goal_list(Some(0), None))
        .await;
    assert!(refusal_text(&zero).contains("GOAL_INVALID"), "{zero:?}");
}

// spec: MEDULLA-DECISION-ENGINE-R901
#[tokio::test]
async fn another_tenants_goal_refuses_exactly_like_an_absent_one() {
    let served = served().await;
    admit(&served, OTHER_PRINCIPAL, true).await;
    let _: GoalControlReceipt = ok(
        "create",
        served.verified(TENANT, CONTROL, create("g1")).await,
    );

    for method in [iterations("g1"), iterations("ghost")] {
        let refused = served
            .verified_as(OTHER_PRINCIPAL, OTHER_TENANT, GOAL_READ, method)
            .await;
        assert!(
            refusal_text(&refused).contains("GOAL_NOT_FOUND"),
            "{refused:?}"
        );
    }
    let foreign = served
        .verified_as(OTHER_PRINCIPAL, OTHER_TENANT, GOAL_READ, iterations("g1"))
        .await;
    let absent = served
        .verified_as(
            OTHER_PRINCIPAL,
            OTHER_TENANT,
            GOAL_READ,
            iterations("ghost"),
        )
        .await;
    assert_eq!(refusal_text(&foreign), refusal_text(&absent));
    let foreign_run = served
        .verified_as(OTHER_PRINCIPAL, OTHER_TENANT, CONTROL, run("g1"))
        .await;
    let absent_run = served
        .verified_as(OTHER_PRINCIPAL, OTHER_TENANT, CONTROL, run("ghost"))
        .await;
    assert!(
        refusal_text(&foreign_run).contains("GOAL_NOT_FOUND"),
        "{foreign_run:?}"
    );
    assert_eq!(refusal_text(&foreign_run), refusal_text(&absent_run));
    let page: GoalPage = ok(
        "list as the other tenant",
        served
            .verified_as(
                OTHER_PRINCIPAL,
                OTHER_TENANT,
                GOAL_READ,
                goal_list(None, None),
            )
            .await,
    );
    assert!(page.goals.is_empty(), "{page:?}");

    // The same id is the other tenant's own goal; ours is untouched.
    let theirs: GoalControlReceipt = ok(
        "create as the other tenant",
        served
            .verified_as(OTHER_PRINCIPAL, OTHER_TENANT, CONTROL, create("g1"))
            .await,
    );
    assert!(!theirs.replayed);
    let ours: GoalPage = ok(
        "list",
        served
            .verified(TENANT, GOAL_READ, goal_list(None, None))
            .await,
    );
    assert_eq!(ours.goals.len(), 1);
    assert_eq!(ours.goals[0].tenant_id, TENANT);
}

// spec: MEDULLA-DECISION-ENGINE-R901
#[tokio::test]
async fn goal_control_needs_runs_control_and_the_reads_need_the_library_read_scope() {
    let served = served().await;
    let by_reader = served.verified(TENANT, GOAL_READ, create("g1")).await;
    assert!(
        by_reader.error.is_some(),
        "a read scope must not control: {by_reader:?}"
    );
    let by_controller = served
        .verified(TENANT, CONTROL, goal_list(None, None))
        .await;
    assert!(
        by_controller.error.is_some(),
        "a control scope must not list: {by_controller:?}"
    );
    let reading = served.verified(TENANT, CONTROL, iterations("g1")).await;
    assert!(
        reading.error.is_some(),
        "a control scope must not read: {reading:?}"
    );
}

fn report(run_id: &str, connection: &str, state: RunState, records: u64) -> Method {
    Method::ConnectorRunRecord {
        run: ConnectorRunInput {
            run_id: run_id.to_string(),
            connection_id: connection.to_string(),
            state,
            counts: std::collections::BTreeMap::from([("records".to_string(), records)]),
        },
    }
}

fn runs(connection: Option<&str>, state: Option<RunState>) -> Method {
    Method::ConnectorRuns {
        connection_id: connection.map(str::to_string),
        state,
        limit: None,
        cursor: None,
    }
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R790
#[tokio::test]
async fn a_run_is_reported_ends_sealed_and_lists_newest_first_by_connection_and_state() {
    let served = served().await;
    let start: ConnectorRunReceipt = ok(
        "start",
        served
            .verified(
                TENANT,
                INGEST_WRITE,
                report("r1", "c1", RunState::Running, 0),
            )
            .await,
    );
    assert_eq!(
        (start.state, start.finished_ms, start.replayed),
        (RunState::Running, None, false)
    );
    assert!(start.started_ms > 0, "the engine clock stamps the start");
    let ended: ConnectorRunReceipt = ok(
        "end",
        with_engine_clock_offset(
            DAY_MS,
            served.verified(
                TENANT,
                INGEST_WRITE,
                report("r1", "c1", RunState::Succeeded, 12),
            ),
        )
        .await,
    );
    assert_eq!(ended.started_ms, start.started_ms);
    assert!(ended.finished_ms.expect("ended") >= start.started_ms + DAY_MS);
    let replay: ConnectorRunReceipt = ok(
        "replay",
        served
            .verified(
                TENANT,
                INGEST_WRITE,
                report("r1", "c1", RunState::Succeeded, 12),
            )
            .await,
    );
    assert!(replay.replayed);
    let sealed = served
        .verified(
            TENANT,
            INGEST_WRITE,
            report("r1", "c1", RunState::Failed, 12),
        )
        .await;
    assert!(
        refusal_text(&sealed).contains("TRANSITION_REFUSED"),
        "{sealed:?}"
    );
    let moved = served
        .verified(
            TENANT,
            INGEST_WRITE,
            report("r1", "c2", RunState::Succeeded, 12),
        )
        .await;
    assert!(
        refusal_text(&moved).contains("CONNECTOR_RUN_INVALID"),
        "{moved:?}"
    );

    let _: ConnectorRunReceipt = ok(
        "second connection",
        with_engine_clock_offset(
            2 * DAY_MS,
            served.verified(
                TENANT,
                INGEST_WRITE,
                report("r2", "c2", RunState::Running, 1),
            ),
        )
        .await,
    );
    let all: ConnectorRunPage = ok(
        "all",
        served.verified(TENANT, INGEST_READ, runs(None, None)).await,
    );
    assert_eq!(
        all.runs
            .iter()
            .map(|run| run.run_id.as_str())
            .collect::<Vec<_>>(),
        ["r2", "r1"]
    );
    let one: ConnectorRunPage = ok(
        "one",
        served
            .verified(TENANT, INGEST_READ, runs(Some("c1"), None))
            .await,
    );
    assert_eq!(one.runs.len(), 1);
    assert_eq!(one.runs[0].counts["records"], 12);
    let none: ConnectorRunPage = ok(
        "no failed run",
        served
            .verified(
                TENANT,
                INGEST_READ,
                runs(Some("c1"), Some(RunState::Failed)),
            )
            .await,
    );
    assert!(none.runs.is_empty());
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R790
#[tokio::test]
async fn another_tenants_connection_refuses_exactly_like_an_absent_one_and_scopes_are_enforced() {
    let served = served().await;
    admit(&served, OTHER_PRINCIPAL, true).await;
    let _: ConnectorRunReceipt = ok(
        "start",
        served
            .verified(
                TENANT,
                INGEST_WRITE,
                report("r1", "c1", RunState::Running, 0),
            )
            .await,
    );
    let foreign = served
        .verified_as(
            OTHER_PRINCIPAL,
            OTHER_TENANT,
            INGEST_READ,
            runs(Some("c1"), None),
        )
        .await;
    let absent = served
        .verified_as(
            OTHER_PRINCIPAL,
            OTHER_TENANT,
            INGEST_READ,
            runs(Some("ghost"), None),
        )
        .await;
    assert!(
        refusal_text(&foreign).contains("CONNECTION_NOT_FOUND"),
        "{foreign:?}"
    );
    assert_eq!(refusal_text(&foreign), refusal_text(&absent));
    let page: ConnectorRunPage = ok(
        "list as the other tenant",
        served
            .verified_as(OTHER_PRINCIPAL, OTHER_TENANT, INGEST_READ, runs(None, None))
            .await,
    );
    assert!(page.runs.is_empty(), "{page:?}");

    let by_reader = served
        .verified(
            TENANT,
            INGEST_READ,
            report("r3", "c1", RunState::Running, 0),
        )
        .await;
    assert!(
        by_reader.error.is_some(),
        "a read scope must not report: {by_reader:?}"
    );
    let by_writer = served
        .verified(TENANT, INGEST_WRITE, runs(None, None))
        .await;
    assert!(
        by_writer.error.is_some(),
        "a write scope must not list: {by_writer:?}"
    );
}

fn connection_input(id: &str, credential: &str) -> SourceConnectionInput {
    SourceConnectionInput {
        connection_id: id.to_string(),
        provider: "github".to_string(),
        label: "Main".to_string(),
        settings: serde_json::json!({"org": "acme"})
            .as_object()
            .cloned()
            .expect("object"),
        credential_ref: credential.to_string(),
        state: None,
    }
}

fn put_connection(id: &str, credential: &str) -> Method {
    Method::SourceConnectionControl {
        op: SourceConnectionOp::Put {
            connection: connection_input(id, credential),
        },
    }
}

fn get_connection(id: &str) -> Method {
    Method::SourceConnectionGet {
        connection_id: id.to_string(),
    }
}

fn propose(id: &str, title: &str) -> Method {
    Method::SourceMappingControl {
        op: SourceMappingOp::Propose {
            connection_id: id.to_string(),
            mapping: serde_json::json!({"title": title})
                .as_object()
                .cloned()
                .expect("object"),
        },
    }
}

fn approve(id: &str, digest: &str) -> Method {
    Method::SourceMappingControl {
        op: SourceMappingOp::Approve {
            connection_id: id.to_string(),
            mapping_digest: digest.to_string(),
        },
    }
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R791
#[tokio::test]
async fn a_connection_keeps_a_locator_lists_by_id_and_deleting_it_removes_its_mapping() {
    let served = served().await;
    // A credential value is refused and nothing is stored.
    let value = served
        .verified(
            TENANT,
            INGEST_WRITE,
            put_connection("c1", "ghp_0123456789abcdef"),
        )
        .await;
    assert!(
        refusal_text(&value).contains("SOURCE_CONNECTION_INVALID"),
        "{value:?}"
    );
    let missing = served
        .verified(TENANT, INGEST_READ, get_connection("c1"))
        .await;
    assert!(
        refusal_text(&missing).contains("CONNECTION_NOT_FOUND"),
        "{missing:?}"
    );

    let first: SourceConnectionReceipt = ok(
        "put",
        served
            .verified(
                TENANT,
                INGEST_WRITE,
                put_connection("c1", "openbao://kv/github"),
            )
            .await,
    );
    assert!(!first.replayed && !first.deleted);
    let again: SourceConnectionReceipt = ok(
        "put again",
        served
            .verified(
                TENANT,
                INGEST_WRITE,
                put_connection("c1", "openbao://kv/github"),
            )
            .await,
    );
    assert!(again.replayed, "the same body replays");
    let _: SourceConnectionReceipt = ok(
        "second",
        served
            .verified(
                TENANT,
                INGEST_WRITE,
                put_connection("c0", "env://GITHUB_TOKEN"),
            )
            .await,
    );
    let read: SourceConnection = ok(
        "get",
        served
            .verified(TENANT, INGEST_READ, get_connection("c1"))
            .await,
    );
    assert_eq!(
        (read.state, read.credential_ref.as_str()),
        (ConnectionState::Active, "openbao://kv/github")
    );
    assert_eq!(read.tenant_id, TENANT);
    let page: SourceConnectionPage = ok(
        "list",
        served
            .verified(
                TENANT,
                INGEST_READ,
                Method::SourceConnectionList {
                    limit: None,
                    cursor: None,
                },
            )
            .await,
    );
    assert_eq!(
        page.connections
            .iter()
            .map(|c| c.connection_id.as_str())
            .collect::<Vec<_>>(),
        ["c0", "c1"]
    );

    // A mapping is proposed and read back; deleting the connection removes it.
    let proposed: SourceMappingOutcome = ok(
        "propose",
        served
            .verified(TENANT, INGEST_WRITE, propose("c1", "name"))
            .await,
    );
    assert_eq!(proposed.state, MappingState::Proposed);
    let deleted: SourceConnectionReceipt = ok(
        "delete",
        served
            .verified(
                TENANT,
                INGEST_WRITE,
                Method::SourceConnectionControl {
                    op: SourceConnectionOp::Delete {
                        connection_id: "c1".to_string(),
                    },
                },
            )
            .await,
    );
    assert!(deleted.deleted);
    let gone = served
        .verified(TENANT, INGEST_READ, get_connection("c1"))
        .await;
    assert!(
        refusal_text(&gone).contains("CONNECTION_NOT_FOUND"),
        "{gone:?}"
    );
    let no_mapping = served
        .verified(
            TENANT,
            INGEST_READ,
            Method::SourceMappingGet {
                connection_id: "c1".to_string(),
            },
        )
        .await;
    assert!(
        refusal_text(&no_mapping).contains("CONNECTION_NOT_FOUND"),
        "{no_mapping:?}"
    );
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R791
#[tokio::test]
async fn a_mapping_is_approved_only_at_the_digest_that_was_reviewed() {
    let served = served().await;
    let _: SourceConnectionReceipt = ok(
        "put",
        served
            .verified(TENANT, INGEST_WRITE, put_connection("c1", "env://TOKEN"))
            .await,
    );
    let none = served
        .verified(
            TENANT,
            INGEST_READ,
            Method::SourceMappingGet {
                connection_id: "c1".to_string(),
            },
        )
        .await;
    assert!(
        refusal_text(&none).contains("MAPPING_NOT_FOUND"),
        "{none:?}"
    );

    let first: SourceMappingOutcome = ok(
        "propose",
        served
            .verified(TENANT, INGEST_WRITE, propose("c1", "name"))
            .await,
    );
    // The mapping is replaced before the approver acts: the reviewed digest is stale.
    let second: SourceMappingOutcome = ok(
        "repropose",
        served
            .verified(TENANT, INGEST_WRITE, propose("c1", "headline"))
            .await,
    );
    assert_ne!(first.digest, second.digest);
    let stale = served
        .verified(TENANT, INGEST_WRITE, approve("c1", &first.digest))
        .await;
    assert!(
        refusal_text(&stale).contains("MAPPING_CHANGED"),
        "{stale:?}"
    );
    let approved: SourceMappingOutcome = ok(
        "approve",
        served
            .verified(TENANT, INGEST_WRITE, approve("c1", &second.digest))
            .await,
    );
    assert_eq!(approved.state, MappingState::Approved);
    let read: SourceMapping = ok(
        "get",
        served
            .verified(
                TENANT,
                INGEST_READ,
                Method::SourceMappingGet {
                    connection_id: "c1".to_string(),
                },
            )
            .await,
    );
    assert_eq!(
        (read.state, read.digest),
        (MappingState::Approved, second.digest.clone())
    );
    // The same proposal keeps it approved; a changed one proposes again.
    let same: SourceMappingOutcome = ok(
        "same",
        served
            .verified(TENANT, INGEST_WRITE, propose("c1", "headline"))
            .await,
    );
    assert!(same.replayed && same.state == MappingState::Approved);
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R791
#[tokio::test]
async fn another_tenants_connection_and_mapping_refuse_exactly_like_absent_ones() {
    let served = served().await;
    admit(&served, OTHER_PRINCIPAL, true).await;
    let _: SourceConnectionReceipt = ok(
        "put",
        served
            .verified(TENANT, INGEST_WRITE, put_connection("c1", "env://TOKEN"))
            .await,
    );
    let foreign = served
        .verified_as(
            OTHER_PRINCIPAL,
            OTHER_TENANT,
            INGEST_READ,
            get_connection("c1"),
        )
        .await;
    let absent = served
        .verified_as(
            OTHER_PRINCIPAL,
            OTHER_TENANT,
            INGEST_READ,
            get_connection("ghost"),
        )
        .await;
    assert!(
        refusal_text(&foreign).contains("CONNECTION_NOT_FOUND"),
        "{foreign:?}"
    );
    assert_eq!(refusal_text(&foreign), refusal_text(&absent));
    let foreign_map = served
        .verified_as(
            OTHER_PRINCIPAL,
            OTHER_TENANT,
            INGEST_WRITE,
            propose("c1", "x"),
        )
        .await;
    assert!(
        refusal_text(&foreign_map).contains("CONNECTION_NOT_FOUND"),
        "{foreign_map:?}"
    );
    let page: SourceConnectionPage = ok(
        "list as the other tenant",
        served
            .verified_as(
                OTHER_PRINCIPAL,
                OTHER_TENANT,
                INGEST_READ,
                Method::SourceConnectionList {
                    limit: None,
                    cursor: None,
                },
            )
            .await,
    );
    assert!(page.connections.is_empty(), "{page:?}");
    let by_reader = served
        .verified(TENANT, INGEST_READ, put_connection("c2", "env://TOKEN"))
        .await;
    assert!(
        by_reader.error.is_some(),
        "a read scope must not write: {by_reader:?}"
    );
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R790
#[tokio::test]
async fn a_run_reads_back_by_id_and_a_foreign_run_refuses_like_an_absent_one() {
    let served = served().await;
    admit(&served, OTHER_PRINCIPAL, false).await;
    let _: ConnectorRunReceipt = ok(
        "start",
        served
            .verified(
                TENANT,
                INGEST_WRITE,
                report("r1", "c1", RunState::Running, 4),
            )
            .await,
    );
    let get = |id: &str| Method::ConnectorRunGet {
        run_id: id.to_string(),
    };
    let read: ConnectorRun = ok("get", served.verified(TENANT, INGEST_READ, get("r1")).await);
    assert_eq!((read.state, read.counts["records"]), (RunState::Running, 4));
    // Cancelling is reporting the run cancelled.
    let cancelled: ConnectorRunReceipt = ok(
        "cancel",
        served
            .verified(
                TENANT,
                INGEST_WRITE,
                report("r1", "c1", RunState::Cancelled, 4),
            )
            .await,
    );
    assert_eq!(cancelled.state, RunState::Cancelled);
    let foreign = served
        .verified_as(OTHER_PRINCIPAL, OTHER_TENANT, INGEST_READ, get("r1"))
        .await;
    let absent = served
        .verified_as(OTHER_PRINCIPAL, OTHER_TENANT, INGEST_READ, get("ghost"))
        .await;
    assert!(
        refusal_text(&foreign).contains("CONNECTOR_RUN_NOT_FOUND"),
        "{foreign:?}"
    );
    assert_eq!(refusal_text(&foreign), refusal_text(&absent));
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R792
#[tokio::test]
async fn the_action_list_names_each_registered_action_with_its_parameter_schema() {
    let served = served().await;
    let empty: ActionList = ok(
        "list none",
        served
            .verified(TENANT, &["actions:read"], Method::ActionList {})
            .await,
    );
    assert!(empty.actions.is_empty());
    for name in ["zeta", "alpha"] {
        let registered = served
            .verified(
                TENANT,
                &["actions:write"],
                Method::ActionRegister {
                    spec: ActionSpec {
                        name: name.to_string(),
                        params: vec![ActionParameter {
                            name: "amount".to_string(),
                            ty: ParamType::Integer,
                            required: true,
                        }],
                        criteria: Vec::new(),
                        effects: Vec::new(),
                    },
                },
            )
            .await;
        assert!(registered.error.is_none(), "{registered:?}");
    }
    let listed: ActionList = ok(
        "list",
        served
            .verified(TENANT, &["actions:read"], Method::ActionList {})
            .await,
    );
    assert_eq!(
        listed
            .actions
            .iter()
            .map(|action| action.name.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "zeta"]
    );
    assert_eq!(
        listed.actions[0].params_schema["properties"]["amount"]["type"],
        serde_json::json!("integer")
    );
    assert_eq!(
        listed.actions[0].params_schema["required"],
        serde_json::json!(["amount"])
    );
    let by_writer = served
        .verified(TENANT, &["actions:write"], Method::ActionList {})
        .await;
    assert!(
        by_writer.error.is_some(),
        "a write scope must not list: {by_writer:?}"
    );
}
