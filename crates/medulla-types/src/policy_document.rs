//! The policy source binding and the review that approved it (MEDULLA-ORCH-VOCAB-R012.8).
//!
//! Active runtime policy lives in the graph; its normative source is a reviewed, versioned
//! repository file. [`PolicyBinding`] names that source revision, the candidate set read
//! from it (the digest `parse_policy_document` computes, R012.7.1), the review receipt that
//! approved the set, the invariant generation it was checked against and the revision of the
//! active policy it was reviewed to replace. Medulla has no writer for normative files: a
//! source change goes through the governed write-back, and a graph edit never rewrites a
//! file. [`PolicyReview`] is the receipt arbora's approval row returns: the binding and the
//! two principals of the review.

use serde::{Deserialize, Serialize};

use crate::refusal_code::RefusalCode;

/// Review receipt missing: no recorded review has the receipt digest the activation names.
pub const POLICY_REVIEW_MISSING: &str = "POLICY_REVIEW_MISSING";
/// The review is for other digests than the activation names, or is recorded with other content.
pub const POLICY_REVIEW_MISMATCH: &str = "POLICY_REVIEW_MISMATCH";
/// The reviewed source or the invariant generation changed after the review.
pub const POLICY_REVIEW_STALE: &str = "POLICY_REVIEW_STALE";
/// A reviewed rule contradicts an invariant of the pinned generation.
pub const POLICY_CONTRADICTS_INVARIANT: &str = "POLICY_CONTRADICTS_INVARIANT";
/// The reviewed set holds a clause that did not parse and so cannot be activated.
pub const POLICY_CLAUSE_UNSUPPORTED: &str = "POLICY_CLAUSE_UNSUPPORTED";

/// The refusal codes of policy activation. The decision plane collects this table with every
/// other family table and refuses a code declared twice.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new(POLICY_REVIEW_MISSING),
    RefusalCode::new(POLICY_REVIEW_MISMATCH),
    RefusalCode::new(POLICY_REVIEW_STALE),
    RefusalCode::new(POLICY_CONTRADICTS_INVARIANT),
    RefusalCode::new(POLICY_CLAUSE_UNSUPPORTED),
];

/// The prefix of the governed-set scope of one repository's active policy:
/// `policy:<repository>`.
pub const POLICY_SCOPE_PREFIX: &str = "policy:";

/// The governed-set scope of the active policy of `repository`.
pub fn policy_scope_ref(repository: &str) -> String {
    format!("{POLICY_SCOPE_PREFIX}{repository}")
}

/// The revision a candidate set becomes when it is activated: the revision ref the champion
/// pointer of `policy:<repository>` names.
pub fn policy_set_target(candidate_set_digest: &str) -> String {
    format!("policyset:{candidate_set_digest}")
}

/// The candidate-set digest a revision ref of [`policy_set_target`] names.
pub fn candidate_set_digest_of(target: &str) -> Option<&str> {
    target.strip_prefix("policyset:")
}

/// The reviewed source and everything an activation must still match at the move.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PolicyBinding {
    /// The repository that holds the normative file.
    pub repository: String,
    /// The path of the normative file inside the repository.
    pub path: String,
    /// The commit the file was reviewed at.
    pub commit: String,
    /// Lowercase hex SHA-256 of the file bytes at that commit.
    pub blob_digest: String,
    /// The candidate-set digest of R012.7.1 (lowercase hex) the review approved.
    pub candidate_set_digest: String,
    /// `sha256:<hex>`: the digest of the review receipt of the arbora approval row.
    pub review_receipt_digest: String,
    /// `sha256:<hex>`: the invariant generation the reviewed rules were checked against.
    pub invariant_generation: String,
    /// The revision the active policy of `repository` had when the review was made; `None`
    /// when no policy was active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_policy_revision: Option<String>,
}

/// The two principals of a review and the binding they approved. The author proposed the
/// set; the reviewer is a different principal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PolicyReview {
    /// What was reviewed.
    pub binding: PolicyBinding,
    /// The principal that authored the reviewed set.
    pub author: String,
    /// The principal that approved it.
    pub reviewer: String,
}

/// What a policy activation does to the active set of a repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum PolicyChange {
    /// Activate the first set: the review replaces nothing.
    Create,
    /// Replace the active set: the review names the revision it replaces.
    Update,
    /// Withdraw the active set: the pointer returns to the revision before it.
    Retire,
}

