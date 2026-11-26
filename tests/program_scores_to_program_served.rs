//! MEDULLA-DECISION-ENGINE-R813.3 / R813.5 / R813.6: a served `ProgramOptimize`
//! submit that names a score source builds its corpus from the `:Score` nodes
//! the caller can see, ignores another tenant's scores, reports the scores it
//! left out for want of a run link, and the jobs compile a program from the
//! corpus and publish its evaluated revision.
//!
//! The colocated job executor is one process-wide pool bound to the first state
//! that submits a job, so this binary runs exactly one journey; every test reads
//! its slice of the one observation.

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

use std::sync::{Arc, Mutex, OnceLock};

use program_job_support::{
    evaluation_of, first_string_with_prefix, link_score_run_as, node_properties, put_node_as,
    refused_submit, result_rows, run_program_job, score_source_request, seed_score,
    strings_with_prefix,
};
use serde_json::Value;

/// Linked scores of the caller: train and test.
const TRAIN_SCORES: usize = 39;
const TEST_SCORES: usize = 10;

/// What the one journey observed.
struct Observed {
    /// The refusal text of the submit made while only another tenant's score existed.
    foreign_refusal: String,
    /// The status of the draft job, as text and as rows.
    draft_text: String,
    draft_rows: Vec<Value>,
    draft_candidate: Option<String>,
    /// The `ProgramRevision` node the evaluated job published.
    revision: Value,
}

static OBSERVED: OnceLock<Observed> = OnceLock::new();

fn observed() -> &'static Observed {
    OBSERVED.get_or_init(|| {
        let slot: Arc<Mutex<Option<Observed>>> = Arc::new(Mutex::new(None));
        let out = Arc::clone(&slot);
        let driver = medulla_server::spawn_engine_driver(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_stack_size(medulla_server::ENGINE_WORKER_STACK_BYTES)
                .enable_all()
                .build()
                .expect("build runtime");
            runtime.block_on(async {
                *out.lock().expect("observation slot") = Some(journey().await);
            });
        })
        .expect("start engine driver");
        medulla_server::join_engine_driver(driver).expect("engine driver failed");
        let observed = slot.lock().expect("observation slot").take();
        observed.expect("the journey ran")
    })
}

/// One authored score and the run it evaluates.
async fn seed_authored_score(
    state: &test_support::SharedState,
    id: u64,
    name: &str,
    value: f64,
    split: &str,
) {
    let run = format!("run-{name}");
    put_node_as(state, id, None, &run, serde_json::json!({"type": "ExperimentRun"})).await;
    put_node_as(
        state,
        id + 1,
        None,
        name,
        serde_json::json!({
            "type": "Score", "value": value, "range_min": 1.0, "range_max": 5.0,
            "grader_kind": "human", "author": "annotator-1", "split": split,
        }),
    )
    .await;
    link_score_run_as(state, id + 2, None, name, &run).await;
}

async fn journey() -> Observed {
    let directory = tempfile::tempdir().expect("private persistence");
    let state = program_job_support::redb_state(directory.path());
    let request = score_source_request("scores");

    // Another tenant's score alone leaves the caller no corpus.
    seed_score(&state, 10, Some("tenant-foreign"), "score-foreign", 5.0, "human", "test").await;
    let foreign_refusal = refused_submit(&state, 20, &request).await;

    // Fifty stored scores of the caller: 49 linked to a run and one that is not.
    let mut id = 1_000;
    for index in 0..TRAIN_SCORES {
        let value = 2.0 + (index % 4) as f64;
        seed_authored_score(&state, id, &format!("score-train-{index}"), value, "train").await;
        id += 3;
    }
    for index in 0..TEST_SCORES {
        let value = 2.0 + (index % 4) as f64;
        seed_authored_score(&state, id, &format!("score-test-{index}"), value, "test").await;
        id += 3;
    }
    put_node_as(
        &state,
        id,
        None,
        "score-unlinked",
        serde_json::json!({
            "type": "Score", "value": 3.0, "range_min": 1.0, "range_max": 5.0,
            "grader_kind": "human", "split": "train",
        }),
    )
    .await;

    let draft = run_program_job(&state, 100_000, &request).await;
    let draft_candidate = first_string_with_prefix(&draft, "eg:program_candidate:");
    let candidate = draft_candidate
        .clone()
        .expect("the job compiles a candidate from the caller's scores");
    let mut evaluated = score_source_request("scores-evaluated");
    evaluated.candidate_evaluations = vec![evaluation_of(&candidate, 0.8)];
    let published = run_program_job(&state, 200_000, &evaluated).await;
    let revision_ref = strings_with_prefix(&published, "eg:program_revision:")
        .into_iter()
        .next()
        .expect("the evaluated job published a revision");
    let revision = node_properties(&state, 300_000, &revision_ref)
        .await
        .expect("the revision node is stored");

    Observed {
        foreign_refusal,
        draft_text: draft.to_string(),
        draft_rows: result_rows(&draft),
        draft_candidate,
        revision,
    }
}

/// Another tenant's scores alone leave the caller no corpus, so the submit is
/// refused; once the caller's own scores exist the job compiles a program and
/// its result never names the other tenant's rows.
// spec: MEDULLA-DECISION-ENGINE-R813.3
#[test]
fn scores_build_a_corpus_and_compile() {
    let observed = observed();
    assert!(
        !observed.foreign_refusal.is_empty(),
        "the foreign score must not build a corpus"
    );
    assert!(
        observed.draft_candidate.is_some(),
        "the job compiles a candidate from the caller's scores"
    );
    assert!(
        !observed.draft_text.contains("score-foreign"),
        "another tenant's score reached the job: {}",
        observed.draft_text
    );
}

/// The corpus names the stored scores it was built from, and the job result
/// reports how many stored scores it left out for want of a run link.
// spec: MEDULLA-DECISION-ENGINE-R813.5
#[test]
fn example_names_score_grader_author_and_run() {
    let observed = observed();
    assert!(!observed.draft_rows.is_empty());
    assert!(
        observed
            .draft_rows
            .iter()
            .all(|row| row["skipped_without_run"] == 1),
        "the one unlinked score is reported on every row"
    );
    let demonstrations: Vec<&str> = observed
        .draft_rows
        .iter()
        .filter_map(|row| row["demonstration_refs"].as_array())
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    assert!(!demonstrations.is_empty(), "a candidate cites its examples");
    assert!(
        demonstrations.iter().all(|reference| reference.starts_with("eg:score:")),
        "an example is named by the score it came from: {demonstrations:?}"
    );
}

/// Fifty stored scores build one corpus, one program compiles from it, and the
/// evaluated candidate publishes its revision over the corpus the scores built.
// spec: MEDULLA-DECISION-ENGINE-R813.6
#[test]
fn served_scores_to_program_journey() {
    let revision = &observed().revision;
    assert_eq!(revision["type"], "ProgramRevision");
    assert_eq!(revision["stage"], "published");
    assert_eq!(
        revision["held_out"]["case_count"],
        TEST_SCORES,
        "the held-out cases are the stored test scores"
    );
    assert_eq!(
        revision["corpus_ref"],
        revision["candidate_record"]["corpus_ref"]
    );
}
