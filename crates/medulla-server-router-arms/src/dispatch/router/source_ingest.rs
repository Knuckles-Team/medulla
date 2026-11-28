use super::*;

#[cfg(feature = "jobs")]
use medulla_ingestion_types::ingest_drain::DrainState;
#[cfg(feature = "jobs")]
use medulla_protocol_types::result_contract::coordination as coordination_results;
use medulla_protocol_types::result_contract::ingestion as ingestion_results;

/// Source ingestion: parse a file or a batch of files, index a repository,
/// observe a screen. All are stateless parses that never touch `state`.
///
/// Hands a method it does not own back as `ControlFlow::Continue`.
pub async fn dispatch_source_ingest_methods(
    ctx: DispatchCtx<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    ControlFlow::Break(match method {
        Method::ParseFile { file_path, source } => {
            dispatch_boxed(parse_file(ctx, file_path, source)).await
        }

        Method::ParseFiles { files_msgpack } => {
            dispatch_boxed(parse_files(ctx, files_msgpack)).await
        }

        // A scoped (branch-aware, durable) batch is a graph write: it continues to
        // the graph-scoped route, after graph ACL and placement (EH-280).
        Method::IndexRepository {
            files_msgpack,
            scope: None,
        } => dispatch_boxed(index_repository(ctx, files_msgpack)).await,

        Method::ObserveScreen { obs_msgpack } => {
            dispatch_boxed(observe_screen(ctx, obs_msgpack)).await
        }
        // MEDULLA-INGEST-WORKER-R001.4.1: the served ingest enqueue.
        #[cfg(feature = "jobs")]
        Method::IngestEnqueue { envelope } => {
            dispatch_boxed(handle_ingest_enqueue(ctx, *envelope)).await
        }
        // MEDULLA-INGEST-WORKER-R001.11: the served backpressure read.
        #[cfg(feature = "jobs")]
        Method::IngestQueueDepth { queue } => dispatch_boxed(ingest_queue_depth(ctx, queue)).await,
        // MEDULLA-INGEST-WORKER-R605.2, R605.5: the chunked drain start and its status.
        #[cfg(feature = "jobs")]
        Method::StartSourceDrain {
            connector,
            stream,
            mode,
        } => dispatch_boxed(start_source_drain(ctx, connector, stream, mode)).await,
        #[cfg(feature = "jobs")]
        Method::SourceDrainStatus { drain_id } => {
            dispatch_boxed(source_drain_status(ctx, drain_id)).await
        }
        #[cfg(not(feature = "jobs"))]
        Method::StartSourceDrain { .. } | Method::SourceDrainStatus { .. } => Response::err(
            ctx.req.id,
            "UNAVAILABLE: source drains require the `jobs` feature".to_string(),
        ),
        #[cfg(not(feature = "jobs"))]
        Method::IngestQueueDepth { .. } => Response::err(
            ctx.req.id,
            "UNAVAILABLE: ingest queue depth requires the `jobs` feature".to_string(),
        ),
        #[cfg(not(feature = "jobs"))]
        Method::IngestEnqueue { .. } => Response::err(
            ctx.req.id,
            "UNAVAILABLE: ingest enqueue requires the `jobs` feature".to_string(),
        ),
        other => return ControlFlow::Continue(other),
    })
}

