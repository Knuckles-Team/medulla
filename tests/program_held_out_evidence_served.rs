//! MEDULLA-DECISION-ENGINE-R810.7: the evaluated program revision a served job
//! commits carries its held-out evidence as the `held_out` property of its node.

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

use program_job_support::{evaluated_journey, test_example_refs, TEST_EXAMPLES};

/// Every evidence field on the stored node: the candidate scored 0.8 against a
/// 0.5 baseline over the two test-split examples.
// spec: MEDULLA-DECISION-ENGINE-R810.7
#[test]
fn evaluated_revision_carries_held_out_evidence() {
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
            let evidence = &journey.revision["held_out"];
            assert!(evidence.is_object(), "the revision has no held_out: {evidence}");
            assert_eq!(evidence["receipt_kind"], "program_evaluation");
            assert_eq!(evidence["challenger_ref"], journey.revision_ref.as_str());
            assert!(evidence.get("champion_ref").is_none() || evidence["champion_ref"].is_null());
            assert_eq!(evidence["challenger_score_ppm"], 800_000);
            assert_eq!(evidence["champion_score_ppm"], 500_000);
            assert_eq!(evidence["delta_ppm"], 300_000);
            assert_eq!(evidence["case_count"], TEST_EXAMPLES);
            assert_eq!(evidence["independent"], true);
            assert_eq!(evidence["grader_kind"], "annotation");
            assert_eq!(
                evidence["evaluation_set_digest"],
                medulla_query_types::solve_scalar::digest_text(
                    medulla_program::HELD_OUT_SET_DIGEST_DOMAIN,
                    &test_example_refs()
                )
            );
            assert_eq!(evidence["receipt_digest"].as_str().expect("receipt digest").len(), 64);
            assert_eq!(evidence["subject"]["kind"], "program");
            assert_eq!(
                evidence["subject"]["program_ref"],
                program_job_support::program_ref().as_str()
            );
        });
    })
    .expect("start engine driver");
    medulla_server::join_engine_driver(driver).expect("engine driver failed");
}
