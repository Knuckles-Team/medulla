//! The Decide layer's Prometheus series (EH-074, DECIDE-LAYER-DESIGN §11.3).
//!
//! Every label value comes from a closed set (method name, outcome arm,
//! evidence class, refusal code token, gate pass/fail), so cardinality is
//! bounded by the contract rather than by traffic.

use super::KernelMetrics;

#[cfg(feature = "metrics")]
impl KernelMetrics {
    /// One finished pipeline stage.
    pub fn decide_stage_seconds(&self, family: &str, stage: &str, seconds: f64) {
        self.decision_stage_seconds
            .with_label_values(&[family, stage])
            .observe(seconds);
    }

    /// One abstention, by its closed code.
    pub fn decide_abstentions(&self, code: &str) {
        self.decision_abstentions.with_label_values(&[code]).inc();
    }

    /// Nodes expanded over the node budget for one solved assembly.
    pub fn solver_nodes_ratio(&self, nodes: u64, budget: u64) {
        if budget > 0 {
            self.decision_solver_nodes_ratio
                .observe(nodes as f64 / budget as f64);
        }
    }

    /// The latest drift reading of one head.
    pub fn head_drift(&self, head: &str, drift_q16: i64) {
        self.decision_head_drift
            .with_label_values(&[head])
            .set(drift_q16);
    }

    /// One answered decision.
    pub fn decision_answered(&self, outcome: &str, evidence: &str, reason: &str) {
        self.decisions
            .with_label_values(&[outcome, evidence, reason])
            .inc();
    }

    /// One refused Decide-layer call.
    pub fn decision_refused(&self, method: &str, code: &str) {
        self.decision_refusals
            .with_label_values(&[method, code])
            .inc();
    }

    /// One exploration draw's branch.
    pub fn decision_explored(&self, explored: bool) {
        let branch = if explored { "explore" } else { "greedy" };
        self.decision_exploration.with_label_values(&[branch]).inc();
    }

    /// One evaluation verdict.
    pub fn decision_evaluated(&self, passed: bool) {
        let verdict = if passed { "passed" } else { "failed" };
        self.decision_evaluations
            .with_label_values(&[verdict])
            .inc();
    }

    /// One policy-bound threshold signal from a newly persisted evaluation.
    /// Callers supply only the closed signal and result vocabulary.
    pub fn decision_threshold_assessed(&self, signal: &'static str, result: &'static str) {
        self.decision_threshold_assessments
            .with_label_values(&[signal, result])
            .inc();
    }

    /// One Decide-layer call's latency.
    pub fn decision_latency(&self, method: &str, seconds: f64) {
        self.decision_latency
            .with_label_values(&[method])
            .observe(seconds);
    }
}

#[cfg(not(feature = "metrics"))]
#[allow(missing_docs)] // no-op shims; the metrics build documents the same functions
impl KernelMetrics {
    pub fn decide_stage_seconds(&self, _family: &str, _stage: &str, _seconds: f64) {}
    pub fn decide_abstentions(&self, _code: &str) {}
    pub fn solver_nodes_ratio(&self, _nodes: u64, _budget: u64) {}
    pub fn head_drift(&self, _head: &str, _drift_q16: i64) {}
    pub fn decision_answered(&self, _outcome: &str, _evidence: &str, _reason: &str) {}
    pub fn decision_refused(&self, _method: &str, _code: &str) {}
    pub fn decision_explored(&self, _explored: bool) {}
    pub fn decision_evaluated(&self, _passed: bool) {}
    pub fn decision_threshold_assessed(&self, _signal: &'static str, _result: &'static str) {}
    pub fn decision_latency(&self, _method: &str, _seconds: f64) {}
}

#[cfg(all(test, feature = "metrics"))]
mod tests {
    #[test]
    fn threshold_series_have_closed_zero_baselines() {
        let text = crate::metrics::KernelMetrics::new().render();
        for signal in ["support", "coverage", "act_risk"] {
            for result in ["ok", "breach", "unavailable"] {
                // The text encoder orders labels by name, so match each label
                // on its own rather than one fixed `signal,result` substring.
                let signal_label = format!("signal=\"{signal}\"");
                let result_label = format!("result=\"{result}\"");
                assert!(
                    text.lines().any(|line| {
                        line.starts_with("medulla_decision_threshold_assessments_total{")
                            && line.contains(&signal_label)
                            && line.contains(&result_label)
                    }),
                    "missing zero baseline for {signal_label},{result_label}"
                );
            }
        }
    }
}
