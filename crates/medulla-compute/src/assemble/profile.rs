//! The assistant-profile constraint on an assembly
//! (MEDULLA-DECISION-ENGINE-R036.107).
//!
//! The handler removes the tool candidates outside the profile's allowlist
//! before the solve and records what it removed in `DecisionInputs::profile`.
//! This module reads that record: the sealed decision lists the premise
//! `profile:<id>@<revision>`, and a required capability that only a removed
//! tool could cover abstains with `PROFILE_TOOL_NOT_ALLOWED` rather than the
//! generic uncovered-capability reason.

use medulla_agent_types::agent_ontology::is_a;
use medulla_decision_types::decision::record::outcome::{
    AbstainReason, DecisionRefusal, RefusalCode, RefusalOrigin,
};
use medulla_decision_types::decision::{
    DecisionInputs, DecisionOutcome, PremiseClass, PremiseProvenance, PremiseRef,
};
use medulla_types::contract::BoundedVec;

use super::conclude::Decided;

const PROFILE_TOOL_NOT_ALLOWED: RefusalCode = RefusalCode::new("PROFILE_TOOL_NOT_ALLOWED");

struct NotAllowed {
    capability: String,
}

impl DecisionRefusal for NotAllowed {
    fn code(&self) -> RefusalCode {
        PROFILE_TOOL_NOT_ALLOWED
    }

    fn detail(&self) -> String {
        format!(
            "the profile's tool allowlist leaves no tool for required capability `{}`",
            self.capability
        )
    }
}

/// The premise a profile-constrained decision lists.
pub(super) fn premise(inputs: &DecisionInputs) -> Option<PremiseRef> {
    let profile = inputs.profile.as_ref()?;
    Some(PremiseRef {
        subject: format!("profile:{}@{}", profile.profile_id, profile.revision),
        fact: "tool_allowlist".to_string(),
        class: PremiseClass::Definition,
        provenance: PremiseProvenance::Request {
            field: "assistant_profile_ref".to_string(),
        },
    })
}

/// Report an uncovered capability that the allowlist alone made uncoverable
/// under the profile's code; every other reason is kept as it is.
pub(super) fn attribute(inputs: &DecisionInputs, decided: &mut Decided) {
    let Some(profile) = &inputs.profile else {
        return;
    };
    let DecisionOutcome::Abstained { reasons } = &decided.outcome else {
        return;
    };
    let mapped: Vec<AbstainReason> = reasons
        .iter()
        .map(|reason| match reason {
            AbstainReason::UncoveredCapability { iri }
                if profile
                    .blocked_capabilities
                    .iter()
                    .any(|blocked| blocked == iri || is_a(blocked, iri)) =>
            {
                AbstainReason::from_refusal(
                    RefusalOrigin::ASSEMBLE,
                    &NotAllowed {
                        capability: iri.clone(),
                    },
                )
            }
            other => other.clone(),
        })
        .collect();
    decided.outcome = DecisionOutcome::Abstained {
        reasons: BoundedVec::new(mapped).expect("the same number of reasons"),
    };
}
