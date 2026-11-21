//! Column profiling for the `DatasetProfile` job kind
//! (MEDULLA-UNIFIED-DATA-PLANE-R761.2): pushed aggregates where the source
//! dialect allows them, a reservoir sample otherwise, a HyperLogLog sketch for
//! distinct counts.

//!
//! The `DatasetProfile` claim (MEDULLA-UNIFIED-DATA-PLANE-R761.3) reads one native
//! dataset from the verified tenant's own table store, profiles each requested column
//! with [`profile_column`], and writes each `ColumnProfile` node with a `profileOf` edge
//! to its catalog column node in ONE `MutationBatch`. The job result lists, per column,
//! the method, the sampled row count and `profiled_at`.

use std::collections::HashMap;

use medulla_catalog_types::profile::{ColumnProfile, HistogramBucket, ProfileMethod, TopValue};
use medulla_compute::sketch::{HyperLogLog, HLL_DEFAULT_PRECISION};

use super::data_plane_write::{commit_job_methods, edge_method, node_method};
use super::dataset_profile::{propose_claims, SensitiveScan};
use super::prelude_jobs::*;
use super::prelude_server::*;
use super::prelude_std::*;
use super::*;
use crate::quality_rule::tenant_node_id;

/// Refusal code: the source supports neither aggregate pushdown nor sampling
/// (MEDULLA-UNIFIED-DATA-PLANE-R761.4).
pub(super) const PROFILE_UNSUPPORTED: &str = "PROFILE_UNSUPPORTED";
/// Refusal code: the dataset is outside the caller's tenant or row-security scope
/// (MEDULLA-UNIFIED-DATA-PLANE-R761.4). A dataset of another tenant and a dataset nobody
/// registered refuse the same way, so a caller cannot enumerate another tenant's datasets.
pub(super) const PROFILE_FORBIDDEN: &str = "PROFILE_FORBIDDEN";

/// The SQL dialect of a profiled source; only the listed dialects take a
/// pushed aggregate statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProfileDialect {
    Postgres,
    Mysql,
    /// A native dataset or a source without aggregate pushdown.
    Other,
}

impl ProfileDialect {
    fn pushes_aggregates(self) -> bool {
        matches!(self, Self::Postgres | Self::Mysql)
    }
}

/// The result of the one pushed aggregate statement.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct AggregateRow {
    pub rows: u64,
    pub non_null: u64,
    pub min: Option<String>,
    pub max: Option<String>,
}

/// What the profiler reads from one dataset.
pub(super) trait ProfileSource {
    /// The dataset's catalog name.
    fn dataset(&self) -> &str;
    /// The source dialect.
    fn dialect(&self) -> ProfileDialect;
    /// Run one aggregate statement through the federated pushdown path.
    fn push_aggregate(&self, statement: &str) -> Result<AggregateRow, String>;
    /// Whether [`Self::scan_column`] is available.
    fn supports_sampling(&self) -> bool;
    /// Stream every value of `column` (`None` = SQL NULL).
    fn scan_column(&self, column: &str, visit: &mut dyn FnMut(Option<&str>)) -> Result<(), String>;
}

/// Profiling knobs (`profile.sample_rows`, `profile.top_k`,
/// `profile.histogram_buckets`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProfileConfig {
    pub sample_rows: u64,
    pub top_k: usize,
    pub histogram_buckets: usize,
    pub profiled_at: i64,
}

impl Default for ProfileConfig {
    fn default() -> Self {
        Self {
            sample_rows: 100_000,
            top_k: 10,
            histogram_buckets: 20,
            profiled_at: 0,
        }
    }
}

/// Why a column could not be profiled; renders as `CODE: detail`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProfileRefusal {
    pub code: &'static str,
    pub detail: String,
}

impl std::fmt::Display for ProfileRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

fn unsupported(detail: impl Into<String>) -> ProfileRefusal {
    ProfileRefusal {
        code: PROFILE_UNSUPPORTED,
        detail: detail.into(),
    }
}

