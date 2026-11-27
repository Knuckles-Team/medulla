//! Policy rule candidates synthesized on repository index (MEDULLA-ORCH-VOCAB-R012.7.2).
//!
//! The files `classify_spec_files` typed `SpecPolicy` are read with
//! `parse_policy_document` and each clause becomes one `PolicyCandidate` node, `proposed`
//! or `inactive`. This activates nothing: no stored candidate is a `GuardrailRule`, and
//! indexing never writes a `ProposeRule`. A reviewer, or arbora from these nodes, makes
//! that call with the node's `rule_id`, `scope` and its `clause_digest`.

use std::collections::BTreeSet;

use medulla_agent_types::policy_document::{
    parse_policy_document, PolicyCandidateStatus, PolicyInactiveReason, PolicyProfile,
    PolicySeverity, PolicySourceRef,
};
use sha2::{Digest, Sha256};

/// The path of the constitution file, read as a `Constitution` document.
const CONSTITUTION_PATH: &str = ".specify/memory/constitution.md";

/// The id of the node of one candidate of `repo`.
pub fn policy_candidate_node_id(repo: &str, key: &str) -> String {
    format!("policycandidate:{repo}:{key}")
}

/// One `upsert_node` operation per clause of every policy file. `policy_paths` are the
/// paths `classify_spec_files` typed `SpecPolicy`; a path that repeats is read once and a
/// file that is not UTF-8 is skipped. A path ending `.rules.md` is a `RuleSet` document,
/// every other policy file is a `Constitution`.
pub fn synthesize_policy_candidates(
    repo: &str,
    commit: &str,
    files: &[(String, Vec<u8>)],
    policy_paths: &[String],
) -> Vec<serde_json::Value> {
    let mut seen = BTreeSet::new();
    let mut operations = Vec::new();
    for path in policy_paths {
        if !seen.insert(path.as_str()) {
            continue;
        }
        let Some((_, bytes)) = files.iter().find(|(name, _)| name == path) else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(bytes) else {
            continue;
        };
        let profile = if path.ends_with(".rules.md") && path != CONSTITUTION_PATH {
            PolicyProfile::RuleSet
        } else {
            PolicyProfile::Constitution
        };
        let source = PolicySourceRef {
            repository: repo.to_owned(),
            path: path.clone(),
            commit: commit.to_owned(),
            blob_digest: hex::encode(Sha256::digest(text.as_bytes())),
            start_line: 0,
            end_line: 0,
        };
        let Ok(candidates) = parse_policy_document(bytes, profile, &source) else {
            continue;
        };
        for candidate in candidates {
            let (status, reason) = match candidate.status {
                PolicyCandidateStatus::Proposed => ("proposed", None),
                PolicyCandidateStatus::Inactive { reason } => (
                    "inactive",
                    Some(match reason {
                        PolicyInactiveReason::ClauseUnparsed => "CLAUSE_UNPARSED",
                        PolicyInactiveReason::ClauseAmbiguous => "CLAUSE_AMBIGUOUS",
                    }),
                ),
            };
            let clause_digest = candidate.clause_digest.clone();
            let key = candidate
                .rule
                .as_ref()
                .map(|rule| rule.rule_id.clone())
                .unwrap_or_else(|| clause_digest.clone());
            let mut properties = serde_json::json!({
                "type": "PolicyCandidate",
                "repo": repo,
                "path": path,
                "status": status,
                "clause_digest": clause_digest,
                "candidate_set_digest": candidate.candidate_set_digest,
                "commit": commit,
                "blob_digest": candidate.source_ref.blob_digest,
                "start_line": candidate.source_ref.start_line,
                "end_line": candidate.source_ref.end_line,
                "grammar_revision": candidate.grammar_revision,
            });
            if let Some(reason) = reason {
                properties["reason"] = reason.into();
            }
            if let Some(rule) = &candidate.rule {
                properties["rule_id"] = rule.rule_id.clone().into();
                properties["scope"] = rule.scope.clone().into();
                properties["severity"] = match rule.severity {
                    PolicySeverity::Info => "info",
                    PolicySeverity::Warn => "warn",
                    PolicySeverity::Block => "block",
                }
                .into();
            }
            operations.push(serde_json::json!({
                "op": "upsert_node",
                "id": policy_candidate_node_id(repo, &key),
                "properties": properties,
            }));
        }
    }
    operations
}

/// The index-result node of one candidate operation; every scalar property is carried as
/// its decimal or text form.
pub fn candidate_index_node(
    op: &serde_json::Value,
) -> medulla_ingestion_types::ingestion_wire::ExtractedNode {
    let properties: std::collections::HashMap<String, String> = op["properties"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| {
            let text = match value {
                serde_json::Value::String(text) => text.clone(),
                serde_json::Value::Number(number) => number.to_string(),
                _ => return None,
            };
            Some((key.clone(), text))
        })
        .collect();
    medulla_ingestion_types::ingestion_wire::ExtractedNode {
        node_id: op["id"].as_str().unwrap_or_default().to_owned(),
        node_type: properties.get("type").cloned().unwrap_or_default(),
        properties,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-ORCH-VOCAB-R012.7.2
    #[test]
    fn index_yields_inactive_candidates_with_spans() {
        let document = "# Constitution\n\n- MUST NOT commit secrets {scope=https://example.invalid/class/Repo}\n- SHOULD review changes\n- a prose item\n";
        let files = vec![
            (
                ".specify/memory/constitution.md".to_owned(),
                document.as_bytes().to_vec(),
            ),
            ("bad.md".to_owned(), vec![0xff, 0xfe]),
        ];
        let paths = vec![
            ".specify/memory/constitution.md".to_owned(),
            ".specify/memory/constitution.md".to_owned(),
            "bad.md".to_owned(),
        ];
        let ops = synthesize_policy_candidates("demo", "c0ffee", &files, &paths);
        assert_eq!(ops.len(), 2);
        assert!(ops
            .iter()
            .all(|op| op["properties"]["type"] == "PolicyCandidate"));
        let proposed = &ops[0]["properties"];
        assert_eq!(proposed["status"], "proposed");
        assert_eq!(proposed["start_line"], 3);
        assert_eq!(proposed["severity"], "block");
        assert_eq!(proposed["grammar_revision"], 1);
        assert!(ops[0]["id"]
            .as_str()
            .unwrap()
            .starts_with("policycandidate:demo:policy-"));
        let inactive = &ops[1]["properties"];
        assert_eq!(inactive["status"], "inactive");
        assert_eq!(inactive["reason"], "CLAUSE_UNPARSED");
        assert_eq!(inactive["start_line"], 4);
        let stripped = Sha256::digest("review changes".as_bytes());
        assert_eq!(inactive["clause_digest"], hex::encode(stripped));
        assert_eq!(
            inactive["candidate_set_digest"],
            proposed["candidate_set_digest"]
        );
        let node = candidate_index_node(&ops[0]);
        assert_eq!(node.node_type, "PolicyCandidate");
        assert_eq!(node.properties["start_line"], "3");
    }
}
