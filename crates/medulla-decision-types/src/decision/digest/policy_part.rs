//! The digest primitives the decision policy needs: the hex digest helper, the
//! policy domain and the policy digest. They sit apart from the record digests so
//! the policy and memory modules do not depend on the record types.

use serde::Serialize;

use crate::decision::policy::DecisionPolicy;
use medulla_query_types::solve_scalar::{digest_text, DIGEST_TEXT_PREFIX};

/// Domain of a decision policy body.
pub const DECISION_POLICY_DIGEST_DOMAIN: &str = "eg/decision-policy/v1";

/// `digest_text` without the `sha256:` prefix: 64 lowercase hex characters.
pub fn digest_text_hex<T: Serialize>(domain: &str, value: &T) -> String {
    let text = digest_text(domain, value);
    text.strip_prefix(DIGEST_TEXT_PREFIX)
        .map(str::to_string)
        .unwrap_or(text)
}

/// The digest of a decision policy body.
pub fn policy_digest(policy: &DecisionPolicy) -> String {
    digest_text(DECISION_POLICY_DIGEST_DOMAIN, policy)
}
