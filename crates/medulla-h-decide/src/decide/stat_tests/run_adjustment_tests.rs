//! Run adjustment, served through `Decide` (MEDULLA-DECISION-ENGINE-R158.3):
//! the family counts the errored runs of one partition in the usage ledger
//! and answers the model, width and budget of the next runs. Another tenant's
//! runs are never counted.

use super::*;
use medulla_decision_types::decision::statistical::declared::{DeclaredNumber, DeclaredOption};
use medulla_decision_types::decision::{PremiseClass, PremiseProvenance, PremiseRef};
use medulla_types::usage::{PriceCatalog, UsageOutcome, UsageRecord, USAGE_SCHEMA_VERSION};

const OTHER: &str = "tenant-other";

fn model(id: &str, tier: i64) -> DeclaredOption {
    DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::default(),
        numbers: BoundedVec::new(vec![DeclaredNumber {
            key: "tier".to_string(),
            q32: tier << 32,
        }])
        .unwrap(),
        texts: BoundedVec::default(),
    }
}

/// `runs` runs of `child` in `tenant`, the first `errored` of them errored.
fn seed(h: &Harness, tenant: &str, child: &str, runs: u64, errored: u64) {
    seed_at(h, tenant, child, runs, errored, 20, 0);
}

/// As `seed`, each call taking `latency_ms`, the newest run recorded
/// `age_ms` before now.
fn seed_at(
    h: &Harness,
    tenant: &str,
    child: &str,
    runs: u64,
    errored: u64,
    latency_ms: u64,
    age_ms: u64,
) {
    let base = medulla_kernel::kernel::dispatch_support::authoritative_now_ms()
        .saturating_sub(age_ms + 1_000);
    let records: Vec<UsageRecord> = (0..runs)
        .map(|i| UsageRecord {
            schema_version: USAGE_SCHEMA_VERSION,
            recorded_at_ms: base + i,
            child_id: child.to_string(),
            operation: "decide".to_string(),
            tenant: tenant.to_string(),
            principal: "p1".to_string(),
            run_id: format!("{child}-run-{i}"),
            model_key: "m1".to_string(),
            tokens_in: 10,
            tokens_out: 10,
            tokens_cached: 0,
            latency_ms,
            outcome: if i < errored {
                UsageOutcome::Error
            } else {
                UsageOutcome::Ok
            },
            content_digest: "a".repeat(64),
        })
        .collect();
    let catalog = PriceCatalog {
        aliases: std::collections::BTreeMap::new(),
        entries: Vec::new(),
    };
    h.store
        .append_usage(tenant, "p1", &catalog, &records, base + 2_000)
        .unwrap();
}

fn adjustment(
    pin: &ComponentDependency,
    child: &str,
    current_model: &str,
    width: i64,
) -> DecideRequest {
    adjustment_in(pin, child, current_model, width, false)
}

fn adjustment_in(
    pin: &ComponentDependency,
    child: &str,
    current_model: &str,
    width: i64,
    mid_run: bool,
) -> DecideRequest {
    let mut request = declared_request(
        pin,
        TENANT,
        QuestionKind::RunAdjustment,
        "m1",
        DecisionPolicyRef::Default,
        vec![
            TypedParam {
                name: "child_id".to_string(),
                value: TypedValue::Text(child.to_string()),
            },
            TypedParam {
                name: "operation".to_string(),
                value: TypedValue::Text("decide".to_string()),
            },
            TypedParam {
                name: "current_model".to_string(),
                value: TypedValue::Iri(current_model.to_string()),
            },
            TypedParam {
                name: "current_width".to_string(),
                value: TypedValue::Int(width),
            },
            TypedParam {
                name: "host_width_ceiling".to_string(),
                value: TypedValue::Int(8),
            },
            TypedParam {
                name: "budget_tokens".to_string(),
                value: TypedValue::Int(1_000),
            },
            TypedParam {
                name: "mid_run".to_string(),
                value: TypedValue::Bool(mid_run),
            },
        ],
    );
    request.candidates = CandidateSource::Declared {
        options: BoundedVec::new(vec![model("m1", 1), model("m2", 2), model("m3", 3)]).unwrap(),
    };
    request
}

