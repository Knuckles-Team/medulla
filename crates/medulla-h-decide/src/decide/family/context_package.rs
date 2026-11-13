//! The `context_package` question family (MEDULLA-DECISION-ENGINE-R137.8,
//! MEDULLA-DECISION-ENGINE-R137.9).
//!
//! Which measured evidence spans fit a model window under token, money and
//! latency ceilings. The package arrives in the typed `context_packages`
//! parameter. The family lowers it and solves it exactly through the one
//! package kernel (`medulla_decision_policy::solve::context_package::select`),
//! which verifies the certificate independently and refuses anything but a
//! proven optimum. The answer is an advisory with one scored option per
//! chosen candidate; a proven optimum is sealed as an optimization with proof
//! evidence.

use std::collections::BTreeSet;

use medulla_decision_policy::solve::context_package::{select_with, PackageRefusal};
use medulla_decision_policy::solve::{Certificate, Model, SolverConfig};
use medulla_decision_stats::decision::ladder::LadderResult;
use medulla_decision_types::decision::context_package::{
    ContextPackageErrorCode, ContextPackageRequest, RemainingBudget, NONE_ADMISSIBLE,
    WORK_EXHAUSTED,
};
use medulla_decision_types::decision::knob_families::context as context_knobs;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, ScoredOption, StatisticalErrorCode,
    StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::AbstainReason;
use medulla_decision_types::decision::{
    EvidenceClass, PremiseClass, PremiseRef, QuantScaleTag, QuantisedValue, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};
use crate::decide::stat_support::refusal;

use super::support::param;
use super::DecisionFamily;

/// The Q32 fixed-point value for "chosen".
const CHOSEN_SCORE: i64 = 1i64 << 32;

/// The parameter naming the graph that holds the cited source nodes.
pub(in crate::decide) const PARAM_SOURCE_GRAPH: &str = "source_graph";

/// The parameter naming the task class whose complexity band bounds the
/// package (R137.16).
const PARAM_TASK_CLASS: &str = "task_class";

/// The family of the `context_package` question kind.
pub struct ContextPackageFamily;

fn invalid(detail: impl std::fmt::Display) -> String {
    refusal(StatisticalErrorCode::ParameterInvalid, detail)
}

/// The one `context_packages` parameter, validated.
fn package(request: &DecideRequest) -> Result<&ContextPackageRequest, String> {
    let mut found = request
        .params
        .iter()
        .filter(|param| param.name == "context_packages");
    let (Some(param), None) = (found.next(), found.next()) else {
        return Err(invalid(
            "a context_package question carries exactly one context_packages parameter",
        ));
    };
    let TypedValue::ContextPackages(package) = &param.value else {
        return Err(invalid("context_packages is not a context package"));
    };
    package
        .validate()
        .map_err(|(code, detail)| invalid(format!("{}: {detail}", code.as_str())))?;
    Ok(package)
}

impl DecisionFamily for ContextPackageFamily {
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        let package = package(request)?;
        let CandidateSource::Declared { options } = &request.candidates else {
            return Err(invalid(
                "a context_package question declares its candidates",
            ));
        };
        let declared: BTreeSet<&str> = options.iter().map(|o| o.option_id.as_str()).collect();
        let packaged: BTreeSet<&str> = package
            .candidates
            .iter()
            .map(|c| c.candidate_id.as_str())
            .collect();
        if declared != packaged || declared.len() != options.len() {
            return Err(invalid(
                "declared options must equal the package candidate ids",
            ));
        }
        Ok(())
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &medulla_decision_types::decision::DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(solve(ctx.tenant_id, request, policy, graph_view))
    }

    fn premises(
        &self,
        _request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        executed
            .family_premises
            .iter()
            .map(|premise| premise.seal(policy_digest))
            .collect()
    }

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        matches!(outcome, StatisticalOutcome::Advisory { .. })
            .then_some(ResolutionKind::Optimization)
    }

    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(EvidenceClass::Proof)
    }
}

