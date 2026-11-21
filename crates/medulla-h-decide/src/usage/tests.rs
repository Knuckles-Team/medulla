//! Served usage append and price put (MEDULLA-DECISION-ENGINE-R801.8).

use super::handle_usage;
use crate::decide::stat_test_support::Harness;
use medulla_decision_types::usage::{
    PriceCatalog, PriceEntry, UsageAppended, UsageOp, UsageOutcome, UsagePriceStored,
    UsageReadResult, UsageReadView, UsageRecord, USAGE_SCHEMA_VERSION,
};
use medulla_protocol_types::protocol::{Response, ResultPayload};
use medulla_server_api::authority_context::VerifiedRequestContext;

const TENANT: &str = "tenant-usage";

fn record(tenant: &str, run: &str) -> UsageRecord {
    UsageRecord {
        schema_version: USAGE_SCHEMA_VERSION,
        recorded_at_ms: 1_700_000_000_000,
        child_id: "child".into(),
        operation: "decide".into(),
        tenant: tenant.into(),
        principal: "ignored".into(),
        run_id: run.into(),
        model_key: "m1".into(),
        tokens_in: 10,
        tokens_out: 5,
        tokens_cached: 2,
        latency_ms: 40,
        outcome: UsageOutcome::Ok,
        content_digest: "a".repeat(64),
    }
}

fn price(valid_from_ms: u64, input: u64) -> PriceEntry {
    PriceEntry {
        schema_version: USAGE_SCHEMA_VERSION,
        model_key: "m1".into(),
        valid_from_ms,
        in_per_mtok_micro: input,
        out_per_mtok_micro: 15_000_000,
        cached_per_mtok_micro: 300_000,
    }
}

fn decode<T: serde::de::DeserializeOwned>(response: Response) -> Result<T, String> {
    if let Some(error) = response.error {
        return Err(error);
    }
    match response.result {
        Some(ResultPayload::Raw(bytes)) => Ok(rmp_serde::from_slice(&bytes).unwrap()),
        other => panic!("unexpected payload {other:?}"),
    }
}

fn service() -> VerifiedRequestContext {
    VerifiedRequestContext::verified_for_test_with_scopes("svc", TENANT, &["usage:write"])
}

fn human() -> VerifiedRequestContext {
    VerifiedRequestContext::verified_for_test_with_scopes(
        "alice",
        TENANT,
        &["usage:read", "kg:write"],
    )
}

#[tokio::test]
async fn service_append_succeeds_and_human_is_refused() {
    // spec: MEDULLA-DECISION-ENGINE-R801.8
    let h = Harness::new().await;
    let append = || UsageOp::Append {
        records: vec![record(TENANT, "r1"), record(TENANT, "r2")],
    };

    let appended: UsageAppended =
        decode(handle_usage(&h.state, 1, &service(), append()).await).unwrap();
    assert_eq!(appended.appended, 2);
    let hex = appended
        .batch_digest
        .strip_prefix("sha256:")
        .unwrap_or(&appended.batch_digest);
    assert_eq!(hex.len(), 64, "{}", appended.batch_digest);

    let refused =
        decode::<UsageAppended>(handle_usage(&h.state, 2, &human(), append()).await).unwrap_err();
    assert!(refused.starts_with("ACCESS_DENIED"), "{refused}");
    let runs = h.store.recent_runs(TENANT, "child", "decide", 10).unwrap();
    assert_eq!(runs.len(), 2, "the refused append wrote nothing");

    let foreign = decode::<UsageAppended>(
        handle_usage(
            &h.state,
            3,
            &service(),
            UsageOp::Append {
                records: vec![record("tenant-other", "r3")],
            },
        )
        .await,
    )
    .unwrap_err();
    assert!(foreign.starts_with("USAGE_UNKNOWN_TENANT"), "{foreign}");

    let put = |entries| UsageOp::PutPrice {
        catalog: PriceCatalog {
            aliases: Default::default(),
            entries,
        },
    };
    let stored: UsagePriceStored =
        decode(handle_usage(&h.state, 4, &service(), put(vec![price(10, 3_000_000)])).await)
            .unwrap();
    assert_eq!(stored.entries, 1);
    let overlap = decode::<UsagePriceStored>(
        handle_usage(&h.state, 5, &service(), put(vec![price(5, 1)])).await,
    )
    .unwrap_err();
    assert!(overlap.starts_with("PRICE_WINDOW_OVERLAP"), "{overlap}");
    let human_put = decode::<UsagePriceStored>(
        handle_usage(&h.state, 6, &human(), put(vec![price(20, 1)])).await,
    )
    .unwrap_err();
    assert!(human_put.starts_with("ACCESS_DENIED"), "{human_put}");
    assert_eq!(h.store.price_entries(TENANT).unwrap().len(), 1);

    // A catalog is one revision: a refused entry leaves none of it stored.
    let mut fresh = price(1, 1);
    fresh.model_key = "m0".into();
    let partial = decode::<UsagePriceStored>(
        handle_usage(&h.state, 7, &service(), put(vec![fresh, price(5, 1)])).await,
    )
    .unwrap_err();
    assert!(partial.starts_with("PRICE_WINDOW_OVERLAP"), "{partial}");
    assert_eq!(h.store.price_entries(TENANT).unwrap().len(), 1);
}

