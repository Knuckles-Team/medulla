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

/// The separator of a branch-scoped graph reference (`<graph>@branch/<name>`).
pub const BRANCH_REFERENCE_SEPARATOR: &str = "@branch/";

/// A signed graph reference, parsed once at admission
/// (MEDULLA-BRANCH-MERGE-R002, D111 v2): the bare graph and, when the
/// reference names one, the branch. The bare graph is `main`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphTarget {
    pub graph: String,
    pub branch: Option<String>,
}

/// Split `<graph>@branch/<name>` into its graph and branch. A reference with
/// no separator names `main`; a named branch must pass the name rule, so a
/// reference cannot address `main` or an engine-owned name as a branch.
pub fn parse_graph_reference(reference: &str) -> Result<GraphTarget, ServerErrorCode> {
    let Some((graph, name)) = reference.split_once(BRANCH_REFERENCE_SEPARATOR) else {
        return Ok(GraphTarget {
            graph: reference.to_string(),
            branch: None,
        });
    };
    if graph.is_empty() {
        return Err(ServerErrorCode::BranchNameInvalid);
    }
    validate_branch_name(name)?;
    Ok(GraphTarget {
        graph: graph.to_string(),
        branch: Some(name.to_string()),
    })
}

/// One commit on a branch's single commit chain (MEDULLA-BRANCH-MERGE-R002).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BranchCommit {
    pub id: String,
    pub branch: String,
    /// The prior head; the first commit's parent is the branch base commit.
    pub parents: Vec<String>,
    /// The branch-local engine version this commit produces (base + sequence).
    pub engine_version: u64,
    /// SHA-256 of the commit's write set.
    pub manifest_digest: String,
    pub actor: String,
    pub created_at: u64,
}

/// A branch commit as stored in the append-only log: the commit, the
/// idempotency key it answers, its write set and the result it returned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredBranchCommit {
    pub commit: BranchCommit,
    pub idempotency_key: String,
    pub write_set: Vec<u8>,
    pub result: Vec<u8>,
}

/// The receipt of a branch-scoped write.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BranchWriteReceipt {
    pub commit: BranchCommit,
    /// The branch head after this write: the commit id.
    pub branch_head: String,
    /// True when an earlier write under the same key answered.
    pub replayed: bool,
}

/// The branch idempotency key `(tenant, graph, branch, idempotency_key)`,
/// length-prefixed like [`BranchRef::registry_key`] so no part forges another.
pub fn branch_idempotency_key(branch: &BranchRef, idempotency_key: &str) -> String {
    format!(
        "{}/{}:{idempotency_key}",
        branch.registry_key(),
        idempotency_key.len()
    )
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

    // spec: MEDULLA-BRANCH-MERGE-R002
    #[test]
    fn graph_reference_names_main_or_one_valid_branch() {
        let cases: &[(&str, Result<(&str, Option<&str>), ServerErrorCode>)] = &[
            ("g", Ok(("g", None))),
            ("g@branch/feature/x", Ok(("g", Some("feature/x")))),
            ("g@branch/main", Err(ServerErrorCode::BranchNameReserved)),
            ("g@branch/Bad", Err(ServerErrorCode::BranchNameInvalid)),
            ("@branch/x", Err(ServerErrorCode::BranchNameInvalid)),
        ];
        for (reference, expected) in cases {
            let parsed = parse_graph_reference(reference);
            let parsed = parsed
                .as_ref()
                .map(|target| (target.graph.as_str(), target.branch.as_deref()))
                .map_err(|code| *code);
            assert_eq!(&parsed, expected, "{reference}");
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
}
