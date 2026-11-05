//! Object-model declarations a connector pack ships: value types, link types and
//! interfaces (MEDULLA-TYPED-PACKS-R650.1).
//!
//! The ontology terms are `:ValueType` (`core-foundation-v1.ttl`) and `:LinkType`,
//! `:Interface`, `:implementsInterface` (`system-v1.ttl`). The records below are
//! closed serde types: an unknown field refuses the declaration.

use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

/// A named constrained datatype. `base` is an XSD datatype IRI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueType {
    pub name: String,
    pub base: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_len: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub one_of: Vec<String>,
}

/// A typed link between two classes. `max_card` of 0 is invalid; absent means unbounded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkType {
    pub name: String,
    pub from_class: String,
    pub to_class: String,
    #[serde(default)]
    pub min_card: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_card: Option<NonZeroU32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub junction: Option<String>,
}

/// An interface: the properties and links an implementing class must carry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceDecl {
    pub name: String,
    #[serde(default)]
    pub required_properties: Vec<String>,
    #[serde(default)]
    pub required_links: Vec<String>,
}
