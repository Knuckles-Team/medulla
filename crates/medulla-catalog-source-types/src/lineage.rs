//! Dataset and column lineage records.
//!
//! * MEDULLA-UNIFIED-DATA-PLANE-R702: ETL runs, content-addressed dataset lineage
//!   edges and the idempotent drain of lineage events. A malformed dataset
//!   reference quarantines with a typed reason and its payload digest; it is
//!   never dropped. A replayed event adds no edge.
//! * MEDULLA-UNIFIED-DATA-PLANE-R763.1: column-level lineage edges.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A content-addressed dataset id: `ds:` plus the SHA-256 of the canonical
/// `namespace/name` reference.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DatasetId(String);

impl DatasetId {
    /// Address a dataset by its `namespace` and `name`. Both must be
    /// non-blank and free of whitespace.
    pub fn address(namespace: &str, name: &str) -> Result<Self, QuarantineReason> {
        let valid = |part: &str| !part.is_empty() && !part.chars().any(char::is_whitespace);
        if !valid(namespace) || !valid(name) {
            return Err(QuarantineReason::MalformedDatasetRef(format!(
                "{namespace:?}/{name:?}"
            )));
        }
        let digest = Sha256::digest(format!("{namespace}/{name}").as_bytes());
        Ok(Self(format!("ds:{}", hex::encode(digest))))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One ETL run that read and wrote datasets.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EtlRun {
    pub run_id: String,
    pub job: String,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
}

/// One dataset lineage edge produced by a run.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LineageEdge {
    pub dataset_in: DatasetId,
    pub dataset_out: DatasetId,
    pub run: String,
}

/// A dataset reference as it arrives on the lineage topic.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetRef {
    pub namespace: String,
    pub name: String,
}

/// One lineage event as received.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageEvent {
    pub event_id: String,
    pub run: EtlRun,
    pub inputs: Vec<DatasetRef>,
    pub outputs: Vec<DatasetRef>,
    /// The SQL statement that produced `outputs` from `inputs`; column lineage is derived
    /// from it (MEDULLA-UNIFIED-DATA-PLANE-R763.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql_text: Option<String>,
    /// The dialect of `sql_text`; Postgres when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialect: Option<LineageDialect>,
    /// The attached source whose stored catalog resolves the columns of `sql_text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_source: Option<String>,
}

impl LineageEvent {
    /// SHA-256 of the canonical JSON payload, kept on quarantine.
    pub fn payload_digest(&self) -> String {
        let bytes = serde_json::to_vec(self).unwrap_or_default();
        hex::encode(Sha256::digest(&bytes))
    }
}

/// Why an event was quarantined.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum QuarantineReason {
    MalformedDatasetRef(String),
    NoInputsOrOutputs,
    /// The statement is not exactly one parsable query (MEDULLA-UNIFIED-DATA-PLANE-R763.4).
    SqlUnparseable(String),
    /// The statement is over `knob lineage.max_sql_bytes` (MEDULLA-UNIFIED-DATA-PLANE-R763.4).
    SqlTooLarge,
}

impl QuarantineReason {
    pub fn code(&self) -> &'static str {
        match self {
            Self::MalformedDatasetRef(_) => "LINEAGE_DATASET_REF_MALFORMED",
            Self::NoInputsOrOutputs => "LINEAGE_EVENT_EMPTY",
            Self::SqlUnparseable(_) => "LINEAGE_SQL_UNPARSEABLE",
            Self::SqlTooLarge => "LINEAGE_SQL_TOO_LARGE",
        }
    }
}

/// An event that could not map, kept with its original payload digest.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuarantinedLineageEvent {
    pub event_id: String,
    pub payload_digest: String,
    pub reason: QuarantineReason,
}

fn address_all(refs: &[DatasetRef]) -> Result<Vec<DatasetId>, QuarantineReason> {
    refs.iter()
        .map(|r| DatasetId::address(&r.namespace, &r.name))
        .collect()
}

/// Map one event to its lineage edges. Pure: the same event always maps to
/// the same edges or the same quarantine reason.
pub fn map_lineage_event(event: &LineageEvent) -> Result<Vec<LineageEdge>, QuarantineReason> {
    if event.inputs.is_empty() || event.outputs.is_empty() {
        return Err(QuarantineReason::NoInputsOrOutputs);
    }
    let inputs = address_all(&event.inputs)?;
    let outputs = address_all(&event.outputs)?;
    let mut edges = Vec::with_capacity(inputs.len() * outputs.len());
    for dataset_in in &inputs {
        for dataset_out in &outputs {
            edges.push(LineageEdge {
                dataset_in: dataset_in.clone(),
                dataset_out: dataset_out.clone(),
                run: event.run.run_id.clone(),
            });
        }
    }
    Ok(edges)
}

