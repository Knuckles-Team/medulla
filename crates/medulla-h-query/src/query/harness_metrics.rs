//! MEDULLA-ORCH-VOCAB-R010.1 to R010.5 and R010.101: the harness metric
//! statements of `medulla_types::harness_metrics::HARNESS_QUERIES`, served by
//! `Method::Uql`. The statement reads the `RunTrace` nodes of the caller's
//! RLS-filtered snapshot, aggregates per group and returns rows; it writes
//! nothing. `run_trace_usage` also reads the pinned `PriceTable` node of the
//! same snapshot.

use super::*;

use medulla_types::harness_metrics::{
    price_table_from_node, run_harness_query, run_trace_usage, HarnessQuery, RunTraceUsageQuery,
    RUN_TRACE_USAGE,
};

/// Answer one harness statement over the caller's visible `RunTrace` nodes.
#[cfg(feature = "query")]
pub(crate) fn handle_harness_metric(
    ctx: &QueryHandlerCtx<'_>,
    query: &HarnessQuery,
    params: &UqlParams,
) -> Response {
    if query.name == RUN_TRACE_USAGE {
        return handle_run_trace_usage(ctx, query, params);
    }
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
            provenance: Default::default(),
        })
        .collect();
    let result = medulla_types::wire::UqlResult::Rows {
        columns: query.columns.iter().map(|c| (*c).to_string()).collect(),
        rows,
        warnings,
        snapshot_revision: version,
        knowledge_revision: 0,
        source_coverage: Vec::new(),
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

/// A text parameter of the statement.
#[cfg(feature = "query")]
fn text_param(params: &UqlParams, name: &str) -> Option<String> {
    match params.get(name)? {
        medulla_types::wire::UqlParam::Str(text) => Some(text.clone()),
        _ => None,
    }
}

/// A numeric parameter of the statement.
#[cfg(feature = "query")]
fn number_param(params: &UqlParams, name: &str) -> Option<f64> {
    match params.get(name)? {
        medulla_types::wire::UqlParam::Num(value) => Some(*value),
        _ => None,
    }
}

/// MEDULLA-ORCH-VOCAB-R010.101: the token and cost totals of the caller's
/// visible `RunTrace` nodes in the window, grouped as asked and priced with the
/// pinned `PriceTable` revision of the same snapshot. The page size is held to
/// the registry knob `orchestration_limits.page_size_max`.
#[cfg(feature = "query")]
fn handle_run_trace_usage(
    ctx: &QueryHandlerCtx<'_>,
    query: &HarnessQuery,
    params: &UqlParams,
) -> Response {
    let page_size_max = medulla_server_api::knob_config::knobs().orchestration_limits_page_size_max;
    let usage = match RunTraceUsageQuery::from_params(
        |name| text_param(params, name),
        |name| number_param(params, name),
        page_size_max,
    ) {
        Ok(usage) => usage,
        Err(refusal) => return Response::err(ctx.req_id, refusal),
    };
    let (snap, version) = sql_read_snapshot(
        ctx.core,
        #[cfg(feature = "security")]
        ctx.caller,
        #[cfg(feature = "security")]
        ctx.rls,
    );
    let mut traces = Vec::new();
    let mut table = None;
    for id in snap.node_map.keys() {
        let Some(props) = snap.node_row_object(id) else {
            continue;
        };
        match props.get("node_type").and_then(|v| v.as_str()) {
            Some("RunTrace") => traces.push(props),
            Some("PriceTable") => {
                if let Some((revision, priced)) = price_table_from_node(&props) {
                    if revision == usage.price_table_revision {
                        table = Some(priced);
                    }
                }
            }
            _ => {}
        }
    }
    let page = match run_trace_usage(&usage, &traces, table.as_ref(), version) {
        Ok(page) => page,
        Err(refusal) => return Response::err(ctx.req_id, refusal),
    };
    let rows = page
        .rows
        .into_iter()
        .map(|row| medulla_types::wire::UqlRow {
            id: row.group,
            score: None,
            channels: row.values,
            knowledge: None,
            proof: None,
            provenance: Default::default(),
        })
        .collect();
    let result = medulla_types::wire::UqlResult::Rows {
        columns: query.columns.iter().map(|c| (*c).to_string()).collect(),
        rows,
        warnings: page.warnings,
        snapshot_revision: version,
        knowledge_revision: 0,
        source_coverage: Vec::new(),
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
