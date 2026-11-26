//! The shadow check of an incremental plan view (MEDULLA-INCREMENTAL-VIEWS-R008.2).
//!
//! [`check_view`] compares what an incremental view maintains with what a recompute
//! over the same graph version answers. A view that disagrees is demoted to
//! recompute mode with the typed code `SHADOW_MISMATCH`, so a wrong maintained
//! result is never served again.

use std::sync::Arc;
use std::time::Duration;

use medulla_server_api::state::ServerState;
use medulla_server_cdc::matview::{self, PlanMatView};
use medulla_types::contract::ServerErrorCode;
use tokio::sync::RwLock;

/// The environment variable of the tick period; the value is the knob `view.shadow_tick_ms`
/// (MEDULLA-INCREMENTAL-VIEWS-R008.3).
pub const TICK_ENV: &str = "MEDULLA_VIEW_SHADOW_TICK_MS";

/// The most due views one tick checks.
const CHECKS_PER_TICK: usize = 4;

/// The longest one check may run before it counts as skipped.
pub const CHECK_DEADLINE: Duration = Duration::from_secs(30);

/// The most rows of each side one mismatch log line carries.
const LOGGED_ROWS: usize = 5;

/// The result of one shadow check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadowOutcome {
    /// The maintained rows equal the recomputed rows.
    Match,
    /// The rows differ and the view was demoted to recompute mode.
    Mismatch,
    /// The graph moved while the check ran, or the view is no longer incremental;
    /// nothing was compared.
    Skipped,
}

/// A row set in a comparable order: sorted by id, scores by bit pattern.
fn comparable(rows: &[(String, Option<f32>)]) -> Vec<(String, Option<u32>)> {
    let mut out: Vec<(String, Option<u32>)> = rows
        .iter()
        .map(|(id, score)| (id.clone(), score.map(f32::to_bits)))
        .collect();
    out.sort();
    out
}

/// Check one incremental view against a recompute of its plan.
///
/// Reads the graph version, takes the maintained rows, recomputes over a snapshot and
/// reads the version again. A version that moved answers `Skipped` and re-arms the
/// view's check. Otherwise a difference demotes the view with `SHADOW_MISMATCH`.
pub async fn check_view(state: &Arc<RwLock<ServerState>>, def: &PlanMatView) -> ShadowOutcome {
    let core = crate::dist_compute::resolve_core(state, &def.graph).await;
    let Some(core) = core else {
        return ShadowOutcome::Skipped;
    };
    let manager = matview::manager();
    let before = core.version();
    let Some(maintained) = manager.incremental_rows(&def.name) else {
        return ShadowOutcome::Skipped;
    };
    let (snapshot, snapshot_version) = core.analysis_snapshot_versioned();
    let semantic = core.semantic_store.read().clone();
    let plan_def = def.clone();
    let recomputed = tokio::task::spawn_blocking(move || {
        matview::materialize(&plan_def, &snapshot, &semantic)
    })
    .await;
    let recomputed = match recomputed {
        Ok(Ok(rows)) => rows,
        _ => {
            manager.rearm_check(&def.name);
            return ShadowOutcome::Skipped;
        }
    };
    if core.version() != before || snapshot_version != before {
        manager.rearm_check(&def.name);
        return ShadowOutcome::Skipped;
    }
    let (kept, fresh) = (comparable(&maintained), comparable(&recomputed));
    if kept == fresh {
        medulla_kernel::metrics::process().view_shadow_checked("plan", "match");
        return ShadowOutcome::Match;
    }
    let code = ServerErrorCode::ShadowMismatch.as_str();
    tracing::error!(
        target: "medulla::matview",
        view = %def.name,
        maintained = ?&kept[..kept.len().min(LOGGED_ROWS)],
        recomputed = ?&fresh[..fresh.len().min(LOGGED_ROWS)],
        "incremental matview disagrees with a recompute; falling back to recompute",
    );
    medulla_kernel::metrics::process().view_shadow_checked("plan", "mismatch");
    medulla_kernel::metrics::process().view_demoted("plan", code);
    manager.demote_view(&def.name, code);
    ShadowOutcome::Mismatch
}

