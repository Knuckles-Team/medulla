//! The two other things an assembly reads from the library besides its
//! candidates: the graph templates a request names, and a pinned decision
//! policy. Each is read by (id, exact revision digest), in the tenant, both at
//! assembly time and again inside the commit's write.

use medulla_agent_types::agent_graph::AgentGraphEntry;
use medulla_agent_types::delegation::AgentGraphEntryRef;
use medulla_agent_types::{
    agent_component::AgentComponentEntry, agent_component::AgentComponentKind,
    agent_component::ComponentDependency,
};
use medulla_decision_types::decision::policy::policy_from_attributes;
use medulla_decision_types::decision::statistical::retrieval_pointer::PointerState;
use medulla_decision_types::{
    decision::DecisionErrorCode, decision::DecisionPolicy, decision::DecisionRecord,
    decision::TemplateFacts,
};

use super::super::agent_component::ComponentLayer;
use super::super::agent_graph::GraphLayer;
use super::super::agent_library::AgentLibraryStore;
use super::super::agent_revision::decode_revision;
use super::super::decision_jobs::decode_artifact;
use super::refuse;

type Write<'a> = medulla_transaction::AdmittedMutation<'a, medulla_storage::AgentLibraryOwner>;

/// Most revisions of one record the in-write lookups below will walk.
const MAX_REVISION_SCAN: usize = 16_384;

fn template_of(entry: AgentGraphEntry) -> TemplateFacts {
    TemplateFacts {
        graph_id: entry.graph_id,
        entry_revision: entry.entry_revision,
        definition_digest: entry.definition_digest,
        shape: entry.shape,
        topology: entry.topology,
    }
}

/// The policy a `DecisionPolicy` component revision carries.
pub(super) fn policy_of(entry: &AgentComponentEntry) -> Result<DecisionPolicy, String> {
    if entry.kind != AgentComponentKind::DecisionPolicy {
        return Err(refuse(
            DecisionErrorCode::PolicyBodyUnavailable,
            format!("'{}' is not a decision policy", entry.component_id),
        ));
    }
    policy_from_attributes(&entry.attributes, &entry.content_digest).map_err(|code| {
        refuse(
            code,
            format!("policy '{}' does not verify", entry.component_id),
        )
    })
}

impl AgentLibraryStore {
    /// The pinned reference of the newest published revision of the graph template
    /// `template_key` in `tenant_id` (MEDULLA-PLACEMENT-R001.12). A key with no published
    /// revision refuses `ASSEMBLY_INPUTS_INVALID`; a retired or withdrawn revision is never
    /// chosen, and no other tenant's template is ever read.
    pub fn assembly_template_ref(
        &self,
        tenant_id: &str,
        template_key: &str,
    ) -> Result<AgentGraphEntryRef, String> {
        self.graph_revisions(tenant_id, template_key)?
            .into_iter()
            .filter(|entry| {
                entry.lifecycle
                    == medulla_agent_types::agent_library::AgentLibraryLifecycle::Published
            })
            .max_by_key(|entry| entry.entry_revision)
            .map(|entry| AgentGraphEntryRef::from_entry(&entry))
            .ok_or_else(|| {
                refuse(
                    DecisionErrorCode::AssemblyInputsInvalid,
                    format!("template_key '{template_key}' has no published revision"),
                )
            })
    }

    /// A request's graph template, at exactly the revision it names.
    pub fn assembly_template(
        &self,
        tenant_id: &str,
        reference: &AgentGraphEntryRef,
    ) -> Result<TemplateFacts, String> {
        if reference.tenant_id != tenant_id {
            return Err("ACCESS_DENIED: the template belongs to another tenant".to_string());
        }
        self.graph_revisions(tenant_id, &reference.graph_id)?
            .into_iter()
            .find(|entry| {
                entry.entry_revision == reference.entry_revision
                    && entry.definition_digest == reference.definition_digest
            })
            .map(template_of)
            .ok_or_else(|| {
                refuse(
                    DecisionErrorCode::AssemblyInputsInvalid,
                    format!("template '{}' has no such revision", reference.graph_id),
                )
            })
    }

