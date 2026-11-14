//! Served usage append and price put (MEDULLA-DECISION-ENGINE-R801.8).

use super::handle_usage;
use crate::decide::stat_test_support::Harness;
use medulla_protocol_types::protocol::{Response, ResultPayload};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_types::usage::{
    PriceCatalog, PriceEntry, UsageAppended, UsageOp, UsageOutcome, UsagePriceStored, UsageRecord,
    USAGE_SCHEMA_VERSION,
};

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
    let hex = appended.batch_digest.strip_prefix("sha256:").unwrap_or(&appended.batch_digest);
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
        decode(handle_usage(&h.state, 1, &service(), append(TENANT, &["a1", "a2"])).await)
            .unwrap();
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
    assert_eq!(h.store.recent_runs(TENANT, "child", "decide", 10).unwrap().len(), 2);
}