/// Refuse a profile at submit, before a job exists (MEDULLA-UNIFIED-DATA-PLANE-R761.4): the
/// dataset must be a table of the caller's own tenant store or a table of an attached SQL
/// source the caller owns (`PROFILE_FORBIDDEN` otherwise), and an attached source must take
/// a pushed aggregate or a sample (`PROFILE_UNSUPPORTED` otherwise). Nothing is written.
pub(super) async fn check_profile_submit(
    state: &Arc<RwLock<ServerState>>,
    authority: &CarrierAuthority,
    dataset: &str,
) -> Result<(), ProfileRefusal> {
    #[cfg(feature = "federation-sql")]
    {
        use medulla_server_ops::api::query_txn::ForeignStateExt;
        if let Some(source) = resolve_sql_source(state, authority.owner_scope(), dataset).await {
            return if source.dialect.pushes_aggregates() {
                Ok(())
            } else {
                Err(unsupported(format!(
                    "dataset {dataset} supports neither aggregate pushdown nor sampling"
                )))
            };
        }
        // A source of any other kind the caller registered takes neither a pushed
        // aggregate nor a sample.
        if let Some((name, _)) = dataset.split_once('.') {
            let catalog = state.read().await.foreign_sources();
            if catalog.has_source(authority.owner_scope(), name) {
                return Err(unsupported(format!(
                    "dataset {dataset} supports neither aggregate pushdown nor sampling"
                )));
            }
        }
    }
    let persist_dir = state.read().await.persist_dir.clone();
    let Some(persist_dir) = persist_dir else {
        return Err(unsupported("this server has no native table store"));
    };
    #[cfg(feature = "query")]
    {
        let visible = medulla_server_auth::sql_tables::tenant_table_store(
            authority.tenant_scope(),
            Path::new(&persist_dir),
        )
        .ok()
        .and_then(|store| store.get_schema(dataset).ok().flatten())
        .is_some();
        if visible {
            return Ok(());
        }
    }
    #[cfg(not(feature = "query"))]
    let _ = persist_dir;
    Err(ProfileRefusal {
        code: PROFILE_FORBIDDEN,
        detail: format!("dataset {dataset} is not visible to the caller"),
    })
}

/// Quote a column identifier for the pushed statement.
fn quote_ident(dialect: ProfileDialect, column: &str) -> String {
    match dialect {
        ProfileDialect::Mysql => format!("`{}`", column.replace('`', "``")),
        _ => format!("\"{}\"", column.replace('"', "\"\"")),
    }
}

/// The text type a pushed statement casts values to.
fn text_type(dialect: ProfileDialect) -> &'static str {
    match dialect {
        ProfileDialect::Mysql => "CHAR",
        _ => "TEXT",
    }
}

/// The one pushed aggregate statement for `column` of `dataset`. `MIN` and `MAX` are
/// cast to text on the source so any column type reads back.
pub(super) fn aggregate_statement(dialect: ProfileDialect, dataset: &str, column: &str) -> String {
    let col = quote_ident(dialect, column);
    let text = text_type(dialect);
    format!(
        "SELECT COUNT(*), COUNT({col}), CAST(MIN({col}) AS {text}), CAST(MAX({col}) AS {text}) FROM {}",
        quote_ident(dialect, dataset)
    )
}

/// The bounded sample statement for `column` of `dataset`: at most `limit` text-cast
/// values.
pub(super) fn sample_statement(
    dialect: ProfileDialect,
    dataset: &str,
    column: &str,
    limit: u64,
) -> String {
    format!(
        "SELECT CAST({} AS {}) FROM {} LIMIT {limit}",
        quote_ident(dialect, column),
        text_type(dialect),
        quote_ident(dialect, dataset)
    )
}

/// Deterministic xorshift generator for the reservoir draw.
struct Xorshift(u64);

impl Xorshift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

/// Values drawn from one scan: the reservoir plus the exact row and null counts.
struct Drawn {
    rows: u64,
    nulls: u64,
    reservoir: Vec<String>,
}

