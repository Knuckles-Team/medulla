//! Skill telemetry served through `Method::SkillTelemetry` (MEDULLA-DECISION-ENGINE-R713.3 to
//! R713.7): a batch is stored whole or not at all, the twelve measures follow the stored
//! invocations, skills rank by their evaluations, peers are counted and never named, and run
//! traces list by eval outcome.

use super::super::planned_write::PlannedWrite;
use super::super::skill_telemetry::{handle_skill_telemetry, invocation_node_id};
use super::*;
use medulla_decision_types::skill_eval::{SuiteBand, SuiteReport};
use medulla_decision_types::skill_telemetry::{
    principal_pseudonym, EvalOutcome, IngestReceipt, PeersAnswer, RankAnswer, SkillMeasures,
    SkillTelemetryOp, TracesAnswer, Window, PEER_COHORT_TOO_SMALL, TELEMETRY_BATCH_TOO_LARGE,
    TELEMETRY_CLOCK_INVALID, TELEMETRY_FIELD_UNKNOWN, TELEMETRY_SCOPE_REQUIRED,
    TELEMETRY_SESSION_UNKNOWN, TELEMETRY_SKILL_UNKNOWN,
};
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_persistence_records::decision_jobs::encode_artifact;
use medulla_protocol_types::protocol::ResultPayload;
use serde_json::{json, Value};

const GRAPH: &str = "__commons__";
const DAY: u64 = 24 * 60 * 60 * 1000;
const SKILL: &str = "skill:triage@3";

fn who() -> VerifiedRequestContext {
    VerifiedRequestContext::verified_for_test_with_scopes(
        "agent-7",
        TENANT,
        &["telemetry:write", "telemetry:read"],
    )
}

fn reader_only() -> VerifiedRequestContext {
    VerifiedRequestContext::verified_for_test_with_scopes("agent-7", TENANT, &["telemetry:read"])
}

async fn core(h: &Harness) -> std::sync::Arc<medulla_graph_core::graph::GraphCore> {
    h.state.read().await.registry.get(GRAPH).expect("graph exists").core.clone()
}

fn put(core: &medulla_graph_core::graph::GraphCore, id: &str, props: Value) {
    core.add_node(id.to_string(), rmp_serde::to_vec_named(&props).unwrap());
}

fn commit(core: &medulla_graph_core::graph::GraphCore, write: &PlannedWrite) {
    for operation in &write.operations {
        match operation["op"].as_str() {
            Some("upsert_node") => put(
                core,
                operation["id"].as_str().unwrap(),
                operation["properties"].clone(),
            ),
            Some("upsert_edge") => {
                core.add_edge(
                    operation["source"].as_str().unwrap().to_string(),
                    operation["target"].as_str().unwrap().to_string(),
                    rmp_serde::to_vec_named(&operation["properties"]).unwrap(),
                )
                .unwrap();
            }
            other => panic!("unexpected operation {other:?}"),
        }
    }
}

fn reply<T: serde::de::DeserializeOwned>(write: &PlannedWrite) -> T {
    match &write.reply {
        ResultPayload::Raw(bytes) => rmp_serde::from_slice(bytes).unwrap(),
        other => panic!("unexpected payload {other:?}"),
    }
}

async fn run(
    h: &Harness,
    caller: &VerifiedRequestContext,
    op: SkillTelemetryOp,
) -> Result<PlannedWrite, String> {
    handle_skill_telemetry(&h.state, caller, op).await
}

fn event(id: &str, skill: &str, user: &str, day: u64, run_id: &str, error: bool) -> Value {
    json!({
        "invocation_id": id, "tenant_id": "ignored", "skill_ref": skill, "skill_version": "3",
        "chosen_by": if day % 2 == 0 { "agent" } else { "user" },
        "principal_pseudonym": principal_pseudonym(TENANT, user, 1),
        "run_id": run_id, "started_ms": day * DAY, "ended_ms": day * DAY + 100 + day,
        "outcome": if error { json!({"error": {"code": "TOOL_TIMEOUT"}}) } else { json!("ok") },
        "cost_delta_micro_usd": 10, "tokens_in": 5, "tokens_out": 6
    })
}

