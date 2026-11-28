//! Council positions coordinated into one evidence-bound decision
//! (MEDULLA-DECISION-ENGINE-R106.1).
//!
//! `Decide` with
//! [`QuestionKind::CouncilCoordination`](super::statistical::QuestionKind::CouncilCoordination)
//! decides from what the caller pins in the request: the positions, the
//! council policy and the current head of every evidence id the caller holds.
//! Medulla reads no store and no topology, so every check is over request text
//! and is deterministic. `Decide` writes nothing; `DecisionCommit` persists
//! the record.

use std::collections::{BTreeMap, BTreeSet};

use medulla_types::contract::Digest256;
use medulla_types::refusal_code::RefusalCode;
use serde::{Deserialize, Serialize};

use medulla_decision_statistical_types::decision::policy::DecisionPolicy;
use medulla_decision_statistical_types::decision::record::outcome::refusal::DecisionRefusal;
use {
    medulla_decision_statistical_types::decision::knobs::KnobSafety,
    medulla_decision_statistical_types::decision::knobs::KnobSpec,
    medulla_decision_statistical_types::decision::knobs::KnobValue,
};

/// No position, an empty id or digest, or a position of another council.
pub const COUNCIL_INCOMPLETE_INPUT: &str = "COUNCIL_INCOMPLETE_INPUT";
/// Two positions share a position id or a persona id.
pub const COUNCIL_DUPLICATE_POSITION: &str = "COUNCIL_DUPLICATE_POSITION";
/// A position names a topology revision other than the request's.
pub const COUNCIL_TOPOLOGY_MISMATCH: &str = "COUNCIL_TOPOLOGY_MISMATCH";
/// A position has no evidence, or cites an id the caller does not hold.
pub const COUNCIL_MISSING_EVIDENCE: &str = "COUNCIL_MISSING_EVIDENCE";
/// A position cites another revision or digest than the current head.
pub const COUNCIL_STALE_EVIDENCE: &str = "COUNCIL_STALE_EVIDENCE";
/// The council policy is empty, unrevised or has an out-of-range quorum.
pub const COUNCIL_POLICY_REFUSED: &str = "COUNCIL_POLICY_REFUSED";
/// The two best-supported options hold equally many positions.
pub const COUNCIL_UNRESOLVED_CONFLICT: &str = "COUNCIL_UNRESOLVED_CONFLICT";
/// The best-supported option does not reach the quorum.
pub const COUNCIL_NO_QUORUM: &str = "COUNCIL_NO_QUORUM";

/// The one option a council question declares.
pub const COUNCIL_OPTION: &str = "council";
/// Request parameter: the council id (text).
pub const PARAM_COUNCIL_ID: &str = "council_id";
/// Request parameter: the current topology revision (int).
pub const PARAM_TOPOLOGY_REVISION: &str = "topology_revision";
/// Request parameter: the positions (JSON text of `Vec<CouncilPosition>`).
pub const PARAM_POSITIONS: &str = "positions";
/// Request parameter: the council policy (JSON text of [`CouncilPolicy`]).
pub const PARAM_COUNCIL_POLICY: &str = "council_policy";
/// Request parameter: the current evidence heads (JSON text of `Vec<EvidenceRef>`).
pub const PARAM_EVIDENCE_CURRENT: &str = "evidence_current";
/// Prefix of the answer option: the JSON of the [`CouncilResult`] follows.
pub const ANSWER_PREFIX: &str = "council_result:";

/// Every refusal code of the family; joins `code_registry::FAMILY_TABLES` under
/// `QuestionKind::CouncilCoordination`.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new(COUNCIL_INCOMPLETE_INPUT),
    RefusalCode::new(COUNCIL_DUPLICATE_POSITION),
    RefusalCode::new(COUNCIL_TOPOLOGY_MISMATCH),
    RefusalCode::new(COUNCIL_MISSING_EVIDENCE),
    RefusalCode::new(COUNCIL_STALE_EVIDENCE),
    RefusalCode::new(COUNCIL_POLICY_REFUSED),
    RefusalCode::new(COUNCIL_UNRESOLVED_CONFLICT),
    RefusalCode::new(COUNCIL_NO_QUORUM),
];

/// The `council.*` knobs: the least quorum a council policy may set.
pub const KNOBS: &[KnobSpec] = &[KnobSpec {
    name: "council.quorum_fraction",
    unit: "fraction",
    default: KnobValue::DecimalMicros(500_000),
    min: KnobValue::DecimalMicros(500_000),
    max: KnobValue::DecimalMicros(1_000_000),
    safety: KnobSafety::Tunable,
}];

