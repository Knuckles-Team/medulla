//! Skill evolution vocabulary: paired rollout sets and the spec-proposal chart
//! names (MEDULLA-DECISION-ENGINE-R654.1), and the skill gap, neologism and
//! merge-candidate proposals (MEDULLA-DECISION-ENGINE-R703.1).
//!
//! A skill revision is a program revision; its champion moves through the
//! promotion lifecycle. A [`RolloutSet`] is the paired evidence for one
//! challenger against one champion, so its subject is always a program: the set
//! carries the `program_ref` and the promotion layer derives
//! `RevisionSubject::Program` from it. The set lives here, below the promotion
//! types, so the subject is held as that reference rather than as the enum.
//!
//! The spec-proposal lifecycle is the explicit `SpecProposalMachine`; its
//! states and events are the closed constants below and the chart definition is
//! built from them by `medulla_store::loop_statechart::spec_proposal_statechart_def`.

use serde::{Deserialize, Serialize};

use medulla_types::contract::BoundedVec;

use medulla_types::refusal_code::RefusalCode;

/// Most rollouts one set may carry.
pub const MAX_ROLLOUTS: usize = 4096;
/// Most run-trace and library ids one merge candidate may cite.
pub const MAX_MERGE_EVIDENCE: usize = 32;
/// The `SkillEvolution` parameter that lists the skill ids runs asked for,
/// one entry per request (MEDULLA-DECISION-ENGINE-R703.3).
pub const PARAM_REQUESTED_SKILLS: &str = "requested_skills";
/// The `SkillEvolution` parameter that lists the terms runs used as skill
/// names, one entry per use.
pub const PARAM_USED_TERMS: &str = "used_terms";
/// The optional text parameter naming the graph whose stored vectors embed the
/// visible skills, keyed by component id.
pub const PARAM_EMBEDDING_GRAPH: &str = "skill_embedding_graph";
/// The parameter the engine writes before the family concludes: a JSON object
/// from skill id to its stored embedding, read through the caller's graph
/// access. A request that carries it is refused as a duplicate.
pub const PARAM_SKILL_EMBEDDINGS: &str = "skill_embeddings";
/// The optional text parameter naming one pair of skills, first id
/// (MEDULLA-DECISION-ENGINE-R703.4). It is given together with
/// [`PARAM_PAIR_B`]; the merge proposals then concern that pair only.
pub const PARAM_PAIR_A: &str = "pair_a";
/// The second skill of the named pair.
pub const PARAM_PAIR_B: &str = "pair_b";
/// The optional JSON text parameter that maps a requested skill id, a used
/// term or a skill id to the run-trace ids the caller read it from
/// (MEDULLA-DECISION-ENGINE-R703.5). Every candidate cites the traces it rests
/// on in its sealed premise.
pub const PARAM_CANDIDATE_TRACES: &str = "candidate_traces";
/// A named pair whose skills differ in side-effect class: two skills of
/// different classes never merge (MEDULLA-DECISION-ENGINE-R703.4).
pub const MERGE_CLASS_MISMATCH: RefusalCode = RefusalCode::new("MERGE_CLASS_MISMATCH");
/// A named pair of which a skill is not visible to the tenant. The refusal is
/// the same for an absent skill, so it never confirms that one exists.
pub const MERGE_PAIR_NOT_VISIBLE: RefusalCode = RefusalCode::new("MERGE_PAIR_NOT_VISIBLE");
/// A rollout lacks the challenger score or the champion score, so its
/// `(seed, task_id)` pair is incomplete (MEDULLA-DECISION-ENGINE-R654.4).
pub const ROLLOUT_UNPAIRED: RefusalCode = RefusalCode::new("ROLLOUT_UNPAIRED");
/// A rollout set holds fewer complete pairs than `knob promotion.min_cases`
/// (MEDULLA-DECISION-ENGINE-R654.4).
pub const ROLLOUT_TOO_FEW: RefusalCode = RefusalCode::new("ROLLOUT_TOO_FEW");
/// The `SkillEvolution` family's refusal table.
pub const REFUSALS: &[RefusalCode] = &[
    MERGE_CLASS_MISMATCH,
    MERGE_PAIR_NOT_VISIBLE,
    ROLLOUT_UNPAIRED,
    ROLLOUT_TOO_FEW,
];
/// Format identity of a [`RolloutSet`].
pub const ROLLOUT_SET_SCHEMA_VERSION: u16 = 1;

/// The name of the spec-proposal statechart definition.
pub const SPEC_PROPOSAL_CHART: &str = "spec_proposal";
/// The closed state set of `SpecProposalMachine`, initial state first.
pub const SPEC_PROPOSAL_STATES: [&str; 6] = [
    "drafted",
    "in_review",
    "approved",
    "rejected",
    "in_development",
    "delivered",
];
/// The closed event set of `SpecProposalMachine`.
pub const SPEC_PROPOSAL_EVENTS: [&str; 5] = ["submit", "approve", "reject", "start", "deliver"];
/// The final states of `SpecProposalMachine`.
pub const SPEC_PROPOSAL_FINALS: [&str; 2] = ["rejected", "delivered"];

/// One paired run of a challenger and the champion on one task under one
/// seed. A score is in parts per million, `0..=1_000_000`; a side without a
/// score leaves the pair incomplete, which the recording refuses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Rollout {
    pub seed: u64,
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub challenger_score_ppm: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub champion_score_ppm: Option<u32>,
    /// The run trace the scores were read from.
    pub trace_ref: String,
}

