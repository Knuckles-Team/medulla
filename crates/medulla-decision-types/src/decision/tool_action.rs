//! Tool-action admission (MEDULLA-DECISION-ENGINE-R038.2).
//!
//! The harness asks, before it executes an action, whether the action may
//! follow the content it read. Medulla answers from the `trust_mark` recorded
//! on that content (`vg:trustMark`, closed values `trusted` and `untrusted`):
//! an action of a listed class that follows untrusted content is denied. The
//! answer is advisory and never grants a human approval.

use serde::{Deserialize, Serialize};

use super::record::outcome::DecisionRefusal;
use medulla_types::refusal_code::RefusalCode;

/// The classes of action the admission gate lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionClass {
    Egress,
    PrivilegedCall,
    Delegation,
}

impl ActionClass {
    /// The class a wire name denotes; `None` for any other name.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "egress" => Some(Self::Egress),
            "privileged_call" => Some(Self::PrivilegedCall),
            "delegation" => Some(Self::Delegation),
            _ => None,
        }
    }
}

/// The answer of the gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolActionVerdict {
    Admit,
    Deny,
}

impl ToolActionVerdict {
    /// The option id the verdict is answered as.
    pub fn option_id(self) -> &'static str {
        match self {
            Self::Admit => "admit",
            Self::Deny => "deny",
        }
    }

    /// The verdict for an action of a listed class that follows content
    /// marked `trust_mark`; `None` when the mark is neither closed value.
    pub fn for_mark(trust_mark: &str) -> Option<Self> {
        match trust_mark {
            "untrusted" => Some(Self::Deny),
            "trusted" => Some(Self::Admit),
            _ => None,
        }
    }
}

/// Why the gate cannot answer.
/// The verdict of a `tool_action_admission` question as a caller reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ToolActionAnswerVerdict {
    Admit,
    Deny,
    /// The question was refused (no readable trust mark); `reason` names the code.
    Abstain,
}

/// The typed answer of a `tool_action_admission` question
/// (MEDULLA-DECISION-ENGINE-R038.4). The answer is advisory: a denied action
/// still needs a human decision on the pending call, and the engine grants
/// none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ToolActionAnswer {
    pub verdict: ToolActionAnswerVerdict,
    /// The pending call the verdict governs: the reference a human approval of
    /// this action is keyed to.
    pub approval_ref: String,
    /// The decision record that sealed this answer.
    pub decision_ref: String,
    /// The refusal code when the verdict is `abstain`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolActionRefusal {
    /// The referenced content carries no readable trust mark in the caller's view.
    TrustMarkMissing { content_ref: String },
}

/// The `REFUSALS` table of the family, registered in `codes::FAMILY_TABLES`.
pub const REFUSALS: &[RefusalCode] = &[RefusalCode::new("TRUST_MARK_MISSING")];

impl DecisionRefusal for ToolActionRefusal {
    fn code(&self) -> RefusalCode {
        REFUSALS[0]
    }

    fn detail(&self) -> String {
        match self {
            Self::TrustMarkMissing { content_ref } => {
                format!("content `{content_ref}` has no readable trust mark")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R038.2
    #[test]
    fn marks_and_classes_are_closed() {
        assert_eq!(
            ToolActionVerdict::for_mark("untrusted"),
            Some(ToolActionVerdict::Deny)
        );
        assert_eq!(
            ToolActionVerdict::for_mark("trusted"),
            Some(ToolActionVerdict::Admit)
        );
        assert_eq!(ToolActionVerdict::for_mark("maybe"), None);
        assert_eq!(ActionClass::parse("egress"), Some(ActionClass::Egress));
        assert_eq!(ActionClass::parse("read"), None);
    }
}
