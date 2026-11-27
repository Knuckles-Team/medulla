//! The engine's relational capability report (MEDULLA-ENGINE-PARITY-R020.1):
//! the wire features, index and constraint kinds, the SQL compatibility
//! matrix digest, replication and point-in-time recovery the build serves.
//! The wire front end reports `server_version` and its parameters from it.

use serde::{Deserialize, Serialize};

/// A wire-protocol feature the front end serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireFeature {
    Startup,
    SimpleQuery,
    ExtendedQuery,
    PreparedStatements,
    CopyIn,
    CopyOut,
    Cancellation,
    Tls,
}

/// An index kind user tables may declare.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum IndexKind {
    Secondary,
    Partial,
    Expression,
    Unique,
}

/// A constraint kind enforced at commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ConstraintKind {
    Unique,
    ForeignKey,
    Check,
    NotNull,
    Shacl,
}

/// The engine capability report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct EngineCapabilities {
    pub server_version: String,
    pub wire_features: Vec<WireFeature>,
    pub index_kinds: Vec<IndexKind>,
    pub constraint_kinds: Vec<ConstraintKind>,
    pub compat_matrix_digest: Option<String>,
    pub replication: bool,
    pub pitr: bool,
}

/// The server version the wire front end reports (MEDULLA-ENGINE-PARITY-R020.2): the one
/// source of `server_version` for the capability report and the startup parameters.
pub const PG_SERVER_VERSION: &str = "16.0";

impl EngineCapabilities {
    /// The report of one build. `pgwire` says the build serves the wire front end (all eight
    /// [`WireFeature`]s, else none), `replication` that it serves replication, and
    /// `compat_matrix_digest` is `PgCompatibilityMatrix::digest` of its form table. `pitr`
    /// stays `false` until MEDULLA-ENGINE-PARITY-R016.2 serves restore to a target.
    pub fn for_build(
        pgwire: bool,
        replication: bool,
        compat_matrix_digest: Option<String>,
    ) -> Self {
        let wire_features = if pgwire {
            vec![
                WireFeature::Startup,
                WireFeature::SimpleQuery,
                WireFeature::ExtendedQuery,
                WireFeature::PreparedStatements,
                WireFeature::CopyIn,
                WireFeature::CopyOut,
                WireFeature::Cancellation,
                WireFeature::Tls,
            ]
        } else {
            Vec::new()
        };
        Self {
            server_version: PG_SERVER_VERSION.to_string(),
            wire_features,
            index_kinds: vec![
                IndexKind::Secondary,
                IndexKind::Partial,
                IndexKind::Expression,
                IndexKind::Unique,
            ],
            constraint_kinds: vec![
                ConstraintKind::Unique,
                ConstraintKind::ForeignKey,
                ConstraintKind::Check,
                ConstraintKind::NotNull,
                ConstraintKind::Shacl,
            ],
            compat_matrix_digest,
            replication,
            pitr: false,
        }
        .canonical()
    }

    /// Sort and deduplicate every list so two reports of one build compare
    /// equal and serialize to the same bytes.
    pub fn canonical(mut self) -> Self {
        self.wire_features.sort();
        self.wire_features.dedup();
        self.index_kinds.sort();
        self.index_kinds.dedup();
        self.constraint_kinds.sort();
        self.constraint_kinds.dedup();
        self
    }

    /// The `(name, value)` parameters the wire front end reports at startup.
    pub fn wire_parameters(&self) -> Vec<(&'static str, String)> {
        vec![
            ("server_version", self.server_version.clone()),
            ("server_encoding", "UTF8".to_string()),
            ("client_encoding", "UTF8".to_string()),
            ("integer_datetimes", "on".to_string()),
        ]
    }
}

/// The rollup ranges of MEDULLA-ENGINE-PARITY and the consumer each one names
/// (plan.md "Consumers"). A rollup without a consumer stays out of the work
/// queue.
pub const ENGINE_PARITY_CONSUMERS: [(&str, &str); 7] = [
    (
        "R001-R005",
        "Concurrent agent writes through arbora; MEDULLA-SCM changeset acceptance",
    ),
    (
        "R006-R009",
        "Outside BI and SQL tools; the native-hosting pilot of MEDULLA-UNIFIED-DATA-PLANE-R026",
    ),
    (
        "R010-R014",
        "Native-hosted application tables of MEDULLA-UNIFIED-DATA-PLANE-R024",
    ),
    (
        "R015-R017",
        "Change subscribers of MEDULLA-UNIFIED-DATA-PLANE-R009; operator recovery; read scale",
    ),
    (
        "R018",
        "Catalog profiling and quality jobs of MEDULLA-UNIFIED-DATA-PLANE-R761 and R762",
    ),
    ("R019", "Every attached-source adapter"),
    ("R020", "The benchmark gate of MEDULLA-DURABLE-KERNEL-R042"),
];