/// The result of draining one batch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DrainBatch {
    pub new_edges: Vec<LineageEdge>,
    pub quarantined: Vec<QuarantinedLineageEvent>,
    pub replayed: usize,
}

/// Idempotent lineage drain. It remembers applied event ids and processes
/// events in batches of `batch_size`, so memory per call is bounded by the
/// batch.
#[derive(Clone, Debug)]
pub struct LineageDrain {
    batch_size: usize,
    seen_events: BTreeSet<String>,
    edges: BTreeSet<LineageEdge>,
}

impl LineageDrain {
    pub fn new(batch_size: usize) -> Self {
        Self {
            batch_size: batch_size.max(1),
            seen_events: BTreeSet::new(),
            edges: BTreeSet::new(),
        }
    }

    pub fn batch_size(&self) -> usize {
        self.batch_size
    }

    pub fn edges(&self) -> &BTreeSet<LineageEdge> {
        &self.edges
    }

    /// Drain up to `batch_size` events from the front of `pending`. A
    /// replayed event id adds no edge; an unmappable event quarantines.
    pub fn drain(&mut self, pending: &mut Vec<LineageEvent>) -> DrainBatch {
        let take = pending.len().min(self.batch_size);
        let mut out = DrainBatch::default();
        for event in pending.drain(..take) {
            if self.seen_events.contains(&event.event_id) {
                out.replayed += 1;
                continue;
            }
            match map_lineage_event(&event) {
                Ok(edges) => {
                    for edge in edges {
                        if self.edges.insert(edge.clone()) {
                            out.new_edges.push(edge);
                        }
                    }
                }
                Err(reason) => out.quarantined.push(QuarantinedLineageEvent {
                    event_id: event.event_id.clone(),
                    payload_digest: event.payload_digest(),
                    reason,
                }),
            }
            self.seen_events.insert(event.event_id);
        }
        out
    }
}

/// The refusal of a lineage event that cannot be keyed (a blank `event_id`).
pub const LINEAGE_EVENT_MALFORMED: &str = "LINEAGE_EVENT_MALFORMED";

/// The node label of one dataset in the lineage graph.
pub const DATASET_LABEL: &str = "Dataset";
/// The node label of one quarantined lineage event.
pub const QUARANTINED_LABEL: &str = "QuarantinedLineageEvent";
/// The relationship of a dataset lineage edge (input to output).
pub const DERIVES_RELATIONSHIP: &str = "derivesInto";

/// What one served `LineageIngest` committed (MEDULLA-UNIFIED-DATA-PLANE-R702.3).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LineageIngestReceipt {
    /// Events received.
    pub received: u32,
    /// Distinct dataset lineage edges in the committed write-set.
    pub edges: u32,
    /// Events kept as `QuarantinedLineageEvent` nodes.
    pub quarantined: u32,
    /// Events whose id repeated inside the request.
    pub replayed: u32,
    /// The envelope id; an identical event set replays the same id.
    pub envelope_id: String,
    /// Distinct column lineage edges derived from `sql_text` in the committed write-set
    /// (MEDULLA-UNIFIED-DATA-PLANE-R763.3).
    #[serde(default)]
    pub column_edges: u32,
    /// Columns of a statement that could not be resolved; never edges.
    #[serde(default)]
    pub column_gaps: Vec<LineageGap>,
    /// The statements kept as quarantine records (MEDULLA-UNIFIED-DATA-PLANE-R763.4).
    #[serde(default)]
    pub quarantine: Vec<QuarantinedLineageEvent>,
}

/// How an output column derives from its input column.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnTransform {
    Identity,
    Expression,
    Aggregate,
    FilterOnly,
}

/// One column lineage edge, bound to the query that produced it.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColumnLineageEdge {
    pub source_column: String,
    pub target_column: String,
    pub transform: ColumnTransform,
    /// SHA-256 of the normalized SQL text, 64 lowercase hex characters.
    pub query_digest: String,
    /// The parser that derived the edge (MEDULLA-UNIFIED-DATA-PLANE-R763.5).
    pub parser_version: String,
}

/// The SQL dialect a lineage statement is written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum LineageDialect {
    /// PostgreSQL family.
    Postgres,
    /// MySQL and MariaDB.
    MySql,
}

