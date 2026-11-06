//! Real, spec-compliant Iceberg v2 **Avro** manifest + manifest-list writer
//! (CONCEPT:MEDULLA-KG.storage.medulla-iceberg-avro-manifest manifest / CONCEPT:MEDULLA-KG.storage.iceberg-manifest-list manifest-list).
//!
//! The Iceberg table spec stores the file-list layer as **Avro** containers, not JSON:
//! a snapshot's `manifest-list` (an Avro file, one `manifest_file` record per manifest)
//! points at one or more **manifest** files (Avro, one `manifest_entry` record per data
//! file). [`crate::iceberg`] writes the pure-JSON `metadata.json` and points its
//! snapshot at [`crate::iceberg::manifest_list_path`]; THIS module materializes the real
//! Avro bytes at that exact path (+ the manifest file it references), so a stock Iceberg
//! reader (Spark / Trino / DuckDB) that follows `manifest-list` finds real, parseable
//! Avro and can read the engine's Iceberg tables end-to-end — no longer a stub.
//!
//! Gated behind the `lake` feature (needs the pure-Rust `apache-avro` codec), the same
//! gate the Parquet transcode lives behind, so the Pi build links no Avro (MEDULLA-317).
//!
//! ## Spec scope (honest)
//! * Format-version **2** manifests: `manifest_entry` carries `status`, `snapshot_id`,
//!   the (null-inherited) `sequence_number` / `file_sequence_number`, and a `data_file`
//!   with `content`, `file_path`, `file_format`, `partition`, `record_count`,
//!   `file_size_in_bytes`. Iceberg **field-ids** are embedded on every Avro field, and
//!   the required manifest metadata keys (`schema`, `schema-id`, `partition-spec`,
//!   `partition-spec-id`, `format-version`, `content`) sit in the Avro file header.
//! * The `manifest_file` records carry the v2 `content` / `sequence_number` /
//!   `min_sequence_number` / added·existing·deleted file & row counts.
//! * **Per-column stats (CONCEPT:MEDULLA-KG.storage.iceberg-avro-manifest-carries):** the `data_file` now carries the Iceberg
//!   stats maps — `column_sizes` (108), `value_counts` (109), `null_value_counts`
//!   (110), `nan_value_counts` (137), `lower_bounds` (125) and `upper_bounds` (128),
//!   each an Avro `logicalType: map` (array of key/value records) keyed by the column's
//!   Iceberg field-id. Bounds are the column min/max serialized to Iceberg single-value
//!   **binary** (little-endian scalars; raw UTF-8 for strings). Gathered as the file is
//!   materialized ([`crate::parquet_io::materialize_with_column_stats`]) and carried on
//!   the [`FileEntry`]; a reader (Spark/Trino) uses the bounds to **skip whole files**.
//!   A file recorded without stats (bare `record_file`) emits the maps as null.
//! * **Partition `field_summary`:** an unpartitioned table (`partition-spec` = `[]`)
//!   has ZERO `manifest_file.partitions` entries, emitted as null, NOT stubbed. A
//!   table identity-partitioned by one column (MEDULLA-UNIFIED-DATA-PLANE-R039.4)
//!   writes one `field_summary` (`contains_null`, `lower_bound`, `upper_bound`)
//!   over the live files' stats of that column, so a reader prunes the manifest.
//! * Container codec is Avro **null** (uncompressed) — universally readable.

use apache_avro::types::Value as AvroValue;
use apache_avro::{Reader, Schema, Writer};

use std::cmp::Ordering;

use crate::iceberg::{
    iceberg_schema, manifest_file_path, manifest_list_path, partition_spec_fields,
    FIRST_PARTITION_FIELD_ID,
};
use crate::schema::{CellValue, LakeField, LakeSchema, LakeType};
use crate::snapshot::{ColumnStat, FileEntry, Lsn, SnapshotLog};