fn window() -> Window {
    Window { start_ms: 0, end_ms: 30 * DAY }
}

async fn seed(h: &Harness) -> std::sync::Arc<medulla_graph_core::graph::GraphCore> {
    let core = core(h).await;
    for skill in [SKILL, "skill:other@1", "skill:fresh@1"] {
        put(&core, skill, json!({"type": "Skill", "tenant_id": TENANT}));
    }
    for run_id in ["run-a", "run-b", "run-c"] {
        put(
            &core,
            &format!("trace-{run_id}"),
            json!({"node_type": "RunTrace", "run_id": run_id, "tenant": TENANT}),
        );
    }
    core
}

fn ingest_op(events: Vec<Value>) -> SkillTelemetryOp {
    SkillTelemetryOp::Ingest { graph: GRAPH.into(), events }
}

// spec: MEDULLA-DECISION-ENGINE-R713.3
#[tokio::test]
async fn ingest_then_measures_round_trip() {
    let h = Harness::with_isolation(ServerState::test_isolation("agent-7")).await;
    let core = seed(&h).await;
    let caller = who();
    let events = vec![
        event("i1", SKILL, "u1", 1, "run-a", false),
        event("i2", SKILL, "u1", 29, "run-b", false),
        event("i3", SKILL, "u2", 2, "run-a", true),
        event("i4", SKILL, "u3", 20, "run-c", false),
        event("i5", SKILL, "u3", 29, "run-c", false),
    ];
    let write = run(&h, &caller, ingest_op(events.clone())).await.expect("ingests");
    let receipt: IngestReceipt = reply(&write);
    assert_eq!(receipt, IngestReceipt { stored: 5, repeated: 0 });
    // Each stored invocation links to its run trace.
    assert!(write.operations.iter().any(|op| op["op"] == "upsert_edge" && op["target"] == "trace-run-a"));
    commit(&core, &write);
    // A repeat of the same ids stores nothing.
    let again = run(&h, &caller, ingest_op(events)).await.expect("repeats");
    assert_eq!(reply::<IngestReceipt>(&again), IngestReceipt { stored: 0, repeated: 5 });
    assert!(again.operations.is_empty());

    // The suite report of R711 feeds the eval measures.
    let store = h.state.write().await.ensure_agent_library().unwrap();
    let report = SuiteReport {
        golden_ppm: 0,
        real_ppm: 1_000_000,
        judged_ppm: 0,
        pass_rate_ppm: 800_000,
        band: SuiteBand::Healthy,
        case_count: 40,
    };
    store
        .put_decision_artifacts(TENANT, &[(format!("skill-eval-report:{SKILL}"), encode_artifact(&report).unwrap())])
        .unwrap();
    let measured: SkillMeasures = reply(
        &run(
            &h,
            &reader_only(),
            SkillTelemetryOp::Measures { graph: GRAPH.into(), skill_ref: SKILL.into(), window: window() },
        )
        .await
        .expect("reads"),
    );
    assert_eq!(measured.invocations, 5);
    assert_eq!(measured.distinct_users, 3);
    assert_eq!(measured.retained_users_ppm, 500_000);
    assert_eq!(measured.error_rate_ppm, 200_000);
    assert_eq!(measured.eval_pass_rate_ppm, 800_000);
    assert_eq!(measured.eval_runs, 40);
    assert_eq!(measured.latency_p50_ms, Some(120));

    // Another tenant sees none of it, and a reader cannot ingest.
    let other = VerifiedRequestContext::verified_for_test_with_scopes("agent-7", "tenant-b", &["telemetry:read"]);
    let none: SkillMeasures = reply(
        &run(
            &h,
            &other,
            SkillTelemetryOp::Measures { graph: GRAPH.into(), skill_ref: SKILL.into(), window: window() },
        )
        .await
        .unwrap(),
    );
    assert_eq!(none.invocations, 0);
    let refused = run(&h, &reader_only(), ingest_op(vec![])).await.err().expect("refuses");
    assert!(refused.starts_with(TELEMETRY_SCOPE_REQUIRED.as_str()), "{refused}");
}

