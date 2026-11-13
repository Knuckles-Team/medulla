//! The changeset review impact filter (MEDULLA-DECISION-ENGINE-R138.101.1).
//!
//! Pure: a finding is filtered only when its severity is `low`, it carries
//! false-positive evidence, and its confidence is below
//! `review.filter_confidence_max`. A `medium`, `high` or `critical` finding is
//! never filtered. Every finding keeps a disposition; nothing is dropped.

use medulla_decision_types::decision::changeset_review::{
    FindingDisposition, ReviewFinding, Severity,
};
use medulla_decision_types::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use medulla_decision_types::decision::DecisionPolicy;

/// The `review.*` knobs. The blocking severity is `Fixed`.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "review.filter_confidence_max",
        unit: "per mille",
        default: KnobValue::Integer(300),
        min: KnobValue::Integer(0),
        max: KnobValue::Integer(1000),
        safety: KnobSafety::Tunable,
    },
    // The severity rank (`low` 0 to `critical` 3) at which a finding blocks.
    KnobSpec {
        name: "review.block_severity",
        unit: "severity rank",
        default: KnobValue::Integer(2),
        min: KnobValue::Integer(2),
        max: KnobValue::Integer(2),
        safety: KnobSafety::Fixed,
    },
];

/// The confidence per mille below which a doubted low finding is filtered.
pub fn filter_confidence_max(policy: &DecisionPolicy) -> u16 {
    match policy.knob(&KNOBS[0]) {
        KnobValue::Integer(v) => u16::try_from(v).unwrap_or(300).min(1000),
        _ => 300,
    }
}

/// The severity at which a finding blocks.
pub fn block_severity(policy: &DecisionPolicy) -> Severity {
    match policy.knob(&KNOBS[1]) {
        KnobValue::Integer(v) => Severity::from_rank(v).unwrap_or(Severity::High),
        _ => Severity::High,
    }
}

/// Whether `finding` is filtered under `confidence_max`.
pub fn is_filtered(finding: &ReviewFinding, confidence_max: u16) -> bool {
    finding.severity == Severity::Low
        && finding
            .false_positive_evidence
            .as_deref()
            .is_some_and(|evidence| !evidence.trim().is_empty())
        && finding.confidence_permille < confidence_max
}

/// The disposition of every finding, in order.
pub fn filter_findings<'a>(
    findings: &'a [ReviewFinding],
    policy: &DecisionPolicy,
) -> Vec<(&'a ReviewFinding, FindingDisposition)> {
    let confidence_max = filter_confidence_max(policy);
    findings
        .iter()
        .map(|finding| {
            let disposition = if is_filtered(finding, confidence_max) {
                FindingDisposition::FilteredLowImpact
            } else {
                FindingDisposition::Kept
            };
            (finding, disposition)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::changeset_review::ReviewClass;
    use medulla_decision_types::decision::knobs::KnobName;

    fn finding(severity: Severity, confidence: u16, doubt: Option<&str>) -> ReviewFinding {
        ReviewFinding {
            class: ReviewClass::Injection,
            path: "src/a.rs".to_string(),
            line_start: 3,
            line_end: 5,
            evidence: "string concatenated into a query".to_string(),
            severity,
            confidence_permille: confidence,
            false_positive_evidence: doubt.map(str::to_string),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R138.101.1
    #[test]
    fn only_evidenced_low_impact_findings_filter() {
        let policy = DecisionPolicy::engine_default();
        let findings = vec![
            finding(Severity::Low, 100, Some("test fixture")),
            finding(Severity::Low, 100, None),
            finding(Severity::Low, 100, Some("  ")),
            finding(Severity::Low, 300, Some("test fixture")),
            finding(Severity::Low, 299, Some("test fixture")),
            finding(Severity::Medium, 0, Some("test fixture")),
            finding(Severity::High, 0, Some("test fixture")),
            finding(Severity::Critical, 0, Some("test fixture")),
        ];
        let kept = FindingDisposition::Kept;
        let dropped = FindingDisposition::FilteredLowImpact;
        let dispositions: Vec<_> = filter_findings(&findings, &policy)
            .into_iter()
            .map(|(_, d)| d)
            .collect();
        assert_eq!(
            dispositions,
            vec![dropped, kept, kept, kept, dropped, kept, kept, kept]
        );
        // Every finding keeps a disposition.
        assert_eq!(dispositions.len(), findings.len());

        // The confidence ceiling is the pinned knob.
        let mut strict = DecisionPolicy::engine_default();
        strict.knobs.insert(
            KnobName::new("review.filter_confidence_max"),
            KnobValue::Integer(50),
        );
        let strict_dispositions: Vec<_> = filter_findings(&findings, &strict)
            .into_iter()
            .map(|(_, d)| d)
            .collect();
        assert!(strict_dispositions.iter().all(|d| *d == kept));
    }

    // spec: MEDULLA-DECISION-ENGINE-R138.101.1
    #[test]
    fn the_block_severity_is_a_fixed_knob_and_findings_validate() {
        assert_eq!(KNOBS[1].safety, KnobSafety::Fixed);
        assert_eq!(
            block_severity(&DecisionPolicy::engine_default()),
            Severity::High
        );
        assert_eq!(
            filter_confidence_max(&DecisionPolicy::engine_default()),
            300
        );
        assert!(finding(Severity::Low, 0, None).validate().is_ok());
        let mut bad = finding(Severity::Low, 0, None);
        bad.line_start = 0;
        assert!(bad.validate().is_err());
        bad = finding(Severity::Low, 1001, None);
        assert!(bad.validate().is_err());
        bad = finding(Severity::Low, 1, None);
        bad.evidence = " ".to_string();
        assert!(bad.validate().is_err());
        let json = serde_json::to_string(&finding(Severity::High, 900, None)).unwrap();
        assert!(json.contains("\"class\":\"injection\""));
        assert!(json.contains("\"severity\":\"high\""));
        for (class, name) in [
            (ReviewClass::AuthBypass, "auth_bypass"),
            (ReviewClass::DataExposure, "data_exposure"),
            (ReviewClass::CryptoFlaw, "crypto_flaw"),
            (ReviewClass::BusinessLogic, "business_logic"),
        ] {
            assert_eq!(
                serde_json::to_string(&class).unwrap(),
                format!("\"{name}\"")
            );
        }
        assert!(Severity::Low < Severity::Medium && Severity::High < Severity::Critical);
    }
}
