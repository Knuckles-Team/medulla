use crate::dispatch::*;
#[cfg(feature = "raft")]
use medulla_server_consensus::dispatch::consensus::propose_native_mutation;
#[cfg(feature = "raft")]
use medulla_server_graph::dispatch::change_envelope::multi_graph_batch_update;
use medulla_server_graph::dispatch::request_boundary::preflight::preflight_request_msgpack;
use medulla_server_graph::dispatch::shared::timed_read;
use medulla_server_graph::dispatch::shared::validate_graph_create_type;
#[cfg(all(feature = "sparql-http", feature = "raft"))]
use medulla_server_graph::dispatch::sparql_update::{
    coordinated_rdf_graph_write, coordinated_sparql_http_update,
};

pub mod authorization;
#[cfg(feature = "raft")]
pub mod consensus;
pub mod delegated_actor;
pub mod method_errors;
pub mod saga;

pub use authorization::{
    check_agent_run_delegation, check_scope_and_admin_authority, check_submit_work_item_context,
    DispatchAuthority,
};

impl DispatchAuthority {
    pub const fn state_machine_authorized(self) -> bool {
        self.state_machine_authorized
    }

    pub const fn identity_bootstrap(self) -> bool {
        self.identity_bootstrap
    }
}
#[cfg(feature = "raft")]
pub use consensus::{check_cluster_placement_before_consensus, route_consensus_before_gateway};
pub use saga::{begin_session_control_saga, finalize_dispatch_response};

pub fn append_native_resource_ops(ops: &mut Vec<&'static str>, available: bool) {
    if available {
        ops.extend([
            "ReserveWorkItemResources",
            "ReleaseWorkItemResources",
            "ReclaimWorkItemResources",
            "QueryWorkItemReservation",
            "ResourceReservationStatus",
            "UpdateResourceHost",
        ]);
    }
}

pub fn append_native_capacity_ops(ops: &mut Vec<&'static str>, available: bool) {
    if available {
        ops.extend([
            "AcquireCapacity",
            "RenewCapacity",
            "ReleaseCapacity",
            "StartCapacity",
            "RevokeCapacity",
            "ReclaimExpiredCapacity",
            "ReconcileCapacity",
            "CapacityStatus",
            "UpdateCapacityCell",
            "ThrottleCapacityCell",
        ]);
    }
}

pub fn append_native_work_item_ops(ops: &mut Vec<&'static str>, available: bool) {
    if available {
        ops.extend([
            "KgDelegate",
            "SubmitWorkItem",
            "SubmitWorkItems",
            // EH-219: the typed caller-view reads over the same native rows.
            "PublishCurrentPlan",
            "GetCurrentPlan",
            "GetRunCheckpoint",
            "PublishRunCheckpoint",
            "GetRunWorkItem",
            "GetSessionUsage",
            "GetShardWriterWidth",
            "GetWorkItem",
            "ListWorkItemTransitions",
            "ListWorkItems",
            "GetWorkItemOutcome",
            // MEDULLA-DURABLE-KERNEL-R820.3: the uncertain-effect protocol.
            "MarkWorkItemEffectStarted",
            "RecordEffectReceipt",
            "SubmitAdmittedRequest",
            "GetEffectReceipt",
            "GetAdmittedRequest",
            "TraceCritique",
            // graph-os MEDULLA-2: native control leases share the WorkItem kernel.
            "IssueHostStartGrant",
            "ApproveSourceQuery",
            "IssueControlLease",
            "TransitionControlLease",
            "GetControlLease",
            "ListControlLeases",
            // EH-348: the work market shares the WorkItem kernel.
            "GapUpsert",
            "GapTransition",
            "GapSettle",
            "WorkOfferPut",
            "GapGet",
            "GapList",
            // EH-558: sealed-record retirement rides the same native kernel.
            "RetireSealedRecord",
        ]);
    }
}

