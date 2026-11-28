//! The `resolve_entity` question family (MEDULLA-DECISION-ENGINE-R001.101).
//!
//! Ranks through the shared matrix ladder. The Record stage adds identity
//! evidence (MEDULLA-UNIFIED-DATA-PLANE-R030.701): each declared pair option
//! contributes, between its two sides, one premise `identity:<kind>` per piece
//! of evidence and one `identity:decision:<merge_decision>` premise. The
//! per-side inputs are the option texts `entity.left.<field>` and
//! `entity.right.<field>` (`name`, `key`); an embedding cosine of a pair is the
//! request param `embedding:<option id>`. Evidence thresholds are the
//! registry knobs `entity_link.entropy_bits` and `entity_link.embedding_min`.
//! The merge decision is always a proposal. A request that declares an
//! approval (`approve_link`) without a named `approver` refuses with
//! `LINK_APPROVER_MISSING`.
//!
//! Candidate stage (MEDULLA-DECISION-ENGINE-R706.3): a question that names a
//! graph in the param `er.graph` also weighs the nearest neighbours of each
//! declared pair side. [`with_candidates`] reads that graph as the caller may
//! see it, indexes the embedded nodes the engine's row-visibility check lets
//! the caller see (never a caller-writable node property), asks the index for `er.candidate_k` neighbours with
//! the tenant predicate inside the search, and adds every unseen pair as one
//! more declared option `er:embedding:<left>|<right>`. The same prelude reads
//! the reviews the decision log holds for proposal options, records the
//! per-tier accept and reject counts and the drift of the accept rate in
//! engine params the caller may not supply, and feeds the `head_drift` gauge.
//!
//! The served pair decision (MEDULLA-DECISION-ENGINE-R032.2.2): every option
//! is one entity pair, its two sides the texts `entity.left` and
//! `entity.right`, each an IRI (`ENTITY_PAIR_INVALID` when one is missing or
//! is not an IRI, `ENTITY_PAIR_SELF` when both sides are equal). The decision
//! acts on a pair only when the pinned head's conformal prediction set is
//! exactly that one pair and the head was calibrated at a miscoverage no
//! looser than the decision-policy knob `resolve_entity.alpha`; otherwise it
//! abstains with `AbstainReason::InsufficientConfidence`. The answer proposes
//! a link and never writes `owl:sameAs`.

use std::sync::Arc;

use medulla_decision_program_types::decision::knob_families::resolve_entity as resolve_entity_knobs;
use medulla_decision_statistical_types::decision::statistical::question_kind::QuestionKind;
use medulla_decision_stats_records::decision::adapter::to_q16;
use medulla_decision_stats_records::decision::candidate::CandidateView;
use medulla_decision_types::decision::statistical::StatisticalOutcome;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, TypedParam, TypedValue,
};
use medulla_index_types::entity_resolution::{
    decide_merge, identity_evidence, ApprovalRefusal, IdentityEvidence, MergeDecision,
};
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::contract::BoundedVec;
use tokio::sync::RwLock;
use {
    medulla_decision_statistical_types::decision::codes::ER_BELOW_THRESHOLD,
    medulla_decision_statistical_types::decision::codes::ER_CROSS_TENANT,
    medulla_decision_statistical_types::decision::codes::ER_DRIFT_HOLD,
};
use {
    medulla_decision_statistical_types::decision::entity_resolution::ResolutionTier,
    medulla_decision_statistical_types::decision::entity_resolution::ReviewOutcome,
};
use {
    medulla_decision_statistical_types::decision::record::premise::PremiseClass,
    medulla_decision_statistical_types::decision::record::premise::PremiseProvenance,
    medulla_decision_statistical_types::decision::record::premise::PremiseRef,
    medulla_decision_statistical_types::decision::DecisionPolicy,
    medulla_decision_types::decision::record::AbstainReason,
};
use {
    medulla_decision_statistical_types::decision::statistical::declared::DeclaredNumber,
    medulla_decision_statistical_types::decision::statistical::declared::DeclaredOption,
    medulla_decision_statistical_types::decision::statistical::declared::DeclaredText,
    medulla_decision_statistical_types::decision::statistical::declared::MAX_DECLARED_OPTIONS,
};
use {
    medulla_decision_stats_records::decision::entity_resolution::accept_rate_drift,
    medulla_decision_stats_records::decision::entity_resolution::proposal_option_id,
    medulla_decision_stats_records::decision::entity_resolution::tier_evaluation,
    medulla_decision_stats_records::decision::entity_resolution::tier_of_option,
    medulla_decision_stats_records::decision::entity_resolution::tier_token,
    medulla_decision_stats_records::decision::entity_resolution::AcceptDrift,
    medulla_decision_stats_records::decision::entity_resolution::CandidateIndex,
    medulla_decision_stats_records::decision::entity_resolution::IndexedNode,
};

