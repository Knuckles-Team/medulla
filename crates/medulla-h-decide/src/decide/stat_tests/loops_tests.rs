//! Served loop tests (MEDULLA-DECISION-ENGINE-R653.2 .. .8): the lease and
//! epoch fence, the gap market, the ledger and ratchet, refusals that change
//! no state, and a read through a second principal, all through the handler.

use super::*;
use crate::decide::loops::handle_loops;
use crate::decide::loops::served::LoopStore;
use medulla_statechart::loops::LoopKnobs;
use medulla_statechart::Context;
use medulla_store::loop_statechart::loop_record_statechart_def;
use medulla_types::loops::{GapState, ImprovementSample, LoopOp, LoopView, RatchetVerdict};

const OTHER_TENANT: &str = "tenant-loops-other";

async fn call(h: &Harness, who: &str, tenant: &str, op: LoopOp) -> Result<LoopView, String> {
    let verified = VerifiedRequestContext::verified_for_test_in_tenant(who, tenant);
    decode(handle_loops(&h.state, 3, &verified, op).await)
}

async fn writer(h: &Harness, op: LoopOp) -> Result<LoopView, String> {
    call(h, "loop-writer", TENANT, op).await
}

fn claim(loop_id: &str) -> LoopOp {
    LoopOp::Claim {
        loop_id: loop_id.into(),
        objective_digest: "sha256:objective".into(),
        priority: 3,
    }
}

fn get(loop_id: &str) -> LoopOp {
    LoopOp::Get {
        loop_id: loop_id.into(),
    }
}

fn file_gap(loop_id: &str, epoch: u64, gap_id: &str, severity_ppm: u32) -> LoopOp {
    LoopOp::FileGap {
        loop_id: loop_id.into(),
        epoch,
        gap_id: gap_id.into(),
        severity_ppm,
        evidence: vec![format!("run:{gap_id}")],
    }
}

fn gap_op(kind: &str, loop_id: &str, epoch: u64, gap_id: &str) -> LoopOp {
    let (loop_id, gap_id) = (loop_id.to_string(), gap_id.to_string());
    match kind {
        "offer" => LoopOp::Offer {
            loop_id,
            epoch,
            gap_id,
        },
        _ => LoopOp::Settle {
            loop_id,
            epoch,
            gap_id,
        },
    }
}

fn sample(at_ms: u64, value_micro: i64) -> LoopOp {
    LoopOp::RecordSample {
        loop_id: "journey-loop".into(),
        epoch: 1,
        sample: ImprovementSample {
            at_ms,
            metric: "pass_rate".into(),
            value_micro,
        },
    }
}

fn is_refusal(result: Result<LoopView, String>, code: &str) {
    let error = result.expect_err("a refusal");
    assert!(error.starts_with(&format!("{code}:")), "{code} vs {error}");
}

