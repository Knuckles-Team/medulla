//! Metastore catalog model, pruning, refusals and scan receipt
//! (MEDULLA-UNIFIED-DATA-PLANE-R038.4.1, .4.2.1, .4.2.3, .4.4, .4.5).
#![cfg(feature = "metastore")]

use std::cell::Cell;
use std::collections::BTreeMap;

use std::sync::Arc;

use arrow::array::{Int64Array, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use medulla_lake::metastore::orc::{
    scan_orc, LocalFileBytes, OrcFiles, StripeBound, StripeOp, StripePredicate,
};
use medulla_lake::metastore::scan::{
    list_all_tables, partition_filter, DataFileSource, MetastoreTableProvider, OpenedFile,
    PartitionPredicate, ScanLimits, ScanReceipt,
};
use medulla_lake::metastore::{
    FileStats, LakeFileFormat, MetastoreCatalogSpec, MetastoreClient, MetastoreColumn,
    MetastoreError, MetastoreTable, PartitionInfo, TablePage,
};
use medulla_types::contract::ServerErrorCode;

fn file(path: &str) -> FileStats {
    FileStats {
        path: path.into(),
        bytes: 100,
        rows: 10,
        column_min_max: BTreeMap::from([("id".to_string(), ("1".to_string(), "9".to_string()))]),
    }
}

fn partition(day: &str, path: &str) -> PartitionInfo {
    PartitionInfo {
        values: vec![day.into()],
        location: format!("/wh/t/day={day}"),
        files: vec![file(path)],
    }
}

fn column(name: &str, data_type: &str) -> MetastoreColumn {
    MetastoreColumn {
        name: name.into(),
        data_type: data_type.into(),
    }
}

fn sample_table(format: LakeFileFormat, paths: [&str; 3]) -> MetastoreTable {
    MetastoreTable {
        database: "db".into(),
        name: "t".into(),
        columns: vec![column("id", "bigint"), column("name", "string")],
        partition_keys: vec!["day".into()],
        format,
        location: "/wh/t".into(),
        partitions: vec![
            partition("mon", paths[0]),
            partition("tue", paths[1]),
            partition("wed", paths[2]),
        ],
    }
}

/// Recorded-response metastore: the filter is `key = 'value'` clauses joined by `and`.
struct FakeMetastore {
    table: MetastoreTable,
    reachable: bool,
}

impl FakeMetastore {
    fn new(table: MetastoreTable) -> Self {
        Self {
            table,
            reachable: true,
        }
    }
}

impl MetastoreClient for FakeMetastore {
    fn list_tables(
        &self,
        _db: &str,
        page: Option<&str>,
        page_size: u64,
    ) -> Result<TablePage, MetastoreError> {
        // Ten tables t0..t9; the cursor is the index of the first table of the page.
        let start: usize = page.map_or(0, |cursor| cursor.parse().unwrap());
        let end = (start + page_size as usize).min(10);
        Ok(TablePage {
            tables: (start..end).map(|n| format!("t{n}")).collect(),
            next: (end < 10).then(|| end.to_string()),
        })
    }

    fn table(&self, _db: &str, _name: &str) -> Result<MetastoreTable, MetastoreError> {
        match self.reachable {
            true => Ok(self.table.clone()),
            false => Err(MetastoreError::SourceUnreachable {
                source_label: "https://metastore.example.invalid:9083".into(),
            }),
        }
    }

    fn partitions_by_filter(
        &self,
        _db: &str,
        _name: &str,
        filter: &str,
    ) -> Result<Vec<PartitionInfo>, MetastoreError> {
        let wanted: Vec<String> = filter
            .split(" and ")
            .filter_map(|c| c.split_once(" = '"))
            .map(|(_, v)| v.trim_end_matches('\'').to_string())
            .collect();
        let keep = |p: &&PartitionInfo| wanted.is_empty() || wanted.contains(&p.values[0]);
        Ok(self.table.partitions.iter().filter(keep).cloned().collect())
    }
}

/// File source that counts every data-file open and serves a fixed Avro writer schema.
struct CountingFiles {
    opens: Cell<u64>,
    writer: Vec<MetastoreColumn>,
}

impl CountingFiles {
    fn new(writer: Vec<MetastoreColumn>) -> Self {
        Self {
            opens: Cell::new(0),
            writer,
        }
    }
}

impl DataFileSource for CountingFiles {
    fn writer_schema(&self, _file: &FileStats) -> Result<Vec<MetastoreColumn>, MetastoreError> {
        Ok(self.writer.clone())
    }

    fn open(&self, file: &FileStats) -> Result<OpenedFile, MetastoreError> {
        self.opens.set(self.opens.get() + 1);
        Ok(OpenedFile {
            stripes_total: 4,
            stripes_read: 1,
            bytes_read: file.bytes,
        })
    }
}

fn provider<'a>(
    client: &'a FakeMetastore,
    files: &'a CountingFiles,
    max_partitions: u64,
) -> MetastoreTableProvider<'a> {
    MetastoreTableProvider {
        client,
        files,
        source_label: "warehouse".into(),
        database: "db".into(),
        table: "t".into(),
        limits: ScanLimits {
            list_page_size: 50,
            max_partitions,
        },
    }
}

