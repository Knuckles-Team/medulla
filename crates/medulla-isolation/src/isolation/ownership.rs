//! Graph ownership planner, transactional apply and exact rollback
//! (MEDULLA-IDENTITY-R700.2).
//!
//! [`plan`] turns one graph's [`GraphOwnershipSignals`] into a deterministic
//! grant plan in O(w): the creator owns the graph when present; otherwise the
//! writer with the most `writes_30d` owns it (ties broken by principal id
//! order). Every other writer whose share of all writes reaches the
//! `identity.ownership_writer_share` knob gets `Writer`; the rest `Reader`.
//! [`apply`] writes the whole plan of one graph in one store transaction and
//! returns the [`RollbackEntry`]s; [`rollback`] restores exactly those prior
//! role sets in one transaction.

use std::collections::{BTreeMap, BTreeSet};

use medulla_identity_types::identity::{
    GrantPlanEntry, GraphOwnershipSignals, OwnershipDisposition, RollbackEntry,
    OWNERSHIP_SCHEMA_VERSION,
};

/// Default of the `identity.ownership_writer_share` knob (fraction).
pub const DEFAULT_OWNERSHIP_WRITER_SHARE: f64 = 0.10;
/// Inclusive bounds of the `identity.ownership_writer_share` knob.
pub const OWNERSHIP_WRITER_SHARE_RANGE: (f64, f64) = (0.01, 0.5);

/// The validated `identity.ownership_writer_share` knob.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OwnershipWriterShare(f64);

impl OwnershipWriterShare {
    /// Accept a fraction inside [`OWNERSHIP_WRITER_SHARE_RANGE`]; anything else
    /// refuses startup.
    pub fn new(value: f64) -> Result<Self, String> {
        let (low, high) = OWNERSHIP_WRITER_SHARE_RANGE;
        if value.is_finite() && (low..=high).contains(&value) {
            Ok(Self(value))
        } else {
            Err(format!(
                "identity.ownership_writer_share must be within [{low}, {high}], got {value}"
            ))
        }
    }

    /// The fraction.
    pub fn get(self) -> f64 {
        self.0
    }
}

impl Default for OwnershipWriterShare {
    fn default() -> Self {
        Self(DEFAULT_OWNERSHIP_WRITER_SHARE)
    }
}

/// The graph-scoped role a disposition grants.
pub fn ownership_role(graph: &str, disposition: OwnershipDisposition) -> String {
    let suffix = match disposition {
        OwnershipDisposition::Owner => "owner",
        OwnershipDisposition::Writer => "writer",
        OwnershipDisposition::Reader => "reader",
    };
    format!("graph:{graph}:{suffix}")
}

/// The deterministic grant plan of one graph, sorted by principal.
pub fn plan(signals: &GraphOwnershipSignals, share: OwnershipWriterShare) -> Vec<GrantPlanEntry> {
    let mut writes: BTreeMap<&str, u64> = BTreeMap::new();
    for (principal, count) in &signals.writers {
        let slot = writes.entry(principal.as_str()).or_default();
        *slot = slot.saturating_add(*count);
    }
    let total: u64 = writes.values().fold(0u64, |acc, n| acc.saturating_add(*n));
    let owner = signals.creator.as_deref().or_else(|| {
        // BTreeMap iterates in principal order, so the first maximum wins ties.
        writes
            .iter()
            .fold(None::<(&str, u64)>, |best, (p, n)| match best {
                Some((_, b)) if b >= *n => best,
                _ => Some((p, *n)),
            })
            .map(|(p, _)| p)
    });
    let mut out: BTreeMap<&str, OwnershipDisposition> = BTreeMap::new();
    for (principal, count) in &writes {
        let writer = total > 0 && (*count as f64) >= share.get() * (total as f64);
        let disposition = if writer {
            OwnershipDisposition::Writer
        } else {
            OwnershipDisposition::Reader
        };
        out.insert(principal, disposition);
    }
    if let Some(owner) = owner {
        out.insert(owner, OwnershipDisposition::Owner);
    }
    out.into_iter()
        .map(|(principal, disposition)| GrantPlanEntry {
            schema_version: OWNERSHIP_SCHEMA_VERSION,
            principal: principal.to_string(),
            disposition,
            full_role_set: BTreeSet::from([ownership_role(&signals.graph, disposition)]),
        })
        .collect()
}

/// The role store the planner writes through. `replace_role_sets` is one
/// transaction: every set lands or none does.
pub trait OwnershipRoleStore {
    /// The store's failure.
    type Error;
    /// A principal's current role set (empty when it has none).
    fn role_set(&self, principal: &str) -> BTreeSet<String>;
    /// Replace each named principal's complete role set in one transaction.
    fn replace_role_sets(
        &mut self,
        graph: &str,
        sets: &[(String, BTreeSet<String>)],
    ) -> Result<(), Self::Error>;
}