    /// A pinned `DecisionPolicy` component's verified body.
    pub fn pinned_decision_policy(
        &self,
        tenant_id: &str,
        pin: &ComponentDependency,
    ) -> Result<DecisionPolicy, String> {
        let entry = self
            .component_revisions(tenant_id, &pin.component_id)?
            .into_iter()
            .find(|entry| entry.definition_digest == pin.definition_digest)
            .ok_or_else(|| {
                refuse(
                    DecisionErrorCode::PolicyBodyUnavailable,
                    format!("policy '{}' has no such revision", pin.component_id),
                )
            })?;
        policy_of(&entry)
    }

    /// The content digest the `champion:knob_set:<policy_id>` pointer names
    /// now, or `None` while nothing has been promoted
    /// (MEDULLA-DECISION-ENGINE-R179.3).
    pub fn champion_policy_target(
        &self,
        tenant_id: &str,
        policy_id: &str,
    ) -> Result<Option<String>, String> {
        let key = format!("pointer:champion:knob_set:{policy_id}");
        let Some(bytes) = self.decision_artifact(tenant_id, &key)? else {
            return Ok(None);
        };
        let state: PointerState = decode_artifact(&bytes, "pointer state")?;
        Ok(state.target().map(str::to_owned))
    }

    /// The verified body of the policy revision the champion pointer of
    /// `policy_id` names; the engine default while nothing is promoted.
    pub fn champion_decision_policy(
        &self,
        tenant_id: &str,
        policy_id: &str,
    ) -> Result<DecisionPolicy, String> {
        let Some(target) = self.champion_policy_target(tenant_id, policy_id)? else {
            return Ok(DecisionPolicy::engine_default());
        };
        let entry = self
            .component_revisions(tenant_id, policy_id)?
            .into_iter()
            .find(|entry| entry.content_digest == target)
            .ok_or_else(|| {
                refuse(
                    DecisionErrorCode::PolicyBodyUnavailable,
                    format!("the champion of policy '{policy_id}' has no such revision"),
                )
            })?;
        policy_of(&entry)
    }
}

/// Inside the commit's write: every template the record stores is still the
/// published revision it names, and a pinned policy is still the published
/// body the record stores.
pub(super) fn check_sources(txn: &Write<'_>, record: &DecisionRecord) -> Result<(), String> {
    let tenant_id = record.tenant_id.as_str();
    let graphs = txn.open_read_table(medulla_storage::AGENT_GRAPH_REVISIONS)?;
    for template in &record.inputs.templates {
        let key = (
            tenant_id,
            template.graph_id.as_str(),
            template.entry_revision,
        );
        let row = graphs.get(key)?.ok_or_else(|| {
            refuse(
                DecisionErrorCode::StaleCatalog,
                format!("template '{}' is gone", template.graph_id),
            )
        })?;
        let published = template_of(decode_revision::<GraphLayer>(row.value())?);
        if &published != template {
            return Err(refuse(
                DecisionErrorCode::StaleCatalog,
                format!(
                    "template '{}' differs from its published revision",
                    template.graph_id
                ),
            ));
        }
    }
    let medulla_agent_types::agent_component::DecisionPolicyRef::Pinned { component } =
        &record.inputs.request.policy
    else {
        return Ok(());
    };
    let published = pinned_in_write(txn, tenant_id, component)?;
    if published != record.inputs.policy {
        return Err(refuse(
            DecisionErrorCode::StaleCatalog,
            "the stored policy is not the pinned revision's body",
        ));
    }
    Ok(())
}

fn pinned_in_write(
    txn: &Write<'_>,
    tenant_id: &str,
    pin: &ComponentDependency,
) -> Result<DecisionPolicy, String> {
    let revisions = txn.open_read_table(medulla_storage::AGENT_COMPONENT_REVISIONS)?;
    for (scanned, row) in revisions
        .range_from((tenant_id, pin.component_id.as_str(), 0))?
        .enumerate()
    {
        let (key, value) = row.map_err(|error| error.to_string())?;
        let (row_tenant, row_id, _) = key.value();
        if row_tenant != tenant_id || row_id != pin.component_id || scanned >= MAX_REVISION_SCAN {
            break;
        }
        let entry = decode_revision::<ComponentLayer>(value.value())?;
        if entry.definition_digest == pin.definition_digest {
            return policy_of(&entry);
        }
    }
    Err(refuse(
        DecisionErrorCode::PolicyBodyUnavailable,
        format!("policy '{}' has no such revision", pin.component_id),
    ))
}
