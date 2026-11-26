//! `Method::RunMetrics` (MEDULLA-ORCH-VOCAB-R811.1): cost, tokens and solved count per run
//! trace group, over the caller's row-projected core.
//!
//! The handler only reads each `:RunTrace` node into the plain `RunSample` of
//! `medulla_types::run_metrics` and calls its one `aggregate`. Rows the caller cannot read are
//! not in the projected core, so the read never reaches them. Nothing here writes.

use super::*;
use medulla_protocol_types::result_contract::coordination::RunMetrics;
use medulla_types::run_metrics::{aggregate, RunMetricsRequest, RunSample};

type Props = serde_json::Map<String, serde_json::Value>;

/// The token counters a run reports; a run reports none when every one is absent.
const TOKEN_KEYS: [&str; 3] = ["uncachedInputTokens", "cachedInputTokens", "outputTokens"];

fn number(props: &Props, key: &str) -> Option<f64> {
    match props.get(key)? {
        serde_json::Value::Number(number) => number.as_f64(),
        // An `xsd:decimal` is stored as its lexical form.
        serde_json::Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

fn text(props: &Props, key: &str) -> String {
    props
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn is_run_trace(props: &Props) -> bool {
    ["node_type", "type", "label"]
        .iter()
        .find_map(|key| props.get(*key).and_then(serde_json::Value::as_str))
        == Some("RunTrace")
}

/// The sample one `:RunTrace` node carries.
fn sample_of(props: &Props) -> RunSample {
    let reported: Vec<f64> = TOKEN_KEYS
        .iter()
        .filter_map(|key| number(props, key))
        .collect();
    RunSample {
        agent: text(props, "agentId"),
        project: text(props, "projectId"),
        termination: text(props, "terminationReason"),
        completed_at_ms: number(props, "completedAtMs").map(|at| at as u64),
        cost_usd: number(props, "costUsd"),
        tokens: (!reported.is_empty()).then(|| reported.iter().map(|n| n.max(0.0) as u64).sum()),
    }
}

/// Serve one `RunMetrics` request over the caller's projection of the request graph.
pub(crate) fn handle_run_metrics(
    ctx: &QueryHandlerCtx<'_>,
    request: RunMetricsRequest,
) -> Response {
    let Some((authority, carrier)) = ctx
        .read_authority
        .and_then(|authority| authority.carrier().map(|carrier| (authority, carrier)))
    else {
        return Response::err(
            ctx.req_id,
            "ACCESS_DENIED: run metrics need a verified caller",
        );
    };
    if let Err(error) = request.validate() {
        return Response::err(ctx.req_id, error);
    }
    // A tenant other than the verified one is refused alike whether or not it exists.
    if medulla_server_api::carrier::tenant_scope_of(&request.tenant) != carrier.tenant_scope() {
        return Response::err(
            ctx.req_id,
            "ACCESS_DENIED: run metrics tenant must match the verified request tenant",
        );
    }
    let projected = authority.project_core(ctx.core);
    let samples = projected
        .node_ids()
        .into_iter()
        .filter_map(|id| {
            let blob = projected.get_node_properties(&id)?;
            medulla_types::msgpack::decode_property_object(&blob).ok()
        })
        .filter(is_run_trace)
        .map(|props| sample_of(&props));
    let answer = aggregate(&request, samples);
    Response::ok(ctx.req_id, ResultPayload::of::<RunMetrics>(answer))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(value: serde_json::Value) -> Props {
        value.as_object().cloned().unwrap()
    }

    // spec: MEDULLA-ORCH-VOCAB-R811.1
    #[test]
    fn a_run_trace_node_reads_into_its_sample_and_other_nodes_are_not_runs() {
        let node = props(serde_json::json!({
            "node_type": "RunTrace",
            "agentId": "agent-a",
            "projectId": "project-a",
            "terminationReason": "final_answer",
            "completedAtMs": 42,
            "costUsd": "0.25",
            "uncachedInputTokens": 10,
            "cachedInputTokens": 5,
            "outputTokens": 7,
        }));
        assert!(is_run_trace(&node));
        let sample = sample_of(&node);
        assert_eq!(sample.agent, "agent-a");
        assert_eq!(sample.completed_at_ms, Some(42));
        assert_eq!(sample.cost_usd, Some(0.25));
        assert_eq!(sample.tokens, Some(22));
        // A run that reports no counter reports no tokens, not zero.
        let bare = sample_of(&props(serde_json::json!({"node_type": "RunTrace"})));
        assert_eq!((bare.tokens, bare.cost_usd), (None, None));
        assert!(!is_run_trace(&props(serde_json::json!({"node_type": "Span"}))));
    }
}
