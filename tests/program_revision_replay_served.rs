//! MEDULLA-DECISION-ENGINE-R810.5: the `ProgramRevision` node a served job
//! commits records the optimizer, seed and corpus binding of its candidate, its
//! stage (`published`), its parent, its dev score and its held-out evidence, and a rerun over
//! one seed derives one content digest.
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

use program_job_support::{
    evaluated_journey, first_string_with_prefix, program_request, run_program_job,
};

/// Two drafts of one corpus under one seed name one candidate, and the revision
/// committed from it carries every field of the record.
// spec: MEDULLA-DECISION-ENGINE-R810.5
#[test]
fn rerun_with_one_seed_gives_one_revision_digest() {
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
            let first = run_program_job(&state, 10, &program_request("rerun-a", Vec::new())).await;
            let second = run_program_job(&state, 50, &program_request("rerun-b", Vec::new())).await;
            let candidate = first_string_with_prefix(&first, "eg:program_candidate:")
                .expect("the first draft names its candidate");
            assert_eq!(
                first_string_with_prefix(&second, "eg:program_candidate:"),
                Some(candidate.clone()),
                "one seed over one corpus names one candidate"
            );

            let journey = evaluated_journey(&state).await;
            let revision = &journey.revision;
            let digest = revision["content_digest"].as_str().expect("content digest");
            assert!(candidate.ends_with(digest), "{candidate} vs {digest}");
            assert_eq!(revision["optimizer"], "labeled_few_shot");
            assert_eq!(revision["seed"], 42);
            assert_eq!(
                revision["corpus_ref"],
                revision["candidate_record"]["corpus_ref"]
            );
            assert_eq!(revision["corpus_snapshot_version"], 7);
            assert_eq!(revision["stage"], "published");
            assert!(revision["parent_ref"].is_null());
            // The fixture corpus holds no validation example, so no dev score.
            assert!(revision["dev_score"].is_null());
            assert!(revision["held_out"].is_object());
        });
    })
    .expect("start engine driver");
    medulla_server::join_engine_driver(driver).expect("engine driver failed");
}
