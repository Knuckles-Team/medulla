//! A SQL connector's column profiles through the served `SourceIngest` path:
//! which column values enter the embedding set and which are excluded, with
//! the reason named in the receipt (MEDULLA-DECISION-ENGINE-R079.4, R079.6).

use medulla_source_types::source_ingestion::column_profile::{
    ColumnExclusion, ColumnExclusionReason, EmbeddingSetEntry, SqlColumnProfile, SqlColumnType,
    Q32_ONE,
};
use medulla_source_types::source_ingestion::{
    SourceIngestionDisposition, SourceIngestionReceipt, SourceIngestionRequest, SourceJson,
};
use medulla_types::contract::{BoundedVec, ResourceId};

use super::ok;
use super::scale::{first_ingest, ingest, pack_graph};

fn profile(sql_type: SqlColumnType, fk: bool, ratio: f64, samples: u32) -> SqlColumnProfile {
    SqlColumnProfile {
        sql_type,
        is_foreign_key: fk,
        distinct_ratio_q32: (ratio * Q32_ONE as f64) as u64,
        sample_count: samples,
    }
}

/// One record with one column per kind, plus two free-text columns on either
/// side of the 100-sample floor.
pub(super) fn profiled_request() -> SourceIngestionRequest {
    let mut batch = first_ingest().as_batch().clone();
    batch.strict_schema = false;
    let mut records = batch.records.as_slice().to_vec();
    records[0].payload = SourceJson::new(serde_json::json!({
        "id": 1,
        "name": "one",
        "owner_id": "u-7",
        "active": "true",
        "created_at": "2026-09-20T00:00:00Z",
        "price": "12.50",
        "status": "open",
        "notes": "A long free-text note about the item.",
        "short_notes": "Measured over too few values.",
    }))
    .expect("bounded source JSON");
    records[0].column_profiles = [
        ("owner_id", profile(SqlColumnType::Text, true, 0.9, 500)),
        ("active", profile(SqlColumnType::Boolean, false, 0.9, 500)),
        (
            "created_at",
            profile(SqlColumnType::Timestamp, false, 0.9, 500),
        ),
        ("price", profile(SqlColumnType::Numeric, false, 0.9, 500)),
        ("status", profile(SqlColumnType::Text, false, 0.4, 500)),
        ("notes", profile(SqlColumnType::Text, false, 0.9, 100)),
        ("short_notes", profile(SqlColumnType::Text, false, 0.9, 99)),
    ]
    .into_iter()
    .map(|(column, profile)| (column.to_string(), profile))
    .collect();
    batch.records = BoundedVec::new(records).expect("bounded source records");
    SourceIngestionRequest::new(batch).expect("valid source ingestion request")
}

fn exclusion(column: &str, reason: ColumnExclusionReason) -> ColumnExclusion {
    ColumnExclusion {
        stream: ResourceId::new("items").unwrap(),
        column: column.into(),
        reason,
    }
}

// spec: MEDULLA-DECISION-ENGINE-R079.4
#[tokio::test]
async fn sql_column_kinds_and_cardinality_control_embedding() {
    let (served, _guard) = pack_graph("sql-columns").await;
    let request = profiled_request();
    let receipt: SourceIngestionReceipt = ok(
        "profiled ingest",
        ingest(&served, "sql-columns:first", &request).await,
    );
    assert_eq!(receipt.disposition, SourceIngestionDisposition::Committed);
    let mut expected = vec![
        exclusion("owner_id", ColumnExclusionReason::ForeignKey),
        exclusion("active", ColumnExclusionReason::Boolean),
        exclusion("created_at", ColumnExclusionReason::Timestamp),
        exclusion("price", ColumnExclusionReason::Numeric),
        exclusion("status", ColumnExclusionReason::Enumeration),
        exclusion("short_notes", ColumnExclusionReason::InsufficientSamples),
    ];
    expected.sort();
    assert_eq!(
        receipt.column_exclusions, expected,
        "the receipt names each excluded column with its reason"
    );
    assert_eq!(
        receipt.embedding_set,
        vec![EmbeddingSetEntry {
            stream: ResourceId::new("items").unwrap(),
            record_id: "item-1".into(),
            column: "notes".into(),
        }],
        "only the free-text column measured over 100 values enters the set"
    );

    let replayed: SourceIngestionReceipt = ok(
        "profiled replay",
        ingest(&served, "sql-columns:again", &request).await,
    );
    assert_eq!(replayed.disposition, SourceIngestionDisposition::Replayed);
    assert_eq!(replayed.column_exclusions, expected);
    assert_eq!(replayed.embedding_set, receipt.embedding_set);
}
