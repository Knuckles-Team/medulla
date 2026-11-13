//! Context bundle compilation under a certified token budget
//! (MEDULLA-FEDERATED-QUERY-R650.1, MEDULLA-FEDERATED-QUERY-R650.2).
//!
//! [`compile`] runs the fixed back half of the retrieval pipeline over ranked
//! hits that the served handler has already scoped, reranked and access
//! checked: calibrate each source, fuse by reciprocal rank, cut the tail at the
//! largest score gap, then select the resolution of every kept item with the
//! one exact package solver of `MEDULLA-DECISION-ENGINE-R137.8`. Nothing here
//! writes; every excluded item carries a closed reason.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_policy::solve::context_package::{select, PackageRefusal};
use medulla_decision_types::decision::context_package::{
    ContextPackageCandidate, ContextPackageRequest, MAX_CONTEXT_PACKAGE_CANDIDATES,
};
use medulla_types::contract::{BoundedVec, Digest256};

use crate::rerank::{
    calibrate, rerank_with, CalibrationCurve, LexicalDoc, ModelBinding, RerankReceipt,
};
pub use medulla_decision_types::decision::context_bundle::*;

/// The registered knobs the compile reads (`context.*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextKnobs {
    /// `knob context.rrf_k`, default 60, [1, 1,000].
    pub rrf_k: u32,
    /// `knob context.min_items`, default 3, [1, 64].
    pub min_items: u32,
    /// `knob context.solver_node_budget`, default 100,000.
    pub solver_node_budget: u32,
    /// `knob context.proof_edges_per_item`, default 3, [0, 16].
    pub proof_edges_per_item: u32,
    /// `knob context.value_floor_micro_per_token`, default 0.
    pub value_floor_micro_per_token: u64,
}

impl Default for ContextKnobs {
    fn default() -> Self {
        Self {
            rrf_k: 60,
            min_items: 3,
            solver_node_budget: 100_000,
            proof_edges_per_item: 3,
            value_floor_micro_per_token: 0,
        }
    }
}

/// One measured resolution of a hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolutionSpan {
    pub resolution: u8,
    pub tokens: u64,
    pub citation: Citation,
}

/// One scored hit of one source.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceHit {
    pub node_id: String,
    pub score: f32,
    pub spans: Vec<ResolutionSpan>,
    pub hop_path: Vec<ProofEdge>,
    pub derived_from: Vec<ProofEdge>,
}

/// The hits of one source, with its pinned calibration curve when it has one.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceRanking {
    pub source: String,
    pub curve: Option<CalibrationCurve>,
    pub hits: Vec<SourceHit>,
}

/// The text a candidate is scored on by the rerank stage.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RerankDoc {
    pub name: String,
    pub description: String,
    pub text: String,
}

/// What the rerank stage needs: the query, the candidates' text and the
/// pinned curve. `rerank_k` is `knob retrieval.rerank_k`.
#[derive(Debug, Clone, PartialEq)]
pub struct RerankInputs {
    pub query: String,
    pub rerank_k: usize,
    pub curve: Option<CalibrationCurve>,
    pub docs: BTreeMap<String, RerankDoc>,
}

/// What the handler hands `compile` after scope, rerank and access check.
#[derive(Debug, Clone, PartialEq)]
pub struct CompileInputs {
    pub tenant: String,
    pub graph_revision: u64,
    pub policy_digest: String,
    pub rankings: Vec<SourceRanking>,
    /// Items an earlier stage already left out (`denied`, `out_of_scope`).
    pub pre_excluded: Vec<ExcludedItem>,
    pub unchecked_dropped: Vec<DroppedRecord>,
    pub knobs: ContextKnobs,
    /// The rerank stage; `None` skips it (no query text to score against).
    pub rerank: Option<RerankInputs>,
}

/// The cache key of a request under its inputs.
pub fn bundle_key(request: &ContextBundleRequest, inputs: &CompileInputs) -> BundleKey {
    bundle_key_of(
        request,
        &inputs.tenant,
        inputs.graph_revision,
        &inputs.policy_digest,
    )
}

