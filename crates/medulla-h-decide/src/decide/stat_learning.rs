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
use medulla_decision_types::decision::statistical::retrieval_calibration::CalibrationFitRequest;
use medulla_decision_types::decision::statistical::retrieval_generation::GenerationCycleRequest;
use medulla_decision_types::decision::statistical::retrieval_generation::GenerationEvalRequest;
use medulla_decision_types::decision::statistical::retrieval_generation::IndexAdapterRef;
use medulla_decision_types::{
    decision::statistical::retrieval::LearningRecorded,
    decision::statistical::retrieval::LearningWrite,
    decision::statistical::retrieval::PointerMovement,
    decision::statistical::retrieval::PointerRef,
};

use super::drift_registry::registry;
use super::stat_adapter::{
    adapter_pointer, ensure_room, fit_adapter, monitor, qualified, qualified_index,
    scoped_adapter_pointer,
};
use super::stat_calibration::{calibration_pointer, fit_calibration, qualified_calibration};
use super::stat_distilled::upload_scores;
use super::stat_executor::ExecutionContext;
use super::stat_generation::{
    evaluate_generation, generation_pointer, qualified_generation, Generations,
};
use super::stat_generation_cycle::{
    job_id, run_cycle, ConfiguredEmbedder, CycleRun, NoEndpoint, Reembedder,
};
use super::stat_log::{store_once, LogReader};
use super::stat_pointer::{activate, read_pointer, rollback, PointerMove, Qualified};
use super::stat_promotion::{qualified_champion, record_rollouts};
use super::stat_retrieval::record_outcome;
use super::stat_run_credit::record_run;
use super::stat_support::{invalid_parameter, refusal};
use super::stat_vectors::{graph_vectors, GraphVectors};
use super::SharedState;
use medulla_decision_stats::decision::drift::DriftThresholds;
use medulla_decision_types::decision::statistical::retrieval_drift::{
    DRIFT_REFRESH_RECEIPT_NODE_TYPE,
    drift_refresh_receipt_node_id, shard_generation_node_id, DriftEvaluated, DriftKind, DriftPolicyNode, DriftRefreshed,
    DriftSignal, FiringShard, RefreshPlan, RefreshPriority, ScheduledRefresh,
    ShardGenerationNode, ShardSwap, RETRIEVAL_DRIFT_SCHEMA_VERSION, SHARD_GENERATION_NODE_TYPE,
};
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_persistence_records::decision_jobs::{decode_artifact, encode_artifact};
use medulla_decision_types::decision::DecisionPolicy;
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
    /// The background analytics lease of a drift evaluation or refresh; `None` for every
    /// other write.
    drift_lease: Option<String>,
    /// The embedder a drift refresh re-embeds with; `None` for every other write.
    drift_embedder: Option<Box<dyn Reembedder>>,
    /// The materialized tenant graph a freshness refresh writes its rows into; `None` for
    /// every other write and for a logical graph that is virtual only.
    refresh_graph: Option<GraphVectors>,
    /// The graph nodes a refresh staged; the served `DecisionLog` commits them through the
    /// durable mutation path once the blocking half returned.
    pub(super) refresh_link: std::sync::Arc<std::sync::Mutex<Option<super::run_record::StagedLink>>>,
    /// What a drift refresh applies in memory (the shard swap and the monitors' new reference)
    /// once its generation pointers committed; dropped when the commit fails.
    pub(super) refresh_apply: std::sync::Arc<std::sync::Mutex<Option<RefreshApply>>>,
}

/// The in-memory half of a drift refresh, run after its durable commit.
pub(super) type RefreshApply = Box<dyn FnOnce() + Send>;

/// One re-embedded node as the drift monitors observe it.
struct RefreshObservation {
    shard: u32,
    node_id: String,
    node_type: String,
    vector: Vec<f32>,
    tokens: Vec<String>,
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