/// Serve `IngestQueueDepth` (MEDULLA-INGEST-WORKER-R001.11): the queued and claimed task count
/// and the oldest age of the verified tenant's reserved ingest queues. The load is the one
/// `task_queue_load` read the enqueue refusal uses. Writes nothing.
#[cfg(feature = "jobs")]
async fn ingest_queue_depth(ctx: DispatchCtx<'_>, queue: Option<String>) -> Response {
    use medulla_source_types::ingest_job::{ingest_lane_of_queue, IngestLane};
    let req_id = ctx.req.id;
    let queues: Vec<&'static str> = match queue.as_deref() {
        None => vec![
            IngestLane::General.queue_name(),
            IngestLane::Hydration.queue_name(),
        ],
        Some(name) => match ingest_lane_of_queue(name) {
            Some(lane) => vec![lane.queue_name()],
            None => {
                return Response::err(
                    req_id,
                    format!(
                        "{}: queue '{name}' is not an ingest queue",
                        medulla_types::contract::EngineErrorCode::InvalidArgument.as_str()
                    ),
                )
            }
        },
    };
    let persist_dir = ctx.state.read().await.persist_dir.clone();
    let jobs = medulla_server_ops::api::ingest::jobs_seam(ctx.state).await;
    let tenant = ctx.verified_context.tenant();
    let mut loads = Vec::with_capacity(queues.len());
    for name in queues {
        let load = match jobs.as_ref() {
            Some(jobs) => {
                let (queued, running, oldest) =
                    match jobs.task_queue_load(&persist_dir, tenant, name) {
                        Ok(load) => load,
                        Err(error) => return Response::err(req_id, error),
                    };
                match jobs.task_queue_dead_letters(&persist_dir, tenant, name) {
                    Ok(dead) => (queued, running, oldest, dead),
                    Err(error) => return Response::err(req_id, error),
                }
            }
            None => (0, 0, None, 0),
        };
        loads.push((name, load));
    }
    let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms() as i64;
    let report = medulla_h_ingest::ingest_queue::queue_depth_report(&loads, now_ms);
    Response::ok(
        req_id,
        ResultPayload::of::<coordination_results::IngestQueueDepth>(report),
    )
}

/// Serve `IngestEnqueue`: read the policy and the queue load, then map the job
/// onto its reserved task queue through the one producer.
#[cfg(feature = "jobs")]
async fn handle_ingest_enqueue(
    ctx: DispatchCtx<'_>,
    envelope: medulla_source_types::ingest_job::IngestJobEnvelope,
) -> Response {
    let req_id = ctx.req.id;
    let (persist_dir, config_store) = {
        let guard = ctx.state.read().await;
        let store = match medulla_persistence::scheduler_config_store::shared_scheduler_config_store(
            &guard,
        ) {
            Ok(store) => store,
            Err(error) => return Response::err(req_id, format!("INTERNAL: {error}")),
        };
        (guard.persist_dir.clone(), store)
    };
    let config = match medulla_h_jobs::scheduler_config::effective_config(&config_store) {
        Ok(config) => config,
        Err(error) => return Response::err(req_id, error),
    };
    let jobs = medulla_server_ops::api::ingest::jobs_seam(ctx.state).await;
    let tenant = ctx.verified_context.tenant();
    let queue = envelope.lane().queue_name();
    let depth = match jobs.as_ref() {
        Some(jobs) => match jobs.task_queue_load(&persist_dir, tenant, queue) {
            Ok((queued, running, _)) => queued.saturating_add(running),
            Err(error) => return Response::err(req_id, error),
        },
        None => 0,
    };
    let caller = medulla_h_ingest::ingest_queue::IngestCaller {
        persist_dir,
        tenant: tenant.to_string(),
        principal: ctx.verified_context.principal().to_string(),
    };
    let policy =
        medulla_source_types::ingest_job::IngestRetryPolicy::from_limits(&config.ingest_retry);
    match medulla_h_ingest::ingest_queue::enqueue_ingest_job(
        jobs,
        &caller,
        &envelope,
        &policy,
        depth,
        config.max_queue_depth,
    ) {
        Ok(receipt) => Response::ok(
            req_id,
            ResultPayload::of::<coordination_results::IngestEnqueue>(receipt),
        ),
        Err(error) => Response::err(req_id, format!("{}: {}", error.code.as_str(), error.detail)),
    }
}

async fn parse_file(ctx: DispatchCtx<'_>, file_path: String, source: Vec<u8>) -> Response {
    let req_id = ctx.req.id;
    #[cfg(feature = "ast")]
    {
        let input_check = validate_ast_logical_path(&file_path).and_then(|()| {
            let limits = ast_input_limits();
            if source.len() > limits.max_source_bytes || source.len() > limits.max_total_bytes {
                Err("AST_INPUT_LIMIT: source content exceeds the configured limit".to_string())
            } else {
                Ok(())
            }
        });
        match input_check
            .and_then(|()| medulla_compute::parser::tree_sitter::parse_file(&file_path, &source))
        {
            Ok(result) => Response::ok(
                req_id,
                ResultPayload::of::<ingestion_results::ParseFile>(result),
            ),
            Err(error) => Response::err(req_id, error),
        }
    }
    #[cfg(not(feature = "ast"))]
    {
        let _ = (file_path, source);
        Response::err(req_id, "AST feature not enabled".to_string())
    }
}

