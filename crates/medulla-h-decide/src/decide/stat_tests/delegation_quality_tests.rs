//! The delegation quality gate through the actual `DecisionEval` handler
//! (MEDULLA-DECISION-ENGINE-R157.3, R157.4): a window of observations is
//! judged, the admission is sealed as the scope's job artifact, a later window
//! replaces it by compare-and-set on `end_ms`, and every refusal returns
//! before a write.

use super::*;
use medulla_decision_types::decision::delegation_quality::{
    artifact_key, DelegationQualityOutcome, DelegationQualityRequest, DelegationQualityWindow,
    QualityObservation, QualityThreshold, StoredDelegation, REFUSALS,
};
use medulla_decision_types::decision::knobs::{KnobName, KnobValue};
use medulla_decision_types::decision::statistical::StatisticalErrorCode;
use medulla_persistence_records::decision_jobs::decode_artifact;
use medulla_types::contract::Digest256;

const HOUR_MS: u64 = 3_600_000;

fn now_ms() -> u64 {
    medulla_kernel::kernel::dispatch_support::authoritative_now_ms()
}

fn observation(id: &str, score: u32, observed_at_ms: u64) -> QualityObservation {
    QualityObservation {
        observation_id: id.to_string(),
        score,
        observed_at_ms,
        evidence_digest: Digest256::sha256(id.as_bytes()),
    }
}

/// A request for `scope` over `[end - 2h, end]` with one observation per
/// score, spread inside the window, judged against a `task_success`
/// threshold of `minimum` micro-units over at least three observations.
fn quality_request(
    tenant: &str,
    scope: &str,
    end_ms: u64,
    scores: &[u32],
    minimum: u32,
    policy: DecisionPolicyRef,
) -> DelegationQualityRequest {
    let start_ms = end_ms - 2 * HOUR_MS;
    let observations: Vec<QualityObservation> = scores
        .iter()
        .enumerate()
        .map(|(n, score)| {
            observation(
                &format!("{scope}-{end_ms}-{n}"),
                *score,
                start_ms + 10 + n as u64,
            )
        })
        .collect();
    DelegationQualityRequest {
        tenant_id: tenant.to_string(),
        scope: scope.to_string(),
        policy,
        window: DelegationQualityWindow {
            start_ms,
            end_ms,
            observation_count: observations.len() as u32,
        },
        observations: BoundedVec::new(observations).unwrap(),
        threshold: Some(QualityThreshold {
            metric_name: "task_success".to_string(),
            minimum_value: minimum,
            min_observations: 3,
        }),
    }
}

async fn submit(
    h: &Harness,
    request: DelegationQualityRequest,
) -> Result<DelegationQualityOutcome, String> {
    decode(
        super::super::jobs::handle_decision_eval(
            &h.state,
            7,
            &verified(),
            DecisionEvalOp::DelegationQuality {
                request: Box::new(request),
            },
        )
        .await,
    )
}

fn stored_bytes(h: &Harness, tenant: &str, scope: &str) -> Option<Vec<u8>> {
    h.store
        .decision_artifact(tenant, &artifact_key(scope))
        .unwrap()
}

fn stored(h: &Harness, scope: &str) -> StoredDelegation {
    decode_artifact(
        &stored_bytes(h, TENANT, scope).expect("the sealed artifact"),
        "delegation quality artifact",
    )
    .unwrap()
}

