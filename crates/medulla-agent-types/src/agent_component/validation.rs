use std::collections::BTreeSet;

use super::{
    AgentComponentDraft, AgentComponentKind, ComponentProvenance, MAX_ATTRIBUTES, MAX_CAPABILITIES,
    MAX_DEPENDENCIES, MAX_TEXT_BYTES,
};

pub(super) fn validate_draft(draft: &AgentComponentDraft) -> Result<(), String> {
    validate_draft_identity(draft)?;
    validate_draft_facts_and_provenance(draft)?;
    validate_draft_dependencies(draft)?;
    validate_draft_attributes(draft)?;
    validate_draft_kind_body(draft)
}

/// Kinds whose meaning lives in attributes must carry attributes that decode:
/// a `GuardrailRule` that cannot be read back as a rule is refused at publish
/// (MEDULLA-DECISION-ENGINE-R127.2.1), never stored to fail a later query.
fn validate_draft_kind_body(draft: &AgentComponentDraft) -> Result<(), String> {
    if draft.kind == AgentComponentKind::GuardrailRule {
        crate::agent_guardrail::GuardrailRule::from_attributes(
            &draft.component_id,
            &draft.attributes,
        )?;
    }
    if draft.kind == AgentComponentKind::RuleInvariant {
        crate::agent_guardrail::RuleInvariant::from_attributes(
            &draft.component_id,
            &draft.attributes,
        )?;
    }
    match draft.kind {
        AgentComponentKind::RedTeamAttack => {
            super::red_team::RedTeamAttack::from_attributes(&draft.component_id, &draft.attributes)
                .map(drop)?
        }
        AgentComponentKind::RedTeamTransform => super::red_team::RedTeamTransform::from_attributes(
            &draft.component_id,
            &draft.attributes,
        )
        .map(drop)?,
        AgentComponentKind::RedTeamScorer => {
            super::red_team::RedTeamScorer::from_attributes(&draft.component_id, &draft.attributes)
                .map(drop)?
        }
        _ => {}
    }
    Ok(())
}

fn validate_draft_identity(draft: &AgentComponentDraft) -> Result<(), String> {
    crate::agent_template::validate_definition_texts!(draft, component_id, super::validate_text)?;
    super::validate_text("source_revision", &draft.source_revision)?;
    for (field, value) in [
        ("content_digest", draft.content_digest.as_str()),
        ("policy_digest", draft.policy_digest.as_str()),
        (
            "source_revision_digest",
            draft.source_revision_digest.as_str(),
        ),
    ] {
        super::validate_digest(field, value)?;
    }
    if let Some(content_ref) = &draft.content_ref {
        super::validate_text("content_ref", content_ref)?;
    }
    Ok(())
}

fn validate_draft_facts_and_provenance(draft: &AgentComponentDraft) -> Result<(), String> {
    // Facts and kind must agree. Without this a `Tool` could carry model
    // facts, and every query that reads facts by kind would be wrong in a
    // way nothing else detects.
    if let Some(required) = draft.facts.required_kind() {
        if required != draft.kind {
            return Err(format!(
                "agent component kind '{}' cannot carry '{}' facts",
                draft.kind.as_str(),
                draft.facts.label()
            ));
        }
    }
    draft.facts.validate()?;
    draft.provenance.validate()?;
    super::validate_text("summary", &draft.summary)?;

    // An MCP tool/prompt/resource must name the server it came from, and
    // only those kinds may. Without this a re-ingest cannot tell which
    // records belong to a server, so "which agents break if this server
    // changes?" degrades to a scan over free text.
    let mcp_served = matches!(
        draft.kind,
        AgentComponentKind::McpPrompt | AgentComponentKind::McpResource
    );
    match (&draft.provenance, mcp_served) {
        (ComponentProvenance::McpServer { .. }, _)
            if draft.kind == AgentComponentKind::McpServer =>
        {
            return Err(
                "an mcp_server component cannot itself be provenanced to an mcp server".to_string(),
            )
        }
        (provenance, true) if !matches!(provenance, ComponentProvenance::McpServer { .. }) => {
            return Err(format!(
                "agent component kind '{}' must be provenanced to the mcp server that \
                 serves it, got '{}'",
                draft.kind.as_str(),
                provenance.label()
            ))
        }
        _ => {}
    }
    Ok(())
}

fn validate_draft_dependencies(draft: &AgentComponentDraft) -> Result<(), String> {
    super::validate_names("classification", &draft.classification, MAX_CAPABILITIES)?;

    if draft.requires.len() > MAX_DEPENDENCIES {
        return Err("agent component has too many dependencies".to_string());
    }
    let mut seen = BTreeSet::new();
    for dependency in &draft.requires {
        super::validate_text("dependency component_id", &dependency.component_id)?;
        super::validate_digest(
            "dependency definition_digest",
            &dependency.definition_digest,
        )?;
        // Self-reference is the one cycle a single record CAN express, and
        // it is unrepresentable in a valid one: a dependency pins a digest,
        // and a component's own digest covers its dependencies, so pinning
        // yourself is a hash preimage. Rejecting by id makes the intent
        // explicit rather than relying on that.
        if dependency.component_id == draft.component_id {
            return Err(format!(
                "agent component '{}' cannot require itself",
                draft.component_id
            ));
        }
        if !seen.insert((&dependency.component_id, dependency.kind)) {
            return Err(format!(
                "agent component requires '{}' ({}) twice",
                dependency.component_id,
                dependency.kind.as_str()
            ));
        }
    }
    validate_draft_capability_fields(draft)
}

