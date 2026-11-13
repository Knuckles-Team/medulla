//! One served retrieval round (MEDULLA-FEDERATED-QUERY-R081.11, R081.3, R081.7, R081.9).
//!
//! `run_round` executes the request's plan over the snapshot the caller's
//! `PlanCtx` is bound to, then runs the decision parts the request carries, in a
//! fixed order: hierarchy, sufficiency, selection. A part that is absent is skipped, and
//! a request with no parts returns the rows only. Nothing here writes. Row-level security
//! and tenant scope are applied by the handler before the `PlanCtx` is built, so the plan
//! and every part can only see what the caller may read.

use crate::retrieval_stop::{decide_stop, round_progress};
use medulla_decision_stats::sparse_select::{joint_sparse_select, SparseSelectError};
use medulla_exec::exec::{execute, PlanCtx};
use medulla_exec::rowset::RowSet;
use medulla_result_types::retrieval_round::{
    selection_policy_digest, sha256_hex_of, ActivationBenchmark, HierarchyPart, InferenceCounts,
    JointCoefficient, JointSelection, RetrievalPremise, RoundRequest, RoundResult, RoundRow,
    SelectionMode, SelectionPart, SolverStatus, StopKnobs,
};
use medulla_result_types::retrieval_verdict::RepairStep;
use medulla_result_types::retrieval_verdict::{
    ChunkScorePair, DescentPolicy, HierarchyRetrievalKind, HierarchyStage, HierarchyStageReceipt,
    RetrievalRefusal, SectionGranularity as WireGranularity, RETRIEVAL_CONTRACT_INVALID,
    RETRIEVAL_EMBEDDING_REVISION_MISMATCH, RETRIEVAL_POLICY_UNVERIFIED,
    RETRIEVAL_SCORE_PAIR_REQUIRED, RETRIEVAL_SOLVER_NOT_CONVERGED,
};
use medulla_retrieval::leanrag::{retrieve_vector_free, SectionGranularity, VectorFreeLimits};
use medulla_retrieval::retrieval_verdict::{
    check_activation, descend_hierarchy, judge_sufficiency, select_chunks, SufficiencyCall,
    SufficiencyRule,
};

use crate::hierarchy_source::{
    load_communities, section_refusal, GraphHierarchySource, GraphSectionTopology, CALLER_SCOPE,
};

/// The registered knobs a round reads (`retrieval.*`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoundKnobs {
    /// `knob retrieval.min_evidence`, rows.
    pub min_evidence: u64,
    /// `knob retrieval.round_limit`, rounds. A caller may lower it, never raise it.
    pub round_limit: u32,
    /// `knob retrieval.rerank_pool`, chunks the reranker scores for selection.
    pub rerank_pool: usize,
    /// `knob retrieval.selection_quality_floor`, nDCG the policy may lose to its baseline.
    pub selection_quality_floor: f64,
    /// `knob retrieval.selection_cost_ratio_max`, the policy's mean count over the baseline's.
    pub selection_cost_ratio_max: f64,
    /// `knob retrieval.section_max_depth`, levels.
    pub section_max_depth: usize,
    /// `knob retrieval.section_max_visited`, records.
    pub section_max_visited: usize,
    /// `knob retrieval.token_limit`, tokens. A caller may lower it, never raise it.
    pub token_limit: u64,
    /// `knob retrieval.inference_limit`, calls. A caller may lower it, never raise it.
    pub inference_limit: u32,
    /// `knob retrieval.cost_limit`, micro-units. A caller may lower it, never raise it.
    pub cost_limit: u64,
    /// `knob retrieval.saturation_epsilon`, score.
    pub saturation_epsilon: f64,
    /// `knob retrieval.joint_lambda`, unitless. Used when a joint request names no lambda.
    pub joint_lambda: f64,
    /// `knob retrieval.joint_tolerance`, unitless. Used when a joint request names none.
    pub joint_tolerance: f64,
    /// `knob retrieval.joint_max_iters`, sweeps. A caller may lower it, never raise it.
    pub joint_max_iters: u32,
}

impl Default for RoundKnobs {
    fn default() -> Self {
        Self {
            min_evidence: 3,
            round_limit: 4,
            rerank_pool: 50,
            selection_quality_floor: 0.01,
            selection_cost_ratio_max: 0.9,
            section_max_depth: 32,
            section_max_visited: 4096,
            token_limit: 32_000,
            inference_limit: 8,
            cost_limit: 1_000_000,
            saturation_epsilon: 0.01,
            joint_lambda: 0.1,
            joint_tolerance: 0.000001,
            joint_max_iters: 1000,
        }
    }
}

