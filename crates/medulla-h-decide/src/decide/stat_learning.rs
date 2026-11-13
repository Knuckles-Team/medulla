//! `DecisionLog.learn`, dispatched (EH-394..EH-397): the one retrieval-learning
//! WRITE. Reads of what was learned are the decision views' relations
//! ([`super::stat_retrieval`]).
//!
//! The async half resolves what a write needs from outside the Agent Library
//! before the blocking store work starts: the verified carrier's tenant scope
//! (the partition pointers are served from -- the same key the query paths
//! derive from their own verified carrier) and the ACL-checked graphs a fit or
//! a generation evaluation probes.

use medulla_decision_stats::decision::adapter::AdapterKernel;
use medulla_decision_types::decision::statistical::log::DecisionLogOp;
use medulla_decision_types::decision::statistical::retrieval_adapter::{
    AdapterFitRequest, AdapterMonitorRequest, AdapterScope,
};
use medulla_decision_types::decision::statistical::retrieval_generation::GenerationCycleRequest;
use medulla_decision_types::decision::statistical::retrieval_generation::GenerationEvalRequest;
use medulla_decision_types::decision::statistical::retrieval_generation::IndexAdapterRef;
use medulla_decision_types::{
    decision::statistical::retrieval::LearningRecorded,
    decision::statistical::retrieval::LearningWrite,
    decision::statistical::retrieval::PointerMovement,
    decision::statistical::retrieval::PointerRef,
};

use super::stat_adapter::{
    adapter_pointer, ensure_room, fit_adapter, monitor, qualified, qualified_index,
    scoped_adapter_pointer,
};
use super::stat_distilled::upload_scores;
use super::stat_executor::ExecutionContext;
use super::stat_generation::{
    evaluate_generation, generation_pointer, qualified_generation, Generations,
};
use super::stat_generation_cycle::{
    job_id, run_cycle, ConfiguredEmbedder, CycleRun, NoEndpoint, Reembedder,
};
use super::stat_log::LogReader;
use super::stat_pointer::{activate, rollback, PointerMove, Qualified};
use super::stat_retrieval::record_outcome;
use super::stat_run_credit::record_run;
use super::stat_support::invalid_parameter;
use super::stat_vectors::{graph_vectors, GraphVectors};
use super::SharedState;
use medulla_mutation_types::capacity_lease::{CapacityResourceClass, LeasePriority};
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_protocol_types::protocol::Method;
use medulla_query_types::solve_scalar::digest_text;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::carrier::CarrierAuthority;
use medulla_work_types::native_control::{
    CapacityAcquireRequest, CapacityDemand, NativeControlSchemaVersion,
};

/// What a learning write resolved before its blocking store work.
pub(super) struct LearningInputs {
    /// The verified carrier's tenant scope: where pointers live.
    served_tenant: String,
    /// The graphs the write probes, in the order it names them.
    graphs: Vec<GraphVectors>,
    /// What a generation cycle runs with; `None` for every other write.
    cycle: Option<CycleSupport>,
}

/// The background lease and embedder of one generation-cycle submission.
struct CycleSupport {
    lease_id: String,
    embedder: Box<dyn Reembedder>,
    /// The index-side adapter a cycle materializes, qualified at prepare.
    index: Option<AdapterKernel>,
}

/// Seconds a cycle's capacity lease lives without a renewing submission.
const CYCLE_LEASE_TTL_MS: u64 = 300_000;

/// One dispatch: the log context and the resolved inputs.
struct Serving<'a> {
    ctx: &'a ExecutionContext<'a>,
    reader: &'a LogReader,
    inputs: &'a LearningInputs,
}

