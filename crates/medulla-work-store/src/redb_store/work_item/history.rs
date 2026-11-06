//! Run history rows a terminal ordinary commit writes
//! (MEDULLA-UNIFIED-DATA-PLANE-R750.754).
//!
//! The run audit row, its tool-call rows and its `Step` nodes are graph rows
//! in the commit's own redb transaction: one batch either lands all of them
//! with the result row or none. Keys are deterministic, so an already-present
//! row is left as it is (append is idempotent per run, call and step).

use super::*;
use medulla_work_types::run_history::{
    validate_history, RunAuditRecord, StoredStep, ToolCallRecord,
};
use sha2::{Digest, Sha256};

/// The graph label of a stored conversation step.
pub const STEP_LABEL: &str = "Step";
/// The graph label of a run audit row.
pub const RUN_AUDIT_LABEL: &str = "RunAudit";
/// The graph label of a tool-call row.
pub const TOOL_CALL_LABEL: &str = "ToolCall";

/// One history row: its node id and its property map.
pub type HistoryRow = (String, serde_json::Map<String, serde_json::Value>);

fn keyed_id(kind: &str, parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in std::iter::once(&kind).chain(parts) {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part.as_bytes());
    }
    format!("{}:{}", kind, hex::encode(digest.finalize()))
}

fn labelled<T: serde::Serialize>(
    label: &str,
    record: &T,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let serde_json::Value::Object(mut props) =
        serde_json::to_value(record).map_err(|error| error.to_string())?
    else {
        return Err("history record is not an object".into());
    };
    props.insert("node_type".into(), label.into());
    Ok(props)
}

/// Validate and lower one commit's history to graph rows. Refuses with the
/// registered `IDENTITY_NOT_REDACTED` when any identity is raw.
pub fn history_rows(
    run: &RunAuditRecord,
    calls: &[ToolCallRecord],
    steps: &[StoredStep],
) -> Result<Vec<HistoryRow>, String> {
    validate_history(run, calls, steps).map_err(|refusal| refusal.message())?;
    let mut rows = vec![(
        keyed_id("run-audit", &[&run.tenant, &run.run_id]),
        labelled(RUN_AUDIT_LABEL, run)?,
    )];
    for call in calls {
        let id = keyed_id("tool-call", &[&call.tenant, &call.run_id, &call.call_id]);
        rows.push((id, labelled(TOOL_CALL_LABEL, call)?));
    }
    for step in steps {
        let index = step.index.to_string();
        let id = keyed_id("step", &[&step.tenant, &step.run_id, &index]);
        rows.push((id, labelled(STEP_LABEL, step)?));
    }
    Ok(rows)
}

/// Write the rows into the open commit transaction, skipping a row whose key
/// is already present, and name each written row in `changed` so the serving
/// projection publishes it.
pub fn write_history_rows(
    graph: &str,
    rows: Vec<HistoryRow>,
    nodes: &mut ScopedOwnerTableMut<'_, (&str, &str), &[u8]>,
    crypto: DurableCrypto<'_>,
    changed: &mut Vec<String>,
) -> Result<(), String> {
    for (id, props) in rows {
        if nodes.get((graph, id.as_str()))?.is_some() {
            continue;
        }
        let bytes = rmp_serde::to_vec_named(&props).map_err(|error| error.to_string())?;
        let sealed = crypto.seal(&bytes);
        nodes
            .insert((graph, id.as_str()), sealed.as_ref())
            .map_err(|error| error.to_string())?;
        changed.push(id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_work_types::run_history::{
        HistoryCommitFacts, RedactedIdentity, RunHistoryInput, StepInput, ToolCallInput,
    };

    fn input(speaker: RedactedIdentity) -> RunHistoryInput {
        RunHistoryInput {
            scope: "project:alpha".into(),
            actor: RedactedIdentity::redact("alice@example.org"),
            trace_node: "trace:r1".into(),
            tool_calls: vec![ToolCallInput {
                call_id: "c1".into(),
                tool: "graph_query".into(),
                trace_node: "trace:r1/c1".into(),
                args_digest: "sha256:00".into(),
            }],
            steps: vec![
                StepInput {
                    index: 0,
                    speaker: speaker.clone(),
                    text: "plan the release".into(),
                },
                StepInput {
                    index: 1,
                    speaker,
                    text: "ship the wheel".into(),
                },
            ],
        }
    }

    const FACTS: HistoryCommitFacts<'static> = HistoryCommitFacts {
        tenant: "acme",
        run_id: "r1",
        outcome: "succeeded",
        at_ms: 7,
    };

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R750.754
    #[test]
    fn history_rows_label_steps_and_refuse_raw_identity() {
        let (run, calls, steps) = input(RedactedIdentity::redact("bob@example.org")).records(FACTS);
        let rows = history_rows(&run, &calls, &steps).unwrap();
        let labels: Vec<&str> = rows
            .iter()
            .map(|(_, p)| p["node_type"].as_str().unwrap())
            .collect();
        assert_eq!(
            labels,
            [RUN_AUDIT_LABEL, TOOL_CALL_LABEL, STEP_LABEL, STEP_LABEL]
        );
        assert_eq!(
            rows,
            history_rows(&run, &calls, &steps).unwrap(),
            "keys are deterministic"
        );

        let raw: RedactedIdentity = serde_json::from_str("\"bob@example.org\"").unwrap();
        let (run, calls, steps) = input(raw).records(FACTS);
        let refused = history_rows(&run, &calls, &steps).unwrap_err();
        assert!(refused.starts_with("IDENTITY_NOT_REDACTED"), "{refused}");
    }
}
