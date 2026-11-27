//! A connector pack release as a versioned contract, and the time-boxed grant
//! that lets an ontology stand alone (MEDULLA-TYPED-PACKS-R107.1).
//!
//! Pure data and field validation, plus the two records the connector's head
//! row stores for the import-time diff (`MEDULLA-TYPED-PACKS-R107.3`): the
//! active release summary and the one pending breaking release.

use std::collections::{BTreeMap, BTreeSet};

use crate::stewardship::ChangeRequest;
use medulla_types::contract::Digest256;
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
    /// Entry URI plus `#in` or `#out` to the nested schema the entry declared
    /// (`MEDULLA-TYPED-PACKS-R001.100`).
    #[serde(default)]
    pub schemas: BTreeMap<String, SchemaDigest>,
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
        for (key, schema) in &self.schemas {
            text("schemas", key)?;
            schema.validate()?;
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
    /// The held pack's contents, staged in the engine body store when the release is held,
    /// so activation installs the whole pack (MEDULLA-TYPED-PACKS-R107.6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged: Option<StagedPack>,
}

/// One engine body of a staged pack stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StagedPart {
    pub sha256: Digest256,
    pub manifest_digest: String,
    pub length: u64,
}

/// The staged import of a held release: the import request and the pack archive, as one byte
/// stream split into engine bodies in order. The stream is the 8-byte big-endian length of the
/// request JSON, the request JSON, then the archive bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StagedPack {
    pub parts: Vec<StagedPart>,
}

impl PendingPackRelease {
    pub fn validate(&self) -> Result<(), PackContractError> {
        self.release.validate()?;
        if self
            .staged
            .as_ref()
            .is_some_and(|staged| staged.parts.is_empty())
        {
            return Err(invalid("staged"));
        }
        for found in &self.breaks {
            found.validate()?;
        }
        if self.breaks.is_empty() || self.total_breaks < self.breaks.len() as u64 {
            return Err(invalid("breaks"));
        }
        Ok(())
    }
}

/// How a nested schema release relates to the one it replaces
/// (`MEDULLA-TYPED-PACKS-R001.100`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum CompatVerdict {
    /// Every field, type and required status is unchanged.
    Identical,
    /// Only new optional fields were added.
    AdditiveOptional,
    /// A field was removed, retyped or changed its required status.
    Breaking,
}

/// One field of a nested schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FieldSpec {
    /// The declared JSON type, or empty when the field declares none.
    pub declared_type: String,
    pub required: bool,
}

/// The nested field schema an entry declares, read from its schema document.
///
/// The document names its own evolution with three top-level members:
/// `x-medulla-schema-version` (default 1), `x-medulla-compat-revision`
/// (default 0) and `x-medulla-base-digest`, the digest of the schema the
/// author evolved from. Each nested object property contributes a path
/// (`#/a`, `#/a/b`); an array contributes its items under `[]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct NestedSchema {
    pub schema_version: u64,
    pub compat_revision: u64,
    #[serde(default)]
    pub base_digest: Option<Digest256>,
    pub fields: BTreeMap<String, FieldSpec>,
}

/// The base and breaks recorded when a schema replaced another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SchemaCompatRecord {
    pub verdict: CompatVerdict,
    /// The digest of the schema this one replaced.
    pub base_digest: Digest256,
    pub changed_paths: Vec<String>,
}

/// A nested schema bound to the digest of the document it was read from, as
/// the active release stores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SchemaDigest {
    pub digest: Digest256,
    pub schema: NestedSchema,
    #[serde(default)]
    pub compat: Option<SchemaCompatRecord>,
}

impl SchemaDigest {
    pub fn validate(&self) -> Result<(), PackContractError> {
        for path in self.schema.fields.keys() {
            text("fields", path)?;
        }
        if let Some(record) = &self.compat {
            for path in &record.changed_paths {
                text("changed_paths", path)?;
            }
        }
        Ok(())
    }
}

fn walk(
    node: &serde_json::Value,
    path: &str,
    required_here: bool,
    out: &mut BTreeMap<String, FieldSpec>,
) {
    let declared_type = node
        .get("type")
        .map(serde_json::Value::to_string)
        .unwrap_or_default();
    if !path.is_empty() {
        out.insert(
            path.to_string(),
            FieldSpec {
                declared_type,
                required: required_here,
            },
        );
    }
    if let Some(properties) = node.get("properties").and_then(|p| p.as_object()) {
        let required: BTreeSet<&str> = node
            .get("required")
            .and_then(|r| r.as_array())
            .map(|names| names.iter().filter_map(|name| name.as_str()).collect())
            .unwrap_or_default();
        for (name, child) in properties {
            walk(
                child,
                &format!("{path}/{name}"),
                required.contains(name.as_str()),
                out,
            );
        }
    }
    if let Some(items) = node.get("items").filter(|items| items.is_object()) {
        walk(items, &format!("{path}/[]"), true, out);
    }
}

