use std::collections::BTreeMap;

use medulla_decision_types::usage::{
    PriceCatalog, PriceEntry, UsageGroupBy, UsageOutcome, UsageRecord, USAGE_SCHEMA_VERSION,
};

use super::super::agent_fixtures::open_agent_store;
use super::*;

const TENANT: &str = "t1";

fn record(run: &str, model: &str, at_ms: u64, tokens: (u64, u64, u64)) -> UsageRecord {
    UsageRecord {
        schema_version: USAGE_SCHEMA_VERSION,
        recorded_at_ms: at_ms,
        child_id: "child".into(),
        operation: "decide".into(),
        tenant: TENANT.into(),
        principal: "p1".into(),
        run_id: run.into(),
        model_key: model.into(),
        tokens_in: tokens.0,
        tokens_out: tokens.1,
        tokens_cached: tokens.2,
        latency_ms: 10,
        outcome: UsageOutcome::Ok,
        content_digest: "a".repeat(64),
        project_id: String::new(),
        agent_id: String::new(),
    }
}

fn price(model: &str, valid_from_ms: u64, in_p: u64, out_p: u64, cached_p: u64) -> PriceEntry {
    PriceEntry {
        schema_version: USAGE_SCHEMA_VERSION,
        model_key: model.into(),
        valid_from_ms,
        in_per_mtok_micro: in_p,
        out_per_mtok_micro: out_p,
        cached_per_mtok_micro: cached_p,
        currency: String::new(),
    }
}

fn catalog() -> PriceCatalog {
    PriceCatalog {
        aliases: BTreeMap::from([("gpt-x-latest".to_string(), "gpt-x".to_string())]),
        entries: vec![price("gpt-x", 0, 3_000_000, 15_000_000, 300_000)],
    }
}

fn row_count(store: &AgentLibraryStore, prefix: &str) -> usize {
    store
        .decision_artifacts_with_prefix(TENANT, prefix, 100_000)
        .unwrap()
        .len()
}

#[test]
fn cost_uses_price_valid_at_record_time() {
    // spec: MEDULLA-DECISION-ENGINE-R801.2
    let catalog = PriceCatalog {
        aliases: BTreeMap::new(),
        entries: vec![
            price("m", 100, 1_000_000, 2_000_000, 500_000),
            price("m", 200, 10_000_000, 20_000_000, 5_000_000),
        ],
    };
    let between = record("r", "m", 150, (1_000, 1_000, 0));
    // 1000 * 1 + 1000 * 2 micro per token = 3000.
    assert_eq!(cost_micro(&between, &catalog), Some(3_000));
    let after = record("r", "m", 200, (1_000, 1_000, 0));
    assert_eq!(cost_micro(&after, &catalog), Some(30_000));
    let before = record("r", "m", 99, (1_000, 1_000, 0));
    assert_eq!(cost_micro(&before, &catalog), None);
    // cached tokens are billed at the cached price, not the input price.
    let cached = record("r", "m", 150, (1_000, 0, 400));
    assert_eq!(cost_micro(&cached, &catalog), Some(600 + 200));
}

#[test]
fn alias_table_normalizes_model_keys() {
    // spec: MEDULLA-DECISION-ENGINE-R801.2
    let catalog = catalog();
    assert_eq!(normalize_model_key(&catalog, "  GPT-X-Latest "), "gpt-x");
    assert_eq!(
        normalize_model_key(&catalog, "Unmapped-Model"),
        "unmapped-model"
    );
    let aliased = record("r", "GPT-X-Latest", 10, (1_000_000, 0, 0));
    assert_eq!(cost_micro(&aliased, &catalog), Some(3_000_000));
}

#[test]
fn day_rollup_reads_buckets_not_records() {
    // spec: MEDULLA-DECISION-ENGINE-R801.2
    let (_dir, store) = open_agent_store();
    let batch: Vec<UsageRecord> = (0..50u64)
        .map(|i| {
            record(
                &format!("run-{i}"),
                "gpt-x",
                (i % 2) * DAY_MS + i,
                (100, 10, 0),
            )
        })
        .collect();
    store
        .append_usage(TENANT, "p1", &catalog(), &batch, 5)
        .unwrap();
    let rollup = store.usage_rollup(TENANT, &catalog(), 0, 1).unwrap();
    assert_eq!(rollup.days.len(), 2);
    assert_eq!(rollup.days.iter().map(|d| d.calls).sum::<u64>(), 50);
    assert!(rollup.rows_read <= rollup.days.len());
    assert!(row_count(&store, "usage:") >= 50);
}