fn options(batch: &DecisionBatch) -> Vec<String> {
    let StatisticalOutcome::Advisory { scores, .. } = &batch.records.as_slice()[0].outcome else {
        panic!(
            "an advisory adjustment, got {:?}",
            batch.records.as_slice()[0].outcome
        )
    };
    let mut ids: Vec<String> = scores.iter().map(|s| s.option_id.clone()).collect();
    ids.sort();
    ids
}

// spec: MEDULLA-DECISION-ENGINE-R158.3
#[tokio::test]
async fn per_run_adjustment_routes() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    seed(&h, TENANT, "noisy", 20, 5);
    seed(&h, TENANT, "quiet", 20, 0);
    seed(&h, TENANT, "few", 3, 3);

    // 5 of 20 runs errored: the next tier up and one less width.
    let high = decide(&h, adjustment(&pin, "noisy", "m1", 4))
        .await
        .unwrap();
    assert_eq!(
        options(&high),
        ["budget_tokens:1000", "model:m2", "width:3"]
    );
    // At the top tier the model stays and the width still drops; width 1 stays 1.
    let top = decide(&h, adjustment(&pin, "noisy", "m3", 1))
        .await
        .unwrap();
    assert_eq!(options(&top), ["budget_tokens:1000", "model:m3", "width:1"]);
    // No errored run: the model stays and the width grows, up to the ceiling.
    let low = decide(&h, adjustment(&pin, "quiet", "m2", 4))
        .await
        .unwrap();
    assert_eq!(options(&low), ["budget_tokens:1000", "model:m2", "width:5"]);
    let ceiling = decide(&h, adjustment(&pin, "quiet", "m2", 8))
        .await
        .unwrap();
    assert_eq!(
        options(&ceiling),
        ["budget_tokens:1000", "model:m2", "width:8"]
    );
    // Fewer runs than the minimum abstain with the registered code.
    let few = decide(&h, adjustment(&pin, "few", "m1", 4)).await.unwrap();
    let StatisticalOutcome::Abstained { reasons } = &few.records.as_slice()[0].outcome else {
        panic!("INSUFFICIENT_SAMPLES abstains")
    };
    assert!(
        format!("{reasons:?}").contains("INSUFFICIENT_SAMPLES"),
        "{reasons:?}"
    );
}

// spec: MEDULLA-DECISION-ENGINE-R158.3
#[tokio::test]
async fn other_tenant_runs_are_not_counted() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    seed(&h, TENANT, "shared", 10, 0);
    // The other tenant's runs of the same child and operation all errored.
    seed(&h, OTHER, "shared", 10, 10);
    let batch = decide(&h, adjustment(&pin, "shared", "m1", 4))
        .await
        .unwrap();
    // Counting them would read a 50% error rate and move the model up.
    assert_eq!(
        options(&batch),
        ["budget_tokens:1000", "model:m1", "width:5"]
    );
    // A child only the other tenant ran has no runs here: it abstains.
    seed(&h, OTHER, "foreign-only", 20, 20);
    let none = decide(&h, adjustment(&pin, "foreign-only", "m1", 4))
        .await
        .unwrap();
    assert!(matches!(
        none.records.as_slice()[0].outcome,
        StatisticalOutcome::Abstained { .. }
    ));
}

fn abstention_text(batch: &DecisionBatch) -> String {
    let StatisticalOutcome::Abstained { reasons } = &batch.records.as_slice()[0].outcome else {
        panic!(
            "an abstention, got {:?}",
            batch.records.as_slice()[0].outcome
        )
    };
    format!("{reasons:?}")
}

