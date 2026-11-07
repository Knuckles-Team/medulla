//! Mutation-outbox consumer metrics (X10, PX10b item 9).
//!
//! Emitted by the consumers themselves from each claim outcome -- the kernel
//! has no metrics dependency. Labels are the consumer and topic only, never a
//! tenant or graph, so cardinality is bounded by the fixed set of consumers;
//! per-scope detail is `Method::MutationOutbox`'s `status`.

use super::KernelMetrics;

#[cfg(feature = "metrics")]
impl KernelMetrics {
    /// Count `rows` dead-lettered for `cause` (`exhausted` by bounded retry,
    /// or the consumer's own reject reason).
    pub fn outbox_dead_lettered(&self, consumer: &str, topic: &str, cause: &str, rows: u64) {
        self.outbox_dead_lettered_total
            .with_label_values(&[consumer, topic, cause])
            .inc_by(rows);
    }

    /// Publish where a consumer's stream head stands.
    pub fn set_outbox_head(&self, consumer: &str, topic: &str, age_ms: u64, attempt: u32) {
        let age = i64::try_from(age_ms).unwrap_or(i64::MAX);
        self.outbox_head_age_ms
            .with_label_values(&[consumer, topic])
            .set(age);
        self.outbox_head_attempt
            .with_label_values(&[consumer, topic])
            .set(i64::from(attempt));
    }
}

#[cfg(not(feature = "metrics"))]
#[allow(missing_docs)] // no-op shims; the metrics build documents the same functions
impl KernelMetrics {
    pub fn outbox_dead_lettered(&self, _consumer: &str, _topic: &str, _cause: &str, _rows: u64) {}
    pub fn set_outbox_head(&self, _consumer: &str, _topic: &str, _age_ms: u64, _attempt: u32) {}
}
