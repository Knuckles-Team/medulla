//! Metadata-only discovery of a registered foreign source
//! (MEDULLA-UNIFIED-DATA-PLANE-R038.2.1).
//!
//! Registration of a source writes one [`MetadataContract`]: the source kind, the source name, a
//! schema version and the SHA-256 of the source's canonical, sorted schema bytes. A discoverer
//! reads catalog metadata only. It is handed a spec and a name and has no row-reading surface,
//! so a discovered contract can never carry entity rows.
//!
//! Which discoverer runs is decided by [`discoverer_for`], an exhaustive `match` over
//! [`SourceCategory`]: a new category fails to compile until it names a discoverer.
//! * PostgreSQL-wire and MySQL-family SQL sources read their live catalog (`information_schema`)
//!   through the same readers the catalog probe uses; a source that cannot be reached refuses with
//!   `DISCOVERY_UNREACHABLE` and nothing is registered.
//! * Every other kind hashes the schema it declares at registration (columns, field map, id and
//!   score fields, query text). Credentials, endpoints and DSNs are never part of the hash.

use std::future::Future;
use std::pin::Pin;

use medulla_catalog_source_types::attached_catalog::AttachedCatalogGraph;
use medulla_lake::metastore::scan::{list_all_tables, ScanLimits};
use medulla_lake::metastore::{MetastoreClient, MetastoreError};
use medulla_query_types::wire::ForeignSourceSpec;
use medulla_types::contract::ServerErrorCode;
use sha2::{Digest, Sha256};
use {
    medulla_catalog_source_types::metadata_acceleration::MetadataContract,
    medulla_catalog_source_types::metadata_acceleration::SourceCategory,
};

/// Discovery could not produce a contract; the registration is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveryRefusal {
    /// `DISCOVERY_UNREACHABLE`.
    pub code: ServerErrorCode,
    /// The source the refusal names.
    pub source: String,
    /// Why, without any DSN or credential.
    pub detail: String,
}

impl std::fmt::Display for DiscoveryRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: source '{}' {}", self.code, self.source, self.detail)
    }
}

impl std::error::Error for DiscoveryRefusal {}

/// The future a discoverer returns.
pub type DiscoveryFuture<'a> =
    Pin<Box<dyn Future<Output = Result<String, DiscoveryRefusal>> + Send + 'a>>;

/// One source category's discovery: the lowercase hex SHA-256 of the source's canonical schema.
pub trait SourceDiscoverer: Sync {
    fn discover<'a>(&'a self, name: &'a str, spec: &'a ForeignSourceSpec) -> DiscoveryFuture<'a>;
}

/// Discovery from the schema the registration declares. No network, no row read.
struct DeclaredShape;

impl SourceDiscoverer for DeclaredShape {
    fn discover<'a>(&'a self, _name: &'a str, spec: &'a ForeignSourceSpec) -> DiscoveryFuture<'a> {
        let hash = hash_lines(&declared_shape(spec));
        Box::pin(async move { Ok(hash) })
    }
}

/// Discovery from the live catalog of a SQL source.
struct LiveSqlCatalog;

impl SourceDiscoverer for LiveSqlCatalog {
    fn discover<'a>(&'a self, name: &'a str, spec: &'a ForeignSourceSpec) -> DiscoveryFuture<'a> {
        Box::pin(async move { live_catalog_hash(name, spec).await })
    }
}

#[cfg(feature = "federation-sql")]
async fn live_catalog_hash(
    name: &str,
    spec: &ForeignSourceSpec,
) -> Result<String, DiscoveryRefusal> {
    let ForeignSourceSpec::Sql { dsn, .. } = spec else {
        return Ok(hash_lines(&declared_shape(spec)));
    };
    let pool = medulla_server_api::knob_config::knobs().foreign_mysql_pool_size as u32;
    let read = if is_postgres(dsn) {
        medulla_plan::postgres_catalog::read_catalog(dsn, &[], pool).await
    } else {
        medulla_plan::mysql_catalog::read_catalog(dsn, &[], pool)
            .await
            .map(|read| read.graph)
    };
    match read {
        Ok(graph) => Ok(catalog_content_hash(&graph)),
        Err(_) => Err(DiscoveryRefusal {
            code: ServerErrorCode::DiscoveryUnreachable,
            source: name.to_string(),
            detail: "catalog could not be read".to_string(),
        }),
    }
}

