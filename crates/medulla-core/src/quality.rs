//! Quality rule compilation, scoring and failure tracking
//! (MEDULLA-UNIFIED-DATA-PLANE-R762.2).
//!
//! The functions here are pure: [`compile_rule`] turns a rule into the
//! statement, shape IRI or lag bound to run, [`evaluate_rule`] turns what the
//! runner measured into a score, and [`track_failure`] counts consecutive
//! failures and opens one incident through an [`IncidentSink`].

use medulla_types::contract::Digest256;
use medulla_types::quality::{InvalidQualityRule, QualityDimension, QualityRule, QualityScore};

/// What a rule compiles to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompiledRule {
    /// One pushed aggregate statement over the rule target.
    Sql { statement: String },
    /// A shape IRI validated over the target's focus nodes.
    Shape { iri: String },
    /// A change-capture lag bound.
    Freshness { max_lag_ms: u64 },
}

/// What the runner measured for a compiled rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Measurement {
    /// Rows (or focus nodes) checked and how many of them failed.
    Rows { checked: u64, failed: u64 },
    /// The observed change-capture lag.
    LagMs(u64),
}

/// One scored evaluation with its evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct Evaluation {
    pub score: QualityScore,
    /// SHA-256 over the compiled statement or shape IRI, lowercase hex.
    pub query_digest: String,
    pub rows_checked: u64,
    pub rows_failed: u64,
}

/// The anomaly handed to the incident authority.
#[derive(Debug, Clone, PartialEq)]
pub struct QualityAnomaly {
    pub entity_ref: String,
    pub signal: String,
    pub observed: f64,
    pub baseline: f64,
}

/// Where a failing streak is reported (`IncidentRecordAnomaly` behind the
/// `DecideApi` seam).
pub trait IncidentSink {
    fn record_anomaly(&mut self, anomaly: &QualityAnomaly);
}

const SHAPE_TARGET_PREFIX: &str = "graph:";

fn refuse(reason: &str) -> InvalidQualityRule {
    InvalidQualityRule::RuleInvalid(reason.to_string())
}

fn balanced(expression: &str) -> bool {
    let mut depth = 0_i64;
    for c in expression.chars() {
        depth += match c {
            '(' => 1,
            ')' => -1,
            _ => 0,
        };
        if depth < 0 {
            return false;
        }
    }
    depth == 0
}

fn check_predicate(expression: &str) -> Result<(), InvalidQualityRule> {
    let smuggles = [";", "--", "/*"].iter().any(|t| expression.contains(t));
    if smuggles || !balanced(expression) {
        return Err(refuse("expression is not a single row predicate"));
    }
    Ok(())
}

fn check_target(target: &str) -> Result<(), InvalidQualityRule> {
    let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-');
    if !target.chars().all(plain) {
        return Err(refuse("target is not a plain dataset reference"));
    }
    Ok(())
}

fn compile_sql(rule: &QualityRule) -> Result<CompiledRule, InvalidQualityRule> {
    check_target(&rule.target)?;
    check_predicate(&rule.expression)?;
    Ok(CompiledRule::Sql {
        statement: format!(
            "SELECT COUNT(*) AS rows_checked, COUNT(*) FILTER (WHERE {}) AS rows_passed FROM {}",
            rule.expression.trim(),
            rule.target
        ),
    })
}

fn compile_freshness(rule: &QualityRule) -> Result<CompiledRule, InvalidQualityRule> {
    let max_lag_ms = rule
        .expression
        .trim()
        .parse()
        .map_err(|_| refuse("freshness expression is not a lag bound in milliseconds"))?;
    Ok(CompiledRule::Freshness { max_lag_ms })
}

fn compile_shape(rule: &QualityRule) -> Result<CompiledRule, InvalidQualityRule> {
    let iri = rule.expression.trim();
    if !iri.contains(':') || iri.contains(char::is_whitespace) {
        return Err(refuse("graph rule expression is not a shape IRI"));
    }
    Ok(CompiledRule::Shape {
        iri: iri.to_string(),
    })
}

/// Compile one rule: a freshness rule reads a lag bound, a rule on a
/// `graph:` target names a shape IRI, any other rule is a SQL row predicate.
pub fn compile_rule(rule: &QualityRule) -> Result<CompiledRule, InvalidQualityRule> {
    rule.validate()?;
    if rule.dimension == QualityDimension::Freshness {
        compile_freshness(rule)
    } else if rule.target.starts_with(SHAPE_TARGET_PREFIX) {
        compile_shape(rule)
    } else {
        compile_sql(rule)
    }
}

fn fingerprint(compiled: &CompiledRule) -> String {
    let text = match compiled {
        CompiledRule::Sql { statement } => statement.clone(),
        CompiledRule::Shape { iri } => iri.clone(),
        CompiledRule::Freshness { max_lag_ms } => format!("lag_ms <= {max_lag_ms}"),
    };
    Digest256::sha256(text.as_bytes()).to_hex()
}

fn ratio(checked: u64, failed: u64) -> f64 {
    if checked == 0 {
        return 1.0;
    }
    (checked - failed.min(checked)) as f64 / checked as f64
}

fn measured(compiled: &CompiledRule, measurement: Measurement) -> (f64, u64, u64) {
    match (compiled, measurement) {
        (CompiledRule::Freshness { max_lag_ms }, Measurement::LagMs(lag)) => {
            (if lag <= *max_lag_ms { 1.0 } else { 0.0 }, 0, 0)
        }
        (_, Measurement::Rows { checked, failed }) => {
            let failed = failed.min(checked);
            (ratio(checked, failed), checked, failed)
        }
        (_, Measurement::LagMs(_)) => (0.0, 0, 0),
    }
}

