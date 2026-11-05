//! Graph ownership signals and the grant plan (MEDULLA-IDENTITY-R700.1).
//!
//! `GraphOwnershipPlan` (R700.3) builds [`GraphOwnershipSignals`] from the
//! graph record and its write counters; the planner (R700.2) turns them into
//! [`GrantPlanEntry`]s and keeps a [`RollbackEntry`] per principal so apply
//! can be undone exactly. Every type carries `schema_version` and refuses
//! unknown fields; [`plan_digest`] is SHA-256 over the canonical plan.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Wire schema version of the ownership types.
pub const OWNERSHIP_SCHEMA_VERSION: u32 = 1;
/// Most writers one signal set may carry.
pub const MAX_OWNERSHIP_WRITERS: usize = 256;
/// Most roles one grant entry may carry.
pub const MAX_OWNERSHIP_ROLES: usize = 64;

/// What the planner reads for one graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphOwnershipSignals {
    pub schema_version: u32,
    pub graph: String,
    #[serde(default)]
    pub creator: Option<String>,
    /// `(principal, writes_30d)`: each writer's writes in the last 30 days.
    #[serde(default)]
    pub writers: Vec<(String, u64)>,
    #[serde(default)]
    pub name_hint: Option<String>,
}

/// The access a planned principal receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnershipDisposition {
    Owner,
    Writer,
    Reader,
}

/// One planned grant: the principal's complete role set after apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantPlanEntry {
    pub schema_version: u32,
    pub principal: String,
    pub disposition: OwnershipDisposition,
    pub full_role_set: BTreeSet<String>,
}

/// The role set a principal held before apply, restored by rollback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollbackEntry {
    pub schema_version: u32,
    pub principal: String,
    pub prior_role_set: BTreeSet<String>,
}

/// An ownership value outside its bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidOwnership {
    SchemaVersion,
    TooManyWriters,
    EmptyRoleSet,
    TooManyRoles,
}

impl std::fmt::Display for InvalidOwnership {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::SchemaVersion => "ownership schema_version is not supported",
            Self::TooManyWriters => "ownership signals carry more than 256 writers",
            Self::EmptyRoleSet => "ownership grant entry has an empty role set",
            Self::TooManyRoles => "ownership grant entry carries more than 64 roles",
        })
    }
}

impl std::error::Error for InvalidOwnership {}

fn check_version(version: u32) -> Result<(), InvalidOwnership> {
    if version == OWNERSHIP_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(InvalidOwnership::SchemaVersion)
    }
}

impl GraphOwnershipSignals {
    /// Validate the bounds.
    pub fn validate(&self) -> Result<(), InvalidOwnership> {
        check_version(self.schema_version)?;
        if self.writers.len() > MAX_OWNERSHIP_WRITERS {
            return Err(InvalidOwnership::TooManyWriters);
        }
        Ok(())
    }
}

impl GrantPlanEntry {
    /// Validate the bounds.
    pub fn validate(&self) -> Result<(), InvalidOwnership> {
        check_version(self.schema_version)?;
        if self.full_role_set.is_empty() {
            return Err(InvalidOwnership::EmptyRoleSet);
        }
        if self.full_role_set.len() > MAX_OWNERSHIP_ROLES {
            return Err(InvalidOwnership::TooManyRoles);
        }
        Ok(())
    }
}

/// SHA-256 over the canonical plan (entries sorted by principal, canonical
/// JSON), 64 lowercase hex characters.
pub fn plan_digest(plan: &[GrantPlanEntry]) -> String {
    let mut sorted: Vec<&GrantPlanEntry> = plan.iter().collect();
    sorted.sort_by(|a, b| a.principal.cmp(&b.principal));
    let canonical = serde_json::to_vec(&sorted).unwrap_or_default();
    hex::encode(Sha256::digest(canonical))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(principal: &str, roles: &[&str]) -> GrantPlanEntry {
        GrantPlanEntry {
            schema_version: OWNERSHIP_SCHEMA_VERSION,
            principal: principal.into(),
            disposition: OwnershipDisposition::Writer,
            full_role_set: roles.iter().map(|r| r.to_string()).collect(),
        }
    }

    fn round_trip<T>(value: &T)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value).unwrap();
        assert_eq!(&serde_json::from_str::<T>(&json).unwrap(), value);
    }

    #[test]
    fn ownership_types_round_trip() {
        let writers = (0..MAX_OWNERSHIP_WRITERS)
            .map(|i| (format!("usr:{i:03}"), i as u64))
            .collect();
        let mut signals = GraphOwnershipSignals {
            schema_version: OWNERSHIP_SCHEMA_VERSION,
            graph: "g".into(),
            creator: Some("usr:000".into()),
            writers,
            name_hint: None,
        };
        assert_eq!(signals.validate(), Ok(()));
        round_trip(&signals);
        signals.writers.push(("usr:256".into(), 1));
        assert_eq!(signals.validate(), Err(InvalidOwnership::TooManyWriters));

        let roles: Vec<String> = (0..MAX_OWNERSHIP_ROLES).map(|i| format!("r{i}")).collect();
        let full = entry(
            "usr:a",
            &roles.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        assert_eq!(full.validate(), Ok(()));
        round_trip(&full);
        assert_eq!(
            entry("usr:a", &[]).validate(),
            Err(InvalidOwnership::EmptyRoleSet)
        );
        let mut over = full.clone();
        over.full_role_set.insert("r64".into());
        assert_eq!(over.validate(), Err(InvalidOwnership::TooManyRoles));

        let rollback = RollbackEntry {
            schema_version: OWNERSHIP_SCHEMA_VERSION,
            principal: "usr:a".into(),
            prior_role_set: BTreeSet::from(["reader".to_string()]),
        };
        round_trip(&rollback);
        for disposition in [
            OwnershipDisposition::Owner,
            OwnershipDisposition::Writer,
            OwnershipDisposition::Reader,
        ] {
            round_trip(&disposition);
        }
        assert!(serde_json::from_str::<RollbackEntry>(
            r#"{"schema_version":1,"principal":"p","prior_role_set":[],"extra":1}"#
        )
        .is_err());

        let a = entry("usr:a", &["writer"]);
        let b = entry("usr:b", &["reader"]);
        let digest = plan_digest(&[a.clone(), b.clone()]);
        assert_eq!(digest.len(), 64);
        assert!(digest.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
        assert_eq!(digest, plan_digest(&[b, a]), "digest is order independent");
    }
}
