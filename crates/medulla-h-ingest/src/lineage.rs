//! Dataset lineage ingest (MEDULLA-UNIFIED-DATA-PLANE-R702.3 / R702.6).
//!
//! `handle_lineage_ingest` drains one served batch of lineage run events with
//! [`LineageDrain`] and lowers the result into ONE write-set that commits
//! through the same ChangeEnvelope authority as `EnrichDocument` -- no second
//! graph writer. Every dataset is a content-addressed `Dataset` node, every
//! edge a `derivesInto` edge from input to output, and every event that cannot
//! map one `QuarantinedLineageEvent` node keyed by its payload digest. The
//! envelope id is the write-set digest, so a replayed batch adds no edge, and
//! an edge the graph already holds is never lowered again.

use std::collections::BTreeMap;

use medulla_catalog_types::lineage::{
    column_node_id, ColumnLineageEdge, DatasetId, LineageDrain, LineageEdge, LineageGap,
    QuarantineReason, QuarantinedLineageEvent, COLUMN_DERIVES_FROM, COLUMN_LABEL, DATASET_LABEL,
    DERIVES_RELATIONSHIP, LINEAGE_EVENT_MALFORMED, QUARANTINED_LABEL,
};
pub use medulla_catalog_types::lineage::{
    LineageDialect, LineageError, LineageEvent, LineageIngestReceipt, LineageResult,
};
use medulla_protocol_types::protocol::Method;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::repository_index::{EnvelopeLabels, IndexWriteSet};

/// The labels a lineage envelope is sealed under.
pub const LABELS: EnvelopeLabels = EnvelopeLabels {
    version_type: "lineage-drain-sha256",
    policy_version: "lineage-drain",
};
const ENVELOPE_PREFIX: &str = "lineage-drain:";

fn encode(value: &Value) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec_named(value).map_err(|error| format!("lineage encoding failed: {error}"))
}

fn node(id: String, properties: Value) -> Result<Method, String> {
    Ok(Method::AddNode {
        node_id: id,
        properties_msgpack: encode(&properties)?,
    })
}

/// The quarantine node id of one event: keyed by its payload digest, so the
/// same malformed payload is kept once.
pub fn quarantine_id(quarantined: &QuarantinedLineageEvent) -> String {
    format!("lineage-quarantine:{}", quarantined.payload_digest)
}

/// Refuse a batch holding an event that cannot be keyed, before any write.
fn check_keys(events: &[LineageEvent]) -> Result<(), String> {
    match events
        .iter()
        .position(|event| event.event_id.trim().is_empty())
    {
        Some(index) => Err(format!(
            "{LINEAGE_EVENT_MALFORMED}: event {index} has a blank event_id"
        )),
        None => Ok(()),
    }
}

/// The dataset nodes of every event that maps, by id, with their reference.
fn datasets(events: &[LineageEvent]) -> BTreeMap<DatasetId, (String, String)> {
    let mut out = BTreeMap::new();
    for event in events {
        for dataset in event.inputs.iter().chain(&event.outputs) {
            if let Ok(id) = DatasetId::address(&dataset.namespace, &dataset.name) {
                out.entry(id)
                    .or_insert_with(|| (dataset.namespace.clone(), dataset.name.clone()));
            }
        }
    }
    out
}

/// Why the column lineage of one statement could not be derived.
#[derive(Debug)]
pub enum ColumnDeriveFailure {
    /// The statement itself: it is quarantined with a typed reason, never dropped.
    Statement(LineageError),
    /// The request cannot be served (for example `LINEAGE_CATALOG_MISSING`): the batch is refused.
    Refused(String),
}

/// What the column lineage derivation needs from the server, through the `QueryApi` seam
/// (MEDULLA-UNIFIED-DATA-PLANE-R763.3): the parser lives below `medulla-h-query` and is not
/// linked here.
pub struct ColumnLineageSeam<'a> {
    /// Derive the edges of `(sql, dialect, catalog source, target dataset)`.
    pub derive:
        &'a dyn Fn(&str, LineageDialect, &str, &str) -> Result<LineageResult, ColumnDeriveFailure>,
    /// Whether the graph already holds `(source_column, target_column, query_digest)`.
    pub known_column_edge: &'a dyn Fn(&str, &str, &str) -> bool,
}

