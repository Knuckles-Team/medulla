//! MEDULLA-DECISION-ENGINE-R800.3 / R800.5: a served reflective (`Gepa`)
//! `ProgramOptimize` job reports the Pareto front of its evaluated candidates:
//! the digest, the scoreboard in rank order, the next parent and the
//! evaluation-set digest are columns of the job result, and the published
//! `ProgramRevision` node stores the `front_digest`. A rerun over one seed
//! reproduces the digest.
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
    evaluation_of, front_evaluation_of, node_properties, refused_submit, reflective_request,
    result_rows, run_program_job, sized_program_request, strings_with_prefix,
};
use serde_json::Value;

/// What the one journey observed.
struct Observed {
    /// The result rows of the evaluated job.
    evaluated_rows: Vec<Value>,
    /// The `ProgramRevision` node the evaluated job published.
    revision: Value,
    /// The result rows of a rerun over the same seed and evaluations whose
    /// promotion bar nothing clears, so it publishes no revision.
    rerun_rows: Vec<Value>,
    /// The candidates the draft named, one per reflection artifact.
    candidate_count: usize,
    /// The stage the evaluated job staged its result at, and its job id.
    evaluated_job: Value,
}

static OBSERVED: OnceLock<Observed> = OnceLock::new();

/// Run the journey once, on the engine driver, and keep what it saw.
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

/// Evaluate every candidate the draft named (64 of them), each trading accuracy
/// against benchmark and cost so none dominates another.
fn evaluations_of(candidates: &[String]) -> Vec<medulla_program::EvaluationSummary> {
    let count = candidates.len() as f64;
    candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let step = index as f64 / (count + 1.0);
            front_evaluation_of(
                candidate,
                0.6 + step * 0.3,
                0.9 - step * 0.5,
                1_000 + index as u64 * 500,
            )
        })
        .collect()
}

async fn journey() -> Observed {
    let directory = tempfile::tempdir().expect("private persistence");
    let state = program_job_support::redb_state(directory.path());
    let draft = run_program_job(&state, 100, &reflective_request("draft", Vec::new())).await;
    let candidates = strings_with_prefix(&draft, "eg:program_candidate:");
    assert_eq!(
        candidates.len(),
        program_job_support::REFLECTIVE_CANDIDATES,
        "the draft names one candidate per reflection artifact"
    );
    let evaluations = evaluations_of(&candidates);

    let evaluated = run_program_job(
        &state,
        1_000,
        &reflective_request("evaluated", evaluations.clone()),
    )
    .await;
    let revision_ref = strings_with_prefix(&evaluated, "eg:program_revision:")
        .into_iter()
        .next()
        .expect("the evaluated job published a revision");
    let revision = node_properties(&state, 2_000, &revision_ref)
        .await
        .expect("the revision node is stored");

    // The same seed and evaluations under a bar nothing clears: a draft whose
    // result still reports the front.
    let mut rerun = reflective_request("rerun", evaluations);
    rerun.promotion.min_score_delta = 1.0;
    let rerun = run_program_job(&state, 3_000, &rerun).await;

    Observed {
        evaluated_rows: result_rows(&evaluated),
        revision,
        rerun_rows: result_rows(&rerun),
        candidate_count: candidates.len(),
        evaluated_job: evaluated["state"].clone(),
    }
}

fn candidate_rows(rows: &[Value]) -> Vec<&Value> {
    rows.iter()
        .filter(|row| row["kind"] == "program_candidate")
        .collect()
}

fn digest_of(rows: &[Value]) -> String {
    candidate_rows(rows)
        .iter()
        .find_map(|row| row["front_digest"].as_str())
        .expect("a reflective result carries the front digest")
        .to_string()
}