use super::support::{invalid, text_param};
use super::DecisionFamily;
use crate::decide::stat_executor::{Executed, Pinned};
use crate::decide::stat_log::{recent_labels_where, LogReader, MAX_LOG_ROWS};
use crate::decide::stat_vectors::graph_vectors;

/// Most identity premises one decision records.
const MAX_IDENTITY_PREMISES: usize = 512;
/// Fixed-point scale of an embedding cosine param: Q32, as the declared numbers.
const Q32: f64 = 4_294_967_296.0;

/// Option text naming the left side of a pair.
pub const LEFT_KEY: &str = "entity.left";
/// Option text naming the right side of a pair.
pub const RIGHT_KEY: &str = "entity.right";

/// Whether `value` is an absolute IRI: a scheme, a colon, a non-empty
/// remainder, and no whitespace or angle brackets.
fn is_iri(value: &str) -> bool {
    let Some((scheme, rest)) = value.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !rest.is_empty()
        && !value
            .chars()
            .any(|c| c.is_whitespace() || c == '<' || c == '>')
}

/// The param naming the graph whose embedded nodes are the candidate corpus.
pub const PARAM_GRAPH: &str = "er.graph";
/// Option text the prelude records on a proposal: the embedding space the
/// pair was proposed in, `<id>@<revision>` or `unspecified`.
const TEXT_MODEL: &str = "er.model";
/// Option text the prelude records on a proposal: SHA-256 of the two embedded
/// texts' hashes, 64 lowercase hex characters.
const TEXT_CONTENT_HASH: &str = "er.content_hash";
/// The model of a store that declares no embedding space.
const MODEL_UNSPECIFIED: &str = "unspecified";
/// Prefix of the engine params the prelude records; a caller may not supply one.
const ENGINE_PARAM_PREFIX: &str = "er.review.";
/// Accepted reviews of a tier (suffix `:<tier>`).
const PARAM_ACCEPTED: &str = "er.review.accepted";
/// Rejected reviews of a tier (suffix `:<tier>`).
const PARAM_REJECTED: &str = "er.review.rejected";
/// The evaluation id of the newest review counted.
const PARAM_LATEST: &str = "er.review.latest";
/// The drift state of the accept rate: `stable` or `drifted`.
const PARAM_DRIFT: &str = "er.review.drift";
/// The absolute accept-rate change between the two windows, in ppm.
const PARAM_DRIFT_CHANGE: &str = "er.review.drift_change_ppm";

const TIERS: [ResolutionTier; 3] = [
    ResolutionTier::Exact,
    ResolutionTier::Embedding,
    ResolutionTier::Probabilistic,
];

/// The family of the `resolve_entity` question kind.
pub struct ResolveEntityFamily;

fn text<'a>(option: &'a DeclaredOption, key: &str) -> Option<&'a str> {
    option
        .texts
        .iter()
        .find(|entry| entry.key == key)
        .map(|entry| entry.text.as_str())
}

fn param<'a>(request: &'a DecideRequest, name: &str) -> Option<&'a TypedParam> {
    request.params.iter().find(|param| param.name == name)
}

/// One per-side input of a pair option: the text `entity.<side>.<field>`.
fn side_text<'a>(option: &'a DeclaredOption, side: &str, field: &str) -> Option<&'a str> {
    let key = format!("{side}.{field}");
    text(option, &key)
}

/// The embedding cosine of a pair option: the request param `embedding:<option id>`.
fn cosine(request: &DecideRequest, option_id: &str) -> Option<f64> {
    let named = param(request, &format!("embedding:{option_id}"))?;
    match &named.value {
        TypedValue::Int(q32) => Some(*q32 as f64 / Q32),
        _ => None,
    }
}