/// The graph named by the `source_graph` parameter, for the executor to open
/// under the caller's access check and row-level filter.
pub(in crate::decide) fn source_graph(request: &DecideRequest) -> Result<&str, String> {
    let mut found = request
        .params
        .iter()
        .filter(|param| param.name == PARAM_SOURCE_GRAPH);
    let (Some(param), None) = (found.next(), found.next()) else {
        return Err(invalid(
            "a context_package question names its source graph in exactly one source_graph parameter",
        ));
    };
    match &param.value {
        TypedValue::Text(graph) if !graph.is_empty() => Ok(graph),
        _ => Err(invalid("source_graph is not a graph name")),
    }
}

/// The package's request-carried knobs must equal the values the policy in
/// force resolves; the family never overrides one silently.
fn check_knobs(
    package: &ContextPackageRequest,
    policy: &medulla_decision_types::decision::DecisionPolicy,
) -> Result<(), String> {
    let node_budget = context_knobs::solver_node_budget(policy);
    if u64::from(package.node_budget) != node_budget {
        return Err(invalid(format!(
            "node_budget {} differs from the policy value of context.solver_node_budget ({node_budget})",
            package.node_budget
        )));
    }
    let floor = context_knobs::value_floor_micro_per_token(policy);
    if package.value_floor_micro_per_token != floor {
        return Err(invalid(format!(
            "value_floor_micro_per_token {} differs from the policy value of context.value_floor_micro_per_token ({floor})",
            package.value_floor_micro_per_token
        )));
    }
    Ok(())
}

/// Every candidate must equal its source node in the caller's visible graph:
/// the node `source_id` carries the same `revision` and `content_digest`. A
/// source the caller cannot see is indistinguishable from an absent one.
fn check_sources(package: &ContextPackageRequest, view: &GraphView) -> Result<(), String> {
    for candidate in &package.candidates {
        let stale = |why: &str| {
            invalid(format!(
                "{}: candidate {} {why}",
                ContextPackageErrorCode::StaleSource.as_str(),
                candidate.candidate_id
            ))
        };
        let Some(node) = view.node_row_object(&candidate.source_id) else {
            return Err(stale("names a source that is not visible"));
        };
        if node.get("revision").and_then(serde_json::Value::as_u64)
            != Some(candidate.source_revision)
        {
            return Err(stale("cites a source revision that has moved"));
        }
        if node
            .get("content_digest")
            .and_then(serde_json::Value::as_str)
            != Some(candidate.content_digest.as_str())
        {
            return Err(stale("cites content that has changed"));
        }
    }
    Ok(())
}

fn refused(refusal: PackageRefusal) -> String {
    invalid(format!("{}: {}", refusal.code.as_str(), refusal.detail))
}

/// An evaluate-only answer that carries `outcome` and nothing else.
fn answered(outcome: StatisticalOutcome) -> Executed {
    Executed {
        matrix: None,
        ladder: LadderResult {
            outcome,
            calibration: None,
            explained: None,
            audit: None,
            logging: Vec::new(),
        },
        reading: None,
        seed: [0u8; 32],
        harness_contrast: None,
        topology: None,
        context_selection: None,
        thought: None,
        family_premises: Vec::new(),
        route: None,
    }
}

/// An abstention of the package family: the registered code and its detail, then
/// any further reasons.
fn abstained(
    code: medulla_types::refusal_code::RefusalCode,
    detail: impl Into<String>,
    more: Vec<AbstainReason>,
) -> Result<Executed, String> {
    let mut reasons = vec![AbstainReason::Family {
        family: QuestionKind::ContextPackage,
        code,
        detail: detail.into(),
    }];
    reasons.extend(more);
    let reasons = BoundedVec::new(reasons)
        .map_err(|detail| refusal(StatisticalErrorCode::CandidateSetTooLarge, detail))?;
    Ok(answered(StatisticalOutcome::Abstained { reasons }))
}

/// A spent node budget abstains with the verified incumbent and lower bound; any other
/// kernel refusal is a request refusal. A bound the verifier did not establish is reported
/// as the least `Scalar`, which every objective satisfies.
fn kernel_refusal(refusal: PackageRefusal) -> Result<Executed, String> {
    let Some(exhaustion) = refusal.exhaustion else {
        return Err(refused(refusal));
    };
    abstained(
        WORK_EXHAUSTED,
        refusal.detail,
        vec![AbstainReason::BudgetExhausted {
            incumbent: exhaustion.incumbent.map(String::from),
            lower_bound: exhaustion
                .lower_bound
                .unwrap_or(medulla_decision_policy::solve::Scalar::new(i128::MIN)),
        }],
    )
}