// spec: MEDULLA-DECISION-ENGINE-R713.8
#[tokio::test]
async fn ingest_files_each_invocation_under_the_read_index_keys() {
    let h = Harness::with_isolation(ServerState::test_isolation("agent-7")).await;
    let core = seed(&h).await;
    let caller = who();
    let events = vec![
        event("i1", SKILL, "u1", 1, "run-a", false),
        event("i2", SKILL, "u2", 1, "run-b", false),
        event("i3", "skill:other@1", "u1", 5, "run-c", false),
    ];
    let write = run(&h, &caller, ingest_op(events)).await.expect("ingests");
    commit(&core, &write);

    // The three keys of the read index are stored on the rows the ingest batch wrote.
    let by = |key: &str, value: &str| {
        let mut ids = core.nodes_by_property(key, value).expect("the key is indexed");
        ids.sort();
        ids
    };
    let node = |id: &str| invocation_node_id(TENANT, id);
    assert_eq!(by("skill_ref", SKILL), [node("i1"), node("i2")]);
    assert_eq!(by("principal_pseudonym", &principal_pseudonym(TENANT, "u1", 1)).len(), 2);
    assert_eq!(by("started_day", "1"), [node("i1"), node("i2")]);
    assert_eq!(by("started_day", "5"), [node("i3")]);

    // A read narrowed by skill and window answers from those keys: a window of days 0 to 1 holds
    // the two rows of the skill, a window of day 5 alone holds none of them.
    let measures = |window: Window| {
        let (h, caller) = (&h, reader_only());
        async move {
            reply::<SkillMeasures>(
                &run(h, &caller, SkillTelemetryOp::Measures { graph: GRAPH.into(), skill_ref: SKILL.into(), window })
                    .await
                    .expect("reads"),
            )
        }
    };
    assert_eq!(measures(Window { start_ms: 0, end_ms: 2 * DAY }).await.invocations, 2);
    assert_eq!(measures(Window { start_ms: 5 * DAY, end_ms: 6 * DAY }).await.invocations, 0);
    // A window that ends inside a day still reads that day's rows it contains.
    assert_eq!(measures(Window { start_ms: DAY, end_ms: DAY + 1 }).await.invocations, 2);
    // A window longer than the index looks up by day reads by skill alone, with the same answer.
    assert_eq!(measures(Window { start_ms: 0, end_ms: 400 * DAY }).await.invocations, 2);
}

// spec: MEDULLA-DECISION-ENGINE-R713.4
#[tokio::test]
async fn bad_event_refuses_whole_batch() {
    let h = Harness::with_isolation(ServerState::test_isolation("agent-7")).await;
    seed(&h).await;
    let caller = who();
    let good = event("g1", SKILL, "u1", 1, "run-a", false);
    let refuse = |result: Result<PlannedWrite, String>| result.err().expect("must refuse");

    let mut extra = event("b1", SKILL, "u1", 1, "run-a", false);
    extra["prompt"] = "please summarise my notes".into();
    let error = refuse(run(&h, &caller, ingest_op(vec![good.clone(), extra])).await);
    assert!(error.starts_with(TELEMETRY_FIELD_UNKNOWN.as_str()), "{error}");

    let unknown = event("b2", "skill:nowhere@1", "u1", 1, "run-a", false);
    let error = refuse(run(&h, &caller, ingest_op(vec![good.clone(), unknown])).await);
    assert!(error.starts_with(TELEMETRY_SKILL_UNKNOWN.as_str()), "{error}");

    let mut backwards = event("b3", SKILL, "u1", 5, "run-a", false);
    backwards["ended_ms"] = 1.into();
    let error = refuse(run(&h, &caller, ingest_op(vec![good.clone(), backwards])).await);
    assert!(error.starts_with(TELEMETRY_CLOCK_INVALID.as_str()), "{error}");

    let many: Vec<Value> = (0..501).map(|n| event(&format!("m{n}"), SKILL, "u1", 1, "run-a", false)).collect();
    let error = refuse(run(&h, &caller, ingest_op(many)).await);
    assert!(error.starts_with(TELEMETRY_BATCH_TOO_LARGE.as_str()), "{error}");

    // Nothing was stored: the good event is still new.
    let write = run(&h, &caller, ingest_op(vec![good])).await.unwrap();
    assert_eq!(reply::<IngestReceipt>(&write).stored, 1);
}

