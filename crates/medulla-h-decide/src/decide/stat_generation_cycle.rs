//! The generation cycle (MEDULLA-DECISION-ENGINE-R174): publish a model,
//! re-embed the shadow graph in checkpointed batches, evaluate the result with
//! the generation evaluator and move the logical graph's pointer with the
//! passing receipt.
//!
//! A cycle is idempotent per logical graph and model revision: its job record
//! is keyed by that pair and carries the whole cycle -- the state machine of
//! [`GenerationCycle`], the batch checkpoint and, once activated, the receipt.
//! A submit that finds the record resumes from the checkpoint; one that finds
//! a terminal cycle, or a shadow graph that is already the active generation,
//! changes nothing. `GenerationCycleMachine` is the only writer of the state.

use medulla_decision_stats::decision::adapter::AdapterKernel;
use medulla_decision_types::decision::statistical::body::canonical_body_bytes;
use medulla_decision_types::decision::statistical::body::content_digest_of;
use medulla_decision_types::decision::statistical::retrieval::LearningRecorded;
use medulla_decision_types::decision::statistical::retrieval_generation::{
    GenerationCycle, GenerationCycleEvent, GenerationCycleJob, GenerationCycleReceipt,
    GenerationCycleRequest, GenerationCycleState, GENERATION_CYCLE_SCHEMA_VERSION,
};
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_persistence_records::decision_jobs::decode_artifact;
use medulla_persistence_records::decision_jobs::encode_artifact;
use medulla_query_types::solve_scalar::digest_text;

use super::stat_executor::ExecutionContext;
use super::stat_generation::{
    evaluate_generation, generation_pointer, qualified_generation, stored_evaluation, Generations,
};
use super::stat_generation_space::model_space;
use super::stat_log::LogReader;
use super::stat_pointer::{activate, read_pointer, PointerMove};
use super::stat_support::{invalid_parameter as invalid, refusal};

/// Domain of a cycle's job id.
const JOB_ID_DOMAIN: &str = "eg/generation-cycle-job/v1";

/// Embeds node text for the shadow generation.
pub trait Reembedder: Send + Sync {
    /// One vector per text, in order.
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String>;
}

/// The engine's configured embedding endpoint.
pub(super) struct ConfiguredEmbedder(
    pub(super) medulla_server_ops::api::ingest::embedder::EmbedderConfig,
);

impl Reembedder for ConfiguredEmbedder {
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
        self.0.embed(&texts)
    }
}

/// The embedder of a cycle that materializes an index-side adapter: the
/// adapter transforms stored vectors, so no embedding endpoint exists to call
/// and any call is an error.
pub(super) struct NoEndpoint;

impl Reembedder for NoEndpoint {
    fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        Err(invalid(
            "an index-side adapter materializes from stored vectors; no embedding endpoint is called",
        ))
    }
}

/// Everything one submission of a cycle runs against.
pub(super) struct CycleRun<'a> {
    pub(super) ctx: &'a ExecutionContext<'a>,
    pub(super) reader: &'a LogReader,
    pub(super) gens: &'a Generations<'a>,
    pub(super) embedder: &'a dyn Reembedder,
    /// The index-side adapter the batches apply to the active generation's
    /// stored vectors instead of embedding node text.
    pub(super) index: Option<&'a AdapterKernel>,
    pub(super) tenant: &'a str,
    /// The background capacity lease requested before the first batch.
    pub(super) lease_id: Option<&'a str>,
    /// Nodes per batch (`knob generation.batch_nodes`).
    pub(super) batch_nodes: usize,
}

/// The job id of the cycle of `logical` under model `revision`.
pub(super) fn job_id(logical: &str, revision: &str) -> String {
    let digest = digest_text(JOB_ID_DOMAIN, &(logical, revision));
    format!("generation-cycle:{}", digest.trim_start_matches("sha256:"))
}

pub(super) fn job_key(job_id: &str) -> String {
    format!("generation-cycle-job:{job_id}")
}

fn fire(
    job: &mut GenerationCycleJob,
    event: GenerationCycleEvent,
    receipt_digest: Option<String>,
) -> Result<(), String> {
    job.cycle
        .apply(event, receipt_digest)
        .map_err(|refused| refused.to_string())
}

/// One job record and the bytes the compare-and-set expects it to hold.
struct Record {
    job: GenerationCycleJob,
    stored: Option<Vec<u8>>,
}

impl Record {
    fn save(&mut self, run: &CycleRun) -> Result<(), String> {
        let bytes = encode_artifact(&self.job)?;
        run.ctx.store.swap_decision_artifact(
            run.tenant,
            &job_key(&self.job.job_id),
            self.stored.clone(),
            bytes.clone(),
        )?;
        self.stored = Some(bytes);
        Ok(())
    }
}