#[tokio::test]
async fn two_tenant_appends_stay_in_their_own_tenant() {
    // spec: MEDULLA-DECISION-ENGINE-R801.8
    const OTHER: &str = "tenant-usage-b";
    let h = Harness::new().await;
    let other =
        VerifiedRequestContext::verified_for_test_with_scopes("svc-b", OTHER, &["usage:write"]);
    let append = |tenant: &str, runs: &[&str]| UsageOp::Append {
        records: runs.iter().map(|run| record(tenant, run)).collect(),
    };
    let a: UsageAppended =
        decode(handle_usage(&h.state, 1, &service(), append(TENANT, &["a1", "a2"])).await).unwrap();
    let b: UsageAppended =
        decode(handle_usage(&h.state, 2, &other, append(OTHER, &["b1"])).await).unwrap();
    assert_eq!((a.appended, b.appended), (2, 1));
    let runs_a = h.store.recent_runs(TENANT, "child", "decide", 10).unwrap();
    let runs_b = h.store.recent_runs(OTHER, "child", "decide", 10).unwrap();
    assert_eq!(runs_a.len(), 2, "tenant A reads only its own entries");
    assert_eq!(runs_b.len(), 1, "tenant B reads only its own entries");
    // A tenant cannot write into the other tenant by naming it in a record.
    let cross =
        decode::<UsageAppended>(handle_usage(&h.state, 3, &other, append(TENANT, &["x"])).await)
            .unwrap_err();
    assert!(cross.starts_with("USAGE_UNKNOWN_TENANT"), "{cross}");
    assert_eq!(
        h.store
            .recent_runs(TENANT, "child", "decide", 10)
            .unwrap()
            .len(),
        2
    );
}

fn read_op(view: UsageReadView, cursor: Option<String>) -> UsageOp {
    UsageOp::Read {
        from_ms: 0,
        to_ms: u64::MAX,
        model_key: None,
        view,
        cursor,
    }
}

