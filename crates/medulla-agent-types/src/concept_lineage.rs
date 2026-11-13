//! Wire contract of the served `ConceptLineage` method
//! (MEDULLA-DURABLE-KERNEL-R094.7 and R094.7.1).
//!
//! One method serves three operations over the tenant's known concept ids and
//! their `superseded_by` edges: `Claim` records a concept id for a repository,
//! `AddEdge` records that one known concept is superseded by another, and
//! `Read` answers one concept with its direct successors. The ids are carried
//! as text and parsed by the handler into `medulla_types::concept_id::ConceptId`,
//! so a malformed id refuses with its registered code instead of failing the
//! wire decode. The store that persists both tables is the Agent Library owner.

use serde::{Deserialize, Serialize};

use crate::agent_library::{validate_key, AgentLibraryMutationContext};

/// Longest repository name a claim or edge may carry.
pub const MAX_CONCEPT_REPOSITORY_BYTES: usize = 128;

/// One concept id with the repository that claimed it and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ConceptRecord {
    pub concept_id: String,
    pub repository: String,
    pub admitted_at_ms: u64,
}

/// One `superseded_by` edge with its evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ConceptEdgeRecord {
    pub superseded: String,
    pub successor: String,
    pub repository: String,
    pub admitted_at_ms: u64,
}

/// What `Read` answers: the concept (absent when it was never claimed) and the
/// concepts that directly supersede it, in id order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ConceptReadResult {
    pub concept: Option<ConceptRecord>,
    pub successors: Vec<ConceptEdgeRecord>,
    /// True when the concept has more direct successors than one read returns.
    pub truncated: bool,
}

/// Typed concept lineage operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ConceptLineageOp {
    /// Record `concept_id` as owned by `repository`.
    Claim {
        context: AgentLibraryMutationContext,
        concept_id: String,
        repository: String,
    },
    /// Record that `superseded` is superseded by `successor`.
    AddEdge {
        context: AgentLibraryMutationContext,
        superseded: String,
        successor: String,
        repository: String,
    },
    /// Read one concept and its direct successors.
    Read {
        tenant_id: String,
        concept_id: String,
    },
}

impl ConceptLineageOp {
    /// Whether this operation commits durable rows.
    pub fn is_mutation(&self) -> bool {
        matches!(self, Self::Claim { .. } | Self::AddEdge { .. })
    }

    /// The tenant the operation names.
    pub fn tenant_id(&self) -> &str {
        match self {
            Self::Claim { context, .. } | Self::AddEdge { context, .. } => &context.tenant_id,
            Self::Read { tenant_id, .. } => tenant_id,
        }
    }

    /// The scope action the caller needs: the concept store is part of the
    /// component catalog, so it takes the component read and write grants.
    pub fn authz_action(&self) -> &'static str {
        if self.is_mutation() {
            "agent:component-write"
        } else {
            "agent:component-read"
        }
    }

    /// Shape checks that need no store. Concept id grammar is checked by the
    /// handler, which owns the registered refusal code for it.
    pub fn validate(&self) -> Result<(), String> {
        validate_key(self.tenant_id(), "tenant_id")?;
        match self {
            Self::Claim {
                context,
                repository,
                ..
            }
            | Self::AddEdge {
                context,
                repository,
                ..
            } => {
                context.validate()?;
                validate_repository(repository)
            }
            Self::Read { .. } => Ok(()),
        }
    }
}

fn validate_repository(repository: &str) -> Result<(), String> {
    if repository.is_empty() || repository.len() > MAX_CONCEPT_REPOSITORY_BYTES {
        return Err(format!(
            "repository must be 1 to {MAX_CONCEPT_REPOSITORY_BYTES} bytes"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DURABLE-KERNEL-R094.7.1
    #[test]
    fn read_is_not_a_mutation_and_writes_take_the_component_write_grant() {
        let read = ConceptLineageOp::Read {
            tenant_id: "t1".into(),
            concept_id: "MEDULLA-KG.memory".into(),
        };
        assert!(!read.is_mutation());
        assert_eq!(read.authz_action(), "agent:component-read");
        assert_eq!(read.tenant_id(), "t1");
        read.validate().unwrap();
        let wire = serde_json::to_value(&read).unwrap();
        assert_eq!(wire["op"], "read");
        assert_eq!(
            serde_json::from_value::<ConceptLineageOp>(wire).unwrap(),
            read
        );
    }

    // spec: MEDULLA-DURABLE-KERNEL-R094.7.1
    #[test]
    fn repository_names_are_bounded() {
        assert!(validate_repository("medulla").is_ok());
        assert!(validate_repository("").is_err());
        assert!(validate_repository(&"r".repeat(MAX_CONCEPT_REPOSITORY_BYTES + 1)).is_err());
    }
}
