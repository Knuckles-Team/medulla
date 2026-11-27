//! ClickHouse attached-source driver config + connect/refusal path
//! (CONCEPT:MEDULLA-UNIFIED-DATA-PLANE-R017.2.1).
//!
//! This slice covers ONLY the typed driver config and the refusal gate: it proves a
//! [`ClickHouseDriverConfig`] can be built from validated host/port/database inputs,
//! and that [`connect`] refuses an unsafe or invalid destination before issuing any
//! HTTP/native-client request. It requires NO live ClickHouse server. Issuing a real
//! request and pushing federated query fragments down to ClickHouse is the live slice
//! `MEDULLA-UNIFIED-DATA-PLANE-R017.2.2` (the `query` and reader half of this file), mirroring the loopback-only refusal gate already
//! used by `connect_remote_engine` in `medulla_federation::federation`.

use std::net::ToSocketAddrs;

/// A validated ClickHouse connection target. Construct only through
/// [`ClickHouseDriverConfig::build`], which refuses an empty host, a zero port, or an
/// empty database before a config can exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClickHouseDriverConfig {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub tls: bool,
}

/// A ClickHouse target handle: [`connect`] returns one that passed the loopback refusal gate,
/// and [`ClickHouseConnection::for_target`] builds the handle of a registered source, whose
/// destination [`query`] vets through the shared outbound guard on every request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClickHouseConnection {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub tls: bool,
    /// The secret reference of the credential (`env://NAME`, the variable holding
    /// `user:password`), never the secret.
    pub auth_ref: Option<String>,
}

impl ClickHouseConnection {
    /// The handle of a registered source. No request is issued and no address is resolved here.
    pub fn for_target(config: &ClickHouseDriverConfig, auth_ref: Option<&str>) -> Self {
        Self {
            host: config.host.clone(),
            port: config.port,
            database: config.database.clone(),
            tls: config.tls,
            auth_ref: auth_ref.map(str::to_string),
        }
    }
}

/// Why a [`ClickHouseDriverConfig`] could not be built, or [`connect`] refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClickHouseConnectError {
    EmptyHost,
    InvalidPort,
    EmptyDatabase,
    UnresolvableDestination,
    DisallowedDestination,
}

impl std::fmt::Display for ClickHouseConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::EmptyHost => "clickhouse: host must not be empty",
            Self::InvalidPort => "clickhouse: port must not be zero",
            Self::EmptyDatabase => "clickhouse: database must not be empty",
            Self::UnresolvableDestination => "clickhouse: unable to resolve destination",
            Self::DisallowedDestination => {
                "clickhouse: destination is not allowed (not a local verified endpoint)"
            }
        };
        f.write_str(message)
    }
}

impl std::error::Error for ClickHouseConnectError {}

impl ClickHouseDriverConfig {
    /// Build a validated driver config, refusing an empty host, a zero port, or an
    /// empty database.
    pub fn build(
        host: &str,
        port: u16,
        database: &str,
        tls: bool,
    ) -> Result<Self, ClickHouseConnectError> {
        if host.trim().is_empty() {
            return Err(ClickHouseConnectError::EmptyHost);
        }
        if port == 0 {
            return Err(ClickHouseConnectError::InvalidPort);
        }
        if database.trim().is_empty() {
            return Err(ClickHouseConnectError::EmptyDatabase);
        }
        Ok(Self {
            host: host.to_string(),
            port,
            database: database.to_string(),
            tls,
        })
    }
}

/// Resolve the configured destination and refuse (never dial) anything that is not a
/// local verified endpoint. Mirrors `connect_remote_engine`'s loopback-only gate in
/// `medulla_federation::federation`; actually opening the HTTP/native-client session is the live
/// slice `MEDULLA-UNIFIED-DATA-PLANE-R017.2.2`.
pub fn connect(
    config: &ClickHouseDriverConfig,
) -> Result<ClickHouseConnection, ClickHouseConnectError> {
    let endpoint = format!("{}:{}", config.host, config.port);
    let addresses: Vec<_> = endpoint
        .to_socket_addrs()
        .map_err(|_| ClickHouseConnectError::UnresolvableDestination)?
        .take(8)
        .collect();
    if addresses.is_empty() || addresses.iter().any(|address| !address.ip().is_loopback()) {
        return Err(ClickHouseConnectError::DisallowedDestination);
    }
    Ok(ClickHouseConnection::for_target(config, None))
}

