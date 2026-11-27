//! The `council_coordination` question family (MEDULLA-DECISION-ENGINE-R106.1).
//!
//! Stage 7 (conclude): the positions of one council, pinned in the request with
//! the council policy and the current evidence heads, are checked in a fixed
//! order and counted per option. The best-supported option is selected only
//! above the policy's quorum; a tie of the two best or a thin majority abstains.
//! Medulla reads no store and no topology. Evaluate-only: `DecisionCommit`
//! persists the record.

use medulla_decision_types::decision::council::{
    check, conclude, quorum_floor_micros, CouncilAbstention, CouncilInput, CouncilOutcome,
    CouncilPolicy, CouncilPosition, CouncilRefusal, EvidenceRef, ANSWER_PREFIX, COUNCIL_OPTION,
    PARAM_COUNCIL_ID, PARAM_COUNCIL_POLICY, PARAM_EVIDENCE_CURRENT, PARAM_POSITIONS,
    PARAM_TOPOLOGY_REVISION,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, StatisticalOutcome,
};
use medulla_decision_types::decision::{DecisionPolicy, PremiseClass, PremiseRef, ResolutionKind};
use medulla_graph_core::graph::GraphView;
use medulla_types::evidence_class::EvidenceClass;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};

use super::support::unsigned_param;
use super::support::{abstained, advisory, invalid, json_param, single_option, text_param};
use super::DecisionFamily;

/// The family of the `council_coordination` question kind.
pub struct CouncilFamily;

/// What the request pinned, owned: the borrowed [`CouncilInput`] points into it.
struct CouncilRequest {
    council_id: String,
    topology_revision: u64,
    positions: Vec<CouncilPosition>,
    policy: CouncilPolicy,
    evidence_current: Vec<EvidenceRef>,
}

impl CouncilRequest {
    fn read(request: &DecideRequest) -> Result<Self, String> {
        let council_id = text_param(request, PARAM_COUNCIL_ID)?
            .ok_or_else(|| invalid(format!("parameter {PARAM_COUNCIL_ID} is required")))?
            .to_string();
        let topology_revision = unsigned_param(request, PARAM_TOPOLOGY_REVISION)?
            .ok_or_else(|| invalid(format!("parameter {PARAM_TOPOLOGY_REVISION} is required")))?;
        Ok(Self {
            council_id,
            topology_revision,
            positions: json_param(request, PARAM_POSITIONS)?,
            policy: json_param(request, PARAM_COUNCIL_POLICY)?,
            evidence_current: json_param(request, PARAM_EVIDENCE_CURRENT)?,
        })
    }

    fn input(&self) -> CouncilInput<'_> {
        CouncilInput {
            council_id: &self.council_id,
            topology_revision: self.topology_revision,
            positions: &self.positions,
            policy: &self.policy,
            evidence_current: &self.evidence_current,
        }
    }

    /// The premises the decision stands on: every position, every cited
    /// evidence revision and the council policy. A cited ref equals its current
    /// head once the request passed its checks, so each is an observation.
    fn premises(&self) -> Vec<FamilyPremise> {
        let premise = |fact: String, class| FamilyPremise {
            subject: "council_coordination".into(),
            fact,
            class,
            source: FamilyPremiseSource::Request(PARAM_POSITIONS),
        };
        let mut premises = Vec::new();
        for position in &self.positions {
            premises.push(premise(
                format!("position:{}", position.position_id),
                PremiseClass::Claim,
            ));
            for cited in &position.evidence_refs {
                premises.push(premise(
                    format!("evidence:{}@{}", cited.id, cited.revision),
                    PremiseClass::Observation,
                ));
            }
        }
        premises.push(premise(
            format!(
                "council_policy:{}@{}",
                self.policy.policy_id, self.policy.revision
            ),
            PremiseClass::Claim,
        ));
        let mut seen = std::collections::BTreeSet::new();
        premises.retain(|premise| seen.insert(premise.fact.clone()));
        premises
    }
}

impl DecisionFamily for CouncilFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, COUNCIL_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        let pinned = CouncilRequest::read(request)?;
        // The floor is the knob's default here: `constraints` sees no policy.
        // `conclude` re-checks under the pinned policy's own floor.
        check(&pinned.input(), 500_000).map_err(|refused| refused.text())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some((|| {
            let pinned = CouncilRequest::read(request)?;
            let input = pinned.input();
            check(&input, quorum_floor_micros(policy)).map_err(|refused| refused.text())?;
            let result = conclude(&input)?;
            let mut executed = match (&result.outcome, result.abstention) {
                (Some(CouncilOutcome::Selected { .. }), _) => {
                    let json = serde_json::to_string(&result).map_err(invalid)?;
                    advisory(vec![format!("{ANSWER_PREFIX}{json}")])?
                }
                (None, Some(abstention)) => abstained(
                    QuestionKind::CouncilCoordination,
                    &CouncilRefusal::new(abstention.code(), abstention_detail(abstention)),
                ),
                (None, None) => return Err(invalid("a council result carries no outcome")),
            };
            executed.family_premises = pinned.premises();
            Ok(executed)
        })())
    }

    fn premises(
        &self,
        request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        let rebuilt;
        let premises = if executed.family_premises.is_empty() {
            let Ok(pinned) = CouncilRequest::read(request) else {
                return Vec::new();
            };
            rebuilt = pinned.premises();
            &rebuilt
        } else {
            &executed.family_premises
        };
        premises
            .iter()
            .map(|premise| premise.seal(policy_digest))
            .collect()
    }

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        Some(match outcome {
            StatisticalOutcome::Abstained { .. } => ResolutionKind::Abstention,
            _ => ResolutionKind::Constraint,
        })
    }

    fn evidence_class(&self, _outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        Some(EvidenceClass::Observation)
    }
}

fn abstention_detail(abstention: CouncilAbstention) -> &'static str {
    match abstention {
        CouncilAbstention::NoQuorum => "the best-supported option does not reach the quorum",
        CouncilAbstention::UnresolvedConflict => {
            "the two best-supported options hold equally many positions"
        }
    }
}
