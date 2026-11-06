//! Memory snapshot, cursor, sync request and outcome (MEDULLA-DURABLE-KERNEL-R085.1).
//!
//! Pure serde types plus the pure compare-and-set core [`decide`]
//! (MEDULLA-DURABLE-KERNEL-R085.3). The served handlers live in
//! `medulla-h-graph` (`graph_ops/memory_snapshot.rs`); nothing here reads a
//! clock, a store or a request context.

use serde::{Deserialize, Serialize};

use crate::contract::{Digest256, ServerErrorCode};

/// The largest `doc_id`, in bytes.
pub const MAX_DOC_ID_BYTES: usize = 256;
/// The largest snapshot content, in bytes (`Fixed`).
pub const MAX_CONTENT_BYTES: usize = 1_048_576;
/// The largest `request_id`, in bytes.
pub const MAX_REQUEST_ID_BYTES: usize = 128;
/// The largest list cursor token, in bytes.
pub const MAX_CURSOR_BYTES: usize = 1024;

/// A refused memory-sync value or request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{}: {detail}", .code.as_str())]
pub struct MemorySyncError {
    pub code: MemorySyncRefusal,
    pub detail: &'static str,
}

/// The refusal codes of the memory-sync surface (MEDULLA-DURABLE-KERNEL-R085.5
/// and R085.7). Each is a registered [`ServerErrorCode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "contract-schema", schemars(with = "String"))]
pub enum MemorySyncRefusal {
    NotAvailable,
    VersionMalformed,
    DigestMismatch,
    ContentTooLarge,
    ContentMalformed,
    IncarnationExpired,
    StorageUnavailable,
    CursorInvalid,
    CursorExpired,
    RetentionExhausted,
}

impl MemorySyncRefusal {
    /// Every code, in declaration order.
    pub const ALL: &'static [Self] = &[
        Self::NotAvailable,
        Self::VersionMalformed,
        Self::DigestMismatch,
        Self::ContentTooLarge,
        Self::ContentMalformed,
        Self::IncarnationExpired,
        Self::StorageUnavailable,
        Self::CursorInvalid,
        Self::CursorExpired,
        Self::RetentionExhausted,
    ];

    /// The registered server error code of this refusal.
    pub fn server_code(self) -> ServerErrorCode {
        match self {
            Self::NotAvailable => ServerErrorCode::MemsyncNotAvailable,
            Self::VersionMalformed => ServerErrorCode::MemsyncVersionMalformed,
            Self::DigestMismatch => ServerErrorCode::MemsyncDigestMismatch,
            Self::ContentTooLarge => ServerErrorCode::MemsyncContentTooLarge,
            Self::ContentMalformed => ServerErrorCode::MemsyncContentMalformed,
            Self::IncarnationExpired => ServerErrorCode::MemsyncIncarnationExpired,
            Self::StorageUnavailable => ServerErrorCode::MemsyncStorageUnavailable,
            Self::CursorInvalid => ServerErrorCode::MemsyncCursorInvalid,
            Self::CursorExpired => ServerErrorCode::MemsyncCursorExpired,
            Self::RetentionExhausted => ServerErrorCode::MemsyncRetentionExhausted,
        }
    }

    /// The wire token.
    pub fn as_str(self) -> &'static str {
        self.server_code().as_str()
    }
}

impl Serialize for MemorySyncRefusal {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for MemorySyncRefusal {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let token = String::deserialize(deserializer)?;
        Self::ALL
            .iter()
            .copied()
            .find(|code| code.as_str() == token)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown memory sync code {token}")))
    }
}

fn refusal(code: MemorySyncRefusal, detail: &'static str) -> MemorySyncError {
    MemorySyncError { code, detail }
}

/// A document id: at most 256 bytes of `[a-z0-9._:/-]`, never empty.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
#[cfg_attr(feature = "contract-schema", schemars(with = "String"))]
pub struct DocId(String);

