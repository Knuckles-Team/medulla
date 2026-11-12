//! Elastic topology admission against the sandbox caps
//! (MEDULLA-DECISION-ENGINE-R046.750).
//!
//! The solver already keeps a plan inside the caps its programme carries; this
//! is the independent check the sealed plan passes before the record names it.
//! A plan never grants itself more width, depth, rounds or tokens than the
//! caps, and the caps it was held to are recorded beside it.

use medulla_agent_types::agent_topology::EnforcementRecord;
use medulla_agent_types::agent_topology::TopologyCaps;
use medulla_agent_types::agent_topology::TopologyPlan;
use medulla_agent_types::agent_topology::TopologyRefusal;
use medulla_decision_types::decision::PremiseClass;
use medulla_decision_types::decision::PremiseProvenance;
use medulla_decision_types::decision::PremiseRef;

/// Hold `plan`, nested `depth` deep, to `caps`: the first slot over the width
/// cap, then the depth, then the first slot over the rounds cap, then the
/// plan's declared tokens over the tokens cap.
pub fn admit_topology(
    plan: &TopologyPlan,
    depth: u8,
    caps: &TopologyCaps,
) -> Result<EnforcementRecord, TopologyRefusal> {
    if let Some(slot) = plan.slots.iter().find(|slot| slot.width > caps.max_width) {
        return Err(TopologyRefusal::WidthExceedsCap {
            node_id: slot.node_id.clone(),
            width: slot.width,
            cap: caps.max_width,
        });
    }
    if depth > caps.max_depth {
        return Err(TopologyRefusal::DepthExceedsCap {
            depth,
            cap: caps.max_depth,
        });
    }
    if let Some(slot) = plan.slots.iter().find(|slot| slot.rounds > caps.max_rounds) {
        return Err(TopologyRefusal::RoundsExceedsCap {
            node_id: slot.node_id.clone(),
            rounds: slot.rounds,
            cap: caps.max_rounds,
        });
    }
    if let Some(cap) = caps.max_tokens {
        let tokens = plan
            .slots
            .iter()
            .filter_map(|slot| slot.tokens)
            .fold(0u64, u64::saturating_add);
        if tokens > cap {
            return Err(TopologyRefusal::TokensExceedsCap { tokens, cap });
        }
    }
    Ok(EnforcementRecord::of(plan, caps))
}

/// The premises an admission adds: one `topology_cap:<name>=<value>` per cap
/// (a definition of the policy in force) and the admitted plan's digest.
pub fn premises(record: &EnforcementRecord, policy_digest: &str) -> Vec<PremiseRef> {
    let policy = || PremiseProvenance::Policy {
        policy_digest: policy_digest.to_string(),
    };
    let mut out: Vec<PremiseRef> = record
        .cap_facts()
        .into_iter()
        .map(|(name, value)| PremiseRef {
            subject: "topology".to_string(),
            fact: format!("topology_cap:{name}={value}"),
            class: PremiseClass::Definition,
            provenance: policy(),
        })
        .collect();
    out.push(PremiseRef {
        subject: "topology".to_string(),
        fact: format!("plan_digest:{}", record.plan_digest),
        class: PremiseClass::Proof,
        provenance: policy(),
    });
    out
}
