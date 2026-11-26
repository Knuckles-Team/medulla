//! `DecisionLog.lifecycle_record` and `DecisionLog.lifecycle_records`: the lifecycle records
//! the one decision log holds (MEDULLA-SDLC-R013, R019.4).
//!
//! A write stores one record under a key its own content determines (the would-dispatch
//! decision id, the scenario run id), so a repeat of an identical write is a replay and
//! never a second row. A scenario run without a phase, a scenario id or a commit refuses
//! `SCENARIO_RUN_INCOMPLETE` before anything is stored. A read lists the stored records of one
//! kind, newest first, by the engine instant each was committed at.

use medulla_decision_types::decision::statistical::lifecycle_record::{
    LifecycleRecordKind, LifecycleRecordQuery, LifecycleRecordStored, LifecycleRecordWrite,
    LifecycleRecordsFound, StoredWouldDispatch, LIFECYCLE_RECORD_SCHEMA_VERSION,
    MAX_LIFECYCLE_RECORDS_READ,
};
use medulla_persistence_records::decision_jobs::{decode_artifact, encode_artifact};
use medulla_source_types::delivery::WouldDispatch;
use medulla_source_types::lifecycle::{sha256_hex, ScenarioRun, StoredScenarioRun};

use super::stat_executor::ExecutionContext;

/// The most stored rows of one kind a read scans.
const MAX_SCAN_ROWS: usize = 100_000;
const KEY_PREFIX: &str = "lifecycle-record:";

fn kind_prefix(kind: LifecycleRecordKind) -> String {
    format!("{KEY_PREFIX}{}:", kind.segment())
}

/// The stable id of one would-dispatch decision: the dispatch decision and the slice root it
/// recorded, so the same slice of the same dispatch is one record.
pub(super) fn would_dispatch_id(record: &WouldDispatch) -> String {
    format!(
        "would-dispatch:{}",
        &sha256_hex(format!("{}\n{}", record.dispatch_decision_id, record.slice_root).as_bytes())
            [..32]
    )
}

/// Serve one `LifecycleRecord` op.
pub(super) fn write(
    ctx: &ExecutionContext,
    write: LifecycleRecordWrite,
) -> Result<LifecycleRecordStored, String> {
    let kind = write.kind();
    let (id, body) = match write {
        LifecycleRecordWrite::WouldDispatch { record } => {
            if record.slice_root.trim().is_empty() || record.dispatch_decision_id.trim().is_empty()
            {
                return Err(
                    "INVALID_ARGUMENT: a would-dispatch record names its slice root and dispatch decision"
                        .to_string(),
                );
            }
            let id = would_dispatch_id(&record);
            let stored = StoredWouldDispatch {
                id: id.clone(),
                record,
                recorded_ms: ctx.now_ms,
            };
            (id, encode_artifact(&stored)?)
        }
        LifecycleRecordWrite::ScenarioRun { record } => {
            let run = record.normalized().map_err(|refusal| refusal.to_string())?;
            let id = run.id();
            let stored = StoredScenarioRun {
                id: id.clone(),
                run,
                recorded_ms: ctx.now_ms,
            };
            (id, encode_artifact(&stored)?)
        }
    };
    let key = format!("{}{id}", kind_prefix(kind));
    if let Some(bytes) = ctx.store.decision_artifact(ctx.tenant_id, &key)? {
        let recorded_ms = match kind {
            LifecycleRecordKind::WouldDispatch => {
                decode_artifact::<StoredWouldDispatch>(&bytes, "would-dispatch record")?
                    .recorded_ms
            }
            LifecycleRecordKind::ScenarioRun => {
                decode_artifact::<StoredScenarioRun>(&bytes, "scenario run")?.recorded_ms
            }
        };
        return Ok(LifecycleRecordStored {
            schema_version: LIFECYCLE_RECORD_SCHEMA_VERSION,
            record_kind: kind,
            record_id: id,
            recorded_ms,
            replayed: true,
        });
    }
    ctx.store
        .put_decision_artifacts(ctx.tenant_id, &[(key, body)])?;
    Ok(LifecycleRecordStored {
        schema_version: LIFECYCLE_RECORD_SCHEMA_VERSION,
        record_kind: kind,
        record_id: id,
        recorded_ms: ctx.now_ms,
        replayed: false,
    })
}

/// Serve one `LifecycleRecords` op.
pub(super) fn read(
    ctx: &ExecutionContext,
    query: &LifecycleRecordQuery,
) -> Result<LifecycleRecordsFound, String> {
    let rows = ctx.store.decision_artifacts_with_prefix(
        ctx.tenant_id,
        &kind_prefix(query.record_kind),
        MAX_SCAN_ROWS,
    )?;
    let mut found = LifecycleRecordsFound {
        record_kind: query.record_kind,
        would_dispatch: Vec::new(),
        scenario_runs: Vec::new(),
    };
    match query.record_kind {
        LifecycleRecordKind::WouldDispatch => {
            for (_, bytes) in rows {
                found
                    .would_dispatch
                    .push(decode_artifact::<StoredWouldDispatch>(
                        &bytes,
                        "would-dispatch record",
                    )?);
            }
            found
                .would_dispatch
                .sort_by(|a, b| b.recorded_ms.cmp(&a.recorded_ms).then(a.id.cmp(&b.id)));
            found.would_dispatch.truncate(MAX_LIFECYCLE_RECORDS_READ);
        }
        LifecycleRecordKind::ScenarioRun => {
            for (_, bytes) in rows {
                let stored = decode_artifact::<StoredScenarioRun>(&bytes, "scenario run")?;
                if matches_query(&stored.run, query) {
                    found.scenario_runs.push(stored);
                }
            }
            found
                .scenario_runs
                .sort_by(|a, b| b.recorded_ms.cmp(&a.recorded_ms).then(a.id.cmp(&b.id)));
            found.scenario_runs.truncate(MAX_LIFECYCLE_RECORDS_READ);
        }
    }
    Ok(found)
}

fn matches_query(run: &ScenarioRun, query: &LifecycleRecordQuery) -> bool {
    query.phase.as_ref().is_none_or(|phase| &run.phase == phase)
        && query.branch.as_ref().is_none_or(|branch| &run.branch == branch)
}