/// One column of a caller-supplied catalog table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LineageCatalogColumn {
    /// Column name.
    pub name: String,
}

/// One table or view of a caller-supplied catalog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LineageCatalogTable {
    /// Schema name; empty for an unqualified source.
    #[serde(default)]
    pub schema: String,
    /// Table or view name.
    pub name: String,
    /// Columns in declaration order.
    pub columns: Vec<LineageCatalogColumn>,
}

/// An output column whose source could not be resolved. Never an edge.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LineageGap {
    /// The output column (`<target>.<name>`).
    pub output: String,
    /// Why no edge exists.
    pub reason: String,
}

/// The column edges and gaps derived from one statement.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LineageResult {
    /// Edges, sorted and distinct.
    pub edges: Vec<ColumnLineageEdge>,
    /// Outputs that could not be resolved, sorted and distinct.
    pub gaps: Vec<LineageGap>,
}

/// Why a statement yields no lineage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum LineageError {
    /// The statement is larger than `knob lineage.max_sql_bytes`.
    TooLarge(usize),
    /// The text is not exactly one parsable statement.
    Parse(String),
    /// The statement is not a query.
    NotAQuery,
}

impl LineageError {
    /// The stable refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::TooLarge(_) => "LINEAGE_SQL_TOO_LARGE",
            Self::Parse(_) => "LINEAGE_SQL_UNPARSEABLE",
            Self::NotAQuery => "LINEAGE_SQL_NOT_A_QUERY",
        }
    }
}

impl std::fmt::Display for LineageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge(n) => write!(f, "{}: {n} bytes", self.code()),
            Self::Parse(m) => write!(f, "{}: {m}", self.code()),
            Self::NotAQuery => write!(f, "{}", self.code()),
        }
    }
}

impl std::error::Error for LineageError {}

/// `knob lineage.max_sql_bytes`: the largest statement parsed for lineage.
pub const LINEAGE_MAX_SQL_BYTES_KNOB: &str = "lineage.max_sql_bytes";
/// Default of `knob lineage.max_sql_bytes`.
pub const LINEAGE_MAX_SQL_BYTES_DEFAULT: usize = 1_048_576;
/// Inclusive bounds of `knob lineage.max_sql_bytes`.
pub const LINEAGE_MAX_SQL_BYTES_RANGE: (usize, usize) = (1_024, 16_777_216);

/// One served `Method::ColumnLineage` operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ColumnLineageOp {
    /// Derive the column edges of one SQL statement against a supplied catalog.
    DeriveFromSql {
        /// The statement text.
        sql: String,
        /// The statement's dialect.
        dialect: LineageDialect,
        /// The target dataset the output columns belong to.
        target: String,
        /// The catalog tables and views the statement reads.
        catalog: Vec<LineageCatalogTable>,
    },
    /// Derive the column edges of every stored view of an attached source's catalog.
    /// Reads the caller's own held catalog; commits nothing.
    DeriveFromCatalog {
        /// The attached source name.
        source: String,
    },
    /// The stored column edges reachable from `column` within `max_depth` hops.
    Impact {
        /// The column, `<schema>.<table>.<column>`.
        column: String,
        /// Which way to walk.
        direction: ImpactDirection,
        /// Hop bound; `knob lineage.impact_max_depth` when absent.
        #[serde(default)]
        max_depth: Option<u32>,
    },
}

/// The direction of an `Impact` walk over stored column edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ImpactDirection {
    /// Columns that read from `column` (what breaks if it changes).
    Downstream,
    /// Columns `column` reads from.
    Upstream,
}

/// `knob lineage.impact_max_depth`: default, inclusive bounds, hops.
pub const LINEAGE_IMPACT_MAX_DEPTH_DEFAULT: u32 = 8;
/// Inclusive bounds of `knob lineage.impact_max_depth`.
pub const LINEAGE_IMPACT_MAX_DEPTH_RANGE: (u32, u32) = (1, 32);
/// Default of `knob lineage.impact_max_columns`.
pub const LINEAGE_IMPACT_MAX_COLUMNS_DEFAULT: usize = 10_000;

/// The node label of one column in the lineage graph.
pub const COLUMN_LABEL: &str = "Column";
/// The relationship of a stored column lineage edge (output column to input column).
pub const COLUMN_DERIVES_FROM: &str = "DERIVES_FROM";

/// The graph id of one column: `col:` plus its qualified name.
pub fn column_node_id(qualified: &str) -> String {
    format!("col:{qualified}")
}