/// Typed refusal: the source cannot be reached or read.
pub const SOURCE_UNAVAILABLE: &str = "SOURCE_UNAVAILABLE";
/// Typed refusal: the source schema no longer matches the registered columns.
pub const SCHEMA_DRIFT: &str = "SCHEMA_DRIFT";
/// Typed refusal: the request needs a capability ClickHouse does not offer.
pub const CAPABILITY_UNSUPPORTED: &str = "CAPABILITY_UNSUPPORTED";
/// The reason of the refusal of a capture request: ClickHouse has no change capture.
pub const CLICKHOUSE_NO_CDC: &str = "clickhouse_no_cdc";

/// The refusal a capture request against ClickHouse gets, from the catalog model's single
/// explicit [`medulla_catalog_types::clickhouse_attached_catalog::ClickHouseCaptureCapability`].
pub fn capture_refusal() -> String {
    format!("{CAPABILITY_UNSUPPORTED}: {CLICKHOUSE_NO_CDC}")
}

/// The decoded rows of one `JSONCompact` reply: `(name, type)` per column from `meta`, and one
/// JSON value per cell, decoded by the type name.
#[cfg(feature = "federation")]
#[derive(Debug, Clone, PartialEq)]
pub struct ClickHouseResult {
    pub columns: Vec<(String, String)>,
    pub rows: Vec<Vec<serde_json::Value>>,
}

#[cfg(feature = "federation")]
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

/// Resolve the credential a handle's secret reference names. Only `env://NAME` resolves in
/// this build; the variable holds `user:password`.
#[cfg(feature = "federation")]
fn credentials(handle: &ClickHouseConnection) -> Result<Option<(String, String)>, String> {
    let Some(reference) = handle.auth_ref.as_deref() else {
        return Ok(None);
    };
    let Some(name) = reference.strip_prefix("env://").filter(|n| !n.is_empty()) else {
        return Err(format!(
            "{SOURCE_UNAVAILABLE}: the credential reference scheme is not resolvable here"
        ));
    };
    let value = std::env::var(name)
        .map_err(|_| format!("{SOURCE_UNAVAILABLE}: the credential reference is not set"))?;
    let (user, password) = value.split_once(':').unwrap_or((value.as_str(), ""));
    Ok(Some((user.to_string(), password.to_string())))
}

/// POST one rendered statement to `/?default_format=JSONCompact` through the shared outbound
/// guard (`medulla_federation::federation_ssrf`): the destination is resolved once, vetted and
/// pinned. Bound values travel as `param_<name>` query parameters. No error reflects the SQL,
/// a parameter or a credential.
#[cfg(feature = "federation")]
pub fn query(
    handle: &ClickHouseConnection,
    statement: &medulla_exec::sql_text::ClickHouseStatement,
) -> Result<ClickHouseResult, String> {
    use std::io::Read;
    let scheme = if handle.tls { "https" } else { "http" };
    let url = format!("{scheme}://{}:{}/", handle.host, handle.port);
    let allow: Vec<String> =
        std::env::var(medulla_federation::federation_ssrf::HTTP_JSON_FEDERATION_ALLOW_ENV)
            .unwrap_or_default()
            .split(',')
            .take(1024)
            .map(str::to_string)
            .collect();
    let target = medulla_federation::federation_ssrf::validate_outbound_http_target(
        &url,
        &allow,
        medulla_federation::federation_ssrf::OutboundAllowPolicy::PublicHttps,
    )
    .map_err(|error| format!("{SOURCE_UNAVAILABLE}: {error}"))?;
    let credentials = credentials(handle)?;
    let timeout = std::time::Duration::from_secs(30);
    let builder = ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(10)))
        .timeout_recv_response(Some(timeout))
        .timeout_recv_body(Some(timeout))
        .timeout_send_request(Some(timeout))
        .timeout_send_body(Some(timeout))
        .timeout_global(Some(std::time::Duration::from_secs(60)));
    let agent = medulla_federation::federation_ssrf::pinned_agent(&target, builder);
    let mut request = agent
        .post(&url)
        .query("default_format", "JSONCompact")
        .query("database", handle.database.as_str())
        .query("output_format_json_quote_64bit_integers", "0")
        .query("readonly", "1");
    for (name, value) in &statement.params {
        request = request.query(name.as_str(), value.as_str());
    }
    if let Some((user, password)) = &credentials {
        request = request
            .header("X-ClickHouse-User", user.as_str())
            .header("X-ClickHouse-Key", password.as_str());
    }
    let response = request
        .send(statement.sql.as_str())
        .map_err(|_| format!("{SOURCE_UNAVAILABLE}: the ClickHouse request failed"))?;
    let mut body = Vec::new();
    response
        .into_body()
        .into_reader()
        .take((MAX_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut body)
        .map_err(|_| format!("{SOURCE_UNAVAILABLE}: the ClickHouse response could not be read"))?;
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(format!(
            "{SOURCE_UNAVAILABLE}: the ClickHouse response exceeds the limit"
        ));
    }
    decode_json_compact(&body)
}

