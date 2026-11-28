//! The stored ownership apply records (MEDULLA-IDENTITY-R700.5).
//!
//! The role sets themselves are the identities' roles in the isolation layer; this map keeps
//! what rollback needs: the exact prior role sets, the plan and signals digests, the actor and
//! the time of the apply. It is part of the persisted identity image, so a restart reads it.

use super::{IdentityRefusal, IdentityStore};
use crate::identity::{OwnershipRecord, MAX_OWNERSHIP_RECORDS};

fn record_key(graph: &str, plan_digest: &str) -> String {
    format!("{graph}\0{plan_digest}")
}

impl IdentityStore {
    /// Store one apply record. A new key is refused with `Full` when
    /// [`MAX_OWNERSHIP_RECORDS`] are held; an existing key is replaced.
    pub fn record_ownership(&mut self, record: OwnershipRecord) -> Result<(), IdentityRefusal> {
        let key = record_key(&record.graph, &record.plan_digest);
        if !self.ownership_records.contains_key(&key)
            && self.ownership_records.len() >= MAX_OWNERSHIP_RECORDS
        {
            return Err(IdentityRefusal::Full);
        }
        self.ownership_records.insert(key, record);
        Ok(())
    }

    /// The record of one applied plan.
    pub fn ownership_record(&self, graph: &str, plan_digest: &str) -> Option<&OwnershipRecord> {
        self.ownership_records.get(&record_key(graph, plan_digest))
    }

    /// The most recently applied record of `graph` (newest `applied_at_ms`, ties by plan digest).
    pub fn latest_ownership_record(&self, graph: &str) -> Option<&OwnershipRecord> {
        self.ownership_records
            .values()
            .filter(|record| record.graph == graph)
            .max_by(|left, right| {
                left.applied_at_ms
                    .cmp(&right.applied_at_ms)
                    .then_with(|| left.plan_digest.cmp(&right.plan_digest))
            })
    }

    /// Mark one record rolled back; false when it is absent.
    pub fn mark_ownership_rolled_back(&mut self, graph: &str, plan_digest: &str) -> bool {
        match self
            .ownership_records
            .get_mut(&record_key(graph, plan_digest))
        {
            Some(record) => {
                record.rolled_back = true;
                true
            }
            None => false,
        }
    }

    /// How many records are held.
    pub fn ownership_record_count(&self) -> usize {
        self.ownership_records.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{RollbackEntry, OWNERSHIP_SCHEMA_VERSION};

    fn record(graph: &str, digest: &str, at: u64) -> OwnershipRecord {
        OwnershipRecord {
            schema_version: OWNERSHIP_SCHEMA_VERSION,
            graph: graph.to_string(),
            actor: "usr:admin".to_string(),
            plan_digest: digest.to_string(),
            signals_digest: "s".repeat(64),
            applied_at_ms: at,
            rollback: vec![RollbackEntry {
                schema_version: OWNERSHIP_SCHEMA_VERSION,
                principal: "usr:a".to_string(),
                prior_role_set: std::collections::BTreeSet::from(["extra".to_string()]),
            }],
            rolled_back: false,
        }
    }

    // spec: MEDULLA-IDENTITY-R700.5
    #[test]
    fn record_round_trips_through_the_image() {
        let mut store = IdentityStore::default();
        store.record_ownership(record("g", "d1", 10)).unwrap();
        let image = serde_json::to_vec(&store).unwrap();
        let back: IdentityStore = serde_json::from_slice(&image).unwrap();
        assert_eq!(back, store);
        assert_eq!(serde_json::to_vec(&back).unwrap(), image);
        assert_eq!(
            back.ownership_record("g", "d1"),
            Some(&record("g", "d1", 10))
        );
        // An empty map leaves the image without the field.
        assert!(
            !String::from_utf8(serde_json::to_vec(&IdentityStore::default()).unwrap())
                .unwrap()
                .contains("ownership_records")
        );
    }

    // spec: MEDULLA-IDENTITY-R700.5
    #[test]
    fn records_are_bounded_marked_and_ordered() {
        let mut store = IdentityStore::default();
        for index in 0..MAX_OWNERSHIP_RECORDS {
            store
                .record_ownership(record("g", &format!("d{index}"), index as u64))
                .unwrap();
        }
        assert_eq!(
            store.record_ownership(record("g", "new", 1)),
            Err(IdentityRefusal::Full)
        );
        // An existing key is replaced, not refused.
        store.record_ownership(record("g", "d0", 5000)).unwrap();
        assert_eq!(
            store.latest_ownership_record("g").unwrap().plan_digest,
            "d0"
        );
        assert!(store.mark_ownership_rolled_back("g", "d0"));
        assert!(store.ownership_record("g", "d0").unwrap().rolled_back);
        assert!(!store.mark_ownership_rolled_back("g", "absent"));
    }
}
