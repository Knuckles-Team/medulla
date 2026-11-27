//! The caller-scoped metastore sources of one `hive()` query
//! (MEDULLA-UNIFIED-DATA-PLANE-R038.4.3).
//!
//! [`OwnedHiveSources`] is built from [`ForeignSourceCatalog::registry_for`], so it holds exactly
//! the metastore sources the verified caller owns (or holds an exact share grant for). A name
//! another tenant registered is not in the list and resolves to the same refusal as a name nobody
//! registered. Scan limits and the connect timeout are the resolved `metastore.*` knob values.

use std::sync::Arc;
use std::time::Duration;

use medulla_lake::metastore::orc::LocalFileBytes;
use medulla_lake::metastore::scan::ScanLimits;
use medulla_lake::metastore::thrift::HmsClient;
use medulla_lake::metastore::MetastoreError;
use medulla_query_types::wire::ForeignSourceSpec;
use medulla_sql::sql::metastore_federation::{HiveSource, HiveSources};

/// The metastore client of one registered source, timeout from `metastore.timeout_ms`.
pub fn hms_client(label: &str, endpoint: &str, database_filter: Option<String>) -> HmsClient {
    let timeout_ms = medulla_server_api::knob_config::knobs().metastore_timeout_ms;
    HmsClient::new(
        endpoint,
        label,
        database_filter,
        Duration::from_millis(timeout_ms),
    )
}

/// The metastore sources one caller may name, by registered name.
pub struct OwnedHiveSources {
    sources: Vec<(String, String, Option<String>)>,
}

impl OwnedHiveSources {
    /// Keeps the `Metastore` entries of `registry` names with their specs.
    pub fn from_specs<'a>(
        specs: impl IntoIterator<Item = (&'a str, &'a ForeignSourceSpec)>,
    ) -> Self {
        let mut sources: Vec<_> = specs
            .into_iter()
            .filter_map(|(name, spec)| match spec {
                ForeignSourceSpec::Metastore {
                    endpoint,
                    database_filter,
                    ..
                } => Some((name.to_string(), endpoint.clone(), database_filter.clone())),
                _ => None,
            })
            .collect();
        sources.sort_by(|a, b| a.0.cmp(&b.0));
        Self { sources }
    }
}

impl HiveSources for OwnedHiveSources {
    fn resolve(&self, source: Option<&str>) -> Result<HiveSource, MetastoreError> {
        let (name, endpoint, filter) = match (source, self.sources.as_slice()) {
            (Some(name), all) => all.iter().find(|(n, _, _)| n == name).ok_or_else(|| {
                MetastoreError::SpecInvalid(format!("metastore source {name} is not registered"))
            })?,
            (None, [only]) => only,
            (None, []) => {
                return Err(MetastoreError::SpecInvalid(
                    "no metastore source is registered".into(),
                ))
            }
            (None, _) => return Err(MetastoreError::SpecInvalid(
                "several metastore sources are registered; name one: hive('source', 'db.table')"
                    .into(),
            )),
        };
        let knobs = medulla_server_api::knob_config::knobs();
        Ok(HiveSource {
            label: name.clone(),
            client: Arc::new(hms_client(name, endpoint, filter.clone())),
            bytes: Arc::new(LocalFileBytes {
                source_label: name.clone(),
            }),
            limits: ScanLimits::new(
                knobs.metastore_list_page_size,
                knobs.metastore_max_partitions,
            ),
        })
    }
}