/// The solver of a served package question. Under test a solver registered for
/// the question id replaces it, so one test corrupts the proof without
/// touching another's.
fn solver(question_id: &str) -> impl FnOnce(&Model, &SolverConfig) -> Certificate {
    #[cfg(test)]
    let replacement = overrides::get(question_id);
    #[cfg(not(test))]
    let _ = question_id;
    move |model: &Model, config: &SolverConfig| {
        #[cfg(test)]
        if let Some(replacement) = replacement {
            return replacement(model, config);
        }
        medulla_decision_policy::solve::solve(model, config)
    }
}

#[cfg(test)]
pub(in crate::decide) mod overrides {
    use std::collections::BTreeMap;
    use std::sync::{Mutex, OnceLock};

    use medulla_decision_policy::solve::{Certificate, Model, SolverConfig};

    /// A replacement solver.
    pub(in crate::decide) type SolverFn = fn(&Model, &SolverConfig) -> Certificate;

    fn table() -> &'static Mutex<BTreeMap<String, SolverFn>> {
        static TABLE: OnceLock<Mutex<BTreeMap<String, SolverFn>>> = OnceLock::new();
        TABLE.get_or_init(|| Mutex::new(BTreeMap::new()))
    }

    /// Replace the solver of the question `question_id`.
    pub(in crate::decide) fn set(question_id: &str, solver: SolverFn) {
        table()
            .lock()
            .expect("override table")
            .insert(question_id.to_string(), solver);
    }

    pub(super) fn get(question_id: &str) -> Option<SolverFn> {
        table()
            .lock()
            .expect("override table")
            .get(question_id)
            .copied()
    }
}

/// The task class the request names, if any.
fn task_class(request: &DecideRequest) -> Result<Option<&str>, String> {
    match param(request, PARAM_TASK_CLASS)? {
        None => Ok(None),
        Some(TypedValue::Iri(iri)) => Ok(Some(iri.as_str())),
        Some(_) => Err(invalid("parameter task_class is not an IRI")),
    }
}

/// The package bounded by what the run has left (R137.16).
enum Bounded {
    /// The package to solve (its caller ceiling already tightened) and the
    /// premises the bound rests on.
    Solve(Box<ContextPackageRequest>, Vec<FamilyPremise>),
    /// The task class holds no `complexityBand`: nothing can be bounded.
    UnknownBand(String),
}

fn micros(value: u64) -> String {
    format!("{}.{:06}", value / 1_000_000, value % 1_000_000)
}

/// `remaining = ceiling - committed - reserved`; the caller ceiling becomes
/// `floor(remaining * fraction)` for the task's measured complexity band,
/// combined by minimum with any ceiling the caller supplied. The tokenizer of
/// the remainder must be every candidate's tokenizer.
fn bound_by_remaining(
    tenant: &str,
    package: &ContextPackageRequest,
    request: &DecideRequest,
    policy: &medulla_decision_types::decision::DecisionPolicy,
    view: &GraphView,
) -> Result<Bounded, String> {
    let class = task_class(request)?;
    let Some(remaining) = &package.remaining else {
        return match class {
            None => Ok(Bounded::Solve(Box::new(package.clone()), Vec::new())),
            Some(_) => Err(invalid(
                "a task_class bounds the package only together with a remaining budget",
            )),
        };
    };
    if let Some(stranger) = package.candidates.iter().find(|c| {
        c.tokenizer_id != remaining.tokenizer_id
            || c.tokenizer_version != remaining.tokenizer_version
    }) {
        return Err(invalid(format!(
            "{}: candidate {} is measured by {}@{}, the remaining budget by {}@{}",
            RemainingBudget::TOKENIZER_MISMATCH.as_str(),
            stranger.candidate_id,
            stranger.tokenizer_id,
            stranger.tokenizer_version,
            remaining.tokenizer_id,
            remaining.tokenizer_version
        )));
    }
    let left = remaining
        .remaining()
        .map_err(|(code, detail)| invalid(format!("{}: {detail}", code.as_str())))?;
    let mut premises = vec![premise(
        format!("context_package.remaining_tokens={left}"),
        PremiseClass::Claim,
        FamilyPremiseSource::Request("params.context_packages"),
    )];
    let mut ceiling = left;
    if let Some(class) = class {
        let band = view
            .node_row_object(class)
            // The view is already the caller's row-level filtered snapshot; a
            // node that names another tenant is never read on top of that.
            .filter(|props| {
                props
                    .get("tenant_id")
                    .and_then(serde_json::Value::as_str)
                    .is_none_or(|owner| owner == tenant)
            })
            .and_then(|props| {
                props
                    .get("complexityBand")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            });
        let Some((band, fraction)) = band.and_then(|band| {
            context_knobs::band_fraction_micros(policy, &band).map(|fraction| (band, fraction))
        }) else {
            return Ok(Bounded::UnknownBand(class.to_string()));
        };
        ceiling = u64::try_from(u128::from(left) * u128::from(fraction) / 1_000_000)
            .map_err(|_| invalid("the band ceiling overflows"))?;
        premises.push(premise(
            format!("context_package.complexity_band={band}"),
            PremiseClass::Observation,
            FamilyPremiseSource::Observation(class.to_string()),
        ));
        premises.push(premise(
            format!("context_package.band_fraction={}", micros(fraction)),
            PremiseClass::Definition,
            FamilyPremiseSource::Policy,
        ));
    }
    let mut bounded = package.clone();
    bounded.budget.caller_ceiling_tokens = Some(
        package
            .budget
            .caller_ceiling_tokens
            .map_or(ceiling, |supplied| supplied.min(ceiling)),
    );
    Ok(Bounded::Solve(Box::new(bounded), premises))
}