/// The cache key from its four inputs, so a handler can look the bundle up
/// before it gathers any hit.
pub fn bundle_key_of(
    request: &ContextBundleRequest,
    tenant: &str,
    graph_revision: u64,
    policy_digest: &str,
) -> BundleKey {
    let request_json = serde_json::to_vec(request).unwrap_or_default();
    let digest = Digest256::framed(
        b"medulla/context-bundle-key/v1",
        &[
            tenant.as_bytes(),
            &request_json,
            &graph_revision.to_be_bytes(),
            policy_digest.as_bytes(),
        ],
    )
    .expect("fixed domain and field count frame");
    BundleKey(digest.to_hex())
}

/// Calibrate and order one source; at most `top_n` ids, best first.
fn rank_source(
    ranking: &SourceRanking,
    top_n: usize,
    evidence: &mut BundleEvidence,
) -> Vec<String> {
    let raw: Vec<f32> = ranking.hits.iter().map(|h| h.score).collect();
    let (calibrated, fallback) = calibrate(&raw, ranking.curve.as_ref());
    evidence.normalization.push(SourceNormalization {
        source: ranking.source.clone(),
        curve_digest: ranking.curve.as_ref().map(|c| c.digest.clone()),
        fallback,
    });
    let mut order: Vec<usize> = (0..ranking.hits.len()).collect();
    order.sort_by(|&a, &b| {
        calibrated[b]
            .partial_cmp(&calibrated[a])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| ranking.hits[a].node_id.cmp(&ranking.hits[b].node_id))
    });
    for &i in &order {
        evidence.input_scores.push(InputScore {
            source: ranking.source.clone(),
            node_id: ranking.hits[i].node_id.clone(),
            raw: raw[i],
            calibrated: calibrated[i],
        });
    }
    order
        .into_iter()
        .take(top_n)
        .map(|i| ranking.hits[i].node_id.clone())
        .collect()
}

/// Keep the head of `fused` up to the largest gap between adjacent scores,
/// but never fewer than `min_items`.
fn cut_at_gap(fused: &[(String, f32)], min_items: usize) -> usize {
    if fused.len() <= min_items {
        return fused.len();
    }
    let mut keep = fused.len();
    let mut widest = f32::NEG_INFINITY;
    for i in min_items.saturating_sub(1)..fused.len() - 1 {
        let gap = fused[i].1 - fused[i + 1].1;
        if gap > widest {
            widest = gap;
            keep = i + 1;
        }
    }
    keep
}

/// Rerank the head of the fused list: the first `rerank_k` candidates are
/// scored, calibrated and reordered; the rest are returned as not scored.
fn rerank_fused(
    fused: Vec<(String, f32)>,
    rerank: &RerankInputs,
    model: ModelBinding<'_>,
) -> (Vec<(String, f32)>, Vec<String>, RerankReceipt) {
    let empty = RerankDoc::default();
    let head = fused.len().min(rerank.rerank_k);
    let docs: Vec<LexicalDoc<'_>> = fused[..head]
        .iter()
        .map(|(id, _)| {
            let doc = rerank.docs.get(id).unwrap_or(&empty);
            LexicalDoc {
                node_id: id,
                name: &doc.name,
                description: &doc.description,
                text: &doc.text,
            }
        })
        .collect();
    let run = rerank_with(
        &rerank.query,
        &docs,
        rerank.rerank_k,
        rerank.curve.as_ref(),
        model,
    );
    let reordered = run
        .hits
        .iter()
        .map(|h| (h.node_id.clone(), h.calibrated))
        .collect();
    let unscored = fused[head..].iter().map(|(id, _)| id.clone()).collect();
    (reordered, unscored, run.receipt)
}

/// Compile one bundle with the lexical scorer. Deterministic: equal inputs
/// give equal bundles.
pub fn compile(
    request: &ContextBundleRequest,
    inputs: &CompileInputs,
) -> Result<ContextBundle, PackageRefusal> {
    compile_with(request, inputs, ModelBinding::Unset)
}

