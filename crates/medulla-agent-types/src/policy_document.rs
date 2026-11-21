//! Policy rule candidates read from a policy document, with source spans
//! (MEDULLA-ORCH-VOCAB-R012.7.1).
//!
//! The parser turns the bytes of one governance document into [`PolicyCandidate`]
//! records. A candidate is `Proposed` when its clause parsed cleanly, and `Inactive`
//! with a typed reason when it did not; no candidate is ever a stored
//! `GuardrailRule`, and prose never becomes a rule. The grammar is revision
//! [`POLICY_GRAMMAR_REVISION`]. Two profiles read the same clause into the same
//! [`PolicyRule`]: a `Constitution` Markdown list, and the bundled `RuleSet`
//! mini-markdown. `crates/medulla-shacl/src/policy.rs` stays the ICV write guard.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The revision of the clause grammar this module defines.
pub const POLICY_GRAMMAR_REVISION: u32 = 1;

/// How strongly a rule binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum PolicySeverity {
    /// Informational (`MAY`).
    Info,
    /// A warning (`SHOULD`, `SHOULD NOT`).
    Warn,
    /// A blocking rule (`MUST`, `MUST NOT`).
    Block,
}

/// Where a clause stands in its document. Lines are 1-based and inclusive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PolicySourceRef {
    /// The repository that holds the document.
    pub repository: String,
    /// The document path inside the repository.
    pub path: String,
    /// The commit the document was read at.
    pub commit: String,
    /// The digest of the document bytes.
    pub blob_digest: String,
    /// The first line of the clause.
    pub start_line: u32,
    /// The last line of the clause.
    pub end_line: u32,
}

/// One rule read from one clause.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PolicyRule {
    /// The rule identity: `policy-` plus 16 digest digits, or the `RuleSet` id.
    pub rule_id: String,
    /// The absolute IRI of the class the rule governs.
    pub scope: String,
    /// How strongly the rule binds.
    pub severity: PolicySeverity,
    /// The clause the rule came from.
    pub source_ref: PolicySourceRef,
    /// Lowercase hex SHA-256 of the normalized clause text.
    pub clause_digest: String,
}

/// Why a clause did not become a rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum PolicyInactiveReason {
    /// The clause has no usable scope or does not match the grammar.
    ClauseUnparsed,
    /// The clause states more than one scope or mixes strong and weak words.
    ClauseAmbiguous,
}

/// Whether a candidate may be proposed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum PolicyCandidateStatus {
    /// The clause parsed cleanly.
    Proposed,
    /// The clause did not parse; nothing is proposed from it.
    Inactive {
        /// Why it did not.
        reason: PolicyInactiveReason,
    },
}

/// One clause of a policy document and what became of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PolicyCandidate {
    /// The rule; `None` exactly when the status is `Inactive`.
    pub rule: Option<PolicyRule>,
    /// The clause span.
    pub source_ref: PolicySourceRef,
    /// The digest of the keyword-stripped clause text. A proposed candidate's rule carries
    /// the same value; an inactive candidate has no rule, so it is held here.
    pub clause_digest: String,
    /// The digest of the whole candidate set of the document.
    pub candidate_set_digest: String,
    /// The grammar revision the document was read at.
    pub grammar_revision: u32,
    /// Proposed or inactive.
    pub status: PolicyCandidateStatus,
}

/// The document format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyProfile {
    /// A Markdown constitution: one clause per list item that starts with a keyword.
    Constitution,
    /// The bundled mini-markdown: one `rule <id> <severity> <scope>: <text>` per line.
    RuleSet,
}

/// Why a document was refused as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PolicyDocumentError {
    /// The document is not UTF-8.
    #[error("POLICY_DOCUMENT_NOT_UTF8: the policy document is not valid UTF-8")]
    NotUtf8,
}