fn check_request(run: &CycleRun, request: &GenerationCycleRequest) -> Result<(), String> {
    let model = &request.model_ref;
    let eval = &request.evaluation;
    let named = !model.id.is_empty()
        && !model.revision.is_empty()
        && !eval.logical.is_empty()
        && !request.capacity_cell.is_empty();
    if !named || request.max_batches == Some(0) {
        return Err(invalid(
            "a generation cycle names a model, a logical graph, a capacity cell and at least one batch",
        ));
    }
    if model.dimension as usize != run.gens.shadow.dimensions() {
        return Err(refusal(
            StatisticalErrorCode::GenerationDimensionMismatch,
            "the model dimension is not the dimension of the shadow generation",
        ));
    }
    if request.index_adapter.is_some() != run.index.is_some() {
        return Err(invalid(
            "a cycle that names an index-side adapter runs with exactly that adapter",
        ));
    }
    if run.index.is_some() && run.gens.active.dimensions() != run.gens.shadow.dimensions() {
        return Err(refusal(
            StatisticalErrorCode::GenerationDimensionMismatch,
            "an index-side adapter keeps the vector width of the active generation",
        ));
    }
    Ok(())
}

fn open(run: &CycleRun, request: &GenerationCycleRequest) -> Result<Record, String> {
    let eval = &request.evaluation;
    let id = job_id(&eval.logical, &request.model_ref.revision);
    // The per-submission batch budget is not part of a cycle's identity.
    let identity = GenerationCycleRequest {
        max_batches: None,
        ..request.clone()
    };
    let request_digest = content_digest_of(&canonical_body_bytes(&identity)?);
    match run.ctx.store.decision_artifact(run.tenant, &job_key(&id))? {
        Some(bytes) => {
            let job: GenerationCycleJob = decode_artifact(&bytes, "generation cycle")?;
            if job.request_digest != request_digest {
                return Err(refusal(
                    StatisticalErrorCode::IdempotencyConflict,
                    "this logical graph and model revision already name a different cycle",
                ));
            }
            Ok(Record {
                job,
                stored: Some(bytes),
            })
        }
        None => {
            let cycle = GenerationCycle::publish(
                &eval.logical,
                request.model_ref.clone(),
                &eval.shadow_graph,
            )
            .map_err(invalid)?;
            Ok(Record {
                job: GenerationCycleJob {
                    schema_version: GENERATION_CYCLE_SCHEMA_VERSION,
                    job_id: id,
                    request_digest,
                    cycle,
                    lease_id: None,
                    next_node: 0,
                    reembedded_nodes: 0,
                    batches: 0,
                    receipt: None,
                    rejection: None,
                },
                stored: None,
            })
        }
    }
}

fn pointer_conflict() -> String {
    refusal(
        StatisticalErrorCode::GenerationPointerConflict,
        "the logical graph's pointer moved while the cycle activated; start a new cycle",
    )
}

/// Re-embed the next batch of shadow nodes and checkpoint it.
fn reembed_batch(
    run: &CycleRun,
    request: &GenerationCycleRequest,
    record: &mut Record,
    ids: &[String],
) -> Result<(), String> {
    let start = record.job.next_node as usize;
    let end = (start + run.batch_nodes).min(ids.len());
    let batch = &ids[start..end];
    if let Some(kernel) = run.index {
        return materialize_batch(run, kernel, record, (start, end), ids);
    }
    let texts: Vec<String> = batch
        .iter()
        .map(|id| run.gens.shadow.node_text(id))
        .collect();
    let vectors = run.embedder.embed(&texts)?;
    if vectors.len() != batch.len()
        || vectors
            .iter()
            .any(|vector| vector.len() != request.model_ref.dimension as usize)
    {
        return Err(invalid(
            "the embedder returned a vector count or width other than the model's",
        ));
    }
    for (id, vector) in batch.iter().zip(vectors) {
        run.gens.shadow.store_embedding(id, vector)?;
    }
    record.job.next_node = end as u64;
    record.job.reembedded_nodes += batch.len() as u64;
    record.job.batches += 1;
    record.save(run)
}