/// Decode a `JSONCompact` reply by the type names of its `meta`.
#[cfg(feature = "federation")]
fn decode_json_compact(body: &[u8]) -> Result<ClickHouseResult, String> {
    let malformed = || format!("{SOURCE_UNAVAILABLE}: the ClickHouse reply is malformed");
    let reply: serde_json::Value = serde_json::from_slice(body).map_err(|_| malformed())?;
    let columns: Vec<(String, String)> = reply
        .get("meta")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(malformed)?
        .iter()
        .map(|column| {
            Some((
                column.get("name")?.as_str()?.to_string(),
                column.get("type")?.as_str()?.to_string(),
            ))
        })
        .collect::<Option<_>>()
        .ok_or_else(malformed)?;
    let data = reply
        .get("data")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(malformed)?;
    let mut rows = Vec::with_capacity(data.len());
    for row in data {
        let cells = row
            .as_array()
            .filter(|c| c.len() == columns.len())
            .ok_or_else(malformed)?;
        rows.push(
            cells
                .iter()
                .zip(&columns)
                .map(|(cell, (_, type_name))| decode_cell(cell, type_name))
                .collect(),
        );
    }
    Ok(ClickHouseResult { columns, rows })
}

/// Decode one cell by its ClickHouse type name: integers and floats become JSON numbers
/// (a quoted 64-bit integer is parsed), `Nullable(T)` decodes as `T` with `null` kept, and
/// every other type stays as the reply gave it.
#[cfg(feature = "federation")]
fn decode_cell(cell: &serde_json::Value, type_name: &str) -> serde_json::Value {
    let inner = type_name
        .strip_prefix("Nullable(")
        .and_then(|t| t.strip_suffix(')'))
        .unwrap_or(type_name);
    if cell.is_null() {
        return serde_json::Value::Null;
    }
    let text = cell.as_str();
    if inner.starts_with("UInt") {
        if let Some(n) = text.and_then(|t| t.parse::<u64>().ok()) {
            return n.into();
        }
    } else if inner.starts_with("Int") {
        if let Some(n) = text.and_then(|t| t.parse::<i64>().ok()) {
            return n.into();
        }
    } else if inner.starts_with("Float") {
        if let Some(n) = text
            .and_then(|t| t.parse::<f64>().ok())
            .and_then(serde_json::Number::from_f64)
        {
            return serde_json::Value::Number(n);
        }
    }
    cell.clone()
}

/// The engine family `system.tables.engine` names.
#[cfg(feature = "federation")]
fn engine_family(
    engine: &str,
) -> medulla_catalog_types::clickhouse_attached_catalog::ClickHouseEngineFamily {
    use medulla_catalog_types::clickhouse_attached_catalog::ClickHouseEngineFamily as Family;
    match engine {
        "MergeTree" => Family::MergeTree,
        "ReplacingMergeTree" => Family::ReplacingMergeTree,
        "Memory" => Family::Memory,
        "Distributed" => Family::Distributed,
        _ => Family::Other,
    }
}