/// Why a round did not return a result.
#[derive(Debug, Clone, PartialEq)]
pub enum RoundError {
    /// The plan failed to execute.
    Plan(String),
    /// A part refused with a typed retrieval code.
    Refusal(RetrievalRefusal),
}

impl From<RetrievalRefusal> for RoundError {
    fn from(refusal: RetrievalRefusal) -> Self {
        Self::Refusal(refusal)
    }
}

fn contract_invalid(detail: &str) -> RoundError {
    RetrievalRefusal::new(RETRIEVAL_CONTRACT_INVALID, detail).into()
}

fn score_pair_required(detail: String) -> RoundError {
    RetrievalRefusal::new(RETRIEVAL_SCORE_PAIR_REQUIRED, detail).into()
}

/// Check what the request must carry before any row is read.
fn validate(request: &RoundRequest) -> Result<(), RoundError> {
    if request.corpus_revision.trim().is_empty() || request.model_revision.trim().is_empty() {
        return Err(contract_invalid(
            "a round names the corpus revision and the model revision it retrieved with",
        ));
    }
    if let Some(part) = &request.sufficiency {
        if part.round == 0 || part.k == 0 {
            return Err(contract_invalid(
                "sufficiency round is 1-based and k is at least 1",
            ));
        }
    }
    Ok(())
}

/// What the steps of one round accumulate beside the rows.
#[derive(Default)]
struct Steps {
    inference: InferenceCounts,
    premises: Vec<RetrievalPremise>,
}

/// The stored text of a chunk: the node property `text` on the round's snapshot.
fn chunk_text(ctx: &PlanCtx, id: &str) -> Option<String> {
    ctx.view
        .node_row_object(id)?
        .get("text")?
        .as_str()
        .map(str::to_string)
}

/// The benchmark digest this round accepts as approval, or `None` when the evidence is
/// absent or misses the quality floor or the cost ratio (R081.7.5). The digest itself is
/// then compared by [`check_activation`], the one comparison path.
fn approved_benchmark<'a>(
    benchmark: Option<&'a ActivationBenchmark>,
    knobs: &RoundKnobs,
) -> Option<&'a str> {
    let evidence = benchmark?;
    let (base, cand) = (evidence.baseline, evidence.candidate);
    let finite = [
        base.ndcg_at_10,
        base.mean_selected,
        cand.ndcg_at_10,
        cand.mean_selected,
    ]
    .iter()
    .all(|v| v.is_finite());
    let quality_ok = base.ndcg_at_10 - cand.ndcg_at_10 <= knobs.selection_quality_floor;
    let cost_ok = cand.mean_selected <= knobs.selection_cost_ratio_max * base.mean_selected;
    (finite && quality_ok && cost_ok).then_some(evidence.digest.as_str())
}

/// Dual-score chunk selection (R081.7.3, R081.7.5). Selection itself calls no model: the
/// embedding score is the executed row's score and the cross-encoder score comes from the
/// bound [`Reranker`](medulla_exec::exec::Reranker), which this step calls once.
fn run_selection(
    request: &RoundRequest,
    part: &SelectionPart,
    rows: &RowSet,
    ctx: &PlanCtx,
    knobs: &RoundKnobs,
    steps: &mut Steps,
) -> Result<medulla_result_types::retrieval_verdict::SelectionReceipt, RoundError> {
    let reranker = ctx.reranker.ok_or_else(|| {
        score_pair_required(
            "no reranker is bound; set `retrieval.reranker_model` to score chunks".into(),
        )
    })?;
    let approved = approved_benchmark(part.benchmark.as_ref(), knobs);
    check_activation(&part.policy, approved)?;
    let pool: Vec<_> = rows.rows().iter().take(knobs.rerank_pool).collect();
    let mut texts = Vec::with_capacity(pool.len());
    for row in &pool {
        if row.score.is_none() {
            return Err(score_pair_required(format!(
                "row `{}` carries no embedding score",
                row.id
            )));
        }
        texts.push(chunk_text(ctx, &row.id).ok_or_else(|| {
            score_pair_required(format!("chunk `{}` has no `text` property", row.id))
        })?);
    }
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let relevance = reranker.score(&part.query, &refs);
    if relevance.len() != refs.len() {
        return Err(score_pair_required(
            "the reranker returned a different number of scores than chunks".into(),
        ));
    }
    steps.inference.reranker += refs.len() as u32;
    let reranker_revision = reranker.revision();
    let pairs: Vec<ChunkScorePair> = pool
        .iter()
        .zip(&relevance)
        .map(|(row, r)| ChunkScorePair {
            chunk_id: row.id.clone(),
            corpus_revision: request.corpus_revision.clone(),
            embedding_score: f64::from(row.score.unwrap_or_default()),
            cross_encoder_score: f64::from(*r),
            embedding_model_revision: request.model_revision.clone(),
            cross_encoder_model_revision: reranker_revision.clone(),
        })
        .collect();
    let receipt = select_chunks(&part.policy, approved, &pairs)?;
    steps.inference.selection += receipt.selection_inference_calls;
    let excluded: Vec<&str> = receipt
        .candidates
        .iter()
        .filter(|c| !c.selected)
        .map(|c| c.chunk_id.as_str())
        .collect();
    let unscored: Vec<&str> = rows
        .rows()
        .iter()
        .skip(knobs.rerank_pool)
        .map(|r| r.id.as_str())
        .collect();
    steps.premises.push(RetrievalPremise {
        step: "selection".into(),
        name: "retrieval.selection".into(),
        value: serde_json::json!({
            "receipt": receipt,
            "policy_digest": selection_policy_digest(&part.policy),
            "embedding_model_revision": request.model_revision,
            "reranker_revision": reranker_revision,
            "selected_count": receipt.selected.len(),
            "excluded": excluded,
            "unscored": unscored,
            "inference": steps.inference,
        }),
    });
    Ok(receipt)
}

