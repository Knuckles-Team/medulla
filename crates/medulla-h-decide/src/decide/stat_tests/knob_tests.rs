//! Evolution-tuned knobs (MEDULLA-DECISION-ENGINE-R179.2, R179.4): a pinned
//! policy's knob values are what a family reads, an absent knob reads its spec
//! default, and a knob-set revision is refused outside the registry's rules.

use std::collections::BTreeMap;

use super::*;
use medulla_decision_types::decision::knobs::{
    check_revision, KnobName, KnobRefusalCode, KnobSafety, KnobSpec, KnobValue,
};

const FAMILY: &[KnobSpec] = &[
    KnobSpec {
        name: "probe.batch",
        unit: "turns",
        default: KnobValue::Integer(8),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(64),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "probe.window",
        unit: "ms",
        default: KnobValue::DurationMs(30_000),
        min: KnobValue::DurationMs(1_000),
        max: KnobValue::DurationMs(600_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "probe.floor",
        unit: "tokens",
        default: KnobValue::Integer(64),
        min: KnobValue::Integer(64),
        max: KnobValue::Integer(64),
        safety: KnobSafety::Fixed,
    },
];

/// Verifies MEDULLA-DECISION-ENGINE-R179.2: Policy carries knob values; families read them.
// spec: MEDULLA-DECISION-ENGINE-R179.2
#[tokio::test]
async fn decide_reads_the_pinned_knob_value_and_falls_back_to_the_default() {
    let h = Harness::new().await;
    let mut policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
    policy
        .knobs
        .insert(KnobName::new("probe.batch"), KnobValue::Integer(16));
    let pin = h.publish_policy("policy-knobs", &policy);
    let pinned = super::super::stat_support::resolve_policy(
        &h.store,
        TENANT,
        &DecisionPolicyRef::Pinned { component: pin },
    )
    .unwrap();
    assert_eq!(pinned.policy.knob(&FAMILY[0]), KnobValue::Integer(16));
    assert_eq!(
        pinned.policy.knob(&FAMILY[1]),
        KnobValue::DurationMs(30_000)
    );
    let default =
        super::super::stat_support::resolve_policy(&h.store, TENANT, &DecisionPolicyRef::Default)
            .unwrap();
    assert_eq!(default.policy.knob(&FAMILY[0]), KnobValue::Integer(8));
}

/// Verifies MEDULLA-DECISION-ENGINE-R179.4: Knob refusals.
// spec: MEDULLA-DECISION-ENGINE-R179.4
#[test]
fn knob_revision_refusals() {
    let revision = |name: &str, value: KnobValue| BTreeMap::from([(KnobName::new(name), value)]);
    assert_eq!(
        check_revision(&[FAMILY], &revision("probe.batch", KnobValue::Integer(32))),
        Ok(())
    );
    for (name, value, code) in [
        (
            "probe.batch",
            KnobValue::Integer(65),
            KnobRefusalCode::KnobOutOfBounds,
        ),
        (
            "probe.window",
            KnobValue::DurationMs(999),
            KnobRefusalCode::KnobOutOfBounds,
        ),
        (
            "probe.batch",
            KnobValue::DurationMs(8),
            KnobRefusalCode::KnobOutOfBounds,
        ),
        (
            "probe.floor",
            KnobValue::Integer(64),
            KnobRefusalCode::KnobNotTunable,
        ),
        (
            "probe.missing",
            KnobValue::Integer(1),
            KnobRefusalCode::KnobUnknown,
        ),
    ] {
        let refused = check_revision(&[FAMILY], &revision(name, value)).unwrap_err();
        assert_eq!(refused, (KnobName::new(name), code));
    }
    assert_eq!(
        KnobRefusalCode::KnobOutOfBounds.code(),
        "KNOB_OUT_OF_BOUNDS"
    );
    assert_eq!(KnobRefusalCode::KnobNotTunable.code(), "KNOB_NOT_TUNABLE");
    assert_eq!(KnobRefusalCode::KnobUnknown.code(), "KNOB_UNKNOWN");
}