fn knob_policy(h: &Harness, id: &str, knob: &str, value: i64) -> DecisionPolicyRef {
    let mut policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
    policy
        .knobs
        .insert(KnobName::new(knob), KnobValue::Integer(value));
    DecisionPolicyRef::Pinned {
        component: h.publish_policy(id, &policy),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R157.3
#[tokio::test]
async fn quality_gate_routes() {
    let h = Harness::new().await;
    let end = now_ms() - 1_000;
    let default = DecisionPolicyRef::Default;

    // An admit: the mean of 900000, 800000 and 850000 is 850000, at or above
    // the threshold. The sealed artifact is the answer, by digest.
    let admit = submit(
        &h,
        quality_request(
            TENANT,
            "agent-a",
            end,
            &[900_000, 800_000, 850_000],
            800_000,
            default.clone(),
        ),
    )
    .await
    .unwrap();
    assert!(admit.replaced);
    assert!(admit.stored.admission.admitted);
    assert_eq!(admit.stored.admission.quality_value, 850_000);
    assert_eq!(admit.stored.window.end_ms, end);
    let bytes = stored_bytes(&h, TENANT, "agent-a").expect("the admission is sealed");
    assert_eq!(stored(&h, "agent-a"), admit.stored);
    assert_eq!(admit.artifact_digest, Digest256::sha256(&bytes));

    // A refuse: below the threshold is a sealed, unadmitted artifact with the
    // stable reason, not an absent one.
    let refuse = submit(
        &h,
        quality_request(
            TENANT,
            "agent-b",
            end,
            &[500_000, 600_000, 700_000],
            800_000,
            default.clone(),
        ),
    )
    .await
    .unwrap();
    assert!(!refuse.stored.admission.admitted);
    assert_eq!(refuse.stored.admission.reason, "DELEGATION_BELOW_THRESHOLD");
    assert_eq!(refuse.stored.admission.quality_value, 600_000);
    assert_eq!(stored(&h, "agent-b"), refuse.stored);
    assert_ne!(
        refuse.artifact_digest, admit.artifact_digest,
        "each scope seals its own artifact"
    );

    // Compare-and-set on end_ms: a window ending later replaces the artifact.
    let later = submit(
        &h,
        quality_request(
            TENANT,
            "agent-a",
            end + 500,
            &[100_000, 100_000, 100_000],
            800_000,
            default.clone(),
        ),
    )
    .await
    .unwrap();
    assert!(later.replaced);
    assert!(!later.stored.admission.admitted);
    assert_eq!(later.stored.window.end_ms, end + 500);
    assert_eq!(stored(&h, "agent-a"), later.stored);

    // A window ending before the stored one does not replace it, and the
    // answer is the stored artifact, byte for byte.
    let before = stored_bytes(&h, TENANT, "agent-a").unwrap();
    let older = submit(
        &h,
        quality_request(
            TENANT,
            "agent-a",
            end,
            &[900_000, 900_000, 900_000],
            800_000,
            default.clone(),
        ),
    )
    .await
    .unwrap();
    assert!(!older.replaced);
    assert_eq!(older.stored, later.stored);
    assert_eq!(older.artifact_digest, later.artifact_digest);
    assert_eq!(stored_bytes(&h, TENANT, "agent-a").unwrap(), before);

    // The same window again is a replay: it replaces with identical bytes.
    let replay = submit(
        &h,
        quality_request(
            TENANT,
            "agent-a",
            end + 500,
            &[100_000, 100_000, 100_000],
            800_000,
            default.clone(),
        ),
    )
    .await
    .unwrap();
    assert!(replay.replaced);
    assert_eq!(stored_bytes(&h, TENANT, "agent-a").unwrap(), before);

    // With no threshold the pinned knobs decide: 20 observations are needed
    // and 800000 is the bar.
    let mut by_knobs = quality_request(TENANT, "agent-c", end, &[850_000; 20], 0, default.clone());
    by_knobs.threshold = None;
    let admitted = submit(&h, by_knobs.clone()).await.unwrap();
    assert!(admitted.stored.admission.admitted);
    let strict = knob_policy(
        &h,
        "policy-delegation-bar",
        "delegation.quality_min",
        900_000,
    );
    by_knobs.policy = strict;
    by_knobs.scope = "agent-d".to_string();
    assert!(
        !submit(&h, by_knobs)
            .await
            .unwrap()
            .stored
            .admission
            .admitted
    );

    // Another tenant's request is refused and writes nothing; the sealed
    // artifact of this tenant is not visible under the other one.
    let foreign = submit(
        &h,
        quality_request(
            "tenant-b",
            "agent-a",
            end,
            &[900_000, 900_000, 900_000],
            800_000,
            default,
        ),
    )
    .await
    .unwrap_err();
    assert!(foreign.starts_with("ACCESS_DENIED"), "{foreign}");
    assert_eq!(stored_bytes(&h, "tenant-b", "agent-a"), None);
}

// spec: MEDULLA-DECISION-ENGINE-R157.4
#[tokio::test]
async fn stale_window_refuses() {
    let h = Harness::new().await;
    let now = now_ms();
    let default = DecisionPolicyRef::Default;
    let good = [900_000, 900_000, 900_000];

    // Every refusal code is registered and is also a request refusal code.
    for code in REFUSALS {
        assert!(
            medulla_decision_types::decision::codes::resolve(code.as_str()).is_some(),
            "{code:?} is registered"
        );
        assert!(
            StatisticalErrorCode::ALL
                .iter()
                .any(|known| known.as_str() == code.as_str()),
            "{code:?} is a statistical refusal code"
        );
    }

    // A sealed artifact must survive every refusal below, byte for byte.
    submit(
        &h,
        quality_request(
            TENANT,
            "agent-a",
            now - 1_000,
            &good,
            800_000,
            default.clone(),
        ),
    )
    .await
    .unwrap();
    let sealed = stored_bytes(&h, TENANT, "agent-a").unwrap();

    // DELEGATION_WINDOW_STALE: the window ended more than a day ago.
    let stale = quality_request(
        TENANT,
        "agent-a",
        now - 86_400_000 - 10_000,
        &good,
        800_000,
        default.clone(),
    );
    let refused = submit(&h, stale.clone()).await.unwrap_err();
    assert!(refused.starts_with("DELEGATION_WINDOW_STALE"), "{refused}");
    // The age bound is the pinned policy's knob, not a default.
    let tight = knob_policy(&h, "policy-delegation-age", "delegation.window_max_age", 60);
    let mut recent = quality_request(TENANT, "agent-e", now - 10 * 60_000, &good, 800_000, tight);
    let refused = submit(&h, recent.clone()).await.unwrap_err();
    assert!(refused.starts_with("DELEGATION_WINDOW_STALE"), "{refused}");
    recent.policy = default.clone();
    submit(&h, recent).await.unwrap();

    // DELEGATION_WINDOW_UNSTABLE: shorter than the minimum window (1 h).
    let mut short = quality_request(
        TENANT,
        "agent-f",
        now - 1_000,
        &good,
        800_000,
        default.clone(),
    );
    short.window.start_ms = short.window.end_ms - 1_000_000;
    short.observations = BoundedVec::new(
        short
            .observations
            .iter()
            .map(|o| QualityObservation {
                observed_at_ms: short.window.start_ms + 10,
                ..o.clone()
            })
            .collect(),
    )
    .unwrap();
    let refused = submit(&h, short).await.unwrap_err();
    assert!(
        refused.starts_with("DELEGATION_WINDOW_UNSTABLE"),
        "{refused}"
    );

    // DELEGATION_THRESHOLD_UNDEFINED: an unknown metric, a value past one
    // million.
    let mut unknown = quality_request(
        TENANT,
        "agent-g",
        now - 1_000,
        &good,
        800_000,
        default.clone(),
    );
    unknown.threshold.as_mut().unwrap().metric_name = "vibes".to_string();
    let refused = submit(&h, unknown).await.unwrap_err();
    assert!(
        refused.starts_with("DELEGATION_THRESHOLD_UNDEFINED"),
        "{refused}"
    );
    let high = quality_request(
        TENANT,
        "agent-g",
        now - 1_000,
        &good,
        1_000_001,
        default.clone(),
    );
    let refused = submit(&h, high).await.unwrap_err();
    assert!(
        refused.starts_with("DELEGATION_THRESHOLD_UNDEFINED"),
        "{refused}"
    );

    // DELEGATION_INSUFFICIENT_SAMPLES: two observations against a minimum of
    // three; an observation outside the window does not count.
    let two = quality_request(
        TENANT,
        "agent-h",
        now - 1_000,
        &good[..2],
        800_000,
        default.clone(),
    );
    let refused = submit(&h, two).await.unwrap_err();
    assert!(
        refused.starts_with("DELEGATION_INSUFFICIENT_SAMPLES"),
        "{refused}"
    );
    let mut outside = quality_request(
        TENANT,
        "agent-h",
        now - 1_000,
        &good,
        800_000,
        default.clone(),
    );
    let mut moved: Vec<QualityObservation> = outside.observations.iter().cloned().collect();
    moved[0].observed_at_ms = 1;
    outside.observations = BoundedVec::new(moved).unwrap();
    let refused = submit(&h, outside).await.unwrap_err();
    assert!(
        refused.starts_with("DELEGATION_INSUFFICIENT_SAMPLES"),
        "{refused}"
    );

    // A refusal stores no artifact and leaves the sealed one untouched.
    for scope in ["agent-f", "agent-g", "agent-h"] {
        assert_eq!(stored_bytes(&h, TENANT, scope), None, "{scope}");
    }
    assert_eq!(stored_bytes(&h, TENANT, "agent-e").is_some(), true);
    let again = submit(&h, stale).await.unwrap_err();
    assert!(again.starts_with("DELEGATION_WINDOW_STALE"), "{again}");
    assert_eq!(stored_bytes(&h, TENANT, "agent-a").unwrap(), sealed);

    // A window cannot end in the future, or it would pin the scope forever.
    let future = quality_request(TENANT, "agent-i", now + 3_600_000, &good, 800_000, default);
    let refused = submit(&h, future).await.unwrap_err();
    assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");
    assert_eq!(stored_bytes(&h, TENANT, "agent-i"), None);
}
