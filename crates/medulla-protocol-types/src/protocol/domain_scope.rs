//! Domain readings of a [`Method`] body: which work-market scope a write names
//! and which ordinary run completion a work-item commit carries. They live with
//! the method registry so domain type crates never name `Method`.

use super::Method;
use medulla_types::run_completion::RunCompletionPayload;

/// The ordinary run completion a `CommitWorkItemResult` carries, validated, or
/// `None` for any other method.
pub fn run_completion_payload(
    method: &Method,
    batch_id: &str,
) -> Result<Option<RunCompletionPayload>, String> {
    let Method::CommitWorkItemResult {
        tenant,
        work_item_id,
        outcome,
        result_ref,
        error_ref,
        retryable,
        outcome_extension,
        run_completion: Some(completion),
        ..
    } = method
    else {
        return Ok(None);
    };
    completion.validate()?;
    if outcome_extension.is_some()
        || *retryable
        || !matches!(outcome.as_str(), "succeeded" | "failed" | "cancelled")
        || (outcome != "succeeded" && result_ref.is_some())
    {
        return Err("ordinary run completion requires an exclusive final result without failed answer artifacts".into());
    }
    Ok(Some(RunCompletionPayload {
        completion: (**completion).clone(),
        tenant: tenant.clone(),
        work_item_id: work_item_id.clone(),
        batch_id: batch_id.into(),
        outcome: outcome.clone(),
        result_ref: result_ref.clone(),
        error_ref: error_ref.clone(),
    }))
}

/// The tenant and idempotency key a work-market write names in its body, or
/// `None` for any other method. One owner for the kernel's tenant binding,
/// batch identity and audit.
pub fn market_write_scope(method: &Method) -> Option<(&str, &str)> {
    match method {
        Method::GapUpsert { request } => {
            Some((request.tenant.as_str(), request.idempotency_key.as_str()))
        }
        Method::GapTransition { request } => {
            Some((request.tenant.as_str(), request.idempotency_key.as_str()))
        }
        Method::GapSettle { request } => {
            Some((request.tenant.as_str(), request.idempotency_key.as_str()))
        }
        Method::WorkOfferPut { request } => {
            Some((request.tenant.as_str(), request.idempotency_key.as_str()))
        }
        _ => None,
    }
}
