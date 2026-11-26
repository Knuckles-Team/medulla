//! `TriageDefect` (MEDULLA-SDLC-R004.3): triage one defect through the `defect_triage` question
//! family, then commit the sealed record through `DecisionLog`, the one place statistical records
//! are committed. The router arm writes the `affects` edges of a decision that did not abstain.
//! The method lives here, not in the ingest handler crate, because a handler crate never calls
//! a sibling.

use medulla_decision_types::decision::defect_triage::{
    TriageDecision, TriageDefectRequest, MAX_PATH_OWNERS,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::log::DecisionLogOp;
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, QuestionSafety, StatisticalQuestion,
    TypedParam, TypedValue,
};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_types::contract::BoundedVec;

use super::SharedState;

/// What one served triage produced: the committed record and its decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriageDefectOutcome {
    pub record_id: String,
    pub decision: TriageDecision,
    /// The node id of the triaged defect: the source of each `affects` edge.
    pub defect_node_id: String,
}

/// The `Decide` request a triage builds: one declared option, the defect id, no facts, and the
/// params sorted by name.
fn decide_request(request: &TriageDefectRequest) -> Result<DecideRequest, String> {
    let invalid = |detail: String| super::stat_support::invalid_parameter(detail);
    if request.defect_id.trim().is_empty() || request.graph.trim().is_empty() {
        return Err(invalid("a triage names its defect and graph".to_string()));
    }
    if request.path_owners.len() > MAX_PATH_OWNERS {
        return Err(invalid(format!(
            "path_owners names more than {MAX_PATH_OWNERS} principals"
        )));
    }
    let param = |name: &str, value: TypedValue| TypedParam {
        name: name.to_string(),
        value,
    };
    let mut params = Vec::new();
    if let Some(at) = request.as_of_ms {
        let at = i64::try_from(at).map_err(|_| invalid("as_of_ms is out of range".to_string()))?;
        params.push(param("as_of_ms", TypedValue::Int(at)));
    }
    if let Some(culprit) = &request.culprit {
        params.push(param("culprit", TypedValue::Text(culprit.clone())));
    }
    params.push(param("graph", TypedValue::Text(request.graph.clone())));
    if !request.path_owners.is_empty() {
        params.push(param(
            "path_owners",
            TypedValue::IriList(BoundedVec::new(request.path_owners.clone()).map_err(invalid)?),
        ));
    }
    Ok(DecideRequest {
        tenant_id: request.tenant_id.clone(),
        question: StatisticalQuestion {
            question_id: format!("defect-triage:{}", request.defect_id),
            kind: QuestionKind::DefectTriage,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![DeclaredOption {
                option_id: request.defect_id.clone(),
                classification: BoundedVec::default(),
                numbers: BoundedVec::default(),
                texts: BoundedVec::default(),
            }])
            .map_err(invalid)?,
        },
        feature_schema: request.feature_schema.clone(),
        head: None,
        policy: request.policy.clone(),
        params: BoundedVec::new(params).map_err(invalid)?,
        max_records: None,
        belief_as_of: BoundedVec::default(),
        evidence_streams: Default::default(),
    })
}

/// Triage one defect and commit the sealed record. A refusal (unknown or closed defect, an
/// invalid parameter) stores no record. `tenant_id` must equal the verified tenant.
pub async fn handle_triage_defect(
    state: &SharedState,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: TriageDefectRequest,
) -> Result<TriageDefectOutcome, String> {
    if request.tenant_id != verified.tenant() {
        return Err(
            "ACCESS_DENIED: TriageDefect tenant must match the verified request tenant".to_string(),
        );
    }
    let decide = decide_request(&request)?;
    let batch = super::stat_decide::serve(state, verified, decide).await?;
    let record = batch
        .records
        .as_slice()
        .first()
        .cloned()
        .ok_or_else(|| "INVALID_ARGUMENT: the triage answered no record".to_string())?;
    let decision = match &record.answer {
        Some(FamilyAnswer::DefectTriage(decision)) => (**decision).clone(),
        _ => return Err("INVALID_ARGUMENT: the triage sealed no decision".to_string()),
    };
    let record_id = record.record_id.clone();
    // The defect's node id needs its repository, which the typed decision does not carry.
    let defect_node_id = match super::triage_read::read_triage_facts(
        state,
        &request.graph,
        verified,
        &request.defect_id,
    )
    .await?
    .defect
    {
        Some(defect) => {
            medulla_source_types::lifecycle::defect_node_id(&defect.repo, &defect.id)
        }
        None => return Err("DEFECT_UNKNOWN: the tenant graph holds no such defect".to_string()),
    };
    let committed = super::stat_log::handle_log(
        state,
        req_id,
        verified,
        DecisionLogOp::Commit {
            record: Box::new(record),
            evaluator: None,
            approval: None,
        },
    )
    .await;
    if let Some(refusal) = committed.refusal_text() {
        return Err(refusal);
    }
    Ok(TriageDefectOutcome {
        record_id,
        decision,
        defect_node_id,
    })
}