/// Iceberg v2 `manifest_entry` Avro schema (CONCEPT:MEDULLA-KG.storage.medulla-iceberg-avro-manifest + per-column stats MEDULLA-350).
/// Field-ids per the spec; the `data_file` carries the unpartitioned projection medulla-lake
/// tracks PLUS the six Iceberg stats maps (`logicalType: map`, keyed by column field-id)
/// that drive external predicate pushdown / file skipping.
const MANIFEST_ENTRY_SCHEMA: &str = r#"{
  "type": "record",
  "name": "manifest_entry",
  "fields": [
    {"name": "status", "type": "int", "field-id": 0},
    {"name": "snapshot_id", "type": ["null", "long"], "default": null, "field-id": 1},
    {"name": "sequence_number", "type": ["null", "long"], "default": null, "field-id": 3},
    {"name": "file_sequence_number", "type": ["null", "long"], "default": null, "field-id": 4},
    {"name": "data_file", "type": {
      "type": "record",
      "name": "r2",
      "fields": [
        {"name": "content", "type": "int", "field-id": 134},
        {"name": "file_path", "type": "string", "field-id": 100},
        {"name": "file_format", "type": "string", "field-id": 101},
        {"name": "partition", "type": {"type": "record", "name": "r102", "fields": []}, "field-id": 102},
        {"name": "record_count", "type": "long", "field-id": 103},
        {"name": "file_size_in_bytes", "type": "long", "field-id": 104},
        {"name": "column_sizes", "type": ["null", {"type": "array", "logicalType": "map", "items": {
          "type": "record", "name": "k117_v118", "fields": [
            {"name": "key", "type": "int", "field-id": 117},
            {"name": "value", "type": "long", "field-id": 118}
          ]}}], "default": null, "field-id": 108},
        {"name": "value_counts", "type": ["null", {"type": "array", "logicalType": "map", "items": {
          "type": "record", "name": "k119_v120", "fields": [
            {"name": "key", "type": "int", "field-id": 119},
            {"name": "value", "type": "long", "field-id": 120}
          ]}}], "default": null, "field-id": 109},
        {"name": "null_value_counts", "type": ["null", {"type": "array", "logicalType": "map", "items": {
          "type": "record", "name": "k121_v122", "fields": [
            {"name": "key", "type": "int", "field-id": 121},
            {"name": "value", "type": "long", "field-id": 122}
          ]}}], "default": null, "field-id": 110},
        {"name": "nan_value_counts", "type": ["null", {"type": "array", "logicalType": "map", "items": {
          "type": "record", "name": "k138_v139", "fields": [
            {"name": "key", "type": "int", "field-id": 138},
            {"name": "value", "type": "long", "field-id": 139}
          ]}}], "default": null, "field-id": 137},
        {"name": "lower_bounds", "type": ["null", {"type": "array", "logicalType": "map", "items": {
          "type": "record", "name": "k126_v127", "fields": [
            {"name": "key", "type": "int", "field-id": 126},
            {"name": "value", "type": "bytes", "field-id": 127}
          ]}}], "default": null, "field-id": 125},
        {"name": "upper_bounds", "type": ["null", {"type": "array", "logicalType": "map", "items": {
          "type": "record", "name": "k129_v130", "fields": [
            {"name": "key", "type": "int", "field-id": 129},
            {"name": "value", "type": "bytes", "field-id": 130}
          ]}}], "default": null, "field-id": 128}
      ]
    }, "field-id": 2}
  ]
}"#;

/// Iceberg v2 `manifest_file` Avro schema for the manifest list (CONCEPT:MEDULLA-KG.storage.iceberg-manifest-list).
const MANIFEST_FILE_SCHEMA: &str = r#"{
  "type": "record",
  "name": "manifest_file",
  "fields": [
    {"name": "manifest_path", "type": "string", "field-id": 500},
    {"name": "manifest_length", "type": "long", "field-id": 501},
    {"name": "partition_spec_id", "type": "int", "field-id": 502},
    {"name": "content", "type": "int", "field-id": 517},
    {"name": "sequence_number", "type": "long", "field-id": 515},
    {"name": "min_sequence_number", "type": "long", "field-id": 516},
    {"name": "added_snapshot_id", "type": "long", "field-id": 503},
    {"name": "added_files_count", "type": "int", "field-id": 504},
    {"name": "existing_files_count", "type": "int", "field-id": 505},
    {"name": "deleted_files_count", "type": "int", "field-id": 506},
    {"name": "added_rows_count", "type": "long", "field-id": 512},
    {"name": "existing_rows_count", "type": "long", "field-id": 513},
    {"name": "deleted_rows_count", "type": "long", "field-id": 514},
    {"name": "partitions", "type": ["null", {"type": "array", "element-id": 508, "items": {
      "type": "record",
      "name": "field_summary",
      "fields": [
        {"name": "contains_null", "type": "boolean", "field-id": 509},
        {"name": "contains_nan", "type": ["null", "boolean"], "default": null, "field-id": 518},
        {"name": "lower_bound", "type": ["null", "bytes"], "default": null, "field-id": 510},
        {"name": "upper_bound", "type": ["null", "bytes"], "default": null, "field-id": 511}
      ]
    }}], "default": null, "field-id": 507}
  ]
}"#;

/// The real Avro Iceberg manifest artifacts for one committed snapshot
/// (CONCEPT:MEDULLA-KG.storage.medulla-iceberg-avro-manifest/MEDULLA-334). The caller persists both byte blobs to the object store at
/// their respective paths (already the paths `metadata.json` references).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IcebergManifests {
    /// Object-store path of the manifest Avro file (matches the manifest-list entry).
    pub manifest_path: String,
    /// Spec-compliant Avro bytes of the manifest file (one entry per live data file).
    pub manifest_avro: Vec<u8>,
    /// Object-store path of the manifest-list Avro file (matches `metadata.json`'s
    /// snapshot `manifest-list`).
    pub manifest_list_path: String,
    /// Spec-compliant Avro bytes of the manifest list (one `manifest_file` record).
    pub manifest_list_avro: Vec<u8>,
    /// The Iceberg snapshot id these manifests describe (derived from the LSN).
    pub snapshot_id: i64,
    /// Number of ADDED data files listed (files this snapshot's commit introduced).
    pub added_files: usize,
    /// Total record count across the ADDED data files.
    pub added_rows: u64,
    /// Number of EXISTING data files listed (live files carried from earlier snapshots).
    pub existing_files: usize,
    /// Total record count across the EXISTING data files.
    pub existing_rows: u64,
    /// Number of DELETED data files listed (files this snapshot's commit retired:
    /// compaction and copy-on-write deletion).
    pub deleted_files: usize,
    /// Total record count across the DELETED data files.
    pub deleted_rows: u64,
}

