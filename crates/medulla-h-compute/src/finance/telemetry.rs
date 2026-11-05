//! Evidence spans for the served finance kernels (MEDULLA-FINANCE-PRIMITIVES-R900.5,
//! R613.5).
//!
//! `kernel_span` opens one named `tracing` span per served kernel call, child of the
//! request span. It records the verified `tenant`, the SHA-256 `inputs_digest` of the
//! canonical request bytes (64 lowercase hex characters) and, once the kernel answers,
//! the `outcome` (`ok` or the refusal code). It never records a sample value.

use medulla_protocol_types::protocol::Method;
use sha2::Digest;
use sha2::Sha256;
use tracing::field::Empty;
use tracing::Span;

/// The served kernels that carry an evidence span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kernel {
    PositionSize,
    DistributionShift,
    RegimeChain,
    ExecutionSchedule,
}

/// The outcome attribute of a kernel call that answered.
pub(super) const OUTCOME_OK: &str = "ok";

/// SHA-256 of the canonical (named MessagePack) request bytes, lowercase hex.
pub(super) fn inputs_digest(method: &Method) -> String {
    let bytes = rmp_serde::to_vec_named(method).unwrap_or_default();
    hex::encode(Sha256::digest(&bytes))
}

/// Open the evidence span of one served kernel call.
pub(super) fn kernel_span(kernel: Kernel, tenant: &str, method: &Method) -> Span {
    let digest = inputs_digest(method);
    match kernel {
        Kernel::PositionSize => tracing::info_span!(
            "medulla.finance.position_size",
            tenant = %tenant,
            inputs_digest = %digest,
            outcome = Empty,
            cap_applied = Empty,
        ),
        Kernel::DistributionShift => tracing::info_span!(
            "medulla.finance.distribution_shift",
            tenant = %tenant,
            inputs_digest = %digest,
            outcome = Empty,
        ),
        Kernel::RegimeChain => tracing::info_span!(
            "medulla.finance.regime_chain",
            tenant = %tenant,
            inputs_digest = %digest,
            outcome = Empty,
        ),
        Kernel::ExecutionSchedule => tracing::info_span!(
            "medulla.finance.execution_schedule",
            tenant = %tenant,
            inputs_digest = %digest,
            outcome = Empty,
        ),
    }
}

/// Record the outcome (`ok` or the refusal code) on the span.
pub(super) fn record_outcome(span: &Span, outcome: &str) {
    span.record("outcome", outcome);
}

/// Emit the refusal trace event (stable code + refused field name) inside the span,
/// and record the code as the span outcome.
pub(super) fn refusal_event(span: &Span, code: &'static str, field: &'static str) {
    record_outcome(span, code);
    span.in_scope(|| {
        tracing::warn!(code, field, "finance kernel parameter refused");
    });
}
