//! What a prompt assignment and a workflow pattern add to an assembly: the definitions the
//! request names, read from the request graph behind the ordinary read ACL
//! (MEDULLA-ORCH-VOCAB-R800.3). The patterns are read and validated here and returned in no
//! result field; each assigned prompt is rendered after the solve, with the pure functions of
//! `medulla_work_types::orchestration::definitions`. A request that names neither reads nothing.

use medulla_decision_types::decision::request::PromptAssignment;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_types::contract::Digest256;
use medulla_work_types::orchestration::definitions::{
    DefinitionRefusal, PatternPrecondition, PromptTemplate, TemplateSlot, WorkflowPattern,
};
use medulla_work_types::orchestration::workflows::WorkflowIdentity;
use serde_json::{Map, Value};

use super::{topology_read, SharedState};

/// The node id of a stored prompt template.
const TEMPLATE_NODE_PREFIX: &str = "prompttemplate:";

/// The most bytes a template body holds.
pub(super) const MAX_BODY_BYTES: usize = 16384;

/// The definitions an assembly request names, read and validated.
pub(super) struct ReadDefinitions {
    /// Each template with its body, in the order of the assignments.
    pub templates: Vec<(PromptTemplate, String)>,
}

fn invalid(detail: impl std::fmt::Display) -> String {
    format!("ASSEMBLY_INPUTS_INVALID: definitions: {detail}")
}

fn node_properties(
    core: &medulla_graph_core::graph::GraphCore,
    tenant: &str,
    id: &str,
) -> Result<Map<String, Value>, String> {
    let not_found = || invalid(format!("{id} not found"));
    let bytes = core.get_node_properties(id).ok_or_else(not_found)?;
    let properties = medulla_types::msgpack::decode_property_object(&bytes)
        .map_err(|_| invalid(format!("{id} is unreadable")))?;
    if properties
        .get("tenant_id")
        .and_then(Value::as_str)
        .is_some_and(|owner| owner != tenant)
    {
        return Err(not_found());
    }
    Ok(properties)
}

fn text<'a>(properties: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    properties.get(name).and_then(Value::as_str)
}

/// A property stored as a JSON value or as the JSON text of one.
fn json_property(properties: &Map<String, Value>, name: &str) -> Option<Value> {
    match properties.get(name)? {
        Value::String(encoded) => serde_json::from_str(encoded).ok(),
        other => Some(other.clone()),
    }
}

fn sha256_text(body: &str) -> String {
    Digest256::sha256(body.as_bytes()).to_hex()
}

/// The template the node `prompttemplate:<template_id>` holds, with its body.
fn read_template(
    core: &medulla_graph_core::graph::GraphCore,
    tenant: &str,
    template_id: &str,
) -> Result<(PromptTemplate, String), String> {
    let id = format!("{TEMPLATE_NODE_PREFIX}{template_id}");
    let properties = node_properties(core, tenant, &id)?;
    let body = text(&properties, "body").unwrap_or_default().to_string();
    let digest = text(&properties, "digest").unwrap_or_default().to_string();
    if body.len() > MAX_BODY_BYTES
        || digest.strip_prefix("sha256:").unwrap_or(&digest) != sha256_text(&body)
    {
        return Err(invalid(format!("{id} digest mismatch")));
    }
    let slots: Vec<TemplateSlot> = json_property(&properties, "slots")
        .and_then(|value| serde_json::from_value(value).ok())
        .ok_or_else(|| invalid(format!("{id} slots invalid")))?;
    Ok((
        PromptTemplate {
            id: template_id.to_string(),
            revision_ref: text(&properties, "revision_ref")
                .unwrap_or_default()
                .to_string(),
            slots,
            digest,
        },
        body,
    ))
}