/// The status a data file carries in a manifest entry (Iceberg v2 `manifest_entry.status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryStatus {
    /// Live and introduced by an earlier snapshot.
    Existing,
    /// Introduced by this snapshot's commit.
    Added,
    /// Retired by this snapshot's commit.
    Deleted,
}

impl EntryStatus {
    fn code(self) -> i32 {
        match self {
            EntryStatus::Existing => 0,
            EntryStatus::Added => 1,
            EntryStatus::Deleted => 2,
        }
    }
}

/// Classify every file that belongs in the manifest of the snapshot committed at
/// `lsn`: live files are ADDED when this commit introduced them and EXISTING
/// otherwise; files this commit retired are DELETED; files retired by an earlier
/// commit are not listed. Append, compaction and copy-on-write deletion all fall out
/// of this one rule (a rewrite is DELETED old files plus ADDED new files).
fn classify_files(snapshot: &SnapshotLog, lsn: Lsn) -> Vec<(&FileEntry, EntryStatus)> {
    snapshot
        .all_files()
        .iter()
        .filter_map(|file| match file.removed_at {
            None if file.added_at == lsn => Some((file, EntryStatus::Added)),
            None => Some((file, EntryStatus::Existing)),
            Some(removed) if removed == lsn => Some((file, EntryStatus::Deleted)),
            Some(_) => None,
        })
        .collect()
}

/// Refuse a snapshot the manifest writer cannot publish faithfully
/// (`MANIFEST_PUBLICATION_INVALID`, with a reason). Nothing is written on refusal.
fn validate_publication(snapshot: &SnapshotLog) -> Result<(), String> {
    let refuse = |reason: String| {
        Err(format!(
            "{}: {reason}",
            crate::catalog::MANIFEST_PUBLICATION_INVALID
        ))
    };
    for file in snapshot.all_files() {
        if file.path.is_empty() {
            return refuse("a data file has an empty path".to_string());
        }
        if file
            .removed_at
            .is_some_and(|removed| removed < file.added_at)
        {
            return refuse(format!(
                "data file {} is retired before it was added",
                file.path
            ));
        }
        if let Some(stats) = &file.column_stats {
            if let Some(stat) = stats
                .iter()
                .find(|stat| stat.value_count != file.num_rows as i64)
            {
                return refuse(format!(
                    "data file {} column {} counts {} values for {} rows",
                    file.path, stat.field_id, stat.value_count, file.num_rows
                ));
            }
        }
    }
    Ok(())
}

/// Serialize a scalar to Iceberg **single-value binary** (CONCEPT:MEDULLA-KG.storage.iceberg-avro-manifest-carries) — the encoding
/// `lower_bounds` / `upper_bounds` values carry. Per the Iceberg spec: `long` / `timestamptz`
/// as little-endian 8-byte, `double` as little-endian IEEE-754 8-byte, `boolean` as one
/// byte (`0x00`/`0x01`), `string` as raw UTF-8 (no length prefix). Bounds are emitted
/// untruncated, which is always spec-valid (truncation is only a size optimization).
fn iceberg_binary(v: &CellValue) -> Vec<u8> {
    match v {
        CellValue::Long(x) => x.to_le_bytes().to_vec(),
        CellValue::Timestamp(x) => x.to_le_bytes().to_vec(),
        CellValue::Double(x) => x.to_le_bytes().to_vec(),
        CellValue::Bool(b) => vec![u8::from(*b)],
        CellValue::String(s) => s.as_bytes().to_vec(),
        CellValue::Null => Vec::new(),
    }
}

/// One entry of an Iceberg `logicalType: map` field: an Avro `{key, value}` record with
/// an `int` key (the column field-id) and the given value (CONCEPT:MEDULLA-KG.storage.iceberg-avro-manifest-carries).
fn map_entry(field_id: i32, value: AvroValue) -> AvroValue {
    AvroValue::Record(vec![
        ("key".into(), AvroValue::Int(field_id)),
        ("value".into(), value),
    ])
}

/// Wrap a built map as the `["null", array]` union value the schema declares
/// (CONCEPT:MEDULLA-KG.storage.iceberg-avro-manifest-carries): union branch 1 (the array) when any entry exists, else branch 0
/// (null) so a reader sees the optional stat as genuinely absent.
fn map_field(entries: Vec<AvroValue>) -> AvroValue {
    if entries.is_empty() {
        AvroValue::Union(0, Box::new(AvroValue::Null))
    } else {
        AvroValue::Union(1, Box::new(AvroValue::Array(entries)))
    }
}