// spec: MEDULLA-DECISION-ENGINE-R713.5
#[tokio::test]
async fn quality_orders_by_evals_then_runs() {
    let h = Harness::with_isolation(ServerState::test_isolation("agent-7")).await;
    let core = seed(&h).await;
    let caller = who();
    let events = vec![
        event("i1", SKILL, "u1", 1, "run-a", false),
        event("i2", "skill:other@1", "u1", 2, "run-a", false),
        event("i3", "skill:other@1", "u2", 3, "run-a", false),
        event("i4", "skill:fresh@1", "u1", 4, "run-a", false),
    ];
    commit(&core, &run(&h, &caller, ingest_op(events)).await.unwrap());
    let store = h.state.write().await.ensure_agent_library().unwrap();
    for (skill, pass, cases) in [(SKILL, 1_000_000, 80), ("skill:other@1", 1_000_000, 20)] {
        let report = SuiteReport {
            golden_ppm: 0,
            real_ppm: 1_000_000,
            judged_ppm: 0,
            pass_rate_ppm: pass,
            band: SuiteBand::Healthy,
            case_count: cases,
        };
        store
            .put_decision_artifacts(TENANT, &[(format!("skill-eval-report:{skill}"), encode_artifact(&report).unwrap())])
            .unwrap();
    }
    let ranked: RankAnswer = reply(
        &run(
            &h,
            &caller,
            SkillTelemetryOp::Rank { graph: GRAPH.into(), skill_refs: Vec::new(), window: window(), limit: None },
        )
        .await
        .unwrap(),
    );
    let order: Vec<(&str, u64)> = ranked.skills.iter().map(|s| (s.skill_ref.as_str(), s.quality_ppm)).collect();
    // 80 of 100 weighted runs, then 20 of 40, then the skill with no evaluation last.
    assert_eq!(order, [(SKILL, 800_000), ("skill:other@1", 500_000), ("skill:fresh@1", 0)]);
}