/// The identity evidence between the two sides of one pair option.
fn evidence_of(
    request: &DecideRequest,
    option: &DeclaredOption,
    entropy_bits: f64,
) -> Vec<IdentityEvidence> {
    let mut evidence = identity_evidence(
        side_text(option, LEFT_KEY, "name").unwrap_or_default(),
        side_text(option, RIGHT_KEY, "name").unwrap_or_default(),
        side_text(option, LEFT_KEY, "key"),
        side_text(option, RIGHT_KEY, "key"),
        entropy_bits,
    );
    if let Some(cosine) = cosine(request, &option.option_id) {
        evidence.push(IdentityEvidence::Embedding { cosine });
    }
    evidence
}

fn decision_fact(decision: MergeDecision) -> &'static str {
    match decision {
        MergeDecision::ProposeMerge => "identity:decision:propose_merge",
        MergeDecision::ProposeLink => "identity:decision:propose_link",
        MergeDecision::NoMerge => "identity:decision:no_merge",
    }
}

/// The identity premises of every declared pair option, computed between its
/// two sides.
pub fn identity_premises(request: &DecideRequest) -> Vec<PremiseRef> {
    let CandidateSource::Declared { options } = &request.candidates else {
        return Vec::new();
    };
    let knobs = medulla_server_api::knob_config::knobs();
    let mut out = Vec::new();
    let provenance = || PremiseProvenance::Request {
        field: "candidates.options".to_string(),
    };
    for option in options.iter() {
        let evidence = evidence_of(request, option, knobs.entity_link_entropy_bits);
        let decision = decide_merge(&evidence, knobs.entity_link_embedding_min);
        for piece in &evidence {
            out.push(PremiseRef {
                subject: option.option_id.clone(),
                fact: piece.premise_fact().to_string(),
                class: PremiseClass::Claim,
                provenance: provenance(),
                trust_tier: None,
            });
        }
        out.push(PremiseRef {
            subject: option.option_id.clone(),
            fact: decision_fact(decision).to_string(),
            class: PremiseClass::Claim,
            provenance: provenance(),
            trust_tier: None,
        });
        if out.len() >= MAX_IDENTITY_PREMISES {
            out.truncate(MAX_IDENTITY_PREMISES);
            return out;
        }
    }
    out
}

/// The `(left, right)` IRIs of one pair option.
fn pair_of(option: &DeclaredOption) -> Option<(&str, &str)> {
    Some((text(option, LEFT_KEY)?, text(option, RIGHT_KEY)?))
}

/// Whether the head's calibration was made at a miscoverage no looser than
/// `alpha_micros` (a fraction times 1,000,000).
fn calibrated_within(pinned: &Pinned, alpha_micros: i64) -> bool {
    pinned
        .head
        .as_ref()
        .and_then(|(head, _)| head.calibration.as_ref())
        .is_some_and(|calibration| {
            // numerator / denominator <= alpha_micros / 1e6, in integers.
            u128::from(calibration.alpha.numerator()) * 1_000_000
                <= u128::from(alpha_micros.max(0) as u64)
                    * u128::from(calibration.alpha.denominator())
        })
}

fn tier_param(base: &str, tier: ResolutionTier) -> String {
    format!("{base}:{}", tier_token(tier))
}

fn count_param(name: String, count: u32) -> TypedParam {
    TypedParam {
        name,
        value: TypedValue::Int(i64::from(count)),
    }
}