/// Build the six Iceberg stats-map field values for a `data_file` record from a file's
/// per-column stats (CONCEPT:MEDULLA-KG.storage.iceberg-avro-manifest-carries). Emits `column_sizes` / `value_counts` /
/// `null_value_counts` / `nan_value_counts` (int→long maps) and `lower_bounds` /
/// `upper_bounds` (int→bytes maps), each keyed by the column's Iceberg field-id. A file
/// with no gathered stats yields six null unions.
fn stats_fields(stats: Option<&Vec<ColumnStat>>) -> [(String, AvroValue); 6] {
    let (mut sizes, mut values, mut nulls, mut nans, mut lowers, mut uppers) =
        (vec![], vec![], vec![], vec![], vec![], vec![]);
    if let Some(cols) = stats {
        for c in cols {
            if let Some(sz) = c.column_size {
                sizes.push(map_entry(c.field_id, AvroValue::Long(sz)));
            }
            values.push(map_entry(c.field_id, AvroValue::Long(c.value_count)));
            nulls.push(map_entry(c.field_id, AvroValue::Long(c.null_count)));
            nans.push(map_entry(c.field_id, AvroValue::Long(c.nan_count)));
            if let Some(lo) = &c.lower {
                lowers.push(map_entry(c.field_id, AvroValue::Bytes(iceberg_binary(lo))));
            }
            if let Some(hi) = &c.upper {
                uppers.push(map_entry(c.field_id, AvroValue::Bytes(iceberg_binary(hi))));
            }
        }
    }
    [
        ("column_sizes".into(), map_field(sizes)),
        ("value_counts".into(), map_field(values)),
        ("null_value_counts".into(), map_field(nulls)),
        ("nan_value_counts".into(), map_field(nans)),
        ("lower_bounds".into(), map_field(lowers)),
        ("upper_bounds".into(), map_field(uppers)),
    ]
}

/// Build one `manifest_entry` Avro value for a live data file (CONCEPT:MEDULLA-KG.storage.medulla-iceberg-avro-manifest). A
/// newly-added file leaves `sequence_number` / `file_sequence_number` null so the reader
/// inherits the manifest's sequence number, per the v2 inheritance rule. The `data_file`
/// carries the per-column Iceberg stats maps (CONCEPT:MEDULLA-KG.storage.iceberg-avro-manifest-carries) when they were gathered.
fn manifest_entry_value(
    location: &str,
    f: &FileEntry,
    status: EntryStatus,
    snapshot_id: i64,
    partition: Option<(&LakeField, i32)>,
) -> AvroValue {
    let mut data_file = vec![
        ("content".into(), AvroValue::Int(0)), // 0 = DATA
        (
            "file_path".into(),
            AvroValue::String(format!("{location}/{}", f.path)),
        ),
        ("file_format".into(), AvroValue::String("PARQUET".into())),
        ("partition".into(), partition_value(f, partition)),
        ("record_count".into(), AvroValue::Long(f.num_rows as i64)),
        (
            "file_size_in_bytes".into(),
            AvroValue::Long(f.size_bytes as i64),
        ),
    ];
    data_file.extend(stats_fields(f.column_stats.as_ref()));
    let data_file = AvroValue::Record(data_file);
    // An ADDED file belongs to this snapshot and inherits the manifest sequence
    // number (null). An EXISTING or DELETED file carries the snapshot and data
    // sequence number it was originally added under (v2 requires them explicit).
    let (entry_snapshot, sequence) = match status {
        EntryStatus::Added => (snapshot_id, None),
        EntryStatus::Existing => (f.added_at.value() as i64, Some(f.added_at.value() as i64)),
        EntryStatus::Deleted => (snapshot_id, Some(f.added_at.value() as i64)),
    };
    let sequence_value = |sequence: Option<i64>| match sequence {
        Some(n) => AvroValue::Union(1, Box::new(AvroValue::Long(n))),
        None => AvroValue::Union(0, Box::new(AvroValue::Null)),
    };
    AvroValue::Record(vec![
        ("status".into(), AvroValue::Int(status.code())),
        (
            "snapshot_id".into(),
            AvroValue::Union(1, Box::new(AvroValue::Long(entry_snapshot))),
        ),
        ("sequence_number".into(), sequence_value(sequence)),
        ("file_sequence_number".into(), sequence_value(sequence)),
        ("data_file".into(), data_file),
    ])
}

/// Write the Avro **manifest** file listing every live data file (CONCEPT:MEDULLA-KG.storage.medulla-iceberg-avro-manifest),
/// with the required Iceberg manifest metadata in the container header.
fn write_manifest(
    schema: &LakeSchema,
    schema_id: i32,
    location: &str,
    files: &[(&FileEntry, EntryStatus)],
    snapshot_id: i64,
) -> Result<Vec<u8>, String> {
    let partition = schema.partition_source();
    let avro_schema = Schema::parse_str(&manifest_entry_schema(partition))
        .map_err(|e| format!("manifest schema: {e}"))?;
    let mut writer =
        Writer::new(&avro_schema, Vec::new()).map_err(|e| format!("manifest avro writer: {e}"))?;

    // Required Iceberg manifest metadata (lands in the Avro file header). A reader uses
    // these to bind field-ids and inherit sequence numbers. `schema-id` is the table's
    // CURRENT schema-id (CONCEPT:MEDULLA-KG.storage.iceberg-per-file-schema-id, INT-P2-4) -- real Iceberg declares the
    // manifest's schema at the MANIFEST level (one schema per manifest file), so this
    // is spec-correct; a live file that predates a later evolution still carries ITS
    // OWN schema-id on the JSON manifest preview (`crate::iceberg::build_iceberg`) even
    // though this single Avro manifest's header names only the current one.
    let ib_schema = iceberg_schema(schema, schema_id).to_string();
    let schema_id_str = schema_id.to_string();
    let partition_spec = partition_spec_fields(Some(schema)).to_string();
    let meta: [(&str, &str); 6] = [
        ("schema", ib_schema.as_str()),
        ("schema-id", schema_id_str.as_str()),
        ("partition-spec", partition_spec.as_str()),
        ("partition-spec-id", "0"),
        ("format-version", "2"),
        ("content", "data"),
    ];
    for (k, v) in meta {
        writer
            .add_user_metadata(k.to_string(), v)
            .map_err(|e| format!("manifest metadata {k}: {e}"))?;
    }

    for (f, status) in files {
        writer
            .append_value(manifest_entry_value(
                location,
                f,
                *status,
                snapshot_id,
                partition,
            ))
            .map_err(|e| format!("append manifest_entry: {e}"))?;
    }
    writer
        .into_inner()
        .map_err(|e| format!("manifest avro: {e}"))
}

