//! Append-only ontology edit chain (MEDULLA-TYPED-PACKS-R651.2, R651.4).
//!
//! Pure functions over [`OntologyEdit`]. The chain links each edit to the
//! digest of the one before it; `invert` and `revert` never delete an edit,
//! they append its inversion. A refusal returns before any edit is appended, so
//! it changes nothing. The caller stores the edits as graph nodes inside the
//! same commit as the schema change.

use std::collections::BTreeSet;
use std::fmt;

use medulla_types::contract::{Digest256, ServerErrorCode};
use medulla_types::ontology_edit::{NamingFinding, NamingMode, OntologyEdit};

/// A refused chain operation: a registered code plus a detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditRefusal {
    /// The registered refusal code.
    pub code: ServerErrorCode,
    /// What was refused.
    pub detail: String,
}

impl fmt::Display for EditRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.detail)
    }
}

impl std::error::Error for EditRefusal {}

fn refuse(code: ServerErrorCode, detail: impl Into<String>) -> EditRefusal {
    EditRefusal {
        code,
        detail: detail.into(),
    }
}

/// SHA-256 of one edit's canonical JSON (the struct's declared field order).
pub fn edit_digest(edit: &OntologyEdit) -> Digest256 {
    let canonical = serde_json::to_vec(edit).unwrap_or_default();
    Digest256::sha256(&canonical)
}

/// The digest of the newest edit, `None` for an empty chain.
pub fn chain_head(chain: &[OntologyEdit]) -> Option<Digest256> {
    chain.last().map(edit_digest)
}

/// What an edit says before the chain stamps its id and link onto it.
#[derive(Clone, Debug)]
pub struct EditDraft {
    /// Add, change or remove.
    pub kind: medulla_types::ontology_edit::EditKind,
    /// The subject the edit is about.
    pub subject_iri: String,
    /// Digest before the edit.
    pub before_digest: Option<Digest256>,
    /// Digest after the edit.
    pub after_digest: Option<Digest256>,
    /// The governed change that seals the edit.
    pub sealed_by: Option<String>,
}

impl EditDraft {
    /// Stamp the draft with the next id and the current chain head.
    pub fn link(self, chain: &[OntologyEdit]) -> OntologyEdit {
        self.link_after(chain.len(), chain_head(chain))
    }

    /// Stamp the draft as the edit after `count` edits whose newest digest is
    /// `head`, for a caller that keeps the head instead of the whole chain.
    pub fn link_after(self, count: usize, head: Option<Digest256>) -> OntologyEdit {
        OntologyEdit {
            edit_id: format!("edit-{:08}", count + 1),
            kind: self.kind,
            subject_iri: self.subject_iri,
            before_digest: self.before_digest,
            after_digest: self.after_digest,
            prev_digest: head,
            sealed_by: self.sealed_by,
        }
    }
}

/// The newest stored edit must hash to the recorded head, else the chain was
/// altered after it was written (`EDIT_CHAIN_BROKEN`).
pub fn verify_head(
    newest: Option<&OntologyEdit>,
    head: Option<Digest256>,
) -> Result<(), EditRefusal> {
    if newest.map(edit_digest) == head {
        return Ok(());
    }
    Err(refuse(
        ServerErrorCode::EditChainBroken,
        "the newest stored edit does not match the recorded chain head",
    ))
}

/// Every edit must link to the digest of the one before it, and the chain must
/// end at `head` (`EDIT_CHAIN_BROKEN` otherwise).
pub fn verify_chain(chain: &[OntologyEdit], head: Option<Digest256>) -> Result<(), EditRefusal> {
    let mut previous = None;
    for edit in chain {
        if edit.prev_digest != previous {
            return Err(refuse(
                ServerErrorCode::EditChainBroken,
                format!("edit '{}' does not link to its predecessor", edit.edit_id),
            ));
        }
        previous = Some(edit_digest(edit));
    }
    verify_head(chain.last(), head)
}

/// Append `edit` to `chain`; its `prev_digest` must be the chain head
/// (`EDIT_CHAIN_BROKEN` otherwise). Returns the new head.
pub fn append(chain: &mut Vec<OntologyEdit>, edit: OntologyEdit) -> Result<Digest256, EditRefusal> {
    let head = chain_head(chain);
    if edit.prev_digest != head {
        return Err(refuse(
            ServerErrorCode::EditChainBroken,
            format!(
                "edit '{}' links to {:?}, the chain head is {:?}",
                edit.edit_id,
                edit.prev_digest.map(Digest256::to_hex),
                head.map(Digest256::to_hex)
            ),
        ));
    }
    let digest = edit_digest(&edit);
    chain.push(edit);
    Ok(digest)
}

