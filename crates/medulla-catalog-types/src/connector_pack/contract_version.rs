//! A connector pack release as a versioned contract, and the time-boxed grant
//! that lets an ontology stand alone (MEDULLA-TYPED-PACKS-R107.1).
//!
//! Pure data and field validation, plus the two records the connector's head
//! row stores for the import-time diff (`MEDULLA-TYPED-PACKS-R107.3`): the
//! active release summary and the one pending breaking release.

use std::collections::{BTreeMap, BTreeSet};

use medulla_types::contract::Digest256;
use crate::stewardship::ChangeRequest;
use serde::{Deserialize, Serialize};

/// Longest connector name, version, IRI, reason or owner a record may carry.
pub const MAX_CONTRACT_TEXT_BYTES: usize = 1024;
/// The stable refusal code for a grant without a reason or an expiry.
pub const GRANT_INCOMPLETE: &str = "GRANT_INCOMPLETE";

/// One released contract version of a connector pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PackContractVersion {
    pub connector: String,
    pub version: String,
    /// `sha256:<64 lowercase hex>` of the whole contract.
    pub contract_digest: String,
    /// `sha256:<64 lowercase hex>` of the pack ontology.
    pub ontology_digest: String,
    /// `sha256:<64 lowercase hex>` of the pack shapes.
    pub shapes_digest: String,
    /// The version this release replaces; absent for the first release.
    #[serde(default)]
    pub supersedes: Option<String>,
}

/// A recorded exception that lets an ontology live without a parent link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StandaloneGrant {
    pub connector: String,
    pub ontology_iri: String,
    pub reason: String,
    pub owner: String,
    /// Unix seconds after which the ontology is reported as an orphan.
    pub expires_at: u64,
}

/// What makes a release break its contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ContractBreakKind {
    RemovedClass,
    RetypedClass,
    RemovedProperty,
    RetypedProperty,
    RemovedField,
    RetypedField,
    NarrowedShape,
    RemovedMapping,
}

/// One breaking change a release makes to the active contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ContractBreak {
    pub kind: ContractBreakKind,
    /// The class, property, field path, shape or mapping the break names.
    pub item: String,
}

impl ContractBreak {
    pub fn validate(&self) -> Result<(), PackContractError> {
        text("item", &self.item)
    }
}

/// A record that failed validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackContractError {
    pub code: &'static str,
    pub field: &'static str,
}

impl std::fmt::Display for PackContractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.field)
    }
}

impl std::error::Error for PackContractError {}

fn invalid(field: &'static str) -> PackContractError {
    PackContractError {
        code: "INVALID_FIELD",
        field,
    }
}

fn text(field: &'static str, value: &str) -> Result<(), PackContractError> {
    if value.trim().is_empty()
        || value.trim() != value
        || value.len() > MAX_CONTRACT_TEXT_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(invalid(field));
    }
    Ok(())
}

fn digest(field: &'static str, value: &str) -> Result<(), PackContractError> {
    let hex = value.strip_prefix("sha256:").unwrap_or_default();
    if hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Ok(())
    } else {
        Err(invalid(field))
    }
}

impl PackContractVersion {
    pub fn validate(&self) -> Result<(), PackContractError> {
        text("connector", &self.connector)?;
        text("version", &self.version)?;
        digest("contract_digest", &self.contract_digest)?;
        digest("ontology_digest", &self.ontology_digest)?;
        digest("shapes_digest", &self.shapes_digest)?;
        if let Some(previous) = &self.supersedes {
            text("supersedes", previous)?;
            if *previous == self.version {
                return Err(invalid("supersedes"));
            }
        }
        Ok(())
    }
}

impl StandaloneGrant {
    pub fn validate(&self) -> Result<(), PackContractError> {
        text("connector", &self.connector)?;
        text("ontology_iri", &self.ontology_iri)?;
        text("owner", &self.owner)?;
        if self.reason.trim().is_empty() {
            return Err(PackContractError {
                code: GRANT_INCOMPLETE,
                field: "reason",
            });
        }
        if self.expires_at == 0 {
            return Err(PackContractError {
                code: GRANT_INCOMPLETE,
                field: "expires_at",
            });
        }
        text("reason", &self.reason)
    }

    /// Whether the grant has lapsed at `now` (unix seconds).
    pub fn expired_at(&self, now: u64) -> bool {
        now >= self.expires_at
    }
}

/// The active release of a connector pack as its head row stores it: the
/// version record plus the tool and prompt fields and the mapping ids the next
/// release is diffed against. The ontology and shapes documents are read from
/// the head's own published members, never copied here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PackRelease {
    pub version: PackContractVersion,
    /// Declared tool field path (`<tool uri>#/<property>`) to its declared type.
    pub fields: BTreeMap<String, String>,
    /// Ids of the pack's mappings, `schema:<id>` or `relationship:<id>`.
    pub mappings: BTreeSet<String>,
}

impl PackRelease {
    pub fn validate(&self) -> Result<(), PackContractError> {
        self.version.validate()?;
        for path in self.fields.keys() {
            text("fields", path)?;
        }
        for id in &self.mappings {
            text("mappings", id)?;
        }
        Ok(())
    }
}

/// A breaking release held behind a change request. It sits beside the head
/// row and never moves the head: queries keep resolving through the active
/// release. Its presence is the `pending` state of `PackReleaseMachine`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PendingPackRelease {
    pub release: PackRelease,
    /// The pack the held release came from.
    pub pack_digest: Digest256,
    /// The first `catalog.contract_break_report_max` breaks.
    pub breaks: Vec<ContractBreak>,
    /// Every break the diff found.
    pub total_breaks: u64,
    pub change_request: ChangeRequest,
}

impl PendingPackRelease {
    pub fn validate(&self) -> Result<(), PackContractError> {
        self.release.validate()?;
        for found in &self.breaks {
            found.validate()?;
        }
        if self.breaks.is_empty() || self.total_breaks < self.breaks.len() as u64 {
            return Err(invalid("breaks"));
        }
        Ok(())
    }
}