/// File and row counts per entry status, for the manifest list record.
#[derive(Default)]
struct ManifestCounts {
    added_files: usize,
    added_rows: u64,
    existing_files: usize,
    existing_rows: u64,
    deleted_files: usize,
    deleted_rows: u64,
}

/// Write the Avro **manifest list** with one `manifest_file` record pointing at the
/// manifest just written (CONCEPT:MEDULLA-KG.storage.iceberg-manifest-list).
fn write_manifest_list(
    manifest_path: &str,
    manifest_len: i64,
    snapshot_id: i64,
    min_sequence_number: i64,
    counts: &ManifestCounts,
    partitions: AvroValue,
) -> Result<Vec<u8>, String> {
    let avro_schema = Schema::parse_str(MANIFEST_FILE_SCHEMA)
        .map_err(|e| format!("manifest_file schema: {e}"))?;
    let mut writer = Writer::new(&avro_schema, Vec::new())
        .map_err(|e| format!("manifest_list avro writer: {e}"))?;

    let record = AvroValue::Record(vec![
        (
            "manifest_path".into(),
            AvroValue::String(manifest_path.to_string()),
        ),
        ("manifest_length".into(), AvroValue::Long(manifest_len)),
        ("partition_spec_id".into(), AvroValue::Int(0)),
        ("content".into(), AvroValue::Int(0)), // 0 = data manifest
        ("sequence_number".into(), AvroValue::Long(snapshot_id)),
        (
            "min_sequence_number".into(),
            AvroValue::Long(min_sequence_number),
        ),
        ("added_snapshot_id".into(), AvroValue::Long(snapshot_id)),
        (
            "added_files_count".into(),
            AvroValue::Int(counts.added_files as i32),
        ),
        (
            "existing_files_count".into(),
            AvroValue::Int(counts.existing_files as i32),
        ),
        (
            "deleted_files_count".into(),
            AvroValue::Int(counts.deleted_files as i32),
        ),
        (
            "added_rows_count".into(),
            AvroValue::Long(counts.added_rows as i64),
        ),
        (
            "existing_rows_count".into(),
            AvroValue::Long(counts.existing_rows as i64),
        ),
        (
            "deleted_rows_count".into(),
            AvroValue::Long(counts.deleted_rows as i64),
        ),
        // One `field_summary` per partition-spec field (MEDULLA-UNIFIED-DATA-PLANE-R039.4);
        // an unpartitioned spec has none, so the list is null (NOT stubbed).
        ("partitions".into(), partitions),
    ]);
    writer
        .append_value(record)
        .map_err(|e| format!("append manifest_file: {e}"))?;
    writer
        .into_inner()
        .map_err(|e| format!("manifest_list avro: {e}"))
}

/// Materialize the real Iceberg Avro manifest + manifest list for the current snapshot
/// (CONCEPT:MEDULLA-KG.storage.medulla-iceberg-avro-manifest/MEDULLA-334). The paths match exactly what [`crate::iceberg::build_iceberg`]
/// references from `metadata.json`, so a committed snapshot resolves to real Avro. The
/// caller persists both byte blobs to the object store.
pub fn build_iceberg_manifests(
    schema: &LakeSchema,
    schema_id: i32,
    snapshot: &SnapshotLog,
    location: &str,
) -> Result<IcebergManifests, String> {
    let lsn: Lsn = snapshot.current_lsn();
    let snapshot_id: i64 = lsn.value() as i64;
    validate_publication(snapshot)?;
    let files = classify_files(snapshot, lsn);
    let mut counts = ManifestCounts::default();
    for (file, status) in &files {
        match status {
            EntryStatus::Added => {
                counts.added_files += 1;
                counts.added_rows += file.num_rows;
            }
            EntryStatus::Existing => {
                counts.existing_files += 1;
                counts.existing_rows += file.num_rows;
            }
            EntryStatus::Deleted => {
                counts.deleted_files += 1;
                counts.deleted_rows += file.num_rows;
            }
        }
    }
    let min_sequence_number = files
        .iter()
        .map(|(file, _)| file.added_at.value() as i64)
        .min()
        .unwrap_or(snapshot_id);

    let manifest_path = manifest_file_path(location, snapshot_id);
    let manifest_list_path = manifest_list_path(location, snapshot_id);

    let manifest_avro = write_manifest(schema, schema_id, location, &files, snapshot_id)?;
    let manifest_list_avro = write_manifest_list(
        &manifest_path,
        manifest_avro.len() as i64,
        snapshot_id,
        min_sequence_number,
        &counts,
        partition_summaries(schema.partition_source(), &files),
    )?;

    Ok(IcebergManifests {
        manifest_path,
        manifest_avro,
        manifest_list_path,
        manifest_list_avro,
        snapshot_id,
        added_files: counts.added_files,
        added_rows: counts.added_rows,
        existing_files: counts.existing_files,
        existing_rows: counts.existing_rows,
        deleted_files: counts.deleted_files,
        deleted_rows: counts.deleted_rows,
    })
}

