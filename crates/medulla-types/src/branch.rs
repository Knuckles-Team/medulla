//! Typed branch model (MEDULLA-BRANCH-MERGE-R001).
//!
//! A branch is one registry row in the branch registry owner file. It names a
//! base engine version of one graph and copies no graph data. The name rule and
//! the reserved names are decided here, before any registry write.

use serde::{Deserialize, Serialize};

use crate::contract::ServerErrorCode;

/// The reserved trunk name.
pub const RESERVED_BRANCH_NAME: &str = "main";
/// The reserved engine-owned name prefix.
pub const RESERVED_BRANCH_PREFIX: &str = "eg/";
/// Longest branch name, in bytes (`[a-z0-9][a-z0-9._/-]{0,127}`).
pub const MAX_BRANCH_NAME_BYTES: usize = 128;

/// One branch registry row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Branch {
    pub tenant: String,
    pub graph: String,
    pub name: String,
    /// The graph engine version the branch starts from.
    pub base_version: u64,
    /// The commit at the branch head; at creation, the base commit.
    pub head_commit: String,
    pub created_by: String,
    pub created_at: u64,
    pub deleted: bool,
}

/// The identity of one branch.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BranchRef {
    pub tenant: String,
    pub graph: String,
    pub name: String,
}

impl BranchRef {
    /// The registry row key: length-prefixed parts, so no part can forge another.
    pub fn registry_key(&self) -> String {
        [&self.tenant, &self.graph, &self.name]
            .iter()
            .map(|part| format!("{}:{part}", part.len()))
            .collect::<Vec<_>>()
            .join("/")
    }
}

fn is_lead_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit()
}

fn is_tail_byte(byte: u8) -> bool {
    is_lead_byte(byte) || matches!(byte, b'.' | b'_' | b'/' | b'-')
}

/// Check a branch name against the name rule, then the reserved names.
pub fn validate_branch_name(name: &str) -> Result<(), ServerErrorCode> {
    let bytes = name.as_bytes();
    let shaped = bytes.len() <= MAX_BRANCH_NAME_BYTES
        && bytes.first().is_some_and(|first| is_lead_byte(*first))
        && bytes.iter().all(|byte| is_tail_byte(*byte));
    if !shaped {
        return Err(ServerErrorCode::BranchNameInvalid);
    }
    if name == RESERVED_BRANCH_NAME || name.starts_with(RESERVED_BRANCH_PREFIX) {
        return Err(ServerErrorCode::BranchNameReserved);
    }
    Ok(())
}

/// The single commit chain appends: one branch-scoped mutation's durable record.
///
/// MEDULLA-BRANCH-MERGE-R002.1 — branch writes stay invisible to the trunk and
/// sibling branches until merge. A source changeset (MEDULLA-SCM-R002) is the
/// payload of the branch commit that carries it. The idempotency key of the
/// batch that produced this commit is scoped
/// `(tenant, graph, branch, idempotency_key)` — MEDULLA-DURABLE-KERNEL-R019.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BranchCommit {
    pub id: String,
    pub branch: String,
    /// Prior heads this commit descends from; every plain branch commit
    /// carries exactly one, the prior head (`ref` semantics).
    pub parents: Vec<String>,
    pub engine_version: u64,
    pub manifest_digest: String,
    pub actor: String,
    pub created_at_ms: u64,
}

/// Typed refusals the branch-mutation path refuses before any graph change.
///
/// Nothing on this list accounts against graph state: an unknown, deleted, or
/// foreign-tenant branch refuses resolution before the batch compiles.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum BranchMutationRefusal {
    BranchUnknown,
    BranchDeleted,
    BranchTenantDenied,
}

impl BranchCommit {
    /// A plain branch commit carries exactly ONE parent, the prior head. Merge
    /// commits gain their second parent at publication (R007), by replacing the
    /// single-parent record — never by sneaking a second chain in here.
    #[allow(clippy::too_many_arguments)]
    pub fn single_parent_commit(
        id: impl Into<String>,
        branch: impl Into<String>,
        parent: impl Into<String>,
        engine_version: u64,
        manifest_digest: impl Into<String>,
        actor: impl Into<String>,
        created_at_ms: u64,
    ) -> Self {
        BranchCommit {
            id: id.into(),
            branch: branch.into(),
            parents: vec![parent.into()],
            engine_version,
            manifest_digest: manifest_digest.into(),
            actor: actor.into(),
            created_at_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-BRANCH-MERGE-R001
    #[test]
    fn branch_name_rule_cases() {
        let long = "a".repeat(MAX_BRANCH_NAME_BYTES + 1);
        let cases: &[(&str, Result<(), ServerErrorCode>)] = &[
            ("feature/x-1.2_b", Ok(())),
            ("0abc", Ok(())),
            ("", Err(ServerErrorCode::BranchNameInvalid)),
            ("-lead", Err(ServerErrorCode::BranchNameInvalid)),
            ("Upper", Err(ServerErrorCode::BranchNameInvalid)),
            ("sp ace", Err(ServerErrorCode::BranchNameInvalid)),
            (long.as_str(), Err(ServerErrorCode::BranchNameInvalid)),
            ("main", Err(ServerErrorCode::BranchNameReserved)),
            ("eg/internal", Err(ServerErrorCode::BranchNameReserved)),
            ("mainline", Ok(())),
        ];
        for (name, expected) in cases {
            assert_eq!(&validate_branch_name(name), expected, "{name}");
        }
    }

    #[test]
    fn registry_key_parts_cannot_collide() {
        let a = BranchRef {
            tenant: "t/1".into(),
            graph: "g".into(),
            name: "b".into(),
        };
        let b = BranchRef {
            tenant: "t".into(),
            graph: "1/g".into(),
            name: "b".into(),
        };
        assert_ne!(a.registry_key(), b.registry_key());
    }

    // spec: MEDULLA-BRANCH-MERGE-R002.1
    #[test]
    fn branch_commit_appends_one_parent_and_round_trips() {
        let prior_head = "c0ffee0000000000000000000000000000000000".to_owned();
        let commit = BranchCommit::single_parent_commit(
            "aabbcc0000000000000000000000000000000000",
            "feature/eg-style-fixes",
            prior_head.clone(),
            42,
            "sha256:manifest",
            "agent:medulla-expert",
            1_762_279_040_000,
        );
        assert_eq!(commit.parents, vec![prior_head.clone()]);
        assert_eq!(commit.branch, "feature/eg-style-fixes");
        assert_eq!(commit.engine_version, 42);

        let encoded = serde_json::to_string(&commit).expect("commit encodes");
        let decoded: BranchCommit = serde_json::from_str(&encoded).expect("commit decodes");
        assert_eq!(decoded, commit);
    }

    // spec: MEDULLA-BRANCH-MERGE-R002.1
    #[test]
    fn refusal_set_and_default_branch_are_closed() {
        for refusal in [
            BranchMutationRefusal::BranchUnknown,
            BranchMutationRefusal::BranchDeleted,
            BranchMutationRefusal::BranchTenantDenied,
        ] {
            let encoded = serde_json::to_string(&refusal).expect("refusal encodes");
            assert_eq!(
                serde_json::from_str::<BranchMutationRefusal>(&encoded).expect("refusal decodes"),
                refusal
            );
        }
        assert_eq!(RESERVED_BRANCH_NAME, "main");
    }
}
