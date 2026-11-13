//! Trust ledger and corrigibility verdict served through `Decide`
//! (MEDULLA-DECISION-ENGINE-R752.755): independent outcomes enter through
//! `DecisionLog` `Evaluate`; a reversible class clears after the clean run, a
//! failure resets it, an irreversible class never clears, and another tenant's
//! outcomes never raise trust.

use super::kernel_family_support::{facts, kernel_request};
use super::path_preference_tests::{pins, run, seed, OTHER};
use super::*;
use medulla_decision_types::decision::autonomy::{
    AutonomyReading, AutonomyRequest, CorrigibilityVerdict, IrreversibilityClass,
};
use medulla_decision_types::decision::ResolutionKind;

async fn ask(
    h: &Harness,
    schema: &ComponentDependency,
    class: &str,
    irreversibility: IrreversibilityClass,
    safety: QuestionSafety,
) -> (StatisticalDecisionRecord, AutonomyReading) {
    let mut request = kernel_request(
        schema,
        QuestionKind::AutonomyVerdict,
        "autonomy_verdict",
        TypedValue::AutonomyVerdict(Box::new(AutonomyRequest { irreversibility })),
        &[class],
    );
    request.question.safety = safety;
    let batch = decide(h, request).await.unwrap();
    let record = batch.records.as_slice()[0].clone();
    let reading = record
        .answer
        .as_ref()
        .and_then(|a| a.autonomy())
        .expect("the family answers a corrigibility reading")
        .clone();
    (record, reading)
}

/// Verifies MEDULLA-DECISION-ENGINE-R752.755: an irreversible class, and a
/// question of safety class security, policy or write-back, always requires
/// confirmation with `ACTION_NEEDS_APPROVAL`, whatever its ledger holds.
// spec: MEDULLA-DECISION-ENGINE-R752.755
#[tokio::test]
async fn served_decide_requires_confirmation_for_irreversible_actions() {
    let h = Harness::new().await;
    let pins = pins(&h, TENANT);
    seed(&h, TENANT, &pins, "db.drop", &run(40, 0)).await;
    let (record, reading) = ask(
        &h,
        &pins.0,
        "db.drop",
        IrreversibilityClass::Irreversible,
        QuestionSafety::Ordinary,
    )
    .await;
    assert_eq!(record.question.kind, QuestionKind::AutonomyVerdict);
    assert_eq!(reading.verdict, CorrigibilityVerdict::RequireConfirmation);
    assert_eq!(reading.code.as_deref(), Some("ACTION_NEEDS_APPROVAL"));
    assert_eq!(
        reading.clean_outcomes, 20,
        "the ledger is read to the clean target"
    );
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    assert_eq!(record.evidence_class, EvidenceClass::Observation);
    let facts = facts(&record);
    assert!(facts.contains(&"autonomy:class=db.drop"), "{facts:?}");
    assert!(facts.contains(&"autonomy:clean=20"), "{facts:?}");
    assert!(
        facts.contains(&"autonomy:verdict=require_confirmation"),
        "{facts:?}"
    );
    for safety in [
        QuestionSafety::Security,
        QuestionSafety::Policy,
        QuestionSafety::WriteBack,
    ] {
        let (_, reading) = ask(
            &h,
            &pins.0,
            "db.drop",
            IrreversibilityClass::Reversible,
            safety,
        )
        .await;
        assert_eq!(reading.verdict, CorrigibilityVerdict::RequireConfirmation);
        assert_eq!(reading.code.as_deref(), Some("ACTION_NEEDS_APPROVAL"));
    }
}

/// Verifies MEDULLA-DECISION-ENGINE-R752.755: a reversible class clears after
/// twenty clean outcomes and a failure resets the count to 0.
// spec: MEDULLA-DECISION-ENGINE-R752.755
#[tokio::test]
async fn trust_clears_after_the_clean_run_and_resets_on_failure() {
    let h = Harness::new().await;
    let pins = pins(&h, TENANT);
    seed(&h, TENANT, &pins, "file.write", &run(20, 0)).await;
    seed(&h, TENANT, &pins, "file.edit", &run(19, 0)).await;
    let mut failed_last = run(30, 0);
    failed_last.push(false);
    seed(&h, TENANT, &pins, "file.move", &failed_last).await;

    let (_, cleared) = ask(
        &h,
        &pins.0,
        "file.write",
        IrreversibilityClass::Reversible,
        QuestionSafety::Ordinary,
    )
    .await;
    assert_eq!(cleared.verdict, CorrigibilityVerdict::Allow);
    assert_eq!((cleared.clean_outcomes, cleared.code), (20, None));

    let (_, short) = ask(
        &h,
        &pins.0,
        "file.edit",
        IrreversibilityClass::Reversible,
        QuestionSafety::Ordinary,
    )
    .await;
    assert_eq!(short.verdict, CorrigibilityVerdict::RequireConfirmation);
    assert_eq!((short.clean_outcomes, short.code), (19, None));

    let (_, reset) = ask(
        &h,
        &pins.0,
        "file.move",
        IrreversibilityClass::Reversible,
        QuestionSafety::Ordinary,
    )
    .await;
    assert_eq!(reset.verdict, CorrigibilityVerdict::RequireConfirmation);
    assert_eq!((reset.clean_outcomes, reset.code), (0, None));
}

/// Verifies MEDULLA-DECISION-ENGINE-R752.755: another tenant's clean run never
/// raises this tenant's trust.
// spec: MEDULLA-DECISION-ENGINE-R752.755
#[tokio::test]
async fn other_tenant_outcomes_never_raise_trust() {
    let h = Harness::new().await;
    let mine = pins(&h, TENANT);
    let theirs = pins(&h, OTHER);
    seed(&h, OTHER, &theirs, "file.write", &run(40, 0)).await;
    let (_, reading) = ask(
        &h,
        &mine.0,
        "file.write",
        IrreversibilityClass::Reversible,
        QuestionSafety::Ordinary,
    )
    .await;
    assert_eq!(reading.verdict, CorrigibilityVerdict::RequireConfirmation);
    assert_eq!(reading.clean_outcomes, 0);
}
