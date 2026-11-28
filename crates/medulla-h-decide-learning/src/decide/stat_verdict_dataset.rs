//! The ladder head's training set: the tenant's stored paired verdicts
//! (MEDULLA-DECISION-ENGINE-R134.12.1).
//!
//! `DecisionFit` with `DatasetSource::PairedVerdicts` reads every
//! `run_record/paired_verdict/` row of the caller's tenant, joins each to the
//! trajectory-ladder decision it answers (to recover the arm and baseline
//! rungs and the decision's time) and labels one item per verdict by its
//! winner. A verdict whose decision is gone, is not a ladder decision, or does
//! not name rungs of that ladder is skipped: it cannot be turned into an item.
//! The set is marked synthetic by the fit, as every caller-authored dataset is.

use medulla_decision_statistical_types::decision::statistical::errors::StatisticalErrorCode;
use medulla_decision_statistical_types::decision::QuantScaleTag;
use medulla_decision_stats::decision::trajectory_ladder::{
    verdict_item, VerdictRow, LADDER_FEATURE_NAMES,
};
use medulla_decision_types::decision::statistical::log::DecisionLogEntry;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_persistence_records::decision_jobs::{decode_artifact, record_key};
use medulla_types::contract::BoundedVec;
use {
    medulla_decision_policy_types::decision::statistical::dataset::LabelledDataset,
    medulla_decision_policy_types::decision::statistical::dataset::LABELLED_DATASET_SCHEMA_VERSION,
};
use {
    medulla_decision_program_types::decision::trajectory_ladder::PairedVerdict,
    medulla_decision_program_types::decision::trajectory_ladder::VERDICT_PREFIX,
};

use medulla_h_decide_base::decide::run_record::ladder_of_entry;
use medulla_h_decide_base::decide::stat_support::refusal;

/// Verdict rows one fit reads.
const MAX_VERDICTS: usize = 4096;

/// One labelled item per stored verdict of `tenant_id`, under the feature
/// schema whose digest is `schema_digest`.
pub fn verdict_dataset(
    store: &AgentLibraryStore,
    tenant_id: &str,
    schema_digest: &str,
) -> Result<LabelledDataset, String> {
    let mut items = Vec::new();
    for (_, bytes) in
        store.decision_artifacts_with_prefix(tenant_id, VERDICT_PREFIX, MAX_VERDICTS)?
    {
        let verdict: PairedVerdict = decode_artifact(&bytes, "paired verdict")?;
        let Some(held) = store.decision_artifact(tenant_id, &record_key(&verdict.decision_id))?
        else {
            continue;
        };
        let entry: DecisionLogEntry = decode_artifact(&held, "decision log entry")?;
        let Some(asked) = ladder_of_entry(&entry) else {
            continue;
        };
        let find = |id: &str| asked.rungs.iter().find(|rung| rung.rung_id == id);
        let (Some(arm), Some(baseline)) = (find(&verdict.rung_id), find(&verdict.baseline_id))
        else {
            continue;
        };
        items.extend(verdict_item(&VerdictRow {
            verdict: &verdict,
            arm,
            baseline,
            decided_at_ms: entry.record.created_at_ms,
        }));
    }
    if items.is_empty() {
        return Err(refusal(
            StatisticalErrorCode::NoAdmissibleLabels,
            "no stored paired verdict of a trajectory-ladder decision",
        ));
    }
    let invalid = |detail: String| refusal(StatisticalErrorCode::DatasetInvalid, detail);
    Ok(LabelledDataset {
        schema_version: LABELLED_DATASET_SCHEMA_VERSION,
        feature_schema_digest: schema_digest.to_string(),
        feature_names: BoundedVec::new(
            LADDER_FEATURE_NAMES.iter().map(|n| n.to_string()).collect(),
        )
        .map_err(invalid)?,
        scale: QuantScaleTag::Q32,
        items: BoundedVec::new(items).map_err(invalid)?,
        synthetic: true,
    })
}
