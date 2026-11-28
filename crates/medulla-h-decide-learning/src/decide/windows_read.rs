//! The observation windows a `run_adjustment` question reads
//! (MEDULLA-ORCH-VOCAB-R009.18.3): the `RunObservationWindow` nodes the run
//! loop wrote to the graph, read behind the ordinary read ACL like
//! `steps_read::read`. The family never reads the graph itself; the serving
//! path reads here and hands the result to the family in the execution context.

use std::collections::BTreeMap;

use medulla_decision_policy_types::decision::run_adjustment::WidthKnobs;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_work_types::run_observation::RunObservationWindow;

use {medulla_h_decide_base::decide::topology_read, medulla_h_decide_base::decide::SharedState};

/// The node label of a stored window.
const WINDOW_LABEL: &str = "RunObservationWindow";
/// The engine-stamped keys the window record does not carry.
const ENGINE_KEYS: [&str; 4] = ["node_type", "tx_from", "error_rate", "run_trace_ref"];

/// The windows of `run_id` in `graph`: the highest `window_revision` of each
/// `window_id`, newest first by `end_ms`. A graph the caller may not read
/// refuses as every other read does.
pub async fn read_windows(
    state: &SharedState,
    graph: &str,
    verified: &VerifiedRequestContext,
    run_id: &str,
) -> Result<Vec<RunObservationWindow>, String> {
    let core = topology_read::graph_core(state, graph, verified).await?;
    let run_id = run_id.to_string();
    tokio::task::spawn_blocking(move || {
        windows_of(core.get_nodes_by_label(WINDOW_LABEL, 0), &run_id)
    })
    .await
    .map_err(|error| format!("observation window read task failed: {error}"))?
}

/// The windows `nodes` hold for `run_id`, newest revision per window id,
/// newest first by `end_ms` (then by window id).
fn windows_of(
    nodes: Vec<(String, Vec<u8>)>,
    run_id: &str,
) -> Result<Vec<RunObservationWindow>, String> {
    let mut newest: BTreeMap<String, RunObservationWindow> = BTreeMap::new();
    for (node_id, blob) in nodes {
        let Ok(mut props) = medulla_types::msgpack::decode_property_object(&blob) else {
            continue;
        };
        if props.get("run_trace_ref").and_then(|v| v.as_str()) != Some(run_id) {
            continue;
        }
        for key in ENGINE_KEYS {
            props.remove(key);
        }
        let window: RunObservationWindow = serde_json::from_value(serde_json::Value::Object(props))
            .map_err(|error| format!("observation window '{node_id}' is malformed: {error}"))?;
        match newest.get(&window.window_id) {
            Some(held) if held.window_revision >= window.window_revision => {}
            _ => {
                newest.insert(window.window_id.clone(), window);
            }
        }
    }
    let mut windows: Vec<RunObservationWindow> = newest.into_values().collect();
    windows.sort_by(|a, b| {
        b.end_ms
            .cmp(&a.end_ms)
            .then_with(|| a.window_id.cmp(&b.window_id))
    });
    Ok(windows)
}

/// The windows inside the age bound, at most `width.window` of them: drops a
/// window whose `end_ms` is older than `now_ms - width.window_max_age_s * 1000`
/// and keeps the newest. `windows` arrive newest first.
pub(in crate::decide) fn select_windows(
    windows: Vec<RunObservationWindow>,
    now_ms: u64,
    knobs: &WidthKnobs,
) -> Vec<RunObservationWindow> {
    let since_ms = now_ms.saturating_sub(knobs.window_max_age_s.saturating_mul(1_000));
    windows
        .into_iter()
        .filter(|window| window.end_ms >= since_ms)
        .take(knobs.window as usize)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_work_types::run_observation::{Measured, WindowUsage};

    fn unknown<T>() -> Measured<T> {
        Measured::Unknown("not reported".into())
    }

    fn window(id: &str, revision: u64, end_ms: u64) -> RunObservationWindow {
        RunObservationWindow {
            run_id: "run-1".into(),
            run_revision: 1,
            window_id: id.into(),
            window_revision: revision,
            start_ms: end_ms.saturating_sub(10),
            end_ms,
            latency_samples_ms: Measured::Measured(vec![5]),
            error_count: Measured::Measured(0),
            eligible_attempts: Measured::Measured(4),
            usage: WindowUsage {
                input_tokens: unknown(),
                output_tokens: unknown(),
                cache_read_tokens: unknown(),
                cache_write_tokens: unknown(),
            },
            cost: unknown(),
            model_ref: unknown(),
            profile_revision: unknown(),
            width: unknown(),
            budget_consumed: unknown(),
            budget_remaining: unknown(),
            capacity_receipt_ref: unknown(),
        }
    }

    fn node(window: &RunObservationWindow, run: &str) -> (String, Vec<u8>) {
        let mut props = serde_json::to_value(window).unwrap();
        props["node_type"] = WINDOW_LABEL.into();
        props["run_trace_ref"] = run.into();
        props["error_rate"] = serde_json::json!({"state": "unknown", "value": "x"});
        props["tx_from"] = 1.into();
        (
            format!("{}@{}", window.window_id, window.window_revision),
            rmp_serde::to_vec_named(&props).unwrap(),
        )
    }

    // spec: MEDULLA-ORCH-VOCAB-R009.18.3
    #[test]
    fn newest_revision_per_window_inside_age_bound() {
        let nodes = vec![
            node(&window("w-a", 1, 1_000_000), "run-1"),
            node(&window("w-a", 2, 1_000_500), "run-1"),
            node(&window("w-b", 1, 1_001_000), "run-1"),
            node(&window("w-old", 1, 10), "run-1"),
            node(&window("w-other", 1, 1_002_000), "run-2"),
        ];
        let read = windows_of(nodes, "run-1").unwrap();
        let ids: Vec<(&str, u64)> = read
            .iter()
            .map(|w| (w.window_id.as_str(), w.window_revision))
            .collect();
        assert_eq!(ids, vec![("w-b", 1), ("w-a", 2), ("w-old", 1)]);

        let knobs = WidthKnobs {
            error_hi_micros: 20_000,
            error_lo_micros: 5_000,
            window: 2,
            min_runs: 1,
            latency_p95_cap_ms: 2_000,
            window_max_age_s: 600,
        };
        let kept = select_windows(read.clone(), 1_100_000, &knobs);
        let kept_ids: Vec<&str> = kept.iter().map(|w| w.window_id.as_str()).collect();
        assert_eq!(
            kept_ids,
            vec!["w-b", "w-a"],
            "the old window is out of the age bound"
        );
        let one = select_windows(read, 1_100_000, &WidthKnobs { window: 1, ..knobs });
        assert_eq!(one.len(), 1, "at most width.window windows");
        assert!(windows_of(Vec::new(), "run-1").unwrap().is_empty());
    }
}
