//! Served finance alerts (MEDULLA-FINANCE-PRIMITIVES-R011.2.3).
//!
//! `FinanceAlertRaise` is gateway-routed (`Route::FinanceRecords`). The door takes
//! no decision: it validates the envelope, refuses another tenant's event node, and
//! commits the event node (its id is the envelope `event_id`) through the staged
//! gateway. The batch compiler builds the `finance.events.v1` outbox fact from the
//! very method that commits the node (`finance_alert_intent`), so node and fact
//! commit together or not at all.
//!
//! A second raise with equal canonical bytes returns the first result and commits
//! nothing (one event id, one node, one outbox fact). Different bytes under the
//! same id refuse `ALERT_REVISION_CONFLICT` before the outbox is touched.

use super::*;

#[cfg(feature = "finance")]
use medulla_types::finance_event_envelope::FinanceAlertEnvelope;

/// The node property naming the opaque verified tenant scope of the raiser.
#[cfg(feature = "finance")]
const TENANT_SCOPE_KEY: &str = "finance_tenant_scope";
/// The node property holding the SHA-256 of the envelope's canonical msgpack.
#[cfg(feature = "finance")]
const DIGEST_KEY: &str = "alert_envelope_sha256";

/// A finance refusal is its closed `ServerErrorCode` wire code, then the detail.
#[cfg(feature = "finance")]
fn refuse(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

#[cfg(feature = "finance")]
fn envelope_digest(canonical: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(canonical))
}

/// The event node an envelope commits as, with its envelope digest.
#[cfg(feature = "finance")]
fn event_node_properties(
    envelope: &FinanceAlertEnvelope,
    tenant_id: &str,
    tenant_scope: &str,
    digest: &str,
) -> Result<Vec<u8>, String> {
    let props = serde_json::json!({
        "type": "FinanceAlert",
        "finance_kind": "FinanceAlert",
        "event_id": envelope.event_id,
        "alert_schema": envelope.outbox_event_schema(),
        "listing_id": envelope.listing_id,
        // msgpack has no 128-bit integer; the decimal string is exact.
        "occurred_at_unix_nanos": envelope.occurred_at_unix_nanos.to_string(),
        "message": envelope.message,
        "tenant": tenant_id,
        TENANT_SCOPE_KEY: tenant_scope,
        DIGEST_KEY: digest,
    });
    rmp_serde::to_vec_named(&props)
        .map_err(|error| format!("INVALID_ARGUMENT: finance alert encoding failed: {error}"))
}

/// What the stored node under `node_id` says about a raise of `digest`.
#[cfg(feature = "finance")]
#[derive(Debug, PartialEq, Eq)]
enum Stored {
    /// No node: the raise commits.
    Absent,
    /// The same envelope is already committed: return the first result.
    Same,
}

#[cfg(feature = "finance")]
fn check_stored(
    core: &GraphCore,
    node_id: &str,
    tenant_scope: &str,
    digest: &str,
) -> Result<Stored, String> {
    let Some(blob) = core.get_node_properties(node_id) else {
        return Ok(Stored::Absent);
    };
    let props = medulla_types::msgpack::decode_property_object(&blob).unwrap_or_default();
    if let Some(owner) = props.get(TENANT_SCOPE_KEY).and_then(|v| v.as_str()) {
        if owner != tenant_scope {
            return Err(format!(
                "ACCESS_DENIED: {node_id} belongs to another tenant"
            ));
        }
    }
    if props.get(DIGEST_KEY).and_then(|v| v.as_str()) == Some(digest) {
        return Ok(Stored::Same);
    }
    Err(refuse(
        "ALERT_REVISION_CONFLICT",
        format!("{node_id} is already committed with different bytes"),
    ))
}

/// The `Route::FinanceRecords` entry for `FinanceAlertRaise`.
pub(super) async fn raise(
    ctx: &MutationCtx<'_>,
    tenant_id: &str,
    method: &Method,
) -> Option<Response> {
    if !matches!(method, Method::FinanceAlertRaise { .. }) {
        return None;
    }
    #[cfg(feature = "finance")]
    {
        let Method::FinanceAlertRaise { envelope } = method else {
            unreachable!("raise is only called for FinanceAlertRaise");
        };
        if let Err(error) = envelope.validate() {
            return Some(Response::err(
                ctx.req_id,
                refuse("ALERT_ENVELOPE_INVALID", error),
            ));
        }
        let canonical = match envelope.canonical_msgpack() {
            Ok(bytes) => bytes,
            Err(error) => {
                return Some(Response::err(ctx.req_id, refuse("INVALID_ARGUMENT", error)))
            }
        };
        let digest = envelope_digest(&canonical);
        let node_id = envelope.event_id.clone();
        match check_stored(ctx.core, &node_id, ctx.tenant_scope, &digest) {
            Ok(Stored::Same) => {
                // The gateway's idempotent replay: the first result, no second batch.
                return Some(Response::ok(ctx.req_id, ResultPayload::String(node_id)));
            }
            Ok(Stored::Absent) => {}
            Err(error) => return Some(Response::err(ctx.req_id, error)),
        }
        let properties = match event_node_properties(envelope, tenant_id, ctx.tenant_scope, &digest)
        {
            Ok(blob) => blob,
            Err(error) => return Some(Response::err(ctx.req_id, error)),
        };
        let plan = MutationPlan::for_method(method);
        let tenant_scope = ctx.tenant_scope.to_string();
        let response = medulla_server_ops::api::ingest::graph::commit_gateway(
            ctx,
            &plan,
            method,
            move |core| {
                // The apply closure runs under the graph lane, so a competing raise
                // that committed since the check above is a compare-and-set failure.
                match check_stored(core, &node_id, &tenant_scope, &digest)? {
                    Stored::Absent => {}
                    Stored::Same => {
                        return Err(refuse(
                            "ALERT_REVISION_CONFLICT",
                            format!("{node_id} was committed concurrently"),
                        ))
                    }
                }
                core.create_node_if_absent(node_id.clone(), properties);
                Ok(ResultPayload::String(node_id))
            },
        )
        .await;
        Some(response)
    }
    #[cfg(not(feature = "finance"))]
    {
        let _ = tenant_id;
        Some(Response::err(
            ctx.req_id,
            "METHOD_NOT_YET_SERVED: FinanceAlertRaise needs the finance feature",
        ))
    }
}