fn draw_reservoir(
    source: &dyn ProfileSource,
    column: &str,
    capacity: u64,
) -> Result<Drawn, ProfileRefusal> {
    let capacity = capacity.max(1) as usize;
    let mut rng = Xorshift(0x9E37_79B9_7F4A_7C15);
    let mut drawn = Drawn {
        rows: 0,
        nulls: 0,
        reservoir: Vec::with_capacity(capacity.min(1 << 16)),
    };
    let mut seen_non_null: u64 = 0;
    source
        .scan_column(column, &mut |value| {
            drawn.rows += 1;
            let Some(value) = value else {
                drawn.nulls += 1;
                return;
            };
            seen_non_null += 1;
            if drawn.reservoir.len() < capacity {
                drawn.reservoir.push(value.to_string());
            } else {
                let slot = (rng.next() % seen_non_null) as usize;
                if slot < capacity {
                    drawn.reservoir[slot] = value.to_string();
                }
            }
        })
        .map_err(unsupported)?;
    Ok(drawn)
}

fn top_values(values: &[String], k: usize) -> Vec<TopValue> {
    let mut counts: HashMap<&str, u64> = HashMap::new();
    for value in values {
        *counts.entry(value.as_str()).or_default() += 1;
    }
    let mut ranked: Vec<(&str, u64)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    ranked.truncate(k);
    ranked
        .into_iter()
        .map(|(value, count)| TopValue {
            value: value.to_string(),
            count,
        })
        .collect()
}

/// Equal-width histogram when every sampled value is numeric; none otherwise.
fn histogram(values: &[String], buckets: usize) -> Vec<HistogramBucket> {
    if buckets == 0 || values.is_empty() {
        return Vec::new();
    }
    let mut numbers = Vec::with_capacity(values.len());
    for value in values {
        match value.trim().parse::<f64>() {
            Ok(n) if n.is_finite() => numbers.push(n),
            _ => return Vec::new(),
        }
    }
    let lo = numbers.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = numbers.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let width = if hi > lo {
        (hi - lo) / buckets as f64
    } else {
        1.0
    };
    let mut counts = vec![0u64; buckets];
    for n in numbers {
        let idx = (((n - lo) / width) as usize).min(buckets - 1);
        counts[idx] += 1;
    }
    counts
        .into_iter()
        .enumerate()
        .map(|(i, count)| HistogramBucket {
            lower: (lo + width * i as f64).to_string(),
            upper: (lo + width * (i + 1) as f64).to_string(),
            count,
        })
        .collect()
}

fn min_max(values: &[String]) -> (Option<String>, Option<String>) {
    (values.iter().min().cloned(), values.iter().max().cloned())
}

/// Profile one column (MEDULLA-UNIFIED-DATA-PLANE-R761.2).
pub(super) fn profile_column(
    source: &dyn ProfileSource,
    column: &str,
    cfg: &ProfileConfig,
) -> Result<ColumnProfile, ProfileRefusal> {
    let pushdown = source.dialect().pushes_aggregates();
    if !pushdown && !source.supports_sampling() {
        return Err(unsupported(format!(
            "dataset {} supports neither aggregate pushdown nor sampling",
            source.dataset()
        )));
    }
    let drawn = if source.supports_sampling() {
        Some(draw_reservoir(source, column, cfg.sample_rows)?)
    } else {
        None
    };
    let sample: &[String] = drawn
        .as_ref()
        .map(|d| d.reservoir.as_slice())
        .unwrap_or(&[]);
    let mut hll = HyperLogLog::new(HLL_DEFAULT_PRECISION);
    for value in sample {
        hll.insert(value.as_str());
    }
    let (rows, nulls, min, max, method) = if pushdown {
        let statement = aggregate_statement(source.dialect(), source.dataset(), column);
        let agg = source.push_aggregate(&statement).map_err(unsupported)?;
        let nulls = agg.rows.saturating_sub(agg.non_null);
        (agg.rows, nulls, agg.min, agg.max, ProfileMethod::Pushdown)
    } else {
        let drawn = drawn.as_ref().expect("sampling checked above");
        let (min, max) = min_max(sample);
        (
            drawn.rows,
            drawn.nulls,
            min,
            max,
            ProfileMethod::Sample {
                size: sample.len() as u64,
            },
        )
    };
    let distinct_estimate = (hll.estimate().round() as u64).min(rows - nulls);
    Ok(ColumnProfile {
        dataset: source.dataset().to_string(),
        column: column.to_string(),
        rows,
        nulls,
        distinct_estimate,
        min,
        max,
        top_k: top_values(sample, cfg.top_k),
        histogram: histogram(sample, cfg.histogram_buckets),
        profiled_at: cfg.profiled_at,
        method,
    })
}

