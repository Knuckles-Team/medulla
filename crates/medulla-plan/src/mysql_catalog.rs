//! Live MySQL and MariaDB catalog read and type map (MEDULLA-UNIFIED-DATA-PLANE-R014.2.2).
//!
//! [`read_catalog`] reads the `information_schema` of a registered source through
//! `medulla_federation::mysql_introspect` (one pool, at most `foreign.mysql.pool_size`
//! connections) and assembles the typed, hashed [`AttachedCatalogGraph`] of
//! MEDULLA-UNIFIED-DATA-PLANE-R003.1. It runs in the probe job of
//! MEDULLA-UNIFIED-DATA-PLANE-R014.2.3, never in the registration write.
//!
//! The scope is the schema list the caller allows; an empty list reads the connection's own
//! database. The type map keeps `tinyint(1)` as `boolean`, `json` as `json` (MariaDB marks a
//! JSON column with a `json_valid` check), `enum` and `set` with their variants, and `decimal`
//! with its exact precision and scale. Integer display widths are dropped so the two engines
//! name one type one way. A type the map does not know stays `opaque:<column type>`.

use std::collections::BTreeMap;
use std::fmt;

use medulla_catalog_types::attached_catalog::{
    AttachedCatalogGraph, CatalogColumn, CatalogConstraint, CatalogConstraintKind,
    CatalogReference, CatalogTable, CatalogView,
};
use medulla_catalog_types::lineage::LineageDialect;
use medulla_federation::mysql_introspect::{read_information_schema, InformationSchema};
use medulla_types::attached_source_dialect::{
    DialectDeclarationRefused, SqlEngineCaptureSupport, SqlEngineDialectEntry, SqlEngineKind,
};

/// The source cannot be reached or its catalog cannot be read.
pub const SOURCE_UNAVAILABLE: &str = "SOURCE_UNAVAILABLE";
/// The catalog read now differs from the baseline digest of the first successful probe.
pub const SCHEMA_DRIFT: &str = "SCHEMA_DRIFT";
/// The server is older than the dialect entry's floor, so change capture is not offered.
pub const CAPABILITY_UNSUPPORTED: &str = "CAPABILITY_UNSUPPORTED";

/// The change-capture mechanism of this adapter.
pub const CAPTURE_CAPABILITY: &str = "debezium";
/// `knob foreign.mysql.pool_size` bounds (connections).
pub const POOL_SIZE_BOUNDS: (u32, u32) = (1, 64);
/// `knob foreign.mysql.pool_size` default.
pub const POOL_SIZE_DEFAULT: u32 = 4;
/// Revision of the type map below.
pub const TYPE_MAP_VERSION: u32 = 1;
/// The catalog graph version this reader writes.
pub const CATALOG_VERSION: u32 = 1;

/// A typed refusal: one of the constants above and what happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogRefusal {
    pub code: &'static str,
    pub detail: String,
}

impl fmt::Display for CatalogRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for CatalogRefusal {}

fn refuse(code: &'static str, detail: impl Into<String>) -> CatalogRefusal {
    CatalogRefusal {
        code,
        detail: detail.into(),
    }
}

/// The explicit dialect entry of one engine: version floor and capture support.
pub fn dialect_entry(
    kind: SqlEngineKind,
) -> Result<SqlEngineDialectEntry, DialectDeclarationRefused> {
    let floor = match kind {
        SqlEngineKind::MySql => "5.7",
        SqlEngineKind::MariaDb => "10.3",
    };
    SqlEngineDialectEntry::declare(
        kind,
        floor,
        SqlEngineCaptureSupport {
            row_based_binlog: true,
            gtid_mode: true,
        },
        TYPE_MAP_VERSION,
    )
}

/// The engine a `SELECT VERSION()` string names: MariaDB when it contains `MariaDB`.
pub fn flavor(version: &str) -> SqlEngineKind {
    if version.contains("MariaDB") {
        SqlEngineKind::MariaDb
    } else {
        SqlEngineKind::MySql
    }
}

/// The numeric `major.minor.patch` of a version string. MariaDB may prefix `5.5.5-` for
/// replication compatibility; that prefix is not the server version.
pub fn numeric_version(version: &str) -> Vec<u64> {
    let trimmed = version.strip_prefix("5.5.5-").unwrap_or(version);
    trimmed
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .next()
        .unwrap_or("")
        .split('.')
        .filter_map(|part| part.parse().ok())
        .collect()
}

fn at_least(version: &[u64], floor: &[u64]) -> bool {
    for index in 0..floor.len().max(version.len()) {
        let have = version.get(index).copied().unwrap_or(0);
        let need = floor.get(index).copied().unwrap_or(0);
        if have != need {
            return have > need;
        }
    }
    true
}