/// The least quorum, in micros, the pinned policy allows: the knob, or its
/// default.
pub fn quorum_floor_micros(policy: &DecisionPolicy) -> u32 {
    match policy.knob(&KNOBS[0]) {
        KnobValue::DecimalMicros(micros) => micros.clamp(500_000, 1_000_000) as u32,
        _ => 500_000,
    }
}

/// One evidence item at an exact revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRef {
    pub id: String,
    pub revision: u64,
    pub digest: String,
}

/// One persona's position on one option.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CouncilPosition {
    pub council_id: String,
    pub topology_revision: u64,
    pub position_id: String,
    pub persona_id: String,
    pub persona_revision: u64,
    pub option_id: String,
    pub claim_digest: String,
    pub evidence_refs: Vec<EvidenceRef>,
    pub constraint_refs: Vec<String>,
}

/// The policy the council decides under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CouncilPolicy {
    pub policy_id: String,
    pub revision: u64,
    pub quorum_micros: u32,
}

/// What a council selected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "outcome", rename_all = "snake_case")]
pub enum CouncilOutcome {
    Selected { option_id: String },
}

/// Why a council selected nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CouncilAbstention {
    NoQuorum,
    UnresolvedConflict,
}

impl CouncilAbstention {
    /// The registered refusal code of this abstention.
    pub const fn code(self) -> &'static str {
        match self {
            Self::NoQuorum => COUNCIL_NO_QUORUM,
            Self::UnresolvedConflict => COUNCIL_UNRESOLVED_CONFLICT,
        }
    }
}

/// The decision over one council's positions. Exactly one of `outcome` and
/// `abstention` is set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CouncilResult {
    pub council_id: String,
    pub topology_revision: u64,
    pub input_position_ids: Vec<String>,
    pub accepted_position_ids: Vec<String>,
    pub rejected_position_ids: Vec<String>,
    pub outcome: Option<CouncilOutcome>,
    pub abstention: Option<CouncilAbstention>,
    pub policy_revision: u64,
    pub positions_digest: String,
    pub decision_digest: String,
}

/// Why a council request is refused or abstains: one registered code and its
/// detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CouncilRefusal {
    pub code: RefusalCode,
    pub detail: String,
}

impl CouncilRefusal {
    /// A refusal under `code`, which must be one of [`REFUSALS`].
    pub fn new(code: &'static str, detail: impl std::fmt::Display) -> Self {
        Self {
            code: RefusalCode::new(code),
            detail: detail.to_string(),
        }
    }

    /// The `"<CODE>: detail"` text of a request refusal.
    pub fn text(&self) -> String {
        format!("{}: {}", self.code.as_str(), self.detail)
    }
}

impl DecisionRefusal for CouncilRefusal {
    fn code(&self) -> RefusalCode {
        self.code
    }

    fn detail(&self) -> String {
        self.detail.clone()
    }
}

/// What the caller pinned in one council request.
#[derive(Debug, Clone, Copy)]
pub struct CouncilInput<'a> {
    pub council_id: &'a str,
    pub topology_revision: u64,
    pub positions: &'a [CouncilPosition],
    pub policy: &'a CouncilPolicy,
    pub evidence_current: &'a [EvidenceRef],
}

