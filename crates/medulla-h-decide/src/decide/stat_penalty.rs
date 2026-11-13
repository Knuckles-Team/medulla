//! The importance penalty of a `DecisionFit` request, resolved for the fit
//! (MEDULLA-DECISION-ENGINE-R151.3) and its evidence read back for an
//! evaluation (MEDULLA-DECISION-ENGINE-R151.5).
//!
//! The caller supplies the importance weights; nothing here computes one. The
//! parent head comes from the tenant's own published components, so a pin that
//! names another tenant's head does not resolve.

use medulla_agent_types::agent_component::AgentComponentKind;
use medulla_decision_stats::decision::importance_penalty::ImportancePenalty;
use medulla_decision_stats::decision::quant::value_of;
use medulla_decision_types::decision::importance_penalty::PenaltyReceipt;
use medulla_decision_types::decision::jobs::DecisionFitRequest;
use medulla_decision_types::decision::statistical::dataset::LabelledDataset;
use medulla_decision_types::decision::statistical::head::DecisionHeadBody;
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_jobs::{decode_artifact, penalty_key};

use super::stat_support::{pinned_body, refusal};

/// A penalty ready for the optimiser, with the evidence it leaves behind.
pub(super) struct ResolvedPenalty {
    pub(super) penalty: ImportancePenalty,
    pub(super) receipt: PenaltyReceipt,
}

/// The penalty a fit request names, or `None` for an unpenalised fit. The
/// importance of a feature without an entry is 0; a missing parent head is a
/// parent of all zeros.
pub(super) fn resolve_penalty(
    store: &AgentLibraryStore,
    request: &DecisionFitRequest,
    schema_digest: &str,
    dataset: &LabelledDataset,
) -> Result<Option<ResolvedPenalty>, String> {
    let Some(objective) = &request.penalty else {
        return Ok(None);
    };
    let names = dataset.feature_names.as_slice();
    check_importance(request, names)?;
    let mut importance = vec![0.0; names.len()];
    for entry in request.features.iter() {
        if let Some(k) = names.iter().position(|name| *name == entry.feature_id) {
            importance[k] = value_of(entry.importance_weight);
        }
    }
    let parent = match &request.parent_head {
        None => vec![0.0; names.len()],
        Some(pin) => {
            let (head, _) = pinned_body::<DecisionHeadBody>(
                store,
                &request.tenant_id,
                pin,
                AgentComponentKind::DecisionHead,
                StatisticalErrorCode::HeadInvalid,
            )?;
            let head = head
                .checked()
                .map_err(|detail| refusal(StatisticalErrorCode::HeadInvalid, detail))?;
            if head.feature_schema_digest != schema_digest || head.weights.len() != names.len() {
                return Err(refusal(
                    StatisticalErrorCode::HeadInvalid,
                    "the parent head was fitted under a different feature schema",
                ));
            }
            head.weights.iter().map(|w| value_of(*w)).collect()
        }
    };
    let penalty = ImportancePenalty::new(
        value_of(objective.loss_coefficient),
        value_of(objective.importance_scale),
        importance,
        parent,
    )
    .ok_or_else(|| {
        refusal(
            StatisticalErrorCode::HeadInvalid,
            "the penalty does not cover the dataset's features",
        )
    })?;
    let receipt = PenaltyReceipt::seal(objective, request.features.as_slice())
        .map_err(|detail| refusal(StatisticalErrorCode::HeadInvalid, detail))?;
    Ok(Some(ResolvedPenalty { penalty, receipt }))
}

/// Refuse a penalty whose importance list is empty, names a feature outside the
/// pinned schema, or carries an entry that is out of range or without
/// well-formed evidence (MEDULLA-DECISION-ENGINE-R151.4). Runs before the fit
/// starts, so a refusal writes no artifact.
fn check_importance(request: &DecisionFitRequest, names: &[String]) -> Result<(), String> {
    use medulla_decision_types::decision::importance_penalty::{
        is_sha256_hex, weight_in_unit_interval,
    };
    if request.features.is_empty() {
        return Err(refusal(
            StatisticalErrorCode::ImportanceFeaturesEmpty,
            "a penalty names at least one feature importance",
        ));
    }
    for entry in request.features.iter() {
        let feature = &entry.feature_id;
        if !weight_in_unit_interval(&entry.importance_weight) {
            return Err(refusal(
                StatisticalErrorCode::ImportanceOutOfRange,
                format!("importance_weight of {feature} is outside [0, 1]"),
            ));
        }
        if !is_sha256_hex(&entry.evidence_digest) {
            return Err(refusal(
                StatisticalErrorCode::ImportanceEvidenceMissing,
                format!("evidence_digest of {feature} is not 64 lowercase hex characters"),
            ));
        }
        if !names.iter().any(|name| name == feature) {
            return Err(refusal(
                StatisticalErrorCode::ImportanceUnknownFeature,
                format!("{feature} is not a feature of the pinned schema"),
            ));
        }
    }
    Ok(())
}

/// The premises a draft's penalty adds to an evaluation of that draft: the one
/// `penalty:<digest>` entry of a penalised fit, none otherwise.
pub(super) fn draft_premises(
    store: &AgentLibraryStore,
    tenant_id: &str,
    draft_sha256: &str,
) -> Result<Vec<String>, String> {
    let Some(bytes) = store.decision_artifact(tenant_id, &penalty_key(draft_sha256))? else {
        return Ok(Vec::new());
    };
    let receipt: PenaltyReceipt = decode_artifact(&bytes, "penalty receipt")?;
    Ok(vec![receipt.premise()])
}
