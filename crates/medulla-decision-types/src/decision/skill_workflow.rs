//! Skill workflow drafts: the model (MEDULLA-DECISION-ENGINE-R156.1).
//!
//! A [`SkillWorkflowDraft`] is the typed structure extracted from one skill
//! document: steps, the types they consume and produce, and how they connect.
//! It holds typed structure only, never prose. `Decide` with
//! `QuestionKind::SkillWorkflowCompile` compiles a draft into a stored program
//! (R135). Nothing here reads a clock or a store.

use medulla_types::contract::{BoundedVec, Digest256};
use serde::{Deserialize, Serialize};

use super::program_store::MAX_PROGRAM_STEPS;
use medulla_types::refusal_code::RefusalCode;

/// Most input or output types one step may declare.
pub const MAX_STEP_TYPES: usize = 16;
/// Most outgoing edges one step may have.
pub const MAX_STEP_EDGES: usize = 16;

/// Request refusal: two steps share an id, a `next` target is absent, or a branch has no label
/// (MEDULLA-DECISION-ENGINE-R156.4).
pub const SKILL_WORKFLOW_AMBIGUOUS: RefusalCode = RefusalCode::new("SKILL_WORKFLOW_AMBIGUOUS");
/// Request refusal: a step with an external action has no `on_error`.
pub const SKILL_WORKFLOW_MISSING_ERROR_HANDLER: RefusalCode =
    RefusalCode::new("SKILL_WORKFLOW_MISSING_ERROR_HANDLER");
/// Request refusal: a step lacks input or output types.
pub const SKILL_WORKFLOW_UNTYPED_STEP: RefusalCode = RefusalCode::new("SKILL_WORKFLOW_UNTYPED_STEP");
/// Request refusal: the `next` edges form a cycle.
pub const SKILL_WORKFLOW_CYCLE: RefusalCode = RefusalCode::new("SKILL_WORKFLOW_CYCLE");
/// Request refusal: more than [`MAX_PROGRAM_STEPS`] steps.
pub const SKILL_WORKFLOW_TOO_LONG: RefusalCode = RefusalCode::new("SKILL_WORKFLOW_TOO_LONG");

/// Every refusal code of the `skill_workflow_compile` family; joins `codes::FAMILY_TABLES`
/// under `QuestionKind::SkillWorkflowCompile`.
pub const REFUSALS: &[RefusalCode] = &[
    SKILL_WORKFLOW_AMBIGUOUS,
    SKILL_WORKFLOW_MISSING_ERROR_HANDLER,
    SKILL_WORKFLOW_UNTYPED_STEP,
    SKILL_WORKFLOW_CYCLE,
    SKILL_WORKFLOW_TOO_LONG,
];

/// The prefix of an internal action, one that calls nothing outside the engine. Every other
/// action is external and needs an `on_error` step.
pub const INTERNAL_ACTION_PREFIX: &str = "internal.";

/// The closed set of value types a step may consume or produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum TypeRef {
    Text,
    Int,
    Bool,
    Iri,
    Json,
    Digest,
}

/// One outgoing edge: the next step and an optional branch label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StepEdge {
    pub step_id: String,
    #[serde(default)]
    pub label: Option<String>,
}

/// One step of a draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DraftStep {
    pub step_id: String,
    /// The action's identifier; never prose.
    pub action: String,
    #[serde(default)]
    pub input_types: BoundedVec<TypeRef, MAX_STEP_TYPES>,
    #[serde(default)]
    pub output_types: BoundedVec<TypeRef, MAX_STEP_TYPES>,
    #[serde(default)]
    pub next: BoundedVec<StepEdge, MAX_STEP_EDGES>,
    /// The step that handles this step's failure.
    #[serde(default)]
    pub on_error: Option<String>,
}

/// The typed structure of one skill document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SkillWorkflowDraft {
    /// SHA-256 (64 lowercase hex characters) of the skill document.
    pub skill_document_digest: Digest256,
    pub steps: BoundedVec<DraftStep, MAX_PROGRAM_STEPS>,
}

/// An identifier: non-empty, at most 128 bytes, letters, digits and `_ - . : /`.
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':' | '/'))
}

