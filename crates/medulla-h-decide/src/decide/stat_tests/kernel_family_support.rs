//! The request builder the pure-kernel family tests share
//! (MEDULLA-DECISION-ENGINE-R751.755, R752.754, R753.754, R753.755): one typed
//! body parameter and the declared candidates that name its subject.

use super::*;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;

/// A `Decide` request of `kind` whose one parameter is `value` and whose
/// declared candidates are `candidate_ids`.
pub(super) fn kernel_request(
    schema: &ComponentDependency,
    kind: QuestionKind,
    param: &str,
    value: TypedValue,
    candidate_ids: &[&str],
) -> DecideRequest {
    DecideRequest {
        tenant_id: TENANT.into(),
        question: StatisticalQuestion {
            question_id: format!("kernel.{param}"),
            kind,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(
                candidate_ids
                    .iter()
                    .map(|id| DeclaredOption {
                        option_id: (*id).to_string(),
                        classification: BoundedVec::default(),
                        numbers: BoundedVec::default(),
                        texts: BoundedVec::default(),
                    })
                    .collect(),
            )
            .unwrap(),
        },
        feature_schema: schema.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![TypedParam {
            name: param.to_string(),
            value,
        }])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

/// The facts of a record's premises.
pub(super) fn facts(record: &StatisticalDecisionRecord) -> Vec<&str> {
    record.premises.iter().map(|p| p.fact.as_str()).collect()
}

/// Serve `request` as a caller of another tenant: the request names
/// `tenant-other`, which the caller does not belong to.
pub(super) async fn decide_as_other_tenant(h: &Harness, mut request: DecideRequest) -> String {
    request.tenant_id = "tenant-other".to_string();
    decide(h, request).await.unwrap_err()
}