impl Serving<'_> {
    fn graph(&self, index: usize) -> Result<&GraphVectors, String> {
        self.inputs
            .graphs
            .get(index)
            .ok_or_else(|| "a learning write resolved too few graphs".to_string())
    }

    fn tenant(&self) -> &str {
        &self.inputs.served_tenant
    }

    fn fit(&self, request: &AdapterFitRequest) -> Result<LearningRecorded, String> {
        let fitted = fit_adapter(
            self.ctx,
            self.reader,
            request,
            self.graph(0)?,
            self.tenant(),
        )?;
        Ok(LearningRecorded::Fitted(Box::new(fitted)))
    }

    fn monitor(&self, request: &AdapterMonitorRequest) -> Result<LearningRecorded, String> {
        let monitored = monitor(
            self.ctx,
            self.reader,
            request,
            self.graph(0)?,
            self.tenant(),
        )?;
        Ok(LearningRecorded::Monitored(Box::new(monitored)))
    }

    fn cycle(&self, request: &GenerationCycleRequest) -> Result<LearningRecorded, String> {
        let support = self
            .inputs
            .cycle
            .as_ref()
            .ok_or("a generation cycle was not prepared")?;
        let gens = Generations {
            active: self.graph(0)?,
            shadow: self.graph(1)?,
        };
        let run = CycleRun {
            ctx: self.ctx,
            reader: self.reader,
            gens: &gens,
            embedder: support.embedder.as_ref(),
            index: support.index.as_ref(),
            tenant: self.tenant(),
            lease_id: Some(&support.lease_id),
            batch_nodes: medulla_server_api::knob_config::knobs().generation_batch_nodes as usize,
        };
        run_cycle(&run, request)
    }

    fn evaluate(&self, request: &GenerationEvalRequest) -> Result<LearningRecorded, String> {
        let gens = Generations {
            active: self.graph(0)?,
            shadow: self.graph(1)?,
        };
        let evaluated = evaluate_generation(self.ctx, self.reader, request, &gens, self.tenant())?;
        Ok(LearningRecorded::Generation(Box::new(evaluated)))
    }

    /// The target a move may activate: the named receipt must have passed
    /// and qualified exactly this target for this pointer.
    fn qualify(
        &self,
        pointer: &PointerRef,
        target: &str,
        receipt: &str,
    ) -> Result<Qualified, String> {
        let store = self.ctx.store;
        match pointer {
            PointerRef::Adapter { graph } => qualified(
                store,
                self.tenant(),
                (graph, &AdapterScope::Graph),
                target,
                receipt,
            ),
            PointerRef::ScopedAdapter { graph, scope } => {
                qualified(store, self.tenant(), (graph, scope), target, receipt)
            }
            PointerRef::Generation { logical } => {
                qualified_generation(store, self.tenant(), (logical, target), receipt)
            }
        }
    }

    fn move_pointer(
        &self,
        pointer: &PointerRef,
        movement: &PointerMovement,
    ) -> Result<LearningRecorded, String> {
        let key = match pointer {
            PointerRef::Adapter { graph } => adapter_pointer(graph),
            PointerRef::ScopedAdapter { graph, scope } => scoped_adapter_pointer(graph, scope),
            PointerRef::Generation { logical } => generation_pointer(logical),
        };
        let at = PointerMove {
            tenant: self.tenant(),
            key,
            principal: &self.reader.principal,
            now_ms: self.ctx.now_ms,
        };
        let state = match movement {
            PointerMovement::Activate {
                target,
                receipt_digest,
            } => {
                let qualified = self.qualify(pointer, target, receipt_digest)?;
                if matches!(
                    pointer,
                    PointerRef::Adapter { .. } | PointerRef::ScopedAdapter { .. }
                ) {
                    ensure_room(self.ctx.store, self.tenant(), &at.key)?;
                }
                activate(self.ctx.store, &at, &qualified)?
            }
            PointerMovement::Rollback => rollback(self.ctx.store, &at)?,
        };
        Ok(LearningRecorded::Pointer(Box::new(state)))
    }
}

fn graphs_of(write: &LearningWrite) -> Vec<&str> {
    match write {
        LearningWrite::FitAdapter { request } => vec![request.graph.as_str()],
        LearningWrite::MonitorAdapter { request } => vec![request.graph.as_str()],
        LearningWrite::EvaluateGeneration { request } => {
            vec![request.active_graph.as_str(), request.shadow_graph.as_str()]
        }
        LearningWrite::RunGenerationCycle { request } => vec![
            request.evaluation.active_graph.as_str(),
            request.evaluation.shadow_graph.as_str(),
        ],
        LearningWrite::RefreshFreshness { request } => vec![request.logical_graph.as_str()],
        LearningWrite::RecordOutcome { .. }
        | LearningWrite::RunOutcome { .. }
        | LearningWrite::UploadDistilledScores { .. }
        | LearningWrite::MovePointer { .. } => Vec::new(),
    }
}

/// Request the background capacity lease a cycle holds, before its first
/// batch. The request is idempotent per job, so a resumed cycle gets the same
/// lease; it expires with its ttl once the cycle stops submitting.
async fn acquire_cycle_lease(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    tenant: &str,
    request: &GenerationCycleRequest,
) -> Result<String, String> {
    let persistence = state.read().await.persistence.clone().ok_or_else(|| {
        "CAPACITY_UNAVAILABLE: a generation cycle needs the durable capacity ledger".to_string()
    })?;
    let eval = &request.evaluation;
    let id = job_id(&eval.logical, &request.model_ref.revision);
    let lease_id = format!("lease:{id}");
    let acquire = CapacityAcquireRequest {
        schema_version: NativeControlSchemaVersion::V1,
        tenant_ref: tenant.to_string(),
        work_item_id: id.clone(),
        owner_digest: digest_text("eg/generation-cycle-owner/v1", &verified.agent_id()),
        idempotency_key: id,
        priority: LeasePriority::BackgroundIngestion,
        demands: vec![CapacityDemand {
            cell_id: request.capacity_cell.clone(),
            // An index-side adapter materializes from stored vectors: CPU
            // work, never an embedding endpoint's slots.
            resource_class: if request.index_adapter.is_some() {
                CapacityResourceClass::Cpu
            } else {
                CapacityResourceClass::LlmEmbedding
            },
            amount: 1,
        }],
        lease_id: Some(lease_id.clone()),
        ttl_ms: CYCLE_LEASE_TTL_MS,
        now_ms: medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
        cost_budget_micros: None,
        token_budget: None,
    };
    persistence
        .commit_capacity_lease(
            &medulla_kernel::persist::sanitize(&eval.logical),
            Method::AcquireCapacity { request: acquire },
        )
        .await
        .map_err(|error| format!("CAPACITY_UNAVAILABLE: {error}"))?;
    Ok(lease_id)
}