    fn calibrate(&self, request: &CalibrationFitRequest) -> Result<LearningRecorded, String> {
        let fitted = fit_calibration(self.ctx, self.reader, request, self.tenant())?;
        Ok(LearningRecorded::Calibration(Box::new(fitted)))
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

    /// The drift policy of `logical`, or `DRIFT_POLICY_MISSING`.
    fn drift_policy(&self, logical: &str) -> Result<DriftPolicyNode, String> {
        let key = format!("drift-policy:{logical}");
        match self.ctx.store.decision_artifact(self.ctx.tenant_id, &key)? {
            Some(bytes) => decode_artifact(&bytes, "drift policy"),
            None => Err(refusal(
                StatisticalErrorCode::DriftPolicyMissing,
                format!("the logical graph {logical} has no registered drift policy"),
            )),
        }
    }

    /// Register or replace the drift policy of `logical` (MEDULLA-DECISION-ENGINE-R174.8.4).
    fn register_drift_policy(
        &self,
        logical: &str,
        region_budget: u32,
    ) -> Result<LearningRecorded, String> {
        let node = DriftPolicyNode::new(logical.to_string(), region_budget)
            .map_err(invalid_parameter)?;
        let key = format!("drift-policy:{logical}");
        let bytes = encode_artifact(&node)?;
        let tenant = self.ctx.tenant_id;
        if self.ctx.store.decision_artifact(tenant, &key)?.is_some() {
            self.ctx.store.replace_decision_artifact(tenant, &key, bytes)?;
        } else {
            self.ctx.store.put_decision_artifacts(tenant, &[(key, bytes)])?;
        }
        Ok(LearningRecorded::DriftPolicy(Box::new(node)))
    }

    /// Evaluate the logical graph's drift monitors on the lease the write acquired and
    /// schedule one refresh plan for the shards that fire (MEDULLA-DECISION-ENGINE-R174.8.3,
    /// R174.8.4, R174.8.5).
    fn evaluate_drift(
        &self,
        logical: &str,
        priority: RefreshPriority,
    ) -> Result<LearningRecorded, String> {
        self.inputs
            .drift_lease
            .as_ref()
            .ok_or("a drift evaluation holds no analytics lease")?;
        let region_budget = self.drift_policy(logical)?.region_budget as usize;
        let policy = DecisionPolicy::engine_default();
        let thresholds = DriftThresholds::from_policy(&policy);
        let (monitored_shards, firing) = registry().evaluate(self.tenant(), logical, &thresholds);
        let mut regions = Vec::new();
        let mut shards = Vec::new();
        let mut signals = Vec::new();
        let mut shard_generations = Vec::new();
        for reading in &firing {
            regions.extend(reading.regions.iter().cloned());
            shards.push(FiringShard {
                shard_id: reading.shard,
                kinds: reading.kinds.iter().map(|(kind, _)| *kind).collect(),
            });
            shard_generations.push((reading.shard, reading.generation));
            for (kind, value) in &reading.kinds {
                let threshold = match kind {
                    DriftKind::DistributionShift => thresholds.shift,
                    DriftKind::VocabularyGrowth => thresholds.vocabulary_points as f64,
                    DriftKind::QualityDecay => thresholds.quality_decay_pct as f64,
                    DriftKind::Staleness => thresholds.max_version_lag as f64,
                };
                signals.push(
                    DriftSignal {
                        schema_version: RETRIEVAL_DRIFT_SCHEMA_VERSION,
                        scope: format!("{}/{logical}/{}", self.tenant(), reading.shard),
                        kind: *kind,
                        value_q16: q16_round(*value),
                        threshold_q16: q16_round(threshold),
                        window: reading.window,
                        reference_generation: format!("{logical}/{}@{}", reading.shard, reading.generation),
                        observed_at: self.ctx.now_ms,
                    }
                    .checked()
                    .map_err(invalid_parameter)?,
                );
            }
        }
        if regions.len() > region_budget {
            return Err(refusal(
                StatisticalErrorCode::RefreshRegionBudget,
                format!(
                    "the plan names {} regions, the drift policy of {logical} allows {region_budget}",
                    regions.len()
                ),
            ));
        }
        let scheduled = match shards.first().and_then(|s| s.kinds.first()).copied() {
            None => None,
            Some(reason) => {
                let to = regions.iter().map(|r| r.revision_to).max().unwrap_or(0);
                let plan = RefreshPlan::new(
                    logical.to_string(),
                    regions,
                    reason,
                    priority,
                    format!("refresh-{logical}@{to}"),
                )
                .map_err(invalid_parameter)?;
                let plan_digest = digest_text("eg/drift-refresh-plan/v1", &plan);
                let record = ScheduledRefresh {
                    plan_digest: plan_digest.clone(),
                    plan,
                    scheduled_at_ms: self.ctx.now_ms,
                    signals,
                    shard_generations,
                };
                Some(store_once(
                    self.ctx,
                    format!("drift-plan:{plan_digest}"),
                    "drift refresh plan",
                    record,
                    |existing, fresh| existing.plan == fresh.plan,
                )?)
            }
        };
        Ok(LearningRecorded::Drift(Box::new(DriftEvaluated {
            logical_graph: logical.to_string(),
            monitored_shards,
            firing: shards,
            scheduled,
        })))
    }

    /// The durable generation pointers of the plan's shards, read from the graph.
    fn durable_generations(
        &self,
        graph: &GraphVectors,
        logical: &str,
        scheduled: &ScheduledRefresh,
    ) -> std::collections::BTreeMap<u32, u64> {
        scheduled
            .shard_generations
            .iter()
            .filter_map(|(shard, _)| {
                let blob = graph
                    .core()
                    .get_node_properties(&shard_generation_node_id(logical, *shard))?;
                let node: serde_json::Value = rmp_serde::from_slice(&blob).ok()?;
                Some((*shard, node.get("generation")?.as_u64()?))
            })
            .collect()
    }

    /// Execute one scheduled refresh plan (MEDULLA-DECISION-ENGINE-R174.8.4, R174.8.5): the
    /// only executor of a plan. A recorded receipt is returned as it is. Otherwise every shard
    /// of the plan is checked against its generation before any embedding, the nodes of the
    /// planned types are re-embedded, the shards are swapped, and the receipt is stored. The
    /// swapped generations become the durable shard pointer nodes the served write commits.
    fn refresh_drift(&self, logical: &str, plan_digest: &str) -> Result<LearningRecorded, String> {
        let tenant = self.ctx.tenant_id;
        let graph = self.graph(0)?;
        // The receipt is a graph node committed in the batch of the shard pointers
        // (MEDULLA-DECISION-ENGINE-R174.8.4), so a recorded receipt is read from the graph.
        if let Some(blob) = graph.core().get_node_properties(&drift_refresh_receipt_node_id(plan_digest)) {
            let mut value: serde_json::Value =
                rmp_serde::from_slice(&blob).map_err(|error| format!("drift refresh receipt: {error}"))?;
            if let Some(object) = value.as_object_mut() {
                object.remove("type");
            }
            let recorded: DriftRefreshed = serde_json::from_value(value)
                .map_err(|error| format!("drift refresh receipt: {error}"))?;
            return Ok(LearningRecorded::DriftRefreshed(Box::new(recorded)));
        }
        self.inputs
            .drift_lease
            .as_ref()
            .ok_or("a drift refresh holds no analytics lease")?;
        let bytes = self
            .ctx
            .store
            .decision_artifact(tenant, &format!("drift-plan:{plan_digest}"))?
            .ok_or_else(|| invalid_parameter("no scheduled refresh plan has that digest"))?;
        let scheduled: ScheduledRefresh = decode_artifact(&bytes, "drift refresh plan")?;
        if scheduled.plan.logical_graph != logical {
            return Err(invalid_parameter("the plan belongs to another logical graph"));
        }
        let embedder = self
            .inputs
            .drift_embedder
            .as_ref()
            .ok_or("a drift refresh was not prepared")?;
        let durable = self.durable_generations(graph, logical, &scheduled);
        let conflict = |found: super::drift_registry::SwapConflict| {
            refusal(
                StatisticalErrorCode::ShardSwapConflict,
                format!(
                    "shard {} is at generation {:?}, the plan was scheduled at {}",
                    found.shard, found.found, found.expected
                ),
            )
        };
        registry()
            .verify_generations(self.tenant(), logical, &scheduled.shard_generations, &durable)
            .map_err(conflict)?;

        // Re-embed each distinct node whose type a region names.
        let regions = scheduled.plan.regions.as_slice();
        let region_of = |node_type: &str| regions.iter().find(|region| region.node_type == node_type);
        let mut nodes: Vec<(String, u32)> = Vec::new();
        for id in graph.node_ids() {
            let node_type = graph
                .visible_properties(&id)
                .and_then(|props| props.get("type")?.as_str().map(str::to_string));
            if let Some(region) = node_type.as_deref().and_then(region_of) {
                nodes.push((id, region.shard_id));
            }
        }
        let batch = (medulla_server_api::knob_config::knobs().generation_batch_nodes as usize).max(1);
        for chunk in nodes.chunks(batch) {
            let texts: Vec<String> = chunk.iter().map(|(id, _)| graph.node_text(id)).collect();
            let vectors = embedder.embed(&texts)?;
            if vectors.len() != chunk.len() {
                return Err(invalid_parameter(
                    "the embedder returned a vector count other than the batch's",
                ));
            }
            for ((id, _), vector) in chunk.iter().zip(vectors) {
                graph.store_embedding(id, vector)?;
            }
        }

        // The generation pointers commit durably first; the in-memory swap and the monitors'
        // new reference follow the commit, so memory is never ahead of the store.
        let swapped = registry()
            .planned_swaps(self.tenant(), logical, &scheduled.shard_generations, &durable)
            .map_err(conflict)?;
        let observations: Vec<RefreshObservation> = nodes
            .iter()
            .filter_map(|(id, shard)| {
                let vector = graph.stored_vector(id)?;
                let node_type = graph
                    .visible_properties(id)
                    .and_then(|props| props.get("type")?.as_str().map(str::to_string))
                    .unwrap_or_default();
                let tokens: Vec<String> = graph
                    .node_text(id)
                    .split_whitespace()
                    .map(str::to_lowercase)
                    .collect();
                Some(RefreshObservation {
                    shard: *shard,
                    node_id: id.clone(),
                    node_type,
                    vector,
                    tokens,
                })
            })
            .collect();
        let version = graph.version();

        let receipt = DriftRefreshed {
            plan_digest: plan_digest.to_string(),
            logical_graph: logical.to_string(),
            regions: regions.to_vec(),
            reembedded_nodes: u32::try_from(nodes.len()).unwrap_or(u32::MAX),
            shards: swapped
                .iter()
                .map(|(shard_id, before, after)| ShardSwap {
                    shard_id: *shard_id,
                    generation_before: *before,
                    generation_after: *after,
                })
                .collect(),
            signals: scheduled.signals.clone(),
        };
        let mut methods = Vec::new();
        for swap in &receipt.shards {
            let node = ShardGenerationNode {
                schema_version: RETRIEVAL_DRIFT_SCHEMA_VERSION,
                logical_graph: logical.to_string(),
                shard_id: swap.shard_id,
                generation: swap.generation_after,
            };
            let mut props = serde_json::to_value(&node).map_err(|error| error.to_string())?;
            props["type"] = serde_json::json!(SHARD_GENERATION_NODE_TYPE);
            methods.push(medulla_protocol_types::protocol::Method::AddNode {
                node_id: shard_generation_node_id(logical, swap.shard_id),
                properties_msgpack: rmp_serde::to_vec_named(&props).map_err(|error| error.to_string())?,
            });
        }
        let mut receipt_props = serde_json::to_value(&receipt).map_err(|error| error.to_string())?;
        receipt_props["type"] = serde_json::json!(DRIFT_REFRESH_RECEIPT_NODE_TYPE);
        methods.push(medulla_protocol_types::protocol::Method::AddNode {
            node_id: drift_refresh_receipt_node_id(plan_digest),
            properties_msgpack: rmp_serde::to_vec_named(&receipt_props).map_err(|error| error.to_string())?,
        });
        let apply = {
            let tenant = self.tenant().to_string();
            let logical = logical.to_string();
            let swapped = swapped.clone();
            move || {
                registry().apply_committed_swaps(&tenant, &logical, &swapped);
                // The embedding writes reach the monitors through the same port the commit
                // path uses, so the refreshed shards' new reference collects them.
                if let Some(signals) = medulla_retrieval::discover_signals::discover_signals() {
                    for seen in &observations {
                        signals.observe_write(&medulla_retrieval::discover_signals::WrittenVector {
                            tenant: &tenant,
                            graph: &logical,
                            shard: seen.shard,
                            node_id: &seen.node_id,
                            node_type: &seen.node_type,
                            vector: &seen.vector,
                            tokens: &seen.tokens,
                            version,
                        });
                    }
                }
            }
        };
        *self
            .inputs
            .refresh_apply
            .lock()
            .map_err(|_| "the staged refresh slot is poisoned".to_string())? = Some(Box::new(apply));
        if !methods.is_empty() {
            *self
                .inputs
                .refresh_link
                .lock()
                .map_err(|_| "the staged refresh slot is poisoned".to_string())? =
                Some(super::run_record::StagedLink {
                    graph: logical.to_string(),
                    core: std::sync::Arc::clone(graph.core()),
                    methods,
                });
        }
        Ok(LearningRecorded::DriftRefreshed(Box::new(receipt)))
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
            PointerRef::RerankCalibration { scorer_version } => {
                qualified_calibration(store, self.tenant(), scorer_version, target, receipt)
            }
            PointerRef::Champion { subject } => {
                qualified_champion(store, self.tenant(), subject, target, receipt)
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
            PointerRef::RerankCalibration { scorer_version } => calibration_pointer(scorer_version),
            PointerRef::Champion { subject } => subject.pointer_key(),
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
            PointerMovement::Rollback => {
                // A champion with no earlier target has nothing to return to
                // (MEDULLA-DECISION-ENGINE-R805.4).
                if matches!(pointer, PointerRef::Champion { .. }) {
                    let (_, state) = read_pointer(self.ctx.store, self.tenant(), &at.key)?;
                    if state.active.is_some() && state.stack.is_empty() {
                        return Err(format!(
                            "{}: no earlier champion to return to",
                            medulla_decision_types::decision::codes::ROLLBACK_NO_PARENT.as_str()
                        ));
                    }
                }
                rollback(self.ctx.store, &at)?
            }
        };
        Ok(LearningRecorded::Pointer(Box::new(state)))
    }
}

/// A reading on the `Q16` scale, rounded to nearest.
fn q16_round(value: f64) -> i64 {
    (value * 65_536.0).round() as i64
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
        // A refresh's logical graph is a virtual graph read through its
        // registered search source (R174.9.3); it needs no materialized graph,
        // and a graph with no source refuses SEARCH_SOURCE_UNREGISTERED (R174.9.4).
        // A drift refresh executes against the materialized graph it re-embeds.
        LearningWrite::RefreshDrift { logical_graph, .. } => vec![logical_graph.as_str()],
        LearningWrite::RefreshFreshness { .. }
        | LearningWrite::RecordOutcome { .. }
        | LearningWrite::FitCalibration { .. }
        | LearningWrite::EvaluateDrift { .. }
        | LearningWrite::RegisterDriftPolicy { .. }
        | LearningWrite::RunOutcome { .. }
        | LearningWrite::UploadDistilledScores { .. }
        | LearningWrite::PackageRun { .. }
        | LearningWrite::RecordFeedback { .. }
        | LearningWrite::RecordRollouts { .. }
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

/// Request the background analytics lease a drift evaluation holds. The request is idempotent
/// per graph and evaluation window, and expires with its ttl.
async fn acquire_drift_lease(
    state: &SharedState,
    verified: &VerifiedRequestContext,
    tenant: &str,
    logical: &str,
    cell: &str,
    refreshing: Option<&str>,
) -> Result<String, String> {
    let persistence = state.read().await.persistence.clone().ok_or_else(|| {
        "CAPACITY_UNAVAILABLE: a drift evaluation needs the durable capacity ledger".to_string()
    })?;
    let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    // An evaluation's job is per graph and window; a refresh's is its plan.
    let id = match refreshing {
        Some(plan_digest) => format!("drift-refresh:{plan_digest}"),
        None => format!(
            "drift-eval:{}",
            digest_text(
                "eg/drift-eval-job/v1",
                &(logical, now_ms / CYCLE_LEASE_TTL_MS)
            )
        ),
    };
    let lease_id = format!("lease:{id}");
    let acquire = CapacityAcquireRequest {
        schema_version: NativeControlSchemaVersion::V1,
        tenant_ref: tenant.to_string(),
        work_item_id: id.clone(),
        owner_digest: digest_text("eg/drift-eval-owner/v1", &verified.agent_id()),
        idempotency_key: id,
        priority: LeasePriority::BackgroundIngestion,
        demands: vec![CapacityDemand {
            cell_id: cell.to_string(),
            resource_class: CapacityResourceClass::Cpu,
            amount: 1,
        }],
        lease_id: Some(lease_id.clone()),
        ttl_ms: CYCLE_LEASE_TTL_MS,
        now_ms,
        cost_budget_micros: None,
        token_budget: None,
    };
    persistence
        .commit_capacity_lease(
            &medulla_kernel::persist::sanitize(logical),
            Method::AcquireCapacity { request: acquire },
        )
        .await
        .map_err(|error| format!("CAPACITY_UNAVAILABLE: {error}"))?;
    Ok(lease_id)
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
    let drift_lease = match write {
        LearningWrite::EvaluateDrift {
            logical_graph,
            capacity_cell,
            ..
        } => Some(
            acquire_drift_lease(
                state,
                verified,
                &served_tenant,
                logical_graph,
                capacity_cell,
                None,
            )
            .await?,
        ),
        LearningWrite::RefreshDrift {
            logical_graph,
            plan_digest,
            capacity_cell,
        } => Some(
            acquire_drift_lease(
                state,
                verified,
                &served_tenant,
                logical_graph,
                capacity_cell,
                Some(plan_digest),
            )
            .await?,
        ),
        _ => None,
    };
    let drift_embedder: Option<Box<dyn Reembedder>> = match write {
        LearningWrite::RefreshDrift { .. } => Some(Box::new(ConfiguredEmbedder(
            medulla_server_ops::api::ingest::embedder::EmbedderConfig::from_env(),
        ))),
        _ => None,
    };
    let refresh_graph = match write {
        LearningWrite::RefreshFreshness { request } => {
            super::stat_freshness::open_graph(state, verified, request).await?
        }
        _ => None,
    };
    Ok(Some(LearningInputs {
        served_tenant,
        graphs,
        cycle,
        drift_lease,
        drift_embedder,
        refresh_graph,
        refresh_link: Default::default(),
        refresh_apply: Default::default(),
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
        LearningWrite::FitCalibration { request } => s.calibrate(&request),
        LearningWrite::EvaluateGeneration { request } => s.evaluate(&request),
        LearningWrite::RunGenerationCycle { request } => s.cycle(&request),
        LearningWrite::RefreshFreshness { request } => {
            let staged = super::stat_freshness::refresh(
                ctx,
                &request,
                inputs.refresh_graph.as_ref(),
                &reader.principal,
            )?;
            *inputs
                .refresh_link
                .lock()
                .map_err(|_| "the staged refresh slot is poisoned".to_string())? = staged.link;
            Ok(LearningRecorded::Refreshed(Box::new(staged.receipt)))
        }
        LearningWrite::RunOutcome { outcome } => Ok(LearningRecorded::Run(Box::new(record_run(
            ctx, reader, *outcome,
        )?))),
        LearningWrite::UploadDistilledScores { upload } => Ok(LearningRecorded::DistilledScores(
            Box::new(upload_scores(ctx, *upload)?),
        )),
        LearningWrite::RecordFeedback { event } => Ok(LearningRecorded::Feedback(Box::new(
            super::stat_feedback::record_feedback(ctx, reader, *event)?,
        ))),
        LearningWrite::PackageRun { run } => Ok(LearningRecorded::PackageRun(Box::new(
            super::stat_package_run::record_package_run(ctx, *run)?,
        ))),
        LearningWrite::EvaluateDrift {
            logical_graph,
            priority,
            ..
        } => s.evaluate_drift(&logical_graph, priority),
        LearningWrite::RegisterDriftPolicy {
            logical_graph,
            region_budget,
        } => s.register_drift_policy(&logical_graph, region_budget),
        LearningWrite::RefreshDrift {
            logical_graph,
            plan_digest,
            ..
        } => s.refresh_drift(&logical_graph, &plan_digest),
        LearningWrite::RecordRollouts { set } => Ok(LearningRecorded::Rollouts(Box::new(
            record_rollouts(ctx, *set)?,
        ))),
        LearningWrite::MovePointer { pointer, movement } => s.move_pointer(&pointer, &movement),
    }
}
