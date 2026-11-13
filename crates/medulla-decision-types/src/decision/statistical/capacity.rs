//! Capacity adjustment proposals and their verdict (MEDULLA-DECISION-ENGINE-R824).
//!
//! A proposal asks to move one tenant's capacity for one resource class from
//! `current_capacity` to `proposed_capacity`. The verdict is `allow` or
//! `refuse` with exactly one [`CapacityRefusalCode`]. The engine, never the
//! caller, supplies the [`Observation`]: the headroom and the cell epoch the
//! capacity ledger reports now. The checks run in the order the codes are
//! declared, so a proposal with several faults reports the first.

use serde::{Deserialize, Serialize};

use crate::decision::record::outcome::RefusalCode;

/// Text parameter: the graph whose capacity ledger holds the cell.
pub const PARAM_GRAPH: &str = "graph";
/// JSON text parameter: the [`CapacityAdjustmentProposal`].
pub const PARAM_PROPOSAL: &str = "proposal";
/// JSON text parameter the engine fills with the [`Observation`]; a caller
/// that supplies it is refused.
pub const PARAM_OBSERVATION: &str = "observation";
/// The one option a capacity proposal question declares.
pub const VERDICT_OPTION: &str = "verdict";
/// The answer that allows the proposal.
pub const ALLOW: &str = "allow";
/// Prefix of the answer that refuses it, followed by the code.
pub const REFUSE_PREFIX: &str = "refuse:";

/// One proposed capacity change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapacityAdjustmentProposal {
    pub proposal_id: String,
    pub tenant_id: String,
    pub resource_class_iri: String,
    pub unit: String,
    pub current_capacity: u64,
    pub proposed_capacity: u64,
    pub min_capacity: u64,
    pub max_capacity: u64,
    pub measured_load: u64,
    pub available_headroom: u64,
    pub window_start_ms: u64,
    pub window_end_ms: u64,
    pub premise_revision: u64,
    pub policy_revision: u64,
    /// SHA-256 of the proposal, 64 lowercase hex characters.
    pub proposal_digest: String,
}

/// What the capacity ledger reports for the proposal's cell now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    /// The wire (snake_case) name of the cell's resource class, the unit the
    /// proposal must use.
    pub unit: String,
    pub available: u64,
    pub epoch: u64,
}

/// Why a proposal is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityRefusalCode {
    DigestInvalid,
    MeasurementInvalid,
    PremiseStale,
    TenantMismatch,
    UnitMismatch,
    WindowInvalid,
    LimitInvalid,
    LimitExceeded,
    HeadroomExceeded,
}

impl CapacityRefusalCode {
    /// Every code, in the order the checks run.
    pub const ALL: [Self; 9] = [
        Self::DigestInvalid,
        Self::TenantMismatch,
        Self::PremiseStale,
        Self::MeasurementInvalid,
        Self::UnitMismatch,
        Self::WindowInvalid,
        Self::LimitInvalid,
        Self::LimitExceeded,
        Self::HeadroomExceeded,
    ];

    /// The registered refusal code.
    pub const fn code(self) -> RefusalCode {
        RefusalCode::new(match self {
            Self::DigestInvalid => "CAPACITY_PROPOSAL_DIGEST_INVALID",
            Self::MeasurementInvalid => "CAPACITY_MEASUREMENT_INVALID",
            Self::PremiseStale => "CAPACITY_PREMISE_STALE",
            Self::TenantMismatch => "CAPACITY_TENANT_MISMATCH",
            Self::UnitMismatch => "CAPACITY_UNIT_MISMATCH",
            Self::WindowInvalid => "CAPACITY_WINDOW_INVALID",
            Self::LimitInvalid => "CAPACITY_LIMIT_INVALID",
            Self::LimitExceeded => "CAPACITY_LIMIT_EXCEEDED",
            Self::HeadroomExceeded => "CAPACITY_HEADROOM_EXCEEDED",
        })
    }
}

/// The family's refusal table, registered in `codes::FAMILY_TABLES`.
pub const REFUSALS: &[RefusalCode] = &[
    CapacityRefusalCode::DigestInvalid.code(),
    CapacityRefusalCode::MeasurementInvalid.code(),
    CapacityRefusalCode::PremiseStale.code(),
    CapacityRefusalCode::TenantMismatch.code(),
    CapacityRefusalCode::UnitMismatch.code(),
    CapacityRefusalCode::WindowInvalid.code(),
    CapacityRefusalCode::LimitInvalid.code(),
    CapacityRefusalCode::LimitExceeded.code(),
    CapacityRefusalCode::HeadroomExceeded.code(),
];