#[tokio::test]
async fn read_is_tenant_scoped() {
    // spec: MEDULLA-DECISION-ENGINE-R801.7
    const OTHER: &str = "tenant-usage-b";
    let h = Harness::new().await;
    let other = VerifiedRequestContext::verified_for_test_with_scopes(
        "svc-b",
        OTHER,
        &["usage:write", "usage:read"],
    );
    let reader_b =
        VerifiedRequestContext::verified_for_test_with_scopes("bob", OTHER, &["usage:read"]);
    let a: UsageAppended = decode(
        handle_usage(
            &h.state,
            1,
            &service(),
            UsageOp::Append {
                records: vec![record(TENANT, "a1"), record(TENANT, "a2")],
            },
        )
        .await,
    )
    .unwrap();
    assert_eq!(a.appended, 2);
    let b: UsageAppended = decode(
        handle_usage(
            &h.state,
            2,
            &other,
            UsageOp::Append {
                records: vec![record(OTHER, "b1")],
            },
        )
        .await,
    )
    .unwrap();
    assert_eq!(b.appended, 1);

    // A human principal with `usage:read` is admitted; its tenant is the verified one.
    let records = |result: UsageReadResult| match result {
        UsageReadResult::Records {
            records,
            next_cursor,
        } => (records, next_cursor),
        other => panic!("records view expected, got {other:?}"),
    };
    let (mine, cursor) = records(
        decode(handle_usage(&h.state, 3, &human(), read_op(UsageReadView::Records, None)).await)
            .unwrap(),
    );
    assert_eq!(mine.len(), 2);
    assert!(mine.iter().all(|r| r.tenant == TENANT));
    assert_eq!(cursor, None);
    let (theirs, _) = records(
        decode(
            handle_usage(
                &h.state,
                4,
                &reader_b,
                read_op(UsageReadView::Records, None),
            )
            .await,
        )
        .unwrap(),
    );
    assert_eq!(theirs.len(), 1);
    assert!(theirs.iter().all(|r| r.tenant == OTHER));

    // The rollup is tenant-scoped too.
    let rollup: UsageReadResult =
        decode(handle_usage(&h.state, 5, &human(), read_op(UsageReadView::Rollup, None)).await)
            .unwrap();
    match rollup {
        UsageReadResult::Rollup { days } => {
            assert_eq!(days.iter().map(|d| d.calls).sum::<u64>(), 2);
        }
        other => panic!("rollup view expected, got {other:?}"),
    }

    // A caller without `usage:read` is refused; an inverted window is invalid.
    let nobody =
        VerifiedRequestContext::verified_for_test_with_scopes("x", TENANT, &["kg:write"]);
    let denied = decode::<UsageReadResult>(
        handle_usage(&h.state, 6, &nobody, read_op(UsageReadView::Records, None)).await,
    )
    .unwrap_err();
    assert!(denied.starts_with("ACCESS_DENIED"), "{denied}");
    let inverted = decode::<UsageReadResult>(
        handle_usage(
            &h.state,
            7,
            &human(),
            UsageOp::Read {
                from_ms: 2,
                to_ms: 1,
                model_key: None,
                view: UsageReadView::Records,
                cursor: None,
            },
        )
        .await,
    )
    .unwrap_err();
    assert!(inverted.starts_with("PARAMETER_INVALID"), "{inverted}");

    // A 1001-record read pages with a cursor.
    for chunk in 0..11u64 {
        let batch: Vec<UsageRecord> = (0..100u64)
            .map(|i| {
                let mut r = record(TENANT, "bulk");
                r.recorded_at_ms = 1_800_000_000_000 + chunk * 100 + i;
                r
            })
            .collect();
        decode::<UsageAppended>(
            handle_usage(
                &h.state,
                100 + chunk,
                &service(),
                UsageOp::Append { records: batch },
            )
            .await,
        )
        .unwrap();
    }
    let (first, cursor) = records(
        decode(handle_usage(&h.state, 200, &human(), read_op(UsageReadView::Records, None)).await)
            .unwrap(),
    );
    assert_eq!(first.len(), 1000);
    let cursor = cursor.expect("a full page carries a cursor");
    let (rest, end) = records(
        decode(
            handle_usage(
                &h.state,
                201,
                &human(),
                read_op(UsageReadView::Records, Some(cursor)),
            )
            .await,
        )
        .unwrap(),
    );
    assert_eq!(rest.len(), 102, "1102 rows in all");
    assert_eq!(end, None);

    // A forged cursor of the other tenant refuses before any read.
    let forged = hex::encode(format!("usage:{OTHER}/{:020}/{:010}", 0, 0));
    let refused = decode::<UsageReadResult>(
        handle_usage(
            &h.state,
            202,
            &human(),
            read_op(UsageReadView::Records, Some(forged)),
        )
        .await,
    )
    .unwrap_err();
    assert!(refused.starts_with("USAGE_BAD_CURSOR"), "{refused}");
    let garbage = decode::<UsageReadResult>(
        handle_usage(
            &h.state,
            203,
            &human(),
            read_op(UsageReadView::Records, Some("zz".into())),
        )
        .await,
    )
    .unwrap_err();
    assert!(garbage.starts_with("USAGE_BAD_CURSOR"), "{garbage}");
}
