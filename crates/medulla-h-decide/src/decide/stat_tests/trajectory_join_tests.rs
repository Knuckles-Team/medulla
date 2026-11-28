//! Trajectory join and distillation, served through `Decide`
//! (MEDULLA-DECISION-ENGINE-R753.751, R753.754): the strongest trajectory wins,
//! the derivation names every source, and the record rests on the weakest
//! evidence among them.

use super::kernel_family_support::{
    decide_as_other_tenant, facts, kernel_request, persisted_files,
};
use super::*;
use medulla_decision_types::decision::trajectory_verdict::{JoinRequest, TrajectoryEntry};
use medulla_decision_types::decision::ResolutionKind;

pub(super) fn entry(id: &str, evidence: EvidenceClass, score: i64) -> TrajectoryEntry {
    TrajectoryEntry::new(
        id.to_string(),
        evidence,
        QuantisedValue {
            scale: medulla_decision_types::decision::QuantScaleTag::Q32,
            value: score,
        },
        "cd".repeat(32),
    )
    .unwrap()
}

pub(super) fn join(schema: &ComponentDependency, entries: Vec<TrajectoryEntry>) -> DecideRequest {
    let mut ids: Vec<String> = entries.iter().map(|e| e.trajectory_id.clone()).collect();
    ids.sort();
    // A request declares at most `MAX_DECLARED_OPTIONS` candidates; an oversize
    // join declares the first of them and carries every entry.
    let refs: Vec<&str> = ids
        .iter()
        .take(medulla_decision_types::decision::statistical::declared::MAX_DECLARED_OPTIONS)
        .map(String::as_str)
        .collect();
    kernel_request(
        schema,
        QuestionKind::TrajectoryJoin,
        "trajectory_join",
        TypedValue::TrajectoryJoin(Box::new(JoinRequest {
            entries: BoundedVec::new(entries).unwrap(),
        })),
        &refs,
    )
}

/// Verifies MEDULLA-DECISION-ENGINE-R753.751 and R753.754: the served join
/// distills the strongest trajectory, whatever order the entries arrive in.
// spec: MEDULLA-DECISION-ENGINE-R753.751
// spec: MEDULLA-DECISION-ENGINE-R753.754
#[tokio::test]
async fn served_decide_joins_and_distills_trajectories() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let entries = vec![
        entry("t-claim", EvidenceClass::Claim, 900),
        entry("t-obs-b", EvidenceClass::Observation, 40),
        entry("t-obs-a", EvidenceClass::Observation, 40),
        entry("t-proof", EvidenceClass::Proof, 1),
    ];
    let batch = decide(&h, join(&schema, entries.clone())).await.unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::TrajectoryJoin);
    let distilled = record
        .answer
        .as_ref()
        .and_then(|a| a.trajectory_join())
        .expect("the family answers a distilled trajectory");
    assert_eq!(distilled.winner_id, "t-proof");
    assert_eq!(
        distilled.source_ids.as_slice(),
        ["t-claim", "t-obs-a", "t-obs-b", "t-proof"]
    );
    assert_eq!(distilled.derivation_digest.len(), 64);
    assert!(matches!(
        record.outcome,
        StatisticalOutcome::Advisory { .. }
    ));
    assert_eq!(record.resolution_kind, ResolutionKind::Optimization);
    assert_eq!(
        record.evidence_class,
        EvidenceClass::Claim,
        "one claim source makes the record a claim"
    );

    // The entries' order does not change the answer.
    let mut reversed = entries.clone();
    reversed.reverse();
    let again = decide(&h, join(&schema, reversed)).await.unwrap();
    assert_eq!(
        again.records.as_slice()[0].answer,
        record.answer,
        "the join ignores input order"
    );

    // Without a claim among the sources the evidence is the weakest of them.
    let strong = decide(
        &h,
        join(
            &schema,
            vec![
                entry("a", EvidenceClass::Proof, 3),
                entry("b", EvidenceClass::Proof, 9),
            ],
        ),
    )
    .await
    .unwrap();
    let strong = &strong.records.as_slice()[0];
    assert_eq!(strong.evidence_class, EvidenceClass::Proof);
    assert_eq!(
        strong
            .answer
            .as_ref()
            .unwrap()
            .trajectory_join()
            .unwrap()
            .winner_id,
        "b"
    );
    let mixed = decide(
        &h,
        join(
            &schema,
            vec![
                entry("a", EvidenceClass::Proof, 3),
                entry("b", EvidenceClass::Observation, 9),
            ],
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        mixed.records.as_slice()[0].evidence_class,
        EvidenceClass::Observation
    );

    // The hot path (R753.753): a 64-trajectory join through the same served
    // call, every source named in id order and the strongest one winning.
    let many: Vec<TrajectoryEntry> = (0..64_i64)
        .map(|n| {
            let evidence = if n == 37 {
                EvidenceClass::Proof
            } else {
                EvidenceClass::Observation
            };
            entry(&format!("t{n:03}"), evidence, (n * 7919) % 1000)
        })
        .collect();
    let wide = decide(&h, join(&schema, many)).await.unwrap();
    let wide = wide.records.as_slice()[0]
        .answer
        .as_ref()
        .and_then(|a| a.trajectory_join())
        .expect("the family answers a distilled trajectory over 64 sources");
    assert_eq!(wide.winner_id, "t037");
    assert_eq!(wide.source_ids.as_slice().len(), 64);
    assert!(wide.source_ids.as_slice().windows(2).all(|w| w[0] < w[1]));
    assert_eq!(wide.derivation_digest.len(), 64);
}