/// Materialize the next batch of shadow nodes from the active generation's
/// stored vectors: each takes the index-side adapter applied to the vector the
/// active generation holds for that node. A node the active generation does
/// not embed (or the caller cannot see) is left unembedded; no embedding
/// endpoint is called.
fn materialize_batch(
    run: &CycleRun,
    kernel: &AdapterKernel,
    record: &mut Record,
    (start, end): (usize, usize),
    ids: &[String],
) -> Result<(), String> {
    let mut stored = 0_u64;
    for id in &ids[start..end] {
        let Some(vector) = run.gens.active.stored_vector(id) else {
            continue;
        };
        let adapted = kernel.apply(&vector).ok_or_else(|| {
            invalid("a stored vector does not have the width of the index-side adapter")
        })?;
        run.gens.shadow.store_embedding(id, adapted)?;
        stored += 1;
    }
    record.job.next_node = end as u64;
    record.job.reembedded_nodes += stored;
    record.job.batches += 1;
    record.save(run)
}

/// Run (or resume) one generation cycle.
pub(super) fn run_cycle(
    run: &CycleRun,
    request: &GenerationCycleRequest,
) -> Result<LearningRecorded, String> {
    check_request(run, request)?;
    let eval = &request.evaluation;
    let pointer_key = generation_pointer(&eval.logical);
    let (before_bytes, before) = read_pointer(run.ctx.store, run.tenant, &pointer_key)?;
    let mut record = open(run, request)?;
    if record.stored.is_none() && before.target() == Some(eval.shadow_graph.as_str()) {
        // The model is already the active generation: nothing to do.
        return Ok(LearningRecorded::Pointer(Box::new(before)));
    }
    let done = |record: Record| Ok(LearningRecorded::Cycle(Box::new(record.job)));
    if record.job.cycle.state().is_terminal() {
        return done(record);
    }
    if let Some(lease) = run.lease_id {
        record.job.lease_id = Some(lease.to_string());
    }
    if record.stored.is_none() {
        record.save(run)?;
    }
    if record.job.cycle.state() == GenerationCycleState::Published {
        // The shadow generation is a new embedding space: a shadow graph that
        // declares none gets the model's, so an activated generation always
        // has a space a reader can pin its query vectors to (R172.12.6).
        run.gens
            .shadow
            .ensure_space(&model_space(&request.model_ref)?)?;
        fire(&mut record.job, GenerationCycleEvent::ReembedBegun, None)?;
        record.save(run)?;
    }
    if record.job.cycle.state() == GenerationCycleState::Reembedding {
        let ids = run.gens.shadow.node_ids();
        let mut ran = 0_u32;
        while (record.job.next_node as usize) < ids.len() {
            if request.max_batches.is_some_and(|max| ran >= max) {
                return done(record);
            }
            reembed_batch(run, request, &mut record, &ids)?;
            ran += 1;
        }
        let evaluated = evaluate_generation(run.ctx, run.reader, eval, run.gens, run.tenant)?;
        let event = if evaluated.receipt.passed {
            GenerationCycleEvent::EvaluationPassed
        } else {
            GenerationCycleEvent::EvaluationFailed
        };
        if !evaluated.receipt.passed {
            record.job.rejection = Some(StatisticalErrorCode::GenerationReceiptFailed.to_string());
        }
        fire(&mut record.job, event, Some(evaluated.receipt_digest))?;
        record.save(run)?;
    }
    if record.job.cycle.state() == GenerationCycleState::Evaluated {
        let digest = record
            .job
            .cycle
            .receipt_digest
            .clone()
            .ok_or("an evaluated cycle holds its receipt digest")?;
        // The pointer must still be what this cycle started from: a move in
        // between is a conflict, never an overwrite.
        let (now_bytes, now) = read_pointer(run.ctx.store, run.tenant, &pointer_key)?;
        if now_bytes != before_bytes && now.target() != Some(eval.shadow_graph.as_str()) {
            return Err(pointer_conflict());
        }
        let qualified = qualified_generation(
            run.ctx.store,
            run.tenant,
            (&eval.logical, &eval.shadow_graph),
            &digest,
        )?;
        let at = PointerMove {
            tenant: run.tenant,
            key: pointer_key,
            principal: &run.reader.principal,
            now_ms: run.ctx.now_ms,
        };
        let after = activate(run.ctx.store, &at, &qualified).map_err(|error| {
            if error.starts_with(StatisticalErrorCode::IdempotencyConflict.as_str()) {
                pointer_conflict()
            } else {
                error
            }
        })?;
        record.job.receipt = Some(GenerationCycleReceipt {
            schema_version: GENERATION_CYCLE_SCHEMA_VERSION,
            model_ref: request.model_ref.clone(),
            reembedded_nodes: record.job.reembedded_nodes,
            evaluation: stored_evaluation(run.ctx.store, run.tenant, &digest)?,
            pointer_before: before.target().map(str::to_string),
            pointer_after: after.target().map(str::to_string),
        });
        fire(&mut record.job, GenerationCycleEvent::PointerMoved, None)?;
        record.save(run)?;
    }
    done(record)
}
