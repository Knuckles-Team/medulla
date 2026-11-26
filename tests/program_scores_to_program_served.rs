//! MEDULLA-DECISION-ENGINE-R813.3: a served `ProgramOptimize` submit that names
//! a score source builds its corpus from the `:Score` nodes the caller can see,
//! ignores another tenant's scores, and the job compiles a program from it.
//!
//! The colocated job executor is one process-wide pool bound to the first state
//! that submits a job, so this binary runs exactly one journey.

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

use program_job_support::{
    first_string_with_prefix, refused_submit, run_program_job, score_source_request, seed_score,
};

/// Another tenant's scores alone leave the caller no corpus, so the submit is
/// refused; once the caller's own scores exist the job compiles a program and
/// its result never names the other tenant's rows.
// spec: MEDULLA-DECISION-ENGINE-R813.3
#[test]
fn scores_build_a_corpus_and_compile() {
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
            let request = score_source_request("scores");

            seed_score(&state, 10, Some("tenant-foreign"), "score-foreign", 5.0, "human", "test")
                .await;
            let text = refused_submit(&state, 20, &request).await;
            assert!(!text.is_empty(), "the foreign score must not build a corpus");

            seed_score(&state, 30, None, "score-own-test", 4.0, "human", "test").await;
            seed_score(&state, 40, None, "score-own-train", 3.0, "human", "train").await;
            let status = run_program_job(&state, 100, &request).await;
            assert!(
                first_string_with_prefix(&status, "eg:program_candidate:").is_some(),
                "the job compiles a candidate from the caller's scores: {status}"
            );
            assert!(
                !status.to_string().contains("score-foreign"),
                "another tenant's score reached the job: {status}"
            );
        });
    })
    .expect("start engine driver");
    medulla_server::join_engine_driver(driver).expect("engine driver failed");
}
