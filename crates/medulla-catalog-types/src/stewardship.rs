//! Stewardship roles, change requests and certification records
//! (MEDULLA-TYPED-PACKS-R802.1).
//!
//! Pure data and field validation. Scope resolution, the change-request flow
//! and served admission are later children of `MEDULLA-TYPED-PACKS-R802`.

use serde::{Deserialize, Serialize};

/// Longest identifier, principal or asset reference a record may carry.
pub const MAX_STEWARDSHIP_TEXT_BYTES: usize = 512;
/// Most reviewers one change request may name.
pub const MAX_CHANGE_REQUEST_REVIEWERS: usize = 32;
/// Default certification lifetime: 365 days, in seconds.
pub const DEFAULT_CERTIFICATION_TTL_SECS: u64 = 365 * 24 * 60 * 60;

/// What a stewardship assignment governs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum StewardshipScope {
    Domain(String),
    ConceptScheme(String),
    Asset(String),
}

impl StewardshipScope {
    fn id(&self) -> &str {
        match self {
            Self::Domain(id) | Self::ConceptScheme(id) | Self::Asset(id) => id,
        }
    }
}

/// The closed set of stewardship roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum StewardRole {
    Owner,
    Steward,
    Custodian,
}

/// One principal holding one role over one scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StewardshipAssignment {
    pub scope: StewardshipScope,
    pub role: StewardRole,
    pub principal: String,
}

pub use medulla_types::change_request_machine::ChangeRequestState;

/// A proposed change to a governed asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ChangeRequest {
    pub target: String,
    /// `sha256:<64 lowercase hex>` of the proposed asset content.
    pub proposed_digest: String,
    pub state: ChangeRequestState,
    pub reviewers: Vec<String>,
}

/// A steward's statement that an asset is trusted, until it expires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Certification {
    pub asset: String,
    pub level: String,
    pub by: String,
    /// Unix seconds after which the certification no longer holds.
    pub expires_at: u64,
}

/// A record field that failed validation, named so a caller can fix it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StewardshipFieldError {
    pub field: &'static str,
    pub reason: &'static str,
}

impl std::fmt::Display for StewardshipFieldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.reason)
    }
}

impl std::error::Error for StewardshipFieldError {}

fn text(field: &'static str, value: &str) -> Result<(), StewardshipFieldError> {
    let reason = if value.trim().is_empty() {
        "must not be empty"
    } else if value.len() > MAX_STEWARDSHIP_TEXT_BYTES {
        "exceeds the text bound"
    } else if value.trim() != value || value.chars().any(char::is_control) {
        "must not carry padding or control characters"
    } else {
        return Ok(());
    };
    Err(StewardshipFieldError { field, reason })
}

fn sha256_digest(field: &'static str, value: &str) -> Result<(), StewardshipFieldError> {
    let hex = value.strip_prefix("sha256:").unwrap_or_default();
    if hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Ok(())
    } else {
        Err(StewardshipFieldError {
            field,
            reason: "must be sha256:<64 lowercase hex>",
        })
    }
}

impl StewardshipAssignment {
    pub fn validate(&self) -> Result<(), StewardshipFieldError> {
        text("scope", self.scope.id())?;
        text("principal", &self.principal)
    }

    /// An assignment read back from the typed properties of its
    /// `cat:StewardshipAssignment` node (`scope_kind`, `scope_id`, `role`, `principal`), the
    /// one decoder the stewardship writer and the catalog reads share. `None` when a field is
    /// absent or names no scope kind or role.
    pub fn from_properties(
        properties: &serde_json::Map<String, serde_json::Value>,
    ) -> Option<Self> {
        let text = |key: &str| properties.get(key).and_then(serde_json::Value::as_str);
        let id = text("scope_id")?.to_string();
        let scope = match text("scope_kind")? {
            "asset" => StewardshipScope::Asset(id),
            "concept_scheme" => StewardshipScope::ConceptScheme(id),
            "domain" => StewardshipScope::Domain(id),
            _ => return None,
        };
        let role = match text("role")? {
            "owner" => StewardRole::Owner,
            "steward" => StewardRole::Steward,
            "custodian" => StewardRole::Custodian,
            _ => return None,
        };
        Some(Self {
            scope,
            role,
            principal: text("principal")?.to_string(),
        })
    }
}

impl ChangeRequest {
    pub fn validate(&self) -> Result<(), StewardshipFieldError> {
        text("target", &self.target)?;
        sha256_digest("proposed_digest", &self.proposed_digest)?;
        if self.reviewers.len() > MAX_CHANGE_REQUEST_REVIEWERS {
            return Err(StewardshipFieldError {
                field: "reviewers",
                reason: "exceeds the reviewer bound",
            });
        }
        let mut seen = std::collections::BTreeSet::new();
        for reviewer in &self.reviewers {
            text("reviewers", reviewer)?;
            if !seen.insert(reviewer.as_str()) {
                return Err(StewardshipFieldError {
                    field: "reviewers",
                    reason: "names a reviewer twice",
                });
            }
        }
        Ok(())
    }
}

