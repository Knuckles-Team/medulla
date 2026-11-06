//! Ingest capability types (MEDULLA-REPO-INGEST-R002.650).
//!
//! `ChangeEnvelope` and `ChangeEnvelopeCommit` stay in
//! `medulla-protocol-types`. These records add the fragment address, the
//! supersession link, the dead-letter item and the conformance report that
//! `ApplyChangeEnvelope` and `SourceIngest` carry. Each is serde-stable and
//! carries `schema_version`.

use serde::{Deserialize, Serialize};

use crate::contract::Digest256;

/// Wire version of every record in this module.
pub const INGEST_CAPABILITIES_SCHEMA_VERSION: u32 = 1;

fn schema_version() -> u32 {
    INGEST_CAPABILITIES_SCHEMA_VERSION
}

/// A byte span of one content-addressed document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FragmentAddress {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    pub content_digest: Digest256,
    pub span_start: u64,
    pub span_end: u64,
}

/// A new fact replacing an old one, recorded by the envelope that did it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Supersession {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    pub old_fact: String,
    pub new_fact: String,
    pub envelope_id: String,
}

/// One parked queue item with the reason it was parked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeadLetterItem {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    pub item_id: String,
    pub queue: String,
    pub reason: String,
    pub attempts: u32,
    pub last_error: String,
    pub parked_at_ms: u64,
    pub payload_digest: Digest256,
}

/// The result of replaying traces against a process model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConformanceReport {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    pub process_id: String,
    pub traces: u64,
    pub fitness_milli: u32,
    pub missing_tokens: u64,
    pub remaining_tokens: u64,
}

/// The type names this module serves, in catalog order.
pub const INGEST_CAPABILITY_TYPE_NAMES: [&str; 4] = [
    "ConformanceReport",
    "DeadLetterItem",
    "FragmentAddress",
    "Supersession",
];
