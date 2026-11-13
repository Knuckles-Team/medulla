//! The `generated_code_admission` question family
//! (MEDULLA-DECISION-ENGINE-R138.100.2).
//!
//! Whether generated code may be admitted, from the adversarial verifier's
//! receipt. The receipt header arrives in the `receipt` parameter and its
//! attack cases arrive as declared options, one per case: the option id is the
//! case id, `classification` holds `kind:<kind>` and `state:<state>`, and the
//! text `retest_artifact_digest` holds the artifact the retest ran against.
//! The family rebuilds the receipt, binds it to the stated
//! `verifier_receipt_digest`, and answers through the one check kernel
//! (`check_verifier_receipt`). It writes nothing; `DecisionLog` persists the
//! admission record, whose weakest evidence class is `Claim`.

use serde::Deserialize;

use medulla_decision_stats::decision::verifier_receipt::{check_verifier_receipt, ReceiptBreach};
use medulla_decision_types::decision::codes;
use medulla_decision_types::decision::context_package::is_sha256_hex;
use medulla_decision_types::decision::digest::verifier_receipt_digest;
use medulla_decision_types::decision::record::outcome::{DecisionRefusal, RefusalCode};
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, StatisticalOutcome,
};
use medulla_decision_types::decision::verifier_receipt::{
    VerifierCase, VerifierCaseState, VerifierReceipt,
};
use medulla_decision_types::decision::{
    DecisionPolicy, EvidenceClass, PremiseClass, PremiseRef, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};

use super::support::{abstained_all, advisory, invalid, json_param, param, text_param};
use super::DecisionFamily;

/// The option an admitted artifact names.
pub const ADMIT_OPTION: &str = "admit";

/// The registered refusal codes of the family, in declaration order.
pub const REFUSALS: &[RefusalCode] = codes::GENERATED_CODE_ADMISSION;

/// The family of the `generated_code_admission` question kind.
pub struct GeneratedCodeAdmissionFamily;

/// The receipt header the verifier states; the cases come as declared options.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptHeader {
    artifact_digest: String,
    tests_digest: String,
    attack_policy_revision: String,
    verifier_revision: String,
}

/// Why generated code was not admitted.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AdmissionRefusal {
    ReceiptMissing(String),
    CaseUnresolved(String),
    ArtifactStale(String),
    TestsChanged(String),
    RecheckFailed(String),
    RevisionStale(String),
}

impl DecisionRefusal for AdmissionRefusal {
    fn code(&self) -> RefusalCode {
        match self {
            Self::ReceiptMissing(_) => codes::VERIFIER_RECEIPT_MISSING,
            Self::CaseUnresolved(_) => codes::VERIFIER_CASE_UNRESOLVED,
            Self::ArtifactStale(_) => codes::VERIFIER_ARTIFACT_STALE,
            Self::TestsChanged(_) => codes::VERIFIER_TESTS_CHANGED,
            Self::RecheckFailed(_) => codes::VERIFIER_RECHECK_FAILED,
            Self::RevisionStale(_) => codes::VERIFIER_REVISION_STALE,
        }
    }

    fn detail(&self) -> String {
        match self {
            Self::ReceiptMissing(d)
            | Self::CaseUnresolved(d)
            | Self::ArtifactStale(d)
            | Self::TestsChanged(d)
            | Self::RecheckFailed(d)
            | Self::RevisionStale(d) => d.clone(),
        }
    }
}

