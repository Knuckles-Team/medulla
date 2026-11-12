//! The context digest a decision seals and the prior records that share it
//! (MEDULLA-DECISION-ENGINE-R020.1.1): history is read only from the same
//! tenant and the same caller principal, inside the knob window.

use super::super::stat_log::{context_digest_of, matched_trajectories, LogReader, MatchWindow};
use super::*;
use medulla_decision_types::decision::statistical::log::{EntryInputs, RecordVisibility};
use medulla_decision_types::decision::statistical::outcome::StatisticalOutcome;
use medulla_decision_types::decision::{PremiseClass, PremiseProvenance};
use medulla_persistence_records::decision_jobs::{encode_artifact, record_key};

const NOW: u64 = 1_000_000_000_000;
const WINDOW: u64 = 86_400_000;

fn reader(agent: &str, tenant: &str) -> LogReader {
    LogReader::of(&VerifiedRequestContext::verified_for_test_in_tenant(
        agent, tenant,
    ))
}

/// Log `base` as another executed decision of `principal` in `tenant`.
fn write_history(
    h: &Harness,
    tenant: &str,
    base: &StatisticalDecisionRecord,
    id: &str,
    principal: &str,
    context: &str,
    age_ms: u64,
) {
    let mut record = base.clone();
    record.record_id = format!("decision:{id}");
    record.tenant_id = tenant.to_string();
    record.caller_principal = principal.to_string();
    record.created_at_ms = NOW - age_ms;
    record.inputs.context_digest = Some(context.to_string());
    record.outcome = StatisticalOutcome::Explored {
        option_id: "tool-a".to_string(),
        propensity: rational(1, 2),
    };
    let entry = DecisionLogEntry {
        schema_version:
            medulla_decision_types::decision::statistical::log::DECISION_LOG_SCHEMA_VERSION,
        visibility: RecordVisibility::Tenant,
        committed_by: principal.to_string(),
        committed_at_ms: record.created_at_ms,
        record: Box::new(record.clone()),
        inputs: EntryInputs::Inline,
    };
    h.store
        .put_decision_artifacts(
            tenant,
            &[(
                record_key(&record.record_id),
                encode_artifact(&entry).unwrap(),
            )],
        )
        .unwrap();
}

// spec: MEDULLA-DECISION-ENGINE-R020.1.1
#[tokio::test]
async fn only_same_principal_history_matches() {
    let h = Harness::new().await;
    h.publish_test_component(
        "tool-a",
        AgentComponentKind::Tool,
        "web search",
        None::<&()>,
        None,
    )
    .unwrap();
    let schema_pin = h
        .publish_test_component(
            "schema",
            AgentComponentKind::FeatureSchema,
            "features",
            Some(&schema()),
            None,
        )
        .unwrap();
    let asked = request(
        &schema_pin,
        None,
        DecisionPolicyRef::Default,
        QuestionSafety::Ordinary,
    );
    let batch = decide(&h, asked.clone()).await.unwrap();
    let base = batch.records.as_slice()[0].clone();

    // The sealed digest is the question and its name-sorted params.
    let digest = context_digest_of(&asked.question.question_id, asked.params.as_slice());
    assert_eq!(base.inputs.context_digest.as_deref(), Some(digest.as_str()));
    let mut reversed = asked.params.as_slice().to_vec();
    reversed.reverse();
    assert_eq!(
        context_digest_of(&asked.question.question_id, &reversed),
        digest,
        "parameter order never changes the context"
    );
    assert_ne!(
        context_digest_of("route.other", asked.params.as_slice()),
        digest
    );
    let premise = base
        .premises
        .iter()
        .find(|p| p.fact == "context_digest")
        .expect("the context digest is a named premise");
    assert_eq!(premise.subject, digest);
    assert_eq!(premise.class, PremiseClass::Definition);
    assert_eq!(
        premise.provenance,
        PremiseProvenance::Request {
            field: "params".to_string()
        }
    );

    let mine = reader("decider", TENANT);
    let stranger = reader("stranger", TENANT);
    let other_tenant = reader("decider", "tenant-b");
    write_history(
        &h,
        TENANT,
        &base,
        "mine-new",
        &mine.principal,
        &digest,
        1_000,
    );
    write_history(
        &h,
        TENANT,
        &base,
        "mine-old",
        &mine.principal,
        &digest,
        2_000,
    );
    write_history(
        &h,
        TENANT,
        &base,
        "mine-stale",
        &mine.principal,
        &digest,
        WINDOW * 2,
    );
    write_history(
        &h,
        TENANT,
        &base,
        "mine-other-context",
        &mine.principal,
        "sha256:other",
        1_000,
    );
    write_history(
        &h,
        TENANT,
        &base,
        "strangers",
        &stranger.principal,
        &digest,
        1_000,
    );
    write_history(
        &h,
        "tenant-b",
        &base,
        "foreign",
        &other_tenant.principal,
        &digest,
        1_000,
    );

    let window = |max_matched| MatchWindow {
        now_ms: NOW,
        window_ms: WINDOW,
        max_matched,
    };
    let ids = |reader: &LogReader, max| -> Vec<String> {
        matched_trajectories(&h.store, reader, &digest, window(max))
            .unwrap()
            .into_iter()
            .map(|m| m.record_id)
            .collect()
    };
    // Same context, same principal, inside the window: both, with the option.
    assert_eq!(ids(&mine, 256), ["decision:mine-new", "decision:mine-old"]);
    let first = matched_trajectories(&h.store, &mine, &digest, window(256)).unwrap();
    assert!(first.iter().all(|m| m.option_id == "tool-a"));
    // The cap keeps the newest.
    assert_eq!(ids(&mine, 1), ["decision:mine-new"]);
    // A foreign principal never sees mine, and a foreign tenant never sees
    // either tenant-a principal's records.
    assert_eq!(ids(&stranger, 256), ["decision:strangers"]);
    assert_eq!(ids(&other_tenant, 256), ["decision:foreign"]);
}
