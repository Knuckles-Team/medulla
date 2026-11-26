//! MEDULLA-DECISION-ENGINE-R810.6: the served program job journey. One program
//! is compiled from a 200-example corpus with
//! `bootstrap_few_shot_with_random_search` by a served `ProgramOptimize` job, its
//! evaluated revision is promoted through the R805 family (`Decide` kind
//! `Promotion`, a committed record, a `MovePointer`), and the champion is read
//! back through the served `DecisionLog` `pointer` read.
//!
//! The colocated job executor is one process-wide pool bound to the first state
//! that submits a job, so this binary runs exactly one journey and sits beside
//! `program_optimize_job_served.rs` instead of inside it.

#![cfg(all(
    feature = "program-optimization",
    feature = "jobs",
    feature = "redb",
    feature = "security",
    feature = "decide"
))]

mod common;
#[path = "common/program_job_support.rs"]
mod program_job_support;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_agent_types::agent_component::DecisionPolicyRef;
use medulla_decision_outcome_types::decision::promotion::ChampionPointer;
use medulla_decision_types::decision::held_out::RevisionSubject;
use medulla_decision_types::decision::statistical::log::{DecisionLogCommitted, DecisionLogOp};
use medulla_decision_types::decision::statistical::retrieval::{
    LearningRecorded, LearningWrite, PointerMovement, PointerRef,
};
use medulla_decision_types::decision::statistical::{
    DecisionBatch, QuestionKind, TypedParam, TypedValue,
};
use medulla_h_decide::decide::stat_test_support::{
    declared_request, publish_route_schema_in, Harness,
};
use medulla_program::OptimizerKind;
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use program_job_support::{
    champion_target, evaluation_of, first_string_with_prefix, node_properties, program_ref,
    run_program_job, sized_program_request, GRAPH, SECRET,
};

/// Train and test examples of the 200-example corpus; the test split holds more
/// cases than `promotion.min_cases` (50).
const CORPUS: (usize, usize) = (140, 60);

async fn call(state: &test_support::SharedState, id: u64, method: Method) -> Response {
    test_support::call_ok(state, SECRET, id, GRAPH, method).await
}

fn raw<T: serde::de::DeserializeOwned>(response: Response) -> T {
    match response.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).expect("typed result"),
        other => panic!("unexpected payload {other:?}"),
    }
}

fn text(name: &str, value: &str) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value: TypedValue::Text(value.to_string()),
    }
}

/// Compile, publish, promote and read the champion back.
// spec: MEDULLA-DECISION-ENGINE-R810.6
#[test]
fn served_program_job_journey() {
    let driver = medulla_server::spawn_engine_driver(|| {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_stack_size(medulla_server::ENGINE_WORKER_STACK_BYTES)
            .enable_all()
            .build()
            .expect("build runtime");
        runtime.block_on(async {
            let directory = tempfile::tempdir().expect("private persistence");
            let state = program_job_support::redb_state(directory.path());
            let optimizer = OptimizerKind::BootstrapFewShotWithRandomSearch;

            // The draft names the candidate the server derived; the evaluated
            // job carries that candidate's evaluation and publishes the revision.
            let draft = run_program_job(
                &state,
                100,
                &sized_program_request("journey-draft", Vec::new(), CORPUS, optimizer),
            )
            .await;
            let candidate = first_string_with_prefix(&draft, "eg:program_candidate:")
                .expect("the draft result names its candidate");
            let evaluated = sized_program_request(
                "journey-evaluated",
                vec![evaluation_of(&candidate, 0.8)],
                CORPUS,
                optimizer,
            );
            let status = run_program_job(&state, 1_000, &evaluated).await;
            let revision_ref = first_string_with_prefix(&status, "eg:program_revision:")
                .expect("the evaluated job names the revision it published");
            let revision = node_properties(&state, 2_000, &revision_ref)
                .await
                .expect("the revision node is stored");
            assert_eq!(revision["optimizer"], "bootstrap_few_shot_with_random_search");
            assert_eq!(revision["stage"], "published");
            assert_eq!(revision["held_out"]["case_count"], CORPUS.1);
            assert!(
                champion_target(&state).await.is_none(),
                "publishing moves no champion"
            );

            // Promote it: a `Promotion` decision over the stored held-out
            // evidence, committed, then a `MovePointer` the record qualifies.
            let harness = Harness {
                _dir: tempfile::tempdir().expect("scratch"),
                state: state.clone(),
                store: medulla_persistence_records::agent_library::AgentLibraryStateExt::ensure_agent_library(
                    &mut *state.write().await,
                )
                .expect("agent library"),
                nonce: std::cell::Cell::new(1),
            };
            let tenant = common::TEST_TENANT;
            let schema = publish_route_schema_in(&harness, tenant);
            let subject = RevisionSubject::Program {
                program_ref: program_ref().as_str().to_string(),
            };
            let asked = declared_request(
                &schema,
                tenant,
                QuestionKind::Promotion,
                &revision_ref,
                DecisionPolicyRef::Default,
                vec![
                    text("graph", GRAPH),
                    text("mode", "promote"),
                    text("receipt_digest", &revision_ref),
                    text("subject", &serde_json::to_string(&subject).unwrap()),
                ],
            );
            let batch: DecisionBatch = raw(call(
                &state,
                3_000,
                Method::Decide {
                    request: Box::new(asked),
                },
            )
            .await);
            let record = batch.records.as_slice()[0].clone();
            assert!(
                record
                    .premises
                    .iter()
                    .any(|premise| premise.fact == "promotion.verdict=promote"),
                "the held-out evidence promotes the revision"
            );
            let committed: DecisionLogCommitted = raw(call(
                &state,
                3_001,
                Method::DecisionLog {
                    op: Box::new(DecisionLogOp::Commit {
                        record: Box::new(record),
                        evaluator: None,
                        approval: None,
                    }),
                },
            )
            .await);
            let _: LearningRecorded = raw(call(
                &state,
                3_002,
                Method::DecisionLog {
                    op: Box::new(DecisionLogOp::Learn {
                        tenant_id: tenant.to_string(),
                        write: LearningWrite::MovePointer {
                            pointer: PointerRef::Champion {
                                subject: subject.clone(),
                            },
                            movement: PointerMovement::Activate {
                                target: revision_ref.clone(),
                                receipt_digest: committed.record_id.clone(),
                            },
                        },
                    }),
                },
            )
            .await);

            // The champion reads back through the served pointer read.
            let champion: ChampionPointer = raw(call(
                &state,
                3_003,
                Method::DecisionLog {
                    op: Box::new(DecisionLogOp::Pointer {
                        tenant_id: tenant.to_string(),
                        pointer_key: subject.pointer_key(),
                    }),
                },
            )
            .await);
            assert_eq!(champion.subject, subject);
            assert_eq!(champion.revision_ref.as_str(), revision_ref);
            assert!(champion.parent_ref.is_none());
            assert_eq!(
                champion_target(&state).await.as_deref(),
                Some(revision_ref.as_str())
            );
        });
    })
    .expect("start engine driver");
    medulla_server::join_engine_driver(driver).expect("engine driver failed");
}