/// Run the due checks of one tick: at most [`CHECKS_PER_TICK`] incremental views whose
/// shadow check is due, each under `deadline`. A check that overruns counts as
/// [`ShadowOutcome::Skipped`] and its view is re-armed, so it is checked again on a
/// later tick. No lock is held across a recompute. Answers each view's name and outcome.
pub async fn run_tick(
    state: &Arc<RwLock<ServerState>>,
    deadline: Duration,
) -> Vec<(String, ShadowOutcome)> {
    let mut outcomes = Vec::new();
    for def in matview::manager().take_due_checks(CHECKS_PER_TICK) {
        let outcome = match tokio::time::timeout(deadline, check_view(state, &def)).await {
            Ok(outcome) => outcome,
            Err(_elapsed) => {
                matview::manager().rearm_check(&def.name);
                tracing::warn!(
                    target: "medulla::matview",
                    view = %def.name,
                    "shadow check overran its deadline; it counts as skipped",
                );
                ShadowOutcome::Skipped
            }
        };
        outcomes.push((def.name.clone(), outcome));
    }
    outcomes
}

/// Spawn the task that runs [`run_tick`] every `view.shadow_tick_ms` milliseconds
/// (`MEDULLA_VIEW_SHADOW_TICK_MS`) for the life of the process. Called by the post-bind
/// matview maintenance (MEDULLA-INCREMENTAL-VIEWS-R008.3). Must run inside a tokio runtime.
pub fn spawn_shadow_checks(state: Arc<RwLock<ServerState>>) -> tokio::task::JoinHandle<()> {
    let period = Duration::from_millis(
        medulla_server_api::knob_config::knobs()
            .view_shadow_tick_ms
            .max(1),
    );
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(period);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            run_tick(&state, CHECK_DEADLINE).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_persistence::redb_backend::RedbBackend;
    use medulla_protocol_types::protocol::{GraphType, Method};
    use medulla_query_types::wire::{Op, Plan};
    use medulla_server_auth::auth::{dispatch_test_on_heap, scoped_test_request_on, ScopedTestCaller};
    use medulla_server_cdc::matview::Mode;

    const SECRET: &str = "matview-shadow-test-secret";
    const ISSUER: &str = "matview-shadow-test-issuer";
    const GRAPH: &str = "matview-shadow-test";

    async fn call(
        state: &Arc<RwLock<ServerState>>,
        id: u64,
        method: Method,
    ) -> medulla_protocol_types::protocol::Response {
        let request = scoped_test_request_on(
            SECRET,
            id,
            GRAPH,
            method,
            ScopedTestCaller {
                principal: ISSUER,
                tenant: "tenant-shadow",
                scopes: &["*"],
                nonce: &format!("matview-shadow-nonce-{id}"),
                idempotency_key: &format!("matview-shadow-key-{id}"),
            },
        );
        dispatch_test_on_heap(state, request).await
    }

    fn view(name: &str, label: &str) -> (PlanMatView, medulla_exec::incremental::Circuit) {
        let plan = Plan::new(vec![Op::Scan {
            label: label.into(),
        }]);
        let circuit = medulla_exec::incremental::Circuit::compile(&plan).unwrap();
        (
            PlanMatView {
                name: name.into(),
                graph: GRAPH.into(),
                plan,
            },
            circuit,
        )
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R008.2
    #[tokio::test(flavor = "multi_thread")]
    async fn mismatch_demotes_and_match_keeps_mode() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = medulla_server::seams_install::new_state_for_test(
            SECRET,
            ServerState::test_isolation(ISSUER),
        );
        server.persist_dir = Some(dir.path().to_string_lossy().into_owned());
        let backend =
            RedbBackend::open(dir.path().join("graphs").to_string_lossy().into_owned(), 64)
                .unwrap();
        server.persistence = Some(Arc::new(backend));
        let state = Arc::new(RwLock::new(server));
        let created = call(
            &state,
            1,
            Method::CreateGraph {
                graph_name: GRAPH.into(),
                graph_type: GraphType::Global,
            },
        )
        .await;
        assert!(created.error.is_none(), "{:?}", created.error);
        let added = call(
            &state,
            2,
            Method::AddNode {
                node_id: "shadow-doc".into(),
                properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
                    "type": "ShadowDoc"
                }))
                .unwrap(),
            },
        )
        .await;
        assert!(added.error.is_none(), "{:?}", added.error);

        let manager = matview::manager();
        // Installed after the write: the circuit never saw the node, so it is wrong.
        let (stale_def, stale_circuit) = view("shadow-stale-view", "ShadowDoc");
        manager.install_incremental(stale_def.clone(), stale_circuit, 0);
        // A view over a label no node carries is empty on both sides.
        let (empty_def, empty_circuit) = view("shadow-empty-view", "ShadowNothing");
        manager.install_incremental(empty_def.clone(), empty_circuit, 0);

        assert_eq!(check_view(&state, &empty_def).await, ShadowOutcome::Match);
        assert_eq!(manager.mode("shadow-empty-view"), Some(Mode::Incremental));

        assert_eq!(check_view(&state, &stale_def).await, ShadowOutcome::Mismatch);
        assert_eq!(manager.mode("shadow-stale-view"), Some(Mode::Recompute));
        assert_eq!(
            manager.fallback_reason("shadow-stale-view").as_deref(),
            Some(ServerErrorCode::ShadowMismatch.as_str())
        );
        // A demoted view is no longer incremental, so a second check compares nothing.
        assert_eq!(check_view(&state, &stale_def).await, ShadowOutcome::Skipped);
    }

    // proves R008.3: Wiring: the check task
    // spec: MEDULLA-INCREMENTAL-VIEWS-R008.3
    #[tokio::test(flavor = "multi_thread")]
    async fn tick_runs_due_checks_with_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = medulla_server::seams_install::new_state_for_test(
            SECRET,
            ServerState::test_isolation(ISSUER),
        );
        server.persist_dir = Some(dir.path().to_string_lossy().into_owned());
        let backend =
            RedbBackend::open(dir.path().join("graphs").to_string_lossy().into_owned(), 64)
                .unwrap();
        server.persistence = Some(Arc::new(backend));
        let state = Arc::new(RwLock::new(server));
        let created = call(
            &state,
            1,
            Method::CreateGraph {
                graph_name: GRAPH.into(),
                graph_type: GraphType::Global,
            },
        )
        .await;
        assert!(created.error.is_none(), "{:?}", created.error);

        let manager = matview::manager();
        // A view over a label no node carries is empty on both sides: a check matches.
        let (def, circuit) = view("tick-empty-view", "TickNothing");
        manager.install_incremental(def, circuit, 0);

        // Nothing is due, so the tick checks nothing.
        assert!(run_tick(&state, CHECK_DEADLINE).await.is_empty());

        // A check that overruns its deadline counts as skipped and is re-armed.
        manager.rearm_check("tick-empty-view");
        let outcomes = run_tick(&state, Duration::ZERO).await;
        assert_eq!(
            outcomes,
            vec![("tick-empty-view".to_string(), ShadowOutcome::Skipped)]
        );
        assert_eq!(manager.mode("tick-empty-view"), Some(Mode::Incremental));

        // The overrun view is due again, and a check within its deadline compares.
        let outcomes = run_tick(&state, CHECK_DEADLINE).await;
        assert_eq!(
            outcomes,
            vec![("tick-empty-view".to_string(), ShadowOutcome::Match)]
        );
        assert!(run_tick(&state, CHECK_DEADLINE).await.is_empty());

        // The tick period is the `view.shadow_tick_ms` knob.
        let knob = medulla_server_api::knob_config::KNOB_TABLE
            .iter()
            .find(|spec| spec.name == "view.shadow_tick_ms")
            .expect("the tick period is a registry knob");
        assert_eq!(knob.env_name(), TICK_ENV);
    }
}
