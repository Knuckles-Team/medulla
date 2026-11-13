//! Package lowering and exact selection kernel (MEDULLA-DECISION-ENGINE-R137.8).
//!
//! [`lower`] turns one [`ContextPackageRequest`] into a 0-1 model: one variable
//! per candidate, one at-most-one row per evidence unit and one knapsack row
//! that caps the total token weight at the effective capacity. [`select`] runs
//! the one exact solver and the independent verifier over that model and
//! returns the chosen package. Served `Decide` (through the package family)
//! and served `ContextCompile` (through `medulla_plan::context_bundle`) both
//! call these two functions, so no second knapsack exists.

use std::collections::BTreeMap;

use medulla_decision_types::decision::context_package::{
    ContextPackageBudget, ContextPackageCandidate, ContextPackageErrorCode, ContextPackageRequest,
    ContextPackageSelection,
};
use medulla_types::contract::BoundedVec;

use super::{
    model::{
        Coefficient, ConstraintBody, ConstraintSpec, ObjectiveLevelSpec, ObjectiveTerm, Relation,
        Term, VarId,
    },
    verify, Certificate, Model, ModelSpec, Scalar, SolveStatus, SolverConfig, SolverConfigSpec,
    Verdict,
};

/// A typed refusal of the package kernel: the closed code and a detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageRefusal {
    /// The closed refusal code.
    pub code: ContextPackageErrorCode,
    /// What was refused.
    pub detail: String,
    /// Where an exhausted search stood, for `WorkExhausted` only.
    pub exhaustion: Option<PackageExhaustion>,
}

/// What the verifier established about a search whose node budget ran out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageExhaustion {
    /// The verified objective of the best package found, when one was found.
    pub incumbent: Option<Scalar>,
    /// The verified lower bound, when any bound leaf was proven.
    pub lower_bound: Option<Scalar>,
}

impl PackageRefusal {
    fn new(code: ContextPackageErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
            exhaustion: None,
        }
    }

    fn invalid(detail: impl Into<String>) -> Self {
        Self::new(ContextPackageErrorCode::InvalidMeasurement, detail)
    }
}

/// The effective token capacity: the least of every supplied ceiling.
///
/// A ceiling joins the minimum only when supplied. A known zero price drops
/// the money term. A known zero throughput gives capacity zero. A ceiling whose
/// rate is missing refuses `CONTEXT_PACKAGE_MISSING_FACT`.
pub fn effective_capacity(budget: &ContextPackageBudget) -> Result<u64, PackageRefusal> {
    let window = budget
        .window_tokens
        .checked_sub(budget.reserved_output_tokens)
        .unwrap_or(0);
    let mut capacity = window;
    if let Some(caller) = budget.caller_ceiling_tokens {
        capacity = capacity.min(caller);
    }
    if let Some(money) = budget.money_ceiling_micro {
        match budget.price_micro_per_token {
            None => {
                return Err(PackageRefusal::new(
                    ContextPackageErrorCode::MissingFact,
                    "a money ceiling needs the price per token",
                ))
            }
            Some(0) => {}
            Some(price) => capacity = capacity.min(money / price),
        }
    }
    if let Some(latency_ms) = budget.latency_ceiling_ms {
        match budget.tokens_per_second {
            None => {
                return Err(PackageRefusal::new(
                    ContextPackageErrorCode::MissingFact,
                    "a latency ceiling needs the measured throughput",
                ))
            }
            Some(0) => capacity = 0,
            Some(rate) => {
                let scaled = latency_ms.checked_mul(rate).ok_or_else(|| {
                    PackageRefusal::invalid("latency ceiling times throughput overflows u64")
                })?;
                capacity = capacity.min(scaled / 1_000);
            }
        }
    }
    Ok(capacity)
}

fn to_i64(value: u64, what: &str) -> Result<i64, PackageRefusal> {
    i64::try_from(value).map_err(|_| PackageRefusal::invalid(format!("{what} exceeds i64")))
}