/// The column edges, gaps and quarantined statements of the batch's events that carry SQL.
#[derive(Default)]
struct ColumnBatch {
    edges: Vec<ColumnLineageEdge>,
    gaps: Vec<LineageGap>,
    quarantined: Vec<QuarantinedLineageEvent>,
}

fn sql_digest(sql: &str) -> String {
    hex::encode(Sha256::digest(sql.as_bytes()))
}

/// Derive the column lineage of every first-seen event that carries `sql_text`. The target
/// of the output columns is the event's first output dataset name.
fn column_batch(
    events: &[LineageEvent],
    seam: &ColumnLineageSeam<'_>,
) -> Result<ColumnBatch, String> {
    let mut batch = ColumnBatch::default();
    let mut seen = std::collections::BTreeSet::new();
    let mut keys = std::collections::BTreeSet::new();
    for event in events {
        let Some(sql) = event.sql_text.as_deref() else {
            continue;
        };
        if !seen.insert(event.event_id.as_str()) {
            continue;
        }
        let Some(target) = event.outputs.first() else {
            continue;
        };
        let source = event.catalog_source.as_deref().unwrap_or_default();
        let dialect = event.dialect.unwrap_or(LineageDialect::Postgres);
        match (seam.derive)(sql, dialect, source, &target.name) {
            Ok(result) => {
                for edge in result.edges {
                    let key = (
                        edge.source_column.clone(),
                        edge.target_column.clone(),
                        edge.query_digest.clone(),
                    );
                    if keys.insert(key)
                        && !(seam.known_column_edge)(
                            &edge.source_column,
                            &edge.target_column,
                            &edge.query_digest,
                        )
                    {
                        batch.edges.push(edge);
                    }
                }
                batch.gaps.extend(result.gaps);
            }
            Err(ColumnDeriveFailure::Statement(error)) => {
                batch.quarantined.push(QuarantinedLineageEvent {
                    event_id: event.event_id.clone(),
                    payload_digest: sql_digest(sql),
                    reason: QuarantineReason::from(&error),
                });
            }
            Err(ColumnDeriveFailure::Refused(refusal)) => return Err(refusal),
        }
    }
    batch.gaps.sort();
    batch.gaps.dedup();
    Ok(batch)
}

