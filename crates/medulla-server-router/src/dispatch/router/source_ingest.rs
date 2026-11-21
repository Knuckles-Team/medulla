use super::*;

use medulla_protocol_types::result_contract::ingestion as ingestion_results;
#[cfg(feature = "jobs")]
use medulla_protocol_types::result_contract::coordination as coordination_results;

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
            dispatch_boxed(ingest_enqueue(ctx, *envelope)).await
        }
        // MEDULLA-INGEST-WORKER-R001.11: the served backpressure read.
        #[cfg(feature = "jobs")]
        Method::IngestQueueDepth { queue } => {
            dispatch_boxed(ingest_queue_depth(ctx, queue)).await
        }
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
async fn ingest_enqueue(
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
