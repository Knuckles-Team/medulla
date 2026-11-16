//! Skill graph bundle types (MEDULLA-TYPED-PACKS-R700.1).
//!
//! A skill graph is a `SKILL.md` with an `index.json`, a `sources.json` and
//! hash-named `reference/` files. `SkillGraphManifest` is the manifest read from
//! the graph's `sources.json` (`files[]`), `ConformanceReport` is what
//! `validate_bundle` (R700.2) returns for it, and `SkillCandidate` is a
//! distilled bundle awaiting a decision. All types are pure serde with
//! `deny_unknown_fields`.

use serde::{Deserialize, Serialize};

/// Why a skill graph value was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SkillGraphTypeError {
    /// A digest was not 64 lowercase hexadecimal characters.
    #[error("sha256 must be 64 lowercase hex characters")]
    BadDigest,
    /// `sources.json` had no `files` array or an entry without `path` or `sha256`.
    #[error("sources.json files[] entry is malformed: {0}")]
    MalformedSources(String),
}

/// A SHA-256 as 64 lowercase hex characters, without any `sha256:` prefix.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha256Hex(String);

impl Sha256Hex {
    /// Validates 64 lowercase hex characters.
    pub fn new(hex: impl Into<String>) -> Result<Self, SkillGraphTypeError> {
        let hex = hex.into();
        let ok = hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if ok {
            Ok(Self(hex))
        } else {
            Err(SkillGraphTypeError::BadDigest)
        }
    }

    /// Accepts the `sources.json` spelling `sha256:<hex>` (prefix stripped) or bare hex.
    pub fn from_sources_value(value: &str) -> Result<Self, SkillGraphTypeError> {
        Self::new(value.strip_prefix("sha256:").unwrap_or(value))
    }

    /// The lowercase hex text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Sha256Hex {
    type Error = SkillGraphTypeError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Sha256Hex> for String {
    fn from(value: Sha256Hex) -> Self {
        value.0
    }
}

/// One file of a skill graph: its path and the SHA-256 of its canonical text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SkillGraphEntry {
    /// Path inside the graph, for example `reference/<hash>.md`.
    pub path: String,
    /// SHA-256 of the file's canonical text.
    #[cfg_attr(feature = "contract-schema", schemars(with = "String"))]
    pub sha256: Sha256Hex,
}

/// The files a skill graph lists in its `sources.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SkillGraphManifest {
    /// One entry per listed file, in manifest order.
    pub entries: Vec<SkillGraphEntry>,
}

impl SkillGraphManifest {
    /// Reads the manifest from a parsed `sources.json` (`files[]`, each with
    /// `path` and `sha256`; the `sha256:` prefix is stripped).
    pub fn from_sources_json(sources: &serde_json::Value) -> Result<Self, SkillGraphTypeError> {
        let files = sources
            .get("files")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| SkillGraphTypeError::MalformedSources("missing files array".into()))?;
        let mut entries = Vec::with_capacity(files.len());
        for file in files {
            let path = file
                .get("path")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    SkillGraphTypeError::MalformedSources("entry without path".into())
                })?;
            let digest = file
                .get("sha256")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    SkillGraphTypeError::MalformedSources(format!("{path} has no sha256"))
                })?;
            entries.push(SkillGraphEntry {
                path: path.to_string(),
                sha256: Sha256Hex::from_sources_value(digest)?,
            });
        }
        Ok(Self { entries })
    }
}

/// The result of checking a skill graph bundle: every rule checked and every
/// violation found.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ConformanceReport {
    /// Ids of the rules that were checked.
    pub checked_rules: Vec<String>,
    /// One message per violation, each naming the failing path.
    pub violations: Vec<String>,
}

impl ConformanceReport {
    /// Whether the bundle conforms (no violation).
    pub fn is_conformant(&self) -> bool {
        self.violations.is_empty()
    }
}

/// The state of a skill candidate. Only `proposed` exists here: promotion is a
/// decision of the decision engine (`MEDULLA-DECISION-ENGINE-R805`), never an
/// automatic publish.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum SkillCandidateState {
    /// Proposed, awaiting a decision.
    Proposed,
}

/// A distilled skill bundle proposed for promotion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SkillCandidate {
    /// Candidate id.
    pub candidate_id: String,
    /// The process (or source set) the candidate was distilled from.
    pub source_process: String,
    /// Always `proposed`.
    pub state: SkillCandidateState,
}