/// Verifies MEDULLA-DECISION-ENGINE-R801.4: Ledger refusals write nothing
#[test]
fn each_refusal_writes_nothing() {
    // spec: MEDULLA-DECISION-ENGINE-R801.4
    let (_dir, store) = open_agent_store();
    let catalog = catalog();
    let good = record("r", "gpt-x", 10, (100, 10, 0));
    store
        .append_usage(TENANT, "p1", &catalog, std::slice::from_ref(&good), 1)
        .unwrap();
    let before = row_count(&store, "usage");

    let mut foreign = good.clone();
    foreign.tenant = "other".into();
    let mut cached = good.clone();
    cached.tokens_cached = 101;
    let mut digest = good.clone();
    digest.content_digest = "a".repeat(40);
    let too_many = vec![good.clone(); 1_001];
    let cases = [
        (vec![good.clone(), foreign], "USAGE_UNKNOWN_TENANT"),
        (vec![good.clone(), cached], "USAGE_CACHED_EXCEEDS_INPUT"),
        (vec![good.clone(), digest], "USAGE_BAD_DIGEST"),
        (too_many, "USAGE_BATCH_TOO_LARGE"),
    ];
    for (batch, code) in cases {
        let refusal = store
            .append_usage(TENANT, "p1", &catalog, &batch, 2)
            .unwrap_err();
        assert_eq!(refusal.code().as_str(), code);
        assert_eq!(row_count(&store, "usage"), before);
    }
    let negative = require_non_negative(-1).unwrap_err();
    assert_eq!(negative.code().as_str(), "USAGE_NEGATIVE_COUNT");
    assert_eq!(require_non_negative(7), Ok(7));

    let first = price("m", 100, 1, 2, 3);
    store.put_price(TENANT, &first).unwrap();
    store.put_price(TENANT, &first).unwrap();
    let prices = row_count(&store, "price");
    assert_eq!(prices, 1);
    let overlap = store
        .put_price(TENANT, &price("m", 100, 9, 9, 9))
        .unwrap_err();
    assert_eq!(overlap.code().as_str(), "PRICE_WINDOW_OVERLAP");
    let older = store
        .put_price(TENANT, &price("m", 50, 9, 9, 9))
        .unwrap_err();
    assert_eq!(older, UsageRefusal::PriceWindowOverlap);
    assert_eq!(row_count(&store, "price"), prices);
    store.put_price(TENANT, &price("m", 101, 9, 9, 9)).unwrap();
    assert_eq!(store.price_entries(TENANT).unwrap().len(), 2);
}

/// Verifies MEDULLA-DECISION-ENGINE-R801.5: Audit line and golden cost report
#[test]
fn append_writes_one_audit_line() {
    // spec: MEDULLA-DECISION-ENGINE-R801.5
    let (_dir, store) = open_agent_store();
    let batch = vec![
        record("r1", "gpt-x", 10, (100, 10, 0)),
        record("r2", "gpt-x", 11, (100, 10, 0)),
    ];
    let receipt = store
        .append_usage(TENANT, "p1", &catalog(), &batch, 42)
        .unwrap();
    assert_eq!(receipt.appended, 2);
    let lines = store
        .decision_artifacts_with_prefix(TENANT, "usage-audit:", 100)
        .unwrap();
    assert_eq!(lines.len(), 1);
    let line: UsageAuditLine = serde_json::from_slice(&lines[0].1).unwrap();
    assert_eq!(line.record_count, 2);
    assert_eq!(line.principal, "p1");
    assert_eq!(line.appended_at_ms, 42);
    assert_eq!(line.batch_digest, receipt.batch_digest);
    assert!(line.batch_digest.starts_with("sha256:"));
    let text = String::from_utf8(lines[0].1.clone()).unwrap();
    assert!(!text.contains("run_id") && !text.contains("tokens"));
}