impl DocId {
    pub fn new(value: impl Into<String>) -> Result<Self, MemorySyncError> {
        let value = value.into();
        let allowed = |byte: u8| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b':' | b'/' | b'-')
        };
        if value.is_empty() || value.len() > MAX_DOC_ID_BYTES || !value.bytes().all(allowed) {
            return Err(refusal(
                MemorySyncRefusal::VersionMalformed,
                "doc_id must be 1..=256 bytes of [a-z0-9._:/-]",
            ));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for DocId {
    type Error = MemorySyncError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<DocId> for String {
    fn from(value: DocId) -> Self {
        value.0
    }
}

/// A caller-chosen request id: 1 to 128 bytes, no control characters.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
#[cfg_attr(feature = "contract-schema", schemars(with = "String"))]
pub struct RequestId(String);

impl RequestId {
    pub fn new(value: impl Into<String>) -> Result<Self, MemorySyncError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_REQUEST_ID_BYTES
            || value.chars().any(char::is_control)
        {
            return Err(refusal(
                MemorySyncRefusal::VersionMalformed,
                "request_id must be 1..=128 bytes without control characters",
            ));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RequestId {
    type Error = MemorySyncError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<RequestId> for String {
    fn from(value: RequestId) -> Self {
        value.0
    }
}

/// An opaque list cursor token (R085.2 signs and verifies it).
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
#[cfg_attr(feature = "contract-schema", schemars(with = "String"))]
pub struct ListCursor(String);

impl ListCursor {
    pub fn new(value: impl Into<String>) -> Result<Self, MemorySyncError> {
        let value = value.into();
        if value.is_empty() || value.len() > MAX_CURSOR_BYTES {
            return Err(refusal(
                MemorySyncRefusal::CursorInvalid,
                "cursor must be 1..=1024 bytes",
            ));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ListCursor {
    type Error = MemorySyncError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<ListCursor> for String {
    fn from(value: ListCursor) -> Self {
        value.0
    }
}

/// Who wrote a revision, under which policy, for which request.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotProvenance {
    pub actor: String,
    pub policy_digest: Digest256,
    pub request_id: RequestId,
}

/// The accepted head of one document.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemorySnapshotHead {
    pub doc_id: DocId,
    pub revision: u64,
    pub parent_revision: Option<u64>,
    pub content_digest: Digest256,
    pub content_len: u32,
    pub graph_incarnation: String,
    pub provenance: SnapshotProvenance,
    pub retain_until_ms: Option<u64>,
}

impl MemorySnapshotHead {
    /// Refuses a head that names more content than the fixed ceiling or a
    /// parent that is not older than the revision.
    pub fn validate(&self) -> Result<(), MemorySyncError> {
        if self.content_len as usize > MAX_CONTENT_BYTES {
            return Err(refusal(
                MemorySyncRefusal::ContentTooLarge,
                "content_len above 1048576",
            ));
        }
        if self.revision == 0
            || self
                .parent_revision
                .is_some_and(|parent| parent >= self.revision)
        {
            return Err(refusal(
                MemorySyncRefusal::VersionMalformed,
                "revision starts at 1 and its parent is older",
            ));
        }
        Ok(())
    }
}

/// How an `Resolve` action settles a retained candidate.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "resolution", rename_all = "snake_case", deny_unknown_fields)]
pub enum Resolution {
    KeepHead,
    TakeCandidate,
    Merged {
        content: String,
        content_digest: Digest256,
    },
}

/// The action one sync request carries.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum SyncAction {
    /// Compare the digest of the file as the client sees it with the head.
    ObserveFile { observed_digest: Digest256 },
    Submit {
        expected_revision: Option<u64>,
        content: String,
        content_digest: Digest256,
    },
    Resolve {
        candidate_revision: u64,
        expected_head_revision: u64,
        resolution: Resolution,
    },
    /// R085.6: the client reports what it materialized.
    Acknowledge {
        revision: u64,
        content_digest: Digest256,
        observation_generation: u64,
    },
}

/// One sync request.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncRequest {
    pub request_id: RequestId,
    pub doc_id: DocId,
    pub action: SyncAction,
}

fn check_content(content: &str, digest: &Digest256) -> Result<(), MemorySyncError> {
    if content.len() > MAX_CONTENT_BYTES {
        return Err(refusal(
            MemorySyncRefusal::ContentTooLarge,
            "content above 1048576 bytes",
        ));
    }
    if Digest256::sha256(content.as_bytes()) != *digest {
        return Err(refusal(
            MemorySyncRefusal::DigestMismatch,
            "content_digest is not the SHA-256 of the content",
        ));
    }
    Ok(())
}

impl SyncRequest {
    /// Checks every bound of the request that needs no store.
    pub fn validate(&self) -> Result<(), MemorySyncError> {
        match &self.action {
            SyncAction::ObserveFile { .. } | SyncAction::Acknowledge { .. } => Ok(()),
            SyncAction::Submit {
                content,
                content_digest,
                ..
            } => check_content(content, content_digest),
            SyncAction::Resolve { resolution, .. } => match resolution {
                Resolution::Merged {
                    content,
                    content_digest,
                } => check_content(content, content_digest),
                Resolution::KeepHead | Resolution::TakeCandidate => Ok(()),
            },
        }
    }
}

/// Whether the client's file matches the accepted head.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    InSync,
    Pending,
    Conflict,
    Unavailable,
}

/// The tagged result of one sync request. `Refused` holds no head field.
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum SyncOutcome {
    Accepted {
        head: MemorySnapshotHead,
    },
    ConflictRetained {
        head: MemorySnapshotHead,
        candidate_revision: u64,
    },
    Resolved {
        head: MemorySnapshotHead,
    },
    Observed {
        state: SyncState,
        head_revision: u64,
        head_digest: Digest256,
    },
    Acknowledged {
        state: SyncState,
    },
    Refused {
        code: MemorySyncRefusal,
    },
}

/// The bounded knobs of the memory-sync surface; defaults and bounds are the
/// rows' (R085.2, R085.3, R085.7).
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemorySyncConfig {
    pub cursor_ttl_s: u64,
    pub receipt_ttl_s: u64,
    pub history_retention_s: u64,
    pub max_revisions_per_doc: u32,
    pub capacity_bytes: u64,
}

impl Default for MemorySyncConfig {
    fn default() -> Self {
        Self {
            cursor_ttl_s: 900,
            receipt_ttl_s: 86_400,
            history_retention_s: 2_592_000,
            max_revisions_per_doc: 64,
            capacity_bytes: 1_073_741_824,
        }
    }
}

impl MemorySyncConfig {
    /// Refuses a value outside its bounds; startup refuses on the error.
    pub fn validate(&self) -> Result<(), String> {
        fn within<T: PartialOrd + std::fmt::Display>(
            name: &str,
            value: T,
            low: T,
            high: T,
        ) -> Result<(), String> {
            if value < low || value > high {
                return Err(format!(
                    "{name} must be within [{low}, {high}], got {value}"
                ));
            }
            Ok(())
        }
        within("memory_sync.cursor_ttl_s", self.cursor_ttl_s, 60, 86_400)?;
        within(
            "memory_sync.receipt_ttl_s",
            self.receipt_ttl_s,
            3_600,
            2_592_000,
        )?;
        within(
            "memory_sync.history_retention_s",
            self.history_retention_s,
            3_600,
            31_536_000,
        )?;
        within(
            "memory_sync.max_revisions_per_doc",
            self.max_revisions_per_doc,
            1,
            4096,
        )?;
        within(
            "memory_sync.capacity_bytes",
            self.capacity_bytes,
            1_048_576,
            1_099_511_627_776,
        )
    }
}

/// The engine-stamped values of one sync request: the verified actor, the
/// policy digest and the graph incarnation. Replay reads them from the log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SyncStamp {
    pub actor: String,
    pub policy_digest: Digest256,
    pub graph_incarnation: String,
}

/// The served `ReadMemorySnapshot` result: the head or a retained revision
/// with its bytes (MEDULLA-DURABLE-KERNEL-R085.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MemorySnapshotRead {
    pub head: MemorySnapshotHead,
    pub content: String,
}

/// The served `ListMemorySnapshots` result: heads in `doc_id` order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MemorySnapshotList {
    pub heads: Vec<MemorySnapshotHead>,
    pub next_cursor: Option<ListCursor>,
}

/// What the store must do for one request (MEDULLA-DURABLE-KERNEL-R085.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Answer without writing.
    Reply(SyncOutcome),
    /// Write a new head revision `revision` with parent `parent_revision`.
    Accept {
        revision: u64,
        parent_revision: Option<u64>,
        content: String,
        content_digest: Digest256,
    },
    /// Keep the content as a retained candidate; the head is unchanged.
    RetainCandidate {
        content: String,
        content_digest: Digest256,
        head_revision: u64,
    },
    /// Apply a resolution; the handler checks the candidate is retained.
    Resolve {
        candidate_revision: u64,
        resolution: Resolution,
        new_revision: u64,
    },
    /// R085.6: store the acknowledgment record with the computed state.
    Acknowledge { state: SyncState },
}

