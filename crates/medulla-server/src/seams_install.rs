//! Installs every crate-split seam implementation into the state (MEDULLA-CONTRACT-R056.8).
//!
//! Owned by `medulla-server`, the only crate that sees every handler crate. Each seam lane fills
//! its own slot below; keep one blank line between slots.

use medulla_server_api::state::ServerState;

/// Install every seam on a state that is already shared (MEDULLA-ORCH-VOCAB-R013.7.5): the
/// state-free seams, then the walkthrough readers, which hold a clone of `shared` and take its
/// read lock for one read at a time.
pub fn install_seams(
    state: &ServerState,
    shared: &std::sync::Arc<tokio::sync::RwLock<ServerState>>,
) {
    install_state_seams(state);
    install_freshness_search_port(state);
    install_walkthrough_seam(state, shared);
    install_downtime_bodies(shared);
}

/// Register the production search source port of the freshness refresh once, over the
/// state's foreign source catalog (MEDULLA-DECISION-ENGINE-R174.9.3.1).
#[allow(unused_variables)]
pub fn install_freshness_search_port(state: &ServerState) {
    #[cfg(all(feature = "decide", feature = "federation"))]
    {
        use medulla_server_ops::api::query_txn::ForeignStateExt;
        medulla_h_decide::decide::stat_freshness::register_search_source_port(std::sync::Arc::new(
            crate::freshness_search_port::FederationSearchPort::new(state.foreign_sources()),
        ));
    }
}

/// Install the body of every shipped downtime intent that has one
/// (MEDULLA-DURABLE-KERNEL-R101.7). `plan_evolution` reads the shared state, so it is installed
/// here and only when called inside a runtime; `live_artifact_refresh` has no body yet.
#[allow(unused_variables)]
pub fn install_downtime_bodies(shared: &std::sync::Arc<tokio::sync::RwLock<ServerState>>) {
    #[cfg(all(feature = "test-support", feature = "jobs"))]
    medulla_server_api::downtime::install_downtime_body(
        crate::downtime_test_steps::INTENT,
        std::sync::Arc::new(crate::downtime_test_steps::TestStepsBody),
    );
    #[cfg(all(feature = "jobs", feature = "sparql", feature = "federation"))]
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        medulla_server_api::downtime::install_downtime_body(
            "planner_evolution",
            std::sync::Arc::new(crate::downtime_plan_evolution::PlannerEvolutionBody::new(
                std::sync::Arc::clone(shared),
                runtime.clone(),
            )),
        );
        // The planner's champion is durable: rebuild it from the pointer at startup.
        let restored = std::sync::Arc::clone(shared);
        runtime.spawn(async move {
            if let Err(error) = crate::planner_promotion::restore_champion(
                &restored,
                medulla_exec::learned_cost::PlanRecordStore::global(),
            )
            .await
            {
                tracing::warn!(%error, "planner champion not restored; the built-in cost head stays in force");
            }
        });
        medulla_server_api::downtime::install_downtime_body(
            "plan_evolution",
            std::sync::Arc::new(crate::downtime_plan_evolution::PlanEvolutionBody::new(
                std::sync::Arc::clone(shared),
                runtime,
            )),
        );
    }
}

/// Install the walkthrough readers once, in the `L6` slot.
pub fn install_walkthrough_seam(
    state: &ServerState,
    shared: &std::sync::Arc<tokio::sync::RwLock<ServerState>>,
) {
    use medulla_server_api::walkthrough_readers::{
        install_walkthrough_readers, WalkthroughReaders,
    };
    let _ = install_walkthrough_readers(
        &state.seams,
        WalkthroughReaders::new(
            std::sync::Arc::new(crate::walkthrough_readers::GraphIndexReader::new(
                std::sync::Arc::clone(shared),
            )),
            std::sync::Arc::new(crate::walkthrough_readers::ServerBlobReader::new(
                std::sync::Arc::clone(shared),
            )),
        ),
    );
}