/// Score one measurement against a rule. An empty dataset scores 1.0 with
/// `rows_checked = 0`; a freshness rule scores 1.0 within its lag bound and
/// 0.0 beyond it.
pub fn evaluate_rule(
    rule: &QualityRule,
    compiled: &CompiledRule,
    measurement: Measurement,
    at: i64,
) -> Evaluation {
    let (value, rows_checked, rows_failed) = measured(compiled, measurement);
    Evaluation {
        score: rule.score(value, at),
        query_digest: fingerprint(compiled),
        rows_checked,
        rows_failed,
    }
}

/// Count consecutive failures and report the streak. A passing score resets
/// the count; once `incident_after` failures run in a row every further
/// failure reports the same signature, which the incident authority folds
/// into the one open incident. Returns the new count.
pub fn track_failure(
    previous: u32,
    rule: &QualityRule,
    evaluation: &Evaluation,
    incident_after: u32,
    sink: &mut dyn IncidentSink,
) -> u32 {
    if evaluation.score.passed {
        return 0;
    }
    let count = previous.saturating_add(1);
    if count >= incident_after {
        sink.record_anomaly(&QualityAnomaly {
            entity_ref: rule.target.clone(),
            signal: format!("quality:{}", dimension_name(rule.dimension)),
            observed: evaluation.score.value,
            baseline: rule.threshold,
        });
    }
    count
}

fn dimension_name(dimension: QualityDimension) -> String {
    serde_json::to_value(dimension)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::quality::QualitySeverity;

    fn rule(dimension: QualityDimension, target: &str, expression: &str) -> QualityRule {
        QualityRule {
            id: "r1".to_string(),
            target: target.to_string(),
            dimension,
            expression: expression.to_string(),
            threshold: 0.9,
            severity: QualitySeverity::Critical,
        }
    }

    /// Records anomalies the way the incident authority folds them: one open
    /// incident per (entity, signal).
    #[derive(Default)]
    struct FakeIncidents {
        calls: Vec<QualityAnomaly>,
    }

    impl FakeIncidents {
        fn open(&self) -> usize {
            let mut keys: Vec<_> = self
                .calls
                .iter()
                .map(|a| (&a.entity_ref, &a.signal))
                .collect();
            keys.sort();
            keys.dedup();
            keys.len()
        }
    }

    impl IncidentSink for FakeIncidents {
        fn record_anomaly(&mut self, anomaly: &QualityAnomaly) {
            self.calls.push(anomaly.clone());
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R762.2
    #[test]
    fn each_rule_kind_scores() {
        let sql = rule(QualityDimension::Validity, "app.orders", "email LIKE '%@%'");
        let shape = rule(
            QualityDimension::Consistency,
            "graph:orders",
            "http://ex/OrderShape",
        );
        let fresh = rule(QualityDimension::Freshness, "app.orders", "5000");
        let cases = [
            (
                &sql,
                Measurement::Rows {
                    checked: 100,
                    failed: 5,
                },
                0.95,
                100,
                5,
            ),
            (
                &shape,
                Measurement::Rows {
                    checked: 4,
                    failed: 1,
                },
                0.75,
                4,
                1,
            ),
            (&fresh, Measurement::LagMs(4_000), 1.0, 0, 0),
            (&fresh, Measurement::LagMs(9_000), 0.0, 0, 0),
            (
                &sql,
                Measurement::Rows {
                    checked: 0,
                    failed: 0,
                },
                1.0,
                0,
                0,
            ),
        ];
        for (rule, measurement, value, checked, failed) in cases {
            let compiled = compile_rule(rule).unwrap();
            let got = evaluate_rule(rule, &compiled, measurement, 7);
            assert_eq!(got.score.value, value);
            assert_eq!((got.rows_checked, got.rows_failed), (checked, failed));
            assert_eq!(got.query_digest.len(), 64);
        }
        assert!(matches!(compile_rule(&sql), Ok(CompiledRule::Sql { .. })));
        assert!(matches!(
            compile_rule(&shape),
            Ok(CompiledRule::Shape { .. })
        ));
        assert_eq!(
            compile_rule(&fresh),
            Ok(CompiledRule::Freshness { max_lag_ms: 5_000 })
        );
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R762.2
    #[test]
    fn three_failures_open_one_incident() {
        let failing = rule(
            QualityDimension::Completeness,
            "app.orders",
            "email IS NOT NULL",
        );
        let compiled = compile_rule(&failing).unwrap();
        let bad = evaluate_rule(
            &failing,
            &compiled,
            Measurement::Rows {
                checked: 10,
                failed: 5,
            },
            1,
        );
        let good = evaluate_rule(
            &failing,
            &compiled,
            Measurement::Rows {
                checked: 10,
                failed: 0,
            },
            2,
        );
        let mut sink = FakeIncidents::default();
        let mut count = 0;
        for _ in 0..2 {
            count = track_failure(count, &failing, &bad, 3, &mut sink);
        }
        assert!(sink.calls.is_empty());
        count = track_failure(count, &failing, &bad, 3, &mut sink);
        assert_eq!((count, sink.open()), (3, 1));
        let call = &sink.calls[0];
        assert_eq!(call.signal, "quality:completeness");
        assert_eq!((call.observed, call.baseline), (0.5, 0.9));
        assert_eq!(track_failure(count, &failing, &good, 3, &mut sink), 0);
    }
}