impl SkillWorkflowDraft {
    /// Check the draft's structure: at least one step, identifier-shaped ids,
    /// actions and labels, unique step ids, and every `next` and `on_error`
    /// naming a declared step.
    pub fn validate(&self) -> Result<(), String> {
        if self.steps.is_empty() {
            return Err("a draft holds at least one step".into());
        }
        let ids: std::collections::BTreeSet<&str> =
            self.steps.iter().map(|s| s.step_id.as_str()).collect();
        if ids.len() != self.steps.len() {
            return Err("step ids are unique".into());
        }
        for step in &self.steps {
            if !identifier(&step.step_id) || !identifier(&step.action) {
                return Err(format!(
                    "step {:?}: step_id and action are identifiers, not prose",
                    step.step_id
                ));
            }
            for edge in &step.next {
                if !ids.contains(edge.step_id.as_str()) {
                    return Err(format!(
                        "step {}: next names the undeclared step {}",
                        step.step_id, edge.step_id
                    ));
                }
                if edge.label.as_deref().is_some_and(|l| !identifier(l)) {
                    return Err(format!(
                        "step {}: branch labels are identifiers",
                        step.step_id
                    ));
                }
            }
            if step
                .on_error
                .as_deref()
                .is_some_and(|target| !ids.contains(target))
            {
                return Err(format!(
                    "step {}: on_error names an undeclared step",
                    step.step_id
                ));
            }
        }
        Ok(())
    }
}

/// A draft the family refuses: the registered code and the detail, which names the step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftRefusal {
    pub code: RefusalCode,
    pub detail: String,
}

impl DraftRefusal {
    fn new(code: RefusalCode, detail: impl Into<String>) -> Self {
        Self { code, detail: detail.into() }
    }
}

impl std::fmt::Display for DraftRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.detail)
    }
}

