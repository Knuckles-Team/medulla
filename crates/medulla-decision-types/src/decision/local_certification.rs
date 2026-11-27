//! Local-model certification attestation: types and the pure rule
//! (MEDULLA-DECISION-ENGINE-R142.100).
//!
//! A host runs a skill against a local model and reports a [`RunReceipt`]: one
//! result per criterion the pinned criteria policy declares. [`certify`] checks
//! the receipt against the request and returns a [`LocalCertificationAttestation`]
//! whose digest is the one decision digest scheme. Medulla runs no model and
//! decides no host policy; it only checks what the request pins.

use medulla_agent_types::agent_component::ComponentDependency;
use medulla_query_types::solve_scalar::digest_text;
use serde::{Deserialize, Serialize};

use super::digest::LOCAL_CERTIFICATION_DIGEST_DOMAIN;
use super::record::outcome::DecisionRefusal;
use super::statistical::ranking_eval::PinnedSet;
use medulla_types::refusal_code::RefusalCode;

/// The wire schema version of a certification request and attestation.
pub const LOCAL_CERTIFICATION_SCHEMA_VERSION: u16 = 1;

/// The receipt names another skill or other pins than the request.
pub const CERT_RECEIPT_MISMATCH: RefusalCode = RefusalCode::new("CERT_RECEIPT_MISMATCH");
/// The receipt's criterion ids are not exactly the request's.
pub const CERT_CRITERIA_INCOMPLETE: RefusalCode = RefusalCode::new("CERT_CRITERIA_INCOMPLETE");
/// A criterion did not pass.
pub const CERT_CRITERION_FAILED: RefusalCode = RefusalCode::new("CERT_CRITERION_FAILED");
/// An atomic skill names dependencies.
pub const CERT_ATOMIC_HAS_DEPENDENCIES: RefusalCode =
    RefusalCode::new("CERT_ATOMIC_HAS_DEPENDENCIES");
/// A workflow names no dependencies.
pub const CERT_WORKFLOW_NO_DEPENDENCIES: RefusalCode =
    RefusalCode::new("CERT_WORKFLOW_NO_DEPENDENCIES");

/// A workflow names an atomic skill that has no stored attestation
/// (MEDULLA-DECISION-ENGINE-R142.101).
pub const CERT_DEPENDENCY_MISSING: RefusalCode = RefusalCode::new("CERT_DEPENDENCY_MISSING");
/// A workflow's atomic skill has an attestation, but the skill, model, dataset or criteria
/// revision it attests is no longer the current one (MEDULLA-DECISION-ENGINE-R142.101).
pub const CERT_DEPENDENCY_STALE: RefusalCode = RefusalCode::new("CERT_DEPENDENCY_STALE");

/// The refusal codes of the certification rule, in the `local_certification` table.
pub const REFUSALS: &[RefusalCode] = &[
    CERT_ATOMIC_HAS_DEPENDENCIES,
    CERT_CRITERIA_INCOMPLETE,
    CERT_CRITERION_FAILED,
    CERT_RECEIPT_MISMATCH,
    CERT_WORKFLOW_NO_DEPENDENCIES,
    CERT_DEPENDENCY_MISSING,
    CERT_DEPENDENCY_STALE,
];

/// Whether a skill stands alone or composes other skills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum SkillClassification {
    Atomic,
    Workflow,
}

/// What a certification is pinned to: the local model, the dataset and the
/// criteria policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LocalSkillPins {
    pub model: ComponentDependency,
    pub dataset: PinnedSet,
    pub criteria_policy: ComponentDependency,
}

/// The outcome of one criterion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CriterionResult {
    pub criterion_id: String,
    pub passed: bool,
    /// 64 lowercase hex characters.
    pub evidence_digest: String,
}

/// What the host reports of one run of the skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RunReceipt {
    pub run_id: String,
    pub skill: ComponentDependency,
    pub pins: LocalSkillPins,
    pub results: Vec<CriterionResult>,
    /// The host's monotonic count of completed runs of this skill, at least 1;
    /// stored and digested, never compared by medulla.
    pub completion_revision: u64,
}