/// The clause that was read, before the candidate set digest is known.
struct Clause {
    start_line: u32,
    end_line: u32,
    clause_digest: String,
    outcome: Result<(String, PolicySeverity, Option<String>), PolicyInactiveReason>,
}

/// Read the candidates of one policy document.
///
/// `source` carries the repository, path, commit and blob digest; the parser fills each
/// candidate's `start_line` and `end_line`. Line endings are normalized to LF first.
pub fn parse_policy_document(
    bytes: &[u8],
    profile: PolicyProfile,
    source: &PolicySourceRef,
) -> Result<Vec<PolicyCandidate>, PolicyDocumentError> {
    let text = std::str::from_utf8(bytes).map_err(|_| PolicyDocumentError::NotUtf8)?;
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let clauses = match profile {
        PolicyProfile::Constitution => constitution_clauses(&text),
        PolicyProfile::RuleSet => rule_set_clauses(&text),
    };
    let mut digests: Vec<&str> = clauses.iter().map(|c| c.clause_digest.as_str()).collect();
    digests.sort_unstable();
    let set_digest = sha256_hex(digests.join("\n").as_bytes());
    Ok(clauses
        .into_iter()
        .map(|clause| {
            let source_ref = PolicySourceRef {
                start_line: clause.start_line,
                end_line: clause.end_line,
                ..source.clone()
            };
            let (rule, status) = match clause.outcome {
                Ok((scope, severity, id)) => {
                    let rule_id = id.unwrap_or_else(|| {
                        format!("policy-{}", &clause.clause_digest[..16])
                    });
                    (
                        Some(PolicyRule {
                            rule_id,
                            scope,
                            severity,
                            source_ref: source_ref.clone(),
                            clause_digest: clause.clause_digest.clone(),
                        }),
                        PolicyCandidateStatus::Proposed,
                    )
                }
                Err(reason) => (None, PolicyCandidateStatus::Inactive { reason }),
            };
            PolicyCandidate {
                rule,
                source_ref,
                clause_digest: clause.clause_digest,
                candidate_set_digest: set_digest.clone(),
                grammar_revision: POLICY_GRAMMAR_REVISION,
                status,
            }
        })
        .collect())
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Trim and collapse whitespace runs to one space.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_absolute_iri(value: &str) -> bool {
    let Some((scheme, rest)) = value.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !rest.is_empty()
        && value
            .chars()
            .all(|c| matches!(c, '\u{21}'..='\u{7a}' | '|' | '~') && c != '{')
}

/// The marker of a list item, when `line` opens one at column 0.
fn list_item_text(line: &str) -> Option<&str> {
    if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
        return Some(rest);
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    line[digits..].strip_prefix(". ")
}

const KEYWORDS: [(&str, PolicySeverity); 5] = [
    ("MUST NOT", PolicySeverity::Block),
    ("MUST", PolicySeverity::Block),
    ("SHOULD NOT", PolicySeverity::Warn),
    ("SHOULD", PolicySeverity::Warn),
    ("MAY", PolicySeverity::Info),
];

fn leading_keyword(text: &str) -> Option<(PolicySeverity, &str)> {
    KEYWORDS.iter().find_map(|(keyword, severity)| {
        let rest = text.strip_prefix(keyword)?;
        (rest.is_empty() || rest.starts_with(char::is_whitespace)).then_some((*severity, rest))
    })
}

/// The scope annotations of `text` and the text without them.
fn split_scopes(text: &str) -> (Vec<String>, String) {
    const OPEN: &str = "{scope=";
    let mut scopes = Vec::new();
    let mut remaining = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(OPEN) {
        remaining.push_str(&rest[..at]);
        remaining.push(' ');
        let after = &rest[at + OPEN.len()..];
        match after.find('}') {
            Some(end) => {
                scopes.push(after[..end].to_string());
                rest = &after[end + 1..];
            }
            None => {
                scopes.push(after.to_string());
                rest = "";
            }
        }
    }
    remaining.push_str(rest);
    (scopes, collapse(&remaining))
}

/// Whole upper-case words of `text` that are `strong` or `weak` keywords.
fn mixes_strong_and_weak(text: &str) -> bool {
    let words: Vec<&str> = text.split(|c: char| !c.is_ascii_alphanumeric()).collect();
    let strong = words.contains(&"MUST");
    let weak = words.iter().any(|w| matches!(*w, "SHOULD" | "MAY"));
    strong && weak
}

fn constitution_clauses(text: &str) -> Vec<Clause> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut clauses = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let Some(first) = list_item_text(lines[index]) else {
            index += 1;
            continue;
        };
        let start = index;
        let mut joined = first.trim().to_string();
        index += 1;
        while index < lines.len()
            && !lines[index].trim().is_empty()
            && lines[index].starts_with(char::is_whitespace)
        {
            joined.push(' ');
            joined.push_str(lines[index].trim());
            index += 1;
        }
        let collapsed = collapse(&joined);
        let Some((severity, rest)) = leading_keyword(&collapsed) else {
            continue;
        };
        let (scopes, clause_text) = split_scopes(rest);
        let outcome = if clause_text.is_empty() || scopes.is_empty() {
            Err(PolicyInactiveReason::ClauseUnparsed)
        } else if scopes.len() > 1 {
            Err(PolicyInactiveReason::ClauseAmbiguous)
        } else if !is_absolute_iri(&scopes[0]) {
            Err(PolicyInactiveReason::ClauseUnparsed)
        } else if mixes_strong_and_weak(&clause_text) {
            Err(PolicyInactiveReason::ClauseAmbiguous)
        } else {
            Ok((scopes[0].clone(), severity, None))
        };
        clauses.push(Clause {
            start_line: (start + 1) as u32,
            end_line: index as u32,
            clause_digest: sha256_hex(clause_text.as_bytes()),
            outcome,
        });
    }
    clauses
}