/// The node type of one column profile.
pub(super) const COLUMN_PROFILE_LABEL: &str = "ColumnProfile";
/// Profile → catalog column edge relation.
pub(super) const PROFILE_OF: &str = "profileOf";

/// What one profile run produced: the column profiles and the sensitive detections of the
/// same pass (MEDULLA-TYPED-PACKS-R803.3).
#[derive(Default)]
struct Profiled {
    profiles: Vec<ColumnProfile>,
    detections: Vec<medulla_query_types::sensitive_detect::Detection>,
}

/// The verified target of one profile claim.
pub(super) struct ProfileTarget {
    pub dataset: String,
    pub columns: Vec<String>,
    pub owner_scope: String,
    pub tenant_scope: String,
    pub tenant: String,
    pub agent: String,
}

/// The profiling knobs, read from the registry.
fn profile_config(profiled_at: i64) -> ProfileConfig {
    let knobs = medulla_server_api::knob_config::knobs();
    ProfileConfig {
        sample_rows: knobs.profile_sample_rows,
        top_k: knobs.profile_top_k as usize,
        histogram_buckets: knobs.profile_histogram_buckets as usize,
        profiled_at,
    }
}

/// One native table of the tenant's table store, read once.
#[cfg(feature = "query")]
struct NativeTable {
    dataset: String,
    columns: Vec<String>,
    rows: Vec<Vec<medulla_tables::schema::Cell>>,
}

#[cfg(feature = "query")]
fn cell_text(cell: &medulla_tables::schema::Cell) -> Option<String> {
    use medulla_tables::schema::Cell;
    match cell {
        Cell::Null => None,
        Cell::Int(value) => Some(value.to_string()),
        Cell::Float(value) => Some(value.to_string()),
        Cell::Text(value) => Some(value.clone()),
        Cell::Bool(value) => Some(value.to_string()),
        Cell::Timestamp(value) => Some(value.to_string()),
        Cell::Bytes(value) => Some(hex::encode(value)),
        Cell::Json(value) => Some(value.to_string()),
        Cell::Vector(value) => Some(format!("{value:?}")),
    }
}

#[cfg(feature = "query")]
impl ProfileSource for NativeTable {
    fn dataset(&self) -> &str {
        &self.dataset
    }

    fn dialect(&self) -> ProfileDialect {
        ProfileDialect::Other
    }

    fn push_aggregate(&self, _statement: &str) -> Result<AggregateRow, String> {
        Err("a native dataset takes no pushed statement".to_string())
    }

    fn supports_sampling(&self) -> bool {
        true
    }

    fn scan_column(&self, column: &str, visit: &mut dyn FnMut(Option<&str>)) -> Result<(), String> {
        let index = self
            .columns
            .iter()
            .position(|name| name == column)
            .ok_or_else(|| format!("column {column} is not a column of {}", self.dataset))?;
        for row in &self.rows {
            match row.get(index).and_then(cell_text) {
                Some(text) => visit(Some(&text)),
                None => visit(None),
            }
        }
        Ok(())
    }
}

/// Profile the requested columns (every column when none is named) of one native
/// dataset of the tenant. Refuses `PROFILE_UNSUPPORTED` for a dataset or column the
/// tenant's table store does not hold.
#[cfg(feature = "query")]
fn profile_native(
    persist_dir: &Path,
    target: &ProfileTarget,
    cfg: &ProfileConfig,
    scan: &SensitiveScan,
) -> Result<Profiled, ProfileRefusal> {
    let store =
        medulla_server_auth::sql_tables::tenant_table_store(&target.tenant_scope, persist_dir)
            .map_err(|_| unsupported("the tenant table store cannot be opened"))?;
    let schema = store
        .get_schema(&target.dataset)
        .map_err(unsupported)?
        .ok_or_else(|| unsupported(format!("{} is not a dataset of the tenant", target.dataset)))?;
    let all: Vec<String> = schema
        .columns()
        .iter()
        .map(|column| column.name.clone())
        .collect();
    let wanted = if target.columns.is_empty() {
        all.clone()
    } else {
        target.columns.clone()
    };
    if let Some(missing) = wanted.iter().find(|column| !all.contains(column)) {
        return Err(unsupported(format!(
            "{missing} is not a column of {}",
            target.dataset
        )));
    }
    let table = NativeTable {
        dataset: target.dataset.clone(),
        columns: all,
        rows: store.scan(&target.dataset).map_err(unsupported)?,
    };
    let mut out = Profiled::default();
    for column in &wanted {
        let profile = profile_column(&table, column, cfg)?;
        profile
            .validate()
            .map_err(|error| unsupported(error.to_string()))?;
        out.detections.extend(scan.scan(&table, &profile)?);
        out.profiles.push(profile);
    }
    Ok(out)
}