/// The inversion of `edit`: before and after swap, `add` and `remove` flip.
/// It is a fresh unsealed edit linked to the current chain head.
pub fn invert(chain: &[OntologyEdit], edit: &OntologyEdit) -> OntologyEdit {
    EditDraft {
        kind: edit.kind.inverse(),
        subject_iri: edit.subject_iri.clone(),
        before_digest: edit.after_digest,
        after_digest: edit.before_digest,
        sealed_by: None,
    }
    .link(chain)
}

/// The edits of `subject` that a revert inverts, newest first.
///
/// Walk back from the subject's current digest; an edit is crossed when it
/// produced the cursor. The walk stops at the subject's first digest.
fn edits_to_invert<'a>(chain: &'a [OntologyEdit], subject: &str) -> Vec<&'a OntologyEdit> {
    let history: Vec<&OntologyEdit> = chain
        .iter()
        .filter(|edit| edit.subject_iri == subject)
        .collect();
    let (Some(first), Some(last)) = (history.first(), history.last()) else {
        return Vec::new();
    };
    let mut cursor = last.after_digest;
    let mut crossed = Vec::new();
    for edit in history.iter().rev() {
        if cursor == first.before_digest {
            break;
        }
        if edit.after_digest == cursor {
            cursor = edit.before_digest;
            crossed.push(*edit);
        }
    }
    crossed
}

/// Revert `subject` to its first digest by appending inversions, newest first.
///
/// An edit with `sealed_by` is crossed only when `approval` names the new
/// governed change that allows it (`EDIT_SEALED` otherwise). Nothing is
/// appended when the revert is refused. Returns the appended inversions.
pub fn revert(
    chain: &mut Vec<OntologyEdit>,
    subject: &str,
    approval: Option<&str>,
) -> Result<Vec<OntologyEdit>, EditRefusal> {
    let crossed: Vec<OntologyEdit> = edits_to_invert(chain, subject)
        .into_iter()
        .cloned()
        .collect();
    if let Some(sealed) = crossed
        .iter()
        .find(|edit| edit.sealed_by.is_some() && approval.is_none())
    {
        return Err(refuse(
            ServerErrorCode::EditSealed,
            format!(
                "edit '{}' of '{subject}' is sealed by '{}'",
                sealed.edit_id,
                sealed.sealed_by.as_deref().unwrap_or_default()
            ),
        ));
    }
    let mut appended = Vec::with_capacity(crossed.len());
    for edit in &crossed {
        let inversion = invert(chain, edit);
        append(chain, inversion.clone())?;
        appended.push(inversion);
    }
    Ok(appended)
}

/// The local name of an IRI: what follows the last `#` or `/`.
fn local_name(iri: &str) -> &str {
    iri.rsplit(['#', '/']).next().unwrap_or(iri)
}

fn is_pascal_case(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_uppercase())
        && name.chars().all(|c| c.is_ascii_alphanumeric())
}

fn is_camel_case(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_lowercase())
        && name.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Classes must be PascalCase and properties camelCase, in O(names).
///
/// Findings are sorted by subject; their severity follows `mode`.
pub fn lint_names(
    classes: &BTreeSet<String>,
    properties: &BTreeSet<String>,
    mode: NamingMode,
) -> Vec<NamingFinding> {
    let checks: [(&BTreeSet<String>, &str, fn(&str) -> bool); 2] = [
        (classes, "class_pascal_case", is_pascal_case),
        (properties, "property_camel_case", is_camel_case),
    ];
    let mut findings = Vec::new();
    for (names, rule, conforms) in checks {
        findings.extend(
            names
                .iter()
                .filter(|iri| !conforms(local_name(iri)))
                .map(|iri| NamingFinding {
                    subject_iri: iri.clone(),
                    rule: rule.to_string(),
                    severity: mode.severity(),
                }),
        );
    }
    findings.sort_by(|a, b| a.subject_iri.cmp(&b.subject_iri));
    findings
}

/// Refuse with `NAMING_VIOLATION` when any finding is an error.
pub fn refuse_naming_errors(findings: &[NamingFinding]) -> Result<(), EditRefusal> {
    let Some(first) = findings
        .iter()
        .find(|f| f.severity == medulla_types::ontology_edit::NamingSeverity::Error)
    else {
        return Ok(());
    };
    Err(refuse(
        ServerErrorCode::NamingViolation,
        format!(
            "'{}' breaks {} (and {} more)",
            first.subject_iri,
            first.rule,
            findings.len() - 1
        ),
    ))
}