/// Parse an Avro container's records back to values (CONCEPT:MEDULLA-KG.storage.medulla-iceberg-avro-manifest) — used by the
/// round-trip test and any reader that wants to verify manifest bytes.
pub fn read_avro_records(bytes: &[u8]) -> Result<Vec<AvroValue>, String> {
    let reader = Reader::new(bytes).map_err(|e| format!("avro open: {e}"))?;
    let mut out = Vec::new();
    for rec in reader {
        out.push(rec.map_err(|e| format!("avro record: {e}"))?);
    }
    Ok(out)
}

/// One decoded `manifest_entry` (MEDULLA-FEDERATED-QUERY-R079.2.1): the fields an
/// acceptance check compares against the snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestEntrySummary {
    /// `0` EXISTING, `1` ADDED, `2` DELETED.
    pub status: i32,
    /// The entry's snapshot id.
    pub snapshot_id: Option<i64>,
    /// The explicit data sequence number (`None` when inherited).
    pub sequence_number: Option<i64>,
    /// The data file path.
    pub file_path: String,
    /// The data file row count.
    pub record_count: i64,
    /// The data file size in bytes.
    pub file_size_in_bytes: i64,
}

/// One decoded `manifest_file` record of a manifest list
/// (MEDULLA-FEDERATED-QUERY-R079.2.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestListSummary {
    /// The manifest file path.
    pub manifest_path: String,
    /// The manifest file length in bytes.
    pub manifest_length: i64,
    /// The snapshot that added the manifest.
    pub added_snapshot_id: i64,
    /// ADDED, EXISTING and DELETED file counts.
    pub file_counts: [i32; 3],
    /// ADDED, EXISTING and DELETED row counts.
    pub row_counts: [i64; 3],
    /// One field summary per partition-spec field; empty when unpartitioned
    /// (MEDULLA-UNIFIED-DATA-PLANE-R039.4).
    pub partitions: Vec<PartitionFieldSummary>,
}

/// One decoded `field_summary` of a manifest list entry
/// (MEDULLA-UNIFIED-DATA-PLANE-R039.4). Bounds are Iceberg single-value binary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartitionFieldSummary {
    pub contains_null: bool,
    pub contains_nan: Option<bool>,
    pub lower_bound: Option<Vec<u8>>,
    pub upper_bound: Option<Vec<u8>>,
}

impl PartitionFieldSummary {
    /// Whether a manifest with this summary may hold rows whose partition value
    /// equals `value` (MEDULLA-UNIFIED-DATA-PLANE-R039.4). `false` means a reader
    /// prunes the whole manifest. Missing bounds never prune.
    pub fn may_contain(&self, ty: LakeType, value: &CellValue) -> bool {
        if matches!(value, CellValue::Null) {
            return self.contains_null;
        }
        let bound = |bytes: &Option<Vec<u8>>| bytes.as_deref().and_then(|b| decode_binary(ty, b));
        let (Some(lower), Some(upper)) = (bound(&self.lower_bound), bound(&self.upper_bound))
        else {
            return true;
        };
        !matches!(cell_order(value, &lower), Some(Ordering::Less))
            && !matches!(cell_order(value, &upper), Some(Ordering::Greater))
    }
}

/// Decode an Iceberg single-value binary back to a cell of type `ty`.
fn decode_binary(ty: LakeType, bytes: &[u8]) -> Option<CellValue> {
    let eight = || <[u8; 8]>::try_from(bytes).ok();
    Some(match ty {
        LakeType::Long => CellValue::Long(i64::from_le_bytes(eight()?)),
        LakeType::Timestamp => CellValue::Timestamp(i64::from_le_bytes(eight()?)),
        LakeType::Double => CellValue::Double(f64::from_le_bytes(eight()?)),
        LakeType::Bool => CellValue::Bool(*bytes.first()? != 0),
        LakeType::String => CellValue::String(String::from_utf8(bytes.to_vec()).ok()?),
    })
}