/// Read `system.columns` (and the engine of `system.tables`) of one table into the attached
/// catalog model (MEDULLA-UNIFIED-DATA-PLANE-R003). A table with no columns is
/// `SCHEMA_DRIFT`, not an empty table.
#[cfg(feature = "federation")]
pub fn read_catalog(
    handle: &ClickHouseConnection,
    table: &str,
) -> Result<medulla_catalog_types::clickhouse_attached_catalog::ClickHouseSourceCatalog, String> {
    use medulla_catalog_types::clickhouse_attached_catalog::{
        ClickHouseColumn, ClickHouseSourceCatalog, ClickHouseTableCatalog,
    };
    use medulla_exec::sql_text::{ClickHouseStatement, CLICKHOUSE_PARAM_PREFIX};
    let bind = |sql: &str| ClickHouseStatement {
        sql: sql.to_string(),
        params: vec![
            (
                format!("{CLICKHOUSE_PARAM_PREFIX}db"),
                handle.database.clone(),
            ),
            (format!("{CLICKHOUSE_PARAM_PREFIX}tbl"), table.to_string()),
        ],
    };
    let columns = query(
        handle,
        &bind(
            "SELECT name, type FROM system.columns \
             WHERE database = {db:String} AND table = {tbl:String} ORDER BY position",
        ),
    )?;
    let engine = query(
        handle,
        &bind(
            "SELECT engine FROM system.tables WHERE database = {db:String} AND name = {tbl:String}",
        ),
    )?;
    let columns: Vec<ClickHouseColumn> = columns
        .rows
        .iter()
        .filter_map(|row| {
            Some(ClickHouseColumn {
                name: row.first()?.as_str()?.to_string(),
                column_type: row.get(1)?.as_str()?.to_string(),
            })
        })
        .collect();
    let engine = engine
        .rows
        .first()
        .and_then(|row| row.first())
        .and_then(serde_json::Value::as_str)
        .map_or(
            medulla_catalog_types::clickhouse_attached_catalog::ClickHouseEngineFamily::Other,
            engine_family,
        );
    let mut catalog = ClickHouseSourceCatalog::default();
    catalog
        .insert_table(ClickHouseTableCatalog {
            database: handle.database.clone(),
            table: table.to_string(),
            engine,
            columns,
            supports_acceleration: false,
        })
        .map_err(|error| format!("{SCHEMA_DRIFT}: {error}"))?;
    Ok(catalog)
}

/// The engine-backed [`medulla_federation::clickhouse_foreign::ClickHouseReader`]: it reads the
/// catalog, refuses a requested column the table lacks as `SCHEMA_DRIFT`, renders the shared
/// ClickHouse `SELECT` with the pushed filters bound as parameters, and decodes the rows.
#[cfg(feature = "federation")]
#[derive(Debug, Default, Clone, Copy)]
pub struct EngineClickHouseReader;

