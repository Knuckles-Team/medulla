//! Admission of `proposals` batches (MEDULLA-INGEST-WORKER-R002.3).
//!
//! The SDK worker runs the model and submits each induced ontology proposal with
//! the terms it observed in the source. The engine never calls a model here: it
//! judges each submission with `judge_proposal` and stores the proposal as node
//! `ontology_proposal:<proposal_id>`. A passing proposal is stored `Admitted`; a
//! failing one is stored `Draft` with the failing check. The refusal lives in the
//! per-item receipt, never as a batch refusal.

use medulla_ingestion_types::ingest_rules::IngestRefusal;
use medulla_ingestion_types::ontology_proposal::{
    score_proposal, ProposalState, ProposalThresholds,
};
use medulla_protocol_types::protocol::Method;
use medulla_source_types::source_ingestion::{ProposalSubmission, SourceJson, MAX_OBSERVED_TERMS};
use medulla_types::contract::ServerErrorCode;
use serde::Serialize;

/// Most submissions one batch carries.
pub const MAX_SUBMISSIONS: usize =
    medulla_source_types::source_ingestion::batch_kind::MAX_PROPOSALS;

/// The judgment of one submission, as it appears in the batch receipt.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProposalItemReceipt {
    pub proposal_id: String,
    pub state: ProposalState,
    /// Share of the observed property names the proposal declares, as judged.
    pub coverage: f64,
    /// Share of classes whose property set duplicates an earlier class, as judged.
    pub redundancy: f64,
    /// `PROPOSAL_NOT_ADMITTED` for a failing proposal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    /// The failing check, `coverage` or `redundancy`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failing_check: Option<String>,
}

/// The methods and receipts of one admitted `proposals` batch.
#[derive(Debug, Default)]
pub struct ProposalAdmission {
    /// One node write per submission, in submission order.
    pub methods: Vec<Method>,
    /// The proposal nodes written: each takes one policy record.
    pub node_ids: Vec<String>,
    pub items: Vec<ProposalItemReceipt>,
}

fn invalid(detail: impl std::fmt::Display) -> String {
    format!(
        "{}: {detail}",
        ServerErrorCode::SourceBatchKindMismatch.as_str()
    )
}

/// Decode the elements of a `proposals` extension.
pub fn decode_proposals(items: &[SourceJson]) -> Result<Vec<ProposalSubmission>, String> {
    if items.len() > MAX_SUBMISSIONS {
        return Err(invalid(format!(
            "a proposals batch holds at most {MAX_SUBMISSIONS} submissions"
        )));
    }
    items
        .iter()
        .map(|item| {
            let submission: ProposalSubmission = serde_json::from_value(item.value()?)
                .map_err(|error| invalid(format!("proposal submission: {error}")))?;
            if submission.observed_terms.len() > MAX_OBSERVED_TERMS {
                return Err(invalid(format!(
                    "observed_terms holds more than {MAX_OBSERVED_TERMS} entries"
                )));
            }
            if submission.proposal.proposal_id.trim().is_empty() {
                return Err(invalid("proposal_id must not be blank"));
            }
            Ok(submission)
        })
        .collect()
}

/// The node id of a stored proposal.
pub fn proposal_node_id(proposal_id: &str) -> String {
    format!("ontology_proposal:{proposal_id}")
}

/// Judge each submission and compile its node write.
pub fn admit_proposals(
    submissions: &[ProposalSubmission],
    thresholds: &ProposalThresholds,
) -> Result<ProposalAdmission, String> {
    let mut admission = ProposalAdmission::default();
    for submission in submissions {
        let mut proposal = submission.proposal.clone();
        // One score, judged once: the scores reported are the ones that decided.
        let score = score_proposal(&proposal, &submission.observed_terms);
        let (state, refusal, failing_check) = match score.failing_check(thresholds) {
            None => (ProposalState::Admitted, None, None),
            Some(check) => (
                ProposalState::Draft,
                Some(IngestRefusal::ProposalNotAdmitted(check).code()),
                Some(format!("{check:?}").to_lowercase()),
            ),
        };
        proposal.state = state;
        let node_id = proposal_node_id(&proposal.proposal_id);
        let properties = serde_json::json!({
            "type": "OntologyProposal",
            "proposal_id": proposal.proposal_id,
            "state": state,
            "failing_check": failing_check,
            "coverage": score.coverage,
            "redundancy": score.redundancy,
            "proposal": proposal,
        });
        admission.methods.push(Method::AddNode {
            node_id: node_id.clone(),
            properties_msgpack: rmp_serde::to_vec_named(&properties)
                .map_err(|error| invalid(format!("proposal encoding failed: {error}")))?,
        });
        admission.node_ids.push(node_id);
        admission.items.push(ProposalItemReceipt {
            proposal_id: proposal.proposal_id,
            state,
            coverage: score.coverage,
            redundancy: score.redundancy,
            refusal,
            failing_check,
        });
    }
    Ok(admission)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn submission(id: &str, declared: &[&str], observed: &[&str]) -> ProposalSubmission {
        let json = serde_json::json!({
            "proposal": {
                "proposal_id": id,
                "classes": [{"name": "Thing", "properties": declared}],
                "properties": [],
                "provenance": {"source_ref": "worker", "induced_by": "m"},
            },
            "observed_terms": observed,
        });
        serde_json::from_value(json).unwrap()
    }

    // spec: MEDULLA-INGEST-WORKER-R002.3
    #[test]
    fn judge_runs_inside_prepare() {
        let passing = submission("p1", &["name", "age"], &["name", "age"]);
        let failing = submission("p2", &["name"], &["name", "age", "city", "zip"]);
        let admission = admit_proposals(
            &[passing, failing],
            &ProposalThresholds {
                min_coverage: 0.8,
                max_redundancy: 0.1,
            },
        )
        .unwrap();
        assert_eq!(
            admission.node_ids,
            ["ontology_proposal:p1", "ontology_proposal:p2"]
        );
        assert_eq!(admission.items[0].state, ProposalState::Admitted);
        assert_eq!(admission.items[0].refusal, None);
        // The receipt carries the scores the judgment used.
        assert_eq!(admission.items[0].coverage, 1.0);
        assert!(admission.items[1].coverage < 0.8);
        // A failing proposal is stored as a draft with the failing check, as an item
        // refusal and not a batch refusal.
        assert_eq!(admission.items[1].state, ProposalState::Draft);
        assert_eq!(
            admission.items[1].refusal.as_deref(),
            Some("PROPOSAL_NOT_ADMITTED")
        );
        assert_eq!(
            admission.items[1].failing_check.as_deref(),
            Some("coverage")
        );
        assert_eq!(admission.methods.len(), 2);

        // Submissions are bounded.
        let too_many: Vec<SourceJson> = (0..=MAX_SUBMISSIONS)
            .map(|i| {
                SourceJson::new(
                    serde_json::to_value(submission(&format!("p{i}"), &[], &[])).unwrap(),
                )
                .unwrap()
            })
            .collect();
        assert!(decode_proposals(&too_many).is_err());
    }
}
