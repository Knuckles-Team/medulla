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

/// Most input or output types one step may declare.
pub const MAX_STEP_TYPES: usize = 16;
/// Most outgoing edges one step may have.
pub const MAX_STEP_EDGES: usize = 16;

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
}
