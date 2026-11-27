// MEDULLA-FINANCE-PRIMITIVES-R005.3.4: the served performance method answers a null card with
// the reason NO_VALUATION, never a zero, for a bound account whose range holds no valuation.

use std::collections::BTreeMap;

use medulla_finance_types::wire_finance::{FinanceRecord, FinanceRecordKind};
use medulla_protocol_types::protocol::Method;
use medulla_types::finance_accounting::FinancePerformanceCard;

use super::connector_pack_harness::{ok, Served, TENANT};

// spec: MEDULLA-FINANCE-PRIMITIVES-R005.3.4
#[tokio::test]
async fn a_range_without_a_valuation_is_a_null_card_with_no_valuation() {
    let served = Served::provisioned("finance-source", &[]).await;
    let mut fields = BTreeMap::new();
    fields.insert(
        "accountBindingState".to_string(),
        "\"verified\"".to_string(),
    );
    let put = served
        .verified(
            TENANT,
            &["*"],
            Method::FinanceRecordPut {
                kind: FinanceRecordKind::Account,
                schema_version: "finance-v1".to_string(),
                record: FinanceRecord {
                    record_id: "acct".to_string(),
                    tenant: TENANT.to_string(),
                    fields,
                },
                expected_head: None,
            },
        )
        .await;
    assert!(put.error.is_none(), "FinanceRecordPut: {put:?}");

    let response = served
        .verified(
            TENANT,
            &["*"],
            Method::FinancePerformance {
                account: "acct".to_string(),
                as_of: "2026-03-01T00:00:00Z".to_string(),
                known_at: "2026-03-01T00:00:00Z".to_string(),
                base_currency: "USD".to_string(),
                method: "twr".to_string(),
                range: "1M".to_string(),
                benchmark: None,
            },
        )
        .await;
    let card: FinancePerformanceCard = ok("FinancePerformance", response);
    assert_eq!(
        card.value_ticks, None,
        "an unavailable figure is null, never zero"
    );
    assert_eq!(card.value, None);
    assert_eq!(card.reason.as_deref(), Some("NO_VALUATION"));
    assert!(card.benchmark.is_none());
}