#[cfg(not(feature = "query"))]
fn profile_native(
    _persist_dir: &Path,
    _target: &ProfileTarget,
    _cfg: &ProfileConfig,
    _scan: &SensitiveScan,
) -> Result<Profiled, ProfileRefusal> {
    Err(unsupported("this worker has no native table store"))
}

/// One table of an attached Postgres or MySQL-family source. The aggregate is pushed;
/// the sample is a bounded text-cast read. Both run on the worker's runtime from the
/// blocking profile thread.
#[cfg(feature = "federation-sql")]
struct SqlSourceTable {
    dataset: String,
    table: String,
    dsn: String,
    dialect: ProfileDialect,
    sample_rows: u64,
    timeout: Duration,
    runtime: tokio::runtime::Handle,
}

#[cfg(feature = "federation-sql")]
impl ProfileSource for SqlSourceTable {
    fn dataset(&self) -> &str {
        &self.dataset
    }

    fn dialect(&self) -> ProfileDialect {
        self.dialect
    }

    fn push_aggregate(&self, statement: &str) -> Result<AggregateRow, String> {
        let row =
            self.runtime
                .block_on(medulla_federation::source_profile::read_profile_aggregate(
                    &self.dsn,
                    statement,
                    self.timeout,
                ))?;
        Ok(AggregateRow {
            rows: row.rows,
            non_null: row.non_null,
            min: row.min,
            max: row.max,
        })
    }

    fn supports_sampling(&self) -> bool {
        self.dialect.pushes_aggregates()
    }

    fn scan_column(&self, column: &str, visit: &mut dyn FnMut(Option<&str>)) -> Result<(), String> {
        let statement = sample_statement(self.dialect, &self.table, column, self.sample_rows);
        let values =
            self.runtime
                .block_on(medulla_federation::source_profile::read_column_sample(
                    &self.dsn,
                    &statement,
                    self.timeout,
                ))?;
        for value in &values {
            visit(value.as_deref());
        }
        Ok(())
    }
}

/// The dialect of an attached source DSN.
#[cfg(feature = "federation-sql")]
fn dialect_of_dsn(dsn: &str) -> ProfileDialect {
    if dsn.starts_with("postgres://") || dsn.starts_with("postgresql://") {
        ProfileDialect::Postgres
    } else if dsn.starts_with("mysql://") || dsn.starts_with("mariadb://") {
        ProfileDialect::Mysql
    } else {
        ProfileDialect::Other
    }
}

/// Profile the named columns of `<source>.<table>` on an attached SQL source. Every
/// column must be named: a source table's column list is not read here.
#[cfg(feature = "federation-sql")]
fn profile_source(
    source: SqlSourceTable,
    columns: &[String],
    cfg: &ProfileConfig,
    scan: &SensitiveScan,
) -> Result<Profiled, ProfileRefusal> {
    if columns.is_empty() {
        return Err(unsupported(
            "a dataset of an attached source needs its columns named",
        ));
    }
    let mut out = Profiled::default();
    for column in columns {
        let mut profile = profile_column(&source, column, cfg)?;
        profile.dataset = source.dataset.clone();
        profile
            .validate()
            .map_err(|error| unsupported(error.to_string()))?;
        out.detections.extend(scan.scan(&source, &profile)?);
        out.profiles.push(profile);
    }
    Ok(out)
}

