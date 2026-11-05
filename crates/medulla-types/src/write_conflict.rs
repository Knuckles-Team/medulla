//! Typed optimistic-write conflict data.
//!
//! This module contains only the wire model. Conflict resolution and the
//! served mutation path live in the crates above `medulla-types` in the workspace
//! DAG.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The policy applied when a writer's expected version is stale.
///
/// This enum is intentionally closed: adding a strategy changes the durable
/// conflict contract and must be reviewed with its resolution semantics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MergeStrategy {
    /// Reject the write without changing the stored state.
    #[default]
    Refuse,
    /// Prefer the incoming value when the scope policy allows it.
    PreferNewer,
    /// Merge non-overlapping fields under the scope policy.
    FieldUnion,
}

/// The version mismatch a durable writer must resolve before committing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteConflict {
    /// Version observed by the losing writer when it prepared its mutation.
    pub expected: u64,
    /// Version currently authoritative for the scope.
    pub actual: u64,
    /// Stable logical scope name used to select the merge policy.
    pub scope: String,
}

/// Which side of a stale write the resolver kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ConflictWinner {
    /// The stored state stood and the incoming write was refused.
    Existing,
    /// The incoming write replaced the stored fields.
    Incoming,
    /// Both sides' fields were kept.
    Merged,
}

impl ConflictWinner {
    /// The closed metric label for this winner.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Existing => "existing",
            Self::Incoming => "incoming",
            Self::Merged => "merged",
        }
    }
}

impl MergeStrategy {
    /// The closed metric label for this strategy.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Refuse => "refuse",
            Self::PreferNewer => "prefer_newer",
            Self::FieldUnion => "field_union",
        }
    }

    /// The winner a strategy names when it resolves (or refuses) a conflict.
    pub const fn winner(self) -> ConflictWinner {
        match self {
            Self::Refuse => ConflictWinner::Existing,
            Self::PreferNewer => ConflictWinner::Incoming,
            Self::FieldUnion => ConflictWinner::Merged,
        }
    }
}

/// What a conflict resolution decided, for the error detail or commit response.
///
/// spec: MEDULLA-DURABLE-KERNEL-R084.5
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WriteConflictEvidence {
    /// The scope the conflict was raised in.
    pub conflict_scope: String,
    /// The strategy that was applied.
    pub strategy: MergeStrategy,
    /// Which side won.
    pub winner: ConflictWinner,
}

impl WriteConflictEvidence {
    /// Evidence for a conflict the resolver settled under `strategy`.
    pub fn resolved(conflict: &WriteConflict, strategy: MergeStrategy) -> Self {
        Self {
            conflict_scope: conflict.scope.clone(),
            strategy,
            winner: strategy.winner(),
        }
    }

    /// Evidence for a conflict that stood: the stored state won.
    pub fn refused(conflict: &WriteConflict, strategy: MergeStrategy) -> Self {
        Self {
            conflict_scope: conflict.scope.clone(),
            strategy,
            winner: ConflictWinner::Existing,
        }
    }
}

/// Resolve a stale write under `strategy`.
///
/// Pure and deterministic. `Refuse` returns the conflict; `PreferNewer`
/// returns `incoming`; `FieldUnion` keeps every key held by one side only and
/// every key with equal values on both sides, and returns the conflict when a
/// key carries different values on both sides.
///
/// spec: MEDULLA-DURABLE-KERNEL-R084.2
pub fn resolve_write_conflict(
    conflict: &WriteConflict,
    strategy: MergeStrategy,
    current: &BTreeMap<String, Value>,
    incoming: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, WriteConflict> {
    match strategy {
        MergeStrategy::Refuse => Err(conflict.clone()),
        MergeStrategy::PreferNewer => Ok(incoming.clone()),
        MergeStrategy::FieldUnion => {
            let mut merged = current.clone();
            for (key, value) in incoming {
                match merged.get(key) {
                    Some(existing) if existing != value => return Err(conflict.clone()),
                    _ => {
                        merged.insert(key.clone(), value.clone());
                    }
                }
            }
            Ok(merged)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DURABLE-KERNEL-R084.1
    #[test]
    fn conflict_and_strategies_round_trip_with_refuse_as_default() {
        let conflict = WriteConflict {
            expected: 7,
            actual: 8,
            scope: "tenant-a/graph-main".to_owned(),
        };
        let encoded = serde_json::to_string(&conflict).expect("conflict encodes");
        assert_eq!(
            serde_json::from_str::<WriteConflict>(&encoded).expect("conflict decodes"),
            conflict
        );

        for strategy in [
            MergeStrategy::Refuse,
            MergeStrategy::PreferNewer,
            MergeStrategy::FieldUnion,
        ] {
            let encoded = serde_json::to_string(&strategy).expect("strategy encodes");
            assert_eq!(
                serde_json::from_str::<MergeStrategy>(&encoded).expect("strategy decodes"),
                strategy
            );
        }
        assert_eq!(MergeStrategy::default(), MergeStrategy::Refuse);
    }

    // spec: MEDULLA-DURABLE-KERNEL-R084.2
    #[test]
    fn resolve_under_each_strategy() {
        let conflict = WriteConflict {
            expected: 1,
            actual: 2,
            scope: "g".to_owned(),
        };
        let map = |pairs: &[(&str, i64)]| -> BTreeMap<String, Value> {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), Value::from(*v)))
                .collect()
        };
        let current = map(&[("a", 1), ("b", 2)]);
        let incoming = map(&[("b", 2), ("c", 3)]);

        assert_eq!(
            resolve_write_conflict(&conflict, MergeStrategy::Refuse, &current, &incoming),
            Err(conflict.clone())
        );
        assert_eq!(
            resolve_write_conflict(&conflict, MergeStrategy::PreferNewer, &current, &incoming),
            Ok(incoming.clone())
        );
        // Disjoint keys plus one equal shared key union.
        assert_eq!(
            resolve_write_conflict(&conflict, MergeStrategy::FieldUnion, &current, &incoming),
            Ok(map(&[("a", 1), ("b", 2), ("c", 3)]))
        );
        // A key with different values on both sides is a conflict.
        let colliding = map(&[("b", 9)]);
        assert_eq!(
            resolve_write_conflict(&conflict, MergeStrategy::FieldUnion, &current, &colliding),
            Err(conflict.clone())
        );
        // Deterministic.
        assert_eq!(
            resolve_write_conflict(&conflict, MergeStrategy::FieldUnion, &current, &incoming),
            resolve_write_conflict(&conflict, MergeStrategy::FieldUnion, &current, &incoming)
        );
    }
}