impl SkillWorkflowDraft {
    /// The completeness rules the compile needs (MEDULLA-DECISION-ENGINE-R156.4), checked in
    /// the order the codes are listed: too long, ambiguous, missing error handler, untyped step,
    /// cycle. A refusal names the step; nothing is repaired and no partial program exists.
    pub fn check_complete(&self) -> Result<(), DraftRefusal> {
        if self.steps.len() > MAX_PROGRAM_STEPS {
            return Err(DraftRefusal::new(
                SKILL_WORKFLOW_TOO_LONG,
                format!("{} steps exceed the {MAX_PROGRAM_STEPS} step limit", self.steps.len()),
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for step in &self.steps {
            if !seen.insert(step.step_id.as_str()) {
                return Err(DraftRefusal::new(
                    SKILL_WORKFLOW_AMBIGUOUS,
                    format!("step {} is declared twice", step.step_id),
                ));
            }
        }
        for step in &self.steps {
            if let Some(edge) = step.next.iter().find(|edge| !seen.contains(edge.step_id.as_str())) {
                return Err(DraftRefusal::new(
                    SKILL_WORKFLOW_AMBIGUOUS,
                    format!("step {}: next names the absent step {}", step.step_id, edge.step_id),
                ));
            }
            if step.next.len() > 1 && step.next.iter().any(|edge| edge.label.is_none()) {
                return Err(DraftRefusal::new(
                    SKILL_WORKFLOW_AMBIGUOUS,
                    format!("step {}: a branch has no label", step.step_id),
                ));
            }
        }
        for step in &self.steps {
            if !step.action.starts_with(INTERNAL_ACTION_PREFIX) && step.on_error.is_none() {
                return Err(DraftRefusal::new(
                    SKILL_WORKFLOW_MISSING_ERROR_HANDLER,
                    format!("step {}: the external action {} has no on_error step", step.step_id, step.action),
                ));
            }
        }
        for step in &self.steps {
            if step.input_types.is_empty() || step.output_types.is_empty() {
                return Err(DraftRefusal::new(
                    SKILL_WORKFLOW_UNTYPED_STEP,
                    format!("step {} declares no input or no output types", step.step_id),
                ));
            }
        }
        if let Some(step_id) = self.cycle_step() {
            return Err(DraftRefusal::new(
                SKILL_WORKFLOW_CYCLE,
                format!("step {step_id} lies on a cycle of next edges"),
            ));
        }
        Ok(())
    }

    /// The first declared step that lies on a cycle of `next` edges, when there is one.
    fn cycle_step(&self) -> Option<&str> {
        let steps = self.steps.as_slice();
        let index: std::collections::BTreeMap<&str, usize> = steps
            .iter()
            .enumerate()
            .map(|(at, step)| (step.step_id.as_str(), at))
            .collect();
        // 0 unvisited, 1 on the path, 2 done.
        let mut state = vec![0u8; steps.len()];
        let mut on_cycle = vec![false; steps.len()];
        for root in 0..steps.len() {
            if state[root] != 0 {
                continue;
            }
            let mut path: Vec<(usize, usize)> = vec![(root, 0)];
            state[root] = 1;
            while let Some(&(node, edge)) = path.last() {
                let targets = steps[node].next.as_slice();
                if edge == targets.len() {
                    state[node] = 2;
                    path.pop();
                    continue;
                }
                let last = path.len() - 1;
                path[last].1 += 1;
                let Some(&target) = index.get(targets[edge].step_id.as_str()) else {
                    continue;
                };
                match state[target] {
                    0 => {
                        state[target] = 1;
                        path.push((target, 0));
                    }
                    1 => {
                        // Everything from `target` along the path to here is on the cycle.
                        if let Some(from) = path.iter().position(|(visited, _)| *visited == target) {
                            for (visited, _) in &path[from..] {
                                on_cycle[*visited] = true;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        on_cycle.iter().position(|flag| *flag).map(|at| steps[at].step_id.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(id: &str, next: &[&str]) -> DraftStep {
        DraftStep {
            step_id: id.into(),
            action: format!("tool.{id}"),
            input_types: BoundedVec::new(vec![TypeRef::Text, TypeRef::Iri]).unwrap(),
            output_types: BoundedVec::new(vec![TypeRef::Json]).unwrap(),
            next: BoundedVec::new(
                next.iter()
                    .map(|n| StepEdge {
                        step_id: (*n).into(),
                        label: Some("ok".into()),
                    })
                    .collect(),
            )
            .unwrap(),
            on_error: None,
        }
    }

    fn draft(steps: Vec<DraftStep>) -> SkillWorkflowDraft {
        SkillWorkflowDraft {
            skill_document_digest: Digest256::sha256(b"skill document"),
            steps: BoundedVec::new(steps).unwrap(),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R156.1
    #[test]
    fn draft_round_trips() {
        let mut fail = step("c", &[]);
        fail.on_error = None;
        let mut first = step("a", &["b"]);
        first.on_error = Some("c".into());
        let original = draft(vec![first, step("b", &["c"]), fail]);
        original.validate().unwrap();

        let wire = serde_json::to_value(&original).unwrap();
        assert_eq!(wire["steps"][0]["input_types"][1], "iri");
        assert_eq!(wire["skill_document_digest"].as_str().unwrap().len(), 64);
        let decoded: SkillWorkflowDraft = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(decoded, original);
        let packed = rmp_serde::to_vec_named(&original).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<SkillWorkflowDraft>(&packed).unwrap(),
            original
        );
        assert_eq!(
            serde_json::to_string(
                &crate::decision::statistical::QuestionKind::SkillWorkflowCompile
            )
            .unwrap(),
            "\"skill_workflow_compile\""
        );

        // 40 steps decode; 41 are refused by the constructor and the decoder.
        let forty: Vec<DraftStep> = (0..MAX_PROGRAM_STEPS)
            .map(|i| step(&format!("s{i}"), &[]))
            .collect();
        assert!(BoundedVec::<DraftStep, MAX_PROGRAM_STEPS>::new(forty.clone()).is_ok());
        let mut forty_one = forty;
        forty_one.push(step("s40", &[]));
        assert!(BoundedVec::<DraftStep, MAX_PROGRAM_STEPS>::new(forty_one.clone()).is_err());
        let mut too_many = wire.clone();
        too_many["steps"] = serde_json::to_value(&forty_one).unwrap();
        assert!(serde_json::from_value::<SkillWorkflowDraft>(too_many).is_err());

        // Unknown fields, a bad digest and an unknown type are refused.
        let mut unknown = wire.clone();
        unknown["steps"][0]["prose"] = serde_json::json!("do the thing");
        assert!(serde_json::from_value::<SkillWorkflowDraft>(unknown).is_err());
        let mut bad_digest = wire.clone();
        bad_digest["skill_document_digest"] = serde_json::json!("ABC");
        assert!(serde_json::from_value::<SkillWorkflowDraft>(bad_digest).is_err());
        let mut bad_type = wire;
        bad_type["steps"][0]["input_types"][0] = serde_json::json!("float");
        assert!(serde_json::from_value::<SkillWorkflowDraft>(bad_type).is_err());

        // Structure checks: dangling edges, duplicate ids, prose actions.
        assert!(draft(vec![step("a", &["missing"])]).validate().is_err());
        assert!(draft(vec![step("a", &[]), step("a", &[])])
            .validate()
            .is_err());
        let mut prose = step("a", &[]);
        prose.action = "do the thing now".into();
        assert!(draft(vec![prose]).validate().is_err());
        assert!(draft(vec![]).validate().is_err());
    }

    fn refused(draft: &SkillWorkflowDraft) -> String {
        draft.check_complete().unwrap_err().to_string()
    }

    fn external(id: &str, next: &[&str]) -> DraftStep {
        let mut step = step(id, next);
        step.on_error = Some("recover".into());
        step
    }

    // spec: MEDULLA-DECISION-ENGINE-R156.4
    #[test]
    fn completeness_rules_name_the_step() {
        let mut recover = step("recover", &[]);
        recover.action = "internal.recover".into();
        // A complete draft passes: the external step has a handler, the handler is internal.
        let ok = draft(vec![external("a", &["b"]), external("b", &[]), recover.clone()]);
        assert_eq!(ok.check_complete(), Ok(()));
        // Two steps with one id.
        let twice = draft(vec![external("a", &[]), external("a", &[]), recover.clone()]);
        assert!(refused(&twice).starts_with("SKILL_WORKFLOW_AMBIGUOUS"), "{}", refused(&twice));
        assert!(refused(&twice).contains("step a"));
        // A next target that is absent, and a branch without a label.
        let absent = draft(vec![external("a", &["ghost"]), recover.clone()]);
        assert!(refused(&absent).starts_with("SKILL_WORKFLOW_AMBIGUOUS"));
        let mut branching = external("a", &["b", "c"]);
        branching.next = BoundedVec::new(vec![
            StepEdge { step_id: "b".into(), label: Some("yes".into()) },
            StepEdge { step_id: "c".into(), label: None },
        ])
        .unwrap();
        let unlabelled = draft(vec![branching, external("b", &[]), external("c", &[]), recover.clone()]);
        assert!(refused(&unlabelled).contains("a branch has no label"));
        // An external action with no handler.
        let unhandled = draft(vec![step("a", &[]), recover.clone()]);
        assert!(refused(&unhandled).starts_with("SKILL_WORKFLOW_MISSING_ERROR_HANDLER"));
        assert!(refused(&unhandled).contains("step a"));
        // A step with no types.
        let mut untyped = external("a", &[]);
        untyped.output_types = BoundedVec::new(Vec::new()).unwrap();
        let bare = draft(vec![untyped, recover.clone()]);
        assert!(refused(&bare).starts_with("SKILL_WORKFLOW_UNTYPED_STEP"));
        // A cycle of next edges, and a self edge.
        let cyclic = draft(vec![external("a", &["b"]), external("b", &["a"]), recover.clone()]);
        assert!(refused(&cyclic).starts_with("SKILL_WORKFLOW_CYCLE"));
        assert!(refused(&cyclic).contains("step a"));
        let selfish = draft(vec![external("a", &["a"]), recover]);
        assert!(refused(&selfish).starts_with("SKILL_WORKFLOW_CYCLE"));
    }
}
