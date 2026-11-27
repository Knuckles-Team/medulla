//! Ingest job metrics (MEDULLA-INGEST-WORKER-R001.13).
//!
//! Emitted by the engine's ingest queue handlers (`enqueue_ingest_job` and the
//! ack and claim hooks) from each job outcome -- the kernel has no ingest
//! dependency. Labels hold the outcome and the queue only, never a tenant,
//! corpus or job id, so cardinality is bounded by the closed outcome set and
//! the fixed queue names. The depth and oldest-age gauges belong to the lane
//! gauges of MEDULLA-INGEST-WORKER-R604.5; no second gauge set exists here.

#[cfg(feature = "metrics")]
use super::KernelMetrics;
#[cfg(feature = "metrics")]
use medulla_types::ingest_lanes::{LaneMetrics, QueueClass};

/// The closed set of ingest job outcomes, the `outcome` label of
/// `medulla_ingest_jobs_total`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IngestJobOutcome {
    /// A new job entered the queue.
    Enqueued,
    /// A repeat of a queued job returned the first receipt.
    Deduplicated,
    /// The enqueue was refused.
    Refused,
    /// A job was acknowledged as done.
    Completed,
    /// A failed job was scheduled for redelivery.
    Retried,
    /// A job exhausted its attempts and was parked in the dead letter.
    DeadLettered,
    /// A stale redelivery was dropped.
    Stale,
}

impl IngestJobOutcome {
    /// The label value.
    pub fn label(self) -> &'static str {
        match self {
            Self::Enqueued => "enqueued",
            Self::Deduplicated => "deduplicated",
            Self::Refused => "refused",
            Self::Completed => "completed",
            Self::Retried => "retried",
            Self::DeadLettered => "dead_lettered",
            Self::Stale => "stale",
        }
    }
}

#[cfg(feature = "metrics")]
impl KernelMetrics {
    /// Count one ingest job `outcome` on `queue`. A dead-lettered job also
    /// counts once in `medulla_ingest_dead_letter_total{queue}`.
    pub fn record_job(&self, outcome: IngestJobOutcome, queue: &str) {
        self.ingest_jobs_total
            .with_label_values(&[outcome.label()])
            .inc();
        if outcome == IngestJobOutcome::DeadLettered {
            self.ingest_dead_letter_total
                .with_label_values(&[queue])
                .inc();
        }
    }

    /// Set the lane gauges from one lane's [`LaneMetrics`] (MEDULLA-INGEST-WORKER-R604.5):
    /// `medulla_lane_depth{lane,queue_class}`, `medulla_lane_oldest_age_seconds{lane}` and
    /// `medulla_lane_inflight{lane}`. These are the only depth and oldest-age gauges of
    /// ingest.
    pub fn set_lane_gauges(&self, lane: &str, queue_class: QueueClass, metrics: &LaneMetrics) {
        let class = match queue_class {
            QueueClass::Fast => "fast",
            QueueClass::Medium => "medium",
            QueueClass::SlowHeavy => "slow_heavy",
        };
        self.lane_depth
            .with_label_values(&[lane, class])
            .set(i64::try_from(metrics.depth).unwrap_or(i64::MAX));
        self.lane_oldest_age_seconds
            .with_label_values(&[lane])
            .set(i64::try_from(metrics.oldest_age_ms / 1_000).unwrap_or(i64::MAX));
        self.lane_inflight
            .with_label_values(&[lane])
            .set(i64::try_from(metrics.inflight).unwrap_or(i64::MAX));
    }

    /// Count one claim that worker admission refused in `lane`.
    pub fn admit_refused(&self, lane: &str) {
        self.admit_refused_total.with_label_values(&[lane]).inc();
    }

    /// Every sample of the ingest worker series this registry holds, read from the one
    /// exposition the scrape endpoint serves (MEDULLA-INGEST-WORKER-R001.18).
    pub fn ingest_status(&self) -> medulla_types::ingest_lanes::IngestWorkerStatusReport {
        medulla_types::ingest_lanes::ingest_series_from_exposition(&self.render())
    }
}