/// One certification request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LocalCertificationRequest {
    pub schema_version: u16,
    pub tenant_id: String,
    pub idempotency_key: String,
    pub skill: ComponentDependency,
    pub classification: SkillClassification,
    /// The ordered atomic skills of a workflow; empty for an atomic skill.
    pub dependencies: Vec<ComponentDependency>,
    pub pins: LocalSkillPins,
    /// The ids the pinned criteria policy declares, sent by the host.
    pub criterion_ids: Vec<String>,
    pub receipt: RunReceipt,
}

/// The attested body, digested as [`LocalCertificationAttestation::digest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LocalCertificationBody {
    pub schema_version: u16,
    pub skill: ComponentDependency,
    pub classification: SkillClassification,
    pub dependencies: Vec<ComponentDependency>,
    pub pins: LocalSkillPins,
    pub run_id: String,
    /// Sorted by `criterion_id`.
    pub results: Vec<CriterionResult>,
    pub completion_revision: u64,
}

/// A certified skill: the body and its digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LocalCertificationAttestation {
    pub body: LocalCertificationBody,
    /// `digest_text(LOCAL_CERTIFICATION_DIGEST_DOMAIN, &body)`.
    pub digest: String,
    /// The evidence the attestation rests on: `skill`, `model`, `dataset`, `criteria`,
    /// `receipt` and one `criterion:<id>` entry per result. Derived from the body, so it is
    /// not part of the digest (MEDULLA-DECISION-ENGINE-R142.101).
    #[serde(default)]
    pub premises: Vec<String>,
}

/// The body of `DecisionEval` op `certification`: read the current attestation of one skill
/// revision (MEDULLA-DECISION-ENGINE-R142.103). Scope `admin:decision-eval`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CertificationReadRequest {
    /// The tenant that owns the attestations; checked against the verified tenant.
    pub tenant_id: String,
    /// The skill component.
    pub component_id: String,
    /// The definition digest of the skill revision the caller runs.
    pub definition_digest: String,
}

/// What `certification` answers: the attestation of the asked skill revision with the highest
/// completion revision, or `None` when the tenant stores none. An unknown skill, a foreign
/// tenant's skill and a revision never attested answer alike, so the read is no existence oracle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CertificationReadAnswer {
    pub attestation: Option<LocalCertificationAttestation>,
}

/// The attestation `request` asks for among `stored`: the one whose skill is the asked
/// component at the asked definition digest, taking the highest completion revision and, on a
/// tie, the greater digest so the answer is deterministic.
pub fn current_attestation(
    stored: Vec<LocalCertificationAttestation>,
    request: &CertificationReadRequest,
) -> Option<LocalCertificationAttestation> {
    stored
        .into_iter()
        .filter(|attested| {
            attested.body.skill.component_id == request.component_id
                && attested.body.skill.definition_digest == request.definition_digest
        })
        .max_by(|a, b| {
            (a.body.completion_revision, &a.digest).cmp(&(b.body.completion_revision, &b.digest))
        })
}

/// Why a certification was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertificationRefusal {
    /// The receipt's `skill` or `pins` differ from the request's.
    ReceiptMismatch { field: &'static str },
    /// The receipt's criterion ids are not exactly the request's.
    CriteriaIncomplete,
    /// The first failing criterion, in sorted order.
    CriterionFailed { criterion_id: String },
    /// An atomic skill names dependencies.
    AtomicHasDependencies,
    /// A workflow names no dependencies.
    WorkflowNoDependencies,
}

impl DecisionRefusal for CertificationRefusal {
    fn code(&self) -> RefusalCode {
        match self {
            Self::ReceiptMismatch { .. } => CERT_RECEIPT_MISMATCH,
            Self::CriteriaIncomplete => CERT_CRITERIA_INCOMPLETE,
            Self::CriterionFailed { .. } => CERT_CRITERION_FAILED,
            Self::AtomicHasDependencies => CERT_ATOMIC_HAS_DEPENDENCIES,
            Self::WorkflowNoDependencies => CERT_WORKFLOW_NO_DEPENDENCIES,
        }
    }

    fn detail(&self) -> String {
        match self {
            Self::ReceiptMismatch { field } => {
                format!("the receipt's {field} is not the request's")
            }
            Self::CriteriaIncomplete => {
                "the receipt's criterion ids are not exactly the request's".to_string()
            }
            Self::CriterionFailed { criterion_id } => {
                format!("criterion {criterion_id} did not pass")
            }
            Self::AtomicHasDependencies => "an atomic skill names no dependencies".to_string(),
            Self::WorkflowNoDependencies => {
                "a workflow names the atomic skills it composes".to_string()
            }
        }
    }
}

