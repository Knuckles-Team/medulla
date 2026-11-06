//! Served factor recording (MEDULLA-FINANCE-PRIMITIVES-R018.5, R018.6).
//!
//! `FinanceFactorRecord` is gateway-routed (`Route::FinanceRecords`). The handler
//! ranks the factors with the one shared kernel (`medulla_finance_quant::factor`),
//! resolves every provenance key to a stored node of the writer's tenant, then
//! commits the `Factor` nodes and one `derivedFrom` edge per provenance key as the
//! one `BatchUpdate`. A refusal writes nothing. `FinanceFactorRank` stays the
//! read-only form and never reaches this file.

use super::*;

#[cfg(feature = "finance")]
use medulla_finance_quant::factor::{self, FactorError, FactorInput, FactorRecord};

/// The node property naming the opaque verified tenant scope of the writer.
#[cfg(feature = "finance")]
const TENANT_SCOPE_KEY: &str = "finance_tenant_scope";
/// The edge label from a factor node to each input it was derived from.
#[cfg(feature = "finance")]
const DERIVED_FROM: &str = "derivedFrom";

/// A factor refusal is its closed `ServerErrorCode` wire code, then the detail.
#[cfg(feature = "finance")]
fn invalid(detail: impl std::fmt::Display) -> String {
    format!("FACTOR_RECORD_INVALID: {detail}")
}

#[cfg(feature = "finance")]
fn kernel_refusal(error: FactorError) -> String {
    match error {
        FactorError::InvalidRecord { .. } => invalid(format!("{error:?}")),
        other => format!("INVALID_ARGUMENT: {other}"),
    }
}

/// Rank the request's factors; the parallel lists must agree and every factor
/// must name at least one provenance key.
#[cfg(feature = "finance")]
fn ranked(
    factor_ids: &[String],
    signals: &[Vec<f64>],
    provenance_keys: &[Vec<String>],
    forward_returns: &[f64],
) -> Result<Vec<FactorRecord>, String> {
    if factor_ids.len() != signals.len() || factor_ids.len() != provenance_keys.len() {
        return Err(invalid(
            "factor_ids, signals and provenance_keys differ in length",
        ));
    }
    if provenance_keys.iter().any(Vec::is_empty) {
        return Err(invalid("a factor has an empty provenance list"));
    }
    let inputs: Vec<FactorInput> = factor_ids
        .iter()
        .zip(signals)
        .zip(provenance_keys)
        .map(|((factor_id, signal), keys)| FactorInput {
            factor_id: factor_id.clone(),
            signal: signal.clone(),
            input_provenance_keys: keys.clone(),
        })
        .collect();
    factor::rank_factors(&inputs, forward_returns).map_err(kernel_refusal)
}

#[cfg(feature = "finance")]
fn factor_node_id(factor_id: &str) -> String {
    format!("finance:Factor:{factor_id}")
}

/// The tenant scope stored on a node, `None` when the node is absent or unscoped.
#[cfg(feature = "finance")]
fn stored_scope(core: &GraphCore, node_id: &str) -> Option<Option<String>> {
    let blob = core.get_node_properties(node_id)?;
    let props = medulla_types::msgpack::decode_property_object(&blob).ok()?;
    Some(
        props
            .get(TENANT_SCOPE_KEY)
            .and_then(|v| v.as_str())
            .map(str::to_owned),
    )
}

/// A node of another tenant is never read or overwritten.
#[cfg(feature = "finance")]
fn ensure_owned(core: &GraphCore, node_id: &str, tenant_scope: &str) -> Result<bool, String> {
    match stored_scope(core, node_id) {
        None => Ok(false),
        Some(Some(owner)) if owner != tenant_scope => Err(format!(
            "ACCESS_DENIED: {node_id} belongs to another tenant"
        )),
        Some(_) => Ok(true),
    }
}