/// Joint sparse nonnegative selection (R081.100): reconstruct the query vector from the
/// stored embeddings of the executed rows and keep the rows with a positive coefficient.
/// The rows are the plan's rows, so tenant and row-level security already bound them; the
/// policy's `max_chunks` bounds the result count. Calls no reranker and no generation model.
fn run_joint_selection(
    request: &RoundRequest,
    part: &SelectionPart,
    rows: &RowSet,
    ctx: &PlanCtx,
    knobs: &RoundKnobs,
    steps: &mut Steps,
) -> Result<JointSelection, RoundError> {
    let SelectionMode::JointSparse {
        embedding_revision,
        lambda,
        tolerance,
        max_iters,
        query_vector,
    } = &part.mode
    else {
        return Err(contract_invalid("not a joint selection"));
    };
    part.policy
        .validate()
        .map_err(|e| contract_invalid(&e.to_string()))?;
    if embedding_revision != &request.model_revision
        || embedding_revision != &part.policy.embedding_model_revision
    {
        return Err(RetrievalRefusal::new(
            RETRIEVAL_EMBEDDING_REVISION_MISMATCH,
            format!(
                "joint selection names embedding revision `{embedding_revision}`; the round and \
                 the policy name `{}` and `{}`",
                request.model_revision, part.policy.embedding_model_revision
            ),
        )
        .into());
    }
    let lambda = lambda.unwrap_or(knobs.joint_lambda);
    let tolerance = tolerance.unwrap_or(knobs.joint_tolerance);
    let sweeps = max_iters
        .unwrap_or(knobs.joint_max_iters)
        .min(knobs.joint_max_iters);
    let unverified =
        |detail: &str| RoundError::from(RetrievalRefusal::new(RETRIEVAL_POLICY_UNVERIFIED, detail));
    if !(lambda.is_finite() && lambda > 0.0) {
        return Err(unverified("lambda must be a positive finite number"));
    }
    if !(tolerance.is_finite() && tolerance > 0.0) || sweeps == 0 {
        return Err(unverified(
            "tolerance must be positive and at least one sweep is required",
        ));
    }
    let query: Vec<f32> = match query_vector {
        Some(vector) => vector.clone(),
        None => {
            if part.query.trim().is_empty() {
                return Err(contract_invalid(
                    "a joint selection needs a query vector or a query text",
                ));
            }
            let embedder = ctx
                .embedder
                .ok_or_else(|| contract_invalid("no text embedder is bound to embed the query"))?;
            steps.inference.embedding += 1;
            embedder.embed(&part.query).map_err(RoundError::Plan)?
        }
    };
    let pool: Vec<_> = rows.rows().iter().take(knobs.rerank_pool).collect();
    let mut matrix: Vec<Vec<f64>> = Vec::with_capacity(pool.len());
    for row in &pool {
        let stored = ctx.semantic.get_embedding(&row.id).ok_or_else(|| {
            score_pair_required(format!("row `{}` carries no stored embedding", row.id))
        })?;
        if stored.len() != query.len() {
            return Err(RetrievalRefusal::new(
                RETRIEVAL_EMBEDDING_REVISION_MISMATCH,
                format!(
                    "row `{}` has {} dimensions; the query has {}",
                    row.id,
                    stored.len(),
                    query.len()
                ),
            )
            .into());
        }
        matrix.push(stored.iter().map(|v| f64::from(*v)).collect());
    }
    let q64: Vec<f64> = query.iter().map(|v| f64::from(*v)).collect();
    let solved = joint_sparse_select(&q64, &matrix, lambda, tolerance, sweeps).map_err(
        |error| match error {
            SparseSelectError::NotConverged { .. } => RoundError::from(RetrievalRefusal::new(
                RETRIEVAL_SOLVER_NOT_CONVERGED,
                error.to_string(),
            )),
            SparseSelectError::Shape(detail) => RoundError::from(RetrievalRefusal::new(
                RETRIEVAL_EMBEDDING_REVISION_MISMATCH,
                detail,
            )),
            SparseSelectError::Parameter(detail) => unverified(&detail),
        },
    )?;
    let mut positive: Vec<JointCoefficient> = pool
        .iter()
        .zip(&solved.coefficients)
        .filter(|(_, c)| **c > 0.0)
        .map(|(row, c)| JointCoefficient {
            id: row.id.clone(),
            coefficient: *c,
        })
        .collect();
    positive.sort_by(|a, b| {
        b.coefficient
            .total_cmp(&a.coefficient)
            .then_with(|| a.id.cmp(&b.id))
    });
    let cap = part.policy.max_chunks as usize;
    let truncated = positive.len().saturating_sub(cap) as u32;
    positive.truncate(cap);
    let selection = JointSelection {
        embedding_revision: embedding_revision.clone(),
        selected: positive,
        residual: solved.residual,
        status: SolverStatus::Converged,
        iterations: solved.iterations,
        lambda,
        tolerance,
        truncated,
    };
    steps.premises.push(RetrievalPremise {
        step: "selection".into(),
        name: "retrieval.joint_selection".into(),
        value: serde_json::json!({
            "selection": selection,
            "policy_digest": selection_policy_digest(&part.policy),
            "candidates": pool.len(),
            "unscored": rows.rows().len().saturating_sub(pool.len()),
            "inference": steps.inference,
        }),
    });
    Ok(selection)
}