#[test]
fn golden_cost_rollup_is_byte_identical() {
    // spec: MEDULLA-DECISION-ENGINE-R801.5
    const GOLDEN: &str = include_str!("../../../../tests/golden/usage_cost_rollup.json");
    let mut errored = record("r2", "GPT-X ", 2_000, (2_000, 1_000, 0));
    errored.outcome = UsageOutcome::Error;
    let batch = vec![
        record("r1", "gpt-x", 1_000, (1_000, 500, 200)),
        errored,
        record("r3", "gpt-x-latest", DAY_MS + 5, (4_000, 2_000, 1_000)),
    ];
    let mut bytes = Vec::new();
    for _ in 0..2 {
        let (_dir, store) = open_agent_store();
        store
            .append_usage(TENANT, "p1", &catalog(), &batch, 9)
            .unwrap();
        let rollup = store.usage_rollup(TENANT, &catalog(), 0, 30).unwrap();
        bytes.push(serde_json::to_string(&rollup.days).unwrap());
    }
    assert_eq!(bytes[0], bytes[1]);
    assert_eq!(bytes[0], GOLDEN.trim_end());
}

/// Verifies MEDULLA-DECISION-ENGINE-R801.2: Append, derived cost, name normalization and day buckets
#[test]
fn recent_runs_flags_errored_runs_newest_first() {
    // spec: MEDULLA-DECISION-ENGINE-R801.2
    let (_dir, store) = open_agent_store();
    let mut bad = record("old", "gpt-x", 10, (10, 1, 0));
    bad.outcome = UsageOutcome::Error;
    let batch = vec![
        bad,
        record("old", "gpt-x", 11, (10, 1, 0)),
        record("new", "gpt-x", 20, (10, 1, 0)),
    ];
    store
        .append_usage(TENANT, "p1", &catalog(), &batch, 1)
        .unwrap();
    let runs = store.recent_runs(TENANT, "child", "decide", 5).unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!((runs[0].run_id.as_str(), runs[0].errored), ("new", false));
    assert_eq!((runs[1].run_id.as_str(), runs[1].errored), ("old", true));
    assert_eq!(
        store
            .recent_runs(TENANT, "child", "decide", 1)
            .unwrap()
            .len(),
        1
    );
    assert!(store
        .recent_runs("t2", "child", "decide", 5)
        .unwrap()
        .is_empty());
}

fn project_record(
    run: &str,
    at_ms: u64,
    project: &str,
    agent: &str,
    latency_ms: u64,
) -> UsageRecord {
    UsageRecord {
        latency_ms,
        project_id: project.into(),
        agent_id: agent.into(),
        ..record(run, "gpt-x", at_ms, (1_000_000, 0, 0))
    }
}

#[test]
fn groups_fold_by_project_and_agent_with_cost_currency_and_p95() {
    // spec: MEDULLA-DECISION-ENGINE-R801.9
    let (_dir, store) = open_agent_store();
    let mut priced = price("gpt-x", 0, 1_000_000, 0, 0);
    priced.currency = "EUR".into();
    let catalog = PriceCatalog {
        aliases: BTreeMap::new(),
        entries: vec![priced],
    };
    // Twenty calls of project alpha with latencies 1..=20, one of project beta.
    let mut batch: Vec<UsageRecord> = (1..=20u64)
        .map(|i| project_record(&format!("a{i}"), 100 + i, "alpha", "agent-1", i))
        .collect();
    batch.push(project_record("b1", 200, "beta", "agent-2", 7));
    store
        .append_usage(TENANT, "p1", &catalog, &batch, 5)
        .unwrap();

    let by_project = store
        .usage_groups(TENANT, &catalog, 0, u64::MAX, UsageGroupBy::Project)
        .unwrap();
    let keys: Vec<&str> = by_project.iter().map(|g| g.key.as_str()).collect();
    assert_eq!(keys, ["alpha", "beta"]);
    let alpha = &by_project[0];
    assert_eq!(alpha.calls, 20);
    assert_eq!(alpha.cost_micro, Some(20_000_000));
    assert_eq!(alpha.currency.as_deref(), Some("EUR"));
    // Nearest rank: the ceil(0.95 * 20) = 19th smallest latency.
    assert_eq!(alpha.latency_p95_ms, 19);
    assert_eq!(by_project[1].latency_p95_ms, 7);

    let by_agent = store
        .usage_groups(TENANT, &catalog, 0, u64::MAX, UsageGroupBy::Agent)
        .unwrap();
    assert_eq!(
        by_agent.iter().map(|g| g.calls).collect::<Vec<_>>(),
        [20, 1]
    );
    // The window bounds the fold.
    let narrow = store
        .usage_groups(TENANT, &catalog, 150, u64::MAX, UsageGroupBy::Project)
        .unwrap();
    assert_eq!(narrow.len(), 1);
    assert_eq!(narrow[0].key, "beta");

    // The ledger of another tenant is empty: nothing of this one is reachable.
    assert!(store
        .usage_groups("other", &catalog, 0, u64::MAX, UsageGroupBy::Project)
        .unwrap()
        .is_empty());
}

