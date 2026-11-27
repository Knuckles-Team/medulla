//! MEDULLA-DECISION-ENGINE-R810.3: a served `ProgramOptimize` job is submitted,
//! claimed and run by the colocated executor over a Redb persistence backend, and
//! publishes its evaluated revision through the governed promotion commit.

#![cfg(all(
    feature = "program-optimization",
    feature = "jobs",
    feature = "redb",
    feature = "security"
))]

mod common;
#[path = "common/program_job_support.rs"]
mod program_job_support;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_decision_types::decision::held_out::GraderKind;
use medulla_program::ExampleSplit;
use program_job_support::{
    evaluated_journey, foreign_status, program_request, refused_submit, refused_submit_bytes,
};

/// The journey reads the evaluated `ProgramRevision` back from the graph, and
/// another tenant's read of the job is refused.
// spec: MEDULLA-DECISION-ENGINE-R810.3
#[test]
fn program_optimize_job_end_to_end() {
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
            let journey = evaluated_journey(&state).await;
            assert_eq!(journey.revision["type"], "ProgramRevision");
            assert_eq!(
                journey.revision["revision_ref"],
                journey.revision_ref.as_str()
            );

            let foreign = foreign_status(&state, 3_000, &journey.job_id).await;
            assert!(
                foreign.error.is_some(),
                "another tenant must not read the job: {foreign:?}"
            );
        });
    })
    .expect("start engine driver");
    medulla_server::join_engine_driver(driver).expect("engine driver failed");
}

/// Runs `body` on the engine driver over a private Redb state.
fn with_state<F, Fut>(body: F)
where
    F: FnOnce(test_support::SharedState) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()>,
{
    let driver = medulla_server::spawn_engine_driver(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_stack_size(medulla_server::ENGINE_WORKER_STACK_BYTES)
            .enable_all()
            .build()
            .expect("build runtime");
        runtime.block_on(async {
            let directory = tempfile::tempdir().expect("private persistence");
            let state = program_job_support::redb_state(directory.path());
            body(state).await;
        });
    })
    .expect("start engine driver");
    medulla_server::join_engine_driver(driver).expect("engine driver failed");
}

/// Each invalid request is refused at submit with its own code, before a job is
/// stored: an optimizer outside the closed list, a budget above a ceiling and a
/// corpus with no test split.
// spec: MEDULLA-DECISION-ENGINE-R810.4
#[test]
fn program_job_refusals() {
    with_state(|state| async move {
        let mut unknown =
            serde_json::to_value(program_request("unknown", Vec::new())).expect("request as json");
        unknown["optimizer"] = "gradient_descent".into();
        let text = refused_submit_bytes(
            &state,
            10,
            rmp_serde::to_vec_named(&unknown).expect("encode request"),
        )
        .await;
        assert!(text.starts_with("PROGRAM_OPTIMIZER_UNKNOWN"), "{text}");

        let mut over_budget = program_request("budget", Vec::new());
        over_budget.budget.max_candidates = medulla_program::MAX_CANDIDATES + 1;
        let text = refused_submit(&state, 20, &over_budget).await;
        assert!(text.starts_with("PROGRAM_BUDGET_EXCEEDED"), "{text}");

        let mut no_test = program_request("no-test", Vec::new());
        no_test
            .corpus
            .examples
            .retain(|example| example.split != ExampleSplit::Test);
        let text = refused_submit(&state, 30, &no_test).await;
        assert!(text.starts_with("PROGRAM_HELDOUT_MISSING"), "{text}");
    });
}

/// A corpus whose whole test split is graded by a model judge cannot decide a
/// promotion: the submit is refused with `EVAL_SET_NOT_INDEPENDENT`, and the
/// empty test split of the same corpus is `PROGRAM_HELDOUT_MISSING`. No job and
/// no evaluated revision is staged by either.
// spec: MEDULLA-DECISION-ENGINE-R804.4
#[test]
fn judge_only_test_split_refuses() {
    with_state(|state| async move {
        let mut judged = program_request("judge-only", Vec::new());
        for example in &mut judged.corpus.examples {
            if example.split == ExampleSplit::Test {
                example.grader_kind = GraderKind::ModelJudge;
            }
        }
        let text = refused_submit(&state, 10, &judged).await;
        assert!(text.starts_with("EVAL_SET_NOT_INDEPENDENT"), "{text}");

        judged
            .corpus
            .examples
            .retain(|example| example.split != ExampleSplit::Test);
        let text = refused_submit(&state, 20, &judged).await;
        assert!(text.starts_with("PROGRAM_HELDOUT_MISSING"), "{text}");

        // The refusal staged nothing: no champion exists for the program.
        assert!(program_job_support::champion_target(&state).await.is_none());
    });
}
