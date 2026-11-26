//! The triage of one defect as a typed answer (MEDULLA-SDLC-R004.1, R004.3).
//!
//! The decision names severity, suspected symbols, an owner and a duplicate link, or abstains with
//! a typed reason. The ranking, the knobs and the kernel that produce it live in the outcome
//! types crate; the record types live here because the typed answer
//! `FamilyAnswer::DefectTriage` is part of the decision record, which sits below it.

use medulla_agent_types::agent_component::ComponentDependency;
use medulla_source_types::lifecycle::Severity;
use serde::{Deserialize, Serialize};

use super::record::outcome::{DecisionRefusal, RefusalCode};
use super::request::DecisionPolicyRef;

/// Most owners a triage request names.
pub const MAX_PATH_OWNERS: usize = 64;

/// Why a triage abstained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TriageAbstainReason {
    NoSymbolMatch,
    NoOwner,
    InsufficientEvidence,
}

impl DecisionRefusal for TriageAbstainReason {
    fn code(&self) -> RefusalCode {
        match self {
            Self::NoSymbolMatch => super::codes::TRIAGE_NO_SYMBOL_MATCH,
            Self::NoOwner => super::codes::TRIAGE_NO_OWNER,
            Self::InsufficientEvidence => super::codes::TRIAGE_INSUFFICIENT_EVIDENCE,
        }
    }

    fn detail(&self) -> String {
        match self {
            Self::NoSymbolMatch => "no symbol matches the symptom",
            Self::NoOwner => "no owner for the suspected symbols",
            Self::InsufficientEvidence => "not enough evidence to triage",
        }
        .to_string()
    }
}

/// The triage of one defect.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TriageDecision {
    pub defect_id: String,
    pub severity: Option<Severity>,
    pub suspected_symbols: Vec<String>,
    pub owner_principal: Option<String>,
    pub duplicate_of: Option<String>,
    pub abstain_reason: Option<TriageAbstainReason>,
    pub evidence_refs: Vec<String>,
}

impl TriageDecision {
    /// The `affects` edges (defect to symbol) this decision sets. The
    /// `introducedBy` edge comes from the `introduced.bisect` derivation rule.
    pub fn affects_edges(&self) -> Vec<(String, String)> {
        self.suspected_symbols
            .iter()
            .map(|symbol| (self.defect_id.clone(), symbol.clone()))
            .collect()
    }
}

/// `TriageDefect`: triage one defect of the tenant graph (MEDULLA-SDLC-R004.3). `culprit` is the
/// changeset a `BisectStep` named, when a bisect ran; `path_owners` are the principals, highest
/// priority first, that the caller read from the repository's ownership file (the engine holds no
/// path-ownership record, so ownership is the caller's claim).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TriageDefectRequest {
    pub tenant_id: String,
    pub graph: String,
    pub defect_id: String,
    #[serde(default)]
    pub as_of_ms: Option<u64>,
    #[serde(default)]
    pub culprit: Option<String>,
    #[serde(default)]
    pub path_owners: Vec<String>,
    pub feature_schema: ComponentDependency,
    pub policy: DecisionPolicyRef,
}

/// What `TriageDefect` answers: the committed record, the decision and the `affects` edges
/// written (none when the triage abstained).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TriageDefectReceipt {
    pub record_id: String,
    pub decision: TriageDecision,
    pub edges_written: u32,
}