fn solve(
    tenant: &str,
    request: &DecideRequest,
    policy: &medulla_decision_types::decision::DecisionPolicy,
    graph_view: Option<&GraphView>,
) -> Result<Executed, String> {
    let package = package(request)?;
    check_knobs(package, policy)?;
    let view = graph_view.ok_or_else(|| {
        invalid("a context_package question reads its sources from the named source graph")
    })?;
    check_sources(package, view)?;
    let (bounded, bound_premises) =
        match bound_by_remaining(tenant, package, request, policy, view)? {
            Bounded::Solve(bounded, premises) => (bounded, premises),
            Bounded::UnknownBand(class) => {
                return Ok(answered(StatisticalOutcome::Abstained {
                    reasons: BoundedVec::new(vec![AbstainReason::UnknownFact {
                        component_id: class,
                        field: "complexityBand".to_string(),
                    }])
                    .map_err(|detail| {
                        refusal(StatisticalErrorCode::CandidateSetTooLarge, detail)
                    })?,
                }));
            }
        };
    let solved = match select_with(&bounded, solver(&request.question.question_id)) {
        Ok(solved) => solved,
        Err(refusal) => return kernel_refusal(refusal),
    };
    if solved.selection.chosen_ids.is_empty() {
        return abstained(
            NONE_ADMISSIBLE,
            format!(
                "no candidate fits the effective capacity of {} tokens",
                solved.selection.effective_capacity_tokens
            ),
            Vec::new(),
        );
    }
    let scores: Vec<ScoredOption> = solved
        .selection
        .chosen_ids
        .iter()
        .map(|id| ScoredOption {
            option_id: id.clone(),
            score: QuantisedValue {
                scale: QuantScaleTag::Q32,
                value: CHOSEN_SCORE,
            },
            probability: None,
        })
        .collect();
    let scores = BoundedVec::new(scores)
        .map_err(|detail| refusal(StatisticalErrorCode::CandidateSetTooLarge, detail))?;
    let mut executed = answered(StatisticalOutcome::Advisory {
        scores,
        calibrated: false,
    });
    let mut premises = bound_premises;
    premises.extend(evidence_premises(package, &solved)?);
    executed.family_premises = premises;
    Ok(executed)
}

/// The version of the exact solver and verifier the package is solved with.
const SOLVER_VERSION: &str = "context-package-exact/1";

fn premise(fact: String, class: PremiseClass, source: FamilyPremiseSource) -> FamilyPremise {
    FamilyPremise {
        subject: "context_package",
        fact,
        class,
        source,
    }
}

/// The SHA-256 of the JSON encoding of `value`, as lowercase hex.
fn digest_of<T: serde::Serialize>(value: &T) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(value)
        .map_err(|error| invalid(format!("package evidence is not encodable: {error}")))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