#[cfg(feature = "federation")]
impl medulla_federation::clickhouse_foreign::ClickHouseReader for EngineClickHouseReader {
    fn read(
        &self,
        request: &medulla_federation::clickhouse_foreign::ClickHouseRead<'_>,
    ) -> Result<medulla_federation::federation_opt::ForeignRows, String> {
        use medulla_exec::sql_text::{
            render_columnar_select, ClickHouseFilter, ClickHouseParamKind,
        };
        use medulla_federation::federation_opt::{Comparison, ForeignRow, ForeignRows};
        let config = ClickHouseDriverConfig::build(
            request.host,
            request.port,
            request.database,
            request.tls,
        )
        .map_err(|error| format!("INVALID_ARGUMENT: {error}"))?;
        let handle = ClickHouseConnection::for_target(&config, request.auth_ref);
        let catalog = read_catalog(&handle, request.table)?;
        let table = catalog
            .tables
            .get(&format!("{}.{}", request.database, request.table))
            .ok_or_else(|| format!("{SCHEMA_DRIFT}: the table is not in the catalog"))?;
        for column in request
            .columns
            .iter()
            .map(String::as_str)
            .chain(request.filters.iter().map(|f| f.column.as_str()))
        {
            if !table.columns.iter().any(|known| known.name == column) {
                return Err(format!(
                    "{SCHEMA_DRIFT}: column {column} is not in the ClickHouse table"
                ));
            }
        }
        let filters = request
            .filters
            .iter()
            .map(|filter| {
                let (value, kind) = match &filter.value {
                    serde_json::Value::String(text) => (text.clone(), ClickHouseParamKind::Text),
                    serde_json::Value::Number(n) if n.is_i64() || n.is_u64() => {
                        (n.to_string(), ClickHouseParamKind::Int64)
                    }
                    serde_json::Value::Number(n) => (n.to_string(), ClickHouseParamKind::Float64),
                    other => {
                        return Err(format!(
                            "INVALID_ARGUMENT: a ClickHouse filter value must be a string or number, got {other}"
                        ))
                    }
                };
                Ok(ClickHouseFilter {
                    column: filter.column.clone(),
                    op: match filter.comparison {
                        Comparison::Eq => "=",
                        Comparison::Ne => "!=",
                        Comparison::Lt => "<",
                        Comparison::Le => "<=",
                        Comparison::Gt => ">",
                        Comparison::Ge => ">=",
                    },
                    value,
                    kind,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let statement = render_columnar_select(request.table, &request.columns, &filters)
            .map_err(|error| format!("INVALID_ARGUMENT: {error}"))?;
        let result = query(&handle, &statement)?;
        Ok(ForeignRows::from_rows(
            result.rows.into_iter().enumerate().map(|(ordinal, cells)| {
                let columns: std::collections::BTreeMap<String, serde_json::Value> = result
                    .columns
                    .iter()
                    .map(|(name, _)| name.clone())
                    .zip(cells)
                    .collect();
                let id = match columns.get(request.id_column) {
                    Some(serde_json::Value::String(text)) if !request.id_column.is_empty() => {
                        text.clone()
                    }
                    Some(serde_json::Value::Null) | None => ordinal.to_string(),
                    Some(other) => other.to_string(),
                };
                ForeignRow {
                    id,
                    score: None,
                    columns,
                }
            }),
        ))
    }
}

/// Install the engine reader as the host's ClickHouse reader.
#[cfg(feature = "federation")]
pub fn register() {
    medulla_federation::clickhouse_foreign::register_clickhouse_reader(std::sync::Arc::new(
        EngineClickHouseReader,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R017.2.1
    #[test]
    fn valid_config_round_trips() {
        let config = ClickHouseDriverConfig::build("127.0.0.1", 8123, "default", false).unwrap();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 8123);
        assert_eq!(config.database, "default");
        assert!(!config.tls);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R017.2.1
    #[test]
    fn empty_host_is_refused() {
        assert_eq!(
            ClickHouseDriverConfig::build("", 8123, "default", false),
            Err(ClickHouseConnectError::EmptyHost)
        );
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R017.2.1
    #[test]
    fn zero_port_is_refused() {
        assert_eq!(
            ClickHouseDriverConfig::build("127.0.0.1", 0, "default", false),
            Err(ClickHouseConnectError::InvalidPort)
        );
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R017.2.1
    #[test]
    fn empty_database_is_refused() {
        assert_eq!(
            ClickHouseDriverConfig::build("127.0.0.1", 8123, "", false),
            Err(ClickHouseConnectError::EmptyDatabase)
        );
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R017.2.1
    #[test]
    fn non_loopback_destination_is_refused_by_connect() {
        // A TEST-NET-3 (RFC 5737) literal IP: resolves with no DNS/network lookup, so
        // this stays deterministic with no live database or network access.
        let config = ClickHouseDriverConfig::build("203.0.113.5", 8123, "default", false)
            .expect("valid config");
        assert_eq!(
            connect(&config),
            Err(ClickHouseConnectError::DisallowedDestination)
        );
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R017.2.1
    #[test]
    fn loopback_destination_passes_the_refusal_gate() {
        let config = ClickHouseDriverConfig::build("127.0.0.1", 8123, "default", false).unwrap();
        let connection = connect(&config).expect("loopback destination must pass the gate");
        assert_eq!(connection.host, "127.0.0.1");
        assert_eq!(connection.port, 8123);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R017.2.2
    #[test]
    fn capture_request_is_refused_with_the_no_cdc_reason() {
        assert_eq!(
            capture_refusal(),
            "CAPABILITY_UNSUPPORTED: clickhouse_no_cdc"
        );
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R017.2.2
    #[cfg(feature = "federation")]
    #[test]
    fn json_compact_rows_decode_by_the_meta_type_names() {
        let body = br#"{"meta":[{"name":"id","type":"UInt64"},{"name":"n","type":"Nullable(Int64)"},{"name":"s","type":"String"},{"name":"f","type":"Float64"}],"data":[["7",null,"a",1.5]],"rows":1}"#;
        let decoded = decode_json_compact(body).unwrap();
        assert_eq!(decoded.rows[0][0], serde_json::json!(7));
        assert_eq!(decoded.rows[0][1], serde_json::Value::Null);
        assert_eq!(decoded.rows[0][2], serde_json::json!("a"));
        assert_eq!(decoded.rows[0][3], serde_json::json!(1.5));
        assert!(decode_json_compact(b"not json")
            .unwrap_err()
            .starts_with("SOURCE_UNAVAILABLE"));
    }
}