async fn parse_files(ctx: DispatchCtx<'_>, files_msgpack: Vec<u8>) -> Response {
    let req_id = ctx.req.id;
    #[cfg(feature = "ast")]
    {
        let owned = match decode_ast_files(&files_msgpack, ast_input_limits()) {
            Ok(files) => files,
            Err(error) => return Response::err(req_id, error),
        };
        // Parse on the blocking pool, NOT the async reactor: parse_files is
        // CPU-bound (rayon tree-sitter over every file) and a large batch
        // would otherwise stall the runtime thread, blocking unrelated
        // requests until it finishes. (CONCEPT:MEDULLA-KG.compute.off-reactor-dispatch — work off-reactor, A4)
        let results = match compute_off_lock(req_id, move || {
            medulla_compute::parser::tree_sitter::parse_files(&owned)
        })
        .await
        {
            Ok(results) => results,
            Err(response) => return response,
        };
        Response::ok(
            req_id,
            ResultPayload::of::<ingestion_results::ParseFiles>(results),
        )
    }
    #[cfg(not(feature = "ast"))]
    {
        let _ = files_msgpack;
        Response::err(req_id, "AST feature not enabled".to_string())
    }
}

async fn index_repository(ctx: DispatchCtx<'_>, files_msgpack: Vec<u8>) -> Response {
    let req_id = ctx.req.id;
    #[cfg(feature = "ast")]
    {
        // Same canonical blob shape as ParseFiles, but parsed AND
        // cross-file-resolved into one IndexResult.
        let owned = match decode_ast_files(&files_msgpack, ast_input_limits()) {
            Ok(files) => files,
            Err(error) => return Response::err(req_id, error),
        };
        // Off-reactor like ParseFiles: parse (rayon) + resolution are
        // CPU-bound over the whole batch. (CONCEPT:MEDULLA-KG.compute.turn-each-project)
        let result = match compute_off_lock(req_id, move || {
            medulla_compute::parser::resolve::index_repository(&owned)
        })
        .await
        {
            Ok(result) => result,
            Err(response) => return response,
        };
        Response::ok(
            req_id,
            ResultPayload::of::<ingestion_results::IndexRepository>(result),
        )
    }
    #[cfg(not(feature = "ast"))]
    {
        let _ = files_msgpack;
        Response::err(req_id, "AST feature not enabled".to_string())
    }
}

async fn observe_screen(ctx: DispatchCtx<'_>, obs_msgpack: Vec<u8>) -> Response {
    let req_id = ctx.req.id;
    // MessagePack map → a captured desktop frame. png rides as a bin field;
    // elements are the AT-SPI accessibles. (CONCEPT:AU-KG.ontology.owl-screen-bridge)
    let input = match decode_screen_observation(&obs_msgpack) {
        Ok(input) => input,
        Err(error) => return Response::err(req_id, error),
    };
    // Inline: PNG hashing + node/edge build over the element set is
    // microsecond-cheap (no AST parse), so it doesn't need the blocking pool.
    let result = medulla_compute::screen::observe_screen(&input);
    Response::ok(
        req_id,
        ResultPayload::of::<ingestion_results::ObserveScreen>(result),
    )
}