fn plain_paths() -> [&'static str; 3] {
    [
        "/wh/t/day=mon/a.orc",
        "/wh/t/day=tue/b.orc",
        "/wh/t/day=wed/c.orc",
    ]
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R038.4.1
#[test]
fn metastore_spec_round_trips() {
    let spec = MetastoreCatalogSpec {
        endpoint: "thrift://hms:9083".into(),
        database_filter: Some("db".into()),
        auth_ref: "env://HMS_TOKEN".into(),
    };
    let json = serde_json::to_string(&spec).unwrap();
    assert_eq!(
        serde_json::from_str::<MetastoreCatalogSpec>(&json).unwrap(),
        spec
    );
    assert!(spec.validate().is_ok());

    for format in [
        LakeFileFormat::Parquet,
        LakeFileFormat::Orc,
        LakeFileFormat::Avro,
    ] {
        let table = sample_table(format, plain_paths());
        let json = serde_json::to_string(&table).unwrap();
        assert_eq!(
            serde_json::from_str::<MetastoreTable>(&json).unwrap(),
            table
        );
        let hash = table.content_hash();
        assert_eq!(hash.len(), 64);
        assert!(hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    }
    assert!(serde_json::from_str::<LakeFileFormat>("\"csv\"").is_err());
    let extra = r#"{"endpoint":"e","database_filter":null,"auth_ref":"env://X","extra":1}"#;
    assert!(serde_json::from_str::<MetastoreCatalogSpec>(extra).is_err());

    let reordered = {
        let mut t = sample_table(LakeFileFormat::Orc, plain_paths());
        t.partitions.reverse();
        t.columns.reverse();
        t
    };
    assert_eq!(
        reordered.content_hash(),
        sample_table(LakeFileFormat::Orc, plain_paths()).content_hash()
    );

    for (endpoint, auth_ref) in [("", "env://X"), ("e", "hunter2"), ("e", "env://")] {
        let bad = MetastoreCatalogSpec {
            endpoint: endpoint.into(),
            database_filter: None,
            auth_ref: auth_ref.into(),
        };
        assert_eq!(
            bad.validate().unwrap_err().code(),
            ServerErrorCode::MetastoreSpecInvalid
        );
    }
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R038.4.2.1
// Proves a pushed predicate on partition keys reads only the matching partitions.
#[test]
fn partition_pruning_reads_only_matching_partitions() {
    let client = FakeMetastore::new(sample_table(LakeFileFormat::Orc, plain_paths()));
    let predicate = PartitionPredicate {
        key: "day".into(),
        value: "tue".into(),
    };
    let filter = partition_filter(&[predicate]);
    assert_eq!(filter, "day = 'tue'");
    let matched = client.partitions_by_filter("db", "t", &filter).unwrap();
    assert_eq!(
        matched
            .iter()
            .map(|p| p.values[0].as_str())
            .collect::<Vec<_>>(),
        ["tue"]
    );

    let files = CountingFiles::new(vec![]);
    let receipt = provider(&client, &files, 8).scan(&filter).unwrap();
    assert_eq!((receipt.partitions_read, receipt.partitions_pruned), (1, 2));
    assert_eq!(files.opens.get(), 1);

    let over = provider(&client, &files, 2).scan("").unwrap_err();
    assert_eq!(over.code(), ServerErrorCode::PartitionLimitExceeded);
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R038.4.2.1
// Proves metastore listing pages at the knob size.
#[test]
fn listing_pages_at_the_knob_size() {
    let client = FakeMetastore::new(sample_table(LakeFileFormat::Orc, plain_paths()));
    let limits = ScanLimits::new(3, 8);
    let first = client
        .list_tables("db", None, limits.list_page_size)
        .unwrap();
    assert_eq!(first.tables.len(), 3, "a page holds the knob size");
    assert_eq!(first.next.as_deref(), Some("3"));
    let all = list_all_tables(&client, "db", limits).unwrap();
    assert_eq!(all.len(), 10);
    assert_eq!((all[0].as_str(), all[9].as_str()), ("t0", "t9"));

    // A page over the asked size is refused.
    struct Oversized;
    impl MetastoreClient for Oversized {
        fn list_tables(
            &self,
            _db: &str,
            _page: Option<&str>,
            _page_size: u64,
        ) -> Result<TablePage, MetastoreError> {
            Ok(TablePage {
                tables: vec!["a".into(), "b".into()],
                next: None,
            })
        }
        fn table(&self, _: &str, _: &str) -> Result<MetastoreTable, MetastoreError> {
            unreachable!("listing reads no table")
        }
        fn partitions_by_filter(
            &self,
            _: &str,
            _: &str,
            _: &str,
        ) -> Result<Vec<PartitionInfo>, MetastoreError> {
            unreachable!("listing reads no partition")
        }
    }
    let refused = list_all_tables(&Oversized, "db", ScanLimits::new(1, 8)).unwrap_err();
    assert_eq!(refused.code(), ServerErrorCode::MetastoreSpecInvalid);
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R038.4.4
#[test]
fn metastore_refusals() {
    let delta_only = [
        "/wh/t/delta_0001/a.orc",
        "/wh/t/delta_0002/b.orc",
        "/wh/t/delete_delta_3/c.orc",
    ];
    let avro = sample_table(LakeFileFormat::Avro, plain_paths());
    let mut unreachable = FakeMetastore::new(avro.clone());
    unreachable.reachable = false;
    let cases = [
        (
            FakeMetastore::new(avro),
            vec![column("id", "string"), column("name", "string")],
            ServerErrorCode::SchemaIncompatible,
        ),
        (
            FakeMetastore::new(sample_table(LakeFileFormat::Orc, delta_only)),
            vec![],
            ServerErrorCode::UnsupportedTableLayout,
        ),
        (unreachable, vec![], ServerErrorCode::SourceUnreachable),
    ];
    for (client, writer, code) in cases {
        let files = CountingFiles::new(writer);
        let error = provider(&client, &files, 8).scan("").unwrap_err();
        assert_eq!(error.code(), code);
        assert_eq!(files.opens.get(), 0, "{code} read a data file");
        assert!(
            !error.to_string().contains("metastore.example.invalid"),
            "{error}"
        );
    }
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R038.4.5
// Proves the scan receipt counts partitions, files, stripes and bytes exactly as the scan opened them.
#[test]
fn receipt_counts_match() {
    let client = FakeMetastore::new(sample_table(LakeFileFormat::Orc, plain_paths()));
    let files = CountingFiles::new(vec![]);
    let receipt = provider(&client, &files, 8)
        .scan("day = 'mon' and day = 'wed'")
        .unwrap();
    assert_eq!(files.opens.get(), 2);
    assert_eq!(
        receipt,
        ScanReceipt {
            partitions_pruned: 1,
            partitions_read: 2,
            files_read: 2,
            stripes_read: 2,
            stripes_skipped: 6,
            bytes_read: 200,
        }
    );
}

/// Writes an ORC file of three stripes: ids 1..=100, 101..=200 and 201..=300.
fn write_three_stripe_orc(path: &std::path::Path) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("name", DataType::Utf8, false),
    ]));
    let file = std::fs::File::create(path).unwrap();
    let mut writer = orc_rust::arrow_writer::ArrowWriterBuilder::new(file, schema.clone())
        .try_build()
        .unwrap();
    for (stripe, prefix) in [(0i64, "a"), (1, "m"), (2, "x")] {
        let ids: Vec<i64> = (stripe * 100 + 1..=stripe * 100 + 100).collect();
        let names: Vec<String> = ids.iter().map(|i| format!("{prefix}{i}")).collect();
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(ids)),
                Arc::new(StringArray::from(names)),
            ],
        )
        .unwrap();
        writer.write(&batch).unwrap();
        writer.flush_stripe().unwrap();
    }
    writer.close().unwrap();
}

