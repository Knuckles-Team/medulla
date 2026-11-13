//! The canonical prompt blueprint (MEDULLA-DECISION-ENGINE-R814.8).
//!
//! A packaged prompt is a JSON document with a fixed shape. The check returns
//! every violation as a field path and a rule name, never the field's value,
//! so a refusal cannot echo a pack's own content.

use std::collections::BTreeSet;

use serde_json::Value;

/// The only blueprint schema version.
pub const BLUEPRINT_SCHEMA_VERSION: &str = "1.0";

/// One broken rule at one field path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlueprintViolation {
    pub path: String,
    pub rule: &'static str,
}

fn violation(path: &str, rule: &'static str) -> BlueprintViolation {
    BlueprintViolation {
        path: path.to_string(),
        rule,
    }
}

fn is_slug(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 128
        && !text.starts_with('-')
        && !text.ends_with('-')
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Check one prompt document. `existing` holds the URIs of the prompts a
/// blueprint may `extends`.
pub fn validate_prompt_blueprint(
    doc: &Value,
    existing: &BTreeSet<String>,
) -> Vec<BlueprintViolation> {
    let Some(object) = doc.as_object() else {
        return vec![violation("$", "must_be_object")];
    };
    let mut found = Vec::new();
    if object.get("schema_version").and_then(Value::as_str) != Some(BLUEPRINT_SCHEMA_VERSION) {
        found.push(violation("schema_version", "must_be_1_0"));
    }
    if !object
        .get("task")
        .and_then(Value::as_str)
        .is_some_and(is_slug)
    {
        found.push(violation("task", "must_be_slug"));
    }
    if object.get("type").and_then(Value::as_str) != Some("prompt") {
        found.push(violation("type", "must_be_prompt"));
    }
    if !object
        .get("source")
        .and_then(Value::as_str)
        .is_some_and(|source| !source.trim().is_empty())
    {
        found.push(violation("source", "must_name_provenance"));
    }
    for top_level in ["content", "input"] {
        if object.contains_key(top_level) {
            found.push(violation(top_level, "body_only_in_core_directive"));
        }
    }
    if !object
        .get("instructions")
        .and_then(|instructions| instructions.get("core_directive"))
        .and_then(Value::as_str)
        .is_some_and(|body| !body.trim().is_empty())
    {
        found.push(violation(
            "instructions.core_directive",
            "must_hold_the_body",
        ));
    }
    if let Some(extends) = object.get("extends") {
        if !extends.as_str().is_some_and(|uri| existing.contains(uri)) {
            found.push(violation("extends", "must_name_an_existing_prompt"));
        }
    }
    if let Some(compose) = object.get("compose") {
        if !matches!(
            compose.as_str(),
            Some("append") | Some("prepend") | Some("replace")
        ) {
            found.push(violation("compose", "must_be_append_prepend_or_replace"));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn conformant() -> Value {
        json!({
            "schema_version": "1.0",
            "task": "incident-triage",
            "type": "prompt",
            "source": "service-desk-connector",
            "instructions": {"core_directive": "Triage the incident."},
        })
    }

    // spec: MEDULLA-DECISION-ENGINE-R814.8
    #[test]
    fn conformant_passes_and_each_rule_names_its_path_only() {
        let existing = BTreeSet::from(["prompt://base".to_string()]);
        assert!(validate_prompt_blueprint(&conformant(), &existing).is_empty());

        let mut extended = conformant();
        extended["extends"] = json!("prompt://base");
        extended["compose"] = json!("append");
        assert!(validate_prompt_blueprint(&extended, &existing).is_empty());

        let legacy = json!({
            "task": "Not A Slug",
            "type": "template",
            "content": "secret body text",
            "input": "more",
            "extends": "prompt://missing",
            "compose": "merge",
        });
        let found = validate_prompt_blueprint(&legacy, &existing);
        let paths: Vec<&str> = found.iter().map(|v| v.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "schema_version",
                "task",
                "type",
                "source",
                "content",
                "input",
                "instructions.core_directive",
                "extends",
                "compose",
            ]
        );
        assert!(found
            .iter()
            .all(|v| !v.rule.contains("secret") && !v.path.contains("secret")));
        assert_eq!(
            validate_prompt_blueprint(&json!([1]), &existing),
            vec![violation("$", "must_be_object")]
        );
    }
}
