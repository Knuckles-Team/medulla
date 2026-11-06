//! Workflow registration identity and registry model (MEDULLA-DECISION-ENGINE-R166.1).
//!
//! This is the serializable type-level registry state. Engine initialization and
//! served invocation are owned by the runtime layer.

use serde::{Deserialize, Serialize};

/// Identity of one immutable workflow definition.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct WorkflowIdentity {
    pub workflow_id: String,
    pub version: String,
    /// Canonical `sha256:` digest of the workflow content.
    pub content_digest: String,
}

impl WorkflowIdentity {
    pub fn validate(&self) -> Result<(), WorkflowRegistryError> {
        if self.workflow_id.trim().is_empty() {
            return Err(WorkflowRegistryError::WorkflowIdMissing);
        }
        if self.version.trim().is_empty() {
            return Err(WorkflowRegistryError::VersionMissing);
        }
        let digest = self
            .content_digest
            .strip_prefix("sha256:")
            .ok_or(WorkflowRegistryError::ContentDigestInvalid)?;
        medulla_types::contract::Digest256::parse(digest)
            .map_err(|_| WorkflowRegistryError::ContentDigestInvalid)?;
        Ok(())
    }
}

/// The registered workflow data is its identity; content is addressed by digest.
pub type WorkflowRegistration = WorkflowIdentity;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum WorkflowRegistryError {
    WorkflowIdMissing,
    VersionMissing,
    ContentDigestInvalid,
    IdentityUnknown,
    /// The workflow exists but its owner scope is not the caller's.
    Unauthorized,
}

impl WorkflowRegistryError {
    /// The registered `ServerErrorCode` token this refusal is reported under
    /// (MEDULLA-DECISION-ENGINE-R166.4).
    pub fn code(self) -> &'static str {
        use medulla_types::contract::ServerErrorCode as Code;
        match self {
            Self::WorkflowIdMissing => Code::WorkflowIdMissing,
            Self::VersionMissing => Code::WorkflowVersionMissing,
            Self::ContentDigestInvalid => Code::WorkflowDigestInvalid,
            Self::IdentityUnknown => Code::WorkflowIdentityUnknown,
            Self::Unauthorized => Code::WorkflowUnauthorized,
        }
        .as_str()
    }
}

/// Where a registered workflow stands (MEDULLA-DECISION-ENGINE-R166.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum WorkflowLifecycle {
    Active,
    Withdrawn,
    Stale,
}

impl WorkflowLifecycle {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Withdrawn => "withdrawn",
            Self::Stale => "stale",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "active" => Some(Self::Active),
            "withdrawn" => Some(Self::Withdrawn),
            "stale" => Some(Self::Stale),
            _ => None,
        }
    }
}

/// One served registration: the immutable identity, the scope that owns it and
/// its lifecycle state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct WorkflowRegistrationEntry {
    pub identity: WorkflowIdentity,
    /// On `Register`, empty or the verified tenant scope; the engine stores the
    /// verified scope and refuses any other value.
    pub owner_scope: String,
    pub lifecycle: WorkflowLifecycle,
}

impl WorkflowRegistrationEntry {
    /// The graph node id a registration of `identity` is stored under.
    pub fn node_id(identity: &WorkflowIdentity) -> String {
        format!(
            "workflow:{}:{}:{}",
            identity.workflow_id, identity.version, identity.content_digest
        )
    }

    /// The tenant graph node properties this registration is stored as.
    pub fn node_properties(&self, tenant_id: &str) -> serde_json::Value {
        serde_json::json!({
            "type": WORKFLOW_NODE_TYPE,
            "workflow_id": self.identity.workflow_id,
            "version": self.identity.version,
            "content_digest": self.identity.content_digest,
            "owner_scope": self.owner_scope,
            "lifecycle": self.lifecycle.as_str(),
            "tenant": tenant_id,
        })
    }

    /// Read a registration back from stored node properties; `None` when the
    /// node is not a workflow registration.
    pub fn from_node_properties(
        properties: &serde_json::Map<String, serde_json::Value>,
    ) -> Option<Self> {
        let text = |key: &str| properties.get(key)?.as_str().map(str::to_string);
        if text("type")? != WORKFLOW_NODE_TYPE {
            return None;
        }
        Some(Self {
            identity: WorkflowIdentity {
                workflow_id: text("workflow_id")?,
                version: text("version")?,
                content_digest: text("content_digest")?,
            },
            owner_scope: text("owner_scope")?,
            lifecycle: WorkflowLifecycle::parse(&text("lifecycle")?)?,
        })
    }
}

