//! MEDULLA-TYPED-PACKS-R714: the delivery-path drift gate.
//!
//! `prepare` calls [`check`] once, before any record is lowered. For each stream
//! of the batch that has an approved contract in force (the typed contract of
//! the graph's `approved:<stream>` schema source, written by `AttachApproved`),
//! the gate infers the shape of the stream's records and classifies it against
//! that contract. The report is observation-class evidence: it mutates nothing.
//! A stream with no approved contract is skipped.
//!
//! This is a handler crate, so it reads the shared source type
//! (`GraphSchemaSources`) and never calls the `medulla-h-admin` handler.

use std::collections::BTreeMap;

use medulla_catalog_types::graph_schema::approval::APPROVED_SOURCE_PREFIX;
use medulla_catalog_types::graph_schema::drift::{
    classify_delivery, EvolutionPolicy, InferredRecordShape,
};
use medulla_graph_core::graph::GraphSchemaSources;
use medulla_source_types::source_ingestion::{SourceRecord, StreamDrift};
use medulla_types::contract::ResourceId;

/// The evolution policy in force for a delivery. No per-source declaration
/// exists, so every stream runs the default: quarantine, fixed.
const FIXED_POLICY: EvolutionPolicy = EvolutionPolicy::Quarantine;

/// Classify each approved stream of `records` against its contract in force.
///
/// At most `sample_max` records of a stream are sampled (the knob
/// `drift.sample_max`). Streams are reported in name order.
pub fn check(
    sources: &GraphSchemaSources,
    records: &[SourceRecord],
    sample_max: usize,
) -> Result<Vec<StreamDrift>, String> {
    let mut by_stream: BTreeMap<&ResourceId, Vec<serde_json::Value>> = BTreeMap::new();
    for record in records {
        let source_id = format!("{APPROVED_SOURCE_PREFIX}{}", record.stream.as_str());
        let has_contract = sources
            .dynamic
            .get(&source_id)
            .is_some_and(|source| source.contract.is_some());
        if !has_contract {
            continue;
        }
        let samples = by_stream.entry(&record.stream).or_default();
        if samples.len() < sample_max {
            samples.push(record.payload.value()?);
        }
    }
    let mut reports = Vec::with_capacity(by_stream.len());
    for (stream, samples) in by_stream {
        let source_id = format!("{APPROVED_SOURCE_PREFIX}{}", stream.as_str());
        let Some(contract) = sources
            .dynamic
            .get(&source_id)
            .and_then(|source| source.contract.as_ref())
        else {
            continue;
        };
        let shape = InferredRecordShape::infer(&samples)
            .map_err(|error| format!("SOURCE_DRIFT_SHAPE_INVALID: stream {stream:?}: {error}"))?;
        reports.push(StreamDrift {
            stream: stream.clone(),
            report: classify_delivery(&source_id, contract, &shape, FIXED_POLICY),
        });
    }
    Ok(reports)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_catalog_types::graph_schema::drift::{DriftAction, DriftClass};
    use medulla_catalog_types::graph_schema::repair::RecordContract;
    use medulla_graph_core::graph::{GraphSchemaSource, SchemaSourceOrigin};
    use medulla_source_types::source_ingestion::{SourceJson, SourceRecordProvenance};
    use std::sync::Arc;

    fn contract() -> RecordContract {
        serde_json::from_value(serde_json::json!({
            "fields": {
                "id": {"required": true, "types": ["integer"]},
                "name": {"required": true, "types": ["string"]},
            }
        }))
        .unwrap()
    }

    fn sources(stream: &str) -> GraphSchemaSources {
        let source = GraphSchemaSource::new(
            SchemaSourceOrigin::Approved {
                name: stream.to_string(),
                approval_lease_id: "lease".to_string(),
                shadow_report_digest: None,
            },
            Some(Arc::from("@prefix sh: <http://www.w3.org/ns/shacl#> .")),
            None,
            0,
        )
        .unwrap()
        .with_contract(contract());
        let mut dynamic = BTreeMap::new();
        dynamic.insert(format!("approved:{stream}"), source);
        GraphSchemaSources::with_dynamic(dynamic).unwrap()
    }

    fn record(stream: &str, payload: serde_json::Value) -> SourceRecord {
        SourceRecord {
            stream: ResourceId::new(stream).unwrap(),
            record_id: "r".to_string(),
            mapping_reference: "connector:c#mappings/item".to_string(),
            payload: SourceJson::new(payload).unwrap(),
            updated_at: None,
            provenance: SourceRecordProvenance {
                connector: ResourceId::new("c").unwrap(),
                adapter_kind: ResourceId::new("mcp").unwrap(),
                server: "s".to_string(),
                tool: "t".to_string(),
                tool_schema_sha256: medulla_types::contract::Digest256::sha256(b"t"),
                source_uri: "demo://r".to_string(),
            },
        }
    }

    #[test]
    fn a_stream_without_an_approved_contract_is_skipped() {
        let reports = check(
            &sources("items"),
            &[record("other", serde_json::json!({"id": 1}))],
            1000,
        )
        .unwrap();
        assert!(reports.is_empty());
    }

    #[test]
    fn a_conforming_delivery_is_compatible_and_a_breaking_one_is_quarantined() {
        let sources = sources("items");
        let ok = check(
            &sources,
            &[record("items", serde_json::json!({"id": 1, "name": "a"}))],
            1000,
        )
        .unwrap();
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].report.class, DriftClass::Compatible);
        assert_eq!(ok[0].report.action, DriftAction::Accepted);
        let bad = check(
            &sources,
            &[record("items", serde_json::json!({"name": 7}))],
            1000,
        )
        .unwrap();
        assert_eq!(bad[0].report.class, DriftClass::Breaking);
        assert_eq!(bad[0].report.action, DriftAction::Quarantined);
    }

    #[test]
    fn the_sample_never_exceeds_the_knob() {
        let sources = sources("items");
        let records = [
            record("items", serde_json::json!({"id": 1, "name": "a"})),
            record("items", serde_json::json!({"id": 1})),
        ];
        // One sample sees only the first (conforming) record.
        let one = check(&sources, &records, 1).unwrap();
        assert_eq!(one[0].report.class, DriftClass::Compatible);
        let both = check(&sources, &records, 2).unwrap();
        assert_eq!(both[0].report.class, DriftClass::Breaking);
    }
}