/// Check `request` and attest it. The checks run in this order: the receipt
/// names the request's skill and pins; its criterion ids are exactly the
/// request's; every result passed; the dependencies fit the classification.
pub fn certify(
    request: &LocalCertificationRequest,
) -> Result<LocalCertificationAttestation, CertificationRefusal> {
    let receipt = &request.receipt;
    if receipt.skill != request.skill {
        return Err(CertificationRefusal::ReceiptMismatch { field: "skill" });
    }
    if receipt.pins != request.pins {
        return Err(CertificationRefusal::ReceiptMismatch { field: "pins" });
    }
    let mut results = receipt.results.clone();
    results.sort_by(|a, b| a.criterion_id.cmp(&b.criterion_id));
    let mut expected = request.criterion_ids.clone();
    expected.sort();
    let reported: Vec<&str> = results.iter().map(|r| r.criterion_id.as_str()).collect();
    let duplicate = expected.windows(2).any(|pair| pair[0] == pair[1]);
    if duplicate
        || reported.len() != expected.len()
        || reported.iter().zip(&expected).any(|(got, want)| *got != want.as_str())
    {
        return Err(CertificationRefusal::CriteriaIncomplete);
    }
    if let Some(failed) = results.iter().find(|result| !result.passed) {
        return Err(CertificationRefusal::CriterionFailed {
            criterion_id: failed.criterion_id.clone(),
        });
    }
    match (request.classification, request.dependencies.is_empty()) {
        (SkillClassification::Atomic, false) => {
            return Err(CertificationRefusal::AtomicHasDependencies)
        }
        (SkillClassification::Workflow, true) => {
            return Err(CertificationRefusal::WorkflowNoDependencies)
        }
        _ => {}
    }
    let body = LocalCertificationBody {
        schema_version: LOCAL_CERTIFICATION_SCHEMA_VERSION,
        skill: request.skill.clone(),
        classification: request.classification,
        dependencies: request.dependencies.clone(),
        pins: request.pins.clone(),
        run_id: receipt.run_id.clone(),
        results,
        completion_revision: receipt.completion_revision,
    };
    let digest = digest_text(LOCAL_CERTIFICATION_DIGEST_DOMAIN, &body);
    let pinned = |dependency: &ComponentDependency| {
        format!("{}@{}", dependency.component_id, dependency.definition_digest)
    };
    let mut premises = vec![
        format!("skill:{}", pinned(&body.skill)),
        format!("model:{}", pinned(&body.pins.model)),
        format!(
            "dataset:{}@{}:{}",
            body.pins.dataset.id, body.pins.dataset.revision, body.pins.dataset.digest
        ),
        format!("criteria:{}", pinned(&body.pins.criteria_policy)),
        format!("receipt:{}", body.run_id),
    ];
    premises.extend(
        body.results
            .iter()
            .map(|result| format!("criterion:{}", result.criterion_id)),
    );
    Ok(LocalCertificationAttestation {
        body,
        digest,
        premises,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_agent_types::agent_component::AgentComponentKind;

    fn dependency(id: &str, kind: AgentComponentKind) -> ComponentDependency {
        ComponentDependency {
            component_id: id.to_string(),
            kind,
            definition_digest: format!("sha256:{}", "a".repeat(64)),
        }
    }

    fn result(id: &str, passed: bool) -> CriterionResult {
        CriterionResult {
            criterion_id: id.to_string(),
            passed,
            evidence_digest: "b".repeat(64),
        }
    }

    fn request() -> LocalCertificationRequest {
        let pins = LocalSkillPins {
            model: dependency("model-a", AgentComponentKind::ModelProfile),
            dataset: PinnedSet {
                id: "dataset-a".to_string(),
                revision: 1,
                digest: "c".repeat(64),
            },
            criteria_policy: dependency("criteria-a", AgentComponentKind::DecisionPolicy),
        };
        let skill = dependency("skill-a", AgentComponentKind::Tool);
        LocalCertificationRequest {
            schema_version: LOCAL_CERTIFICATION_SCHEMA_VERSION,
            tenant_id: "tenant-a".to_string(),
            idempotency_key: "key-1".to_string(),
            skill: skill.clone(),
            classification: SkillClassification::Atomic,
            dependencies: Vec::new(),
            pins: pins.clone(),
            criterion_ids: vec!["accuracy".to_string(), "format".to_string()],
            receipt: RunReceipt {
                run_id: "run-1".to_string(),
                skill,
                pins,
                results: vec![result("format", true), result("accuracy", true)],
                completion_revision: 3,
            },
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R142.103
    #[test]
    fn current_attestation_is_the_latest_of_the_asked_revision() {
        let first = certify(&request()).unwrap();
        let mut later_request = request();
        later_request.receipt.completion_revision = 4;
        let later = certify(&later_request).unwrap();
        let mut other_request = request();
        other_request.skill.definition_digest = format!("sha256:{}", "f".repeat(64));
        other_request.receipt.skill = other_request.skill.clone();
        let other = certify(&other_request).unwrap();
        let ask = CertificationReadRequest {
            tenant_id: "tenant-a".to_string(),
            component_id: "skill-a".to_string(),
            definition_digest: format!("sha256:{}", "a".repeat(64)),
        };
        let stored = vec![first.clone(), other, later.clone()];
        assert_eq!(current_attestation(stored.clone(), &ask), Some(later));
        let mut unknown = ask.clone();
        unknown.definition_digest = "sha256:none".to_string();
        assert_eq!(current_attestation(stored, &unknown), None);
        assert_eq!(current_attestation(Vec::new(), &ask), None);
        let json = serde_json::to_string(&CertificationReadAnswer {
            attestation: Some(first),
        })
        .unwrap();
        assert!(serde_json::from_str::<CertificationReadAnswer>(&json).is_ok());
    }

    // spec: MEDULLA-DECISION-ENGINE-R142.100
    #[test]
    fn attestation_round_trip_and_digest() {
        let asked = request();
        let json = serde_json::to_string(&asked).unwrap();
        assert_eq!(serde_json::from_str::<LocalCertificationRequest>(&json).unwrap(), asked);
        let attested = certify(&asked).unwrap();
        let json = serde_json::to_string(&attested).unwrap();
        assert_eq!(
            serde_json::from_str::<LocalCertificationAttestation>(&json).unwrap(),
            attested
        );
        assert!(attested.digest.starts_with("sha256:"));
        assert_eq!(attested.body.results[0].criterion_id, "accuracy");

        // The digest does not depend on the order of the receipt's results.
        let mut reordered = request();
        reordered.receipt.results.reverse();
        assert_eq!(certify(&reordered).unwrap().digest, attested.digest);
        // It changes with the completion revision.
        let mut later = request();
        later.receipt.completion_revision = 4;
        assert_ne!(certify(&later).unwrap().digest, attested.digest);

        // One case per refusal.
        let mut other_skill = request();
        other_skill.receipt.skill.component_id = "skill-b".to_string();
        assert_eq!(
            certify(&other_skill),
            Err(CertificationRefusal::ReceiptMismatch { field: "skill" })
        );
        let mut other_pins = request();
        other_pins.receipt.pins.dataset.revision = 2;
        assert_eq!(
            certify(&other_pins),
            Err(CertificationRefusal::ReceiptMismatch { field: "pins" })
        );
        let mut missing = request();
        missing.receipt.results.pop();
        assert_eq!(certify(&missing), Err(CertificationRefusal::CriteriaIncomplete));
        let mut extra = request();
        extra.receipt.results.push(result("zzz", true));
        assert_eq!(certify(&extra), Err(CertificationRefusal::CriteriaIncomplete));
        let mut failing = request();
        failing.receipt.results = vec![result("format", false), result("accuracy", false)];
        assert_eq!(
            certify(&failing),
            Err(CertificationRefusal::CriterionFailed {
                criterion_id: "accuracy".to_string()
            })
        );
        let mut atomic = request();
        atomic.dependencies = vec![dependency("skill-b", AgentComponentKind::Tool)];
        assert_eq!(certify(&atomic), Err(CertificationRefusal::AtomicHasDependencies));
        let mut workflow = request();
        workflow.classification = SkillClassification::Workflow;
        assert_eq!(
            certify(&workflow),
            Err(CertificationRefusal::WorkflowNoDependencies)
        );
        assert_eq!(
            CertificationRefusal::CriteriaIncomplete.code().as_str(),
            "CERT_CRITERIA_INCOMPLETE"
        );
    }
}