/// Install the seam implementations that need no shared state. Called while the startup
/// composes the state, before it is shared; `install_walkthrough_seam` follows once it is.
#[allow(unused_variables)]
pub fn install_state_seams(state: &ServerState) {
    // L1 server helpers: none (moved items need no installation).

    // The plan record store keeps `query.plan_records` records per tenant
    // (MEDULLA-FEDERATED-QUERY-R732.1); exec cannot read the server knobs, so it is bound here.
    medulla_exec::learned_cost::PlanRecordStore::global().set_per_tenant_cap(
        usize::try_from(medulla_server_api::knob_config::knobs().query_plan_records)
            .unwrap_or(usize::MAX),
    );
    medulla_exec::learned_cost::PlanRecordStore::global().set_shadow_fraction(
        medulla_server_api::knob_config::knobs().query_planner_shadow_fraction,
    );

    // L2 dispatch re-entry.
    let _ = state.seams.dispatch.set(std::sync::Arc::new(
        medulla_server_router::dispatch::seam::DispatchSeam,
    ));

    // L3 ingest, graph, jobs, compute.
    let _ = medulla_server_ops::api::ingest::install_ingest_seam(
        &state.seams,
        std::sync::Arc::new(medulla_h_ingest_repository::repository_index::IngestSeam),
    );
    let _ = medulla_server_ops::api::ingest::install_graph_seam(
        &state.seams,
        std::sync::Arc::new(GraphSeam),
    );
    #[cfg(feature = "jobs")]
    let _ = medulla_server_ops::api::ingest::install_jobs_seam(
        &state.seams,
        std::sync::Arc::new(medulla_h_jobs::jobs::JobsSeam),
    );

    // L4 admin, decide.
    let _ = medulla_server_ops::api::admin::install_admin_seam(
        &state.seams,
        std::sync::Arc::new(medulla_h_admin_schema::admin::AdminSeam),
    );
    let _ = medulla_server_ops::api::admin::install_decide_seam(
        &state.seams,
        std::sync::Arc::new(medulla_h_decide::decide::seam::DecideSeam),
    );

    // The branch-merge conflict decider (MEDULLA-DECISION-ENGINE-R828).
    #[cfg(feature = "decide")]
    let _ = medulla_server_api::api::conflict_decider::install_conflict_decider(
        &state.seams,
        std::sync::Arc::new(medulla_h_decide::decide::stat_decide::ConflictDeciderSeam),
    );

    // Drift monitors: the Discover signal source and the write observer.
    #[cfg(feature = "decide")]
    let _ = medulla_h_decide_base::decide::drift_registry::install();

    // L5 query, txn, wire.
    #[cfg(all(feature = "query", feature = "nl-query"))]
    medulla_h_query_read::query::expansion::install_expansion_provider();
    #[cfg(all(feature = "mining", feature = "server"))]
    medulla_kernel::kernel::persistence::install_mining_replay(medulla_h_mining::mining::replay);

    #[cfg(any(
        feature = "query",
        feature = "cypher",
        feature = "graphql",
        feature = "mining"
    ))]
    let _ = medulla_server_ops::api::query_txn::install_query_seam(
        &state.seams,
        std::sync::Arc::new(crate::query_seam::QuerySeam),
    );
    let _ = medulla_server_ops::api::query_txn::install_txn_seam(
        &state.seams,
        std::sync::Arc::new(medulla_h_txn::txn::TxnSeam),
    );
    #[cfg(feature = "query")]
    let _ = medulla_server_ops::api::query_txn::install_wire_seam(
        &state.seams,
        std::sync::Arc::new(medulla_wire_core::wire::WireSeam),
    );

    // CREATE TRIGGER names a stored statechart; the table store reads it through this source
    // (MEDULLA-ENGINE-PARITY-R012.2).
    #[cfg(all(feature = "query", feature = "statechart"))]
    medulla_table_store::tables::install_trigger_machine_source(std::sync::Arc::new(
        StatechartMachines,
    ));

    // L6 persistence, raft, server-api.
}

/// Reads trigger machines from the engine's statechart store.
#[cfg(all(feature = "query", feature = "statechart"))]
struct StatechartMachines;

#[cfg(all(feature = "query", feature = "statechart"))]
impl medulla_table_store::tables::TriggerMachineSource for StatechartMachines {
    fn machine(
        &self,
        persist_dir: &std::path::Path,
        machine_id: &str,
    ) -> Result<medulla_statechart::StatechartDef, String> {
        let persist_dir = persist_dir
            .to_str()
            .ok_or_else(|| "the persistence directory is not valid UTF-8".to_string())?;
        medulla_h_jobs::statechart::statechart_store(Some(persist_dir))?
            .get_def(machine_id)
            .map_err(|error| error.to_string())
    }
}

/// Graph handler behaviour installed on the seam registry's `graph` slot.
struct GraphSeam;

#[async_trait::async_trait]
impl medulla_server_ops::api::ingest::GraphApi for GraphSeam {
    async fn graph_ops_try_handle(
        &self,
        state: &std::sync::Arc<tokio::sync::RwLock<ServerState>>,
        req_id: u64,
        caller: Option<&str>,
        graph_name: &str,
        read_authority: &medulla_server_api::carrier::GraphReadAuthority,
        core: std::sync::Arc<medulla_graph_core::graph::GraphCore>,
        method: medulla_protocol_types::protocol::Method,
    ) -> medulla_protocol_types::protocol::Response {
        medulla_h_graph::graph_ops::terminal::try_handle(
            state,
            req_id,
            caller,
            graph_name,
            read_authority,
            core,
            method,
        )
        .await
    }
}

/// A test state with every seam installed, the way server startup composes it. Test builders use this
/// so each state that reaches the dispatch re-entry seam carries the `DispatchSeam`.
#[cfg(any(test, feature = "test-support"))]
pub fn new_state_for_test(
    auth_secret: impl Into<String>,
    isolation: medulla_isolation::isolation::IsolationLayer,
) -> ServerState {
    let state = ServerState::new_for_test(auth_secret, isolation);
    install_state_seams(&state);
    state
}

/// Install every seam on a shared state built by a harness below this crate.
#[cfg(any(test, feature = "test-support"))]
pub async fn install_seams_on(state: &std::sync::Arc<tokio::sync::RwLock<ServerState>>) {
    install_seams(&*state.read().await, state);
}