/// The attached SQL source a `<source>.<table>` dataset names, when the owner has
/// registered one.
#[cfg(feature = "federation-sql")]
async fn resolve_sql_source(
    state: &Arc<RwLock<ServerState>>,
    owner_scope: &str,
    dataset: &str,
) -> Option<SqlSourceTable> {
    use medulla_server_ops::api::query_txn::ForeignStateExt;
    let (source, table) = dataset.split_once('.')?;
    let catalog = state.read().await.foreign_sources();
    let dsn = catalog.sql_source_dsn(owner_scope, source)?;
    let knobs = medulla_server_api::knob_config::knobs();
    Some(SqlSourceTable {
        dataset: dataset.to_string(),
        table: table.to_string(),
        dialect: dialect_of_dsn(&dsn),
        dsn,
        sample_rows: knobs.profile_sample_rows,
        timeout: Duration::from_millis(knobs.profile_source_timeout_ms),
        runtime: tokio::runtime::Handle::current(),
    })
}

/// The sampled row count of a profile: the reservoir size, or every row for a pushed
/// aggregate.
fn sampled_rows(profile: &ColumnProfile) -> u64 {
    match profile.method {
        ProfileMethod::Pushdown => profile.rows,
        ProfileMethod::Sample { size } => size,
    }
}

/// The graph id of the catalog column node of `dataset.column`.
fn column_node(profile: &ColumnProfile) -> String {
    medulla_catalog_types::lineage::column_node_id(&format!(
        "{}.{}",
        profile.dataset, profile.column
    ))
}

/// The id of the one current profile node of a tenant's column.
fn profile_node_id(tenant: &str, profile: &ColumnProfile) -> String {
    tenant_node_id(
        "column_profile",
        tenant,
        &[&profile.dataset, &profile.column],
    )
}

/// The graph methods of one profile run: the catalog column node where the graph has
/// none yet, each profile node, and each `profileOf` edge.
fn profile_methods(
    core: &medulla_graph_core::graph::GraphCore,
    target: &ProfileTarget,
    profiles: &[ColumnProfile],
) -> Result<Vec<Method>, String> {
    let mut methods = Vec::with_capacity(profiles.len() * 3);
    for profile in profiles {
        let column_id = column_node(profile);
        if core.get_node_properties(&column_id).is_none() {
            methods.push(node_method(
                column_id.clone(),
                &serde_json::json!({
                    "type": medulla_catalog_types::lineage::COLUMN_LABEL,
                    "name": format!("{}.{}", profile.dataset, profile.column),
                    "tenant_id": target.tenant,
                    "_owner": target.agent,
                }),
            )?);
        }
        let body = serde_json::to_string(profile)
            .map_err(|_| "column profile encoding failed".to_string())?;
        let profile_id = profile_node_id(&target.tenant, profile);
        methods.push(node_method(
            profile_id.clone(),
            &serde_json::json!({
                "type": COLUMN_PROFILE_LABEL,
                "dataset": profile.dataset,
                "column": profile.column,
                "rows": profile.rows,
                "nulls": profile.nulls,
                "distinct_estimate": profile.distinct_estimate,
                "min": profile.min,
                "max": profile.max,
                "top_k": profile.top_k,
                "histogram": profile.histogram,
                "profiled_at": profile.profiled_at,
                "method": profile.method,
                "sampled_rows": sampled_rows(profile),
                "profile": body,
                "tenant_id": target.tenant,
                "_owner": target.agent,
            }),
        )?);
        methods.push(edge_method(
            profile_id,
            column_id,
            &serde_json::json!({
                "relationship": PROFILE_OF,
                "tenant_id": target.tenant,
                "_owner": target.agent,
            }),
        )?);
    }
    Ok(methods)
}