/// The review premises of the engine params the prelude recorded: one per tier
/// with reviews (`er:tier_evaluation:<tier>=<accepted>/<rejected>/<rate_ppm>`)
/// and the drift state.
fn review_premises(request: &DecideRequest) -> Vec<PremiseRef> {
    let count = |name: &str| match param(request, name).map(|p| &p.value) {
        Some(TypedValue::Int(n)) => u32::try_from(*n).ok(),
        _ => None,
    };
    let Some(TypedValue::Text(latest)) = param(request, PARAM_LATEST).map(|p| &p.value) else {
        return Vec::new();
    };
    let observed = |fact: String| PremiseRef {
        subject: "reviews".to_string(),
        fact,
        class: PremiseClass::Observation,
        provenance: PremiseProvenance::Observation {
            evaluation_id: latest.clone(),
        },
        trust_tier: None,
    };
    let mut out = Vec::new();
    for tier in TIERS {
        let (Some(accepted), Some(rejected)) = (
            count(&tier_param(PARAM_ACCEPTED, tier)),
            count(&tier_param(PARAM_REJECTED, tier)),
        ) else {
            continue;
        };
        let evaluation =
            medulla_decision_statistical_types::decision::entity_resolution::TierEvaluation::new(
                tier, accepted, rejected,
            );
        out.push(observed(format!(
            "er:tier_evaluation:{}={accepted}/{rejected}/{}",
            tier_token(tier),
            evaluation.accept_rate_ppm
        )));
    }
    if let (Some(TypedValue::Text(state)), Some(change)) = (
        param(request, PARAM_DRIFT).map(|p| &p.value),
        count(PARAM_DRIFT_CHANGE),
    ) {
        out.push(observed(format!("er:drift:{state}={change}")));
    }
    out
}

/// The pair `(left, right)` in canonical order.
fn ordered<'a>(a: &'a str, b: &'a str) -> (&'a str, &'a str) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// The anchor's declared numbers weighed by the similarity of the neighbour to
/// the anchor's side: the cosine `1 - distance`, floored at zero. A neighbour
/// pair therefore never outranks the pair it grew from on declared evidence.
fn discounted<const N: usize>(
    numbers: &BoundedVec<DeclaredNumber, N>,
    distance: f32,
) -> Result<BoundedVec<DeclaredNumber, N>, String> {
    let similarity = f64::from(1.0 - distance).clamp(0.0, 1.0);
    BoundedVec::new(
        numbers
            .iter()
            .map(|n| DeclaredNumber {
                key: n.key.clone(),
                q32: (n.q32 as f64 * similarity).round() as i64,
            })
            .collect(),
    )
}

/// The neighbour pairs of the declared pair sides, as options.
fn neighbour_options(
    declared: &[DeclaredOption],
    view: &crate::decide::stat_vectors::GraphVectors,
    tenant: &str,
    candidate_k: usize,
    max_nodes: usize,
) -> Result<Vec<DeclaredOption>, String> {
    let mut nodes = Vec::new();
    for id in view.node_ids() {
        // `node_ids` and `stored_vector` answer only for nodes the engine's one
        // row-visibility check lets the caller see; no node property decides
        // whose node it is, so a forged `tenant_id` cannot admit a node.
        let Some(vector) = view.stored_vector(&id) else {
            continue;
        };
        nodes.push(IndexedNode {
            tenant: tenant.to_string(),
            node: id,
            vector,
        });
        if nodes.len() > max_nodes {
            return Err(format!(
                "{}: the candidate graph holds more than {max_nodes} embedded nodes",
                medulla_decision_statistical_types::decision::codes::ER_INDEX_TOO_LARGE.as_str()
            ));
        }
    }
    let index = CandidateIndex::build(view.dimensions(), &nodes);
    let mut seen: std::collections::BTreeSet<(String, String)> = declared
        .iter()
        .filter_map(pair_of)
        .map(|(l, r)| {
            let (l, r) = ordered(l, r);
            (l.to_string(), r.to_string())
        })
        .collect();
    let taken: std::collections::BTreeSet<&str> =
        declared.iter().map(|o| o.option_id.as_str()).collect();
    let mut added: Vec<DeclaredOption> = Vec::new();
    'anchors: for anchor in declared {
        let Some((left, right)) = pair_of(anchor) else {
            continue;
        };
        for side in [left, right] {
            let Some(query) = view.stored_vector(side) else {
                continue;
            };
            for hit in index.neighbours(tenant, side, &query, candidate_k) {
                if !is_iri(&hit.node) {
                    continue;
                }
                let (l, r) = ordered(side, &hit.node);
                let id = proposal_option_id(ResolutionTier::Embedding, l, r);
                if declared.len() + added.len() >= MAX_DECLARED_OPTIONS {
                    break 'anchors;
                }
                if taken.contains(id.as_str()) || !seen.insert((l.to_string(), r.to_string())) {
                    continue;
                }
                let mut texts = vec![
                    DeclaredText {
                        key: LEFT_KEY.to_string(),
                        text: l.to_string(),
                    },
                    DeclaredText {
                        key: RIGHT_KEY.to_string(),
                        text: r.to_string(),
                    },
                ];
                texts.sort_by(|a, b| a.key.cmp(&b.key));
                added.push(DeclaredOption {
                    option_id: id,
                    classification: anchor.classification.clone(),
                    numbers: discounted(&anchor.numbers, hit.distance)?,
                    texts: BoundedVec::new(texts)?,
                });
            }
        }
    }
    Ok(added)
}

fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// Whether the caller's tenant holds an embedding for `node`. An absent node
/// and another tenant's node answer alike, so the check leaks nothing.
fn held(view: &crate::decide::stat_vectors::GraphVectors, node: &str) -> bool {
    view.stored_vector(node).is_some()
}

/// Refuse a declared pair that joins a node the asking tenant holds with one
/// it does not (MEDULLA-DECISION-ENGINE-R706.4). A pair neither of whose
/// sides the graph embeds carries no embedding evidence and is not refused.
fn check_tenants(
    options: &[DeclaredOption],
    view: &crate::decide::stat_vectors::GraphVectors,
) -> Result<(), String> {
    for option in options {
        let Some((left, right)) = pair_of(option) else {
            continue;
        };
        if held(view, left) != held(view, right) {
            return Err(format!(
                "{}: option '{}' pairs a node of this tenant with one it does not hold",
                ER_CROSS_TENANT.as_str(),
                option.option_id
            ));
        }
    }
    Ok(())
}

/// Record the model and content hash of every proposal option whose two sides
/// the caller's tenant holds (MEDULLA-DECISION-ENGINE-R706.5). A caller may
/// not supply either text.
fn stamp_proposals(
    options: Vec<DeclaredOption>,
    view: &crate::decide::stat_vectors::GraphVectors,
) -> Result<Vec<DeclaredOption>, String> {
    let model = view
        .core()
        .semantic_store
        .read()
        .space()
        .map(|space| format!("{}@{}", space.id, space.revision))
        .unwrap_or_else(|| MODEL_UNSPECIFIED.to_string());
    options
        .into_iter()
        .map(|mut option| {
            if tier_of_option(&option.option_id).is_none() {
                return Ok(option);
            }
            let Some((left, right)) = pair_of(&option) else {
                return Ok(option);
            };
            if !(held(view, left) && held(view, right)) {
                return Ok(option);
            }
            let hash = sha256_hex(&format!(
                "{}|{}",
                sha256_hex(&view.node_text(left)),
                sha256_hex(&view.node_text(right))
            ));
            let mut texts = option.texts.as_slice().to_vec();
            texts.push(DeclaredText {
                key: TEXT_MODEL.to_string(),
                text: model.clone(),
            });
            texts.push(DeclaredText {
                key: TEXT_CONTENT_HASH.to_string(),
                text: hash,
            });
            texts.sort_by(|a, b| a.key.cmp(&b.key));
            option.texts = BoundedVec::new(texts)?;
            Ok(option)
        })
        .collect()
}

/// The premises of every proposal option (MEDULLA-DECISION-ENGINE-R706.5):
/// its tier, and the model and content hash the prelude recorded, each a
/// `Claim` taken from the request's candidates.
fn proposal_premises(request: &DecideRequest) -> Vec<PremiseRef> {
    let CandidateSource::Declared { options } = &request.candidates else {
        return Vec::new();
    };
    let claim = |option: &DeclaredOption, fact: String| PremiseRef {
        subject: option.option_id.clone(),
        fact,
        class: PremiseClass::Claim,
        provenance: PremiseProvenance::Request {
            field: "candidates".to_string(),
        },
        trust_tier: None,
    };
    let mut out = Vec::new();
    for option in options.iter() {
        let Some(tier) = tier_of_option(&option.option_id) else {
            continue;
        };
        let id = &option.option_id;
        out.push(claim(option, format!("er:tier:{id}={}", tier_token(tier))));
        for (key, name) in [(TEXT_MODEL, "model"), (TEXT_CONTENT_HASH, "content_hash")] {
            if let Some(value) = text(option, key) {
                out.push(claim(option, format!("er:{name}:{id}={value}")));
            }
        }
    }
    out
}