#[cfg(not(feature = "federation-sql"))]
async fn live_catalog_hash(
    _name: &str,
    spec: &ForeignSourceSpec,
) -> Result<String, DiscoveryRefusal> {
    Ok(hash_lines(&declared_shape(spec)))
}

fn is_postgres(dsn: &str) -> bool {
    dsn.starts_with("postgres://") || dsn.starts_with("postgresql://")
}

/// Discovery of a metastore catalog: its tables' content hashes, read from the catalog only
/// (never a data file). The hash folds each `database.table` with `MetastoreTable::content_hash`.
struct MetastoreDiscoverer;

impl SourceDiscoverer for MetastoreDiscoverer {
    fn discover<'a>(&'a self, name: &'a str, spec: &'a ForeignSourceSpec) -> DiscoveryFuture<'a> {
        Box::pin(async move {
            let unreachable = |_| DiscoveryRefusal {
                code: ServerErrorCode::DiscoveryUnreachable,
                source: name.to_string(),
                detail: "catalog could not be read".to_string(),
            };
            let ForeignSourceSpec::Metastore {
                endpoint,
                database_filter,
                ..
            } = spec
            else {
                return Ok(hash_lines(&declared_shape(spec)));
            };
            let client = super::hive_sources::hms_client(name, endpoint, database_filter.clone());
            let page = medulla_server_api::knob_config::knobs().metastore_list_page_size;
            let limits = ScanLimits::new(page, u64::MAX);
            let lines =
                tokio::task::spawn_blocking(move || -> Result<Vec<String>, MetastoreError> {
                    let mut lines = Vec::new();
                    for db in client.databases()? {
                        for table in list_all_tables(&client, &db, limits)? {
                            let description = client.table(&db, &table)?;
                            lines
                                .push(format!("table {db}.{table} {}", description.content_hash()));
                        }
                    }
                    Ok(lines)
                })
                .await
                .map_err(|_| DiscoveryRefusal {
                    code: ServerErrorCode::DiscoveryUnreachable,
                    source: name.to_string(),
                    detail: "catalog read did not finish".to_string(),
                })?
                .map_err(unreachable)?;
            Ok(hash_lines(&lines))
        })
    }
}

/// The one discoverer of a category. Exhaustive: no wildcard arm.
pub fn discoverer_for(category: SourceCategory) -> &'static dyn SourceDiscoverer {
    match category {
        SourceCategory::PostgresWire | SourceCategory::RelationalSql => &LiveSqlCatalog,
        SourceCategory::HadoopCatalog => &MetastoreDiscoverer,
        SourceCategory::OltpRowStore
        | SourceCategory::OlapColumnar
        | SourceCategory::TimeSeries
        | SourceCategory::SearchIndex
        | SourceCategory::DocumentStore
        | SourceCategory::WebMetasearch
        | SourceCategory::KeyValueStore
        | SourceCategory::DataLakeTables
        | SourceCategory::SparkEngine
        | SourceCategory::ArrowFlightSql
        | SourceCategory::PropertyGraph
        | SourceCategory::ObjectStorage => &DeclaredShape,
    }
}

