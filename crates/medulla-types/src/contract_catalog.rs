//! The served engine contract rows and page (MEDULLA-CONTRACT-R054).
//!
//! The row types carry exactly the fields of the `catalog_json` rows served by
//! `medulla_capabilities::catalog::catalog_json`, typed: the wire payload of
//! `Method::DescribeContract` is one bounded page of method rows plus, on the
//! first page only, the scope registry rows.

use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContractMethodRow {
    /// The exact `Method` serde tag.
    pub id: String,
    /// The domain registry key the descriptor row is filed under.
    pub domain: String,
    /// True when at least one generated consumer profile exists.
    pub is_wire_callable: bool,
    /// The error codes the engine may legitimately return for the method.
    pub error_set: Vec<String>,
    /// The method's policy block.
    pub policy: ContractPolicyRow,
    /// The replay class, as its enum name.
    pub replay_class: String,
    /// The generated consumer profiles.
    pub consumer_profiles: Vec<String>,
    /// `stable`, `internal` or `deprecated`.
    pub stability: String,
    /// The row's free-text note.
    pub note: String,
}

/// The policy block of a [`ContractMethodRow`], typed from the `catalog_json`
/// row's `policy` object.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContractPolicyRow {
    pub mutates: bool,
    pub durability_domain: String,
    pub authz_action: String,
    pub idempotent: bool,
    pub audited: bool,
    pub emits_cdc: bool,
    pub txn_participation: String,
}

/// One scope-registry row, typed from the `catalog_json` scope entries.
/// `approver_group` is present exactly for approver-class scopes.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContractScopeRow {
    pub scope: String,
    pub class: String,
    pub owner: String,
    pub approver_group: Option<String>,
}

/// One bounded page of the served engine contract.
///
/// `methods` is in ascending `id` byte order and starts at the first `id`
/// strictly greater than the request's `cursor`. `scopes` is filled only when
/// the request carried no cursor (the first page) and is empty on later pages.
/// `schema_digest` (64 lowercase hex, `contract_catalog_digest()`) is on every
/// page, so a client that sees it change between pages restarts. `next_cursor`
/// is the `id` of the page's last method when more pages follow, and `None` on
/// the last page.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DescribeContractPage {
    pub methods: Vec<ContractMethodRow>,
    pub scopes: Vec<ContractScopeRow>,
    pub schema_digest: String,
    pub next_cursor: Option<String>,
}
