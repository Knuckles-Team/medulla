//! Entity-level three-way merge (MEDULLA-BRANCH-MERGE-R004).
//!
//! `handle_branch_merge` (R003) calls [`merge_entities`] when the merge
//! outcome is neither `up_to_date` nor `fast_forward`. The three versions of
//! each entity are read from the pinned snapshots of the base, source and
//! target heads (R006 `BranchReadAt`) and compared entity by entity:
//!
//! - unchanged on both sides stays on the target without row copy;
//! - changed on one side is adopted;
//! - changed on both sides becomes a conflict for R005's `ConflictRecord`.
//!
//! Every entity whose version differs from the base passes schema, constraint
//! and ABox validation through the authority of
//! MEDULLA-DECISION-ENGINE-R102.1 ([`SchemaRepairActivation`]); this module
//! adds no second validator. A single failure refuses the whole merge with
//! [`MergeAdoptionRefusal`] (`MERGE_ADOPTION_INVALID{entity}`) and publishes
//! nothing. File-level line merge for source files is the separate row
//! MEDULLA-BRANCH-MERGE-R010. Writes: none until R007 publishes the atomic
//! result.

use std::collections::{BTreeMap, BTreeSet};

use medulla_catalog_types::graph_schema::shadow_branch::SchemaRepairActivation;

/// One entity's identity inside one graph (MEDULLA-REPO-INGEST-R009.1).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntityIdentity {
    /// A node: `(tenant, type, key)`.
    Node {
        tenant: String,
        node_type: String,
        key: String,
    },
    /// An edge: `(source, target, relation_type)`.
    Edge {
        source: String,
        target: String,
        relation_type: String,
    },
}

impl EntityIdentity {
    /// The wire description rendered inside `MERGE_ADOPTION_INVALID{entity}`.
    pub fn describe(&self) -> String {
        match self {
            Self::Node {
                tenant,
                node_type,
                key,
            } => format!("node({tenant},{node_type},{key})"),
            Self::Edge {
                source,
                target,
                relation_type,
            } => format!("edge({source},{target},{relation_type})"),
        }
    }
}

/// One pinned-snapshot version of an entity: its ordered property values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityVersion {
    /// Ordered property values; two versions are equal iff every property is.
    pub properties: BTreeMap<String, String>,
}

/// The pinned-snapshot read of one branch head (R006 `BranchReadAt`): every
/// entity present at that head and its version. Absence from the snapshot is
/// a deletion.
pub type EntitySnapshot = BTreeMap<EntityIdentity, EntityVersion>;

/// The R102.1 activation authority recorded for each entity whose version the
/// merge claims. The merge itself adds no second validator: each recorded
/// gate is checked through
/// [`SchemaRepairActivation::checked_for_activation`], and nothing else.
pub type ValidationGates = BTreeMap<EntityIdentity, SchemaRepairActivation>;

/// The planned target state of one three-way merge; R007 publishes it
/// atomically. A merge that cannot complete never produces one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergePlan {
    /// One-sided changes adopted from the source: `Some(version)` is the row
    /// R007 copies into the target, `None` is a deletion R007 removes from it.
    pub adopted_from_source: BTreeMap<EntityIdentity, Option<EntityVersion>>,
    /// One-sided changes held on the target: already there, no row copy.
    pub held_on_target: BTreeMap<EntityIdentity, EntityVersion>,
    /// Unchanged on both sides: stays on the target without row copy.
    pub unchanged: Vec<EntityIdentity>,
    /// Changed on both sides: R005 raises a `ConflictRecord` per identity.
    pub conflicts: Vec<EntityIdentity>,
}

impl MergePlan {
    /// The target state after R007 publishes: the current target rows plus the
    /// adopted source rows (a deletion adoption removes the row). Conflicts
    /// keep the target's row until R005 resolves them.
    pub fn published_target(&self, target: &EntitySnapshot) -> EntitySnapshot {
        let mut published = target.clone();
        for (identity, version) in &self.adopted_from_source {
            match version {
                Some(version) => {
                    published.insert(identity.clone(), version.clone());
                }
                None => {
                    published.remove(identity);
                }
            }
        }
        published
    }
}

/// Refuses the whole merge: an entity the merge claims failed the R102.1
/// authority (or has no recorded gate). Nothing publishes; R007 never runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeAdoptionRefusal {
    /// The entity whose claimed version failed validation.
    pub entity: EntityIdentity,
    /// The authority's refusal detail.
    pub detail: String,
}

impl MergeAdoptionRefusal {
    /// The registered refusal code.
    pub const CODE: &'static str = "MERGE_ADOPTION_INVALID";