pub(super) async fn execute_profile_claim(
    state: &Arc<RwLock<ServerState>>,
    store: &Arc<JobStore>,
    job: &medulla_jobs::AnalyticsJob,
    worker_ref: &str,
    epoch: u64,
    target: ProfileTarget,
) -> Result<(), String> {
    store
        .checkpoint_fenced(
            &job.job_id,
            worker_ref,
            epoch,
            Checkpoint {
                progress: 0.1,
                label: "profiling".to_string(),
                state_blob: None,
                updated_at_ms: unix_ms(),
            },
            unix_ms(),
        )
        .map_err(|error| error.to_string())?;
    let (_, _, core) = resolve_core_ref(state, &job.input_snapshot.graph)
        .await
        .ok_or_else(|| "analytics target graph is unavailable".to_string())?;
    let persist_dir = state
        .read()
        .await
        .persist_dir
        .clone()
        .map(std::path::PathBuf::from);
    let Some(persist_dir) = persist_dir else {
        let _ = store.fail_attempt_fenced(
            &job.job_id,
            worker_ref,
            epoch,
            PROFILE_UNSUPPORTED,
            unix_ms(),
        );
        return Ok(());
    };
    let cfg = profile_config(unix_ms());
    // Every detector is validated here, before any value is read.
    let scan = match SensitiveScan::load(&core, &target.tenant, cfg.profiled_at) {
        Ok(scan) => scan,
        Err(refusal) => {
            let _ =
                store.fail_attempt_fenced(&job.job_id, worker_ref, epoch, refusal.code, unix_ms());
            return Ok(());
        }
    };
    #[cfg(feature = "federation-sql")]
    let attached = resolve_sql_source(state, &target.owner_scope, &target.dataset).await;
    let (profiled, target, scan) = tokio::task::spawn_blocking(move || {
        #[cfg(feature = "federation-sql")]
        if let Some(source) = attached {
            let profiled = profile_source(source, &target.columns, &cfg, &scan);
            return (profiled, target, scan);
        }
        let profiled = profile_native(&persist_dir, &target, &cfg, &scan);
        (profiled, target, scan)
    })
    .await
    .map_err(|_| "profile kernel failed".to_string())?;
    let Profiled {
        profiles,
        detections,
    } = match profiled {
        Ok(profiled) => profiled,
        Err(refusal) => {
            let _ =
                store.fail_attempt_fenced(&job.job_id, worker_ref, epoch, refusal.code, unix_ms());
            return Ok(());
        }
    };
    let mut methods = profile_methods(&core, &target, &profiles)?;
    methods.extend(propose_claims(&core, &target, &scan, &detections)?);
    commit_job_methods(state, job, "dataset-profile", methods).await?;
    let now = unix_ms();
    store
        .checkpoint_fenced(
            &job.job_id,
            worker_ref,
            epoch,
            Checkpoint {
                progress: 0.9,
                label: "profiled".to_string(),
                state_blob: None,
                updated_at_ms: now,
            },
            now,
        )
        .map_err(|error| error.to_string())?;
    let result = typed_profile_result(job, &target, &profiles)?;
    let staged = store
        .stage_result_fenced(&job.job_id, worker_ref, epoch, result, unix_ms())
        .map_err(|error| error.to_string())?;
    publish_staged_result(state, store, staged, worker_ref, epoch).await
}