/// A quarantine reason for one statement of a lineage event.
impl From<&LineageError> for QuarantineReason {
    fn from(error: &LineageError) -> Self {
        match error {
            LineageError::TooLarge(_) => Self::SqlTooLarge,
            LineageError::Parse(detail) => Self::SqlUnparseable(detail.clone()),
            LineageError::NotAQuery => Self::SqlUnparseable("not a query".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dataset(namespace: &str, name: &str) -> DatasetRef {
        DatasetRef {
            namespace: namespace.to_string(),
            name: name.to_string(),
        }
    }

    fn event(id: &str, inputs: Vec<DatasetRef>, outputs: Vec<DatasetRef>) -> LineageEvent {
        LineageEvent {
            event_id: id.to_string(),
            run: EtlRun {
                run_id: format!("run-{id}"),
                job: "nightly".to_string(),
                started_at_ms: 1,
                ended_at_ms: Some(2),
            },
            inputs,
            outputs,
            sql_text: None,
            dialect: None,
            catalog_source: None,
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R702.1
    #[test]
    fn lineage_model_round_trips_and_addresses_by_content() {
        let a = DatasetId::address("pg://app", "public.orders").unwrap();
        assert_eq!(a, DatasetId::address("pg://app", "public.orders").unwrap());
        assert_ne!(a, DatasetId::address("pg://app", "public.items").unwrap());
        assert!(a.as_str().starts_with("ds:") && a.as_str().len() == 67);
        assert!(DatasetId::address("pg://app", "bad name").is_err());
        assert!(DatasetId::address("", "orders").is_err());

        let original = event("e1", vec![dataset("pg", "a")], vec![dataset("lake", "b")]);
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(
            serde_json::from_str::<LineageEvent>(&json).unwrap(),
            original
        );
        let edges = map_lineage_event(&original).unwrap();
        let json = serde_json::to_string(&edges).unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<LineageEdge>>(&json).unwrap(),
            edges
        );
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R702.2
    #[test]
    fn mapping_is_pure_and_drain_is_batched() {
        for size in [10usize, 1_000, 100_000] {
            let events: Vec<LineageEvent> = (0..size)
                .map(|i| {
                    event(
                        &i.to_string(),
                        vec![dataset("pg", &format!("t{i}"))],
                        vec![dataset("lake", &format!("t{i}"))],
                    )
                })
                .collect();
            assert_eq!(
                map_lineage_event(&events[0]).unwrap(),
                map_lineage_event(&events[0]).unwrap()
            );
            let mut drain = LineageDrain::new(256);
            let mut pending = events;
            let mut batches = 0;
            while !pending.is_empty() {
                let batch = drain.drain(&mut pending);
                assert!(batch.new_edges.len() <= drain.batch_size());
                batches += 1;
            }
            assert_eq!(drain.edges().len(), size);
            assert_eq!(batches, size.div_ceil(256));
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R702.4
    #[test]
    fn replay_adds_no_edge_and_malformed_ref_quarantines() {
        let mut drain = LineageDrain::new(16);
        let good = event("e1", vec![dataset("pg", "a")], vec![dataset("lake", "b")]);
        let bad = event("e2", vec![dataset("pg", "a b")], vec![dataset("lake", "b")]);
        let mut pending = vec![good.clone(), bad.clone(), good.clone()];
        let batch = drain.drain(&mut pending);
        assert_eq!(batch.new_edges.len(), 1);
        assert_eq!(batch.replayed, 1);
        assert_eq!(batch.quarantined.len(), 1);
        assert_eq!(
            batch.quarantined[0].reason.code(),
            "LINEAGE_DATASET_REF_MALFORMED"
        );
        let before = drain.edges().clone();
        let again = drain.drain(&mut vec![good]);
        assert!(again.new_edges.is_empty());
        assert_eq!(drain.edges(), &before);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R702.5
    #[test]
    fn quarantined_event_keeps_its_payload_digest() {
        let bad = event("e9", vec![], vec![dataset("lake", "b")]);
        let expected = bad.payload_digest();
        let batch = LineageDrain::new(4).drain(&mut vec![bad.clone()]);
        let record = &batch.quarantined[0];
        assert_eq!(record.payload_digest, expected);
        assert_eq!(record.reason, QuarantineReason::NoInputsOrOutputs);
        let json = serde_json::to_string(record).unwrap();
        assert_eq!(
            serde_json::from_str::<QuarantinedLineageEvent>(&json).unwrap(),
            *record
        );
        assert_eq!(expected, bad.payload_digest(), "digest is reproducible");
    }
}