/// The pattern the node `id` holds. Its `steps` must all be steps of its stored canonical
/// workflow.
fn read_pattern(
    core: &medulla_graph_core::graph::GraphCore,
    tenant: &str,
    id: &str,
) -> Result<WorkflowPattern, String> {
    let properties = node_properties(core, tenant, id)?;
    let digest = text(&properties, "workflowDigest").unwrap_or_default();
    // The compile writer stores `sha256:<64 hex>` and nothing else; any other form is refused
    // (hard cutover, one digest format).
    if !medulla_types::process_import::is_stored_workflow_digest(digest) {
        return Err(invalid(format!("{id} digest malformed")));
    }
    let content_digest = digest.to_string();
    let version = properties
        .get("version")
        .map(|value| match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
        .unwrap_or_default();
    let steps: Vec<String> = json_property(&properties, "steps")
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    let preconditions: Vec<PatternPrecondition> = match json_property(&properties, "preconditions")
    {
        None => Vec::new(),
        Some(value) => serde_json::from_value(value)
            .map_err(|_| invalid(format!("{id} preconditions invalid")))?,
    };
    if let Some(canonical) = json_property(&properties, "canonical") {
        let known: Vec<&str> = canonical
            .get("steps")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|step| step.get("id").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(unknown) = steps.iter().find(|step| !known.contains(&step.as_str())) {
            return Err(format!(
                "{}: {id} names step {unknown}, which its workflow does not register",
                DefinitionRefusal::PatternStepUnknown.as_str()
            ));
        }
    }
    Ok(WorkflowPattern {
        workflow: WorkflowIdentity {
            workflow_id: text(&properties, "process_iri")
                .unwrap_or_default()
                .to_string(),
            version: version.clone(),
            content_digest,
        },
        steps,
        preconditions,
        version: version.parse().unwrap_or_default(),
        process_revision: text(&properties, "processRevision")
            .unwrap_or_default()
            .to_string(),
    })
}

/// Read the patterns and the templates a request names. Reads nothing when both lists are
/// empty.
pub(super) async fn read_definitions(
    state: &SharedState,
    graph: &str,
    verified: &VerifiedRequestContext,
    patterns: &[String],
    prompts: &[PromptAssignment],
) -> Result<ReadDefinitions, String> {
    if patterns.is_empty() && prompts.is_empty() {
        return Ok(ReadDefinitions {
            templates: Vec::new(),
        });
    }
    let core = topology_read::graph_core(state, graph, verified).await?;
    let tenant = verified.tenant();
    // A named pattern is read to prove it exists and is well formed; assembly consumes
    // only the templates, so the validated patterns are not kept.
    for id in patterns {
        read_pattern(&core, tenant, id)?;
    }
    let templates = prompts
        .iter()
        .map(|assignment| read_template(&core, tenant, &assignment.template_id))
        .collect::<Result<Vec<_>, String>>()?;
    Ok(ReadDefinitions { templates })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core_with(nodes: &[(&str, Value)]) -> medulla_graph_core::graph::GraphCore {
        let core = medulla_graph_core::graph::GraphCore::new();
        for (id, properties) in nodes {
            let encoded = rmp_serde::to_vec_named(properties).expect("properties encode");
            core.add_node((*id).to_string(), encoded);
        }
        core
    }

    fn workflow_node(steps: &[&str]) -> Value {
        serde_json::json!({
            "type": "WorkflowPattern",
            "tenant_id": "t1",
            "process_iri": "process:invoice",
            "version": 1,
            "steps": steps,
            "workflowDigest": format!("sha256:{}", "ab".repeat(32)),
            "canonical": serde_json::json!({"steps": [{"id": "s1"}, {"id": "s2"}]}).to_string(),
            "preconditions": "[]",
        })
    }

    // spec: MEDULLA-ORCH-VOCAB-R800.3
    #[test]
    fn pattern_with_unknown_step_refuses() {
        let core = core_with(&[
            ("workflow:process:invoice:v1", workflow_node(&["s1", "s2"])),
            ("workflow:process:invoice:v2", workflow_node(&["s1", "s9"])),
        ]);
        let pattern = read_pattern(&core, "t1", "workflow:process:invoice:v1").unwrap();
        assert_eq!(pattern.steps, ["s1", "s2"]);
        assert_eq!(pattern.version, 1);
        assert!(pattern.workflow.content_digest.starts_with("sha256:"));
        let refused = read_pattern(&core, "t1", "workflow:process:invoice:v2").unwrap_err();
        assert!(refused.starts_with("PATTERN_STEP_UNKNOWN"), "{refused}");
        // Another tenant's pattern and an absent one read as not found.
        let foreign = read_pattern(&core, "t2", "workflow:process:invoice:v1").unwrap_err();
        assert!(foreign.contains("not found"), "{foreign}");
        let missing = read_pattern(&core, "t1", "workflow:nope:v1").unwrap_err();
        assert!(missing.starts_with("ASSEMBLY_INPUTS_INVALID: definitions:"));
    }
}
