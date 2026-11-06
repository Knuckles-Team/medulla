//! Evidence of a committed uncertain-effect write (MEDULLA-DURABLE-KERNEL-R820.5).
//!
//! After the commit, never inside the store crate: one operation-audit row and
//! the uncertain-effect metrics. A replayed write changes nothing, so it adds
//! no evidence.

use medulla_protocol_types::protocol::{Method, ResultPayload};
use medulla_work_types::work_item_read::EffectWriteResult;

use medulla_persistence::AsRedb;

use super::HandleContext;

/// The audited operation name of a start mark or a receipt; `None` for every
/// other method.
pub(super) fn audited_effect_op(method: &Method) -> Option<&'static str> {
    match method {
        Method::MarkWorkItemEffectStarted { .. } => Some("MarkWorkItemEffectStarted"),
        Method::RecordEffectReceipt { .. } => Some("RecordEffectReceipt"),
        _ => None,
    }
}

/// How far a write moves the open-uncertain gauge: a mark opens one, a
/// receipt closes one.
fn open_delta(op: &str) -> i64 {
    if op == "MarkWorkItemEffectStarted" {
        1
    } else {
        -1
    }
}

/// Count and audit one committed write. The audit refusal of the writer is
/// returned as it is.
pub(super) async fn record(
    ctx: &HandleContext<'_>,
    op: &'static str,
    payload: &ResultPayload,
) -> Result<(), String> {
    let result: EffectWriteResult =
        crate::delegation::validation::decode_native_result(payload.clone())?;
    if result.replayed {
        return Ok(());
    }
    audit(ctx, op, &result).await?;
    medulla_kernel::metrics::work_item_uncertain_effect(result.outcome_label());
    medulla_kernel::metrics::work_item_uncertain_open_add(open_delta(op));
    Ok(())
}

async fn audit(
    ctx: &HandleContext<'_>,
    op: &'static str,
    result: &EffectWriteResult,
) -> Result<(), String> {
    let Some(redb) = ctx.persistence.as_ref().and_then(|p| p.as_redb()) else {
        return Ok(());
    };
    let principal = ctx.verified_context.principal_persistence_id();
    redb.audit_effect_write(
        &medulla_kernel::persist::sanitize(ctx.graph_name),
        medulla_persistence::redb_backend::EffectAudit {
            tenant: ctx.verified_context.tenant(),
            principal: &principal,
            op,
            result,
            now_ms: medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
        },
    )
    .await?;
    // Every appended audit entry is one admitted maintenance batch.
    ctx.core.mark_dirty();
    Ok(())
}
