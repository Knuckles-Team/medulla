//! Graph-native code edits (MEDULLA-SCM-R013.1): an edit names a symbol and its new
//! text instead of a file diff. The projection to file bytes and one changeset
//! lives in `medulla-compute`.

use serde::{Deserialize, Serialize};

/// What an op does to its symbol.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SymbolEditKind {
    ReplaceSymbol,
    ReplaceBody,
    InsertBefore,
    InsertAfter,
    Delete,
}

/// One op against one symbol, guarded by the digest of the span it expects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SymbolEditOp {
    pub kind: SymbolEditKind,
    pub symbol_id: String,
    pub anchor_span_digest: String,
    pub new_text: String,
}

/// What one op recorded: the span digest before, the digest of the text that replaced it and
/// the re-parse verdict (`confirmed`; a refused edit never lands, so no other value is stored).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SymbolEditOpEvidence {
    pub symbol_id: String,
    pub kind: SymbolEditKind,
    pub old_span_digest: String,
    pub new_span_digest: String,
    pub verdict: String,
}

/// The evidence an accepted symbol edit stores inside its `Changeset` (MEDULLA-SCM-R013.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SymbolEditEvidence {
    /// SHA-256 (64 lowercase hex) over the canonical JSON of the `SymbolEdit`.
    pub edit_digest: String,
    pub ops: Vec<SymbolEditOpEvidence>,
}

impl SymbolEdit {
    /// The digest the changeset records: SHA-256 of the canonical JSON of this edit.
    pub fn digest(&self) -> String {
        crate::lifecycle::sha256_hex(&serde_json::to_vec(self).unwrap_or_default())
    }
}

/// An edit of one branch at one base revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SymbolEdit {
    pub repo: String,
    pub branch: String,
    pub base_rev: String,
    pub ops: Vec<SymbolEditOp>,
    pub message: String,
    pub run_trace_id: String,
}
