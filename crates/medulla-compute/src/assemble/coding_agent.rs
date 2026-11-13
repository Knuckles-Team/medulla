//! Composing a verified coding agent with an ordered list of skill
//! transformations (MEDULLA-DECISION-ENGINE-R036.101).
//!
//! The caller supplies the agent fact, its discovery receipt and the skills;
//! this module only checks and orders what it is given and never scans a path.
//! Adjacent transformations must type-check as primitive edges
//! ([`super::primitive::edge_type_checks`]); the last one must produce the
//! artifact contract digest the plan binds. Pure: the receipt age limit is the
//! resolved `coding_agent.max_receipt_age` knob, recorded in the inputs.

use std::collections::BTreeSet;

use medulla_decision_types::decision::codes;
use medulla_decision_types::decision::coding_agent::{
    CodingAgentPlan, CodingAgentRequest, SkillTransformation,
};
use medulla_decision_types::decision::primitive::{PrimitiveContract, PrimitiveKind};
use medulla_decision_types::decision::record::outcome::{
    AbstainReason, DecisionRefusal, RefusalCode, RefusalOrigin,
};
use medulla_decision_types::decision::{DecisionErrorCode, DecisionInputs};
use medulla_decision_types::decision::{PremiseClass, PremiseProvenance, PremiseRef};

use super::primitive::edge_type_checks;
use super::AssembleError;

/// Why a coding-agent composition is refused; the code is registered in
/// `medulla_decision_types::decision::codes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodingAgentRefusal {
    pub code: RefusalCode,
    pub detail: String,
}

impl DecisionRefusal for CodingAgentRefusal {
    fn code(&self) -> RefusalCode {
        self.code
    }

    fn detail(&self) -> String {
        self.detail.clone()
    }
}

fn refuse(code: RefusalCode, detail: String) -> CodingAgentRefusal {
    CodingAgentRefusal { code, detail }
}

fn contract(skill: &SkillTransformation) -> PrimitiveContract {
    PrimitiveContract {
        primitive_id: skill.skill_id.clone(),
        kind: PrimitiveKind::Tool,
        version: skill.version.clone(),
        input_schema_digest: skill.input_schema_digest.clone(),
        output_schema_digest: skill.output_schema_digest.clone(),
        effects: BTreeSet::new(),
        capabilities: BTreeSet::new(),
    }
}

/// The plan for `request`, or the first refusal: a missing receipt, a stale
/// receipt, adjacent skills whose schemas differ, then a last skill that does
/// not produce the artifact contract.
pub fn compose_coding_agent_plan(
    request: &CodingAgentRequest,
    max_receipt_age_s: u64,
) -> Result<CodingAgentPlan, CodingAgentRefusal> {
    let agent = &request.agent;
    if agent.discovery_receipt_digest.trim().is_empty() {
        return Err(refuse(
            codes::DISCOVERY_RECEIPT_MISSING,
            format!("agent `{}` has no discovery receipt", agent.agent_id),
        ));
    }
    if agent.discovery_age_s > max_receipt_age_s {
        return Err(refuse(
            codes::DISCOVERY_RECEIPT_STALE,
            format!(
                "the discovery of `{}` is {} s old, over coding_agent.max_receipt_age ({max_receipt_age_s} s)",
                agent.agent_id, agent.discovery_age_s
            ),
        ));
    }
    let skills = request.skills.as_slice();
    for pair in skills.windows(2) {
        if !edge_type_checks(&contract(&pair[0]), &contract(&pair[1])) {
            return Err(refuse(
                codes::TRANSFORMATION_SCHEMA_MISMATCH,
                format!(
                    "`{}` output does not match `{}` input",
                    pair[0].skill_id, pair[1].skill_id
                ),
            ));
        }
    }
    if let Some(last) = skills.last() {
        if last.output_schema_digest != request.artifact_contract_digest {
            return Err(refuse(
                codes::ARTIFACT_DIGEST_CHANGED,
                format!(
                    "`{}` does not produce the artifact contract the plan binds",
                    last.skill_id
                ),
            ));
        }
    }
    Ok(CodingAgentPlan {
        agent_id: agent.agent_id.clone(),
        agent_revision: agent.revision.clone(),
        discovery_receipt_digest: agent.discovery_receipt_digest.clone(),
        skills: request.skills.clone(),
        artifact_contract_digest: request.artifact_contract_digest.clone(),
    })
}

/// The abstention reasons of the request's coding-agent question; `None` when
/// the request carries none or the plan composes.
pub(super) fn refusals(
    inputs: &DecisionInputs,
) -> Result<Option<Vec<AbstainReason>>, AssembleError> {
    let Some(request) = &inputs.request.coding_agent else {
        return Ok(None);
    };
    let limit = inputs.receipt_age_limit_s.ok_or_else(|| {
        AssembleError::new(
            DecisionErrorCode::AssemblyInputsInvalid,
            "a coding-agent question records the resolved coding_agent.max_receipt_age",
        )
    })?;
    Ok(compose_coding_agent_plan(request, limit)
        .err()
        .map(|refusal| {
            vec![AbstainReason::from_refusal(
                RefusalOrigin::ASSEMBLE,
                &refusal,
            )]
        }))
}

fn claim(subject: String, fact: &str) -> PremiseRef {
    PremiseRef {
        subject,
        fact: fact.to_string(),
        class: PremiseClass::Claim,
        provenance: PremiseProvenance::Request {
            field: "coding_agent".to_string(),
        },
    }
}

/// The premises the question rests on, each a claim the caller made: the
/// agent at its revision, its receipt and every skill at its version.
pub(super) fn premises(inputs: &DecisionInputs) -> Vec<PremiseRef> {
    let Some(request) = &inputs.request.coding_agent else {
        return Vec::new();
    };
    let agent = &request.agent;
    let mut out = vec![claim(
        format!("agent:{}@{}", agent.agent_id, agent.revision),
        "verified_agent",
    )];
    if !agent.discovery_receipt_digest.trim().is_empty() {
        out.push(claim(
            format!("receipt:{}", agent.discovery_receipt_digest),
            "discovery_receipt",
        ));
    }
    out.extend(request.skills.iter().map(|skill| {
        claim(
            format!("skill:{}@{}", skill.skill_id, skill.version),
            "skill_transformation",
        )
    }));
    out
}