// spec: MEDULLA-DECISION-ENGINE-R713.6
#[tokio::test]
async fn peers_never_named_and_small_cohort_refused() {
    let caller = who();
    let me = principal_pseudonym(TENANT, &caller.principal_persistence_id(), 1);
    // The caller and `count` peers share the skill `SKILL`; every peer also used `skill:other@1`.
    let ingest_for = |count: usize| {
        let mut events = vec![event("m1", SKILL, "ignored", 1, "run-a", false)];
        events[0]["principal_pseudonym"] = me.clone().into();
        for n in 1..=count {
            events.push(event(&format!("s{n}"), SKILL, &format!("peer-{n}"), 2, "run-a", false));
            events.push(event(&format!("o{n}"), "skill:other@1", &format!("peer-{n}"), 2, "run-a", false));
        }
        events
    };

    let h = Harness::with_isolation(ServerState::test_isolation("agent-7")).await;
    let core = seed(&h).await;
    commit(&core, &run(&h, &caller, ingest_op(ingest_for(5))).await.unwrap());
    // The caller names no cohort: the engine computes it from the telemetry of the same class.
    let answer: PeersAnswer = reply(
        &run(&h, &caller, SkillTelemetryOp::Peers { graph: GRAPH.into(), window: window() })
            .await
            .unwrap(),
    );
    assert_eq!(answer.skills.len(), 1);
    assert_eq!(answer.skills[0].skill_ref, "skill:other@1");
    assert_eq!(answer.skills[0].cohort_users, 5);
    // No pseudonym appears in the answer.
    let text = serde_json::to_string(&answer).unwrap();
    assert!((1..=5).all(|n| !text.contains(&principal_pseudonym(TENANT, &format!("peer-{n}"), 1))));
    assert!(!text.contains(&me));

    // A class with four other principals is under the floor of five.
    let small = Harness::with_isolation(ServerState::test_isolation("agent-7")).await;
    let small_core = seed(&small).await;
    commit(&small_core, &run(&small, &caller, ingest_op(ingest_for(4))).await.unwrap());
    let error = run(&small, &caller, SkillTelemetryOp::Peers { graph: GRAPH.into(), window: window() })
        .await
        .err()
        .expect("refuses");
    assert!(error.starts_with(PEER_COHORT_TOO_SMALL.as_str()), "{error}");
}

// spec: MEDULLA-DECISION-ENGINE-R713.7
#[tokio::test]
async fn traces_by_eval_uses_the_edge_index() {
    let h = Harness::with_isolation(ServerState::test_isolation("agent-7")).await;
    let core = seed(&h).await;
    let caller = who();
    let events = vec![
        event("i1", SKILL, "u1", 1, "run-a", false),
        event("i2", SKILL, "u1", 2, "run-b", false),
        event("i3", SKILL, "u1", 3, "run-c", false),
    ];
    commit(&core, &run(&h, &caller, ingest_op(events)).await.unwrap());
    // run-a passed, run-b failed, run-c has no score.
    put(&core, "score-a", json!({"node_type": "Score", "value": 0.9, "range_min": 0.0, "range_max": 1.0}));
    put(&core, "score-b", json!({"node_type": "Score", "value": 0.1, "range_min": 0.0, "range_max": 1.0}));
    for (trace, score) in [("trace-run-a", "score-a"), ("trace-run-b", "score-b")] {
        core.add_edge(
            trace.to_string(),
            score.to_string(),
            rmp_serde::to_vec_named(&json!({"relationship": "scoredBy"})).unwrap(),
        )
        .unwrap();
    }
    let read = |outcome| SkillTelemetryOp::TracesByEval {
        graph: GRAPH.into(),
        skill_ref: SKILL.into(),
        outcome,
        limit: Some(10),
        cursor: None,
    };
    for (outcome, expected) in [
        (EvalOutcome::Pass, vec!["trace-run-a"]),
        (EvalOutcome::Fail, vec!["trace-run-b"]),
        (EvalOutcome::Unscored, vec!["trace-run-c"]),
    ] {
        let answer: TracesAnswer = reply(&run(&h, &caller, read(outcome)).await.unwrap());
        assert_eq!(answer.trace_ids, expected);
    }
    // Pages follow the cursor, newest end first.
    put(&core, "score-c", json!({"node_type": "Score", "passed": true}));
    core.add_edge(
        "trace-run-c".to_string(),
        "score-c".to_string(),
        rmp_serde::to_vec_named(&json!({"relationship": "scoredBy"})).unwrap(),
    )
    .unwrap();
    let page = |cursor: Option<String>| SkillTelemetryOp::TracesByEval {
        graph: GRAPH.into(),
        skill_ref: SKILL.into(),
        outcome: EvalOutcome::Pass,
        limit: Some(1),
        cursor,
    };
    let first: TracesAnswer = reply(&run(&h, &caller, page(None)).await.unwrap());
    assert_eq!(first.trace_ids, ["trace-run-c"]);
    let second: TracesAnswer = reply(&run(&h, &caller, page(first.next_cursor)).await.unwrap());
    assert_eq!(second.trace_ids, ["trace-run-a"]);
    assert_eq!(second.next_cursor, None);
}