/// Net value of one candidate: `value_micro - floor * token_weight`, checked.
fn net_value(candidate: &ContextPackageCandidate, floor: u64) -> Result<i64, PackageRefusal> {
    let penalty = i128::from(floor) * i128::from(candidate.token_weight);
    let net = i128::from(candidate.value_micro) - penalty;
    i64::try_from(net).map_err(|_| PackageRefusal::invalid("net value exceeds i64"))
}

/// Lower a request to its model and effective capacity.
///
/// Variables follow the order of `candidate_id`, which also breaks every
/// remaining tie, so the model does not depend on request order.
pub fn lower(request: &ContextPackageRequest) -> Result<(ModelSpec, u64), PackageRefusal> {
    request
        .validate()
        .map_err(|(code, detail)| PackageRefusal::new(code, detail))?;
    let capacity = effective_capacity(&request.budget)?;
    let mut ordered: Vec<&ContextPackageCandidate> = request.candidates.iter().collect();
    ordered.sort_by(|a, b| a.candidate_id.cmp(&b.candidate_id));

    let mut units: BTreeMap<&str, Vec<VarId>> = BTreeMap::new();
    let mut weights = Vec::with_capacity(ordered.len());
    let mut net_terms = Vec::with_capacity(ordered.len());
    let mut token_terms = Vec::with_capacity(ordered.len());
    for (index, candidate) in ordered.iter().enumerate() {
        let var = VarId(u32::try_from(index).map_err(|_| PackageRefusal::invalid("too many"))?);
        units
            .entry(candidate.evidence_unit.as_str())
            .or_default()
            .push(var);
        let weight = to_i64(candidate.token_weight, "token weight")?;
        weights.push(Term {
            var,
            coefficient: weight,
        });
        net_terms.push(ObjectiveTerm {
            var,
            coefficient: Coefficient::Known(
                net_value(candidate, request.value_floor_micro_per_token)?
                    .checked_neg()
                    .ok_or_else(|| PackageRefusal::invalid("net value negation overflows"))?,
            ),
        });
        token_terms.push(ObjectiveTerm {
            var,
            coefficient: Coefficient::Known(weight),
        });
    }

    let mut constraints = Vec::with_capacity(units.len() + 1);
    for (unit, vars) in &units {
        constraints.push(ConstraintSpec {
            label: format!("unit:{unit}"),
            body: ConstraintBody::Linear {
                terms: vars
                    .iter()
                    .map(|&var| Term {
                        var,
                        coefficient: 1,
                    })
                    .collect(),
                relation: Relation::LessEqual,
                rhs: 1,
            },
        });
    }
    constraints.push(ConstraintSpec {
        label: "capacity".to_string(),
        body: ConstraintBody::Linear {
            terms: weights,
            relation: Relation::LessEqual,
            rhs: to_i64(capacity, "capacity")?,
        },
    });
    let spec = ModelSpec {
        variables: ordered.iter().map(|c| c.candidate_id.clone()).collect(),
        constraints,
        objective: vec![
            ObjectiveLevelSpec {
                label: "net_value".to_string(),
                terms: net_terms,
            },
            ObjectiveLevelSpec {
                label: "tokens".to_string(),
                terms: token_terms,
            },
        ],
    };
    Ok((spec, capacity))
}

/// The solved and independently verified package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolvedPackage {
    /// The selection record with its digests.
    pub selection: ContextPackageSelection,
    /// The solver certificate the verifier accepted.
    pub certificate: Certificate,
}

/// Lower, solve under `request.node_budget` and verify. A certificate that is
/// not a verified optimum with zero gap is refused; a greedy fallback never
/// qualifies.
pub fn select(request: &ContextPackageRequest) -> Result<SolvedPackage, PackageRefusal> {
    select_with(request, super::solve)
}