/// The rollouts of one skill-revision challenger against its champion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RolloutSet {
    pub schema_version: u16,
    /// The program the revisions belong to (`RevisionSubject::Program`).
    pub program_ref: String,
    pub challenger_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub champion_ref: Option<String>,
    pub rollouts: BoundedVec<Rollout, MAX_ROLLOUTS>,
}

/// A skill that runs asked for and the library does not have.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SkillGap {
    pub skill_id: String,
    pub requested_by_runs: u32,
}

/// A term used as a skill name that nothing defines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SkillNeologism {
    pub term: String,
    pub uses: u32,
}

/// Two skills close enough that they should merge. `evidence` lists the
/// run-trace ids and library component ids the proposal rests on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SkillMergeCandidate {
    pub a: String,
    pub b: String,
    pub overlap_ppm: u32,
    pub cosine_ppm: u32,
    pub evidence: BoundedVec<String, MAX_MERGE_EVIDENCE>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rollout(seed: u64, task: &str, challenger: Option<u32>, champion: Option<u32>) -> Rollout {
        Rollout {
            seed,
            task_id: task.into(),
            challenger_score_ppm: challenger,
            champion_score_ppm: champion,
            trace_ref: format!("trace-{seed}-{task}"),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R654.1
    #[test]
    fn rollout_set_round_trips() {
        let set = RolloutSet {
            schema_version: ROLLOUT_SET_SCHEMA_VERSION,
            program_ref: "prog-1".into(),
            challenger_ref: "rev-2".into(),
            champion_ref: Some("rev-1".into()),
            rollouts: BoundedVec::new(vec![
                rollout(7, "t1", Some(800_000), Some(700_000)),
                rollout(8, "t1", Some(650_000), None),
            ])
            .unwrap(),
        };
        let json = serde_json::to_string(&set).unwrap();
        assert_eq!(
            json,
            r#"{"schema_version":1,"program_ref":"prog-1","challenger_ref":"rev-2","champion_ref":"rev-1","rollouts":[{"seed":7,"task_id":"t1","challenger_score_ppm":800000,"champion_score_ppm":700000,"trace_ref":"trace-7-t1"},{"seed":8,"task_id":"t1","challenger_score_ppm":650000,"trace_ref":"trace-8-t1"}]}"#
        );
        assert_eq!(serde_json::from_str::<RolloutSet>(&json).unwrap(), set);

        // The set is bounded at 4096 rollouts and refuses unknown fields.
        let at_bound = vec![rollout(0, "t", Some(1), Some(1)); MAX_ROLLOUTS];
        assert!(BoundedVec::<Rollout, MAX_ROLLOUTS>::new(at_bound.clone()).is_ok());
        let mut over = at_bound;
        over.push(rollout(1, "t", Some(1), Some(1)));
        assert!(BoundedVec::<Rollout, MAX_ROLLOUTS>::new(over).is_err());
        assert!(serde_json::from_str::<Rollout>(
            r#"{"seed":1,"task_id":"t","trace_ref":"x","extra":1}"#
        )
        .is_err());

        // The chart vocabulary is closed and its initial state leads.
        assert_eq!(SPEC_PROPOSAL_STATES[0], "drafted");
        assert_eq!(SPEC_PROPOSAL_STATES.len(), 6);
        assert_eq!(
            SPEC_PROPOSAL_EVENTS,
            ["submit", "approve", "reject", "start", "deliver"]
        );
    }

    /// The wire form is fixed: field names, order and number encoding.
    // proves R703.1: Model
    // spec: MEDULLA-DECISION-ENGINE-R703.1
    #[test]
    fn skill_candidates_round_trip_with_golden_vector() {
        let gap = SkillGap {
            skill_id: "summarize-pdf".into(),
            requested_by_runs: 5,
        };
        assert_eq!(
            serde_json::to_string(&gap).unwrap(),
            r#"{"skill_id":"summarize-pdf","requested_by_runs":5}"#
        );
        let neologism = SkillNeologism {
            term: "frobnicate".into(),
            uses: 3,
        };
        assert_eq!(
            serde_json::to_string(&neologism).unwrap(),
            r#"{"term":"frobnicate","uses":3}"#
        );
        let merge = SkillMergeCandidate {
            a: "skill-a".into(),
            b: "skill-b".into(),
            overlap_ppm: 750_000,
            cosine_ppm: 930_000,
            evidence: BoundedVec::new(vec!["trace-1".into(), "component-9".into()]).unwrap(),
        };
        let json = serde_json::to_string(&merge).unwrap();
        assert_eq!(
            json,
            r#"{"a":"skill-a","b":"skill-b","overlap_ppm":750000,"cosine_ppm":930000,"evidence":["trace-1","component-9"]}"#
        );
        assert_eq!(
            serde_json::from_str::<SkillMergeCandidate>(&json).unwrap(),
            merge
        );
        assert_eq!(
            serde_json::from_str::<SkillGap>(
                r#"{"skill_id":"summarize-pdf","requested_by_runs":5}"#
            )
            .unwrap(),
            gap
        );

        // Evidence is bounded at 32 ids; unknown fields refuse.
        let too_many: Vec<String> = (0..=MAX_MERGE_EVIDENCE).map(|n| format!("e{n}")).collect();
        assert!(BoundedVec::<String, MAX_MERGE_EVIDENCE>::new(too_many).is_err());
        assert!(serde_json::from_str::<SkillGap>(
            r#"{"skill_id":"s","requested_by_runs":1,"x":1}"#
        )
        .is_err());
        assert!(serde_json::from_str::<SkillNeologism>(r#"{"term":"t","uses":1,"x":1}"#).is_err());
    }
}
