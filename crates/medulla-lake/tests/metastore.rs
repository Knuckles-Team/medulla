//! Metastore catalog model, pruning, refusals and scan receipt
//! (MEDULLA-UNIFIED-DATA-PLANE-R038.4.1, .4.2.1, .4.4, .4.5).
#![cfg(feature = "metastore")]

use std::cell::Cell;
use std::collections::BTreeMap;

use medulla_lake::metastore::scan::{
    partition_filter, DataFileSource, MetastoreTableProvider, OpenedFile, PartitionPredicate,
    ScanLimits, ScanReceipt,
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
    fn list_tables(&self, _db: &str, _page: Option<&str>) -> Result<TablePage, MetastoreError> {
        Ok(TablePage {
            tables: vec![self.table.name.clone()],
            next: None,
        })
    }

    fn table(&self, _db: &str, _name: &str) -> Result<MetastoreTable, MetastoreError> {
        match self.reachable {
            true => Ok(self.table.clone()),
            false => Err(MetastoreError::SourceUnreachable {
                source_label: "https://hms.internal:9083".into(),
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
        assert!(!error.to_string().contains("hms.internal"), "{error}");
    }
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R038.4.5
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