/// Every provenance key must resolve to a node of the writer's tenant, and no
/// factor node of another tenant may be overwritten.
#[cfg(feature = "finance")]
fn check_nodes(
    core: &GraphCore,
    tenant_scope: &str,
    records: &[FactorRecord],
) -> Result<(), String> {
    for record in records {
        ensure_owned(core, &factor_node_id(&record.factor_id), tenant_scope)?;
        for key in &record.input_provenance_keys {
            if !ensure_owned(core, key, tenant_scope)? {
                return Err(invalid(format!("provenance key {key} does not resolve")));
            }
        }
    }
    Ok(())
}

/// The batch operations: one `upsert_node` per factor, one `upsert_edge` per key.
#[cfg(feature = "finance")]
fn batch_operations(
    records: &[FactorRecord],
    tenant_id: &str,
    tenant_scope: &str,
) -> Result<Vec<u8>, String> {
    let mut operations = Vec::new();
    for record in records {
        let node_id = factor_node_id(&record.factor_id);
        operations.push(serde_json::json!({
            "op": "upsert_node",
            "id": node_id,
            "properties": {
                "type": "Factor",
                "finance_kind": "Factor",
                "factorId": record.factor_id,
                "informationCoefficient": record.information_coefficient,
                "informationRatio": record.information_ratio,
                "factorRank": record.rank,
                "irKernelVersion": factor::KERNEL_REVISION,
                "icKernelVersion": factor::KERNEL_REVISION,
                "tenant": tenant_id,
                TENANT_SCOPE_KEY: tenant_scope,
            },
        }));
        operations.extend(record.input_provenance_keys.iter().map(|key| {
            serde_json::json!({
                "op": "upsert_edge",
                "source": node_id,
                "target": key,
                "properties": {"relationship": DERIVED_FROM},
            })
        }));
    }
    rmp_serde::to_vec_named(&operations)
        .map_err(|error| format!("INVALID_ARGUMENT: factor record encoding failed: {error}"))
}

/// Validate, then build the `BatchUpdate` the request commits as and its result.
#[cfg(feature = "finance")]
fn admit(
    core: &GraphCore,
    tenant_id: &str,
    tenant_scope: &str,
    method: &Method,
) -> Result<(Method, ResultPayload), String> {
    let Method::FinanceFactorRecord {
        factor_ids,
        signals,
        provenance_keys,
        forward_returns,
    } = method
    else {
        unreachable!("admit is only called for FinanceFactorRecord");
    };
    let records = ranked(factor_ids, signals, provenance_keys, forward_returns)?;
    check_nodes(core, tenant_scope, &records)?;
    let operations_msgpack = batch_operations(&records, tenant_id, tenant_scope)?;
    let payload = ResultPayload::of::<
        medulla_protocol_types::result_contract::graph::FinanceFactorRecord,
    >(records.into_iter().map(Into::into).collect())?;
    Ok((Method::BatchUpdate { operations_msgpack }, payload))
}

#[cfg(feature = "finance")]
async fn commit(ctx: &MutationCtx<'_>, tenant_id: &str, method: &Method) -> Response {
    let (write, payload) = match admit(ctx.core, tenant_id, ctx.tenant_scope, method) {
        Ok(admitted) => admitted,
        Err(error) => return Response::err(ctx.req_id, error),
    };
    let Method::BatchUpdate { operations_msgpack } = &write else {
        unreachable!("admit builds a BatchUpdate");
    };
    let operations = operations_msgpack.clone();
    let plan = MutationPlan::for_method(&write);
    medulla_server_ops::api::ingest::graph::commit_gateway(ctx, &plan, &write, move |core| {
        medulla_graph_algos::algorithms::batch_update(core, &operations).map(|_| payload)
    })
    .await
}

/// The `Route::FinanceRecords` entry for `FinanceFactorRecord`.
pub(super) async fn record(
    ctx: &MutationCtx<'_>,
    tenant_id: &str,
    method: &Method,
) -> Option<Response> {
    if !matches!(method, Method::FinanceFactorRecord { .. }) {
        return None;
    }
    #[cfg(feature = "finance")]
    {
        Some(commit(ctx, tenant_id, method).await)
    }
    #[cfg(not(feature = "finance"))]
    {
        let _ = tenant_id;
        Some(Response::err(
            ctx.req_id,
            "METHOD_NOT_YET_SERVED: FinanceFactorRecord needs the finance feature",
        ))
    }
}