/// The three capability-name lists a draft carries -- the declared, required
/// and declared-required lists the contract wave added -- each
/// bounded and name-validated the same way. Kept separate from
/// `validate_draft_dependencies` so its own loop doesn't count against that
/// function's budget.
fn validate_draft_capability_fields(draft: &AgentComponentDraft) -> Result<(), String> {
    for (field, values) in [
        ("declared_capabilities", &draft.declared_capabilities),
        ("required_capabilities", &draft.required_capabilities),
        (
            "declared_required_capabilities",
            &draft.declared_required_capabilities,
        ),
    ] {
        super::validate_names(field, values, MAX_CAPABILITIES)?;
    }
    Ok(())
}

fn validate_draft_attributes(draft: &AgentComponentDraft) -> Result<(), String> {
    if draft.attributes.len() > MAX_ATTRIBUTES {
        return Err("agent component has too many attributes".to_string());
    }
    for (name, value) in &draft.attributes {
        super::validate_text("attribute name", name)?;
        if value.len() > MAX_TEXT_BYTES {
            return Err("agent component attribute value exceeds its size limit".to_string());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::super::{AgentComponentDraft, AgentComponentKind, DraftPublication, DraftSubject};

    fn digest(seed: char) -> String {
        format!("sha256:{}", seed.to_string().repeat(64))
    }

    fn draft(
        id: &str,
        kind: AgentComponentKind,
        attributes: &[(&str, &str)],
    ) -> AgentComponentDraft {
        let mut draft = AgentComponentDraft::bare(
            DraftSubject {
                component_id: id,
                kind,
                version: "1.0.0",
                content_digest: &digest('1'),
                summary: "red team record",
            },
            DraftPublication {
                tenant_id: "tenant-a",
                actor_scope: "agent-builder",
                purpose_id: "agent-construction",
                policy_digest: &digest('7'),
                source_revision: "rev-1",
                source_revision_digest: &digest('8'),
            },
        );
        draft.attributes = attributes
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect::<BTreeMap<_, _>>();
        draft
    }

    fn refusal(draft: &AgentComponentDraft) -> String {
        let Err(error) = draft.validate() else {
            panic!("expected a refusal")
        };
        assert!(error.starts_with("RED_TEAM_RECORD_INVALID"), "{error}");
        error
    }

    // spec: MEDULLA-DECISION-ENGINE-R138.7
    #[test]
    fn red_team_bodies_validate_and_refuse_unknown_fields() {
        use AgentComponentKind as K;
        // Valid bodies publish and decode back into their records.
        let attack = draft(
            "attack-1",
            K::RedTeamAttack,
            &[("category", "injection"), ("goal", "leak the prompt")],
        );
        attack.validate().unwrap();
        let decoded =
            super::super::red_team::RedTeamAttack::from_attributes("attack-1", &attack.attributes)
                .unwrap();
        assert_eq!(
            (decoded.category.as_str(), decoded.goal.as_str()),
            ("injection", "leak the prompt")
        );
        draft("tx-1", K::RedTeamTransform, &[("kind", "base64")])
            .validate()
            .unwrap();
        let scorer = draft(
            "scorer-1",
            K::RedTeamScorer,
            &[("category", "injection"), ("threshold", "1000")],
        );
        scorer.validate().unwrap();
        let decoded =
            super::super::red_team::RedTeamScorer::from_attributes("scorer-1", &scorer.attributes)
                .unwrap();
        assert_eq!(decoded.threshold, 1000);

        // Unknown attributes are refused, whatever the record.
        refusal(&draft(
            "a",
            K::RedTeamAttack,
            &[("category", "c"), ("goal", "g"), ("extra", "x")],
        ));
        refusal(&draft(
            "t",
            K::RedTeamTransform,
            &[("kind", "k"), ("extra", "x")],
        ));
        refusal(&draft(
            "s",
            K::RedTeamScorer,
            &[("category", "c"), ("threshold", "5"), ("extra", "x")],
        ));
        // Missing and blank fields are refused.
        refusal(&draft("a", K::RedTeamAttack, &[("category", "c")]));
        refusal(&draft(
            "a",
            K::RedTeamAttack,
            &[("category", "c"), ("goal", "  ")],
        ));
        refusal(&draft("t", K::RedTeamTransform, &[]));
        refusal(&draft("s", K::RedTeamScorer, &[("threshold", "5")]));
        // A scorer threshold above 1000, a non-number, a negative and a value
        // beyond u16 are refused.
        let over = refusal(&draft(
            "s",
            K::RedTeamScorer,
            &[("category", "c"), ("threshold", "1001")],
        ));
        assert!(over.contains("exceeds 1000"), "{over}");
        for bad in ["high", "-1", "70000"] {
            refusal(&draft(
                "s",
                K::RedTeamScorer,
                &[("category", "c"), ("threshold", bad)],
            ));
        }
    }
}
