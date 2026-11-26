//! MEDULLA-DECISION-ENGINE-R805.8: a served `ProgramOptimize` job publishes its
//! evaluated revision at stage `published` and writes no champion: the pointer
//! `champion:program:<program_ref>` of the governed pointer store stays empty
//! until a `DecisionLog` `learn` `MovePointer` activates a revision, and the
//! job commit no longer writes a `ProgramActiveRevision` node.
//!
//! The colocated job executor is one process-wide pool bound to the first state
//! that submits a job, so this binary runs exactly one journey and sits beside
//! `program_optimize_job_served.rs` instead of inside it.

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

use program_job_support::{champion_target, evaluated_journey, node_properties};

/// The evaluated job commits its revision and leaves the champion empty.
// spec: MEDULLA-DECISION-ENGINE-R805.8
#[test]
fn job_publishes_a_revision_without_moving_the_champion() {
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
            assert!(champion_target(&state).await.is_none());

            let journey = evaluated_journey(&state).await;
            assert_eq!(journey.revision["type"], "ProgramRevision");
            assert_eq!(journey.revision["stage"], "published");
            assert!(journey.revision["parent_ref"].is_null());

            // No champion moved: the pointer store is still empty, and the
            // job wrote no node of the retired `ProgramActiveRevision` type.
            assert!(champion_target(&state).await.is_none());
            let candidate = journey.revision["candidate_ref"]
                .as_str()
                .expect("the revision names its candidate");
            let candidate_node = node_properties(&state, 4_000, candidate)
                .await
                .expect("the candidate node is stored");
            assert_eq!(candidate_node["type"], "ProgramCandidate");
            assert_ne!(journey.revision["type"], "ProgramActiveRevision");
        });
    })
    .expect("start engine driver");
    medulla_server::join_engine_driver(driver).expect("engine driver failed");
}