impl AdmissionRefusal {
    /// The refusal that reports one breach of the check kernel.
    fn from_breach(breach: &ReceiptBreach) -> Self {
        match breach {
            ReceiptBreach::MalformedDigest { field } if *field == "tests_digest" => {
                Self::TestsChanged("the receipt's tests digest is malformed".to_string())
            }
            ReceiptBreach::MalformedDigest { field } => {
                Self::ArtifactStale(format!("the receipt's {field} is malformed"))
            }
            ReceiptBreach::ArtifactMismatch { expected, actual } => Self::ArtifactStale(format!(
                "the receipt verified artifact {actual}, the proposed one is {expected}"
            )),
            ReceiptBreach::TestsMismatch { expected, actual } => Self::TestsChanged(format!(
                "the receipt verified against tests {actual}, the declared ones are {expected}"
            )),
            ReceiptBreach::AttackPolicyStale { expected, actual } => Self::RevisionStale(format!(
                "attack policy {actual} is not the current revision {expected}"
            )),
            ReceiptBreach::VerifierStale { expected, actual } => Self::RevisionStale(format!(
                "verifier {actual} is not the current revision {expected}"
            )),
            ReceiptBreach::TooFewCases { required, observed } => Self::ReceiptMissing(format!(
                "the receipt holds {observed} cases, the policy requires {required}"
            )),
            ReceiptBreach::CaseNotRetested { case_id, state } => match state {
                VerifierCaseState::Failed => {
                    Self::RecheckFailed(format!("the retest of case {case_id} failed"))
                }
                _ => Self::CaseUnresolved(format!("case {case_id} is not retested")),
            },
            ReceiptBreach::CaseRetestMismatch { case_id } => Self::ArtifactStale(format!(
                "case {case_id} was retested against another artifact"
            )),
        }
    }

    /// Position in the reported order: one reason per code, in code order.
    fn rank(&self) -> usize {
        codes::GENERATED_CODE_ADMISSION
            .iter()
            .position(|code| *code == self.code())
            .unwrap_or(usize::MAX)
    }
}

/// The case one declared option carries.
fn case_of(
    option: &medulla_decision_types::decision::statistical::declared::DeclaredOption,
) -> Result<VerifierCase, String> {
    let tagged = |prefix: &str| {
        let mut found = option
            .classification
            .iter()
            .filter_map(|entry| entry.strip_prefix(prefix));
        match (found.next(), found.next()) {
            (Some(value), None) => Ok(value.to_string()),
            _ => Err(invalid(format!(
                "option {} carries exactly one `{prefix}` classification",
                option.option_id
            ))),
        }
    };
    let state = tagged("state:")?;
    let state: VerifierCaseState = serde_json::from_value(serde_json::Value::String(state))
        .map_err(|_| {
            invalid(format!(
                "option {} names a state outside open, fixed, retested, failed",
                option.option_id
            ))
        })?;
    let retest = option
        .texts
        .iter()
        .find(|text| text.key == "retest_artifact_digest")
        .map(|text| text.text.clone());
    if let Some(digest) = &retest {
        if !is_sha256_hex(digest) {
            return Err(invalid(format!(
                "option {} retest_artifact_digest is not 64 lowercase hex characters",
                option.option_id
            )));
        }
    }
    Ok(VerifierCase {
        case_id: option.option_id.clone(),
        kind: tagged("kind:")?,
        state,
        retest_artifact_digest: retest,
    })
}

/// What the request states: the proposed artifact and tests, the current
/// revisions and, when present, the receipt with the digest the caller binds it to.
struct Admission {
    artifact_digest: String,
    tests_digest: String,
    attack_policy_revision: String,
    verifier_revision: String,
    receipt: Option<(VerifierReceipt, String)>,
}

fn required_text(request: &DecideRequest, name: &str) -> Result<String, String> {
    text_param(request, name)?
        .map(str::to_string)
        .ok_or_else(|| invalid(format!("parameter {name} is required")))
}

