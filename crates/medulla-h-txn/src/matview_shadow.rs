//! Shadow verification of incremental plan views (MEDULLA-INCREMENTAL-VIEWS-R008.2/.3).
//!
//! A due view is recomputed from the graph and compared with its maintained state. A
//! divergence demotes the view to recompute mode with `SHADOW_MISMATCH`.

use std::sync::Arc;
use std::time::Duration;

use medulla_graph_core::graph::GraphCore;
use medulla_server_api::knob_config::knobs;
use medulla_server_api::state::ServerState;
use medulla_server_cdc::matview::{self, PlanMatView};
use medulla_types::contract::ServerErrorCode;
use tokio::sync::RwLock;

use crate::dist_compute::resolve_core;

/// The most views one tick checks.
const DUE_PER_TICK: usize = 4;
/// The longest one check may run before it counts as skipped.
const CHECK_DEADLINE: Duration = Duration::from_secs(30);
/// The most rows of each side one mismatch log line carries.
const LOGGED_ROWS: usize = 5;

/// The result of one shadow check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadowOutcome {
    /// The maintained state equals the recompute.
    Match,
    /// The two diverged; the view was demoted.
    Mismatch,
    /// The graph moved during the check or the view left incremental mode.
    Skipped,
}

type Rows = Vec<(String, Option<f32>)>;

/// Sort rows as `(id, score)` pairs with a total order on the score.
fn sorted(mut rows: Rows) -> Rows {
    rows.sort_by(|a, b| {
        let key = |r: &(String, Option<f32>)| (r.0.clone(), r.1.map(f32::to_bits));
        key(a).cmp(&key(b))
    });
    rows
}

/// Compare the maintained rows of `def` with a recompute at one graph version.
pub fn check_view(core: &GraphCore, def: &PlanMatView) -> ShadowOutcome {
    let manager = matview::manager();
    let before = core.version();
    let maintained = manager.incremental_rows(&def.name);
    let (snapshot, _) = core.analysis_snapshot_versioned();
    let semantic = core.semantic_store.read().clone();
    let recomputed = matview::materialize(def, &snapshot, &semantic);
    let (Some(maintained), Ok(recomputed)) = (maintained, recomputed) else {
        return skipped(&def.name);
    };
    if core.version() != before {
        return skipped(&def.name);
    }
    let (maintained, recomputed) = (sorted(maintained), sorted(recomputed));
    if maintained == recomputed {
        medulla_kernel::metrics::view_shadow_checked("plan", "match");
        return ShadowOutcome::Match;
    }
    let code = ServerErrorCode::ShadowMismatch.as_str();
    tracing::error!(
        target: "medulla::matview",
        view = %def.name,
        maintained = ?&maintained[..maintained.len().min(LOGGED_ROWS)],
        recomputed = ?&recomputed[..recomputed.len().min(LOGGED_ROWS)],
        "incremental view diverged from its recompute; demoting to recompute",
    );
    manager.demote_incremental(&def.name, code);
    medulla_kernel::metrics::view_shadow_checked("plan", "mismatch");
    medulla_kernel::metrics::view_demoted("plan", code);
    ShadowOutcome::Mismatch
}

/// A skipped check re-arms so the next tick retries it.
fn skipped(name: &str) -> ShadowOutcome {
    matview::manager().rearm_check(name);
    ShadowOutcome::Skipped
}

/// One check of a view: its definition and the core of its graph, when the graph exists.
type CheckItem = (PlanMatView, Option<Arc<GraphCore>>);
/// The check body; tests substitute a slow one.
type CheckFn = fn(&GraphCore, &PlanMatView) -> ShadowOutcome;

/// Run every item on the blocking pool, each under `deadline`; a timeout is `Skipped`.
async fn run_checks(
    items: Vec<CheckItem>,
    deadline: Duration,
    check: CheckFn,
) -> Vec<ShadowOutcome> {
    let mut outcomes = Vec::with_capacity(items.len());
    for (def, core) in items {
        let Some(core) = core else {
            outcomes.push(skipped(&def.name));
            continue;
        };
        let name = def.name.clone();
        let job = tokio::task::spawn_blocking(move || check(&core, &def));
        outcomes.push(match tokio::time::timeout(deadline, job).await {
            Ok(Ok(outcome)) => outcome,
            _ => skipped(&name),
        });
    }
    outcomes
}