/// The order of two cells of the same type; `None` across types or for NaN.
fn cell_order(a: &CellValue, b: &CellValue) -> Option<Ordering> {
    match (a, b) {
        (CellValue::Long(x), CellValue::Long(y))
        | (CellValue::Timestamp(x), CellValue::Timestamp(y)) => Some(x.cmp(y)),
        (CellValue::Double(x), CellValue::Double(y)) => x.partial_cmp(y),
        (CellValue::Bool(x), CellValue::Bool(y)) => Some(x.cmp(y)),
        (CellValue::String(x), CellValue::String(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

/// The Avro type of an identity partition value of `ty`.
fn partition_avro_type(ty: LakeType) -> &'static str {
    match ty {
        LakeType::Long => r#""long""#,
        LakeType::Double => r#""double""#,
        LakeType::Bool => r#""boolean""#,
        LakeType::String => r#""string""#,
        LakeType::Timestamp => {
            r#"{"type": "long", "logicalType": "timestamp-micros", "adjust-to-utc": true}"#
        }
    }
}

/// The `manifest_entry` schema with the `data_file.partition` record of the spec:
/// empty when unpartitioned (byte-identical to before), else one optional field
/// with the partition field id.
fn manifest_entry_schema(partition: Option<(&LakeField, i32)>) -> String {
    let Some((field, _)) = partition else {
        return MANIFEST_ENTRY_SCHEMA.to_string();
    };
    let record = format!(
        r#"{{"type": "record", "name": "r102", "fields": [{{"name": {}, "type": ["null", {}], "default": null, "field-id": {FIRST_PARTITION_FIELD_ID}}}]}}"#,
        serde_json::Value::String(field.name.clone()),
        partition_avro_type(field.ty),
    );
    MANIFEST_ENTRY_SCHEMA.replacen(
        r#"{"type": "record", "name": "r102", "fields": []}"#,
        &record,
        1,
    )
}

/// The file's column stat for the partition source field.
fn source_stat(f: &FileEntry, source_id: i32) -> Option<&ColumnStat> {
    f.column_stats
        .as_ref()?
        .iter()
        .find(|stat| stat.field_id == source_id)
}

/// The identity partition value of one data file: its single value when the
/// file's bounds agree, else null.
fn partition_value(f: &FileEntry, partition: Option<(&LakeField, i32)>) -> AvroValue {
    let Some((field, source_id)) = partition else {
        return AvroValue::Record(vec![]);
    };
    let single = source_stat(f, source_id).and_then(|stat| match (&stat.lower, &stat.upper) {
        (Some(lower), Some(upper)) if lower == upper && stat.null_count == 0 => Some(lower),
        _ => None,
    });
    let value = match single {
        Some(CellValue::Long(x)) => AvroValue::Union(1, Box::new(AvroValue::Long(*x))),
        Some(CellValue::Timestamp(x)) => {
            AvroValue::Union(1, Box::new(AvroValue::TimestampMicros(*x)))
        }
        Some(CellValue::Double(x)) => AvroValue::Union(1, Box::new(AvroValue::Double(*x))),
        Some(CellValue::Bool(x)) => AvroValue::Union(1, Box::new(AvroValue::Boolean(*x))),
        Some(CellValue::String(x)) => AvroValue::Union(1, Box::new(AvroValue::String(x.clone()))),
        Some(CellValue::Null) | None => AvroValue::Union(0, Box::new(AvroValue::Null)),
    };
    AvroValue::Record(vec![(field.name.clone(), value)])
}

/// The manifest list `partitions` value (MEDULLA-UNIFIED-DATA-PLANE-R039.4): null
/// when unpartitioned, else one `field_summary` over the live files' stats of the
/// partition column. A live file without stats makes the bounds unknown (null)
/// and `contains_null` true, so a reader never prunes on a guess.
fn partition_summaries(
    partition: Option<(&LakeField, i32)>,
    files: &[(&FileEntry, EntryStatus)],
) -> AvroValue {
    let Some((field, source_id)) = partition else {
        return AvroValue::Union(0, Box::new(AvroValue::Null));
    };
    let mut contains_null = false;
    let mut contains_nan = false;
    let mut unknown = false;
    let mut lower: Option<CellValue> = None;
    let mut upper: Option<CellValue> = None;
    for (file, status) in files {
        if matches!(status, EntryStatus::Deleted) {
            continue;
        }
        let Some(stat) = source_stat(file, source_id) else {
            unknown = true;
            continue;
        };
        contains_null |= stat.null_count > 0;
        contains_nan |= stat.nan_count > 0;
        if let Some(lo) = &stat.lower {
            if lower
                .as_ref()
                .is_none_or(|cur| cell_order(lo, cur) == Some(Ordering::Less))
            {
                lower = Some(lo.clone());
            }
        }
        if let Some(hi) = &stat.upper {
            if upper
                .as_ref()
                .is_none_or(|cur| cell_order(hi, cur) == Some(Ordering::Greater))
            {
                upper = Some(hi.clone());
            }
        }
    }
    if unknown {
        contains_null = true;
        lower = None;
        upper = None;
    }
    let optional_bytes = |cell: Option<CellValue>| match cell {
        Some(cell) => AvroValue::Union(1, Box::new(AvroValue::Bytes(iceberg_binary(&cell)))),
        None => AvroValue::Union(0, Box::new(AvroValue::Null)),
    };
    let nan = if matches!(field.ty, LakeType::Double) && !unknown {
        AvroValue::Union(1, Box::new(AvroValue::Boolean(contains_nan)))
    } else {
        AvroValue::Union(0, Box::new(AvroValue::Null))
    };
    let summary = AvroValue::Record(vec![
        ("contains_null".into(), AvroValue::Boolean(contains_null)),
        ("contains_nan".into(), nan),
        ("lower_bound".into(), optional_bytes(lower)),
        ("upper_bound".into(), optional_bytes(upper)),
    ]);
    AvroValue::Union(1, Box::new(AvroValue::Array(vec![summary])))
}

/// Decode the `partitions` union of a manifest list record.
fn decode_partitions(value: AvroValue) -> Result<Vec<PartitionFieldSummary>, String> {
    // `take_field` already unwrapped the `["null", array]` union.
    let items = match value {
        AvroValue::Null => return Ok(Vec::new()),
        AvroValue::Array(items) => items,
        other => return Err(format!("partitions is not an array: {other:?}")),
    };
    let optional_bytes = |value: AvroValue| match value {
        AvroValue::Bytes(bytes) => Some(bytes),
        _ => None,
    };
    items
        .into_iter()
        .map(|item| {
            let mut fields = record_fields(item)?;
            let contains_null = match take_field(&mut fields, "contains_null")? {
                AvroValue::Boolean(flag) => flag,
                other => return Err(format!("contains_null is not a boolean: {other:?}")),
            };
            let contains_nan = match take_field(&mut fields, "contains_nan")? {
                AvroValue::Boolean(flag) => Some(flag),
                _ => None,
            };
            Ok(PartitionFieldSummary {
                contains_null,
                contains_nan,
                lower_bound: optional_bytes(take_field(&mut fields, "lower_bound")?),
                upper_bound: optional_bytes(take_field(&mut fields, "upper_bound")?),
            })
        })
        .collect()
}

fn record_fields(value: AvroValue) -> Result<Vec<(String, AvroValue)>, String> {
    match value {
        AvroValue::Record(fields) => Ok(fields),
        other => Err(format!("expected an avro record, got {other:?}")),
    }
}

fn take_field(fields: &mut Vec<(String, AvroValue)>, name: &str) -> Result<AvroValue, String> {
    let at = fields
        .iter()
        .position(|(field, _)| field == name)
        .ok_or_else(|| format!("avro record has no field {name}"))?;
    let mut value = fields.swap_remove(at).1;
    while let AvroValue::Union(_, inner) = value {
        value = *inner;
    }
    Ok(value)
}

fn as_long(value: AvroValue, name: &str) -> Result<Option<i64>, String> {
    match value {
        AvroValue::Null => Ok(None),
        AvroValue::Long(n) => Ok(Some(n)),
        AvroValue::Int(n) => Ok(Some(i64::from(n))),
        other => Err(format!("field {name} is not an integer: {other:?}")),
    }
}

fn required_long(fields: &mut Vec<(String, AvroValue)>, name: &str) -> Result<i64, String> {
    as_long(take_field(fields, name)?, name)?.ok_or_else(|| format!("field {name} is null"))
}

/// Decode every `manifest_entry` of a manifest Avro file
/// (MEDULLA-FEDERATED-QUERY-R079.2.1).
pub fn read_manifest_entries(bytes: &[u8]) -> Result<Vec<ManifestEntrySummary>, String> {
    read_avro_records(bytes)?
        .into_iter()
        .map(|record| {
            let mut fields = record_fields(record)?;
            let status = required_long(&mut fields, "status")? as i32;
            let snapshot_id = as_long(take_field(&mut fields, "snapshot_id")?, "snapshot_id")?;
            let sequence_number = as_long(
                take_field(&mut fields, "sequence_number")?,
                "sequence_number",
            )?;
            let mut data_file = record_fields(take_field(&mut fields, "data_file")?)?;
            let file_path = match take_field(&mut data_file, "file_path")? {
                AvroValue::String(path) => path,
                other => return Err(format!("file_path is not a string: {other:?}")),
            };
            Ok(ManifestEntrySummary {
                status,
                snapshot_id,
                sequence_number,
                file_path,
                record_count: required_long(&mut data_file, "record_count")?,
                file_size_in_bytes: required_long(&mut data_file, "file_size_in_bytes")?,
            })
        })
        .collect()
}

/// Decode every `manifest_file` record of a manifest-list Avro file
/// (MEDULLA-FEDERATED-QUERY-R079.2.1).
pub fn read_manifest_list(bytes: &[u8]) -> Result<Vec<ManifestListSummary>, String> {
    read_avro_records(bytes)?
        .into_iter()
        .map(|record| {
            let mut fields = record_fields(record)?;
            let manifest_path = match take_field(&mut fields, "manifest_path")? {
                AvroValue::String(path) => path,
                other => return Err(format!("manifest_path is not a string: {other:?}")),
            };
            Ok(ManifestListSummary {
                manifest_path,
                manifest_length: required_long(&mut fields, "manifest_length")?,
                added_snapshot_id: required_long(&mut fields, "added_snapshot_id")?,
                file_counts: [
                    required_long(&mut fields, "added_files_count")? as i32,
                    required_long(&mut fields, "existing_files_count")? as i32,
                    required_long(&mut fields, "deleted_files_count")? as i32,
                ],
                row_counts: [
                    required_long(&mut fields, "added_rows_count")?,
                    required_long(&mut fields, "existing_rows_count")?,
                    required_long(&mut fields, "deleted_rows_count")?,
                ],
                partitions: decode_partitions(take_field(&mut fields, "partitions")?)?,
            })
        })
        .collect()
}