/// The hierarchy step (R081.9.3, R081.9.5, R081.9.7.3, R081.9.7.5). Returns the selected
/// entity ids (community policy) or section ids (document-section policy).
fn run_hierarchy(
    request: &RoundRequest,
    part: &HierarchyPart,
    ctx: &PlanCtx,
    knobs: &RoundKnobs,
    steps: &mut Steps,
) -> Result<Vec<String>, RoundError> {
    match part.policy.kind {
        HierarchyRetrievalKind::Community => run_community(request, part, ctx, steps),
        HierarchyRetrievalKind::DocumentSection => run_sections(part, ctx, knobs, steps),
    }
}

fn run_community(
    request: &RoundRequest,
    part: &HierarchyPart,
    ctx: &PlanCtx,
    steps: &mut Steps,
) -> Result<Vec<String>, RoundError> {
    let policy = &part.policy;
    policy
        .validate()
        .map_err(|e| contract_invalid(&e.to_string()))?;
    if part.query.trim().is_empty() {
        return Err(contract_invalid("a community descent needs a query"));
    }
    if !policy.section_nodes.is_empty() || part.section.is_some() {
        return Err(contract_invalid(
            "a community descent carries no section nodes or section request",
        ));
    }
    let embedder = ctx
        .embedder
        .ok_or_else(|| contract_invalid("no text embedder is bound to score summaries"))?;
    let query = embedder.embed(&part.query).map_err(RoundError::Plan)?;
    steps.inference.embedding += 1;
    let parents: Vec<(String, String)> = policy
        .parent_links
        .iter()
        .map(|l| (l.parent_id.clone(), l.child_id.clone()))
        .collect();
    let communities = load_communities(
        ctx.view,
        &policy.community_ids,
        &policy.summary_ids,
        &parents,
    )?;
    let descent = DescentPolicy {
        hierarchy_revision: policy.hierarchy_revision.clone(),
        policy_revision: policy.hierarchy_revision.clone(),
        beam_width: policy.beam_width,
        max_depth: policy.max_depth,
        max_visited_communities: policy.max_visited_communities,
        max_entity_candidates: policy.max_entity_candidates,
        max_reranker_calls: policy.max_reranker_calls,
        max_tokens: u64::from(policy.max_tokens),
        max_cost: policy.max_total_cost_micros,
    };
    let mut source = GraphHierarchySource::new(ctx.view, ctx.semantic, query);
    let receipt = descend_hierarchy(&descent, CALLER_SCOPE, &communities, &mut source)
        .map_err(|refusal| RoundError::Refusal(refusal.refusal))?;
    steps.inference.reranker += receipt.reranker_calls;
    // The flat baseline of the same query: scoring every stored entity once.
    let mut flat: Vec<&str> = communities
        .iter()
        .flat_map(|c| c.entity_ids.iter().map(String::as_str))
        .collect();
    flat.sort_unstable();
    flat.dedup();
    steps.premises.push(RetrievalPremise {
        step: "hierarchy".into(),
        name: "retrieval.hierarchy".into(),
        value: serde_json::json!({
            "receipt": receipt,
            "summary_revision": policy.hierarchy_revision,
            "corpus_revision": request.corpus_revision,
            "flat_visits": flat.len(),
            "flat_candidates": flat.len(),
        }),
    });
    Ok(receipt.entities.iter().map(|v| v.id.clone()).collect())
}

