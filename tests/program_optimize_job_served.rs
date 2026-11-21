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

use program_job_support::{evaluated_journey, foreign_status};

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
            assert_eq!(journey.revision["revision_ref"], journey.revision_ref.as_str());

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
