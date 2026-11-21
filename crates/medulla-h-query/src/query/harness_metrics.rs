//! MEDULLA-ORCH-VOCAB-R010.1 to R010.5 and R010.101: the harness metric
//! statements of `medulla_mutation_types::harness_metrics::HARNESS_QUERIES`, served by
//! `Method::Uql`. The statement reads the `RunTrace` nodes of the caller's
//! RLS-filtered snapshot, aggregates per group and returns rows; it writes
//! nothing. `run_trace_usage` also reads the pinned `PriceTable` node of the
//! same snapshot.

use super::*;

use medulla_mutation_types::evolution_metrics::{
    evolution_trace_metrics, EvolutionTraceQuery, EVOLUTION_TRACE_METRICS,
};
use medulla_mutation_types::harness_metrics::{
    harness_config_evidence, price_table_from_node, run_harness_query, run_trace_usage,
    ConfigEvidenceQuery, HarnessQuery, RunTraceUsageQuery, TraceSufficiency,
    HARNESS_CONFIG_EVIDENCE, RUN_TRACE_USAGE,
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
    if query.name == EVOLUTION_TRACE_METRICS {
        return handle_evolution_trace_metrics(ctx, query, params);
    }
    if query.name == HARNESS_CONFIG_EVIDENCE {
        return handle_harness_config_evidence(ctx, query, params);
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
        .map(|row| medulla_query_types::wire::UqlRow {
            id: row.group,
            score: None,
            channels: row.values,
            knowledge: None,
            proof: None,
            provenance: Default::default(),
        })
        .collect();
    let result = medulla_query_types::wire::UqlResult::Rows {
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
        medulla_query_types::wire::UqlParam::Str(text) => Some(text.clone()),
        _ => None,
    }
}

/// A numeric parameter of the statement.
#[cfg(feature = "query")]
fn number_param(params: &UqlParams, name: &str) -> Option<f64> {
    match params.get(name)? {
        medulla_query_types::wire::UqlParam::Num(value) => Some(*value),
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
        .map(|row| medulla_query_types::wire::UqlRow {
            id: row.group,
            score: None,
            channels: row.values,
            knowledge: None,
            proof: None,
            provenance: Default::default(),
        })
        .collect();
    let result = medulla_query_types::wire::UqlResult::Rows {
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

/// MEDULLA-ORCH-VOCAB-R010.100: one evolution proposal joined to its measured source traces.
/// One RLS-filtered snapshot is read; a proposal of another tenant is outside it and reads as
/// `EVOLUTION_REVISION_UNKNOWN`. The statement writes nothing.
#[cfg(feature = "query")]
fn handle_evolution_trace_metrics(
    ctx: &QueryHandlerCtx<'_>,
    query: &HarnessQuery,
    params: &UqlParams,
) -> Response {
    let evolution = match EvolutionTraceQuery::from_params(
        |name| text_param(params, name),
        |name| number_param(params, name),
    ) {
        Ok(evolution) => evolution,
        Err(refusal) => return Response::err(ctx.req_id, refusal),
    };
    let (snap, version) = sql_read_snapshot(
        ctx.core,
        #[cfg(feature = "security")]
        ctx.caller,
        #[cfg(feature = "security")]
        ctx.rls,
    );
    let nodes: std::collections::BTreeMap<_, _> = snap
        .node_map
        .keys()
        .filter_map(|id| snap.node_row_object(id).map(|props| (id.clone(), props)))
        .collect();
    let answer = match evolution_trace_metrics(&evolution, &nodes) {
        Ok(answer) => answer,
        Err(refusal) => return Response::err(ctx.req_id, refusal),
    };
    let warnings = answer
        .missing_ids
        .iter()
        .map(|id| format!("missing trace: {id}"))
        .collect();
    let result = medulla_query_types::wire::UqlResult::Rows {
        columns: query.columns.iter().map(|c| (*c).to_string()).collect(),
        rows: answer.to_rows(),
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

/// MEDULLA-ORCH-VOCAB-R010.6: the cost per solved task of each harness configuration
/// of one model and task class, with the trace ids behind each row and whether
/// they reach the `$min_traces` threshold. One RLS-filtered snapshot is read;
/// the statement writes nothing and decides nothing.
#[cfg(feature = "query")]
fn handle_harness_config_evidence(
    ctx: &QueryHandlerCtx<'_>,
    query: &HarnessQuery,
    params: &UqlParams,
) -> Response {
    let evidence = match ConfigEvidenceQuery::from_params(
        |name| text_param(params, name),
        |name| number_param(params, name),
    ) {
        Ok(evidence) => evidence,
        Err(refusal) => return Response::err(ctx.req_id, refusal),
    };
    let (snap, version) = sql_read_snapshot(
        ctx.core,
        #[cfg(feature = "security")]
        ctx.caller,
        #[cfg(feature = "security")]
        ctx.rls,
    );
    let nodes: Vec<(String, _)> = snap
        .node_map
        .keys()
        .filter_map(|id| snap.node_row_object(id).map(|props| (id.clone(), props)))
        .filter(|(_, props)| props.get("node_type").and_then(|v| v.as_str()) == Some("RunTrace"))
        .collect();
    let traces: Vec<_> = nodes.iter().map(|(id, props)| (id.clone(), props)).collect();
    let answered = harness_config_evidence(&evidence, &traces);
    let warnings = answered
        .iter()
        .filter_map(|row| {
            row.abstained
                .map(|reason| format!("abstained {reason}: {}", row.config))
        })
        .collect();
    let rows = answered
        .into_iter()
        .map(|row| medulla_query_types::wire::UqlRow {
            id: row.config,
            score: None,
            channels: vec![
                row.cost_per_solved_task,
                Some(row.solved as f64),
                Some(row.total as f64),
            ],
            knowledge: None,
            proof: None,
            provenance: [
                (
                    "trace_ids".to_string(),
                    serde_json::to_string(&row.trace_ids).unwrap_or_default(),
                ),
                (
                    "sufficiency".to_string(),
                    match row.sufficiency {
                        TraceSufficiency::Sufficient => "sufficient",
                        TraceSufficiency::Insufficient => "insufficient",
                    }
                    .to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        })
        .collect();
    let result = medulla_query_types::wire::UqlResult::Rows {
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