// spec: MEDULLA-DECISION-ENGINE-R713.3
#[tokio::test]
async fn a_foreign_tenants_invocation_id_never_collides_or_answers_differently() {
    const FOREIGN: &str = "tenant-foreign";
    const FOREIGN_SKILL: &str = "skill:foreign@1";
    let h = Harness::with_isolation(ServerState::test_isolation("agent-7")).await;
    let core = seed(&h).await;
    put(&core, FOREIGN_SKILL, json!({"type": "Skill", "tenant_id": FOREIGN}));
    let owner = who();
    let foreign = VerifiedRequestContext::verified_for_test_with_scopes(
        "agent-7",
        FOREIGN,
        &["telemetry:write", "telemetry:read"],
    );

    // The owner stores invocation `i1`.
    let mine = run(&h, &owner, ingest_op(vec![event("i1", SKILL, "u1", 1, "run-a", false)]))
        .await
        .expect("ingests");
    assert_eq!(reply::<IngestReceipt>(&mine), IngestReceipt { stored: 1, repeated: 0 });
    commit(&core, &mine);

    // Another tenant ingests the same invocation id. It answers exactly as it would on an empty
    // graph (stored, not repeated), so the id tells it nothing about the owner's rows.
    let theirs = run(&h, &foreign, ingest_op(vec![event("i1", FOREIGN_SKILL, "u9", 1, "run-z", false)]))
        .await
        .expect("ingests");
    assert_eq!(reply::<IngestReceipt>(&theirs), IngestReceipt { stored: 1, repeated: 0 });
    commit(&core, &theirs);

    // The two rows are two nodes, each stamped with its own tenant.
    let (own_node, foreign_node) = (invocation_node_id(TENANT, "i1"), invocation_node_id(FOREIGN, "i1"));
    assert_ne!(own_node, foreign_node);
    let tenant_of = |id: &str| {
        let blob = core.get_node_properties(id).expect("the node exists");
        medulla_types::msgpack::decode_property_value(&blob).unwrap()["tenant_id"].clone()
    };
    assert_eq!(tenant_of(&own_node), json!(TENANT));
    assert_eq!(tenant_of(&foreign_node), json!(FOREIGN));

    // A repeat is still found, but only inside the tenant that stored it.
    let again = run(&h, &owner, ingest_op(vec![event("i1", SKILL, "u1", 1, "run-a", false)]))
        .await
        .expect("repeats");
    assert_eq!(reply::<IngestReceipt>(&again), IngestReceipt { stored: 0, repeated: 1 });

    // Neither tenant's measures count the other's invocation.
    let measured = |caller: &VerifiedRequestContext, skill: &str| {
        let op = SkillTelemetryOp::Measures {
            graph: GRAPH.into(),
            skill_ref: skill.into(),
            window: window(),
        };
        let (h, caller) = (&h, caller.clone());
        async move { reply::<SkillMeasures>(&run(h, &caller, op).await.expect("reads")).invocations }
    };
    assert_eq!(measured(&owner, SKILL).await, 1);
    assert_eq!(measured(&foreign, FOREIGN_SKILL).await, 1);
    assert_eq!(measured(&foreign, SKILL).await, 0);
}

/// A person: the principal is its own effective agent, with no delegation and no service or
/// agent prefix, so the engine treats it as a human principal.
fn person(name: &str, tenant: &str, scopes: &[&str]) -> VerifiedRequestContext {
    VerifiedRequestContext::from_verified_claims(
        medulla_identity_types::acl::RequestContextClaims {
            principal: name.to_string(),
            tenant: tenant.to_string(),
            audience: "medulla".to_string(),
            agent_id: name.to_string(),
            roles: Vec::new(),
            scopes: scopes.iter().map(|scope| scope.to_string()).collect(),
            policy_version: "policy-test".to_string(),
            delegation: Vec::new(),
            node: None,
            priority: None,
        },
        format!("test:{name}"),
    )
}