/// [`compile`] with the model binding the server holds for the rerank stage
/// (MEDULLA-FEDERATED-QUERY-R652.7).
pub fn compile_with(
    request: &ContextBundleRequest,
    inputs: &CompileInputs,
    model: ModelBinding<'_>,
) -> Result<ContextBundle, PackageRefusal> {
    let knobs = inputs.knobs;
    let mut evidence = BundleEvidence {
        input_scores: Vec::new(),
        normalization: Vec::new(),
        rrf_k: knobs.rrf_k,
        selected: Vec::new(),
        excluded: inputs.pre_excluded.clone(),
        resolutions: BTreeMap::new(),
        budget_used_tokens: 0,
        certificate_bound_tokens: 0,
        key_inputs: KeyInputs {
            tenant: inputs.tenant.clone(),
            graph_revision: inputs.graph_revision,
            policy_digest: inputs.policy_digest.clone(),
        },
        unchecked_dropped: inputs.unchecked_dropped.clone(),
        rerank: None,
        stages: Vec::new(),
    };
    let key = bundle_key(request, inputs);

    // Sources fuse in name order so the result never depends on arrival order.
    let mut rankings: Vec<&SourceRanking> = inputs.rankings.iter().collect();
    rankings.sort_by(|a, b| a.source.cmp(&b.source));
    let top_n = request.top_n.clamp(1, MAX_TOP_N) as usize;
    let mut lists: Vec<Vec<String>> = Vec::new();
    let mut hits: BTreeMap<&str, &SourceHit> = BTreeMap::new();
    for ranking in &rankings {
        lists.push(rank_source(ranking, top_n, &mut evidence));
        for hit in &ranking.hits {
            hits.entry(hit.node_id.as_str()).or_insert(hit);
        }
    }
    let refs: Vec<&[String]> = lists.iter().map(Vec::as_slice).collect();
    let mut fused = medulla_text::rrf_fuse(&refs, knobs.rrf_k as f32);
    evidence.stages.push(PipelineStage::Fuse);
    // Fuse, then rerank the head, then cut (MEDULLA-FEDERATED-QUERY-R650,
    // MEDULLA-FEDERATED-QUERY-R652.7).
    let mut unscored: Vec<String> = Vec::new();
    if let Some(rerank) = &inputs.rerank {
        let (reordered, rest, receipt) = rerank_fused(fused, rerank, model);
        fused = reordered;
        unscored = rest;
        // The scorer ran, then its raw scores were calibrated (rerank_with).
        evidence.stages.push(PipelineStage::Rerank);
        evidence.stages.push(PipelineStage::Calibrate);
        evidence.rerank = Some(RerankEvidence {
            scorer_name: receipt.scorer.name,
            scorer_version: receipt.scorer.version,
            calibration_digest: receipt.calibration_digest,
            input_ranks: receipt.input_ranks,
            output_ranks: receipt.output_ranks,
            fallback: receipt.fallback,
        });
    }
    let keep = cut_at_gap(&fused, knobs.min_items as usize);
    evidence.stages.push(PipelineStage::Cut);

    let mut excluded: Vec<ExcludedItem> = inputs.pre_excluded.clone();
    for node in unscored {
        excluded.push(ExcludedItem {
            item_id: node,
            reason: ExclusionReason::BelowGap,
        });
    }
    for (node, _) in &fused[keep..] {
        excluded.push(ExcludedItem {
            item_id: node.clone(),
            reason: ExclusionReason::BelowGap,
        });
    }

    // Candidates: every measured resolution of every kept hit, up to the
    // solver's candidate bound; a hit that no longer fits is over budget.
    let mut candidates: Vec<ContextPackageCandidate> = Vec::new();
    let mut kept: Vec<(&str, &SourceHit)> = Vec::new();
    for (node, score) in &fused[..keep] {
        let hit = hits[node.as_str()];
        if hit.spans.is_empty() {
            excluded.push(ExcludedItem {
                item_id: node.clone(),
                reason: ExclusionReason::OutOfScope,
            });
            continue;
        }
        if candidates.len() + hit.spans.len() > MAX_CONTEXT_PACKAGE_CANDIDATES {
            excluded.push(ExcludedItem {
                item_id: node.clone(),
                reason: ExclusionReason::OverBudget,
            });
            continue;
        }
        let fused_micro = (f64::from(*score) * 1_000_000.0).round() as u64;
        let widest = hit.spans.iter().map(|s| s.tokens).max().unwrap_or(1).max(1);
        for span in &hit.spans {
            // The widest resolution carries the full fused value; a narrower
            // one carries its share of it, never less than one micro unit.
            let share = u128::from(fused_micro) * u128::from(span.tokens) / u128::from(widest);
            candidates.push(ContextPackageCandidate {
                candidate_id: format!("{node}@{}", span.resolution),
                evidence_unit: node.clone(),
                resolution: span.resolution,
                source_id: span.citation.source_id.clone(),
                cited_start: span.citation.cited_start,
                cited_end: span.citation.cited_end,
                source_revision: span.citation.source_revision,
                content_digest: span.citation.content_digest.clone(),
                tokenizer_id: request.tokenizer_id.clone(),
                tokenizer_version: request.tokenizer_version.clone(),
                token_weight: span.tokens,
                value_micro: u64::try_from(share).unwrap_or(u64::MAX).max(1),
                measurement_provenance: "bundle_compile".to_string(),
            });
        }
        kept.push((node.as_str(), hit));
    }

    if candidates.is_empty() {
        evidence.excluded = excluded.clone();
        evidence.certificate_bound_tokens = 0;
        return Ok(ContextBundle {
            items: Vec::new(),
            excluded,
            certificate: None,
            key,
            evidence,
        });
    }

    let package = ContextPackageRequest {
        budget: request.budget.clone(),
        candidates: BoundedVec::new(candidates.clone()).map_err(|_| PackageRefusal {
            code: medulla_decision_types::decision::context_package::ContextPackageErrorCode::InvalidMeasurement,
            detail: "candidate list exceeds the solver bound".to_string(),
        })?,
        value_floor_micro_per_token: knobs.value_floor_micro_per_token,
        node_budget: knobs.solver_node_budget,
    };
    let solved = select(&package)?;
    evidence.stages.push(PipelineStage::Select);
    let chosen: BTreeSet<&str> = solved
        .selection
        .chosen_ids
        .iter()
        .map(String::as_str)
        .collect();

    let mut items = Vec::new();
    let mut chosen_digests: BTreeSet<&str> = BTreeSet::new();
    let mut chosen_units: BTreeSet<&str> = BTreeSet::new();
    for candidate in &candidates {
        if chosen.contains(candidate.candidate_id.as_str()) {
            chosen_digests.insert(candidate.content_digest.as_str());
            chosen_units.insert(candidate.evidence_unit.as_str());
        }
    }
    for (node, hit) in &kept {
        if chosen_units.contains(node) {
            let candidate = candidates
                .iter()
                .find(|c| c.evidence_unit == *node && chosen.contains(c.candidate_id.as_str()))
                .expect("a chosen unit has a chosen candidate");
            let span = hit
                .spans
                .iter()
                .find(|s| s.resolution == candidate.resolution)
                .expect("candidate came from this span");
            let edges = if hit.hop_path.is_empty() {
                &hit.derived_from
            } else {
                &hit.hop_path
            };
            items.push(ContextItem {
                item_id: (*node).to_string(),
                resolution: candidate.resolution,
                tokens: candidate.token_weight,
                value_micro: candidate.value_micro,
                citation: span.citation.clone(),
                proof_edges: edges
                    .iter()
                    .take(knobs.proof_edges_per_item as usize)
                    .cloned()
                    .collect(),
            });
            evidence.selected.push((*node).to_string());
            evidence
                .resolutions
                .insert((*node).to_string(), candidate.resolution);
        } else {
            let duplicate = hit
                .spans
                .iter()
                .all(|s| chosen_digests.contains(s.citation.content_digest.as_str()));
            excluded.push(ExcludedItem {
                item_id: (*node).to_string(),
                reason: if duplicate {
                    ExclusionReason::Dominated
                } else {
                    ExclusionReason::OverBudget
                },
            });
        }
    }
    evidence.excluded = excluded.clone();
    evidence.budget_used_tokens = solved.selection.total_tokens;
    evidence.certificate_bound_tokens = solved.selection.effective_capacity_tokens;
    Ok(ContextBundle {
        items,
        excluded,
        certificate: Some(solved.selection),
        key,
        evidence,
    })
}

#[cfg(test)]
mod tests;