/// Verifies MEDULLA-DECISION-ENGINE-R753.754: the derivation names every source
/// in id order, the premises carry the winner, and the candidates must be the
/// sorted trajectory ids.
// spec: MEDULLA-DECISION-ENGINE-R753.754
#[tokio::test]
async fn derivation_names_every_source() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let batch = decide(
        &h,
        join(
            &schema,
            vec![
                entry("zeta", EvidenceClass::Observation, 5),
                entry("alpha", EvidenceClass::Observation, 7),
            ],
        ),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    let facts = facts(record);
    let sources: Vec<&&str> = facts
        .iter()
        .filter(|f| f.starts_with("join:source="))
        .collect();
    assert_eq!(sources.len(), 2);
    assert!(sources[0].starts_with("join:source=alpha/observation/"));
    assert!(sources[1].starts_with("join:source=zeta/observation/"));
    assert!(facts.contains(&"join:winner=alpha"));
    let digest = &record
        .answer
        .as_ref()
        .and_then(|a| a.trajectory_join())
        .unwrap()
        .derivation_digest;
    assert!(facts.contains(&format!("join:derivation={digest}").as_str()));

    // Candidates other than the trajectory ids are refused.
    let mut request = join(&schema, vec![entry("a", EvidenceClass::Claim, 1)]);
    request.candidates = kernel_request(
        &schema,
        QuestionKind::TrajectoryJoin,
        "trajectory_join",
        TypedValue::Int(0),
        &["not-a"],
    )
    .candidates;
    let error = decide(&h, request).await.unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");
}

/// Verifies MEDULLA-DECISION-ENGINE-R753.754: a request for another tenant is
/// refused before anything is read.
// spec: MEDULLA-DECISION-ENGINE-R753.754
#[tokio::test]
async fn other_tenant_trajectories_are_not_read() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let refused =
        decide_as_other_tenant(&h, join(&schema, vec![entry("a", EvidenceClass::Claim, 1)])).await;
    assert!(refused.starts_with("ACCESS_DENIED"), "{refused}");
}

/// Verifies MEDULLA-DECISION-ENGINE-R753.752 (join half): a join over no
/// trajectory is `JOIN_EMPTY_SET`, one over more than 64 is the shared
/// `CANDIDATE_SET_TOO_LARGE`, both codes are registered, and nothing is written.
// spec: MEDULLA-DECISION-ENGINE-R753.752
#[tokio::test]
async fn join_refusals() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let dir = std::path::PathBuf::from(h.state.read().await.persist_dir.clone().unwrap());
    decide(&h, join(&schema, vec![entry("a", EvidenceClass::Claim, 1)]))
        .await
        .unwrap();
    let before = persisted_files(&dir);

    let error = decide(&h, join(&schema, Vec::new())).await.unwrap_err();
    assert!(error.starts_with("JOIN_EMPTY_SET"), "{error}");

    let sixty_four: Vec<TrajectoryEntry> = (0..64)
        .map(|i| entry(&format!("t{i:03}"), EvidenceClass::Claim, i))
        .collect();
    decide(&h, join(&schema, sixty_four.clone())).await.unwrap();
    let before_large = persisted_files(&dir);
    let mut sixty_five = sixty_four;
    sixty_five.push(entry("t064", EvidenceClass::Claim, 64));
    let error = decide(&h, join(&schema, sixty_five)).await.unwrap_err();
    assert!(error.starts_with("CANDIDATE_SET_TOO_LARGE"), "{error}");
    assert_eq!(
        persisted_files(&dir),
        before_large,
        "a refusal writes nothing"
    );
    assert!(before_large.len() >= before.len());

    for code in medulla_decision_types::decision::trajectory_verdict::JOIN_REFUSALS {
        assert_eq!(
            medulla_decision_types::decision::code_registry::resolve(code.as_str()),
            Some(*code)
        );
    }
}