/// The request checks, in their fixed order; the first failure refuses.
/// `quorum_floor_micros` is [`quorum_floor_micros`] of the pinned policy.
pub fn check(input: &CouncilInput<'_>, quorum_floor_micros: u32) -> Result<(), CouncilRefusal> {
    let incomplete = |detail: &str| CouncilRefusal::new(COUNCIL_INCOMPLETE_INPUT, detail);
    if input.council_id.is_empty() || input.positions.is_empty() {
        return Err(incomplete(
            "a council request names its council and positions",
        ));
    }
    for position in input.positions {
        if position.position_id.is_empty()
            || position.persona_id.is_empty()
            || position.option_id.is_empty()
            || position.claim_digest.is_empty()
        {
            return Err(incomplete("a position carries an empty id or digest"));
        }
        if position.council_id != input.council_id {
            return Err(incomplete(&format!(
                "position {} belongs to council {}",
                position.position_id, position.council_id
            )));
        }
    }
    let (mut position_ids, mut persona_ids) = (BTreeSet::new(), BTreeSet::new());
    for position in input.positions {
        if !position_ids.insert(position.position_id.as_str())
            || !persona_ids.insert(position.persona_id.as_str())
        {
            return Err(CouncilRefusal::new(
                COUNCIL_DUPLICATE_POSITION,
                format!(
                    "position {} repeats a position id or persona id",
                    position.position_id
                ),
            ));
        }
    }
    if let Some(position) = input
        .positions
        .iter()
        .find(|p| p.topology_revision != input.topology_revision)
    {
        return Err(CouncilRefusal::new(
            COUNCIL_TOPOLOGY_MISMATCH,
            format!(
                "position {} names topology revision {}, current is {}",
                position.position_id, position.topology_revision, input.topology_revision
            ),
        ));
    }
    let heads: BTreeMap<&str, &EvidenceRef> = input
        .evidence_current
        .iter()
        .map(|head| (head.id.as_str(), head))
        .collect();
    for position in input.positions {
        if position.evidence_refs.is_empty()
            || position
                .evidence_refs
                .iter()
                .any(|cited| !heads.contains_key(cited.id.as_str()))
        {
            return Err(CouncilRefusal::new(
                COUNCIL_MISSING_EVIDENCE,
                format!("position {} cites missing evidence", position.position_id),
            ));
        }
    }
    for position in input.positions {
        for cited in &position.evidence_refs {
            let head = heads[cited.id.as_str()];
            if head.revision != cited.revision || head.digest != cited.digest {
                return Err(CouncilRefusal::new(
                    COUNCIL_STALE_EVIDENCE,
                    format!(
                        "position {} cites evidence {} at a superseded revision",
                        position.position_id, cited.id
                    ),
                ));
            }
        }
    }
    let policy = input.policy;
    if policy.policy_id.is_empty()
        || policy.revision == 0
        || policy.quorum_micros < quorum_floor_micros
        || policy.quorum_micros > 1_000_000
    {
        return Err(CouncilRefusal::new(
            COUNCIL_POLICY_REFUSED,
            format!(
                "council policy {} needs a revision and a quorum in [{quorum_floor_micros}, 1000000]",
                policy.policy_id
            ),
        ));
    }
    Ok(())
}

fn digest_of<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_vec(value)
        .map(|bytes| Digest256::sha256(&bytes).to_hex())
        .map_err(|error| error.to_string())
}