async fn person_harness() -> (Harness, std::sync::Arc<medulla_graph_core::graph::GraphCore>) {
    let mut isolation = medulla_isolation::isolation::IsolationLayer::new();
    for name in ["alice", "bob"] {
        isolation.register_agent(medulla_identity_types::acl::AgentIdentity {
            agent_id: name.into(),
            role: medulla_identity_types::acl::AgentRole::Agent,
            teams: Vec::new(),
            roles: Vec::new(),
        });
    }
    let h = Harness::with_isolation(isolation).await;
    let core = core(&h).await;
    put(&core, SKILL, json!({"type": "Skill", "tenant_id": TENANT, "_owner": "alice", "_visibility": "public"}));
    put(&core, "trace-alice", json!({"node_type": "RunTrace", "run_id": "run-alice", "tenant": TENANT, "_owner": "alice"}));
    put(&core, "trace-bob", json!({"node_type": "RunTrace", "run_id": "run-bob", "tenant": TENANT, "_owner": "bob", "_visibility": "public"}));
    (h, core)
}

// spec: MEDULLA-IDENTITY-R737
#[tokio::test]
async fn a_person_ingests_spans_of_its_own_sessions_attributed_to_the_verified_principal() {
    let (h, core) = person_harness().await;
    let alice = person("alice", TENANT, &["telemetry:write", "telemetry:read"]);
    // The event claims another principal's pseudonym; the engine overwrites it.
    let claimed = event("p1", SKILL, "someone-else", 1, "run-alice", false);
    let write = run(&h, &alice, ingest_op(vec![claimed])).await.expect("a person ingests its own session");
    assert_eq!(reply::<IngestReceipt>(&write), IngestReceipt { stored: 1, repeated: 0 });
    commit(&core, &write);
    let blob = core.get_node_properties(&invocation_node_id(TENANT, "p1")).expect("stored");
    let row = medulla_types::msgpack::decode_property_value(&blob).unwrap();
    let verified = principal_pseudonym(
        TENANT,
        &alice.principal_persistence_id(),
        medulla_server_api::knob_config::knobs().skill_pseudonym_salt,
    );
    assert_eq!(row["principal_pseudonym"], json!(verified));
    assert_eq!(row["tenant_id"], json!(TENANT));
}

// spec: MEDULLA-IDENTITY-R737
#[tokio::test]
async fn spans_of_another_principals_session_answer_exactly_like_an_absent_session() {
    let (h, _core) = person_harness().await;
    let alice = person("alice", TENANT, &["telemetry:write", "telemetry:read"]);
    // `run-bob` belongs to another principal and is visible to alice; `run-missing` does not exist.
    let foreign = run(&h, &alice, ingest_op(vec![event("p2", SKILL, "u", 1, "run-bob", false)]))
        .await
        .expect_err("another principal's session is refused");
    let absent = run(&h, &alice, ingest_op(vec![event("p2", SKILL, "u", 1, "run-missing", false)]))
        .await
        .expect_err("an absent session is refused");
    assert!(foreign.starts_with(TELEMETRY_SESSION_UNKNOWN.as_str()), "{foreign}");
    assert_eq!(
        foreign.replace("run-bob", "<run>"),
        absent.replace("run-missing", "<run>"),
        "foreign and absent answer the same"
    );
}

// spec: MEDULLA-IDENTITY-R737
#[tokio::test]
async fn a_person_without_the_write_scope_is_refused() {
    let (h, _core) = person_harness().await;
    let reader = person("alice", TENANT, &["telemetry:read"]);
    let refused = run(&h, &reader, ingest_op(vec![event("p3", SKILL, "u", 1, "run-alice", false)]))
        .await
        .expect_err("the write scope is required");
    assert!(refused.starts_with(TELEMETRY_SCOPE_REQUIRED.as_str()), "{refused}");
}
