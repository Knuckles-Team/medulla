//! A proposed canonical hierarchical concept path (MEDULLA-TYPED-PACKS-R071.100).
//!
//! Producers submit proposed paths as `concept_path` pack entries; the engine's
//! vocabulary validation owns canonical identity. The record is plain data: the
//! grammar and the resolution against the pinned vocabulary revision live in the
//! importer, so one validator serves every producer.

use serde::{Deserialize, Serialize};

/// Format identity of [`ConceptPathRecord`].
pub const CONCEPT_PATH_SCHEMA_VERSION: u16 = 1;
/// Most alias paths one record may carry.
pub const MAX_CONCEPT_PATH_ALIASES: usize = 16;
/// Most bytes in one path (canonical or alias).
pub const MAX_CONCEPT_PATH_BYTES: usize = 1024;

/// One proposed canonical path for one vocabulary class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ConceptPathRecord {
    /// Always [`CONCEPT_PATH_SCHEMA_VERSION`].
    pub schema_version: u16,
    /// The canonical path: `/`-joined segments, `%2F` a literal slash, `%25` a
    /// literal percent, NFC, case-sensitive.
    pub path: String,
    /// The existing class IRI the path names. Published IRIs never change.
    pub class_iri: String,
    /// The vocabulary revision (the composed schema digest) the path was
    /// proposed against.
    pub vocabulary_revision: String,
    /// Other paths that resolve to the same class.
    #[serde(default)]
    pub aliases: Vec<String>,
}