const KNOWN_TYPES: &[&str] = &[
    "tinyint",
    "smallint",
    "mediumint",
    "int",
    "bigint",
    "decimal",
    "float",
    "double",
    "bit",
    "char",
    "varchar",
    "tinytext",
    "text",
    "mediumtext",
    "longtext",
    "binary",
    "varbinary",
    "tinyblob",
    "blob",
    "mediumblob",
    "longblob",
    "date",
    "time",
    "datetime",
    "timestamp",
    "year",
    "enum",
    "set",
    "json",
];

const INTEGER_TYPES: &[&str] = &["tinyint", "smallint", "mediumint", "int", "bigint"];

/// Map one MySQL or MariaDB column to its catalog type name.
pub fn map_type(data_type: &str, column_type: &str) -> String {
    let data_type = data_type.to_ascii_lowercase();
    let column_type = column_type.trim().to_ascii_lowercase();
    if data_type == "tinyint" && column_type.starts_with("tinyint(1)") {
        return "boolean".to_string();
    }
    if data_type == "json" {
        return "json".to_string();
    }
    if INTEGER_TYPES.contains(&data_type.as_str()) {
        let suffix = column_type
            .split_once(')')
            .map(|(_, rest)| rest.trim())
            .unwrap_or("");
        return if suffix.is_empty() {
            data_type
        } else {
            format!("{data_type} {suffix}")
        };
    }
    if KNOWN_TYPES.contains(&data_type.as_str()) {
        return column_type;
    }
    format!("opaque:{column_type}")
}

fn is_json_check(clause: &str, column: &str) -> bool {
    let clause = clause.to_ascii_lowercase();
    clause.contains(&format!("json_valid(`{}`)", column.to_ascii_lowercase()))
}

/// Assemble the typed catalog from one `information_schema` read. Pure.
pub fn build_catalog(
    schema: &InformationSchema,
    max_sql_bytes: usize,
) -> Result<MysqlCatalogRead, CatalogRefusal> {
    let engine = flavor(&schema.version);
    let entry = dialect_entry(engine).map_err(|e| refuse(CAPABILITY_UNSUPPORTED, e.reason))?;
    if !at_least(
        &numeric_version(&schema.version),
        &numeric_version(&entry.min_version),
    ) {
        return Err(refuse(
            CAPABILITY_UNSUPPORTED,
            format!(
                "{:?} {} is older than the supported floor {}",
                engine, schema.version, entry.min_version
            ),
        ));
    }
    let mut tables: BTreeMap<(String, String), CatalogTable> = schema
        .tables
        .iter()
        .map(|(s, n)| {
            (
                (s.clone(), n.clone()),
                CatalogTable {
                    schema: s.clone(),
                    name: n.clone(),
                    columns: Vec::new(),
                    constraints: Vec::new(),
                },
            )
        })
        .collect();
    for column in &schema.columns {
        let Some(table) = tables.get_mut(&(column.schema.clone(), column.table.clone())) else {
            continue; // a view's column: the view is a CatalogView
        };
        let json = schema.checks.iter().any(|(s, t, clause)| {
            s == &column.schema && t == &column.table && is_json_check(clause, &column.name)
        });
        let type_name = if json {
            "json".to_string()
        } else {
            map_type(&column.data_type, &column.column_type)
        };
        table.columns.push(CatalogColumn {
            name: column.name.clone(),
            type_name,
            nullable: column.nullable,
        });
    }
    // Keys: group by (table, constraint) in key order.
    let mut keyed: BTreeMap<(String, String, String), CatalogConstraint> = BTreeMap::new();
    for key in &schema.keys {
        let entry = keyed
            .entry((
                key.schema.clone(),
                key.table.clone(),
                key.constraint.clone(),
            ))
            .or_insert_with(|| CatalogConstraint {
                name: key.constraint.clone(),
                kind: match (&key.referenced, key.constraint.as_str()) {
                    (Some(_), _) => CatalogConstraintKind::ForeignKey,
                    (None, "PRIMARY") => CatalogConstraintKind::PrimaryKey,
                    (None, _) => CatalogConstraintKind::Unique,
                },
                columns: Vec::new(),
                references: None,
            });
        entry.columns.push(key.column.clone());
        if let Some((rs, rt, rc)) = &key.referenced {
            let reference = entry.references.get_or_insert_with(|| CatalogReference {
                schema: rs.clone(),
                table: rt.clone(),
                columns: Vec::new(),
            });
            reference.columns.push(rc.clone());
        }
    }
    for ((s, t, _), constraint) in keyed {
        if let Some(table) = tables.get_mut(&(s, t)) {
            table.constraints.push(constraint);
        }
    }
    let mut indexed: BTreeMap<(String, String, String), CatalogConstraint> = BTreeMap::new();
    for index in &schema.indexes {
        indexed
            .entry((
                index.schema.clone(),
                index.table.clone(),
                index.index.clone(),
            ))
            .or_insert_with(|| CatalogConstraint {
                name: index.index.clone(),
                kind: CatalogConstraintKind::Index,
                columns: Vec::new(),
                references: None,
            })
            .columns
            .push(index.column.clone());
    }
    for ((s, t, _), constraint) in indexed {
        if let Some(table) = tables.get_mut(&(s, t)) {
            table.constraints.push(constraint);
        }
    }
    let views = schema
        .views
        .iter()
        .map(|v| {
            CatalogView::bounded(
                &v.schema,
                &v.name,
                LineageDialect::MySql,
                &v.definition,
                max_sql_bytes,
            )
        })
        .collect();
    let graph = AttachedCatalogGraph {
        tables: tables.into_values().collect(),
        views,
        version: CATALOG_VERSION,
    };
    graph
        .validate()
        .map_err(|duplicate| refuse(SOURCE_UNAVAILABLE, duplicate.to_string()))?;
    Ok(MysqlCatalogRead {
        graph,
        engine,
        version: schema.version.clone(),
        capture: CAPTURE_CAPABILITY,
    })
}