fn run_sections(
    part: &HierarchyPart,
    ctx: &PlanCtx,
    knobs: &RoundKnobs,
    steps: &mut Steps,
) -> Result<Vec<String>, RoundError> {
    let policy = &part.policy;
    policy
        .validate_served_bounds()
        .map_err(|e| contract_invalid(&e.to_string()))?;
    let request = part
        .section
        .as_ref()
        .ok_or_else(|| contract_invalid("a document-section policy needs a section request"))?;
    let granularity = match request.granularity {
        WireGranularity::Document => SectionGranularity::Document,
        WireGranularity::Section => SectionGranularity::Section,
        WireGranularity::SourceSpan => SectionGranularity::SourceSpan,
    };
    let limits = VectorFreeLimits {
        max_depth: (policy.max_depth as usize).min(knobs.section_max_depth),
        max_visited: (policy.max_visited_communities as usize).min(knobs.section_max_visited),
    };
    let topology = GraphSectionTopology::new(ctx.view);
    let walk = retrieve_vector_free(
        &topology,
        &request.root_id,
        &request.document_revision,
        granularity,
        limits,
    )
    .map_err(|e| RoundError::Refusal(section_refusal(e)))?;
    let reads = topology.reads();
    let on_path: std::collections::BTreeSet<&str> =
        walk.paths.iter().flatten().map(String::as_str).collect();
    let visited_ids: Vec<String> = reads.iter().map(|r| r.id.clone()).collect();
    let digests: Vec<String> = reads
        .iter()
        .map(|r| format!("{}:{}", r.id, r.summary_digest))
        .collect();
    let receipt = HierarchyStageReceipt {
        hierarchy_revision: policy.hierarchy_revision.clone(),
        kind: HierarchyRetrievalKind::DocumentSection,
        stage: HierarchyStage::DocumentSection,
        pruned_ids: visited_ids
            .iter()
            .filter(|id| !on_path.contains(id.as_str()))
            .cloned()
            .collect(),
        selected_id: walk.selected.first().map(|r| r.section_id.clone()),
        visited_ids,
        score_provenance: policy.score_provenance.clone(),
        beam_width: policy.beam_width,
        depth: walk
            .paths
            .iter()
            .map(|p| p.len() as u32 - 1)
            .max()
            .unwrap_or(0),
        paths: walk.paths.clone(),
        selected_spans: walk
            .selected
            .iter()
            .map(
                |r| medulla_result_types::retrieval_verdict::SectionSourceSpan {
                    start: r.source_span.start,
                    end: r.source_span.end,
                },
            )
            .collect(),
        document_revision: request.document_revision.clone(),
        summary_digest: sha256_hex_of(&digests),
        policy_revision: policy.hierarchy_revision.clone(),
    };
    receipt
        .validate()
        .map_err(|e| contract_invalid(&e.to_string()))?;
    // Every visited record with its root-to-record path, so rejected branches can be
    // rebuilt from the premise alone.
    let parent_of: std::collections::BTreeMap<&str, Option<&str>> = reads
        .iter()
        .map(|r| (r.id.as_str(), r.parent_id.as_deref()))
        .collect();
    let visits: Vec<_> = reads
        .iter()
        .map(|r| {
            let mut path = vec![r.id.clone()];
            let mut cursor = r.parent_id.as_deref();
            while let Some(parent) = cursor {
                path.push(parent.to_string());
                cursor = parent_of.get(parent).copied().flatten();
            }
            path.reverse();
            serde_json::json!({ "id": r.id, "path": path })
        })
        .collect();
    steps.premises.push(RetrievalPremise {
        step: "hierarchy".into(),
        name: "retrieval.sections".into(),
        value: serde_json::json!({ "receipt": receipt, "visits": visits }),
    });
    Ok(walk.selected.into_iter().map(|r| r.section_id).collect())
}