fn id_at_least(value: i64) -> StripePredicate {
    StripePredicate {
        column: "id".into(),
        op: StripeOp::Ge,
        value: StripeBound::Int(value),
    }
}

// proves R038.4.2.2: ORC scans skip stripes by stripe statistics
// spec: MEDULLA-UNIFIED-DATA-PLANE-R038.4.2.2
#[test]
fn orc_stripe_pruning_skips_non_matching_stripes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("facts.orc");
    write_three_stripe_orc(&path);
    let bytes = bytes::Bytes::from(std::fs::read(&path).unwrap());

    // No predicate: every stripe is read.
    let all = scan_orc(bytes.clone(), &[], &[]).unwrap();
    assert_eq!((all.opened.stripes_total, all.opened.stripes_read), (3, 3));
    let rows = |scan: &medulla_lake::metastore::orc::OrcScan| -> usize {
        scan.batches.iter().map(|b| b.num_rows()).sum()
    };
    assert_eq!(rows(&all), 300);

    // id in [150, 160]: only the middle stripe can hold a match.
    let middle = scan_orc(
        bytes.clone(),
        &[
            id_at_least(150),
            StripePredicate {
                column: "id".into(),
                op: StripeOp::Le,
                value: StripeBound::Int(160),
            },
        ],
        &["id".to_string()],
    )
    .unwrap();
    assert_eq!(
        (middle.opened.stripes_total, middle.opened.stripes_read),
        (3, 1)
    );
    assert_eq!(rows(&middle), 100);
    // Only the projected column is decoded.
    assert!(middle.batches.iter().all(|b| b.num_columns() == 1));
    for batch in &middle.batches {
        let ids = batch
            .column_by_name("id")
            .unwrap()
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert!(ids.iter().flatten().all(|id| (101..=200).contains(&id)));
    }
    assert!(middle.opened.bytes_read < all.opened.bytes_read);

    // A text equality prunes by the string bounds.
    let by_name = scan_orc(
        bytes.clone(),
        &[StripePredicate {
            column: "name".into(),
            op: StripeOp::Eq,
            value: StripeBound::Text("x250".into()),
        }],
        &[],
    )
    .unwrap();
    assert_eq!(by_name.opened.stripes_read, 1);

    // A value above every stripe skips all of them and decodes nothing.
    let none = scan_orc(bytes, &[id_at_least(1000)], &[]).unwrap();
    assert_eq!((none.opened.stripes_read, none.opened.bytes_read), (0, 0));
    assert!(none.batches.is_empty());

    // Through the scan: the receipt counts the skipped stripes of the file it opened.
    let client = FakeMetastore::new(sample_table(
        LakeFileFormat::Orc,
        [path.to_str().unwrap(), "/wh/none/b.orc", "/wh/none/c.orc"],
    ));
    let reader = LocalFileBytes {
        source_label: "warehouse".into(),
    };
    let files = OrcFiles {
        bytes: &reader,
        predicates: vec![id_at_least(250)],
        projection: vec![],
    };
    let receipt = MetastoreTableProvider {
        client: &client,
        files: &files,
        source_label: "warehouse".into(),
        database: "db".into(),
        table: "t".into(),
        limits: ScanLimits {
            list_page_size: 50,
            max_partitions: 8,
        },
    }
    .scan("day = 'mon'")
    .unwrap();
    assert_eq!(
        (
            receipt.files_read,
            receipt.stripes_read,
            receipt.stripes_skipped
        ),
        (1, 1, 2)
    );
}