/// The kernel of the index-side adapter a cycle names, once the adapter is
/// qualified for the active generation's graph and space.
async fn index_kernel(
    state: &SharedState,
    tenant: &str,
    graphs: &[GraphVectors],
    request: &GenerationCycleRequest,
    reference: &IndexAdapterRef,
) -> Result<AdapterKernel, String> {
    let eval = &request.evaluation;
    let active = graphs.first().ok_or("a cycle resolved no active graph")?;
    if !active.admits_space(&eval.active_space) {
        return Err(invalid_parameter(
            "the active generation's store declares another embedding space",
        ));
    }
    let open = state.read().await.agent_library();
    let store = match open {
        Some(store) => store,
        None => state.write().await.ensure_agent_library()?,
    };
    let (tenant, graph, space, reference) = (
        tenant.to_string(),
        eval.active_graph.clone(),
        eval.active_space.clone(),
        reference.clone(),
    );
    let fitted = tokio::task::spawn_blocking(move || {
        qualified_index(&store, &tenant, &graph, &reference, &space)
    })
    .await
    .map_err(|error| error.to_string())??;
    AdapterKernel::of(&fitted.body)
}

/// Resolve the inputs of a learning write; `None` for every other log op.
pub(super) async fn prepare(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    op: &DecisionLogOp,
) -> Result<Option<LearningInputs>, String> {
    let DecisionLogOp::Learn { write, .. } = op else {
        return Ok(None);
    };
    let served_tenant = CarrierAuthority::from_verified(verified)?
        .tenant_scope()
        .to_string();
    let mut graphs = Vec::new();
    for graph in graphs_of(write) {
        graphs.push(graph_vectors(state, verified.agent_id(), graph).await?);
    }
    let cycle = match write {
        LearningWrite::RunGenerationCycle { request } => {
            let index = match &request.index_adapter {
                Some(reference) => {
                    Some(index_kernel(state, &served_tenant, &graphs, request, reference).await?)
                }
                None => None,
            };
            let embedder: Box<dyn Reembedder> = if index.is_some() {
                Box::new(NoEndpoint)
            } else {
                Box::new(ConfiguredEmbedder(
                    medulla_server_ops::api::ingest::embedder::EmbedderConfig::from_env(),
                ))
            };
            Some(CycleSupport {
                lease_id: acquire_cycle_lease(state, verified, &served_tenant, request).await?,
                embedder,
                index,
            })
        }
        _ => None,
    };
    Ok(Some(LearningInputs {
        served_tenant,
        graphs,
        cycle,
    }))
}

/// Serve one learning write.
pub(super) fn dispatch(
    ctx: &ExecutionContext,
    reader: &LogReader,
    write: LearningWrite,
    inputs: Option<&LearningInputs>,
) -> Result<LearningRecorded, String> {
    let inputs = inputs.ok_or("learning write inputs were not prepared")?;
    let s = Serving {
        ctx,
        reader,
        inputs,
    };
    match write {
        LearningWrite::RecordOutcome { outcome } => Ok(LearningRecorded::Outcome(Box::new(
            record_outcome(ctx, reader, *outcome)?,
        ))),
        LearningWrite::FitAdapter { request } => s.fit(&request),
        LearningWrite::MonitorAdapter { request } => s.monitor(&request),
        LearningWrite::EvaluateGeneration { request } => s.evaluate(&request),
        LearningWrite::RunGenerationCycle { request } => s.cycle(&request),
        LearningWrite::RefreshFreshness { request } => Ok(LearningRecorded::Refreshed(Box::new(
            super::stat_freshness::refresh(ctx, &request)?,
        ))),
        LearningWrite::RunOutcome { outcome } => Ok(LearningRecorded::Run(Box::new(record_run(
            ctx, reader, *outcome,
        )?))),
        LearningWrite::UploadDistilledScores { upload } => Ok(LearningRecorded::DistilledScores(
            Box::new(upload_scores(ctx, *upload)?),
        )),
        LearningWrite::MovePointer { pointer, movement } => s.move_pointer(&pointer, &movement),
    }
}