/// The graph node type of a stored registration.
pub const WORKFLOW_NODE_TYPE: &str = "WorkflowRegistration";

/// Answer a `Read` of `identity` from the stored node properties the caller can
/// see (`None` when no visible node exists), on behalf of `caller_scope`.
///
/// The visible registration is loaded into a [`WorkflowRegistry`] and read back
/// by exact identity, so an invalid or unknown identity refuses with the
/// registry's own error and no default workflow is ever returned. A registration
/// owned by another scope refuses [`WorkflowRegistryError::Unauthorized`].
pub fn read_registration(
    stored: Option<&serde_json::Map<String, serde_json::Value>>,
    caller_scope: &str,
    identity: &WorkflowIdentity,
) -> Result<WorkflowRegistrationEntry, WorkflowRegistryError> {
    let entry = stored.and_then(WorkflowRegistrationEntry::from_node_properties);
    let mut registry = WorkflowRegistry::default();
    if let Some(entry) = &entry {
        registry.register(entry.identity.clone())?;
    }
    registry.read(identity)?;
    let entry = entry.ok_or(WorkflowRegistryError::IdentityUnknown)?;
    if entry.owner_scope != caller_scope {
        return Err(WorkflowRegistryError::Unauthorized);
    }
    Ok(entry)
}

/// One workflow registry operation (`Method::WorkflowRegistry`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum WorkflowRegistryOp {
    /// Store one registration; registering the same record again is a no-op.
    Register {
        registration: WorkflowRegistrationEntry,
    },
    /// Read one registration by exact identity.
    Read { identity: WorkflowIdentity },
}

impl WorkflowRegistryOp {
    /// Whether this operation changes durable state.
    pub fn is_mutation(&self) -> bool {
        matches!(self, Self::Register { .. })
    }
}

/// Deterministic, serializable registration records with exact-identity lookup.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct WorkflowRegistry {
    registrations: Vec<WorkflowRegistration>,
}

impl WorkflowRegistry {
    pub fn register(
        &mut self,
        registration: WorkflowRegistration,
    ) -> Result<WorkflowRegistration, WorkflowRegistryError> {
        registration.validate()?;
        if !self.registrations.contains(&registration) {
            self.registrations.push(registration.clone());
            self.registrations.sort();
        }
        Ok(registration)
    }

    pub fn read(
        &self,
        identity: &WorkflowIdentity,
    ) -> Result<&WorkflowRegistration, WorkflowRegistryError> {
        identity.validate()?;
        self.registrations
            .iter()
            .find(|registration| *registration == identity)
            .ok_or(WorkflowRegistryError::IdentityUnknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> WorkflowIdentity {
        WorkflowIdentity {
            workflow_id: "summarize".into(),
            version: "1.2.0".into(),
            content_digest: format!("sha256:{}", "a".repeat(64)),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R166.1
    #[test]
    fn registration_round_trips_and_reads_by_exact_identity() {
        let expected = identity();
        let mut registry = WorkflowRegistry::default();
        assert_eq!(registry.register(expected.clone()), Ok(expected.clone()));
        let encoded = serde_json::to_string(&registry).unwrap();
        let restored: WorkflowRegistry = serde_json::from_str(&encoded).unwrap();
        let read = restored.read(&expected).unwrap();
        assert_eq!(read.workflow_id, expected.workflow_id);
        assert_eq!(read.version, expected.version);
        assert_eq!(read.content_digest, expected.content_digest);
    }

    // spec: MEDULLA-DECISION-ENGINE-R166.1
    #[test]
    fn invalid_or_unknown_identity_is_typed_refusal() {
        let mut registry = WorkflowRegistry::default();
        let mut invalid = identity();
        invalid.workflow_id.clear();
        assert_eq!(
            registry.register(invalid),
            Err(WorkflowRegistryError::WorkflowIdMissing)
        );
        let mut invalid = identity();
        invalid.version.clear();
        assert_eq!(
            registry.register(invalid),
            Err(WorkflowRegistryError::VersionMissing)
        );
        let mut invalid = identity();
        invalid.content_digest.clear();
        assert_eq!(
            registry.register(invalid),
            Err(WorkflowRegistryError::ContentDigestInvalid)
        );
        assert_eq!(
            registry.read(&identity()),
            Err(WorkflowRegistryError::IdentityUnknown)
        );
    }
}