/// Execute the plan, then run the requested parts in order: hierarchy, sufficiency,
/// selection, then stop (feedback and arbitration join with their own rows). The selected
/// hierarchy ids join the evidence set the sufficiency part counts.
pub fn run_round(
    request: &RoundRequest,
    ctx: &PlanCtx,
    knobs: &RoundKnobs,
) -> Result<RoundResult, RoundError> {
    run_round_with_state(request, ctx, knobs, None)
}

/// What a `state` part reads: the stored-state source, the bound encoder and the
/// segment policy the states were built under (MEDULLA-FEDERATED-QUERY-R081.101.3).
pub struct StateRuntime<'a> {
    pub source: &'a dyn crate::doc_state::StateSource,
    pub encoder: &'a dyn crate::doc_state::StateEncoder,
    pub policy_revision: String,
}

/// [`run_round`] with the state runtime the server binds. A `state` part with no
/// runtime refuses `RETRIEVAL_STATE_STALE`: nothing is built at query time.
pub fn run_round_with_state(
    request: &RoundRequest,
    ctx: &PlanCtx,
    knobs: &RoundKnobs,
    state_runtime: Option<&StateRuntime<'_>>,
) -> Result<RoundResult, RoundError> {
    validate(request)?;
    let rows = execute(&request.plan, ctx).map_err(RoundError::Plan)?;
    let mut steps = Steps::default();
    let hierarchy_selected = match &request.hierarchy {
        None => Vec::new(),
        Some(part) => {
            let _span = tracing::info_span!("medulla.retrieval.hierarchy").entered();
            run_hierarchy(request, part, ctx, knobs, &mut steps)?
        }
    };
    let sufficiency = match &request.sufficiency {
        None => None,
        Some(part) => {
            let _span = tracing::info_span!("medulla.retrieval.sufficiency").entered();
            let evidence = RowSet::from_rows(
                rows.rows()
                    .iter()
                    .map(|r| (r.id.clone(), r.score))
                    .chain(hierarchy_selected.iter().map(|id| (id.clone(), None))),
            );
            let rule = SufficiencyRule {
                min_evidence: knobs.min_evidence,
                ..SufficiencyRule::default()
            };
            let call = SufficiencyCall {
                round: part.round,
                round_limit: part.round_limit.map(|l| l.min(knobs.round_limit)),
                k: part.k,
                fallback_mode: part.fallback_mode.clone(),
                fallback: part.fallback,
            };
            Some(judge_sufficiency(&evidence, &rule, &call)?)
        }
    };
    let mut joint_selection = None;
    let selection = match &request.selection {
        None => None,
        Some(part) if matches!(part.mode, SelectionMode::JointSparse { .. }) => {
            let _span = tracing::info_span!("medulla.retrieval.joint_selection").entered();
            joint_selection = Some(run_joint_selection(
                request, part, &rows, ctx, knobs, &mut steps,
            )?);
            None
        }
        Some(part) => {
            let _span = tracing::info_span!("medulla.retrieval.selection").entered();
            Some(run_selection(request, part, &rows, ctx, knobs, &mut steps)?)
        }
    };
    let mut sufficiency = sufficiency;
    let stop = match &request.stop {
        None => None,
        Some(part) => {
            let _span = tracing::info_span!("medulla.retrieval.stop").entered();
            let round = part
                .history
                .as_slice()
                .last()
                .map(|answer| answer.round)
                .or_else(|| request.sufficiency.as_ref().map(|s| s.round))
                .unwrap_or(1);
            // Progress is measured only when an earlier round left evidence to compare.
            let measured =
                !part.prior_row_ids.as_slice().is_empty() || part.prior_quality.is_some();
            let stop_knobs = StopKnobs {
                round_limit: knobs.round_limit,
                token_limit: knobs.token_limit,
                inference_limit: knobs.inference_limit,
                cost_limit: knobs.cost_limit,
            };
            let policy = format!("{}:{}", request.corpus_revision, request.model_revision);
            let context = part
                .context(&stop_knobs, round, measured, &policy)
                .map_err(|detail| contract_invalid(&detail))?;
            let returned: Vec<&str> = rows.rows().iter().map(|r| r.id.as_str()).collect();
            let scores: Vec<f64> = rows
                .rows()
                .iter()
                .filter_map(|r| r.score.map(f64::from))
                .collect();
            let quality =
                (!scores.is_empty()).then(|| scores.iter().sum::<f64>() / scores.len() as f64);
            let progress = round_progress(
                &returned,
                part.prior_row_ids.as_slice(),
                quality,
                part.prior_quality,
                knobs.saturation_epsilon,
            );
            let verdict = decide_stop(
                &part.calibration,
                part.history.as_slice(),
                &context,
                &progress,
            );
            if verdict.stop {
                if let Some(s) = sufficiency.as_mut() {
                    s.repair = RepairStep::Stop;
                }
            }
            Some(verdict)
        }
    };
    let state = match &request.state {
        None => None,
        Some(part) => {
            let _span = tracing::info_span!("medulla.retrieval.state").entered();
            let Some(runtime) = state_runtime else {
                return Err(RetrievalRefusal::new(
                    medulla_result_types::retrieval_verdict::RETRIEVAL_STATE_STALE,
                    "no document state encoder is bound",
                )
                .into());
            };
            let texts: Vec<(String, Option<String>)> = rows
                .rows()
                .iter()
                .map(|r| (r.id.clone(), chunk_text(ctx, &r.id)))
                .collect();
            let candidates: Vec<crate::doc_state::StateCandidate<'_>> = texts
                .iter()
                .map(|(id, text)| crate::doc_state::StateCandidate {
                    id,
                    text: text.as_deref(),
                })
                .collect();
            Some(crate::doc_state::rerank_with_state(
                runtime.encoder,
                runtime.source,
                &runtime.policy_revision,
                &part.query,
                &candidates,
            )?)
        }
    };
    Ok(RoundResult {
        state,
        rows: rows
            .rows()
            .iter()
            .map(|r| RoundRow {
                id: r.id.clone(),
                score: r.score,
            })
            .collect(),
        sufficiency,
        selection,
        joint_selection,
        hierarchy_selected,
        stop,
        inference: steps.inference,
        premises: steps.premises,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_graph_core::graph::GraphCore;
    use medulla_result_types::retrieval_round::{BenchmarkResult, SufficiencyPart};
    use medulla_result_types::retrieval_verdict::{
        FallbackPolicy, RepairStep, Verdict, RETRIEVAL_ROUND_LIMIT_REQUIRED,
    };
    use medulla_semantic_index::semantic::SemanticStore;
    use medulla_types::wire::{Op, Plan};

    const KNOBS: RoundKnobs = RoundKnobs {
        min_evidence: 3,
        round_limit: 4,
        rerank_pool: 50,
        selection_quality_floor: 0.01,
        selection_cost_ratio_max: 0.9,
        section_max_depth: 32,
        section_max_visited: 4096,
        token_limit: 32_000,
        inference_limit: 8,
        cost_limit: 1_000_000,
        saturation_epsilon: 0.01,
        joint_lambda: 0.1,
        joint_tolerance: 0.000001,
        joint_max_iters: 1000,
    };

    fn blob(label: &str) -> Vec<u8> {
        rmp_serde::to_vec_named(&serde_json::json!({ "type": label })).expect("encodes")
    }

    fn request(sufficiency: Option<SufficiencyPart>) -> RoundRequest {
        RoundRequest {
            plan: Plan::new(vec![Op::Scan {
                label: "Doc".into(),
            }]),
            corpus_revision: "corpus-1".into(),
            model_revision: "model-1".into(),
            sufficiency,
            selection: None,
            hierarchy: None,
            stop: None,
            state: None,
        }
    }

    fn part(round_limit: Option<u32>) -> SufficiencyPart {
        SufficiencyPart {
            round: 1,
            round_limit,
            k: 4,
            fallback_mode: "extractive".into(),
            fallback: FallbackPolicy::Lenient,
        }
    }

    fn run(docs: usize, request: &RoundRequest) -> Result<RoundResult, RoundError> {
        let core = GraphCore::new();
        for i in 0..docs {
            core.add_node(format!("d{i}"), blob("Doc"));
        }
        core.add_node("other".into(), blob("Other"));
        let view = core.analysis_snapshot();
        let semantic = SemanticStore::new();
        run_round(request, &PlanCtx::new(&view, &semantic), &KNOBS)
    }

    #[test]
    fn run_round_returns_verdict_for_rule_plan() {
        // Two rows are below min_evidence 3: insufficient, boost k on round 1.
        let short = run(2, &request(Some(part(Some(4))))).expect("round runs");
        assert_eq!(short.rows.len(), 2);
        let verdict = short.sufficiency.expect("verdict beside rows");
        assert_eq!(verdict.verdict, Verdict::Insufficient);
        assert_eq!(verdict.evidence_count, 2);
        assert_eq!(verdict.repair, RepairStep::BoostK { k: 8 });

        // Three rows meet the rule.
        let enough = run(3, &request(Some(part(Some(4))))).expect("round runs");
        let verdict = enough.sufficiency.expect("verdict beside rows");
        assert_eq!(
            (verdict.verdict, verdict.repair),
            (Verdict::Sufficient, RepairStep::Stop)
        );

        // A caller cannot raise the round limit above the knob.
        let capped = run(3, &request(Some(part(Some(99))))).expect("round runs");
        assert_eq!(capped.sufficiency.expect("verdict").round_limit, 4);

        // No sufficiency part: rows only.
        let bare = run(3, &request(None)).expect("round runs");
        assert_eq!(bare.rows.len(), 3);
        assert!(bare.sufficiency.is_none());

        // A missing round limit and a malformed part refuse with typed codes.
        match run(3, &request(Some(part(None)))) {
            Err(RoundError::Refusal(r)) => assert_eq!(r.code, RETRIEVAL_ROUND_LIMIT_REQUIRED),
            other => panic!("expected a refusal, got {other:?}"),
        }
        let mut zero = part(Some(4));
        zero.round = 0;
        match run(3, &request(Some(zero))) {
            Err(RoundError::Refusal(r)) => assert_eq!(r.code, RETRIEVAL_CONTRACT_INVALID),
            other => panic!("expected a refusal, got {other:?}"),
        }
        let mut blank = request(None);
        blank.model_revision = " ".into();
        assert!(matches!(run(1, &blank), Err(RoundError::Refusal(_))));
    }

    fn selection_request() -> RoundRequest {
        use medulla_result_types::retrieval_verdict::{ScoreNormalization, SelectionPolicy};
        let mut request = request(None);
        request.selection = Some(SelectionPart {
            query: "alpha".into(),
            policy: SelectionPolicy {
                policy_revision: "p1".into(),
                corpus_revision: "corpus-1".into(),
                embedding_model_revision: "model-1".into(),
                cross_encoder_model_revision: "builtin:lexical-overlap@1".into(),
                normalization: ScoreNormalization::Identity,
                embedding_threshold: 0.5,
                relevance_threshold: 0.5,
                fusion_weight: 0.5,
                fusion_threshold: 0.5,
                max_chunks: 3,
                activation_benchmark_digest: Some("b1".into()),
            },
            benchmark: Some(ActivationBenchmark {
                digest: "b1".into(),
                baseline: BenchmarkResult {
                    ndcg_at_10: 0.8,
                    mean_selected: 4.0,
                },
                candidate: BenchmarkResult {
                    ndcg_at_10: 0.8,
                    mean_selected: 3.0,
                },
            }),
            mode: SelectionMode::DualScore,
        });
        request
    }

    #[test]
    fn selection_refuses_without_a_reranker_or_an_embedding_score() {
        use medulla_exec::exec::LexicalOverlapReranker;
        let core = GraphCore::new();
        core.add_node("d0".into(), blob("Doc"));
        let view = core.analysis_snapshot();
        let semantic = SemanticStore::new();
        let code = |result: Result<RoundResult, RoundError>| match result {
            Err(RoundError::Refusal(r)) => r.code,
            other => panic!("expected a refusal, got {other:?}"),
        };

        // No reranker bound: the selection part refuses.
        let bare = PlanCtx::new(&view, &semantic);
        assert_eq!(
            code(run_round(&selection_request(), &bare, &KNOBS)),
            RETRIEVAL_SCORE_PAIR_REQUIRED
        );

        // A bound reranker still needs an embedding score on every executed row: a plain
        // scan carries none.
        let reranker = LexicalOverlapReranker;
        let bound = PlanCtx::new(&view, &semantic).with_reranker(&reranker);
        assert_eq!(
            code(run_round(&selection_request(), &bound, &KNOBS)),
            RETRIEVAL_SCORE_PAIR_REQUIRED
        );
    }

    #[test]
    fn lexical_overlap_reranker_scores_the_share_of_query_tokens() {
        use medulla_exec::exec::{LexicalOverlapReranker, Reranker};
        let scores = LexicalOverlapReranker
            .score("alpha beta", &["Alpha, BETA!", "alpha only", "nothing", ""]);
        assert_eq!(scores, [1.0, 0.5, 0.0, 0.0]);
        assert_eq!(LexicalOverlapReranker.score("", &["x"]), [0.0]);
        assert_eq!(
            LexicalOverlapReranker.revision(),
            "builtin:lexical-overlap@1"
        );
    }
}