#[cfg(feature = "cost")]
pub async fn dispatch_resource_stats(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified_context: &VerifiedRequestContext,
    request: medulla_persistence::cost::ResourceStatsRequest,
) -> Response {
    // ResourceStats is service-scoped, so construct the same verified graph
    // read authority used by graph reads before scanning the registry.  The
    // cost collector filters tenant + ACL before it increments any aggregate,
    // cursor, or candidate state.
    let isolation = timed_read(state).await.isolation.clone();
    let authority = match GraphReadAuthority::from_verified(verified_context, &isolation) {
        Ok(authority) => authority,
        Err(error) => return Response::err(req_id, error),
    };
    match medulla_persistence::cost::collect_resource_stats_authorized(
        state,
        &authority,
        verified_context.tenant(),
        request,
    )
    .await
    {
        Ok(snapshot) => Response::ok(
            req_id,
            ResultPayload::of::<
                medulla_protocol_types::result_contract::coordination::ResourceStatsPage,
            >(snapshot),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

/// Explicitly erases a dispatch-arm future's concrete (often enormous,
/// datafusion/cypher/sql-plan-carrying) type to `dyn Future + Send` at the
/// match-arm boundary, once, here -- rather than letting `dispatch_inner`'s
/// own generated per-arm state machine hold N structurally distinct
/// concrete future types simultaneously, which is what overflowed rustc's
/// trait-resolution recursion limit (E0275) once this lane's extraction
/// gave the match 53 separate `async fn` calls instead of one inlined body.
/// `Box<dyn Future<Output = Response> + Send>` is trivially `Send` by
/// construction, so this bounds the Send-proof cost per arm to O(1) instead
/// of O(the whole call graph). Same fix as `server::transport.rs`'s spawn
/// site and the pre-existing `dispatch_on_heap` test helper (dispatch.rs /
/// registry_reaper.rs), applied at the dispatch_inner match itself.
pub fn dispatch_boxed<'a>(
    fut: impl std::future::Future<Output = Response> + Send + 'a,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send + 'a>> {
    Box::pin(fut)
}

pub async fn compute_identity_bootstrap(
    state: &Arc<RwLock<ServerState>>,
    req: &Request,
    verified_context: &VerifiedRequestContext,
    state_machine_authorized: bool,
) -> bool {
    !state_machine_authorized && {
        let state = timed_read(state).await;
        state.isolation.identity_bootstrap_pending()
            && req.graph == "__commons__"
            && matches!(
                &req.method,
                Method::RegisterIdentity {
                    agent_id,
                    role: medulla_identity_core_types::acl::AgentRole::System,
                    teams,
                    roles,
                    ..
                } if agent_id == verified_context.agent_id()
                    && teams.is_empty()
                    && roles.is_empty()
            )
            && verified_context.allows_identity_bootstrap()
    }
}

pub async fn preflight_commit_owner(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    req: &Request,
    verified_context: &VerifiedRequestContext,
) -> Result<(), Response> {
    let Method::Commit { txn_id, .. } = &req.method else {
        return Ok(());
    };
    medulla_h_txn::txn::preflight_open_txn_authority(state, req_id, verified_context, txn_id).await
}

/// Derive and clear identity secrets while preserving the request error id.
#[cfg(feature = "security")]
pub async fn stamp_identity_request(
    state: &Arc<RwLock<ServerState>>,
    req: &mut Request,
    verified_context: &VerifiedRequestContext,
    state_machine_authorized: bool,
) -> Result<(), Response> {
    medulla_server_identity::dispatch::identity_store::stamp_identity(
        state,
        &mut req.method,
        verified_context,
        medulla_server_identity::dispatch::elevation::ElevationStampAuthority::of(
            state_machine_authorized,
        ),
    )
    .await
    .map_err(|error| Response::err(req.id, error))
}

/// Register the sandbox principal as an ordinary agent when it has not been seen
/// (MEDULLA-IDENTITY-R003.9). A name that already belongs to an agent of another role (the
/// owner's `System` agent among them) is refused with the one fixed text, so a sandbox can
/// never act as an identity the engine holds for itself, and the refusal reveals nothing
/// about which names exist.
pub async fn admit_sandbox_agent(
    state: &Arc<RwLock<ServerState>>,
    verified_context: &VerifiedRequestContext,
) -> Result<(), String> {
    #[cfg(feature = "security")]
    {
        use {
            medulla_identity_core_types::acl::AgentIdentity,
            medulla_identity_core_types::acl::AgentRole,
        };
        let refused = || medulla_server_api::anonymous::SANDBOX_REFUSAL.to_string();
        let principal = verified_context.agent_id();
        let held = timed_read(state).await.isolation.get_identity(principal);
        match held {
            Some(identity) if matches!(identity.role, AgentRole::Agent) => Ok(()),
            Some(_) => Err(refused()),
            None => state
                .write()
                .await
                .isolation
                .try_register_agent_from_request(AgentIdentity {
                    agent_id: principal.to_string(),
                    role: AgentRole::Agent,
                    teams: Vec::new(),
                    roles: Vec::new(),
                })
                .map_err(|_| refused()),
        }
    }
    #[cfg(not(feature = "security"))]
    {
        let _ = (state, verified_context);
        Ok(())
    }
}

pub async fn dispatch_preamble_checks(
    state: &Arc<RwLock<ServerState>>,
    mut req: Request,
    verified_context: &VerifiedRequestContext,
    state_machine_authorized: bool,
) -> Result<(Request, DispatchAuthority), Response> {
    // EH-404: who acts on an elevation comes from the verified context, never
    // the body, and is fixed here -- before consensus routing -- so a replica
    // applies exactly the actor this boundary authorized.
    medulla_server_identity::dispatch::elevation::stamp_elevation_actor(
        &mut req.method,
        verified_context,
        medulla_server_identity::dispatch::elevation::ElevationStampAuthority::of(
            state_machine_authorized,
        ),
    );
    // EH-560: the governed-change actor, the same way.
    medulla_server_identity::dispatch::governed::stamp_governed_actor(
        &mut req.method,
        verified_context,
        medulla_server_identity::dispatch::elevation::ElevationStampAuthority::of(
            state_machine_authorized,
        ),
    );
    // MEDULLA-DURABLE-KERNEL-R095.1002: a bus write names the verified tenant
    // and principal, never the request fields; a replica applies the stamp.
    if !state_machine_authorized {
        authorization::stamp_bus_identity(&mut req.method, verified_context);
    }
    let method_policy = medulla_capabilities::policy(&req.method);
    let action = method_policy.authz_action;
    let identity_bootstrap =
        compute_identity_bootstrap(state, &req, verified_context, state_machine_authorized).await;
    let authority = DispatchAuthority {
        state_machine_authorized,
        identity_bootstrap,
    };
    // Resource reservation authority is deliberately narrower than the coarse
    // `kg:write` aggregate.  The resolved-profile assertion and host telemetry
    // are controller inputs; an ordinary graph writer must not be able to forge
    // a heavy reservation or overwrite shared physical-host accounting merely by
    // carrying a WorkItem-shaped request.  Replicated apply already carries a
    // verified native authority and bypasses the external gate here.
    check_scope_and_admin_authority(
        state,
        &req,
        verified_context,
        authority,
        action,
        method_policy.mutates,
    )
    .await?;

    // Engine-internal methods (e.g. EH-346's `PolicyEvolutionStore`) are
    // dispatched by the engine to itself; a caller may never name one. The
    // carrier half of the rule lives in `ChangeEnvelope::validate`.
    if let Some(refusal) = req
        .method
        .engine_internal_message()
        .filter(|_| !state_machine_authorized)
    {
        return Err(Response::err(req.id, refusal));
    }

    // After the ledger's scope check, before consensus routing --
    // stamp the actor, check the op's exact identity authority, derive every
    // hash/verdict/sealed value and clear every plaintext secret, so the
    // replicated command never carries one.
    #[cfg(feature = "security")]
    stamp_identity_request(state, &mut req, verified_context, state_machine_authorized).await?;

    // A client change-envelope draft becomes its governed envelope here, after
    // authorization and before material preflight, consensus and dispatch.
    let req =
        medulla_server_graph::dispatch::change_envelope_draft::compile_change_envelope_drafts(
            state,
            req,
            verified_context,
        )
        .await?;

    if let Err(error) = preflight_request_msgpack(&req.method) {
        return Err(Response::err(req.id, error));
    }

    // BUG-254 / NE-023: reject an unsupported lifecycle type at the first
    // authenticated, authoritative dispatch boundary.  This runs before
    // placement routing, consensus proposal, session-control saga creation,
    // persistence, or registry publication, so a bad CreateGraph request can
    // never enter the multi-minute retry path or leave partial state behind.
    // The transport decoder has a matching closed-enum guard for raw wire
    // callers; this check protects in-process and future-variant callers too.
    if let Method::CreateGraph { graph_type, .. } = &req.method {
        if let Err(error) = validate_graph_create_type(*graph_type) {
            return Err(Response::err(req.id, error));
        }
    }

    // A transaction id is a routing handle, never a bearer credential. Check an
    // open Commit owner before local or consensus routing; durable keyed replay
    // remains in the post-removal receipt boundary.
    preflight_commit_owner(state, req.id, &req, verified_context).await?;

    check_submit_work_item_context(&req, verified_context, authority)
        .map_err(|response| *response)?;
    check_agent_run_delegation(state, &req, verified_context, authority).await?;

    // Cluster writes cross consensus at the authenticated request boundary. The
    // complete mutation inventory is partitioned between graph commands and typed
    // native commands; a command constructor failure is returned before any local
    // saga or store mutation.
    #[cfg(feature = "raft")]
    check_cluster_placement_before_consensus(state, &req).await?;

    #[cfg(feature = "raft")]
    let req = route_consensus_before_gateway(
        state,
        req,
        verified_context,
        authority.identity_bootstrap(),
    )
    .await?;

    Ok((req, authority))
}

#[cfg(test)]
mod native_resource_capability_tests {
    use super::{append_native_resource_ops, append_native_work_item_ops};

    /// EH-219: `client.supports("GetWorkItem")` is how a caller learns the
    /// typed reads exist, so they ride the native WorkItem advertisement.
    #[test]
    fn typed_work_item_and_lease_ops_are_advertised_only_beside_native_submission() {
        for available in [false, true] {
            let mut ops = Vec::new();
            append_native_work_item_ops(&mut ops, available);
            for op in [
                "GetRunWorkItem",
                "GetSessionUsage",
                "GetShardWriterWidth",
                "GetWorkItem",
                "ListWorkItemTransitions",
                "ListWorkItems",
                "GetWorkItemOutcome",
                "IssueHostStartGrant",
                "ApproveSourceQuery",
                "IssueControlLease",
                "TransitionControlLease",
                "GetControlLease",
                "ListControlLeases",
                "GapUpsert",
                "GapGet",
                "GapList",
                "RetireSealedRecord",
            ] {
                assert_eq!(ops.contains(&op), available, "{op} when {available}");
            }
        }
    }

    #[test]
    fn native_resource_ops_are_advertised_only_when_backend_declares_support() {
        let mut dark = Vec::new();
        append_native_resource_ops(&mut dark, false);
        assert!(dark.is_empty());

        let mut served = Vec::new();
        append_native_resource_ops(&mut served, true);
        assert_eq!(
            served,
            vec![
                "ReserveWorkItemResources",
                "ReleaseWorkItemResources",
                "ReclaimWorkItemResources",
                "QueryWorkItemReservation",
                "ResourceReservationStatus",
                "UpdateResourceHost",
            ]
        );
    }
}