// spec: MEDULLA-DECISION-ENGINE-R158.7
#[tokio::test]
async fn windows_drive_model_and_width_and_refuse_mid_run_widening() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    // Quiet and fast: the width grows. Quiet and slow (p95 above the 2000 ms
    // cap): the width drops instead. Noisy: the model moves up.
    seed_at(&h, TENANT, "fast", 20, 0, 100, 0);
    seed_at(&h, TENANT, "slow", 20, 0, 3_000, 0);
    seed_at(&h, TENANT, "noisy", 20, 5, 100, 0);
    // Runs older than width.window_max_age_s (600 s) are not observed.
    seed_at(&h, TENANT, "stale", 20, 20, 100, 3_600_000);
    // A foreign tenant's slow runs of the same child are not counted.
    seed_at(&h, OTHER, "fast", 20, 0, 9_000, 0);

    let fast = decide(&h, adjustment(&pin, "fast", "m2", 4)).await.unwrap();
    assert_eq!(
        options(&fast),
        ["budget_tokens:1000", "model:m2", "width:5"]
    );
    let slow = decide(&h, adjustment(&pin, "slow", "m2", 4)).await.unwrap();
    assert_eq!(
        options(&slow),
        ["budget_tokens:1000", "model:m2", "width:3"]
    );
    let noisy = decide(&h, adjustment(&pin, "noisy", "m1", 4))
        .await
        .unwrap();
    assert_eq!(
        options(&noisy),
        ["budget_tokens:1000", "model:m2", "width:3"]
    );
    // Only stale runs: nothing is observed, so it abstains for lack of samples.
    let stale = decide(&h, adjustment(&pin, "stale", "m1", 4))
        .await
        .unwrap();
    assert!(abstention_text(&stale).contains("INSUFFICIENT_SAMPLES"));

    // A request made mid-run may narrow or hold, never widen.
    let widen = decide(&h, adjustment_in(&pin, "fast", "m2", 4, true)).await;
    let refusal = widen.expect_err("a mid-run request may not widen");
    assert!(
        format!("{refusal:?}").contains("WIDEN_MID_RUN"),
        "{refusal:?}"
    );
    let narrow = decide(&h, adjustment_in(&pin, "slow", "m2", 4, true))
        .await
        .unwrap();
    assert_eq!(
        options(&narrow),
        ["budget_tokens:1000", "model:m2", "width:3"]
    );
}

/// `request` with the parameter `name` set to `value`.
fn with_param(mut request: DecideRequest, name: &str, value: TypedValue) -> DecideRequest {
    let mut params: Vec<TypedParam> = request.params.iter().cloned().collect();
    params.retain(|p| p.name != name);
    params.push(TypedParam {
        name: name.to_string(),
        value,
    });
    params.sort_by(|a, b| a.name.cmp(&b.name));
    request.params = BoundedVec::new(params).unwrap();
    request
}

/// The refusal a request earns: `PARAMETER_INVALID` naming the registered code.
fn refused_with(result: Result<DecisionBatch, String>, code: &str) {
    let refusal = result.expect_err("the request is refused");
    assert!(
        refusal.contains("PARAMETER_INVALID") && refusal.contains(code),
        "expected PARAMETER_INVALID naming {code}, got {refusal}"
    );
}