/// One total recomputed with checked arithmetic over the chosen candidates; a
/// product or sum that overflows is refused, and a rate the request never gave
/// leaves the total `unknown`.
fn checked_total(
    chosen: &[&medulla_decision_types::decision::context_package::ContextPackageCandidate],
    unit: Option<u64>,
) -> Result<Option<u64>, String> {
    let mut tokens = 0_u64;
    for candidate in chosen {
        tokens = tokens
            .checked_add(candidate.token_weight)
            .ok_or_else(|| invalid("the chosen token total overflows"))?;
    }
    Ok(unit
        .map(|rate| (tokens, rate))
        .and_then(|(t, r)| t.checked_mul(r)))
}

/// The evidence the answer rests on (MEDULLA-DECISION-ENGINE-R137.11). The
/// universe digest covers every candidate, chosen or not; the totals are
/// recomputed here from the chosen candidates, never read from the selection
/// record; the certificate premises are proof, the request's measurements are
/// claims.
fn evidence_premises(
    package: &ContextPackageRequest,
    solved: &medulla_decision_policy::solve::context_package::SolvedPackage,
) -> Result<Vec<FamilyPremise>, String> {
    let mut universe: Vec<_> = package
        .candidates
        .iter()
        .map(|c| {
            (
                &c.candidate_id,
                &c.source_id,
                c.cited_start,
                c.cited_end,
                c.source_revision,
                &c.content_digest,
            )
        })
        .collect();
    universe.sort();
    let selection = &solved.selection;
    let chosen: Vec<_> = package
        .candidates
        .iter()
        .filter(|c| selection.chosen_ids.iter().any(|id| *id == c.candidate_id))
        .collect();
    let mut tokens = 0_u64;
    for candidate in &chosen {
        tokens = tokens
            .checked_add(candidate.token_weight)
            .ok_or_else(|| invalid("the chosen token total overflows"))?;
    }
    let cost = checked_total(&chosen, package.budget.price_micro_per_token)?;
    let latency = match package.budget.tokens_per_second.filter(|rate| *rate > 0) {
        Some(rate) => tokens.checked_mul(1_000).map(|ms| ms / rate),
        None => None,
    };
    let render =
        |value: Option<u64>| value.map_or_else(|| "unknown".to_string(), |v| v.to_string());
    let request_claim = |fact: String| {
        premise(
            fact,
            PremiseClass::Claim,
            FamilyPremiseSource::Request("params.context_packages"),
        )
    };
    let proven = |fact: String| {
        premise(
            fact,
            PremiseClass::Proof,
            FamilyPremiseSource::Observation(selection.certificate_digest.clone()),
        )
    };
    let tokenizer = package
        .candidates
        .iter()
        .map(|c| format!("{}@{}", c.tokenizer_id, c.tokenizer_version))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(",");
    Ok(vec![
        request_claim(format!(
            "context_package.universe_digest={}",
            digest_of(&universe)?
        )),
        request_claim(format!("context_package.tokenizer={tokenizer}")),
        request_claim(format!(
            "context_package.budget_digest={}",
            digest_of(&package.budget)?
        )),
        proven(format!(
            "context_package.capacity_tokens={}",
            selection.effective_capacity_tokens
        )),
        premise(
            format!(
                "context_package.objective=value_floor_micro_per_token:{};rounding:floor",
                package.value_floor_micro_per_token
            ),
            PremiseClass::Definition,
            FamilyPremiseSource::Policy,
        ),
        premise(
            format!("context_package.solver_version={SOLVER_VERSION}"),
            PremiseClass::Definition,
            FamilyPremiseSource::Policy,
        ),
        proven(format!(
            "context_package.model_digest={}",
            selection.model_digest
        )),
        proven(format!(
            "context_package.certificate_digest={}",
            selection.certificate_digest
        )),
        proven(format!(
            "context_package.certificate_status={}",
            if selection.verified {
                "verified_optimal"
            } else {
                "unverified"
            }
        )),
        proven(format!("context_package.total_tokens={tokens}")),
        proven(format!("context_package.total_cost_micro={}", render(cost))),
        proven(format!(
            "context_package.total_latency_ms={}",
            render(latency)
        )),
    ])
}