/// Writes an Avro file in an older writer schema: `id` as an Avro `int`, `name` as a string,
/// and no `email` field.
fn write_old_schema_avro() -> Vec<u8> {
    let schema = apache_avro::Schema::parse_str(
        r#"{"type":"record","name":"t","fields":[
            {"name":"id","type":"int"},
            {"name":"name","type":"string"}]}"#,
    )
    .unwrap();
    let mut writer = apache_avro::Writer::new(&schema, Vec::new()).unwrap();
    for (id, name) in [(1, "ada"), (2, "grace")] {
        let mut record = apache_avro::types::Record::new(&schema).unwrap();
        record.put("id", id);
        record.put("name", name);
        writer.append(record).unwrap();
    }
    writer.into_inner().unwrap()
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R038.4.2.3
#[test]
fn avro_reader_schema_projection() {
    use medulla_lake::metastore::avro::{avro_schema, read_avro, AvroFiles};
    use medulla_lake::metastore::scan::RowFileSource;

    let bytes = write_old_schema_avro();
    assert_eq!(
        avro_schema(&bytes).unwrap(),
        vec![column("id", "int"), column("name", "string")]
    );

    // The reader schema wins: `id` is promoted to bigint and the added optional `email` is null.
    let reader = vec![
        column("id", "bigint"),
        column("name", "string"),
        column("email", "string"),
    ];
    let (batch, opened) = read_avro(&bytes, &reader, &[]).unwrap();
    assert_eq!(batch.num_rows(), 2);
    assert_eq!(batch.schema().field(0).data_type(), &DataType::Int64);
    let ids = batch
        .column(0)
        .as_any()
        .downcast_ref::<Int64Array>()
        .unwrap();
    assert_eq!(ids.values().to_vec(), vec![1, 2]);
    let email = batch.column_by_name("email").unwrap();
    assert_eq!(email.null_count(), 2);
    assert_eq!(opened.bytes_read, bytes.len() as u64);
    assert_eq!(opened.stripes_total, 0);

    // A projection returns only the named reader columns.
    let (projected, _) = read_avro(&bytes, &reader, &["email".to_string()]).unwrap();
    assert_eq!(projected.num_columns(), 1);
    assert_eq!(projected.column(0).null_count(), 2);

    // A writer type the reader type cannot promote from is refused before any row decodes.
    let narrowing = vec![column("name", "bigint")];
    let refused = read_avro(&bytes, &narrowing, &[]).unwrap_err();
    assert_eq!(refused.code(), ServerErrorCode::SchemaIncompatible);

    // Through the port: one file read whole, rows in the reader schema.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("facts.avro");
    std::fs::write(&path, &bytes).unwrap();
    let local = LocalFileBytes {
        source_label: "hms".into(),
    };
    let files = AvroFiles {
        bytes: &local,
        reader_schema: reader,
        projection: vec![],
    };
    let (batches, opened) = RowFileSource::read(&files, &file(path.to_str().unwrap())).unwrap();
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].num_columns(), 3);
    assert_eq!(opened.bytes_read, bytes.len() as u64);
}
