//! The `feature_match` question family (MEDULLA-DECISION-ENGINE-R001.101, R707.3).
//!
//! Features are `Declared` options: `option_id` is the feature id, the first
//! classification is its ontology label, and the texts `project` and `name`
//! say which project it belongs to and what it is called (`token_digest` is
//! optional). A number keyed `cos:<other feature id>` is the cosine of the two
//! features' embeddings (Q32, `1 << 32` is 1.0); a pair without one counts as
//! cosine zero. The one parameter `local_project` names the project whose
//! capabilities the others are compared against.
//!
//! The kernel `dedup` scores every pair inside a same-label block under the
//! registry knobs `match.*`. The answer is an uncalibrated advisory with one
//! option per match, scored by its similarity, and one option per plan. A
//! feature of another project that matches no local feature is a gap, and the
//! gaps of one question are closed by one `PlanProposal`. Language model calls
//! stay outside medulla and nothing is written: the ledger is `Method::FeatureLedger`.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_stats::decision::feature_match::{dedup, name_only_pair, MatchParams, PPM};
use medulla_decision_types::decision::feature_match::{
    match_option_id, plan_option_id, FeatureMatchAnswer, FeatureMatrixRow, Match, PlanProposal,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionSafety, ScoredOption, StatisticalOutcome,
};
use medulla_decision_types::decision::{
    DecisionPolicy, EvidenceClass, QuantScaleTag, QuantisedValue, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::{BoundedVec, Digest256};

use crate::decide::stat_executor::{Executed, ExecutionContext, FamilyExecuted};
use crate::decide::stat_support::invalid_parameter;

use super::kernel::claim;
use super::support::text_param;
use super::DecisionFamily;

const KIND: &str = "feature_match";
const PROJECT_PARAM: &str = "local_project";
const COSINE_PREFIX: &str = "cos:";
const ONE_Q32: i128 = 1 << 32;

/// The declared features and the cosines the caller supplied.
struct Parsed {
    rows: Vec<FeatureMatrixRow>,
    cosine: BTreeMap<(String, String), u32>,
    local_project: String,
}

fn text<'a>(option: &'a DeclaredOption, key: &str) -> Option<&'a str> {
    option
        .texts
        .iter()
        .find(|t| t.key == key)
        .map(|t| t.text.as_str())
}