fn valid_rule_id(id: &str) -> bool {
    let mut chars = id.chars();
    (2..=64).contains(&id.len())
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn rule_set_clauses(text: &str) -> Vec<Clause> {
    let mut clauses = Vec::new();
    for (index, line) in text.split('\n').enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let number = (index + 1) as u32;
        let parsed = parse_rule_line(trimmed);
        let (clause_text, outcome) = match parsed {
            Some((id, severity, scope, clause_text)) => (
                clause_text,
                Ok((scope, severity, Some(id))),
            ),
            None => (collapse(trimmed), Err(PolicyInactiveReason::ClauseUnparsed)),
        };
        clauses.push(Clause {
            start_line: number,
            end_line: number,
            clause_digest: sha256_hex(clause_text.as_bytes()),
            outcome,
        });
    }
    clauses
}

/// `rule <id> <info|warn|block> <scope>: <text>`.
fn parse_rule_line(line: &str) -> Option<(String, PolicySeverity, String, String)> {
    let mut parts = line.splitn(5, char::is_whitespace);
    if parts.next()? != "rule" {
        return None;
    }
    let id = parts.next()?;
    let severity = match parts.next()? {
        "info" => PolicySeverity::Info,
        "warn" => PolicySeverity::Warn,
        "block" => PolicySeverity::Block,
        _ => return None,
    };
    let scope = parts.next()?.strip_suffix(':')?;
    let clause_text = collapse(parts.next().unwrap_or(""));
    (valid_rule_id(id) && is_absolute_iri(scope) && !clause_text.is_empty())
        .then(|| (id.to_string(), severity, scope.to_string(), clause_text))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCOPE: &str = "https://example.invalid/class/Repo";

    fn source() -> PolicySourceRef {
        PolicySourceRef {
            repository: "repo".into(),
            path: "policy.md".into(),
            commit: "c0ffee".into(),
            blob_digest: "d1".into(),
            start_line: 0,
            end_line: 0,
        }
    }

    // spec: MEDULLA-ORCH-VOCAB-R012.7.1
    #[test]
    fn candidate_round_trips_and_inactive_has_reason() {
        let document = format!(
            "# Constitution\n\nProse is not a rule.\n\n- MUST NOT commit secrets {{scope={SCOPE}}}\n- SHOULD document it\n- plain prose item\n- MAY use a long\n  wrapped clause {{scope={SCOPE}}} {{scope={SCOPE}2}}\n"
        );
        let candidates =
            parse_policy_document(document.as_bytes(), PolicyProfile::Constitution, &source())
                .unwrap();
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].status, PolicyCandidateStatus::Proposed);
        assert_eq!(candidates[0].source_ref.start_line, 5);
        assert_eq!(candidates[0].source_ref.end_line, 5);
        let rule = candidates[0].rule.as_ref().unwrap();
        assert_eq!(rule.severity, PolicySeverity::Block);
        assert_eq!(rule.scope, SCOPE);
        assert_eq!(rule.source_ref, candidates[0].source_ref);
        assert!(rule.rule_id.starts_with("policy-"));
        assert_eq!(rule.rule_id.len(), "policy-".len() + 16);
        assert_eq!(
            candidates[1].status,
            PolicyCandidateStatus::Inactive {
                reason: PolicyInactiveReason::ClauseUnparsed
            }
        );
        assert!(candidates[1].rule.is_none());
        assert_eq!(
            candidates[2].status,
            PolicyCandidateStatus::Inactive {
                reason: PolicyInactiveReason::ClauseAmbiguous
            }
        );
        assert_eq!(
            (candidates[2].source_ref.start_line, candidates[2].source_ref.end_line),
            (8, 9)
        );
        assert!(candidates
            .iter()
            .all(|c| c.candidate_set_digest == candidates[0].candidate_set_digest
                && c.grammar_revision == POLICY_GRAMMAR_REVISION));
        for candidate in &candidates {
            let wire = serde_json::to_string(candidate).unwrap();
            assert_eq!(serde_json::from_str::<PolicyCandidate>(&wire).unwrap(), *candidate);
        }
        let wire = serde_json::to_value(&candidates[1].status).unwrap();
        assert_eq!(wire["inactive"]["reason"], "CLAUSE_UNPARSED");
        assert_eq!(
            parse_policy_document(&[0xff, 0xfe], PolicyProfile::Constitution, &source()),
            Err(PolicyDocumentError::NotUtf8)
        );
    }

    // spec: MEDULLA-ORCH-VOCAB-R012.7.1
    #[test]
    fn both_formats_parse_to_one_type() {
        let constitution = format!("- MUST NOT commit secrets {{scope={SCOPE}}}\n");
        let rule_set = format!(
            "# bundled\n\nrule no-secrets block {SCOPE}: commit secrets\nnot a rule line\n"
        );
        let a = parse_policy_document(constitution.as_bytes(), PolicyProfile::Constitution, &source())
            .unwrap();
        let b = parse_policy_document(rule_set.as_bytes(), PolicyProfile::RuleSet, &source())
            .unwrap();
        let (ra, rb) = (a[0].rule.clone().unwrap(), b[0].rule.clone().unwrap());
        assert_eq!((ra.severity, &ra.scope), (rb.severity, &rb.scope));
        assert_eq!(ra.clause_digest, rb.clause_digest);
        assert_eq!(rb.rule_id, "no-secrets");
        assert_ne!(ra.rule_id, rb.rule_id);
        assert_eq!((b[0].source_ref.start_line, b[0].source_ref.end_line), (3, 3));
        assert_eq!(
            b[1].status,
            PolicyCandidateStatus::Inactive {
                reason: PolicyInactiveReason::ClauseUnparsed
            }
        );
        assert_eq!(b[1].source_ref.start_line, 4);
    }
}