/// One result row per profiled column: its method, sampled row count and `profiled_at`.
fn typed_profile_result(
    job: &medulla_jobs::AnalyticsJob,
    target: &ProfileTarget,
    profiles: &[ColumnProfile],
) -> Result<TypedJobResult, String> {
    let dataset = job.input_snapshot.dataset_ref.clone();
    let rows = profiles
        .iter()
        .map(|profile| {
            let method = match profile.method {
                ProfileMethod::Pushdown => "pushdown",
                ProfileMethod::Sample { .. } => "sample",
            };
            BTreeMap::from([
                (
                    "id".to_string(),
                    serde_json::json!(profile_node_id(&target.tenant, profile)),
                ),
                ("kind".to_string(), serde_json::json!("column_profile")),
                ("confidence".to_string(), serde_json::json!(1.0)),
                ("evidence_refs".to_string(), serde_json::json!([dataset])),
                ("source_refs".to_string(), serde_json::json!([dataset])),
                ("proof_ids".to_string(), serde_json::json!([])),
                ("contradiction_ids".to_string(), serde_json::json!([])),
                ("dataset".to_string(), serde_json::json!(profile.dataset)),
                ("column".to_string(), serde_json::json!(profile.column)),
                ("method".to_string(), serde_json::json!(method)),
                (
                    "sampled_rows".to_string(),
                    serde_json::json!(sampled_rows(profile)),
                ),
                ("rows".to_string(), serde_json::json!(profile.rows)),
                (
                    "profiled_at".to_string(),
                    serde_json::json!(profile.profiled_at),
                ),
            ])
        })
        .collect();
    TypedJobResult::new(
        [
            ("id", "string", false),
            ("kind", "string", false),
            ("confidence", "float64", false),
            ("evidence_refs", "list<string>", false),
            ("source_refs", "list<string>", false),
            ("proof_ids", "list<string>", false),
            ("contradiction_ids", "list<string>", false),
            ("dataset", "string", false),
            ("column", "string", false),
            ("method", "string", false),
            ("sampled_rows", "int64", false),
            ("rows", "int64", false),
            ("profiled_at", "int64", false),
        ]
        .into_iter()
        .map(|(name, logical_type, nullable)| ResultColumn {
            name: name.to_string(),
            logical_type: logical_type.to_string(),
            nullable,
        })
        .collect(),
        rows,
        vec![dataset],
        Vec::new(),
        None,
        None,
        reproducibility_manifest(&job.input_snapshot, &job.algo, &job.policy),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        dialect: ProfileDialect,
        sampling: bool,
        values: Vec<Option<String>>,
    }

    impl ProfileSource for Fixture {
        fn dataset(&self) -> &str {
            "fixture"
        }
        fn dialect(&self) -> ProfileDialect {
            self.dialect
        }
        fn push_aggregate(&self, _statement: &str) -> Result<AggregateRow, String> {
            let non_null: Vec<&String> = self.values.iter().flatten().collect();
            Ok(AggregateRow {
                rows: self.values.len() as u64,
                non_null: non_null.len() as u64,
                min: non_null.iter().min().map(|s| s.to_string()),
                max: non_null.iter().max().map(|s| s.to_string()),
            })
        }
        fn supports_sampling(&self) -> bool {
            self.sampling
        }
        fn scan_column(
            &self,
            _column: &str,
            visit: &mut dyn FnMut(Option<&str>),
        ) -> Result<(), String> {
            for value in &self.values {
                visit(value.as_deref());
            }
            Ok(())
        }
    }

    fn values() -> Vec<Option<String>> {
        (0..5_000)
            .map(|i| (i % 7 != 0).then(|| (i % 300).to_string()))
            .collect()
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R761.2
    #[test]
    fn sample_profile_counts_are_consistent() {
        let source = Fixture {
            dialect: ProfileDialect::Other,
            sampling: true,
            values: values(),
        };
        let cfg = ProfileConfig {
            sample_rows: 1_000,
            ..ProfileConfig::default()
        };
        let profile = profile_column(&source, "c", &cfg).unwrap();
        assert!(profile.nulls <= profile.rows);
        assert_eq!(profile.rows, 5_000);
        assert_eq!(profile.method, ProfileMethod::Sample { size: 1_000 });
        assert!(profile.top_k.len() <= cfg.top_k);
        assert_eq!(profile.histogram.len(), cfg.histogram_buckets);
        profile.validate().unwrap();

        let pushed = Fixture {
            dialect: ProfileDialect::Postgres,
            ..source
        };
        let profile = profile_column(&pushed, "c", &cfg).unwrap();
        assert_eq!(profile.method, ProfileMethod::Pushdown);
        assert!(profile.nulls <= profile.rows);
        profile.validate().unwrap();

        let none = Fixture {
            dialect: ProfileDialect::Other,
            sampling: false,
            values: Vec::new(),
        };
        let refusal = profile_column(&none, "c", &cfg).unwrap_err();
        assert_eq!(refusal.code, PROFILE_UNSUPPORTED);
        assert!(refusal.to_string().starts_with("PROFILE_UNSUPPORTED: "));
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R761.2
    #[test]
    fn each_pushdown_dialect_quotes_its_identifiers() {
        assert!(ProfileDialect::Mysql.pushes_aggregates());
        assert!(ProfileDialect::Postgres.pushes_aggregates());
        assert!(!ProfileDialect::Other.pushes_aggregates());
        assert_eq!(
            aggregate_statement(ProfileDialect::Mysql, "orders", "a`b"),
            "SELECT COUNT(*), COUNT(`a``b`), CAST(MIN(`a``b`) AS CHAR), CAST(MAX(`a``b`) AS CHAR) FROM `orders`"
        );
        assert_eq!(
            aggregate_statement(ProfileDialect::Postgres, "orders", "a\"b"),
            "SELECT COUNT(*), COUNT(\"a\"\"b\"), CAST(MIN(\"a\"\"b\") AS TEXT), CAST(MAX(\"a\"\"b\") AS TEXT) FROM \"orders\""
        );
    }
}