impl NestedSchema {
    /// Read the nested schema out of a schema document.
    pub fn from_document(document: &serde_json::Value) -> Result<Self, PackContractError> {
        let version = |key: &'static str, default: u64| -> Result<u64, PackContractError> {
            match document.get(key) {
                None => Ok(default),
                Some(value) => value.as_u64().ok_or_else(|| invalid(key)),
            }
        };
        let base_digest = match document.get("x-medulla-base-digest") {
            None => None,
            Some(value) => {
                let text = value
                    .as_str()
                    .ok_or_else(|| invalid("x-medulla-base-digest"))?;
                digest("x-medulla-base-digest", text)?;
                Some(
                    Digest256::parse(text.trim_start_matches("sha256:"))
                        .map_err(|_| invalid("x-medulla-base-digest"))?,
                )
            }
        };
        let mut fields = BTreeMap::new();
        walk(document, "", true, &mut fields);
        Ok(Self {
            schema_version: version("x-medulla-schema-version", 1)?,
            compat_revision: version("x-medulla-compat-revision", 0)?,
            base_digest,
            fields,
        })
    }
}

/// Compare a next schema against the recorded base. A new optional field is
/// additive; a removed field, a changed type or a changed required status is
/// breaking. The changed paths are every path that differs.
pub fn compat_verdict(base: &SchemaDigest, next: &NestedSchema) -> (CompatVerdict, Vec<String>) {
    let mut verdict = CompatVerdict::Identical;
    let mut changed = Vec::new();
    for (path, was) in &base.schema.fields {
        match next.fields.get(path) {
            None => {
                verdict = CompatVerdict::Breaking;
                changed.push(format!("removed {path}"));
            }
            Some(now) if now.declared_type != was.declared_type => {
                verdict = CompatVerdict::Breaking;
                changed.push(format!("retyped {path}"));
            }
            Some(now) if now.required != was.required => {
                verdict = CompatVerdict::Breaking;
                changed.push(format!("required changed {path}"));
            }
            Some(_) => {}
        }
    }
    for (path, now) in &next.fields {
        if base.schema.fields.contains_key(path) {
            continue;
        }
        changed.push(format!("added {path}"));
        if now.required {
            verdict = CompatVerdict::Breaking;
        } else if verdict == CompatVerdict::Identical {
            verdict = CompatVerdict::AdditiveOptional;
        }
    }
    (verdict, changed)
}

#[cfg(test)]
mod nested_schema_tests {
    use super::*;

    fn digest_of(seed: u8) -> Digest256 {
        Digest256::from_bytes([seed; 32])
    }

    fn recorded(document: serde_json::Value, seed: u8) -> SchemaDigest {
        SchemaDigest {
            digest: digest_of(seed),
            schema: NestedSchema::from_document(&document).expect("schema reads"),
            compat: None,
        }
    }

    fn base() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "title": {"type": "string"},
                "owner": {
                    "type": "object",
                    "properties": {"name": {"type": "string"}},
                    "required": ["name"]
                }
            },
            "required": ["title"]
        })
    }

    fn next(change: impl FnOnce(&mut serde_json::Value)) -> NestedSchema {
        let mut document = base();
        change(&mut document);
        NestedSchema::from_document(&document).expect("schema reads")
    }

    // spec: MEDULLA-TYPED-PACKS-R001.100
    #[test]
    fn verdicts_follow_the_row() {
        let held = recorded(base(), 1);
        assert_eq!(
            compat_verdict(&held, &next(|_| {})).0,
            CompatVerdict::Identical
        );
        let added = next(|d| {
            d["properties"]["owner"]["properties"]["team"] = serde_json::json!({"type": "string"});
        });
        let (verdict, changed) = compat_verdict(&held, &added);
        assert_eq!(verdict, CompatVerdict::AdditiveOptional);
        assert_eq!(changed, vec!["added /owner/team".to_string()]);
        let removed = next(|d| {
            d["properties"]["owner"]["properties"]
                .as_object_mut()
                .unwrap()
                .remove("name");
        });
        assert_eq!(compat_verdict(&held, &removed).0, CompatVerdict::Breaking);
        let retyped = next(|d| {
            d["properties"]["title"] = serde_json::json!({"type": "integer"});
        });
        assert_eq!(compat_verdict(&held, &retyped).0, CompatVerdict::Breaking);
        let relaxed = next(|d| d["required"] = serde_json::json!([]));
        assert_eq!(compat_verdict(&held, &relaxed).0, CompatVerdict::Breaking);
        let required_new = next(|d| {
            d["properties"]["tag"] = serde_json::json!({"type": "string"});
            d["required"] = serde_json::json!(["title", "tag"]);
        });
        assert_eq!(
            compat_verdict(&held, &required_new).0,
            CompatVerdict::Breaking
        );
    }

    // spec: MEDULLA-TYPED-PACKS-R001.100
    #[test]
    fn evolution_members_are_read_and_checked() {
        let mut document = base();
        document["x-medulla-schema-version"] = serde_json::json!(3);
        document["x-medulla-compat-revision"] = serde_json::json!(2);
        document["x-medulla-base-digest"] =
            serde_json::json!(format!("sha256:{}", "ab".repeat(32)));
        let schema = NestedSchema::from_document(&document).expect("reads");
        assert_eq!((schema.schema_version, schema.compat_revision), (3, 2));
        assert!(schema.base_digest.is_some());
        document["x-medulla-base-digest"] = serde_json::json!("sha256:short");
        assert!(NestedSchema::from_document(&document).is_err());
        document["x-medulla-base-digest"] = serde_json::json!(null);
        assert!(NestedSchema::from_document(&document).is_err());
    }
}