/// The option the ladder's outcome names first: the acted option, or the
/// highest scored advisory option.
fn leading_option(outcome: &StatisticalOutcome) -> Option<&str> {
    match outcome {
        StatisticalOutcome::Acted { option_id, .. } => Some(option_id.as_str()),
        StatisticalOutcome::Advisory { scores, .. } => scores
            .iter()
            .max_by(|a, b| {
                a.score
                    .value
                    .cmp(&b.score.value)
                    .then_with(|| b.option_id.cmp(&a.option_id))
            })
            .map(|scored| scored.option_id.as_str()),
        _ => None,
    }
}

/// The first declared option, in option id order, that is a tiered proposal.
fn first_declared_proposal(request: &DecideRequest) -> Option<String> {
    let CandidateSource::Declared { options } = &request.candidates else {
        return None;
    };
    options
        .iter()
        .map(|option| option.option_id.as_str())
        .filter(|id| tier_of_option(id).is_some())
        .min()
        .map(str::to_string)
}

fn family_abstention(
    code: medulla_types::refusal_code::RefusalCode,
    detail: String,
) -> Result<StatisticalOutcome, String> {
    Ok(StatisticalOutcome::Abstained {
        reasons: BoundedVec::new(vec![AbstainReason::Family {
            family: QuestionKind::ResolveEntity,
            code,
            detail,
        }])?,
    })
}

/// The reviews the decision log holds for proposal options: the per-tier counts
/// and the accept-rate drift, recorded as engine params, and the drift fed to
/// the `head_drift` gauge (MEDULLA-DECISION-ENGINE-R706.3). Reads the caller's
/// own tenant's log through the one label rule.
fn review_params(
    store: &AgentLibraryStore,
    verified: &VerifiedRequestContext,
) -> Result<Vec<TypedParam>, String> {
    let reader = LogReader::of(verified);
    let labels = recent_labels_where(
        store,
        &reader,
        |option| tier_of_option(option).is_some(),
        MAX_LOG_ROWS,
    )?;
    let Some(newest) = labels.first() else {
        return Ok(Vec::new());
    };
    let mut params = vec![TypedParam {
        name: PARAM_LATEST.to_string(),
        value: TypedValue::Text(newest.evaluation_id.clone()),
    }];
    // Oldest first: the arrival order the drift windows read.
    let reviews: Vec<(String, ReviewOutcome)> = labels
        .iter()
        .rev()
        .map(|label| {
            let outcome = if label.success {
                ReviewOutcome::Accepted
            } else {
                ReviewOutcome::Rejected
            };
            (label.option_id.clone(), outcome)
        })
        .collect();
    for tier in TIERS {
        let evaluation = tier_evaluation(tier, &reviews);
        if evaluation.accepted + evaluation.rejected == 0 {
            continue;
        }
        params.push(count_param(
            tier_param(PARAM_ACCEPTED, tier),
            evaluation.accepted,
        ));
        params.push(count_param(
            tier_param(PARAM_REJECTED, tier),
            evaluation.rejected,
        ));
    }
    let knobs = medulla_server_api::knob_config::knobs();
    let outcomes: Vec<ReviewOutcome> = reviews.iter().map(|(_, outcome)| *outcome).collect();
    let window = usize::try_from(knobs.er_drift_window).map_err(invalid)?;
    let max_ppm = u32::try_from(knobs.er_drift_max_ppm).map_err(invalid)?;
    let (state, change_ppm) = match accept_rate_drift(&outcomes, window, max_ppm) {
        AcceptDrift::Insufficient { .. } => return Ok(params),
        AcceptDrift::Stable { change_ppm, .. } => ("stable", change_ppm),
        AcceptDrift::Drifted { change_ppm, .. } => ("drifted", change_ppm),
    };
    medulla_kernel::metrics::process().head_drift(
        "entity_resolution",
        to_q16(f64::from(change_ppm) / 1_000_000.0),
    );
    params.push(TypedParam {
        name: PARAM_DRIFT.to_string(),
        value: TypedValue::Text(state.to_string()),
    });
    params.push(count_param(PARAM_DRIFT_CHANGE.to_string(), change_ppm));
    Ok(params)
}

