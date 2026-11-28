//! Served harness session index write (MEDULLA-ORCH-VOCAB-R009.101.1).
//!
//! `IndexHarnessSessionBatch` is gateway-routed (`Route::IndexHarnessSessionBatch`). The batch
//! is admitted against the tenant's graph by `medulla_h_ingest::harness_session::admit_batch`
//! (every record verified before anything is planned) and the planned index nodes, receipt
//! node, counter node and `indexesRun` edges commit as the one `BatchUpdate` through the staged
//! gateway. The commit closure re-reads the counter revision, so two racing batches never both
//! write index revision `n + 1`. An identical replay plans no write and answers from the plan.

use super::process_compile::{batch, refuse};
use super::*;
use medulla_protocol_types::result_contract::coordination::IndexHarnessSessionBatch;
use medulla_server_api::knob_config::knobs;
use medulla_types::harness_session::SESSION_CONTENT_CHANGED;
use {
    medulla_h_ingest_repository::harness_session::admit_batch,
    medulla_h_ingest_repository::harness_session::index_revision,
};

/// The `Route::IndexHarnessSessionBatch` entry.
pub async fn handle_index_harness_session_batch(
    ctx: &MutationCtx<'_>,
    _tenant_id: &str,
    method: &Method,
) -> Option<Response> {
    let Method::IndexHarnessSessionBatch { batch: request } = method else {
        return None;
    };
    let limit =
        usize::try_from(knobs().orchestration_limits_session_batch_max).unwrap_or(usize::MAX);
    let admission = match admit_batch(ctx.core, ctx.tenant_scope, request, limit) {
        Ok(admission) => admission,
        Err(error) => return Some(Response::err(ctx.req_id, error)),
    };
    if admission.operations.is_empty() {
        return Some(
            match ResultPayload::of::<IndexHarnessSessionBatch>(admission.result) {
                Ok(payload) => Response::ok(ctx.req_id, payload),
                Err(error) => Response::err(ctx.req_id, error),
            },
        );
    }
    let write = match batch(&admission.operations) {
        Ok(write) => write,
        Err(error) => return Some(Response::err(ctx.req_id, error)),
    };
    let Method::BatchUpdate { operations_msgpack } = &write else {
        unreachable!("an admitted batch is a BatchUpdate");
    };
    let operations = operations_msgpack.clone();
    let tenant = ctx.tenant_scope.to_string();
    let (result, before) = (admission.result, admission.revision_before);
    let plan = MutationPlan::for_method(&write);
    Some(
        medulla_server_ops::api::ingest::graph::commit_gateway(ctx, &plan, &write, move |core| {
            let current = index_revision(core, &tenant);
            if current != before {
                return Err(refuse(
                    SESSION_CONTENT_CHANGED,
                    format!("the index moved to revision {current} while the batch was admitted"),
                ));
            }
            medulla_graph_algos::algorithms::batch_update(core, &operations)
                .and_then(|_| ResultPayload::of::<IndexHarnessSessionBatch>(result))
        })
        .await,
    )
}
