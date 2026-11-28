//! Typed retrieval-plan selection (`MEDULLA-DECISION-ENGINE-R029`): the decision
//! ladder selects among retrieval plans through the same typed decision
//! mechanism as any other declared-candidate question
//! (`QuestionKind::RetrievalPlan`, [`super::declared`]), never a hard-coded
//! rule naming one plan.
//!
//! This module fixes WHICH plan families a routing decision may choose
//! among. The decision itself -- choosing one plan for one request -- runs
//! through the ordinary `Decide` executor over `CandidateSource::Declared`
//! options built from [`RetrievalPlanKind::ALL`]; it adds no plan-specific
//! comparison path. A plan kind here names a retrieval STRATEGY a request
//! may route to (`crates/medulla-plan`'s LeanRAG module, a reciprocal-rank-fusion
//! plan, or a direct SPARQL query); it is distinct from `Op::FuseRrf`, which
//! is an operator inside an already-chosen plan's query DAG, not a
//! candidate this decision ranks.
//!
//! [`super::retrieval`] records what a committed retrieval-plan decision's
//! run RETURNED and CITED after the fact; this module fixes what a request
//! may choose BEFORE that decision runs.

use medulla_agent_types::agent_component::ComponentDependency;
use medulla_types::refusal_code::RefusalCode;
use serde::{Deserialize, Serialize};

/// The parameter naming the retrieval provider a plan is bound to: a JSON
/// [`ComponentDependency`] pin (MEDULLA-DECISION-ENGINE-R029.1).
pub const PROVIDER_PARAM: &str = "retrieval.provider";
/// The parameter naming the reasoning model, resolved independently of the
/// provider: a JSON [`ComponentDependency`] pin.
pub const REASONING_MODEL_PARAM: &str = "retrieval.reasoning_model";

/// The provider is not served: retired, withdrawn, unknown or not visible to
/// the tenant. Medulla never substitutes another provider.
pub const RETRIEVAL_PROVIDER_UNAVAILABLE: RefusalCode =
    RefusalCode::new("RETRIEVAL_PROVIDER_UNAVAILABLE");
/// The pin's definition digest is not the provider's current revision.
pub const RETRIEVAL_PROVIDER_STALE: RefusalCode = RefusalCode::new("RETRIEVAL_PROVIDER_STALE");

/// The parameter naming the digest of the command configuration the plan runs
/// under: `sha256:` and 64 lowercase hex characters
/// (MEDULLA-DECISION-ENGINE-R029.100).
pub const COMMAND_CONFIG_DIGEST_PARAM: &str = "retrieval.command_config_digest";
/// The parameter naming the digest of the inference mode the plan runs under.
pub const INFERENCE_MODE_DIGEST_PARAM: &str = "retrieval.inference_mode_digest";

/// A command-config or inference-mode digest is not `sha256:` plus 64
/// lowercase hex characters.
pub const INFERENCE_MODE_DIGEST_INVALID: RefusalCode =
    RefusalCode::new("INFERENCE_MODE_DIGEST_INVALID");
/// One of the two digests was supplied without the other.
pub const INFERENCE_MODE_BINDING_INCOMPLETE: RefusalCode =
    RefusalCode::new("INFERENCE_MODE_BINDING_INCOMPLETE");

/// The refusal codes of the committed provider binding, in the
/// `RetrievalPlan` table.
pub const REFUSALS: &[RefusalCode] = &[
    INFERENCE_MODE_BINDING_INCOMPLETE,
    INFERENCE_MODE_DIGEST_INVALID,
    RETRIEVAL_PROVIDER_STALE,
    RETRIEVAL_PROVIDER_UNAVAILABLE,
];

/// Whether `value` is `sha256:` followed by 64 lowercase hex characters.
pub fn is_prefixed_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// The provider (and optional reasoning model) a retrieval-plan decision
/// commits to. The consumer executes only this provider
/// (MEDULLA-DECISION-ENGINE-R029.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RetrievalProviderBinding {
    pub provider: ComponentDependency,
    /// The provider revision's content digest at decision time.
    pub config_digest: String,
    /// Digest of the question and its typed parameters.
    pub request_digest: String,
    pub tenant_id: String,
    #[serde(default)]
    pub reasoning_model: Option<ComponentDependency>,
}