// spec: MEDULLA-DECISION-ENGINE-R158.4
#[tokio::test]
async fn invalid_policy_refuses() {
    use medulla_decision_types::decision::knobs::{KnobName, KnobValue};
    use medulla_decision_types::decision::run_adjustment::{
        ADJUSTMENT_POLICY_UNDEFINED, CURRENT_MODEL_NOT_CANDIDATE, ERRORS_UNPARTITIONED,
        HOST_CEILING_EXCEEDED, INSUFFICIENT_SAMPLES,
    };
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    seed(&h, TENANT, "noisy", 20, 5);
    seed(&h, TENANT, "few", 3, 3);

    // An empty partition refuses before any run is read.
    refused_with(
        decide(&h, adjustment(&pin, "", "m1", 4)).await,
        ERRORS_UNPARTITIONED,
    );
    refused_with(
        decide(
            &h,
            with_param(
                adjustment(&pin, "noisy", "m1", 4),
                "operation",
                TypedValue::Text(String::new()),
            ),
        )
        .await,
        ERRORS_UNPARTITIONED,
    );
    // The current width above the host ceiling, and a zero ceiling.
    refused_with(
        decide(&h, adjustment(&pin, "noisy", "m1", 9)).await,
        HOST_CEILING_EXCEEDED,
    );
    refused_with(
        decide(
            &h,
            with_param(
                adjustment(&pin, "noisy", "m1", 4),
                "host_width_ceiling",
                TypedValue::Int(0),
            ),
        )
        .await,
        HOST_CEILING_EXCEEDED,
    );
    // The current model is not among the declared options.
    refused_with(
        decide(&h, adjustment(&pin, "noisy", "m9", 4)).await,
        CURRENT_MODEL_NOT_CANDIDATE,
    );
    // A pinned policy whose low threshold exceeds its high one.
    let mut policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
    policy.knobs.insert(
        KnobName::new("width.error_hi"),
        KnobValue::DecimalMicros(10_000),
    );
    policy.knobs.insert(
        KnobName::new("width.error_lo"),
        KnobValue::DecimalMicros(500_000),
    );
    let mut inverted = adjustment(&pin, "noisy", "m1", 4);
    inverted.policy = DecisionPolicyRef::Pinned {
        component: h.publish_policy("policy-inverted-width", &policy),
    };
    refused_with(decide(&h, inverted).await, ADJUSTMENT_POLICY_UNDEFINED);
    // Too few runs is an abstention, not a refusal.
    let few = decide(&h, adjustment(&pin, "few", "m1", 4)).await.unwrap();
    assert!(abstention_text(&few).contains(INSUFFICIENT_SAMPLES));
}

// spec: MEDULLA-DECISION-ENGINE-R158.5
#[tokio::test]
async fn adjustment_evidence_binds() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    seed(&h, TENANT, "noisy", 20, 5);
    let batch = decide(&h, adjustment(&pin, "noisy", "m1", 4))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    let fact = |prefix: &str| -> Vec<&PremiseRef> {
        record
            .premises
            .iter()
            .filter(|p| p.fact.starts_with(prefix))
            .collect()
    };
    let one = |prefix: &str| -> &PremiseRef {
        let found = fact(prefix);
        assert_eq!(found.len(), 1, "one {prefix} premise, got {found:?}");
        found[0]
    };
    // The counts are Observation premises read from the ledger.
    for (prefix, value) in [
        ("run_adjustment.runs=", "20".to_string()),
        ("run_adjustment.errored_runs=", "5".to_string()),
        // 5 of 20 runs errored: 0.25 in Q32.
        ("run_adjustment.error_rate_q32=", (1u64 << 30).to_string()),
    ] {
        let premise = one(prefix);
        assert_eq!(premise.fact, format!("{prefix}{value}"));
        assert_eq!(premise.class, PremiseClass::Observation);
        assert!(matches!(
            premise.provenance,
            PremiseProvenance::Observation { .. }
        ));
    }
    // The request's partition and current values are Request premises.
    for (prefix, value) in [
        ("run_adjustment.child_id=", "noisy"),
        ("run_adjustment.operation=", "decide"),
        ("run_adjustment.current_model=", "m1"),
        ("run_adjustment.current_width=", "4"),
        ("run_adjustment.host_width_ceiling=", "8"),
        ("run_adjustment.budget_tokens=", "1000"),
    ] {
        let premise = one(prefix);
        assert_eq!(premise.fact, format!("{prefix}{value}"));
        assert!(matches!(
            premise.provenance,
            PremiseProvenance::Request { .. }
        ));
    }
    // The rationale and its digest, and the before and after values.
    assert_eq!(
        one("run_adjustment.rationale=").fact,
        "run_adjustment.rationale=error_high"
    );
    assert_eq!(
        one("run_adjustment.rationale_digest=").fact.len(),
        "run_adjustment.rationale_digest=".len() + 64
    );
    assert_eq!(
        options(&batch),
        ["budget_tokens:1000", "model:m2", "width:3"]
    );
    // The width knobs the decision read are sealed (R179.5).
    let knobs = fact("knob:width.");
    assert_eq!(knobs.len(), 6, "{knobs:?}");
    assert!(knobs
        .iter()
        .any(|p| p.fact == "knob:width.error_hi=0.020000"));
}