/// The category a registered spec belongs to (the coverage matrix of the plan).
pub fn category_of(spec: &ForeignSourceSpec) -> SourceCategory {
    match spec {
        ForeignSourceSpec::Sql { dsn, .. } if is_postgres(dsn) => SourceCategory::PostgresWire,
        ForeignSourceSpec::Sql { .. } => SourceCategory::RelationalSql,
        ForeignSourceSpec::SqliteFile { .. } => SourceCategory::OltpRowStore,
        ForeignSourceSpec::Trino { .. } => SourceCategory::OlapColumnar,
        ForeignSourceSpec::SparkBatch { .. } => SourceCategory::SparkEngine,
        ForeignSourceSpec::Metastore { .. } => SourceCategory::HadoopCatalog,
        // The coverage matrix files the warehouse and lake sources (R020.2.2) under OLAP and
        // columnar.
        ForeignSourceSpec::Warehouse { .. } | ForeignSourceSpec::ClickHouse { .. } => {
            SourceCategory::OlapColumnar
        }
        ForeignSourceSpec::RemoteEngine { .. }
        | ForeignSourceSpec::Cypher { .. }
        | ForeignSourceSpec::BrainGuarded { .. } => SourceCategory::PropertyGraph,
        ForeignSourceSpec::Mcp { .. } => SourceCategory::WebMetasearch,
        ForeignSourceSpec::HttpJson { .. }
        | ForeignSourceSpec::Api { .. }
        | ForeignSourceSpec::A2a { .. }
        | ForeignSourceSpec::EventStream { .. }
        | ForeignSourceSpec::GraphQl { .. }
        | ForeignSourceSpec::Named { .. } => SourceCategory::DocumentStore,
    }
}

/// Discover `spec` registered as `name`: the contract to write, at `prior`'s schema version
/// when the hash is unchanged and one version higher when it changed.
pub async fn discover_source(
    name: &str,
    spec: &ForeignSourceSpec,
    prior: Option<&MetadataContract>,
) -> Result<MetadataContract, DiscoveryRefusal> {
    let category = category_of(spec);
    let content_hash = discoverer_for(category).discover(name, spec).await?;
    let schema_version = match prior {
        None => 1,
        Some(prior) if prior.content_hash == content_hash => prior.schema_version,
        Some(prior) => prior.schema_version.saturating_add(1),
    };
    Ok(MetadataContract {
        source_kind: serde_json::to_value(category)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default(),
        source_name: name.to_string(),
        schema_version,
        content_hash,
    })
}

fn hash_lines(lines: &[String]) -> String {
    let mut digest = Sha256::new();
    for line in lines {
        digest.update((line.len() as u64).to_be_bytes());
        digest.update(line.as_bytes());
    }
    hex::encode(digest.finalize())
}

/// SHA-256 over the canonical bytes of a catalog: tables, columns, constraints and views sorted.
pub fn catalog_content_hash(graph: &AttachedCatalogGraph) -> String {
    let mut lines = vec![format!("version={}", graph.version)];
    let mut tables: Vec<_> = graph.tables.iter().collect();
    tables.sort_by(|a, b| (&a.schema, &a.name).cmp(&(&b.schema, &b.name)));
    for table in tables {
        lines.push(format!("table {}.{}", table.schema, table.name));
        let mut columns: Vec<_> = table.columns.iter().collect();
        columns.sort_by(|a, b| a.name.cmp(&b.name));
        for column in columns {
            lines.push(format!(
                "column {} {} {}",
                column.name, column.type_name, column.nullable
            ));
        }
        let mut constraints: Vec<_> = table.constraints.iter().collect();
        constraints.sort();
        for constraint in constraints {
            lines.push(format!("constraint {constraint:?}"));
        }
    }
    let mut views: Vec<_> = graph.views.iter().collect();
    views.sort_by(|a, b| (&a.schema, &a.name).cmp(&(&b.schema, &b.name)));
    for view in views {
        lines.push(format!(
            "view {}.{} {:?} {} {}",
            view.schema, view.name, view.dialect, view.definition_gap, view.definition_sql
        ));
    }
    hash_lines(&lines)
}

fn sorted(values: &[String]) -> String {
    let mut values = values.to_vec();
    values.sort();
    values.join(",")
}