// spec: MEDULLA-DECISION-ENGINE-R653.2
// spec: MEDULLA-DECISION-ENGINE-R653.3
// spec: MEDULLA-DECISION-ENGINE-R653.7
// spec: MEDULLA-DECISION-ENGINE-R653.8
#[tokio::test]
async fn loop_journey() {
    let h = Harness::new().await;
    let id = "journey-loop";

    let claimed = writer(&h, claim(id)).await.unwrap();
    assert_eq!(claimed.instance.state, "running");
    assert_eq!(claimed.instance.tenant_id, TENANT);
    assert_eq!(claimed.instance.priority, 3);
    assert_eq!(claimed.instance.lease.as_ref().unwrap().epoch, 1);
    assert_eq!(
        claimed
            .transitions
            .iter()
            .map(|t| (t.event.as_str(), t.to.as_str()))
            .collect::<Vec<_>>(),
        [("claim", "running")]
    );

    let beaconed = writer(
        &h,
        LoopOp::Beacon {
            loop_id: id.into(),
            epoch: 1,
            stage: "learn".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(beaconed.beacons.len(), 1);

    writer(&h, file_gap(id, 1, "g1", 800_000)).await.unwrap();
    let filed = writer(&h, file_gap(id, 1, "g2", 400_000)).await.unwrap();
    assert!(filed.gaps.iter().all(|g| g.state == GapState::Open));
    // Filing the same gap again is a replay.
    assert_eq!(
        writer(&h, file_gap(id, 1, "g2", 400_000))
            .await
            .unwrap()
            .gaps
            .len(),
        2
    );

    // The first offer meets an empty market; the second meets one open offer.
    let first = writer(&h, gap_op("offer", id, 1, "g1")).await.unwrap();
    assert_eq!(first.offers[0].rate_ppm, 800_000);
    let second = writer(&h, gap_op("offer", id, 1, "g2")).await.unwrap();
    let evidence = &second.offers[1];
    assert_eq!(evidence.open_offers, 1);
    assert_eq!(evidence.saturation_ppm, 125_000);
    assert_eq!(evidence.rate_ppm, 350_000);
    // A repeated offer on an offered gap does not stack.
    assert_eq!(
        writer(&h, gap_op("offer", id, 1, "g2"))
            .await
            .unwrap()
            .offers
            .len(),
        2
    );

    let settled = writer(&h, gap_op("settle", id, 1, "g1")).await.unwrap();
    assert_eq!(
        settled
            .gaps
            .iter()
            .find(|g| g.gap_id == "g1")
            .unwrap()
            .state,
        GapState::Settled
    );

    for (at, value) in [(1, 100_000), (2, 200_000), (3, 300_000)] {
        writer(&h, sample(at, value)).await.unwrap();
    }
    for (at, value) in [(10, 400_000), (11, 500_000), (12, 600_000)] {
        writer(&h, sample(at, value)).await.unwrap();
    }
    let ratcheted = writer(
        &h,
        LoopOp::Ratchet {
            loop_id: id.into(),
            epoch: 1,
            metric: "pass_rate".into(),
            change_at_ms: 10,
        },
    )
    .await
    .unwrap();
    let verdict = &ratcheted.ratchets[0];
    assert_eq!(verdict.verdict, RatchetVerdict::Keep);
    assert_eq!(
        (verdict.before_mean_micro, verdict.before_samples),
        (200_000, 3)
    );
    assert_eq!(
        (verdict.after_mean_micro, verdict.after_samples),
        (500_000, 3)
    );

    let seq = |steps: &[&str]| steps.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let mined = writer(
        &h,
        LoopOp::Mine {
            loop_id: id.into(),
            epoch: 1,
            failure_sequences: vec![
                seq(&["compile", "test", "retry"]),
                seq(&["compile", "test"]),
                seq(&["lint", "compile", "test"]),
            ],
        },
    )
    .await
    .unwrap();
    assert_eq!(mined.patterns.len(), 1);
    assert_eq!(mined.patterns[0].sequence, seq(&["compile", "test"]));
    assert_eq!(mined.patterns[0].support, 3);

    // A stale epoch is fenced out; a second tenant cannot see the loop.
    is_refusal(
        writer(&h, file_gap(id, 2, "g3", 1)).await,
        "LOOP_LEASE_STALE",
    );
    is_refusal(
        call(&h, "loop-writer", OTHER_TENANT, get(id)).await,
        "LOOP_NOT_FOUND",
    );

    // Release ends the holder; the old epoch no longer acts; a new claim
    // raises the epoch and the chart goes orphaned -> running.
    let released = writer(
        &h,
        LoopOp::Release {
            loop_id: id.into(),
            epoch: 1,
        },
    )
    .await
    .unwrap();
    assert_eq!(released.instance.state, "orphaned");
    is_refusal(
        writer(&h, file_gap(id, 1, "g3", 1)).await,
        "LOOP_LEASE_STALE",
    );
    let again = writer(&h, claim(id)).await.unwrap();
    assert_eq!(again.instance.state, "running");
    assert_eq!(again.instance.lease.unwrap().epoch, 2);
    assert_eq!(
        again
            .transitions
            .iter()
            .map(|t| t.event.as_str())
            .collect::<Vec<_>>(),
        ["claim", "release", "claim"]
    );
    // Everything written before survives the re-claim.
    assert_eq!(again.gaps.len(), 2);
    assert_eq!(again.ledger.samples.len(), 6);
}

// A lease that expires under a running loop is taken over: the chart goes
// running -> orphaned (lease_lost) -> running (claim) and the epoch rises.
// spec: MEDULLA-DECISION-ENGINE-R653.2
#[tokio::test]
async fn expired_lease_is_reclaimed_with_a_higher_epoch() {
    let h = Harness::new().await;
    let dir = h._dir.path().to_string_lossy().into_owned();
    let knobs = LoopKnobs {
        lease_ttl_s: 5,
        ..LoopKnobs::default()
    };
    let a = LoopStore::open(Some(&dir), TENANT, "holder-a")
        .unwrap()
        .with_knobs(knobs);
    let b = LoopStore::open(Some(&dir), TENANT, "holder-b")
        .unwrap()
        .with_knobs(knobs);
    let view = |payload| -> LoopView { decode_payload(payload) };

    let first = view(a.serve_at(claim("expiring"), 1_000).unwrap());
    assert_eq!(first.instance.lease.as_ref().unwrap().expires_at_ms, 6_000);
    // While the lease is live the second holder is refused.
    let held = b.serve_at(claim("expiring"), 5_999).unwrap_err();
    assert!(held.starts_with("LOOP_LEASE_HELD:"), "{held}");
    // At expiry the second holder takes over.
    let second = view(b.serve_at(claim("expiring"), 6_000).unwrap());
    let lease = second.instance.lease.as_ref().unwrap();
    assert_eq!((lease.epoch, lease.holder.as_str()), (2, "holder-b"));
    assert_eq!(second.instance.state, "running");
    let events: Vec<&str> = second
        .transitions
        .iter()
        .map(|t| t.event.as_str())
        .collect();
    assert_eq!(events, ["claim", "lease_lost", "claim"]);
    // The first holder is fenced out.
    let stale = a
        .serve_at(
            LoopOp::Beacon {
                loop_id: "expiring".into(),
                epoch: 1,
                stage: "x".into(),
            },
            6_001,
        )
        .unwrap_err();
    assert!(stale.starts_with("LOOP_LEASE_STALE:"), "{stale}");
}

fn decode_payload<T: serde::de::DeserializeOwned>(
    payload: medulla_protocol_types::protocol::ResultPayload,
) -> T {
    match payload {
        medulla_protocol_types::protocol::ResultPayload::Raw(bytes) => {
            rmp_serde::from_slice(&bytes).unwrap()
        }
        other => panic!("unexpected payload {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R653.4
#[tokio::test]
async fn loop_refusals_change_no_state() {
    let h = Harness::new().await;
    let id = "refusal-loop";
    writer(&h, claim(id)).await.unwrap();
    writer(&h, file_gap(id, 1, "open-gap", 500_000))
        .await
        .unwrap();
    writer(&h, file_gap(id, 1, "done-gap", 500_000))
        .await
        .unwrap();
    writer(&h, gap_op("offer", id, 1, "done-gap"))
        .await
        .unwrap();
    let before = writer(&h, gap_op("settle", id, 1, "done-gap"))
        .await
        .unwrap();

    // Each refusal leaves the stored loop exactly as it was.
    let cases: Vec<(LoopOp, &str)> = vec![
        (get("no-such-loop"), "LOOP_NOT_FOUND"),
        (claim(id), "LOOP_LEASE_HELD"),
        (file_gap(id, 9, "x", 1), "LOOP_LEASE_STALE"),
        (gap_op("offer", id, 1, "no-such-gap"), "GAP_NOT_FOUND"),
        (gap_op("settle", id, 1, "done-gap"), "GAP_ALREADY_SETTLED"),
        (gap_op("offer", id, 1, "done-gap"), "GAP_ALREADY_SETTLED"),
        (gap_op("settle", id, 1, "open-gap"), "GAP_NOT_OFFERED"),
        (
            LoopOp::Ratchet {
                loop_id: id.into(),
                epoch: 1,
                metric: "pass_rate".into(),
                change_at_ms: 10,
            },
            "RATCHET_NO_BASELINE",
        ),
    ];
    for (op, code) in cases {
        is_refusal(writer(&h, op).await, code);
        assert_eq!(writer(&h, get(id)).await.unwrap(), before, "after {code}");
    }

    // A malformed request is refused before any write.
    is_refusal(
        writer(&h, file_gap(id, 1, "huge", 1_000_001)).await,
        "PARAMETER_INVALID",
    );
    assert_eq!(writer(&h, get(id)).await.unwrap(), before);

    let dir = h._dir.path().to_string_lossy().into_owned();
    let seeded = LoopStore::open(Some(&dir), TENANT, "seeder").unwrap();

    // A loop whose stored state is outside the loop chart reads as
    // LOOP_STATE_UNKNOWN: seed both instances from the record chart.
    let good = writer(&h, get(id)).await.unwrap();
    let record_def = seeded
        .statecharts()
        .define(&loop_record_statechart_def())
        .unwrap();
    for kind in ["loop", "loop_record"] {
        seeded
            .statecharts()
            .instantiate_batch(
                &record_def,
                Context::from_value(serde_json::json!({"record": good})).unwrap(),
                TENANT,
                "seeder",
                &seeded.key(kind, "corrupt-loop"),
                1,
            )
            .unwrap();
    }
    is_refusal(writer(&h, get("corrupt-loop")).await, "LOOP_STATE_UNKNOWN");
    is_refusal(
        writer(
            &h,
            LoopOp::Release {
                loop_id: "corrupt-loop".into(),
                epoch: 1,
            },
        )
        .await,
        "LOOP_STATE_UNKNOWN",
    );
}

// spec: MEDULLA-DECISION-ENGINE-R653.5
#[tokio::test]
async fn loop_evidence_reads_back_through_a_second_principal() {
    let h = Harness::new().await;
    let id = "evidence-loop";
    writer(&h, claim(id)).await.unwrap();
    writer(&h, file_gap(id, 1, "g1", 900_000)).await.unwrap();
    writer(&h, gap_op("offer", id, 1, "g1")).await.unwrap();
    for (at, value) in [(1, 10), (2, 20), (3, 30), (10, 50), (11, 60), (12, 70)] {
        writer(
            &h,
            LoopOp::RecordSample {
                loop_id: id.into(),
                epoch: 1,
                sample: ImprovementSample {
                    at_ms: at,
                    metric: "pass_rate".into(),
                    value_micro: value,
                },
            },
        )
        .await
        .unwrap();
    }
    writer(
        &h,
        LoopOp::Ratchet {
            loop_id: id.into(),
            epoch: 1,
            metric: "pass_rate".into(),
            change_at_ms: 10,
        },
    )
    .await
    .unwrap();
    let written = writer(
        &h,
        LoopOp::Mine {
            loop_id: id.into(),
            epoch: 1,
            failure_sequences: vec![vec!["a".into(), "b".into()]; 3],
        },
    )
    .await
    .unwrap();

    let read = call(&h, "loop-reader", TENANT, get(id)).await.unwrap();
    assert_eq!(read, written, "every field reads back");
    assert_eq!(read.instance.lease.as_ref().unwrap().epoch, 1);
    assert_eq!(read.transitions.len(), 1);
    assert_eq!(read.transitions[0].event, "claim");
    assert!(!read.transitions[0].actor.is_empty());
    assert_eq!(read.offers[0].severity_ppm, 900_000);
    assert_eq!(read.offers[0].capacity, 8);
    assert_eq!(read.patterns[0].support, 3);
    assert_eq!(read.ratchets[0].before_mean_micro, 20);
    assert_eq!(read.ratchets[0].after_mean_micro, 60);

    is_refusal(
        call(&h, "loop-reader", OTHER_TENANT, get(id)).await,
        "LOOP_NOT_FOUND",
    );
}
