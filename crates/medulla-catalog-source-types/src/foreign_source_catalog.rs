//! The served foreign source catalog (MEDULLA-UNIFIED-DATA-PLANE-R765): what a caller reads
//! back about the sources it may see. A view names a source and its kind. It never carries
//! the registered specification, so no connection string, token or other credential value
//! can leave through it; secrets stay references inside the registration.

use serde::{Deserialize, Serialize};

/// Refusal code: a named source is not visible to the caller. A source nobody registered and a
/// source another principal registered and did not share refuse the same way.
pub const SOURCE_NOT_FOUND: &str = "SOURCE_NOT_FOUND";
/// Refusal code: only the owner of a source may share or unshare it.
pub const SOURCE_OWNER_REQUIRED: &str = "SOURCE_OWNER_REQUIRED";
/// Most principals one share names.
pub const MAX_SHARE_GRANTEES: usize = 64;
/// Refusal code: a parity request names no table, too many tables or a blank table.
pub const PARITY_TABLES_INVALID: &str = "PARITY_TABLES_INVALID";
/// Refusal code: a parity sample size is zero or past [`MAX_PARITY_SAMPLE`].
pub const PARITY_SAMPLE_INVALID: &str = "PARITY_SAMPLE_INVALID";
/// Most tables one parity request names.
pub const MAX_PARITY_TABLES: usize = 64;
/// Most sampled keys one parity digest covers.
pub const MAX_PARITY_SAMPLE: u32 = 10_000;

/// One source visible to the caller.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ForeignSourceView {
    /// The name the caller addresses it by: the plain name for an owned source, the
    /// qualified `<owner agent>/<name>` for a shared one.
    pub name: String,
    /// The agent that registered it.
    pub owner: String,
    /// The source kind (`Sql`, `HttpJson`, `Mcp`, ...), never its configuration.
    pub kind: String,
    /// Whether the caller registered it.
    pub owned: bool,
    /// The catalog probe state, in snake case.
    pub probe_state: String,
}

/// The answer of one probe: the state the source's catalog probe stands in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ForeignSourceProbe {
    /// The probed source's name.
    pub name: String,
    /// The catalog probe state after the probe, in snake case.
    pub probe_state: String,
}

/// The answer of one parity read (MEDULLA-UNIFIED-DATA-PLANE-R770): the row count of each named
/// table and one digest over the sampled keys. It carries no row content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ForeignSourceParityAnswer {
    /// The row count of each requested table, keyed by the table name as requested.
    pub counts: std::collections::BTreeMap<String, u64>,
    /// `sha256:<64 hex>` over up to `sample` keys, taken in sorted `(table, key)` order.
    pub sample_digest: String,
}
