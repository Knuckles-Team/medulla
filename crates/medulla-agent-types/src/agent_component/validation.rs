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
        AgentComponentKind::Playbook => validate_playbook_attributes(draft)?,
        AgentComponentKind::A2aAgentCard => validate_a2a_card_attributes(draft)?,
        _ => {}
    }
    Ok(())
}

/// Whether `value` is an absolute IRI the projection can store: `<scheme>:<rest>` with an ASCII
/// letter then letters, digits, `+`, `-` or `.` as the scheme, a non-empty rest, no whitespace
/// and none of `<`, `>`, `"`, `{`, `}`, `\`, `^`, `|` or a backtick.
fn is_absolute_iri(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty()
        || value
            .chars()
            .any(|c| c.is_whitespace() || "<>\"{}\\^|`".contains(c))
    {
        return false;
    }
    let Some((scheme, rest)) = value.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !rest.is_empty()
}

/// An `A2aAgentCard`'s optional `agent_kind` attribute and every declared capability are absolute
/// IRIs, so the projection can store them as graph terms (MEDULLA-ORCH-VOCAB-R003.6).
fn validate_a2a_card_attributes(draft: &AgentComponentDraft) -> Result<(), String> {
    let id = &draft.component_id;
    if let Some(kind) = draft.attributes.get(super::A2A_AGENT_KIND_ATTRIBUTE) {
        if !is_absolute_iri(kind) {
            return Err(format!(
                "MALFORMED_A2A_AGENT_CARD: a2a agent card '{id}' agent_kind '{kind}' is not an absolute IRI"
            ));
        }
    }
    for capability in &draft.declared_capabilities {
        if !is_absolute_iri(capability) {
            return Err(format!(
                "MALFORMED_A2A_AGENT_CARD: a2a agent card '{id}' capability '{capability}' is not an absolute IRI"
            ));
        }
    }
    Ok(())
}

/// A `Playbook` names the trigger it answers from the closed set, and a
/// `ticket_class` trigger names the class (MEDULLA-DECISION-ENGINE-R704.1).
fn validate_playbook_attributes(draft: &AgentComponentDraft) -> Result<(), String> {
    use super::{PLAYBOOK_TICKET_CLASS_ATTRIBUTE, PLAYBOOK_TRIGGERS, PLAYBOOK_TRIGGER_ATTRIBUTE};
    let attribute = |name: &str| {
        draft
            .attributes
            .get(name)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
    };
    let id = &draft.component_id;
    let Some(trigger) = attribute(PLAYBOOK_TRIGGER_ATTRIBUTE) else {
        return Err(format!(
            "MALFORMED_PLAYBOOK: playbook '{id}' needs a non-empty '{PLAYBOOK_TRIGGER_ATTRIBUTE}' attribute"
        ));
    };
    if !PLAYBOOK_TRIGGERS.contains(&trigger) {
        return Err(format!(
            "MALFORMED_PLAYBOOK: playbook '{id}' trigger '{trigger}' is not one of {PLAYBOOK_TRIGGERS:?}"
        ));
    }
    validate_playbook_steps(id, attribute(super::PLAYBOOK_STEPS_ATTRIBUTE))?;
    let class = attribute(PLAYBOOK_TICKET_CLASS_ATTRIBUTE);
    match (trigger == "ticket_class", class) {
        (true, None) => Err(format!(
            "MALFORMED_PLAYBOOK: playbook '{id}' with trigger 'ticket_class' needs a non-empty \
             '{PLAYBOOK_TICKET_CLASS_ATTRIBUTE}' attribute"
        )),
        (false, Some(_)) => Err(format!(
            "MALFORMED_PLAYBOOK: playbook '{id}' carries '{PLAYBOOK_TICKET_CLASS_ATTRIBUTE}' \
             but its trigger is '{trigger}'"
        )),
        _ => Ok(()),
    }
}

/// The `steps` attribute of a `Playbook`, when present, is a JSON array of at
/// most `PLAYBOOK_MAX_STEPS` objects with a non-empty `step_id` and
/// `action_ref` and a boolean `writes`, and nothing else.
fn validate_playbook_steps(id: &str, steps: Option<&str>) -> Result<(), String> {
    let Some(text) = steps else { return Ok(()) };
    let bad = |why: &str| {
        Err(format!(
            "MALFORMED_PLAYBOOK: playbook '{id}' attribute '{}' {why}",
            super::PLAYBOOK_STEPS_ATTRIBUTE
        ))
    };
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(text)
    else {
        return bad("is not a JSON array");
    };
    if items.len() > super::PLAYBOOK_MAX_STEPS {
        return bad("holds more steps than a playbook may propose");
    }
    for item in &items {
        let serde_json::Value::Object(fields) = item else {
            return bad("holds a step that is not an object");
        };
        let text_of = |name: &str| {
            fields
                .get(name)
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
        };
        let writes = fields
            .get("writes")
            .is_some_and(serde_json::Value::is_boolean);
        if fields.len() != 3 || !text_of("step_id") || !text_of("action_ref") || !writes {
            return bad("holds a step without step_id, action_ref and writes");
        }
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

    // spec: MEDULLA-ORCH-VOCAB-R003.6
    #[test]
    fn a2a_card_with_non_iri_kind_is_refused() {
        use AgentComponentKind as K;
        let kind = "https://knuckles-team.github.io/ontology/core#agentKindVerifier";
        draft("card-ok", K::A2aAgentCard, &[("agent_kind", kind)])
            .validate()
            .unwrap();
        draft("card-no-kind", K::A2aAgentCard, &[])
            .validate()
            .unwrap();
        for bad in [
            "",
            " ",
            "verifier",
            "has space:x",
            "a:",
            "1a:b",
            "a:b<c",
            "a:b\"c",
            "a:b`c",
        ] {
            let error = draft("card-bad", K::A2aAgentCard, &[("agent_kind", bad)])
                .validate()
                .unwrap_err();
            assert!(
                error.starts_with("MALFORMED_A2A_AGENT_CARD"),
                "{bad}: {error}"
            );
        }
        let mut capability = draft("card-cap", K::A2aAgentCard, &[]);
        capability.declared_capabilities = vec!["not an iri".to_string()];
        let error = capability.validate().unwrap_err();
        assert!(error.starts_with("MALFORMED_A2A_AGENT_CARD"), "{error}");
        assert!(error.contains("not an iri"), "{error}");
    }
}
