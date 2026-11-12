//! MEDULLA-ORCH-VOCAB-R010.1 to R010.5: the harness metric statements of
//! `medulla_types::harness_metrics::HARNESS_QUERIES`, served by `Method::Uql`.
//! The statement reads the `RunTrace` nodes of the caller's RLS-filtered
//! snapshot, aggregates per group and returns rows; it writes nothing.

use super::*;

use medulla_types::harness_metrics::{run_harness_query, HarnessQuery};

/// Answer one harness statement over the caller's visible `RunTrace` nodes.
#[cfg(feature = "query")]
pub(crate) fn handle_harness_metric(ctx: &QueryHandlerCtx<'_>, query: &HarnessQuery) -> Response {
    let (snap, version) = sql_read_snapshot(
        ctx.core,
        #[cfg(feature = "security")]
        ctx.caller,
        #[cfg(feature = "security")]
        ctx.rls,
    );
    let traces: Vec<_> = snap
        .node_map
        .keys()
        .filter_map(|id| snap.node_row_object(id))
        .filter(|props| props.get("node_type").and_then(|v| v.as_str()) == Some("RunTrace"))
        .collect();
    let answered = run_harness_query(query, &traces);
    let warnings = answered
        .iter()
        .filter_map(|row| {
            row.abstained
                .map(|reason| format!("abstained {reason}: {}", row.group))
        })
        .collect();
    let rows = answered
        .into_iter()
        .map(|row| medulla_types::wire::UqlRow {
            id: row.group,
            score: None,
            channels: row.values,
            knowledge: None,
            proof: None,
        })
        .collect();
    let result = medulla_types::wire::UqlResult::Rows {
        columns: query.columns.iter().map(|c| (*c).to_string()).collect(),
        rows,
        warnings,
        snapshot_revision: version,
        knowledge_revision: 0,
    };
    served_response::<query_results::Uql>(
        ctx.req_id,
        Ok(Ok(result)),
        "UQL",
        #[cfg(feature = "result-cache")]
        ctx.core,
        #[cfg(feature = "result-cache")]
        None,
    )
}