/// What one live catalog read produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MysqlCatalogRead {
    pub graph: AttachedCatalogGraph,
    /// The flavor the server reported.
    pub engine: SqlEngineKind,
    /// The server's `VERSION()` text.
    pub version: String,
    /// The change-capture capability this adapter offers: `debezium`.
    pub capture: &'static str,
}

/// Read the live catalog of the MySQL or MariaDB source at `dsn`. The destination is checked
/// by the shared outbound gate first. `schemas` is the allowed scope (empty reads the
/// connection's own database); `pool_size` is `foreign.mysql.pool_size`, clamped to its bounds.
pub async fn read_catalog(
    dsn: &str,
    schemas: &[String],
    pool_size: u32,
) -> Result<MysqlCatalogRead, CatalogRefusal> {
    medulla_federation::federation_ssrf::check_sql_dsn(dsn)
        .map_err(|e| refuse(SOURCE_UNAVAILABLE, e))?;
    let pool_size = pool_size.clamp(POOL_SIZE_BOUNDS.0, POOL_SIZE_BOUNDS.1);
    let schema = read_information_schema(dsn, schemas, pool_size)
        .await
        .map_err(|e| refuse(SOURCE_UNAVAILABLE, e))?;
    build_catalog(
        &schema,
        medulla_federation::sql_analysis::lineage_max_sql_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_federation::mysql_introspect::{IsColumn, IsIndexColumn, IsKeyColumn, IsView};

    fn col(table: &str, name: &str, data_type: &str, column_type: &str) -> IsColumn {
        IsColumn {
            schema: "app".into(),
            table: table.into(),
            name: name.into(),
            data_type: data_type.into(),
            column_type: column_type.into(),
            nullable: false,
        }
    }

    fn fixture(version: &str) -> InformationSchema {
        InformationSchema {
            version: version.into(),
            tables: vec![
                ("app".into(), "orders".into()),
                ("app".into(), "customers".into()),
            ],
            columns: vec![
                col("customers", "id", "bigint", "bigint(20) unsigned"),
                col("customers", "flag", "tinyint", "tinyint(1)"),
                col("customers", "attrs", "longtext", "longtext"),
                col("orders", "id", "bigint", "bigint unsigned"),
                col("orders", "customer_id", "bigint", "bigint unsigned"),
                col("orders", "total", "decimal", "decimal(12,2)"),
                col("orders", "kind", "enum", "enum('a','b')"),
                col("orders", "spot", "point", "point"),
            ],
            keys: vec![
                IsKeyColumn {
                    schema: "app".into(),
                    table: "customers".into(),
                    constraint: "PRIMARY".into(),
                    column: "id".into(),
                    referenced: None,
                },
                IsKeyColumn {
                    schema: "app".into(),
                    table: "orders".into(),
                    constraint: "fk_customer".into(),
                    column: "customer_id".into(),
                    referenced: Some(("app".into(), "customers".into(), "id".into())),
                },
            ],
            indexes: vec![IsIndexColumn {
                schema: "app".into(),
                table: "orders".into(),
                index: "idx_customer".into(),
                column: "customer_id".into(),
            }],
            views: vec![IsView {
                schema: "app".into(),
                name: "v".into(),
                definition: "select 1".into(),
            }],
            checks: if version.contains("MariaDB") {
                vec![(
                    "app".into(),
                    "customers".into(),
                    "json_valid(`attrs`)".into(),
                )]
            } else {
                vec![]
            },
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R014.2.2
    #[test]
    fn the_type_map_keeps_boolean_json_enum_decimal_and_opaque_types() {
        assert_eq!(map_type("tinyint", "tinyint(1)"), "boolean");
        assert_eq!(map_type("tinyint", "tinyint(4)"), "tinyint");
        assert_eq!(map_type("bigint", "bigint(20) unsigned"), "bigint unsigned");
        assert_eq!(map_type("bigint", "bigint unsigned"), "bigint unsigned");
        assert_eq!(map_type("json", "json"), "json");
        assert_eq!(map_type("enum", "enum('a','b')"), "enum('a','b')");
        assert_eq!(map_type("decimal", "decimal(12,2)"), "decimal(12,2)");
        assert_eq!(map_type("varchar", "varchar(80)"), "varchar(80)");
        assert_eq!(map_type("point", "point"), "opaque:point");
        assert_eq!(map_type("geometry", "geometry"), "opaque:geometry");
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R014.2.2
    #[test]
    fn the_flavor_and_version_floor_follow_the_server_string() {
        assert_eq!(flavor("8.0.36"), SqlEngineKind::MySql);
        assert_eq!(
            flavor("10.11.6-MariaDB-1:10.11.6+maria~ubu2204"),
            SqlEngineKind::MariaDb
        );
        assert_eq!(numeric_version("5.5.5-10.11.6-MariaDB"), vec![10, 11, 6]);
        assert_eq!(numeric_version("8.0.36-0ubuntu0"), vec![8, 0, 36]);
        let old = build_catalog(&fixture("5.6.51"), 1_024).unwrap_err();
        assert_eq!(old.code, CAPABILITY_UNSUPPORTED);
        let old_maria = build_catalog(&fixture("10.2.44-MariaDB"), 1_024).unwrap_err();
        assert_eq!(old_maria.code, CAPABILITY_UNSUPPORTED);
        assert!(build_catalog(&fixture("5.7.44"), 1_024).is_ok());
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R014.2.2
    #[test]
    fn a_catalog_is_typed_keyed_hashed_and_names_the_debezium_capture() {
        let mysql = build_catalog(&fixture("8.0.36"), 1_024).unwrap();
        let maria = build_catalog(&fixture("10.11.6-MariaDB"), 1_024).unwrap();
        assert_eq!(mysql.engine, SqlEngineKind::MySql);
        assert_eq!(maria.engine, SqlEngineKind::MariaDb);
        assert_eq!(mysql.capture, "debezium");
        let orders = &mysql
            .graph
            .tables
            .iter()
            .find(|t| t.name == "orders")
            .unwrap();
        let types: Vec<(&str, &str)> = orders
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c.type_name.as_str()))
            .collect();
        assert_eq!(
            types,
            vec![
                ("id", "bigint unsigned"),
                ("customer_id", "bigint unsigned"),
                ("total", "decimal(12,2)"),
                ("kind", "enum('a','b')"),
                ("spot", "opaque:point"),
            ]
        );
        let fk = orders
            .constraints
            .iter()
            .find(|c| c.kind == CatalogConstraintKind::ForeignKey)
            .unwrap();
        assert_eq!(fk.references.as_ref().unwrap().table, "customers");
        assert!(orders
            .constraints
            .iter()
            .any(|c| c.kind == CatalogConstraintKind::Index && c.name == "idx_customer"));
        // MariaDB marks JSON through a check; both engines read the same typed catalog.
        let customers = |g: &MysqlCatalogRead| {
            g.graph
                .tables
                .iter()
                .find(|t| t.name == "customers")
                .unwrap()
                .columns
                .clone()
        };
        assert_eq!(customers(&maria)[2].type_name, "json");
        assert_eq!(customers(&mysql)[2].type_name, "longtext");
        assert_eq!(customers(&mysql)[1].type_name, "boolean");
        assert_eq!(customers(&mysql)[0].type_name, "bigint unsigned");
        // A reread of the same schema in another order hashes identically.
        let mut shuffled = fixture("8.0.36");
        shuffled.columns.reverse();
        shuffled.tables.reverse();
        let again = build_catalog(&shuffled, 1_024).unwrap();
        assert_eq!(again.graph.content_hash(), mysql.graph.content_hash());
        assert_eq!(mysql.graph.views.len(), 1);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R014.2.2
    #[tokio::test]
    async fn a_destination_the_outbound_gate_refuses_is_source_unavailable() {
        let refusal = read_catalog("mysql://u:p@169.254.169.254:3306/db", &[], 4)
            .await
            .unwrap_err();
        assert_eq!(refusal.code, SOURCE_UNAVAILABLE);
    }
}