/// Take the due views and check each one.
async fn tick(state: &Arc<RwLock<ServerState>>, deadline: Duration, check: CheckFn) {
    let mut items = Vec::new();
    for def in matview::manager().take_due_checks(DUE_PER_TICK) {
        let core = resolve_core(state, &def.graph).await;
        items.push((def, core));
    }
    run_checks(items, deadline, check).await;
}

/// Start the shadow-check task at `view.shadow_tick_ms`. It holds no lock across a recompute.
pub fn spawn_shadow_checks(state: Arc<RwLock<ServerState>>) {
    let period = Duration::from_millis(knobs().view_shadow_tick_ms);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(period);
        loop {
            interval.tick().await;
            tick(&state, CHECK_DEADLINE, check_view).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::wire::{Op, Plan};

    fn props(ty: &str) -> Vec<u8> {
        rmp_serde::to_vec_named(&serde_json::json!({ "type": ty })).unwrap()
    }

    /// An incremental `Scan{Doc}` view named `name` over a core holding `docs` documents.
    fn fixture(name: &str, docs: usize) -> (Arc<GraphCore>, PlanMatView) {
        let core = Arc::new(GraphCore::new());
        for i in 0..docs {
            core.add_node(format!("d{i}"), props("Doc"));
        }
        let plan = Plan::new(vec![Op::Scan {
            label: "Doc".into(),
        }]);
        let mut circuit = medulla_exec::incremental::Circuit::compile(&plan).unwrap();
        circuit
            .apply(&matview::incremental::seed_delta_from_core(&core))
            .unwrap();
        let def = PlanMatView {
            name: name.into(),
            graph: "g".into(),
            plan,
        };
        matview::manager().install_incremental(def.clone(), circuit, 0);
        (core, def)
    }

    fn slow(_: &GraphCore, _: &PlanMatView) -> ShadowOutcome {
        std::thread::sleep(Duration::from_millis(300));
        ShadowOutcome::Match
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R008.2
    #[test]
    fn mismatch_demotes_and_match_keeps_mode() {
        let (core, def) = fixture("shadow-compare", 3);
        assert_eq!(check_view(&core, &def), ShadowOutcome::Match);
        assert_eq!(
            matview::manager().mode(&def.name),
            Some(matview::Mode::Incremental)
        );

        core.add_node("late".into(), props("Doc"));
        assert_eq!(check_view(&core, &def), ShadowOutcome::Mismatch);
        assert_eq!(
            matview::manager().mode(&def.name),
            Some(matview::Mode::Recompute)
        );
        assert_eq!(
            matview::manager().fallback_reason(&def.name).as_deref(),
            Some("SHADOW_MISMATCH")
        );
        assert_eq!(check_view(&core, &def), ShadowOutcome::Skipped);
    }

    // spec: MEDULLA-INCREMENTAL-VIEWS-R008.3
    #[tokio::test]
    async fn tick_runs_due_checks_with_deadline() {
        let (core, def) = fixture("shadow-tick", 2);
        let items = vec![(def.clone(), Some(core.clone()))];
        let fast = run_checks(items.clone(), Duration::from_secs(5), check_view).await;
        assert_eq!(fast, [ShadowOutcome::Match]);

        let late = run_checks(items, Duration::from_millis(20), slow).await;
        assert_eq!(late, [ShadowOutcome::Skipped]);
        let rearmed = matview::manager().take_due_checks(DUE_PER_TICK);
        assert!(
            rearmed.iter().any(|d| d.name == def.name),
            "a timeout re-arms"
        );

        let missing = run_checks(vec![(def, None)], Duration::from_secs(1), check_view).await;
        assert_eq!(missing, [ShadowOutcome::Skipped]);
    }
}
