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
pub enum DashboardRole {
    /// Read the dashboard.
    View,
    /// Read and save the dashboard.
    Edit,
}

/// A grant of one role to one principal or group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DashboardGrant {
    /// The principal or group the grant names.
    pub principal_or_group: String,
    /// The role it holds.
    pub role: DashboardRole,
}

/// One panel and the links that name its data source, query, terms and concepts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
