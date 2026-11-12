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
    ContextPackageBudget, ContextPackageCandidate, ContextPackageRequest, ContextPackageSelection,
    MAX_CONTEXT_PACKAGE_CANDIDATES,
};
use medulla_types::contract::{BoundedVec, Digest256};
use serde::{Deserialize, Serialize};

use crate::rerank::{calibrate, CalibrationCurve, FallbackReason};

/// Default `top_n`: candidates kept per source before fusion.
pub const DEFAULT_TOP_N: u32 = 256;
/// Largest accepted `top_n`.
pub const MAX_TOP_N: u32 = 1_024;

fn default_top_n() -> u32 {
    DEFAULT_TOP_N
}

/// Which retrieval arms feed the bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ContextMode {
    /// Lexical, semantic and graph arms fused.
    #[default]
    Hybrid,
    /// The lexical arm only.
    Lexical,
    /// The semantic arm only.
    Semantic,
}

/// A served `ContextCompile` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextBundleRequest {
    pub query_text: String,
    #[serde(default)]
    pub query_embedding: Vec<f32>,
    pub budget: ContextPackageBudget,
    pub tokenizer_id: String,
    pub tokenizer_version: String,
    #[serde(default)]
    pub mode: ContextMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_ref: Option<String>,
    #[serde(default = "default_top_n")]
    pub top_n: u32,
}

impl ContextBundleRequest {
    /// `top_n` must stay inside [1, 1,024].
    pub fn top_n_in_range(&self) -> bool {
        (1..=MAX_TOP_N).contains(&self.top_n)
    }
}

/// Where a cited span came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Citation {
    pub source_id: String,
    pub cited_start: u64,
    pub cited_end: u64,
    pub source_revision: u64,
    pub content_digest: String,
}

/// One proof edge from an item to the evidence it rests on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProofEdge {
    pub from: String,
    pub relationship: String,
    pub to: String,
}

/// One item of a bundle at the resolution the budget allowed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextItem {
    pub item_id: String,
    pub resolution: u8,
    pub tokens: u64,
    pub value_micro: u64,
    pub citation: Citation,
    pub proof_edges: Vec<ProofEdge>,
}

/// Why an item is not in the bundle. Closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionReason {
    BelowGap,
    OverBudget,
    Denied,
    OutOfScope,
    Dominated,
}

/// An item left out, with its reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExcludedItem {
    pub item_id: String,
    pub reason: ExclusionReason,
}

/// The stable cache key of a bundle: SHA-256 hex of tenant, request, graph
/// revision and policy digest.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BundleKey(pub String);

/// Why a record was dropped by the access check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DroppedReason {
    Denied,
    Timeout,
    Unavailable,
}

/// A record the access check dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DroppedRecord {
    pub record_id: String,
    pub reason: DroppedReason,
}

/// One raw and calibrated input score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputScore {
    pub source: String,
    pub node_id: String,
    pub raw: f32,
    pub calibrated: f32,
}

/// How one source was normalised.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceNormalization {
    pub source: String,
    /// The pinned curve digest, or `None` for min-max scaling.
    pub curve_digest: Option<String>,
    pub fallback: Option<FallbackReason>,
}

/// The inputs the key hashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyInputs {
    pub tenant: String,
    pub graph_revision: u64,
    pub policy_digest: String,
}

/// Inline evidence of one compile. Nothing is stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleEvidence {
    pub input_scores: Vec<InputScore>,
    pub normalization: Vec<SourceNormalization>,
    pub rrf_k: u32,
    pub selected: Vec<String>,
    pub excluded: Vec<ExcludedItem>,
    pub resolutions: BTreeMap<String, u8>,
    pub budget_used_tokens: u64,
    pub certificate_bound_tokens: u64,
    pub key_inputs: KeyInputs,
    pub unchecked_dropped: Vec<DroppedRecord>,
}

/// The compiled bundle. `certificate` is `None` only when no candidate
/// survived, so there was nothing to solve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextBundle {
    pub items: Vec<ContextItem>,
    pub excluded: Vec<ExcludedItem>,
    pub certificate: Option<ContextPackageSelection>,
    pub key: BundleKey,
    pub evidence: BundleEvidence,
}

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
}

/// The cache key of a request under its inputs.
pub fn bundle_key(request: &ContextBundleRequest, inputs: &CompileInputs) -> BundleKey {
    let request_json = serde_json::to_vec(request).unwrap_or_default();
    let digest = Digest256::framed(
        b"medulla/context-bundle-key/v1",
        &[
            inputs.tenant.as_bytes(),
            &request_json,
            &inputs.graph_revision.to_be_bytes(),
            inputs.policy_digest.as_bytes(),
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

/// Compile one bundle. Deterministic: equal inputs give equal bundles.
pub fn compile(
    request: &ContextBundleRequest,
    inputs: &CompileInputs,
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
    let fused = medulla_text::rrf_fuse(&refs, knobs.rrf_k as f32);
    let keep = cut_at_gap(&fused, knobs.min_items as usize);

    let mut excluded: Vec<ExcludedItem> = inputs.pre_excluded.clone();
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