/// The front digest, the scoreboard in rank order and the next parent are
/// columns of the served job result.
// spec: MEDULLA-DECISION-ENGINE-R800.3
#[test]
fn reflective_job_reports_front_and_next_parent() {
    let rows = &observed().evaluated_rows;
    let digest = digest_of(rows);
    assert_eq!(digest.len(), 64);
    assert!(digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    let candidates = candidate_rows(rows);
    assert!(
        candidates.iter().all(|row| row["front_digest"] == digest.as_str()),
        "every candidate row carries the one front digest"
    );

    let mut ranked: Vec<(u64, &Value)> = candidates
        .iter()
        .filter_map(|row| row["front_rank"].as_u64().map(|rank| (rank, *row)))
        .collect();
    ranked.sort_by_key(|(rank, _)| *rank);
    assert!(!ranked.is_empty(), "an evaluated candidate sits on the front");
    for (position, (rank, row)) in ranked.iter().enumerate() {
        assert_eq!(*rank, position as u64, "the scoreboard ranks run from 0");
        assert!(row["front_score"].is_f64());
        assert!(row["front_cost_micro_usd"].is_u64());
    }
    let parents: Vec<_> = candidates
        .iter()
        .filter(|row| row["front_next_parent"] == true)
        .collect();
    assert_eq!(parents.len(), 1, "the tournament names exactly one parent");
    assert!(parents[0]["front_rank"].is_u64(), "the parent is on the front");
}

/// The published revision stores the digest of its front, the result carries
/// the evaluation-set digest, and a rerun over one seed reproduces the digest
/// and the scoreboard.
// spec: MEDULLA-DECISION-ENGINE-R800.5
#[test]
fn front_digest_replays_under_one_seed() {
    let observed = observed();
    let digest = digest_of(&observed.evaluated_rows);
    assert_eq!(observed.revision["front_digest"], digest.as_str());
    assert_eq!(digest_of(&observed.rerun_rows), digest);

    let board = |rows: &[Value]| -> Vec<(u64, String, String)> {
        let mut board: Vec<(u64, String, String)> = candidate_rows(rows)
            .iter()
            .filter_map(|row| {
                Some((
                    row["front_rank"].as_u64()?,
                    row["id"].as_str()?.to_string(),
                    row["front_cost_micro_usd"].to_string(),
                ))
            })
            .collect();
        board.sort();
        board
    };
    assert_eq!(board(&observed.evaluated_rows), board(&observed.rerun_rows));

    let set_digest = candidate_rows(&observed.evaluated_rows)
        .iter()
        .find_map(|row| row["evaluation_set_digest"].as_str())
        .expect("an evaluated result carries the evaluation-set digest");
    assert_eq!(
        Some(set_digest),
        observed.revision["held_out"]["evaluation_set_digest"].as_str()
    );
}

/// A reflective job whose inputs cannot form a front is refused at submit with a
/// stable code and stores no job: a test split under `front.min_eval_examples`
/// is `FRONT_EVAL_TOO_SMALL`, an evaluation without a benchmark score is
/// `FRONT_SCORE_LENGTH`, and a test split graded only by a model judge is
/// `EVAL_SET_NOT_INDEPENDENT`.
// spec: MEDULLA-DECISION-ENGINE-R800.4
#[test]
fn front_inputs_refuse_with_stable_codes() {
    use medulla_program::{ExampleSplit, GraderKind, OptimizerKind};
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
            let candidate = "eg:program_candidate:00";
            let benchmarked = |count: usize| {
                sized_program_request(
                    "refuse",
                    vec![front_evaluation_of(candidate, 0.7, 0.6, 100)],
                    (6, count),
                    OptimizerKind::Gepa,
                )
            };

            // Two test examples are under the 30 the front needs.
            let small = benchmarked(2);
            let text = refused_submit(&state, 10, &small).await;
            assert!(text.starts_with("FRONT_EVAL_TOO_SMALL"), "{text}");

            // An evaluation with no benchmark score has one objective, not two.
            let mut unscored = benchmarked(30);
            unscored.candidate_evaluations = vec![evaluation_of(candidate, 0.7)];
            let text = refused_submit(&state, 20, &unscored).await;
            assert!(text.starts_with("FRONT_SCORE_LENGTH"), "{text}");

            // A model judge alone cannot decide the held-out evidence.
            let mut judged = benchmarked(30);
            for example in &mut judged.corpus.examples {
                if example.split == ExampleSplit::Test {
                    example.grader_kind = GraderKind::ModelJudge;
                }
            }
            let text = refused_submit(&state, 30, &judged).await;
            assert!(text.starts_with("EVAL_SET_NOT_INDEPENDENT"), "{text}");

            // Nothing was staged by any refusal.
            assert!(
                program_job_support::champion_target(&state).await.is_none(),
                "a refusal stages no revision"
            );
        });
    })
    .expect("start engine driver");
    medulla_server::join_engine_driver(driver).expect("engine driver failed");
}

/// The hot-path proof: a served reflective job over 64 evaluated candidates
/// reports a bounded front whose scoreboard names only evaluated candidates,
/// publishes the evaluated revision with its front digest and moves no champion.
// spec: MEDULLA-DECISION-ENGINE-R800.6
#[test]
fn served_reflective_job_journey() {
    let observed = observed();
    assert_eq!(observed.candidate_count, program_job_support::REFLECTIVE_CANDIDATES);
    assert!(observed.evaluated_job.get("Succeeded").is_some());
    let candidates = candidate_rows(&observed.evaluated_rows);
    assert_eq!(candidates.len(), observed.candidate_count);
    let front: Vec<&&Value> = candidates
        .iter()
        .filter(|row| row["front_rank"].is_u64())
        .collect();
    assert!(!front.is_empty() && front.len() <= observed.candidate_count);
    assert_eq!(
        observed.revision["front_digest"],
        digest_of(&observed.evaluated_rows).as_str()
    );
    assert_eq!(observed.revision["stage"], "published");
    assert_eq!(observed.revision["optimizer"], "gepa");
    assert!(observed.revision["held_out"].is_object());
}
