//! The data types of entry trust (MEDULLA-DECISION-ENGINE-R132.1, R132.3).
//!
//! They live here, beneath `TypedValue` and `DecisionPolicy`, because both hold
//! them; the admission behaviour stays in `medulla-decision-outcome-types`.

use std::collections::BTreeSet;

use medulla_types::refusal_code::RefusalCode;
use serde::{Deserialize, Serialize};

/// Every retrieved entry that names a required fact was refused
/// (MEDULLA-DECISION-ENGINE-R132.4).
pub const POISONED_ENTRY: RefusalCode = RefusalCode::new("POISONED_ENTRY");

/// The `entry_trust` table of registered refusal codes (MEDULLA-DECISION-ENGINE-R010.100).
pub const REFUSALS: &[RefusalCode] = &[POISONED_ENTRY];

/// Closed trust tiers, ordered weakest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum TrustTier {
    Unverified,
    Attested,
    Verified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct EntryTrust {
    pub tier: TrustTier,
    pub source_digest: String,
    pub observed_at: i64,
}

/// One retrieved memory or knowledge entry offered as a premise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RetrievedEntry {
    pub entry_id: String,
    pub subject: String,
    pub fact_key: String,
    pub value: String,
    pub trust: EntryTrust,
}

/// The source record: the digests of sources the engine actually holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct EntryTrustPolicy {
    pub known_source_digests: BTreeSet<String>,
}

impl EntryTrustPolicy {
    /// Whether the policy knows no source digest: the serde skip predicate that
    /// keeps an existing policy's digest.
    pub fn is_empty(&self) -> bool {
        self.known_source_digests.is_empty()
    }
}

/// The named rule that refused an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum EntryCheck {
    TierOrder,
    PoisonCheck,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RefusedEntry {
    pub entry_id: String,
    pub tier: TrustTier,
    pub check: EntryCheck,
}