/// The shape a spec declares, as lines: never an endpoint, DSN, secret or request context.
fn declared_shape(spec: &ForeignSourceSpec) -> Vec<String> {
    let score = |score: &Option<String>| format!("score={}", score.as_deref().unwrap_or(""));
    match spec {
        ForeignSourceSpec::RemoteEngine {
            graph,
            uql,
            cypher,
            id_field,
            ..
        } => vec![
            "remote_engine".into(),
            format!("graph={graph}"),
            format!("uql={uql}"),
            format!("cypher={cypher}"),
            format!("id={id_field}"),
        ],
        ForeignSourceSpec::HttpJson {
            json_path,
            field_map,
            ..
        } => {
            let mut lines = vec![
                "http_json".into(),
                format!("json_path={json_path}"),
                format!("id={}", field_map.id),
                score(&field_map.score),
            ];
            lines.extend(
                field_map
                    .columns
                    .iter()
                    .map(|(column, field)| format!("column {column}={field}")),
            );
            lines
        }
        ForeignSourceSpec::Sql {
            query,
            id_field,
            score_field,
            columns,
            ..
        } => vec![
            "sql".into(),
            format!("query={query}"),
            format!("id={id_field}"),
            score(score_field),
            format!("columns={}", sorted(columns)),
        ],
        ForeignSourceSpec::Named { name } => vec!["named".into(), format!("name={name}")],
        ForeignSourceSpec::Trino {
            catalog,
            schema,
            query,
            id_field,
            score_field,
            ..
        } => vec![
            "trino".into(),
            format!("catalog={catalog}"),
            format!("schema={schema}"),
            format!("query={query}"),
            format!("id={id_field}"),
            score(score_field),
        ],
        ForeignSourceSpec::Cypher {
            backend,
            graph,
            query,
            id_field,
            score_field,
            ..
        } => vec![
            "cypher".into(),
            format!("backend={backend:?}"),
            format!("graph={graph}"),
            format!("query={query}"),
            format!("id={id_field}"),
            score(score_field),
        ],
        ForeignSourceSpec::SparkBatch {
            artifact_ref,
            id_field,
            score_field,
        } => vec![
            "spark_batch".into(),
            format!("artifact={artifact_ref}"),
            format!("id={id_field}"),
            score(score_field),
        ],
        ForeignSourceSpec::Api {
            operation,
            capabilities,
        } => vec![
            "api".into(),
            format!("operation={operation}"),
            format!("capabilities={capabilities:?}"),
        ],
        ForeignSourceSpec::Mcp {
            server,
            tool,
            capabilities,
        } => vec![
            "mcp".into(),
            format!("server={server}"),
            format!("tool={tool}"),
            format!("capabilities={capabilities:?}"),
        ],
        ForeignSourceSpec::EventStream {
            server,
            tool,
            from_ms,
            to_ms,
            capabilities,
        } => vec![
            "event_stream".into(),
            format!("server={server}"),
            format!("tool={tool}"),
            format!(
                "from_ms={}",
                from_ms.map(|ms| ms.to_string()).unwrap_or_default()
            ),
            format!(
                "to_ms={}",
                to_ms.map(|ms| ms.to_string()).unwrap_or_default()
            ),
            format!("capabilities={capabilities:?}"),
        ],
        ForeignSourceSpec::A2a {
            agent,
            skill,
            capabilities,
        } => vec![
            "a2a".into(),
            format!("agent={agent}"),
            format!("skill={skill}"),
            format!("capabilities={capabilities:?}"),
        ],
        ForeignSourceSpec::GraphQl {
            operation,
            capabilities,
            ..
        } => vec![
            "graphql".into(),
            format!("operation={operation}"),
            format!("capabilities={capabilities:?}"),
        ],
        ForeignSourceSpec::SqliteFile {
            table,
            columns,
            watermark,
            ..
        } => vec![
            "sqlite_file".into(),
            format!("table={table}"),
            format!("columns={}", sorted(columns)),
            format!("watermark={}", watermark.as_deref().unwrap_or("")),
        ],
        ForeignSourceSpec::Warehouse {
            config,
            table,
            columns,
            id_column,
            prefer_flight_sql,
            snapshot_id,
        } => vec![
            "warehouse".into(),
            format!("kind={:?}", config.kind),
            format!("table={table}"),
            format!("columns={}", sorted(columns)),
            format!("id={id_column}"),
            format!("flight_sql={prefer_flight_sql}"),
            format!(
                "snapshot={}",
                snapshot_id.map(|id| id.to_string()).unwrap_or_default()
            ),
        ],
        ForeignSourceSpec::ClickHouse {
            database,
            table,
            columns,
            id_column,
            ..
        } => vec![
            "clickhouse".into(),
            format!("database={database}"),
            format!("table={table}"),
            format!("columns={}", sorted(columns)),
            format!("id={id_column}"),
        ],
        ForeignSourceSpec::Metastore {
            database_filter, ..
        } => vec![
            "metastore".into(),
            format!(
                "database_filter={}",
                database_filter.as_deref().unwrap_or("")
            ),
        ],
        ForeignSourceSpec::BrainGuarded {
            guard_policy,
            query,
            id_field,
            score_field,
            ..
        } => vec![
            "brain_guarded".into(),
            format!("guard_policy={guard_policy}"),
            format!("query={query}"),
            format!("id={id_field}"),
            score(score_field),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use {
        medulla_catalog_source_types::attached_catalog::CatalogColumn,
        medulla_catalog_source_types::attached_catalog::CatalogTable,
    };

    fn http(secretish: &str, columns: &[(&str, &str)]) -> ForeignSourceSpec {
        ForeignSourceSpec::HttpJson {
            url: format!("https://{secretish}.example.invalid/items"),
            json_path: "$.items".into(),
            field_map: medulla_query_types::wire::HttpFieldMap {
                id: "id".into(),
                score: None,
                columns: columns
                    .iter()
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .collect(),
            },
        }
    }

    fn pg(dsn: &str) -> ForeignSourceSpec {
        ForeignSourceSpec::Sql {
            dsn: dsn.into(),
            query: "SELECT id FROM t".into(),
            id_field: "id".into(),
            score_field: None,
            columns: vec![],
        }
    }

    fn table(name: &str, columns: &[&str]) -> CatalogTable {
        CatalogTable {
            schema: "public".into(),
            name: name.into(),
            columns: columns
                .iter()
                .map(|column| CatalogColumn {
                    name: column.to_string(),
                    type_name: "text".into(),
                    nullable: true,
                })
                .collect(),
            constraints: vec![],
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R038.2.1
    #[tokio::test]
    async fn contract_hash_is_sha256_hex_and_excludes_endpoints() {
        let a = discover_source("docs", &http("one", &[("title", "t")]), None)
            .await
            .unwrap();
        let b = discover_source("docs", &http("two", &[("title", "t")]), None)
            .await
            .unwrap();
        assert_eq!(a.content_hash.len(), 64);
        assert!(a
            .content_hash
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
        assert_eq!(a, b, "the endpoint is not part of the discovered shape");
        assert_eq!(a.source_kind, "document_store");
        assert_eq!(a.schema_version, 1);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R038.2.1
    #[tokio::test]
    async fn schema_version_rises_only_when_the_hash_changes() {
        let first = discover_source("docs", &http("x", &[("title", "t")]), None)
            .await
            .unwrap();
        let same = discover_source("docs", &http("x", &[("title", "t")]), Some(&first))
            .await
            .unwrap();
        assert_eq!(same.schema_version, 1);
        let changed = discover_source("docs", &http("x", &[("body", "b")]), Some(&same))
            .await
            .unwrap();
        assert_eq!(changed.schema_version, 2);
        assert_ne!(changed.content_hash, same.content_hash);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R038.2.1
    #[test]
    fn catalog_hash_is_stable_under_reordering() {
        let a = AttachedCatalogGraph {
            version: 1,
            views: vec![],
            tables: vec![table("b", &["y", "x"]), table("a", &["id"])],
        };
        let b = AttachedCatalogGraph {
            version: 1,
            views: vec![],
            tables: vec![table("a", &["id"]), table("b", &["x", "y"])],
        };
        assert_eq!(catalog_content_hash(&a), catalog_content_hash(&b));
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R038.2.1
    #[test]
    fn postgres_dsn_is_postgres_wire_and_others_are_relational() {
        assert_eq!(
            category_of(&pg("postgres://h/db")),
            SourceCategory::PostgresWire
        );
        assert_eq!(
            category_of(&pg("mysql://h/db")),
            SourceCategory::RelationalSql
        );
    }
}
