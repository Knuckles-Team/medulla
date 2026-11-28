//! The retention verification, split out above the replay it re-runs.

use super::stat_replay_hi::replay;
use medulla_decision_statistical_types::decision::statistical::errors::StatisticalErrorCode;
use medulla_h_decide_base::decide::stat_executor::ExecutionContext;
use medulla_h_decide_base::decide::stat_support::refusal;
use {
    medulla_decision_types::decision::statistical::log::DecisionLogVerification,
    medulla_decision_types::decision::statistical::log::EntryInputs,
};
use {
    medulla_h_decide_base::decide::stat_log_reader::visible_entry,
    medulla_h_decide_base::decide::stat_log_reader::LogReader,
};
use {
    medulla_h_decide_base::decide::stat_retention::full_record,
    medulla_h_decide_base::decide::stat_retention::Retention,
};

/// Re-verify one logged record against its inputs, wherever they now live.
pub(super) fn verify(
    ctx: &ExecutionContext,
    reader: &LogReader,
    retention: &Retention,
    record_id: &str,
) -> Result<DecisionLogVerification, String> {
    let entry = visible_entry(ctx.store, reader, record_id)?.ok_or_else(|| {
        refusal(
            StatisticalErrorCode::ParameterInvalid,
            "no committed record with that id is visible",
        )
    })?;
    let Some(record) = full_record(retention, ctx.tenant_id, &entry)? else {
        let EntryInputs::Retired { blob, .. } = &entry.inputs else {
            return Err(refusal(
                StatisticalErrorCode::ParameterInvalid,
                "inputs unavailable",
            ));
        };
        return Ok(DecisionLogVerification::InputsRetired {
            record_digest: entry.record.record_digest.clone(),
            blob_sha256: blob.sha256.clone(),
        });
    };
    replay(ctx, &record)?;
    Ok(DecisionLogVerification::Verified {
        record_digest: record.record_digest,
    })
}
