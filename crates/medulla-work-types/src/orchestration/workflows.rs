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
}

/// Most steps one registration may declare.
pub const MAX_WORKFLOW_STEPS: usize = 256;
/// Most assets one step may name.
pub const MAX_STEP_ASSETS: usize = 64;

/// One step of the registered definition, declared for the metadata catalog
/// (MEDULLA-TYPED-PACKS-R108.6). The step names the assets that serve it, by absolute
/// IRI: the agent versions and tools it runs. The definition body stays addressed by its
/// digest; a step is a catalog descriptor, not a copy of the body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct WorkflowStep {
    /// The step id, unique within the definition.
    pub step_id: String,
    /// Absolute IRIs of the agent version and tool assets that serve this step.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub served_by: Vec<String>,
}

/// Most skills one registration may declare.
pub const MAX_WORKFLOW_SKILLS: usize = 256;
/// The only `kind` of a declared skill, the wire name of the skill component kind.
pub const WORKFLOW_SKILL_KIND: &str = "skill";

/// One skill the workflow runs, pinned by the definition digest of its revision
/// (MEDULLA-DECISION-ENGINE-R166.7). The three fields are those of a
/// `ComponentDependency` of kind `skill`, so a caller reads a registration's skills and
/// an agent library entry's skills the same way.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct WorkflowSkill {
    pub component_id: String,
    /// Always `skill`.
    pub kind: String,
    pub definition_digest: String,
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
    /// The steps of the definition, declared for the catalog (R108.6). Not stored on the
    /// registration node: the catalog emitter reads them when the registration commits.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<WorkflowStep>,
    /// The skills the workflow runs, stored on the registration node and answered by
    /// `Read` (R166.7). A run's attestation gate reads them from here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<WorkflowSkill>,
}

impl WorkflowRegistrationEntry {
    /// The declared skills are bounded, of kind `skill`, named and unique.
    pub fn validate_skills(&self) -> Result<(), String> {
        let invalid = |detail: String| format!("INVALID_ARGUMENT: workflow skills: {detail}");
        if self.skills.len() > MAX_WORKFLOW_SKILLS {
            return Err(invalid(format!("at most {MAX_WORKFLOW_SKILLS} skills")));
        }
        let mut seen = std::collections::BTreeSet::new();
        for skill in &self.skills {
            if skill.kind != WORKFLOW_SKILL_KIND {
                return Err(invalid(format!(
                    "skill {} has kind {}, not {WORKFLOW_SKILL_KIND}",
                    skill.component_id, skill.kind
                )));
            }
            if skill.component_id.trim().is_empty() || skill.definition_digest.trim().is_empty() {
                return Err(invalid("a skill names a component and a digest".to_string()));
            }
            if !seen.insert(skill.component_id.as_str()) {
                return Err(invalid(format!(
                    "skill {} is declared twice",
                    skill.component_id
                )));
            }
        }
        Ok(())
    }

    /// The declared steps are bounded, named and unique; each served-by reference is
    /// non-empty. IRI shape is checked by the catalog emitter, which owns the links.
    pub fn validate_steps(&self) -> Result<(), String> {
        let invalid = |detail: String| format!("INVALID_ARGUMENT: workflow steps: {detail}");
        if self.steps.len() > MAX_WORKFLOW_STEPS {
            return Err(invalid(format!("at most {MAX_WORKFLOW_STEPS} steps")));
        }
        let mut seen = std::collections::BTreeSet::new();
        for step in &self.steps {
            if step.step_id.trim().is_empty() {
                return Err(invalid("a step id is empty".to_string()));
            }
            if !seen.insert(step.step_id.as_str()) {
                return Err(invalid(format!("step {} is declared twice", step.step_id)));
            }
            if step.served_by.len() > MAX_STEP_ASSETS
                || step.served_by.iter().any(|asset| asset.trim().is_empty())
            {
                return Err(invalid(format!(
                    "step {} names at most {MAX_STEP_ASSETS} non-empty assets",
                    step.step_id
                )));
            }
        }
        Ok(())
    }

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
            "skills": self.skills,
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
            lifecycle: super::workflow_machine::WORKFLOW_MACHINE
                .state_from_stored(&text("lifecycle")?)?,
            steps: Vec::new(),
            skills: match properties.get("skills") {
                Some(value) => serde_json::from_value(value.clone()).ok()?,
                None => Vec::new(),
            },
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
