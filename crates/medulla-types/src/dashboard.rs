//! The dashboard asset record (MEDULLA-TYPED-PACKS-R109.1).
//!
//! A dashboard is a catalog asset whose body (panels, queries, layout, parameters) the engine
//! stores. The data a panel shows is not stored. The revision only increases; a save carries the
//! revision it read (MEDULLA-TYPED-PACKS-R109.2).

use serde::{Deserialize, Serialize};

/// The refusal code of a body over `catalog.dashboard_body_max_bytes`.
pub const DASHBOARD_BODY_TOO_LARGE: &str = "DASHBOARD_BODY_TOO_LARGE";
/// The refusal code of a revision that does not rise.
pub const DASHBOARD_REVISION_STALE: &str = "DASHBOARD_REVISION_STALE";
/// The refusal code of an empty id, owner or grant principal, or a grant listed twice.
pub const DASHBOARD_INVALID: &str = "DASHBOARD_INVALID";

/// `knob catalog.dashboard_retention_days`: how long a deleted dashboard keeps its body.
pub const DASHBOARD_RETENTION_DAYS_KNOB: &str = "catalog.dashboard_retention_days";
/// Default of the retention knob, in days.
pub const DASHBOARD_RETENTION_DAYS_DEFAULT: u64 = 30;
/// Inclusive bounds of the retention knob, in days.
pub const DASHBOARD_RETENTION_DAYS_RANGE: (u64, u64) = (1, 365);
/// Environment override of the retention knob.
pub const ENV_DASHBOARD_RETENTION_DAYS: &str = "MEDULLA_CATALOG_DASHBOARD_RETENTION_DAYS";

/// The effective retention in days: the environment value inside the bounds, the default
/// otherwise.
pub fn dashboard_retention_days() -> u64 {
    std::env::var(ENV_DASHBOARD_RETENTION_DAYS)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|n| {
            (DASHBOARD_RETENTION_DAYS_RANGE.0..=DASHBOARD_RETENTION_DAYS_RANGE.1).contains(n)
        })
        .unwrap_or(DASHBOARD_RETENTION_DAYS_DEFAULT)
}

/// Label of the asset node `dashboard:<id>`.
pub const DASHBOARD_ASSET_LABEL: &str = "DashboardAsset";
/// Label of the history node `dashboard_revision:<id>:<n>`.
pub const DASHBOARD_REVISION_LABEL: &str = "DashboardRevision";

/// The node id of a dashboard asset.
pub fn dashboard_node_id(id: &str) -> String {
    format!("dashboard:{id}")
}

/// The node id of one saved revision of a dashboard.
pub fn dashboard_revision_node_id(id: &str, revision: u64) -> String {
    format!("dashboard_revision:{id}:{revision}")
}

/// `knob catalog.dashboard_body_max_bytes`: largest stored body.
pub const DASHBOARD_BODY_MAX_BYTES_KNOB: &str = "catalog.dashboard_body_max_bytes";
/// Default of the body size knob.
pub const DASHBOARD_BODY_MAX_BYTES_DEFAULT: u64 = 1_048_576;
/// Inclusive bounds of the body size knob.
pub const DASHBOARD_BODY_MAX_BYTES_RANGE: (u64, u64) = (65_536, 8_388_608);
/// Environment override of the body size knob.
pub const ENV_DASHBOARD_BODY_MAX_BYTES: &str = "MEDULLA_CATALOG_DASHBOARD_BODY_MAX_BYTES";

/// The effective body size limit in bytes: the environment value inside the bounds, the default
/// otherwise.
pub fn dashboard_body_max_bytes() -> u64 {
    std::env::var(ENV_DASHBOARD_BODY_MAX_BYTES)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|n| {
            (DASHBOARD_BODY_MAX_BYTES_RANGE.0..=DASHBOARD_BODY_MAX_BYTES_RANGE.1).contains(n)
        })
        .unwrap_or(DASHBOARD_BODY_MAX_BYTES_DEFAULT)
}

/// What a grant lets its holder do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum DashboardRole {
    /// Read the dashboard.
    View,
    /// Read and save the dashboard.
    Edit,
}

/// A grant of one role to one principal or group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DashboardGrant {
    /// The principal or group the grant names.
    pub principal_or_group: String,
    /// The role it holds.
    pub role: DashboardRole,
}

/// One panel and the links that name its data source, query, terms and concepts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DashboardPanel {
    /// The panel id, unique in the dashboard.
    pub id: String,
    /// The data source the panel reads.
    pub source: String,
    /// The query the panel runs.
    pub query: String,
    /// Glossary terms the panel names.
    #[serde(default)]
    pub terms: Vec<String>,
    /// Ontology concepts the panel names.
    #[serde(default)]
    pub concepts: Vec<String>,
}