/// One clause of an activated set: the rule it became and the typed node it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PolicyEntityRef {
    /// The rule identity of the clause.
    pub rule_id: String,
    /// The digest of the clause text.
    pub clause_digest: String,
    /// The graph node of the candidate the clause was read into.
    pub entity_id: String,
}

/// What one policy activation changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PolicyActivated {
    /// The governed-set scope that moved.
    pub scope_ref: String,
    /// What was done.
    pub change: PolicyChange,
    /// The revision active after the change; `None` when none is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// The candidate-set digest the review approved.
    pub candidate_set_digest: String,
    /// One reference per clause of the reviewed set.
    pub entities: Vec<PolicyEntityRef>,
}

fn is_hex64(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_sha256_form(text: &str) -> bool {
    text.strip_prefix("sha256:").is_some_and(is_hex64)
}

impl PolicyBinding {
    /// The governed-set scope this binding activates under.
    pub fn scope_ref(&self) -> String {
        policy_scope_ref(&self.repository)
    }

    /// The revision ref this binding activates.
    pub fn target(&self) -> String {
        policy_set_target(&self.candidate_set_digest)
    }

    /// Whether every field is present and every digest has its one form.
    pub fn check(&self) -> Result<(), String> {
        for (name, value) in [
            ("repository", &self.repository),
            ("path", &self.path),
            ("commit", &self.commit),
        ] {
            if value.trim().is_empty() {
                return Err(format!("the policy binding names no {name}"));
            }
        }
        if !is_hex64(&self.blob_digest) {
            return Err("blob_digest is not 64 lowercase hex characters".to_string());
        }
        if !is_hex64(&self.candidate_set_digest) {
            return Err("candidate_set_digest is not 64 lowercase hex characters".to_string());
        }
        if !is_sha256_form(&self.review_receipt_digest) {
            return Err("review_receipt_digest is not `sha256:` and 64 lowercase hex".to_string());
        }
        if !is_sha256_form(&self.invariant_generation) {
            return Err("invariant_generation is not `sha256:` and 64 lowercase hex".to_string());
        }
        if self
            .expected_policy_revision
            .as_deref()
            .is_some_and(|revision| revision.trim().is_empty())
        {
            return Err("expected_policy_revision is empty".to_string());
        }
        Ok(())
    }
}

impl PolicyReview {
    /// The binding checks, plus two non-empty principals.
    pub fn check(&self) -> Result<(), String> {
        self.binding.check()?;
        if self.author.trim().is_empty() || self.reviewer.trim().is_empty() {
            return Err("a review names its author and its reviewer".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> PolicyBinding {
        PolicyBinding {
            repository: "org/repo".to_string(),
            path: ".specify/memory/constitution.md".to_string(),
            commit: "c0ffee".to_string(),
            blob_digest: "ab".repeat(32),
            candidate_set_digest: "cd".repeat(32),
            review_receipt_digest: format!("sha256:{}", "ef".repeat(32)),
            invariant_generation: format!("sha256:{}", "01".repeat(32)),
            expected_policy_revision: None,
        }
    }

    // spec: MEDULLA-ORCH-VOCAB-R012.8
    #[test]
    fn binding_round_trips_and_names_its_scope_and_target() {
        let binding = binding();
        binding.check().unwrap();
        let wire = serde_json::to_string(&binding).unwrap();
        assert!(!wire.contains("expected_policy_revision"));
        let back: PolicyBinding = serde_json::from_str(&wire).unwrap();
        assert_eq!(back, binding);
        assert_eq!(binding.scope_ref(), "policy:org/repo");
        assert_eq!(
            candidate_set_digest_of(&binding.target()),
            Some(binding.candidate_set_digest.as_str())
        );
    }

    // spec: MEDULLA-ORCH-VOCAB-R012.8
    #[test]
    fn malformed_digests_and_principals_are_refused() {
        let mut bad = binding();
        bad.review_receipt_digest = "ef".repeat(32);
        assert!(bad.check().is_err());
        let mut bad = binding();
        bad.blob_digest = "AB".repeat(32);
        assert!(bad.check().is_err());
        let review = PolicyReview {
            binding: binding(),
            author: "alice".to_string(),
            reviewer: " ".to_string(),
        };
        assert!(review.check().is_err());
        let mut codes: Vec<&str> = REFUSALS.iter().map(|code| code.as_str()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), REFUSALS.len());
    }
}