fn pair_key(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// A Q32 cosine on parts per million, clamped to `0..=PPM`.
fn q32_to_ppm(q32: i64) -> u32 {
    let ppm = i128::from(q32).clamp(0, ONE_Q32) * i128::from(PPM) / ONE_Q32;
    u32::try_from(ppm).unwrap_or(PPM)
}

fn ppm_to_q32(ppm: u32) -> i64 {
    i64::try_from(i128::from(ppm) * ONE_Q32 / i128::from(PPM)).unwrap_or(i64::MAX)
}

fn parse(request: &DecideRequest) -> Result<Parsed, String> {
    if request.question.safety != QuestionSafety::Ordinary
        || request.head.is_some()
        || !request.belief_as_of.is_empty()
    {
        return Err(invalid_parameter(format!(
            "{KIND} needs ordinary safety, no head and no belief slices"
        )));
    }
    let CandidateSource::Declared { options } = &request.candidates else {
        return Err(invalid_parameter(format!("{KIND} declares its candidates")));
    };
    if request.params.iter().any(|p| p.name != PROJECT_PARAM) {
        return Err(invalid_parameter(format!(
            "{KIND} takes one parameter, {PROJECT_PARAM}"
        )));
    }
    let local_project = text_param(request, PROJECT_PARAM)?
        .filter(|p| !p.is_empty())
        .ok_or_else(|| invalid_parameter(format!("{KIND} needs the parameter {PROJECT_PARAM}")))?
        .to_string();
    let ids: BTreeSet<&str> = options.iter().map(|o| o.option_id.as_str()).collect();
    let mut rows = Vec::new();
    let mut cosine: BTreeMap<(String, String), u32> = BTreeMap::new();
    for option in options.iter() {
        let id = option.option_id.as_str();
        if id.starts_with("match:") || id.starts_with("plan:") {
            return Err(invalid_parameter(format!(
                "{KIND} reserves the option id prefixes match: and plan:"
            )));
        }
        if option
            .texts
            .iter()
            .any(|t| !matches!(t.key.as_str(), "project" | "name" | "token_digest"))
        {
            return Err(invalid_parameter(format!(
                "{KIND} option {id} declares a text other than project, name or token_digest"
            )));
        }
        let required = |key: &str| {
            text(option, key)
                .filter(|v| !v.is_empty())
                .ok_or_else(|| invalid_parameter(format!("{KIND} option {id} declares no {key}")))
        };
        let label = option
            .classification
            .iter()
            .next()
            .filter(|l| !l.is_empty())
            .ok_or_else(|| {
                invalid_parameter(format!("{KIND} option {id} declares no ontology label"))
            })?;
        for number in option.numbers.iter() {
            let Some(peer) = number.key.strip_prefix(COSINE_PREFIX) else {
                return Err(invalid_parameter(format!(
                    "{KIND} option {id} declares the number {}, which is not cos:<feature id>",
                    number.key
                )));
            };
            if !ids.contains(peer) || peer == id {
                return Err(invalid_parameter(format!(
                    "{KIND} option {id} names the cosine peer {peer}, which is not another declared feature"
                )));
            }
            let ppm = q32_to_ppm(number.q32);
            if let Some(known) = cosine.insert(pair_key(id, peer), ppm) {
                if known != ppm {
                    return Err(invalid_parameter(format!(
                        "{KIND} declares two different cosines for {id} and {peer}"
                    )));
                }
            }
        }
        rows.push(FeatureMatrixRow {
            feature_id: id.to_string(),
            project: required("project")?.to_string(),
            name: required("name")?.to_string(),
            label: label.clone(),
            token_digest: text(option, "token_digest").unwrap_or_default().to_string(),
        });
    }
    Ok(Parsed {
        rows,
        cosine,
        local_project,
    })
}

fn match_params() -> Result<MatchParams, String> {
    let knobs = medulla_server_api::knob_config::knobs();
    let params = MatchParams {
        w_token: u32::try_from(knobs.match_w_token).unwrap_or(u32::MAX),
        w_label: u32::try_from(knobs.match_w_label).unwrap_or(u32::MAX),
        w_embedding: u32::try_from(knobs.match_w_embedding).unwrap_or(u32::MAX),
        max_block: usize::try_from(knobs.match_max_block).unwrap_or(usize::MAX),
        score_min: u32::try_from(knobs.match_score_min).unwrap_or(u32::MAX),
        entropy_mbit: u32::try_from(knobs.match_entropy_mbit).unwrap_or(u32::MAX),
    };
    params.validate().map_err(invalid_parameter)?;
    Ok(params)
}

fn q32(value: i64) -> QuantisedValue {
    QuantisedValue {
        scale: QuantScaleTag::Q32,
        value,
    }
}

/// The plan that closes `gaps`: its id and rationale are digests of the gap
/// set and of the matches the gaps were measured against.
fn plan_for(gaps: Vec<String>, matches: &[Match]) -> Result<PlanProposal, String> {
    let mut framed = Vec::new();
    for id in &gaps {
        framed.extend_from_slice(id.as_bytes());
        framed.push(0);
    }
    let plan_id = format!("plan-{}", &Digest256::sha256(&framed).to_hex()[..16]);
    let rationale = serde_json::to_vec(matches).map_err(|error| {
        invalid_parameter(format!("{KIND}: the matches do not serialise: {error}"))
    })?;
    Ok(PlanProposal {
        plan_id,
        feature_ids: gaps,
        rationale_digest: Digest256::sha256(&rationale).to_hex(),
    })
}

/// The family of the `feature_match` question kind.
pub struct FeatureMatchFamily;

impl DecisionFamily for FeatureMatchFamily {
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        parse(request).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(parse(request).and_then(|parsed| {
            let params = match_params()?;
            let embedding = |a: &str, b: &str| {
                parsed
                    .cosine
                    .get(&pair_key(a, b))
                    .copied()
                    .unwrap_or_default()
            };
            if let Some((left, right)) = name_only_pair(&parsed.rows, &embedding, &params) {
                return Err(format!(
                    "{}: features {left} and {right} share a high-entropy name and no other evidence",
                    medulla_decision_types::decision::feature_match::MATCH_HIGH_ENTROPY_NAME_ONLY
                        .as_str()
                ));
            }
            let found = dedup(&parsed.rows, &embedding, &params);
            let project_of: BTreeMap<&str, &str> = parsed
                .rows
                .iter()
                .map(|r| (r.feature_id.as_str(), r.project.as_str()))
                .collect();
            let is_local = |id: &str| project_of.get(id) == Some(&parsed.local_project.as_str());
            let mut covered: BTreeSet<&str> = BTreeSet::new();
            for m in &found.matches {
                match (is_local(&m.local), is_local(&m.external)) {
                    (true, false) => {
                        covered.insert(m.external.as_str());
                    }
                    (false, true) => {
                        covered.insert(m.local.as_str());
                    }
                    _ => {}
                }
            }
            let gaps: Vec<String> = parsed
                .rows
                .iter()
                .filter(|r| {
                    r.project != parsed.local_project && !covered.contains(r.feature_id.as_str())
                })
                .map(|r| r.feature_id.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let plans = if gaps.is_empty() {
                Vec::new()
            } else {
                vec![plan_for(gaps, &found.matches)?]
            };

            let mut ranked: Vec<&Match> = found.matches.iter().collect();
            ranked.sort_by(|a, b| {
                b.score_ppm
                    .cmp(&a.score_ppm)
                    .then_with(|| (&a.local, &a.external).cmp(&(&b.local, &b.external)))
            });
            let mut scores: Vec<ScoredOption> = ranked
                .iter()
                .map(|m| ScoredOption {
                    option_id: match_option_id(&m.local, &m.external),
                    score: q32(ppm_to_q32(m.score_ppm)),
                    probability: None,
                })
                .collect();
            scores.extend(plans.iter().map(|plan| ScoredOption {
                option_id: plan_option_id(&plan.plan_id),
                score: q32(ppm_to_q32(PPM)),
                probability: None,
            }));
            let scores = BoundedVec::new(scores)
                .map_err(|detail| invalid_parameter(format!("{KIND}: {detail}")))?;
            let premises = vec![
                claim(
                    KIND,
                    format!("feature_match:local_project={}", parsed.local_project),
                    "params.local_project",
                ),
                claim(
                    KIND,
                    format!("feature_match:features={}", parsed.rows.len()),
                    "candidates",
                ),
            ];
            Ok(Executed::from_family(
                StatisticalOutcome::Advisory {
                    scores,
                    calibrated: false,
                },
                Some(FamilyExecuted {
                    answer: Some(FamilyAnswer::FeatureMatch(Box::new(FeatureMatchAnswer {
                        matches: found.matches,
                        clusters: found.clusters,
                        unscored: found.unscored,
                        plans,
                    }))),
                    premises,
                    resolution: None,
                    evidence: Some(EvidenceClass::Claim),
                }),
            ))
        }))
    }

    fn resolution_kind(&self, _outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        Some(ResolutionKind::Statistical)
    }

    fn evidence_class(&self, _outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        Some(EvidenceClass::Claim)
    }
}