/// The candidate stage of a `resolve_entity` question, before the matrix is
/// computed (MEDULLA-DECISION-ENGINE-R706.3). Adds the tenant-scoped
/// neighbours of the declared pair sides as options when the question names a
/// graph, and records the review counts and drift as engine params.
pub(in crate::decide) async fn with_candidates(
    state: &Arc<RwLock<ServerState>>,
    store: &AgentLibraryStore,
    verified: &VerifiedRequestContext,
    mut request: DecideRequest,
) -> Result<DecideRequest, String> {
    if let Some(reserved) = request
        .params
        .iter()
        .find(|p| p.name.starts_with(ENGINE_PARAM_PREFIX))
    {
        return Err(invalid(format!(
            "parameter {} is recorded by the engine and cannot be supplied",
            reserved.name
        )));
    }
    if let CandidateSource::Declared { options } = &request.candidates {
        if let Some(reserved) = options.iter().find(|o| {
            o.texts
                .iter()
                .any(|t| t.key == TEXT_MODEL || t.key == TEXT_CONTENT_HASH)
        }) {
            return Err(invalid(format!(
                "option {} carries a text the engine records and the caller cannot supply",
                reserved.option_id
            )));
        }
    }
    let mut params = request.params.as_slice().to_vec();
    if let Some(graph) = text_param(&request, PARAM_GRAPH)? {
        let CandidateSource::Declared { options } = &request.candidates else {
            return Err(invalid(
                "a resolve_entity question declares the pairs its candidates grow from",
            ));
        };
        let view = graph_vectors(state, verified.agent_id(), graph).await?;
        let knobs = medulla_server_api::knob_config::knobs();
        let k = usize::try_from(knobs.er_candidate_k).map_err(invalid)?;
        let max_nodes = usize::try_from(knobs.er_index_max_nodes).map_err(invalid)?;
        check_tenants(options.as_slice(), &view)?;
        let added = neighbour_options(options.as_slice(), &view, verified.tenant(), k, max_nodes)?;
        let mut all = options.as_slice().to_vec();
        if !added.is_empty() {
            all.extend(added);
            all.sort_by(|a, b| a.option_id.cmp(&b.option_id));
        }
        request.candidates = CandidateSource::Declared {
            options: BoundedVec::new(stamp_proposals(all, &view)?).map_err(invalid)?,
        };
    }
    params.extend(review_params(store, verified)?);
    request.params = BoundedVec::new(params).map_err(invalid)?;
    Ok(request)
}