/// [`select`] with the solver supplied by the caller. The verifier is always
/// the independent one, so a solver that returns a corrupted certificate is
/// refused `CONTEXT_PACKAGE_CERTIFICATE_UNACCEPTABLE`.
pub fn select_with(
    request: &ContextPackageRequest,
    solver: impl FnOnce(&Model, &SolverConfig) -> Certificate,
) -> Result<SolvedPackage, PackageRefusal> {
    let (spec, capacity) = lower(request)?;
    let model = Model::try_from(spec.clone())
        .map_err(|error| PackageRefusal::invalid(format!("model refused: {error}")))?;
    let config = SolverConfig::try_from(SolverConfigSpec {
        node_budget: u64::from(request.node_budget),
        max_certificate_leaves: super::certificate::DEFAULT_CERTIFICATE_LEAVES,
        bound_denominator: 1,
        accepted_gap: Scalar::new(0),
    })
    .map_err(|error| PackageRefusal::invalid(format!("solver config refused: {error:?}")))?;
    let certificate = solver(&model, &config);
    let verdict = verify(&model, &certificate).map_err(|error| {
        PackageRefusal::new(
            ContextPackageErrorCode::CertificateUnacceptable,
            format!("certificate refused: {error}"),
        )
    })?;
    if let Verdict::Unresolved {
        incumbent,
        lower_bound,
    } = &verdict
    {
        return Err(PackageRefusal {
            code: ContextPackageErrorCode::WorkExhausted,
            detail: format!(
                "the node budget of {} was spent before an optimal package was proven",
                request.node_budget
            ),
            exhaustion: Some(PackageExhaustion {
                incumbent: *incumbent,
                lower_bound: *lower_bound,
            }),
        });
    }
    let proven = matches!(
        verdict,
        Verdict::ProvenOptimal { .. } | Verdict::OptimalityRequiresReplay { .. }
    ) && matches!(
        certificate.status,
        SolveStatus::Optimal | SolveStatus::OptimalByDeterministicSearch { .. }
    );
    let incumbent = match (&certificate.incumbent, proven) {
        (Some(incumbent), true) => incumbent,
        _ => {
            return Err(PackageRefusal::new(
                ContextPackageErrorCode::CertificateUnacceptable,
                "the solver did not prove an optimal package within its node budget",
            ))
        }
    };
    let by_id: BTreeMap<&str, &ContextPackageCandidate> = request
        .candidates
        .iter()
        .map(|c| (c.candidate_id.as_str(), c))
        .collect();
    let mut chosen = Vec::new();
    let mut total_tokens = 0_u64;
    for (name, picked) in spec.variables.iter().zip(&incumbent.selected) {
        if *picked {
            chosen.push(name.clone());
            total_tokens += by_id[name.as_str()].token_weight;
        }
    }
    let selection = ContextPackageSelection {
        chosen_ids: BoundedVec::new(chosen)
            .map_err(|_| PackageRefusal::invalid("chosen ids exceed the bound"))?,
        effective_capacity_tokens: capacity,
        total_tokens,
        total_cost_micro: request
            .budget
            .price_micro_per_token
            .and_then(|price| price.checked_mul(total_tokens)),
        total_latency_ms: request
            .budget
            .tokens_per_second
            .filter(|rate| *rate > 0)
            .and_then(|rate| total_tokens.checked_mul(1_000).map(|ms| ms / rate)),
        objective_micro: i64::try_from(
            incumbent
                .objective
                .levels
                .first()
                .map(|level| level_known_sum(level))
                .unwrap_or(0),
        )
        .unwrap_or(i64::MIN),
        input_digest: String::from(super::Sha256Digest::of_json(
            "medulla/context-package/input/v1",
            request,
        )),
        model_digest: String::from(model.digest()),
        certificate_digest: String::from(certificate.digest()),
        verified: true,
    };
    Ok(SolvedPackage {
        selection,
        certificate,
    })
}

fn level_known_sum(level: &medulla_decision_types::solve::objective::LevelValue) -> i128 {
    level.known.get()
}

#[cfg(test)]
mod tests;
