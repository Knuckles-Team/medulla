//! `DecisionLog.learn` with a package run (MEDULLA-DECISION-ENGINE-R160.3).
//!
//! The caller that ran a rendered skill package records whether the run
//! passed. The row is stored once under the tenant's decision artifact store at
//! `package_run/<package digest>/<run id>`: an equal replay answers the stored
//! row and the same key with another verdict refuses `IDEMPOTENCY_CONFLICT`.
//! `PublishSkillPackage` reads the history back; this write is the single
//! writer of `package_run/` rows.

use medulla_decision_types::decision::package_validation::{package_run_key, RunHistoryEntry};

use medulla_h_decide_base::decide::stat_executor::ExecutionContext;
use medulla_h_decide_base::decide::stat_log_reader::store_once;

/// Store one package run exactly once.
pub(super) fn record_package_run(
    ctx: &ExecutionContext,
    run: RunHistoryEntry,
) -> Result<RunHistoryEntry, String> {
    store_once(
        ctx,
        package_run_key(&run.package_digest, &run.run_id),
        "package run",
        run,
        |a, b| a == b,
    )
}