/// The pure compare-and-set core. `head` is the accepted head of
/// `request.doc_id`, `None` for an absent document. A request that fails
/// [`SyncRequest::validate`] is refused before any head is consulted.
pub fn decide(head: Option<&MemorySnapshotHead>, request: &SyncRequest) -> Decision {
    if let Err(error) = request.validate() {
        return Decision::Reply(SyncOutcome::Refused { code: error.code });
    }
    let not_available = || {
        Decision::Reply(SyncOutcome::Refused {
            code: MemorySyncRefusal::NotAvailable,
        })
    };
    match &request.action {
        SyncAction::ObserveFile { observed_digest } => match head {
            None => not_available(),
            Some(head) => Decision::Reply(SyncOutcome::Observed {
                state: if head.content_digest == *observed_digest {
                    SyncState::InSync
                } else {
                    SyncState::Pending
                },
                head_revision: head.revision,
                head_digest: head.content_digest,
            }),
        },
        SyncAction::Submit {
            expected_revision,
            content,
            content_digest,
        } => match (head, expected_revision) {
            (None, None) => Decision::Accept {
                revision: 1,
                parent_revision: None,
                content: content.clone(),
                content_digest: *content_digest,
            },
            (None, Some(_)) => not_available(),
            (Some(head), expected) if *expected == Some(head.revision) => {
                if head.content_digest == *content_digest {
                    Decision::Reply(SyncOutcome::Accepted { head: head.clone() })
                } else {
                    Decision::Accept {
                        revision: head.revision + 1,
                        parent_revision: Some(head.revision),
                        content: content.clone(),
                        content_digest: *content_digest,
                    }
                }
            }
            (Some(head), _) => Decision::RetainCandidate {
                content: content.clone(),
                content_digest: *content_digest,
                head_revision: head.revision,
            },
        },
        SyncAction::Resolve {
            candidate_revision,
            expected_head_revision,
            resolution,
        } => match head {
            Some(head) if head.revision == *expected_head_revision => Decision::Resolve {
                candidate_revision: *candidate_revision,
                resolution: resolution.clone(),
                new_revision: head.revision + 1,
            },
            Some(_) => Decision::Reply(SyncOutcome::Refused {
                code: MemorySyncRefusal::VersionMalformed,
            }),
            None => not_available(),
        },
        SyncAction::Acknowledge {
            revision,
            content_digest,
            ..
        } => match head {
            None => not_available(),
            Some(head) => Decision::Acknowledge {
                state: if head.revision == *revision && head.content_digest == *content_digest {
                    SyncState::InSync
                } else {
                    SyncState::Pending
                },
            },
        },
    }
}