/// The fixed set of retrieval-plan families a routing decision may choose
/// among. Adding a plan is a registry change here, not a new code path at
/// each caller.
///
/// This registry's namespace is reserved: an option id is checked against it
/// only when the id itself starts with [`OPTION_ID_PREFIX`]. A declared
/// candidate outside that namespace (an ordinary arbitrary plan identifier,
/// as other `QuestionKind::RetrievalPlan` tests already use) is left alone,
/// so adding this gate changes nothing for existing declared-candidate
/// questions that never claimed to be a registry entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum RetrievalPlanKind {
    LeanRag,
    ReciprocalRankFusion,
    DirectSparql,
}

/// The reserved namespace prefix of every registered plan's option id. An
/// option id in this namespace is held to the registry; one outside it is
/// an ordinary declared option this module has no opinion about.
pub const OPTION_ID_PREFIX: &str = "retrieval-plan:";

impl RetrievalPlanKind {
    /// Every registered plan family, in a stable order.
    pub const ALL: [RetrievalPlanKind; 3] = [
        Self::LeanRag,
        Self::ReciprocalRankFusion,
        Self::DirectSparql,
    ];

    /// The declared-option id a decision candidate for this plan carries.
    pub fn option_id(self) -> &'static str {
        match self {
            Self::LeanRag => "retrieval-plan:leanrag",
            Self::ReciprocalRankFusion => "retrieval-plan:reciprocal-rank-fusion",
            Self::DirectSparql => "retrieval-plan:direct-sparql",
        }
    }

    /// Whether `option_id` claims membership in this registry's namespace,
    /// whether or not it actually names a registered plan.
    pub fn claims_registry_namespace(option_id: &str) -> bool {
        option_id.starts_with(OPTION_ID_PREFIX)
    }

    /// The plan family an option id names, or `None` outside the registry.
    pub fn from_option_id(option_id: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|plan| plan.option_id() == option_id)
    }
}

/// One request's candidate retrieval plans, as the option ids the caller
/// intends to offer a routing decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RetrievalPlanRequest {
    pub candidate_plans: Vec<String>,
}

impl RetrievalPlanRequest {
    /// At least one candidate, and every candidate option id names a plan in
    /// [`RetrievalPlanKind::ALL`].
    pub fn check(&self) -> Result<(), String> {
        if self.candidate_plans.is_empty() {
            return Err("a retrieval-plan request names at least one candidate plan".to_string());
        }
        for option_id in &self.candidate_plans {
            if RetrievalPlanKind::from_option_id(option_id).is_none() {
                return Err(format!(
                    "UNSUPPORTED_RETRIEVAL_PLAN: '{option_id}' is not in the retrieval plan \
                     registry (leanrag, reciprocal-rank-fusion, direct-sparql)"
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::code_registry;

    // spec: MEDULLA-DECISION-ENGINE-R029.1
    #[test]
    fn binding_refusals_are_registered() {
        for code in REFUSALS {
            assert_eq!(code_registry::resolve(code.as_str()), Some(*code));
        }
    }

    #[test]
    fn every_plan_option_id_resolves_back_to_its_plan() {
        for plan in RetrievalPlanKind::ALL {
            assert_eq!(
                RetrievalPlanKind::from_option_id(plan.option_id()),
                Some(plan)
            );
        }
    }

    #[test]
    fn only_the_reserved_prefix_claims_the_namespace() {
        assert!(RetrievalPlanKind::claims_registry_namespace(
            "retrieval-plan:leanrag"
        ));
        assert!(RetrievalPlanKind::claims_registry_namespace(
            "retrieval-plan:vector-only"
        ));
        assert!(!RetrievalPlanKind::claims_registry_namespace("plan-hyde"));
        assert!(!RetrievalPlanKind::claims_registry_namespace("plan-deep"));
    }

    // spec: MEDULLA-DECISION-ENGINE-R029
    #[test]
    fn a_request_naming_only_registered_plans_is_accepted() {
        let request = RetrievalPlanRequest {
            candidate_plans: vec![
                RetrievalPlanKind::LeanRag.option_id().to_string(),
                RetrievalPlanKind::DirectSparql.option_id().to_string(),
            ],
        };
        assert!(request.check().is_ok());
    }

    // spec: MEDULLA-DECISION-ENGINE-R029
    #[test]
    fn a_request_naming_an_unregistered_plan_is_refused() {
        let request = RetrievalPlanRequest {
            candidate_plans: vec!["retrieval-plan:vector-only".to_string()],
        };
        let error = request.check().expect_err("must be refused");
        assert!(error.contains("UNSUPPORTED_RETRIEVAL_PLAN"), "got: {error}");
    }

    #[test]
    fn an_empty_request_is_refused() {
        assert!(RetrievalPlanRequest {
            candidate_plans: vec![]
        }
        .check()
        .is_err());
    }
}