/// The stored body: panels, layout and parameters. Layout and parameters are opaque to the
/// engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DashboardBody {
    /// The panels.
    pub panels: Vec<DashboardPanel>,
    /// The layout the viewer renders.
    #[serde(default)]
    pub layout: serde_json::Value,
    /// The parameters the queries take.
    #[serde(default)]
    pub parameters: serde_json::Value,
}

/// A dashboard catalog asset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DashboardAsset {
    /// The asset id.
    pub id: String,
    /// The stored body.
    pub body: DashboardBody,
    /// A number that only increases with each save.
    pub revision: u64,
    /// The owning principal.
    pub owner: String,
    /// Who else may view or edit.
    #[serde(default)]
    pub grants: Vec<DashboardGrant>,
}

impl DashboardAsset {
    /// Check the identifying fields, the grants and the body size.
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() || self.owner.trim().is_empty() {
            return Err(format!("{DASHBOARD_INVALID}: id and owner must not be empty"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for grant in &self.grants {
            if grant.principal_or_group.trim().is_empty()
                || !seen.insert(grant.principal_or_group.as_str())
            {
                return Err(format!(
                    "{DASHBOARD_INVALID}: a grant needs a principal or group, once"
                ));
            }
        }
        let size = serde_json::to_vec(&self.body)
            .map_err(|error| format!("{DASHBOARD_INVALID}: body does not encode: {error}"))?
            .len() as u64;
        let max = dashboard_body_max_bytes();
        if size > max {
            return Err(format!(
                "{DASHBOARD_BODY_TOO_LARGE}: the body is {size} bytes, over {max}"
            ));
        }
        Ok(())
    }

    /// Check that `self` may replace `previous`: the revision rises.
    pub fn check_successor_of(&self, previous: &DashboardAsset) -> Result<(), String> {
        if self.id != previous.id || self.revision <= previous.revision {
            return Err(format!(
                "{DASHBOARD_REVISION_STALE}: revision {} does not rise above {}",
                self.revision, previous.revision
            ));
        }
        Ok(())
    }
}

/// One dashboard operation (`Method::DashboardAsset`, MEDULLA-TYPED-PACKS-R109.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum DashboardAssetOp {
    /// Store `body` as the next revision. `revision` is the revision the caller read; a new
    /// dashboard carries 0 and the caller becomes its owner.
    Save {
        /// The dashboard id.
        id: String,
        /// The body to store.
        body: DashboardBody,
        /// The revision the caller read.
        revision: u64,
    },
    /// Replace the grants. Only the owner may.
    Share {
        /// The dashboard id.
        id: String,
        /// The grants that replace the stored ones.
        grants: Vec<DashboardGrant>,
    },
    /// Set the role of one grantee, leaving every other grant as stored. The grantee's grant is
    /// replaced when it exists and added when it does not. Only the owner may.
    Grant {
        /// The dashboard id.
        id: String,
        /// The grant to set for its principal or group.
        grant: DashboardGrant,
    },
    /// Remove the grant of one grantee, leaving every other grant as stored. Removing a grantee
    /// that holds no grant succeeds and changes nothing. Only the owner may.
    Revoke {
        /// The dashboard id.
        id: String,
        /// The principal or group whose grant is removed.
        principal_or_group: String,
    },
    /// Soft delete: the body stays for `catalog.dashboard_retention_days`. Only the owner may.
    /// A delete is not a view: it never raises `usage_count` or sets `last_viewed_ms`.
    Delete {
        /// The dashboard id.
        id: String,
        /// The revision the caller read. When given and not current, the delete refuses
        /// `DASHBOARD_REVISION_CONFLICT` and writes nothing. When absent the delete does not
        /// check the revision.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<u64>,
    },
    /// Bring a deleted dashboard back inside its retention. Only the owner may.
    Restore {
        /// The dashboard id.
        id: String,
    },
    /// Read the dashboard through the handler: checks the grant and counts the view.
    Get {
        /// The dashboard id.
        id: String,
    },
}

/// What every dashboard operation answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DashboardReceipt {
    /// The dashboard id.
    pub id: String,
    /// The current revision.
    pub revision: u64,
    /// The owning principal.
    pub owner: String,
    /// The stored grants.
    pub grants: Vec<DashboardGrant>,
    /// The body; present on `Get` and `Save`, absent once purged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<DashboardBody>,
    /// When it was deleted; absent while live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_at_ms: Option<u64>,
    /// How many reads the handler has counted.
    pub usage_count: u64,
    /// When the handler last served a read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_viewed_ms: Option<u64>,
}