    /// The refusal code carried by this refusal.
    pub fn code(&self) -> &'static str {
        Self::CODE
    }

    /// The wire message: `MERGE_ADOPTION_INVALID{entity}`.
    pub fn message(&self) -> String {
        format!("{}{{{}}}", Self::CODE, self.entity.describe())
    }
}

impl std::fmt::Display for MergeAdoptionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for MergeAdoptionRefusal {}

/// The entity-level three-way merge (MEDULLA-BRANCH-MERGE-R004).
///
/// `base`, `source` and `target` are the pinned snapshots of the merge base,
/// the source head and the target head (R006 `BranchReadAt`). `gates` records
/// the R102.1 activation authority of every entity whose version the merge
/// claims.
///
/// Outcomes per entity: unchanged on both sides stays on the target without
/// row copy; changed on one side is adopted (from the source, or held on the
/// target); changed on both sides becomes a conflict for R005. Every claimed
/// entity must pass [`SchemaRepairActivation::checked_for_activation`]; one
/// failure refuses the whole merge with [`MergeAdoptionRefusal`] and
/// publishes nothing.
pub fn merge_entities(
    base: &EntitySnapshot,
    source: &EntitySnapshot,
    target: &EntitySnapshot,
    gates: &ValidationGates,
) -> Result<MergePlan, MergeAdoptionRefusal> {
    // Every identity present in any snapshot; `BTreeSet` order keeps the
    // refusal deterministic when more than one claim fails.
    let mut identities: BTreeSet<&EntityIdentity> = base.keys().collect();
    identities.extend(source.keys());
    identities.extend(target.keys());

    let mut plan = MergePlan::default();
    let mut claimed: BTreeSet<EntityIdentity> = BTreeSet::new();
    for identity in identities {
        let base_v = base.get(identity);
        let source_v = source.get(identity);
        let target_v = target.get(identity);
        match (base_v != source_v, base_v != target_v) {
            (false, false) => plan.unchanged.push(identity.clone()),
            (true, false) => {
                plan.adopted_from_source
                    .insert(identity.clone(), source_v.cloned());
                claimed.insert(identity.clone());
            }
            (false, true) => {
                plan.held_on_target
                    .insert(identity.clone(), target_v.cloned().expect("changed implies present"));
                claimed.insert(identity.clone());
            }
            (true, true) => plan.conflicts.push(identity.clone()),
        }
    }

    for identity in claimed {
        let gate = match gates.get(&identity) {
            Some(gate) => gate,
            None => {
                return Err(MergeAdoptionRefusal {
                    entity: identity,
                    detail: "no recorded R102.1 activation gate".to_string(),
                });
            }
        };
        if let Err(detail) = gate.checked_for_activation() {
            return Err(MergeAdoptionRefusal { entity: identity, detail });
        }
    }

    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_catalog_types::graph_schema::shadow_branch::SchemaRepairStep;

    fn node(node_type: &str, key: &str) -> EntityIdentity {
        EntityIdentity::Node {
            tenant: "t".into(),
            node_type: node_type.into(),
            key: key.into(),
        }
    }

    fn edge(source: &str, target: &str, relation_type: &str) -> EntityIdentity {
        EntityIdentity::Edge {
            source: source.into(),
            target: target.into(),
            relation_type: relation_type.into(),
        }
    }

    fn version(props: &[(&str, &str)]) -> EntityVersion {
        EntityVersion {
            properties: props
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        }
    }

    /// The passed R102.1 gate: every required step, in order.
    fn passed_gate(digest: &str) -> SchemaRepairActivation {
        SchemaRepairActivation {
            candidate_digest: digest.into(),
            completed: SchemaRepairActivation::REQUIRED_ORDER.to_vec(),
        }
    }

    // spec: MEDULLA-BRANCH-MERGE-R004
    #[test]
    fn entity_level_outcomes() {
        let person42 = node("person", "42");
        let person7 = node("person", "7");
        let knows = edge("42", "7", "KNOWS");

        // The base head: one node and one edge.
        let base = EntitySnapshot::from([
            (person42.clone(), version(&[("name", "Ada")])),
            (knows.clone(), version(&[("since", "1900")])),
        ]);

        // Disjoint: the source adds person 7, the target renames person 42.
        let source = EntitySnapshot::from([
            (person42.clone(), version(&[("name", "Ada")])),
            (person7.clone(), version(&[("name", "Grace")])),
            (knows.clone(), version(&[("since", "1900")])),
        ]);
        let target = EntitySnapshot::from([
            (person42.clone(), version(&[("name", "Ada Lovelace")])),
            (knows.clone(), version(&[("since", "1900")])),
        ]);
        let gates = ValidationGates::from([
            (person42.clone(), passed_gate("sha256:target-rename")),
            (person7.clone(), passed_gate("sha256:source-add")),
        ]);
        let plan = merge_entities(&base, &source, &target, &gates)
            .expect("disjoint and one-sided changes merge clean");
        assert!(plan.conflicts.is_empty(), "{plan:?}");
        assert_eq!(plan.unchanged, vec![knows.clone()]);
        assert_eq!(
            plan.adopted_from_source.get(&person7),
            Some(&Some(version(&[("name", "Grace")]))),
        );
        assert_eq!(
            plan.held_on_target.get(&person42),
            Some(&version(&[("name", "Ada Lovelace")])),
        );
        // The published target state: the target rows plus the adopted row.
        let published = plan.published_target(&target);
        assert_eq!(
            published.get(&person42),
            Some(&version(&[("name", "Ada Lovelace")])),
        );
        assert_eq!(
            published.get(&person7),
            Some(&version(&[("name", "Grace")])),
        );

        // One-sided: only the source renames person 42; the target keeps base.
        let source_side = EntitySnapshot::from([
            (person42.clone(), version(&[("name", "Ada Byron")])),
            (knows.clone(), version(&[("since", "1900")])),
        ]);
        let target_side = base.clone();
        let source_gates = ValidationGates::from([(
            person42.clone(),
            passed_gate("sha256:source-rename"),
        )]);
        let plan = merge_entities(&base, &source_side, &target_side, &source_gates)
            .expect("a one-sided change is adopted");
        assert_eq!(
            plan.adopted_from_source.get(&person42),
            Some(&Some(version(&[("name", "Ada Byron")])),),
        );
        assert!(plan.held_on_target.is_empty());
        assert!(plan.conflicts.is_empty());
        assert_eq!(
            plan.published_target(&target_side).get(&person42),
            Some(&version(&[("name", "Ada Byron")])),
        );
        // A one-sided deletion is adopted the same way: the row is removed.
        let source_delete = EntitySnapshot::from([(knows.clone(), version(&[("since", "1900")]))]);
        let delete_gates =
            ValidationGates::from([(person42.clone(), passed_gate("sha256:delete"))]);
        let plan =
            merge_entities(&base, &source_delete, &target_side, &delete_gates)
                .expect("a one-sided deletion is adopted");
        assert_eq!(plan.adopted_from_source.get(&person42), Some(&None));
        assert_eq!(plan.published_target(&target_side).len(), 1);

        // Both sides: source and target each rename person 42 differently.
        let target_rename = EntitySnapshot::from([
            (person42.clone(), version(&[("name", "Ada Lovelace")])),
            (knows.clone(), version(&[("since", "1900")])),
        ]);
        // Conflicts are not adopted, so no gate is recorded for them.
        let plan =
            merge_entities(&base, &source_side, &target_rename, &ValidationGates::new())
                .expect("a both-sides change is a conflict, not a failure");
        assert_eq!(plan.conflicts, vec![person42.clone()]);
        assert!(plan.adopted_from_source.is_empty());
        assert!(plan.held_on_target.is_empty());
        // The conflict keeps the target's row until R005 resolves it.
        assert_eq!(
            plan.published_target(&target_rename).get(&person42),
            Some(&version(&[("name", "Ada Lovelace")])),
        );

        // Invalid: the adopted entity's gate missed the shadow-branch ingest
        // step; the whole merge refuses with MERGE_ADOPTION_INVALID{entity}.
        let broken_gate = SchemaRepairActivation {
            candidate_digest: "sha256:adopted".into(),
            completed: vec![
                SchemaRepairStep::MappingDecided,
                SchemaRepairStep::AboxCheckPassed,
                SchemaRepairStep::Approved,
            ],
        };
        let broken_gates = ValidationGates::from([(person42.clone(), broken_gate)]);
        let refusal = merge_entities(&base, &source_side, &target_side, &broken_gates)
            .expect_err("an invalid adoption refuses the whole merge");
        assert_eq!(refusal.code(), MergeAdoptionRefusal::CODE);
        assert_eq!(refusal.entity, person42);
        // spec: MEDULLA-BRANCH-MERGE-R004
        assert_eq!(
            refusal.message(),
            format!("MERGE_ADOPTION_INVALID{{{}}}", person42.describe())
        );
        assert!(refusal.detail.contains("ShadowBranchIngested"));

        // A claimed entity with no recorded gate refuses the same way.
        let refusal =
            merge_entities(&base, &source_side, &target_side, &ValidationGates::new())
                .expect_err("a missing gate refuses");
        assert_eq!(refusal.entity, person42);
        assert_eq!(refusal.code(), MergeAdoptionRefusal::CODE);
    }
}
