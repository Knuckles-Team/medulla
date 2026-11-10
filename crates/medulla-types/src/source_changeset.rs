//! The source changeset record (MEDULLA-SCM-R002): one commit-shaped change to a
//! repository, with its structured trailers (MEDULLA-SCM-R002.1).
//!
//! Pure data. Acceptance, rebase and git import and export live above this
//! crate; MEDULLA-SDLC derivation reads the typed trailer fields.

use serde::{Deserialize, Serialize};

/// What a changeset did to one path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathOp {
    Add,
    Modify,
    Delete,
    Rename,
}

/// One path that changed against the first parent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangedPath {
    pub path: String,
    pub op: PathOp,
    pub old_blob: Option<String>,
    pub new_blob: Option<String>,
    pub mode: u32,
}

/// One file version a changeset names (MEDULLA-SCM-R001). `digest` is the value
/// `BlobCommit` returned for the stored bytes; `size` counts the bytes from 0
/// to `changeset_max_blob_bytes`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmittedBlobRef {
    pub digest: String,
    pub size: u64,
}

/// Typed refusal of the source-changeset family; each variant serves one
/// stable refusal code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangesetRefusal {
    /// Declared and recomputed digests or sizes differ.
    BlobDigestMismatch,
    /// A declared blob exceeds `changeset_max_blob_bytes`.
    BlobTooLarge,
    /// The digest is unknown to the blob store, or unreadable.
    BlobNotFound,
}

impl ChangesetRefusal {
    /// The stable refusal code served on the wire.
    pub fn code(&self) -> &'static str {
        match self {
            Self::BlobDigestMismatch => "BLOB_DIGEST_MISMATCH",
            Self::BlobTooLarge => "BLOB_TOO_LARGE",
            Self::BlobNotFound => "BLOB_NOT_FOUND",
        }
    }
}

/// The structured form of the `Spec:`, `Fixes:` and `Spec-Withdraw:` trailers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trailers {
    pub spec_ids: Vec<String>,
    pub fixes: Vec<String>,
    pub withdraw: Vec<String>,
    pub withdraw_reason: Option<String>,
}

fn split_ids(value: &str) -> impl Iterator<Item = String> + '_ {
    value
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

impl Trailers {
    /// Parse the trailers from the last paragraph of a commit message. A
    /// `Spec: none (reason)` line cites no requirement.
    pub fn parse(message: &str) -> Self {
        let mut out = Self::default();
        let Some(block) = message.trim_end().rsplit("\n\n").next() else {
            return out;
        };
        for line in block.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "Spec" if !value.starts_with("none") => out.spec_ids.extend(split_ids(value)),
                "Fixes" => out.fixes.extend(split_ids(value)),
                "Spec-Withdraw" => {
                    let (ids, reason) = match value.split_once(" (") {
                        Some((ids, reason)) => (ids, reason.strip_suffix(')')),
                        None => (value, None),
                    };
                    out.withdraw.extend(split_ids(ids));
                    out.withdraw_reason = reason.map(str::to_owned);
                }
                _ => {}
            }
        }
        out
    }

    /// Render the trailer lines, in the order `Spec`, `Fixes`, `Spec-Withdraw`.
    pub fn render(&self) -> String {
        let mut lines = Vec::new();
        if !self.spec_ids.is_empty() {
            lines.push(format!("Spec: {}", self.spec_ids.join(", ")));
        }
        if !self.fixes.is_empty() {
            lines.push(format!("Fixes: {}", self.fixes.join(", ")));
        }
        if !self.withdraw.is_empty() {
            let reason = self
                .withdraw_reason
                .as_ref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default();
            lines.push(format!(
                "Spec-Withdraw: {}{reason}",
                self.withdraw.join(", ")
            ));
        }
        lines.join("\n")
    }
}

/// A changeset: the paths that changed against the first parent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Changeset {
    pub id: String,
    pub repo: String,
    pub parents: Vec<String>,
    pub author_principal: String,
    pub message: String,
    pub changed_paths: Vec<ChangedPath>,
    pub created_at: u64,
    pub trailers: Trailers,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies MEDULLA-SCM-R002.1: Structured trailers on a changeset
    // spec: MEDULLA-SCM-R002.1
    #[test]
    fn trailers_parse_and_render_round_trip() {
        let message = "feat: x\n\nbody\n\nSpec: MEDULLA-A-R001, MEDULLA-A-R002.1\nFixes: DEF-7\nSpec-Withdraw: MEDULLA-A-R009 (superseded)";
        let trailers = Trailers::parse(message);
        assert_eq!(trailers.spec_ids, ["MEDULLA-A-R001", "MEDULLA-A-R002.1"]);
        assert_eq!(trailers.fixes, ["DEF-7"]);
        assert_eq!(trailers.withdraw, ["MEDULLA-A-R009"]);
        assert_eq!(trailers.withdraw_reason.as_deref(), Some("superseded"));
        assert_eq!(Trailers::parse(&trailers.render()), trailers);
        assert!(Trailers::parse("fix\n\nSpec: none (ci)")
            .spec_ids
            .is_empty());
    }

    /// Verifies MEDULLA-SCM-R001: the submitted blob ref is a closed wire
    /// shape, and the family's blob refusals carry their stable codes.
    // spec: MEDULLA-SCM-R001
    #[test]
    fn submitted_blob_ref_is_closed_and_refusal_codes_are_stable() {
        let submitted = SubmittedBlobRef {
            digest: "a".repeat(64),
            size: 134_217_728,
        };
        let encoded = serde_json::to_string(&submitted).unwrap();
        assert_eq!(
            serde_json::from_str::<SubmittedBlobRef>(&encoded).unwrap(),
            submitted
        );
        // The wire shape is closed: an unknown field is refused.
        assert!(
            serde_json::from_str::<SubmittedBlobRef>(r#"{"digest":"a","size":1,"extra":0}"#)
                .is_err()
        );
        assert_eq!(
            ChangesetRefusal::BlobDigestMismatch.code(),
            "BLOB_DIGEST_MISMATCH"
        );
        assert_eq!(ChangesetRefusal::BlobTooLarge.code(), "BLOB_TOO_LARGE");
        assert_eq!(ChangesetRefusal::BlobNotFound.code(), "BLOB_NOT_FOUND");
    }
}