#[test]
fn a_group_with_unpriced_calls_or_mixed_currencies_reports_no_cost() {
    // spec: MEDULLA-DECISION-ENGINE-R801.9
    let (_dir, store) = open_agent_store();
    let mut eur = price("gpt-x", 0, 1_000_000, 0, 0);
    eur.currency = "EUR".into();
    let mut usd = price("gpt-x", 1_000, 1_000_000, 0, 0);
    usd.currency = "USD".into();
    let catalog = PriceCatalog {
        aliases: BTreeMap::new(),
        entries: vec![eur, usd],
    };
    let batch = vec![
        project_record("m1", 10, "mixed", "a", 1),
        project_record("m2", 2_000, "mixed", "a", 1),
        project_record("c1", 20, "clean", "a", 1),
        UsageRecord {
            model_key: "unpriced-model".into(),
            ..project_record("u1", 30, "gap", "a", 1)
        },
    ];
    store
        .append_usage(TENANT, "p1", &catalog, &batch, 5)
        .unwrap();
    let groups = store
        .usage_groups(TENANT, &catalog, 0, u64::MAX, UsageGroupBy::Project)
        .unwrap();
    let get = |key: &str| groups.iter().find(|g| g.key == key).unwrap();
    // No conversion is ever made: two currencies leave the cost absent.
    assert_eq!(
        (get("mixed").cost_micro, get("mixed").currency.clone()),
        (None, None)
    );
    assert_eq!(get("clean").cost_micro, Some(1_000_000));
    assert_eq!(get("clean").currency.as_deref(), Some("EUR"));
    assert_eq!(get("gap").cost_micro, None);
}

#[test]
fn watermark_is_the_newest_recorded_time_and_survives_older_appends() {
    // spec: MEDULLA-DECISION-ENGINE-R801.9
    let (_dir, store) = open_agent_store();
    assert_eq!(store.usage_watermark(TENANT).unwrap(), None);
    let catalog = catalog();
    store
        .append_usage(
            TENANT,
            "p1",
            &catalog,
            &[record("r1", "gpt-x", 500, (1, 1, 0))],
            1,
        )
        .unwrap();
    assert_eq!(store.usage_watermark(TENANT).unwrap(), Some(500));
    // An append of older events never moves the watermark back.
    store
        .append_usage(
            TENANT,
            "p1",
            &catalog,
            &[record("r2", "gpt-x", 100, (1, 1, 0))],
            2,
        )
        .unwrap();
    assert_eq!(store.usage_watermark(TENANT).unwrap(), Some(500));
    store
        .append_usage(
            TENANT,
            "p1",
            &catalog,
            &[record("r3", "gpt-x", 900, (1, 1, 0))],
            3,
        )
        .unwrap();
    assert_eq!(store.usage_watermark(TENANT).unwrap(), Some(900));
    // Another tenant's ledger has its own.
    assert_eq!(store.usage_watermark("other").unwrap(), None);
}