/// Decide over checked input: count positions per option, abstain on a tie of
/// the two best, select the best only above the quorum.
pub fn conclude(input: &CouncilInput<'_>) -> Result<CouncilResult, String> {
    let mut sorted: Vec<&CouncilPosition> = input.positions.iter().collect();
    sorted.sort_by(|a, b| a.position_id.cmp(&b.position_id));
    let mut counts: BTreeMap<&str, u64> = BTreeMap::new();
    for position in &sorted {
        *counts.entry(position.option_id.as_str()).or_default() += 1;
    }
    let mut ranked: Vec<(&str, u64)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let total = sorted.len() as u64;
    let (outcome, abstention) = match ranked.as_slice() {
        [(_, first), (_, second), ..] if first == second => {
            (None, Some(CouncilAbstention::UnresolvedConflict))
        }
        [(option, count), ..]
            if count * 1_000_000 > u64::from(input.policy.quorum_micros) * total =>
        {
            (
                Some(CouncilOutcome::Selected {
                    option_id: (*option).to_string(),
                }),
                None,
            )
        }
        _ => (None, Some(CouncilAbstention::NoQuorum)),
    };
    let selected = outcome
        .as_ref()
        .map(|CouncilOutcome::Selected { option_id }| option_id.as_str());
    let (mut accepted, mut rejected) = (Vec::new(), Vec::new());
    for position in &sorted {
        if Some(position.option_id.as_str()) == selected {
            accepted.push(position.position_id.clone());
        } else {
            rejected.push(position.position_id.clone());
        }
    }
    let positions_digest = digest_of(&sorted)?;
    let decision_digest = digest_of(&(
        &positions_digest,
        &input.policy.policy_id,
        input.policy.revision,
        &outcome,
        &abstention,
    ))?;
    Ok(CouncilResult {
        council_id: input.council_id.to_string(),
        topology_revision: input.topology_revision,
        input_position_ids: sorted.iter().map(|p| p.position_id.clone()).collect(),
        accepted_position_ids: accepted,
        rejected_position_ids: rejected,
        outcome,
        abstention,
        policy_revision: input.policy.revision,
        positions_digest,
        decision_digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(id: &str) -> EvidenceRef {
        EvidenceRef {
            id: id.to_string(),
            revision: 3,
            digest: format!("sha256:{id}"),
        }
    }

    fn position(id: &str, persona: &str, option: &str) -> CouncilPosition {
        CouncilPosition {
            council_id: "c".to_string(),
            topology_revision: 7,
            position_id: id.to_string(),
            persona_id: persona.to_string(),
            persona_revision: 1,
            option_id: option.to_string(),
            claim_digest: format!("sha256:claim-{id}"),
            evidence_refs: vec![head("e1")],
            constraint_refs: vec!["k1".to_string()],
        }
    }

    fn policy() -> CouncilPolicy {
        CouncilPolicy {
            policy_id: "p".to_string(),
            revision: 2,
            quorum_micros: 500_000,
        }
    }

    fn input<'a>(
        positions: &'a [CouncilPosition],
        policy: &'a CouncilPolicy,
        heads: &'a [EvidenceRef],
    ) -> CouncilInput<'a> {
        CouncilInput {
            council_id: "c",
            topology_revision: 7,
            positions,
            policy,
            evidence_current: heads,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R106.1
    #[test]
    fn council_types_and_digests_round_trip() {
        let positions = vec![
            position("p2", "persona-b", "a"),
            position("p1", "persona-a", "a"),
            position("p3", "persona-c", "b"),
        ];
        let (policy, heads) = (policy(), [head("e1")]);
        let input = input(&positions, &policy, &heads);
        check(&input, 500_000).expect("a complete request passes");
        let result = conclude(&input).expect("decides");
        assert_eq!(result.input_position_ids, ["p1", "p2", "p3"]);
        assert_eq!(result.accepted_position_ids, ["p1", "p2"]);
        assert_eq!(result.rejected_position_ids, ["p3"]);
        assert_eq!(
            result.outcome,
            Some(CouncilOutcome::Selected {
                option_id: "a".to_string()
            })
        );
        assert!(result.abstention.is_none());
        let json = serde_json::to_string(&result).unwrap();
        assert_eq!(
            serde_json::from_str::<CouncilResult>(&json).unwrap(),
            result
        );
        // Input order does not change the digests: positions sort by id.
        let mut shuffled = positions.clone();
        shuffled.reverse();
        let again = conclude(&self::input(&shuffled, &policy, &heads)).unwrap();
        assert_eq!(again.positions_digest, result.positions_digest);
        assert_eq!(again.decision_digest, result.decision_digest);
        assert_eq!(result.positions_digest.len(), 64);
        let mut other = policy.clone();
        other.revision = 3;
        let moved = conclude(&self::input(&positions, &other, &heads)).unwrap();
        assert_ne!(moved.decision_digest, result.decision_digest);
        assert_eq!(moved.positions_digest, result.positions_digest);
    }

    // spec: MEDULLA-DECISION-ENGINE-R106.1
    #[test]
    fn ties_and_thin_majorities_abstain_and_checks_run_in_order() {
        let (policy, heads) = (policy(), [head("e1")]);
        let tie = vec![position("p1", "a", "x"), position("p2", "b", "y")];
        let result = conclude(&input(&tie, &policy, &heads)).unwrap();
        assert_eq!(
            result.abstention,
            Some(CouncilAbstention::UnresolvedConflict)
        );
        assert!(result.accepted_position_ids.is_empty());
        let split = vec![
            position("p1", "a", "x"),
            position("p2", "b", "x"),
            position("p3", "c", "y"),
            position("p4", "d", "z"),
        ];
        let strict = CouncilPolicy {
            quorum_micros: 750_000,
            ..policy.clone()
        };
        let result = conclude(&input(&split, &strict, &heads)).unwrap();
        assert_eq!(result.abstention, Some(CouncilAbstention::NoQuorum));
        let code = |input: CouncilInput<'_>| check(&input, 500_000).unwrap_err().code;
        let mut stale = vec![position("p1", "a", "x")];
        stale[0].evidence_refs[0].digest = "sha256:old".to_string();
        assert_eq!(
            code(input(&stale, &policy, &heads)),
            RefusalCode::new(COUNCIL_STALE_EVIDENCE)
        );
        let mut mismatch = vec![position("p1", "a", "x")];
        mismatch[0].topology_revision = 8;
        assert_eq!(
            code(input(&mismatch, &policy, &heads)),
            RefusalCode::new(COUNCIL_TOPOLOGY_MISMATCH)
        );
        let dup = vec![position("p1", "a", "x"), position("p2", "a", "x")];
        assert_eq!(
            code(input(&dup, &policy, &heads)),
            RefusalCode::new(COUNCIL_DUPLICATE_POSITION)
        );
        let weak = CouncilPolicy {
            quorum_micros: 400_000,
            ..policy.clone()
        };
        let one = vec![position("p1", "a", "x")];
        assert_eq!(
            code(input(&one, &weak, &heads)),
            RefusalCode::new(COUNCIL_POLICY_REFUSED)
        );
        assert_eq!(
            code(input(&one, &policy, &[])),
            RefusalCode::new(COUNCIL_MISSING_EVIDENCE)
        );
        assert_eq!(
            code(input(&[], &policy, &heads)),
            RefusalCode::new(COUNCIL_INCOMPLETE_INPUT)
        );
    }
}