/// Drain `events` and lower them into one write-set. `known_edge` answers
/// whether the graph already holds an input-to-output edge; such an edge is
/// not lowered again. Refuses `LINEAGE_EVENT_MALFORMED` before any write.
pub fn handle_lineage_ingest(
    events: Vec<LineageEvent>,
    known_edge: &dyn Fn(&str, &str) -> bool,
    columns: &ColumnLineageSeam<'_>,
) -> Result<(IndexWriteSet, LineageIngestReceipt), String> {
    check_keys(&events)?;
    let column = column_batch(&events, columns)?;
    let received = events.len();
    let nodes = datasets(&events);
    let mut drain = LineageDrain::new(received);
    let mut pending = events;
    let batch = drain.drain(&mut pending);
    let edges: Vec<&LineageEdge> = batch
        .new_edges
        .iter()
        .filter(|edge| !known_edge(edge.dataset_in.as_str(), edge.dataset_out.as_str()))
        .collect();
    let mut methods = Vec::new();
    for (id, (namespace, name)) in &nodes {
        let used = edges
            .iter()
            .any(|edge| &edge.dataset_in == id || &edge.dataset_out == id);
        if used {
            methods.push(node(
                id.as_str().to_string(),
                json!({ "type": DATASET_LABEL, "namespace": namespace, "name": name }),
            )?);
        }
    }
    for edge in &edges {
        methods.push(Method::AddEdge {
            source_id: edge.dataset_in.as_str().to_string(),
            target_id: edge.dataset_out.as_str().to_string(),
            properties_msgpack: encode(
                &json!({ "relationship": DERIVES_RELATIONSHIP, "run": edge.run }),
            )?,
        });
    }
    let mut columns_used = BTreeMap::new();
    for edge in &column.edges {
        columns_used.insert(edge.source_column.as_str(), ());
        columns_used.insert(edge.target_column.as_str(), ());
    }
    for qualified in columns_used.keys() {
        methods.push(node(
            column_node_id(qualified),
            json!({ "type": COLUMN_LABEL, "name": qualified }),
        )?);
    }
    for edge in &column.edges {
        methods.push(Method::AddEdge {
            source_id: column_node_id(&edge.target_column),
            target_id: column_node_id(&edge.source_column),
            properties_msgpack: encode(&json!({
                "relationship": COLUMN_DERIVES_FROM,
                "transform": edge.transform,
                "query_digest": edge.query_digest,
                "parser_version": edge.parser_version,
            }))?,
        });
    }
    let quarantined_all: Vec<QuarantinedLineageEvent> = batch
        .quarantined
        .iter()
        .chain(&column.quarantined)
        .cloned()
        .collect();
    for quarantined in &quarantined_all {
        methods.push(node(
            quarantine_id(quarantined),
            json!({
                "type": QUARANTINED_LABEL,
                "event_id": quarantined.event_id,
                "payload_digest": quarantined.payload_digest,
                "code": quarantined.reason.code(),
                "reason": serde_json::to_value(&quarantined.reason).unwrap_or(Value::Null),
            }),
        )?);
    }
    let write_set = IndexWriteSet::from_methods(ENVELOPE_PREFIX, b"eg/lineage-drain", methods)?;
    let receipt = LineageIngestReceipt {
        received: received as u32,
        edges: edges.len() as u32,
        quarantined: quarantined_all.len() as u32,
        replayed: batch.replayed as u32,
        envelope_id: write_set.envelope_id.clone(),
        column_edges: column.edges.len() as u32,
        column_gaps: column.gaps,
        quarantine: quarantined_all,
    };
    Ok((write_set, receipt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_catalog_types::lineage::{DatasetRef, EtlRun};

    const NO_COLUMNS: ColumnLineageSeam<'static> = ColumnLineageSeam {
        derive: &|_, _, _, _| Ok(LineageResult::default()),
        known_column_edge: &|_, _, _| false,
    };

    fn event(id: &str, input: &str, output: &str) -> LineageEvent {
        LineageEvent {
            event_id: id.into(),
            run: EtlRun {
                run_id: format!("run-{id}"),
                job: "job".into(),
                started_at_ms: 1,
                ended_at_ms: Some(2),
            },
            inputs: vec![DatasetRef {
                namespace: "ns".into(),
                name: input.into(),
            }],
            outputs: vec![DatasetRef {
                namespace: "ns".into(),
                name: output.into(),
            }],
            sql_text: None,
            dialect: None,
            catalog_source: None,
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R702.6
    #[test]
    fn replay_lowers_no_edge_and_malformed_events_quarantine() {
        let events = vec![event("a", "x", "y"), event("b", "y", "z")];
        let (first, receipt) =
            handle_lineage_ingest(events.clone(), &|_, _| false, &NO_COLUMNS).unwrap();
        assert_eq!(receipt.edges, 2);
        let (again, _) = handle_lineage_ingest(events, &|_, _| false, &NO_COLUMNS).unwrap();
        assert_eq!(first.envelope_id, again.envelope_id);
        let (held, receipt) =
            handle_lineage_ingest(vec![event("a", "x", "y")], &|_, _| true, &NO_COLUMNS).unwrap();
        assert_eq!(receipt.edges, 0);
        assert!(held.methods.is_empty());
        let (_, receipt) = handle_lineage_ingest(
            vec![event("c", "bad name", "y")],
            &|_, _| false,
            &NO_COLUMNS,
        )
        .unwrap();
        assert_eq!(receipt.quarantined, 1);
        let blank = handle_lineage_ingest(vec![event(" ", "x", "y")], &|_, _| false, &NO_COLUMNS);
        assert!(blank
            .err()
            .expect("a blank method name is refused")
            .starts_with(LINEAGE_EVENT_MALFORMED));
    }

    fn sql_event(id: &str, sql: &str) -> LineageEvent {
        LineageEvent {
            sql_text: Some(sql.into()),
            catalog_source: Some("pg".into()),
            ..event(id, "orders", "v")
        }
    }

    fn column_edge(source: &str, target: &str) -> ColumnLineageEdge {
        ColumnLineageEdge {
            source_column: source.into(),
            target_column: target.into(),
            transform: medulla_catalog_types::lineage::ColumnTransform::Identity,
            query_digest: "d".repeat(64),
            parser_version: "sqlparser-0.62".into(),
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R763.3
    #[test]
    fn column_edges_lower_once_and_skip_stored_ones() {
        let derive = |sql: &str, _: LineageDialect, source: &str, target: &str| {
            assert_eq!((source, target), ("pg", "v"));
            assert!(sql.starts_with("SELECT"));
            Ok(LineageResult {
                edges: vec![column_edge("public.orders.id", "v.id")],
                gaps: vec![],
            })
        };
        let seam = ColumnLineageSeam {
            derive: &derive,
            known_column_edge: &|_, _, _| false,
        };
        let events = vec![
            sql_event("a", "SELECT id FROM public.orders"),
            sql_event("a", "SELECT id FROM public.orders"),
        ];
        let (set, receipt) = handle_lineage_ingest(events.clone(), &|_, _| false, &seam).unwrap();
        assert_eq!(receipt.column_edges, 1);
        assert_eq!(
            set.methods
                .iter()
                .filter(
                    |m| matches!(m, Method::AddEdge { source_id, .. } if source_id == "col:v.id")
                )
                .count(),
            1
        );
        let stored = ColumnLineageSeam {
            derive: &derive,
            known_column_edge: &|_, _, _| true,
        };
        let (_, again) = handle_lineage_ingest(events, &|_, _| false, &stored).unwrap();
        assert_eq!(again.column_edges, 0);
        let refusing = ColumnLineageSeam {
            derive: &|_, _, _, _| {
                Err(ColumnDeriveFailure::Refused(
                    "LINEAGE_CATALOG_MISSING".into(),
                ))
            },
            known_column_edge: &|_, _, _| false,
        };
        let refused =
            handle_lineage_ingest(vec![sql_event("b", "SELECT 1")], &|_, _| false, &refusing);
        assert!(refused.unwrap_err().starts_with("LINEAGE_CATALOG_MISSING"));
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R763.4
    #[test]
    fn unparseable_sql_quarantines_with_its_digest_and_good_sql_still_derives() {
        let derive = |sql: &str, _: LineageDialect, _: &str, _: &str| {
            if sql.starts_with("SELEC ") {
                Err(ColumnDeriveFailure::Statement(LineageError::Parse(
                    "bad".into(),
                )))
            } else if sql.len() > 100 {
                Err(ColumnDeriveFailure::Statement(LineageError::TooLarge(
                    sql.len(),
                )))
            } else {
                Ok(LineageResult {
                    edges: vec![column_edge("public.orders.id", "v.id")],
                    gaps: vec![],
                })
            }
        };
        let seam = ColumnLineageSeam {
            derive: &derive,
            known_column_edge: &|_, _, _| false,
        };
        let bad = "SELEC id FROM public.orders";
        let big = format!("SELECT {}", "x".repeat(200));
        let events = vec![
            sql_event("bad", bad),
            sql_event("big", &big),
            sql_event("good", "SELECT id FROM public.orders"),
        ];
        let (_, receipt) = handle_lineage_ingest(events, &|_, _| false, &seam).unwrap();
        assert_eq!(receipt.column_edges, 1);
        assert_eq!(receipt.quarantined, 2);
        let record = &receipt.quarantine[0];
        assert_eq!(record.payload_digest, sql_digest(bad));
        assert_eq!(record.payload_digest.len(), 64);
        assert_eq!(record.reason.code(), "LINEAGE_SQL_UNPARSEABLE");
        assert_eq!(receipt.quarantine[1].reason.code(), "LINEAGE_SQL_TOO_LARGE");
    }
}