/// Apply one graph's plan in one transaction; returns the exact prior role
/// sets for [`rollback`]. A failed transaction changes nothing.
pub fn apply<S: OwnershipRoleStore>(
    store: &mut S,
    graph: &str,
    plan: &[GrantPlanEntry],
) -> Result<Vec<RollbackEntry>, S::Error> {
    let rollback: Vec<RollbackEntry> = plan
        .iter()
        .map(|entry| RollbackEntry {
            schema_version: OWNERSHIP_SCHEMA_VERSION,
            principal: entry.principal.clone(),
            prior_role_set: store.role_set(&entry.principal),
        })
        .collect();
    let sets: Vec<(String, BTreeSet<String>)> = plan
        .iter()
        .map(|entry| (entry.principal.clone(), entry.full_role_set.clone()))
        .collect();
    store.replace_role_sets(graph, &sets)?;
    Ok(rollback)
}

/// Restore exactly the prior role sets in one transaction.
pub fn rollback<S: OwnershipRoleStore>(
    store: &mut S,
    graph: &str,
    entries: &[RollbackEntry],
) -> Result<(), S::Error> {
    let sets: Vec<(String, BTreeSet<String>)> = entries
        .iter()
        .map(|entry| (entry.principal.clone(), entry.prior_role_set.clone()))
        .collect();
    store.replace_role_sets(graph, &sets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MemStore {
        roles: BTreeMap<String, BTreeSet<String>>,
        transactions: usize,
    }

    impl OwnershipRoleStore for MemStore {
        type Error = std::convert::Infallible;
        fn role_set(&self, principal: &str) -> BTreeSet<String> {
            self.roles.get(principal).cloned().unwrap_or_default()
        }
        fn replace_role_sets(
            &mut self,
            _graph: &str,
            sets: &[(String, BTreeSet<String>)],
        ) -> Result<(), Self::Error> {
            self.transactions += 1;
            for (principal, set) in sets {
                if set.is_empty() {
                    self.roles.remove(principal);
                } else {
                    self.roles.insert(principal.clone(), set.clone());
                }
            }
            Ok(())
        }
    }

    fn signals(n: usize, creator: Option<&str>) -> GraphOwnershipSignals {
        GraphOwnershipSignals {
            schema_version: OWNERSHIP_SCHEMA_VERSION,
            graph: "g".into(),
            creator: creator.map(str::to_string),
            // Reverse order so the plan must sort; principal 0 writes most.
            writers: (0..n)
                .rev()
                .map(|i| (format!("p{i:06}"), (n - i) as u64 * 10))
                .collect(),
            name_hint: None,
        }
    }

    // spec: MEDULLA-IDENTITY-R700.2
    #[test]
    fn plan_is_deterministic_and_rollback_exact() {
        let share = OwnershipWriterShare::default();
        for n in [10usize, 1_000, 100_000] {
            let input = signals(n, None);
            let first = plan(&input, share);
            let mut reversed = input.clone();
            reversed.writers.reverse();
            assert_eq!(first, plan(&reversed, share), "order-independent at {n}");
            assert_eq!(first.len(), n);
            assert_eq!(first[0].principal, "p000000");
            assert_eq!(first[0].disposition, OwnershipDisposition::Owner);
            let total: u64 = input.writers.iter().map(|(_, w)| *w).sum();
            for entry in &first[1..] {
                let w = input
                    .writers
                    .iter()
                    .find(|(p, _)| *p == entry.principal)
                    .unwrap()
                    .1;
                let expect = if (w as f64) >= 0.10 * total as f64 {
                    OwnershipDisposition::Writer
                } else {
                    OwnershipDisposition::Reader
                };
                assert_eq!(entry.disposition, expect);
            }

            let mut store = MemStore::default();
            store
                .roles
                .insert("p000001".into(), BTreeSet::from(["legacy".to_string()]));
            let before = store.roles.clone();
            let undo = apply(&mut store, "g", &first).unwrap();
            assert_eq!(store.transactions, 1, "one transaction per graph");
            assert_eq!(store.role_set("p000000"), first[0].full_role_set);
            rollback(&mut store, "g", &undo).unwrap();
            assert_eq!(store.roles, before, "rollback restores exactly at {n}");
        }

        // The creator owns even without writes; ties go to the lower id.
        let with_creator = plan(&signals(10, Some("zed")), share);
        let owner = with_creator.iter().find(|e| e.principal == "zed").unwrap();
        assert_eq!(owner.disposition, OwnershipDisposition::Owner);
        let tie = GraphOwnershipSignals {
            writers: vec![("b".into(), 5), ("a".into(), 5)],
            ..signals(0, None)
        };
        assert_eq!(
            plan(&tie, share)[0].disposition,
            OwnershipDisposition::Owner
        );
        assert_eq!(plan(&tie, share)[0].principal, "a");

        assert!(OwnershipWriterShare::new(0.005).is_err());
        assert!(OwnershipWriterShare::new(0.6).is_err());
        assert!(OwnershipWriterShare::new(0.5).is_ok());
    }
}