/// The verdict on one proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityAdjustmentVerdict {
    Allow,
    Refuse(CapacityRefusalCode),
}

impl CapacityAdjustmentVerdict {
    /// The option id the verdict is answered as.
    pub fn option_id(self) -> String {
        match self {
            Self::Allow => ALLOW.to_string(),
            Self::Refuse(code) => format!("{REFUSE_PREFIX}{}", code.code().as_str()),
        }
    }
}

fn is_digest(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The verdict of `proposal` made for `tenant_id` against what the ledger
/// reports now.
pub fn judge(
    proposal: &CapacityAdjustmentProposal,
    tenant_id: &str,
    observed: &Observation,
) -> CapacityAdjustmentVerdict {
    use CapacityAdjustmentVerdict::{Allow, Refuse};
    use CapacityRefusalCode::*;
    if !is_digest(&proposal.proposal_digest) {
        return Refuse(DigestInvalid);
    }
    if proposal.tenant_id != tenant_id {
        return Refuse(TenantMismatch);
    }
    if proposal.premise_revision != observed.epoch {
        return Refuse(PremiseStale);
    }
    // A load above the capacity it runs on, or a headroom claim above what the
    // ledger reports, is not a measurement.
    if proposal.measured_load > proposal.current_capacity
        || proposal.available_headroom > observed.available
    {
        return Refuse(MeasurementInvalid);
    }
    if proposal.unit != observed.unit {
        return Refuse(UnitMismatch);
    }
    if proposal.window_start_ms >= proposal.window_end_ms {
        return Refuse(WindowInvalid);
    }
    if proposal.min_capacity > proposal.max_capacity
        || proposal.current_capacity < proposal.min_capacity
        || proposal.current_capacity > proposal.max_capacity
    {
        return Refuse(LimitInvalid);
    }
    if proposal.proposed_capacity < proposal.min_capacity
        || proposal.proposed_capacity > proposal.max_capacity
    {
        return Refuse(LimitExceeded);
    }
    if proposal
        .proposed_capacity
        .saturating_sub(proposal.current_capacity)
        > proposal.available_headroom
    {
        return Refuse(HeadroomExceeded);
    }
    Allow
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn proposal() -> CapacityAdjustmentProposal {
        CapacityAdjustmentProposal {
            proposal_id: "p-1".into(),
            tenant_id: "t".into(),
            resource_class_iri: "cell-1".into(),
            unit: "llm_generator".into(),
            current_capacity: 10,
            proposed_capacity: 14,
            min_capacity: 2,
            max_capacity: 20,
            measured_load: 8,
            available_headroom: 5,
            window_start_ms: 1,
            window_end_ms: 2,
            premise_revision: 3,
            policy_revision: 1,
            proposal_digest: "a".repeat(64),
        }
    }

    pub(super) fn observed() -> Observation {
        Observation {
            unit: "llm_generator".into(),
            available: 6,
            epoch: 3,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R824
    #[test]
    fn each_fault_reports_its_own_code_and_a_clean_proposal_is_allowed() {
        use CapacityAdjustmentVerdict::{Allow, Refuse};
        use CapacityRefusalCode::*;
        assert_eq!(judge(&proposal(), "t", &observed()), Allow);
        let faulty: [(Box<dyn Fn(&mut CapacityAdjustmentProposal)>, _); 8] = [
            (
                Box::new(|p| p.proposal_digest = "ABC".into()),
                DigestInvalid,
            ),
            (Box::new(|p| p.premise_revision = 2), PremiseStale),
            (Box::new(|p| p.measured_load = 11), MeasurementInvalid),
            (Box::new(|p| p.unit = "gpu".into()), UnitMismatch),
            (Box::new(|p| p.window_end_ms = 1), WindowInvalid),
            (
                Box::new(|p| {
                    p.current_capacity = 1;
                    p.measured_load = 0;
                }),
                LimitInvalid,
            ),
            (Box::new(|p| p.proposed_capacity = 21), LimitExceeded),
            (Box::new(|p| p.proposed_capacity = 16), HeadroomExceeded),
        ];
        for (mutate, code) in faulty {
            let mut p = proposal();
            mutate(&mut p);
            assert_eq!(judge(&p, "t", &observed()), Refuse(code), "{code:?}");
        }
        assert_eq!(
            judge(&proposal(), "other", &observed()),
            Refuse(TenantMismatch)
        );
        assert_eq!(CapacityRefusalCode::ALL.len(), REFUSALS.len());
    }
}