impl Certification {
    pub fn validate(&self) -> Result<(), StewardshipFieldError> {
        text("asset", &self.asset)?;
        text("level", &self.level)?;
        text("by", &self.by)?;
        if self.expires_at == 0 {
            return Err(StewardshipFieldError {
                field: "expires_at",
                reason: "must be a positive unix time",
            });
        }
        Ok(())
    }

    /// Whether the certification still holds at `now` (unix seconds).
    pub fn holds_at(&self, now: u64) -> bool {
        now < self.expires_at
    }
}

/// The scopes one target belongs to, named on the wire so the engine can resolve
/// the governing assignments (MEDULLA-TYPED-PACKS-R802.3).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StewardshipTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concept_scheme: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
}

/// Node label of one principal's trust flag on one asset (MEDULLA-TYPED-PACKS-R802.7).
pub const TRUST_FLAG_LABEL: &str = "cat:TrustFlag";
/// The mutation outbox topic a warning or deprecation flag is announced on.
pub const TRUST_FLAG_TOPIC: &str = "catalog.trust_flag";
/// The flags a principal may set on an asset.
pub const TRUST_FLAGS: [&str; 3] = ["endorsement", "warning", "deprecation"];
/// Whether setting `flag` is announced on [`TRUST_FLAG_TOPIC`]: a warning or a deprecation.
pub fn trust_flag_is_announced(flag: &str) -> bool {
    matches!(flag, "warning" | "deprecation")
}

/// The property of a trust-flag node that names the tenant that set it.
pub const TRUST_FLAG_TENANT_PROPERTY: &str = "tenant_id";

/// The assets `tenant` has flagged `deprecation`: every one of `flags` (the properties of the
/// `cat:TrustFlag` nodes) whose `flag` is `deprecation` and whose tenant is `tenant`. One
/// principal's deprecation stands whatever another principal flags the same asset
/// (MEDULLA-FEDERATED-QUERY-R708.9).
pub fn deprecated_assets<'a>(
    flags: impl IntoIterator<Item = &'a serde_json::Map<String, serde_json::Value>>,
    tenant: &str,
) -> std::collections::BTreeSet<String> {
    fn text<'n>(node: &'n serde_json::Map<String, serde_json::Value>, key: &str) -> Option<&'n str> {
        node.get(key).and_then(serde_json::Value::as_str)
    }
    flags
        .into_iter()
        .filter(|node| {
            text(node, "flag") == Some("deprecation")
                && text(node, TRUST_FLAG_TENANT_PROPERTY) == Some(tenant)
        })
        .filter_map(|node| text(node, "asset").map(str::to_string))
        .collect()
}

/// One stewardship operation (`Method::Stewardship`, MEDULLA-TYPED-PACKS-R802.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum StewardshipOp {
    /// Record one principal's role over one scope.
    Assign { assignment: StewardshipAssignment },
    /// Remove one assignment.
    Revoke { assignment: StewardshipAssignment },
    /// Open a change request for `target`, naming its governing owners and stewards
    /// as reviewers.
    OpenChangeRequest {
        target: String,
        proposed_digest: String,
        scopes: StewardshipTarget,
    },
    /// A reviewer approves or rejects an open request.
    Review { request_id: String, approve: bool },
    /// A reviewer applies an approved request and records its evidence.
    Apply { request_id: String },
    /// A steward or owner certifies an asset at `level`.
    Certify { asset: String, level: String },
    /// A steward or owner marks a glossary term or column critical
    /// (MEDULLA-TYPED-PACKS-R802.8). The element then needs an owner, a steward and one
    /// active quality rule; a repeat refreshes what it misses.
    MarkCritical { element: String },
    /// Set the caller's trust flag on an asset (MEDULLA-TYPED-PACKS-R802.7): `flag` is
    /// `endorsement`, `warning` or `deprecation`, and `reason` says why. A caller holds one
    /// flag per asset; setting another replaces it.
    SetTrustFlag {
        asset: String,
        flag: String,
        reason: String,
    },
}

impl StewardshipOp {
    /// Every stewardship operation writes.
    pub fn is_mutation(&self) -> bool {
        true
    }
}

/// The answer of a stewardship operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StewardshipReceipt {
    /// The id of the node the operation wrote.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_request: Option<ChangeRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certification: Option<Certification>,
    /// The owners that govern the addressed target, so a caller can decide whom to admit.
    pub owners: Vec<String>,
    /// The stewards that govern the addressed target.
    pub stewards: Vec<String>,
}

#[cfg(test)]
mod deprecation_tests {
    use super::*;
    use serde_json::json;

    fn flag(asset: &str, flag: &str, tenant: &str) -> serde_json::Map<String, serde_json::Value> {
        json!({"asset": asset, "flag": flag, "tenant_id": tenant})
            .as_object()
            .cloned()
            .unwrap_or_default()
    }

    // spec: MEDULLA-FEDERATED-QUERY-R708.9
    #[test]
    fn only_the_tenants_deprecation_flags_name_a_deprecated_asset() {
        let flags = [
            flag("orders", "deprecation", "t1"),
            flag("users", "endorsement", "t1"),
            flag("ledger", "deprecation", "t2"),
        ];
        let found = deprecated_assets(&flags, "t1");
        assert_eq!(found.into_iter().collect::<Vec<_>>(), vec!["orders".to_string()]);
    }
}
