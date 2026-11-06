//! Typed optimistic-write conflict data.
//!
//! This module holds the wire model and the pure per-node resolver
//! (`resolve_write_conflict`). The served mutation path lives in the crates
//! above `medulla-types` in the workspace DAG.

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

/// Which side of a resolved conflict the durable state reflects
/// (MEDULLA-DURABLE-KERNEL-R084.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ConflictWinner {
    /// The stored state stands (a refusal).
    Existing,
    /// The incoming write replaced the stored values (`PreferNewer`).
    Incoming,
    /// The stored and incoming fields were merged (`FieldUnion`).
    Merged,
}

impl ConflictWinner {
    /// The stable metric label value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Existing => "existing",
            Self::Incoming => "incoming",
            Self::Merged => "merged",
        }
    }

    /// The winner a strategy yields when it resolves the conflict.
    pub fn for_strategy(strategy: MergeStrategy) -> Self {
        match strategy {
            MergeStrategy::Refuse => Self::Existing,
            MergeStrategy::PreferNewer => Self::Incoming,
            MergeStrategy::FieldUnion => Self::Merged,
        }
    }
}

impl MergeStrategy {
    /// The stable metric label value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Refuse => "refuse",
            Self::PreferNewer => "prefer_newer",
            Self::FieldUnion => "field_union",
        }
    }
}

/// The record every conflict resolution returns: scope, strategy and winner
/// (MEDULLA-DURABLE-KERNEL-R084.5). A refusal carries it in the error detail;
/// a merge carries it in the commit response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WriteConflictEvidence {
    pub conflict_scope: String,
    pub strategy: MergeStrategy,
    pub winner: ConflictWinner,
}

/// Resolve one node's stale write under `strategy` (MEDULLA-DURABLE-KERNEL-R084.2).
///
/// `Refuse` returns the conflict. `PreferNewer` returns `incoming`.
/// `FieldUnion` keeps every key only one side holds and every key both sides
/// hold with equal values; a key with different values returns the conflict.
/// Pure and deterministic.
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
                match current.get(key) {
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

    fn props(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    // spec: MEDULLA-DURABLE-KERNEL-R084.2
    #[test]
    fn resolve_under_each_strategy() {
        let conflict = WriteConflict {
            expected: 3,
            actual: 4,
            scope: "graph-main".to_owned(),
        };
        let current = props(&[("a", Value::from(1)), ("shared", Value::from("x"))]);
        let disjoint = props(&[("b", Value::from(2)), ("shared", Value::from("x"))]);
        let colliding = props(&[("a", Value::from(9))]);

        assert_eq!(
            resolve_write_conflict(&conflict, MergeStrategy::Refuse, &current, &disjoint),
            Err(conflict.clone())
        );
        assert_eq!(
            resolve_write_conflict(&conflict, MergeStrategy::PreferNewer, &current, &colliding),
            Ok(colliding.clone())
        );
        assert_eq!(
            resolve_write_conflict(&conflict, MergeStrategy::FieldUnion, &current, &disjoint),
            Ok(props(&[
                ("a", Value::from(1)),
                ("b", Value::from(2)),
                ("shared", Value::from("x")),
            ]))
        );
        assert_eq!(
            resolve_write_conflict(&conflict, MergeStrategy::FieldUnion, &current, &colliding),
            Err(conflict.clone())
        );
        // Deterministic: the same inputs give the same output.
        assert_eq!(
            resolve_write_conflict(&conflict, MergeStrategy::FieldUnion, &current, &disjoint),
            resolve_write_conflict(&conflict, MergeStrategy::FieldUnion, &current, &disjoint)
        );
    }

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
}