impl DecisionFamily for ResolveEntityFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        for option in options {
            let (left, right) = match pair_of(option) {
                Some((left, right)) if is_iri(left) && is_iri(right) => (left, right),
                _ => {
                    return Err(format!(
                        "{}: option '{}' must declare the texts {LEFT_KEY} and {RIGHT_KEY}, \
                         each an IRI",
                        medulla_decision_statistical_types::decision::codes::ENTITY_PAIR_INVALID
                            .as_str(),
                        option.option_id
                    ));
                }
            };
            if left == right {
                return Err(format!(
                    "{}: option '{}' pairs an entity with itself",
                    medulla_decision_statistical_types::decision::codes::ENTITY_PAIR_SELF.as_str(),
                    option.option_id
                ));
            }
        }
        Ok(())
    }

    fn refine(
        &self,
        request: Option<&DecideRequest>,
        pinned: &Pinned,
        _views: &[CandidateView],
        policy: &DecisionPolicy,
        _policy_digest: &str,
        mut executed: Executed,
    ) -> Result<Executed, String> {
        // Without a pinned head nothing was read and the ladder's own answer stands.
        if pinned.head.is_none() {
            return Ok(executed);
        }
        let alpha = resolve_entity_knobs::alpha_micros(policy);
        let leading = leading_option(&executed.ladder.outcome).map(str::to_string);
        let proposal = leading.as_deref().and_then(tier_of_option);
        let drifted = request.is_some_and(|request| {
            matches!(
                param(request, PARAM_DRIFT).map(|p| &p.value),
                Some(TypedValue::Text(state)) if state == "drifted"
            )
        });
        if let (Some(ResolutionTier::Probabilistic), true, Some(id)) =
            (proposal, drifted, leading.as_deref())
        {
            executed.ladder.outcome = family_abstention(
                ER_DRIFT_HOLD,
                format!("probabilistic proposal '{id}' is held while the accept rate drifts"),
            )?;
            return Ok(executed);
        }
        // The ladder abstained for insufficient confidence while the request
        // carries proposals: no proposal reached the head's certified act
        // threshold, which is the family's typed below-threshold abstention.
        if let (StatisticalOutcome::Abstained { reasons }, Some(request)) =
            (&executed.ladder.outcome, request)
        {
            if reasons.as_slice() == [AbstainReason::InsufficientConfidence] {
                if let Some(id) = first_declared_proposal(request) {
                    executed.ladder.outcome = family_abstention(
                        ER_BELOW_THRESHOLD,
                        format!("proposal '{id}' scored under the head's certified act threshold"),
                    )?;
                    return Ok(executed);
                }
            }
        }
        let abstain = match &executed.ladder.outcome {
            StatisticalOutcome::Acted { prediction_set, .. } => {
                prediction_set.len() != 1 || !calibrated_within(pinned, alpha)
            }
            StatisticalOutcome::Advisory { .. } => true,
            StatisticalOutcome::Explored { .. } | StatisticalOutcome::Abstained { .. } => false,
        };
        if abstain {
            let below = matches!(executed.ladder.outcome, StatisticalOutcome::Advisory { .. })
                && proposal.is_some();
            executed.ladder.outcome = match (below, leading.as_deref()) {
                (true, Some(id)) => family_abstention(
                    ER_BELOW_THRESHOLD,
                    format!("proposal '{id}' scored under the head's certified act threshold"),
                )?,
                _ => StatisticalOutcome::Abstained {
                    reasons: BoundedVec::new(vec![AbstainReason::InsufficientConfidence])?,
                },
            };
        }
        Ok(executed)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        let approving = matches!(
            param(request, "approve_link").map(|p| &p.value),
            Some(TypedValue::Bool(true))
        );
        let approver = match param(request, "approver").map(|p| &p.value) {
            Some(TypedValue::Text(name)) => name.as_str(),
            _ => "",
        };
        if approving && approver.trim().is_empty() {
            return Err(format!(
                "{}: an entity link approval names its approver",
                ApprovalRefusal::ApproverRequired.code()
            ));
        }
        Ok(())
    }

    fn premises(
        &self,
        request: &DecideRequest,
        executed: &Executed,
        _policy_digest: &str,
    ) -> Vec<PremiseRef> {
        let mut out = identity_premises(request);
        if let CandidateSource::Declared { options } = &request.candidates {
            for option in options.iter() {
                if let Some((left, right)) = pair_of(option) {
                    out.push(PremiseRef {
                        subject: option.option_id.clone(),
                        fact: format!("entity_pair:{left}={right}"),
                        class: PremiseClass::Claim,
                        provenance: PremiseProvenance::Request {
                            field: "candidates.options".to_string(),
                        },
                        trust_tier: None,
                    });
                }
            }
        }
        out.extend(proposal_premises(request));
        if let StatisticalOutcome::Acted { option_id, .. } = &executed.ladder.outcome {
            if tier_of_option(option_id) == Some(ResolutionTier::Probabilistic) {
                out.push(PremiseRef {
                    subject: option_id.clone(),
                    fact: format!("er:hypothesis:{option_id}"),
                    class: PremiseClass::Claim,
                    provenance: PremiseProvenance::Request {
                        field: "candidates".to_string(),
                    },
                    trust_tier: None,
                });
            }
        }
        out.extend(review_premises(request));
        out.push(PremiseRef {
            subject: "head".to_string(),
            fact: "calibration".to_string(),
            class: PremiseClass::Observation,
            provenance: PremiseProvenance::Observation {
                evaluation_id: request
                    .head
                    .as_ref()
                    .map(|pin| pin.definition_digest.clone())
                    .unwrap_or_default(),
            },
            trust_tier: None,
        });
        out
    }
}