fn admission(request: &DecideRequest) -> Result<Admission, String> {
    let artifact_digest = required_text(request, "artifact_digest")?;
    let tests_digest = required_text(request, "tests_digest")?;
    for (name, digest) in [
        ("artifact_digest", &artifact_digest),
        ("tests_digest", &tests_digest),
    ] {
        if !is_sha256_hex(digest) {
            return Err(invalid(format!(
                "parameter {name} is not 64 lowercase hex characters"
            )));
        }
    }
    let attack_policy_revision = required_text(request, "attack_policy_revision")?;
    let verifier_revision = required_text(request, "verifier_revision")?;
    let stated = text_param(request, "verifier_receipt_digest")?;
    let receipt = match (param(request, "receipt")?, stated) {
        // No receipt at all: the verifier was skipped.
        (None, _) | (_, None) => None,
        (Some(_), Some(stated)) => {
            let header: ReceiptHeader = json_param(request, "receipt")?;
            for (field, digest) in [
                ("artifact_digest", &header.artifact_digest),
                ("tests_digest", &header.tests_digest),
            ] {
                if !is_sha256_hex(digest) {
                    return Err(invalid(format!(
                        "receipt.{field} is not 64 lowercase hex characters"
                    )));
                }
            }
            let CandidateSource::Declared { options } = &request.candidates else {
                return Err(invalid(
                    "a generated_code_admission question declares its cases",
                ));
            };
            let cases = options
                .iter()
                .map(case_of)
                .collect::<Result<Vec<_>, String>>()?;
            let cases = BoundedVec::new(cases)
                .map_err(|detail| invalid(format!("the receipt holds too many cases: {detail}")))?;
            let receipt = VerifierReceipt {
                artifact_digest: header.artifact_digest,
                tests_digest: header.tests_digest,
                attack_policy_revision: header.attack_policy_revision,
                verifier_revision: header.verifier_revision,
                cases,
            };
            if verifier_receipt_digest(&receipt) != stated {
                return Err(invalid(
                    "verifier_receipt_digest does not match the stated receipt",
                ));
            }
            Some((receipt, stated.to_string()))
        }
    };
    Ok(Admission {
        artifact_digest,
        tests_digest,
        attack_policy_revision,
        verifier_revision,
        receipt,
    })
}

impl DecisionFamily for GeneratedCodeAdmissionFamily {
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        admission(request).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some((|| {
            let stated = admission(request)?;
            let Some((receipt, _digest)) = &stated.receipt else {
                return abstained_all(
                    QuestionKind::GeneratedCodeAdmission,
                    &[AdmissionRefusal::ReceiptMissing(
                        "no verifier receipt accompanies the proposed artifact".to_string(),
                    )],
                );
            };
            let breaches = check_verifier_receipt(
                receipt,
                &stated.artifact_digest,
                &stated.tests_digest,
                &stated.attack_policy_revision,
                &stated.verifier_revision,
                policy,
            );
            let mut premises = vec![FamilyPremise {
                subject: "verifier",
                fact: format!("verifier.revision={}", stated.verifier_revision),
                class: PremiseClass::Definition,
                source: FamilyPremiseSource::Request("params.verifier_revision"),
            }];
            premises.extend(receipt.cases.iter().map(|case| FamilyPremise {
                subject: "verifier",
                fact: format!("verifier.case:{}={}", case.case_id, state_name(case.state)),
                class: PremiseClass::Claim,
                source: FamilyPremiseSource::Request("candidates.options"),
            }));
            let mut executed = if breaches.is_empty() {
                advisory(vec![ADMIT_OPTION.to_string()])?
            } else {
                // One reason per distinct code, in code order.
                let mut refusals: Vec<AdmissionRefusal> =
                    breaches.iter().map(AdmissionRefusal::from_breach).collect();
                refusals.sort_by_key(AdmissionRefusal::rank);
                refusals.dedup_by_key(|refusal| refusal.rank());
                abstained_all(QuestionKind::GeneratedCodeAdmission, &refusals)?
            };
            executed.family_premises = premises;
            Ok(executed)
        })())
    }

    fn premises(
        &self,
        _request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        executed
            .family_premises
            .iter()
            .map(|premise| premise.seal(policy_digest))
            .collect()
    }

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(ResolutionKind::Constraint)
    }

    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(EvidenceClass::Claim)
    }
}

fn state_name(state: VerifierCaseState) -> &'static str {
    match state {
        VerifierCaseState::Open => "open",
        VerifierCaseState::Fixed => "fixed",
        VerifierCaseState::Retested => "retested",
        VerifierCaseState::Failed => "failed",
    }
}