/// The graph a routing record names, or `None` when there is none for the verified tenant. A
/// failed read is `None` too: the route then falls through to the next scope.
#[cfg(feature = "jobs")]
async fn routed_graph_of(
    state: &Arc<RwLock<ServerState>>,
    target: &medulla_server_graph::dispatch::graph_client::GraphTarget<'_>,
    scope: medulla_ingestion_types::ingest_drain::RouteScope,
    key: &str,
) -> Option<String> {
    let id = medulla_ingestion_types::ingest_drain::route_node_id(scope, key)?;
    let properties = medulla_server_graph::dispatch::graph_client::read_node(state, target, &id)
        .await
        .ok()
        .flatten()?;
    if properties
        .get("tenant_id")
        .and_then(serde_json::Value::as_str)
        != Some(target.verified.tenant())
    {
        return None;
    }
    properties
        .get("graph")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// Serve `StartSourceDrain` (MEDULLA-INGEST-WORKER-R605.2): route the drain's graph, store the
/// `source_drain:<id>` node in the control graph, enqueue page 0 and answer the handle.
#[cfg(feature = "jobs")]
async fn start_source_drain(
    ctx: DispatchCtx<'_>,
    connector: String,
    stream: String,
    mode: String,
) -> Response {
    use medulla_ingestion_types::ingest_drain::{route_ingest_graph, RouteInputs, RouteScope};
    use medulla_server_graph::dispatch::graph_client::{write_batch, GraphTarget};
    let req_id = ctx.req.id;
    let tenant = ctx.verified_context.tenant();
    let control = medulla_server_api::knob_config::default_ingest_graph();
    let target = GraphTarget {
        graph: control,
        req_id,
        caller: ctx.req.agent_id.as_deref(),
        verified: ctx.verified_context,
    };
    let source = routed_graph_of(ctx.state, &target, RouteScope::Source, &connector).await;
    let repository = routed_graph_of(ctx.state, &target, RouteScope::Repository, &stream).await;
    let tenant_graph = routed_graph_of(ctx.state, &target, RouteScope::Tenant, tenant).await;
    let route = match route_ingest_graph(&RouteInputs {
        source: source.as_deref(),
        repository: repository.as_deref(),
        tenant: tenant_graph.as_deref(),
        default: control,
    }) {
        Ok(route) => route,
        Err(error) => {
            return Response::err(
                req_id,
                format!(
                    "{}: {error}",
                    medulla_types::contract::EngineErrorCode::InvalidArgument.as_str()
                ),
            )
        }
    };
    let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms() as i64;
    let started = match medulla_h_ingest::ingest_drain::start_drain(
        tenant,
        &connector,
        &stream,
        &mode,
        route.graph(),
        now_ms,
    ) {
        Ok(started) => started,
        Err(error) => {
            return Response::err(
                req_id,
                format!(
                    "{}: {error}",
                    medulla_types::contract::EngineErrorCode::InvalidArgument.as_str()
                ),
            )
        }
    };
    let agent = ctx.verified_context.agent_id();
    let write = |record: &medulla_ingestion_types::ingest_drain::DrainRecord| {
        medulla_h_ingest::ingest_drain::drain_node_operation(record, agent)
    };
    let operation = match write(&started.record) {
        Ok(operation) => operation,
        Err(error) => return Response::err(req_id, format!("INTERNAL: {error}")),
    };
    if let Err(refusal) = write_batch(ctx.state, &target, &[operation]).await {
        return Response::err(req_id, refusal);
    }
    let enqueued = handle_ingest_enqueue(ctx, started.first_page.clone()).await;
    if let Some(refusal) = enqueued.refusal_text() {
        // Page 0 never entered the queue: the drain it names must not stay Running.
        if let Ok(operation) = write(&started.record.failed(now_ms)) {
            let _ = write_batch(ctx.state, &target, &[operation]).await;
        }
        return Response::err(req_id, refusal);
    }
    Response::ok(
        req_id,
        ResultPayload::of::<coordination_results::StartSourceDrain>(started.handle),
    )
}

/// The drain `drain_id` of the verified tenant, read from the control graph. An id that is not a
/// drain id, an absent node and another tenant's node all read as `None`.
#[cfg(feature = "jobs")]
async fn read_drain(
    ctx: DispatchCtx<'_>,
    drain_id: &str,
) -> Option<medulla_ingestion_types::ingest_drain::DrainRecord> {
    use medulla_server_graph::dispatch::graph_client::{read_node, GraphTarget};
    medulla_ingestion_types::ingest_drain::DrainHandle::new(drain_id).ok()?;
    let target = GraphTarget {
        graph: medulla_server_api::knob_config::default_ingest_graph(),
        req_id: ctx.req.id,
        caller: ctx.req.agent_id.as_deref(),
        verified: ctx.verified_context,
    };
    let properties = read_node(
        ctx.state,
        &target,
        &medulla_ingestion_types::ingest_drain::drain_node_id(drain_id),
    )
    .await
    .ok()
    .flatten()?;
    medulla_h_ingest::ingest_drain::drain_from_node(&properties, ctx.verified_context.tenant())
}

/// Store `record` as the drain node of the control graph.
#[cfg(feature = "jobs")]
async fn write_drain(
    ctx: DispatchCtx<'_>,
    record: &medulla_ingestion_types::ingest_drain::DrainRecord,
) -> Result<(), String> {
    use medulla_server_graph::dispatch::graph_client::{write_batch, GraphTarget};
    let target = GraphTarget {
        graph: medulla_server_api::knob_config::default_ingest_graph(),
        req_id: ctx.req.id,
        caller: ctx.req.agent_id.as_deref(),
        verified: ctx.verified_context,
    };
    let operation = medulla_h_ingest::ingest_drain::drain_node_operation(
        record,
        ctx.verified_context.agent_id(),
    )?;
    write_batch(ctx.state, &target, &[operation]).await
}

#[cfg(feature = "jobs")]
fn drain_unknown(req_id: u64) -> Response {
    Response::err(
        req_id,
        format!(
            "{}: no drain of this tenant has that id",
            medulla_types::contract::ServerErrorCode::DrainUnknown.as_str()
        ),
    )
}

/// Serve `SourceDrainStatus` (MEDULLA-INGEST-WORKER-R605.5).
#[cfg(feature = "jobs")]
async fn source_drain_status(ctx: DispatchCtx<'_>, drain_id: String) -> Response {
    match read_drain(ctx, &drain_id).await {
        Some(record) => Response::ok(
            ctx.req.id,
            ResultPayload::of::<coordination_results::SourceDrainStatus>(record.status()),
        ),
        None => drain_unknown(ctx.req.id),
    }
}

/// Serve a `TaskQueueAck` that carries a drain report (MEDULLA-INGEST-WORKER-R605.2, R605.4).
///
/// A page below the stored checkpoint is answered `DRAIN_CURSOR_STALE` and changes nothing.
/// Otherwise the drain node is rewritten first, then the task acks through the one task-queue
/// handler, then the next page is enqueued under its deterministic task id, so a repeat of any step
/// is harmless (see [`run_drain_ack_plan`]). A caller without the live lease writes nothing.
#[cfg(feature = "jobs")]
pub async fn ack_drain_page(ctx: DispatchCtx<'_>, method: Method) -> Response {
    use medulla_h_ingest::ingest_drain::{drain_ack_plan, on_ingest_drain_ack, DrainAckOutcome};
    use medulla_work_types::task_queue::TaskOutcome;
    let req_id = ctx.req.id;
    let Method::TaskQueueAck {
        task_id,
        worker_id,
        outcome,
        detail,
        drain,
    } = method
    else {
        return Response::err(req_id, "router dispatch helper routing mismatch");
    };
    let invalid = |detail: String| {
        Response::err(
            req_id,
            format!(
                "{}: {detail}",
                medulla_types::contract::EngineErrorCode::InvalidArgument.as_str()
            ),
        )
    };
    let worker = worker_id.clone();
    let plain = Method::TaskQueueAck {
        task_id: task_id.clone(),
        worker_id,
        outcome,
        detail,
        drain: None,
    };
    let tenant = ctx.verified_context.tenant();
    let payload = match medulla_h_jobs::task_queue::task_payload(ctx.state, tenant, &task_id).await
    {
        Ok(Some((payload, _))) => payload,
        // An unknown or foreign task acks as any other and is refused as not claimed.
        Ok(None) => return ack_plain(ctx, plain).await,
        Err(error) => return Response::err(req_id, error),
    };
    let page =
        match serde_json::from_str::<medulla_source_types::ingest_job::IngestJobEnvelope>(&payload)
        {
            Ok(envelope) => envelope.page,
            Err(_) => None,
        };
    let Some(page) = page else {
        return invalid("the task is not a drain page".to_string());
    };
    let Some(record) = read_drain(ctx, page.drain_id()).await else {
        return drain_unknown(req_id);
    };
    let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms() as i64;
    if outcome != TaskOutcome::Succeeded {
        let acked = ack_plain(ctx, plain).await;
        if acked.refusal_text().is_none() && record.state == DrainState::Running {
            let failed = matches!(
                medulla_h_jobs::task_queue::task_payload(ctx.state, tenant, &task_id).await,
                Ok(Some((_, true)))
            );
            if failed {
                if let Err(refusal) = write_drain(ctx, &record.failed(now_ms)).await {
                    return Response::err(req_id, refusal);
                }
            }
        }
        return acked;
    }
    let max_items = medulla_server_api::knob_config::knobs().ingest_drain_page_records;
    let held = match medulla_h_jobs::task_queue::task_lease_held(
        ctx.state,
        ctx.verified_context,
        &task_id,
        &worker,
    )
    .await
    {
        Ok(held) => held,
        Err(error) => return Response::err(req_id, error),
    };
    let decision = on_ingest_drain_ack(
        &record,
        &page,
        &drain.unwrap_or_default(),
        max_items,
        now_ms,
        held,
    );
    let outcome = match decision {
        Ok(outcome) => outcome,
        Err(error) => return invalid(error.to_string()),
    };
    if outcome == DrainAckOutcome::Stale {
        medulla_kernel::metrics::process().record_job(
            medulla_kernel::metrics::IngestJobOutcome::Stale,
            medulla_source_types::ingest_job::IngestLane::General.queue_name(),
        );
        return Response::err(
            req_id,
            format!(
                "{}: page {} is below the stored checkpoint of the drain",
                medulla_types::contract::ServerErrorCode::DrainCursorStale.as_str(),
                page.page_index()
            ),
        );
    }
    run_drain_ack_plan(ctx, plain, drain_ack_plan(&outcome, held)).await
}

/// Run the steps of a drain ack in order and stop at the first failure
/// (MEDULLA-INGEST-WORKER-R605.2). The answer is the task ack's. The order (node, task ack, next
/// page) is `drain_ack_plan`'s: a crash after the node write leaves the task leased, so the
/// worker's retry replays; a crash before the enqueue is recovered by the scheduler tick.
#[cfg(feature = "jobs")]
async fn run_drain_ack_plan(
    ctx: DispatchCtx<'_>,
    plain: Method,
    plan: Vec<medulla_h_ingest::ingest_drain::DrainAckStep>,
) -> Response {
    use medulla_h_ingest::ingest_drain::DrainAckStep;
    let req_id = ctx.req.id;
    let mut answer = None;
    for step in plan {
        match step {
            DrainAckStep::WriteNode(record) => {
                if let Err(refusal) = write_drain(ctx, &record).await {
                    return Response::err(req_id, refusal);
                }
            }
            DrainAckStep::AckTask => {
                let acked = ack_plain(ctx, plain.clone()).await;
                if acked.refusal_text().is_some() {
                    return acked;
                }
                answer = Some(acked);
            }
            DrainAckStep::EnqueuePage(next) => {
                let enqueued = handle_ingest_enqueue(ctx, next).await;
                if let Some(refusal) = enqueued.refusal_text() {
                    return Response::err(req_id, refusal);
                }
            }
        }
    }
    answer.unwrap_or_else(|| Response::err(req_id, "INTERNAL: a drain ack plan holds no task ack"))
}

/// The task-queue handler's answer to `method`.
#[cfg(feature = "jobs")]
async fn ack_plain(ctx: DispatchCtx<'_>, method: Method) -> Response {
    match medulla_h_jobs::task_queue::try_handle(
        ctx.state,
        ctx.req.id,
        ctx.verified_context,
        method,
    )
    .await
    {
        Ok(response) => response,
        Err(_) => Response::err(ctx.req.id, "router dispatch helper routing mismatch"),
    }
}
