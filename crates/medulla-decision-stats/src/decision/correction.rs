//! Pure correction kernels (MEDULLA-DECISION-ENGINE-R702.2).
//!
//! No I/O and no `MutationBatch`: [`commit_correction`] turns one correction
//! into the plan of its correction node, its three effect nodes and the edges
//! between them, and [`nearest_corrections`] picks the nearest prior corrections
//! of one tenant. The handler (R702.3) turns the plan into the write.

use medulla_decision_types::correction::{
    Correction, CorrectionCommit, CorrectionEdge, CorrectionEffect, CorrectionNode,
    CorrectionOutcome, CorrectionShapeError, EvalCase, OutcomeTag, RuleCandidate,
};
use medulla_types::contract::Digest256;
use serde_json::{json, Map, Value};

/// The class of the correction node.
const CORRECTION_CLASS: &str = "Correction";

fn node(id: &str, class: &'static str, entries: Vec<(&str, Value)>) -> CorrectionNode {
    let mut properties = Map::new();
    for (key, value) in entries {
        properties.insert(key.to_string(), value);
    }
    CorrectionNode {
        id: id.to_string(),
        class,
        properties,
    }
}

/// Plan the commit of one correction: validate it, derive the receipt id from
/// its serialized bytes, and derive every node and edge from the correction and
/// the receipt id alone.
pub fn commit_correction(correction: &Correction) -> Result<CorrectionCommit, CorrectionShapeError> {
    correction.validate()?;
    let bytes = serde_json::to_vec(correction).expect("a correction of plain fields serializes");
    let receipt_id = Digest256::sha256(&bytes).to_hex();
    let effect = CorrectionEffect {
        receipt_id: receipt_id.clone(),
        rule_candidate: RuleCandidate {
            rule_id: format!("rule:{receipt_id}"),
            text_digest: Digest256::sha256(correction.after.as_bytes()).to_hex(),
        },
        outcome: CorrectionOutcome {
            outcome_id: format!("outcome:{receipt_id}"),
            tag: OutcomeTag::Independent,
            value: 0.0,
        },
        eval_case: EvalCase {
            case_id: format!("case:{receipt_id}"),
            input_digest: Digest256::sha256(correction.before.as_bytes()).to_hex(),
            expected_digest: Digest256::sha256(correction.after.as_bytes()).to_hex(),
        },
    };
    let correction_node = format!("correction:{receipt_id}");
    let nodes = vec![
        node(
            &correction_node,
            CORRECTION_CLASS,
            vec![
                ("correction_id", json!(correction.correction_id)),
                ("target", json!(correction.target)),
                ("before", json!(correction.before)),
                ("after", json!(correction.after)),
                ("produced_by", json!(correction.produced_by)),
                ("actor", json!(correction.actor)),
                ("at_ms", json!(correction.at_ms)),
                ("receipt_id", json!(receipt_id)),
            ],
        ),
        node(
            &effect.rule_candidate.rule_id,
            "RuleCandidate",
            vec![
                ("rule_id", json!(effect.rule_candidate.rule_id)),
                ("text_digest", json!(effect.rule_candidate.text_digest)),
                ("receipt_id", json!(receipt_id)),
            ],
        ),
        node(
            &effect.outcome.outcome_id,
            "CorrectionOutcome",
            vec![
                ("outcome_id", json!(effect.outcome.outcome_id)),
                ("tag", json!("independent")),
                ("value", json!(effect.outcome.value)),
                ("receipt_id", json!(receipt_id)),
            ],
        ),
        node(
            &effect.eval_case.case_id,
            "EvalCase",
            vec![
                ("case_id", json!(effect.eval_case.case_id)),
                ("input_digest", json!(effect.eval_case.input_digest)),
                ("expected_digest", json!(effect.eval_case.expected_digest)),
                ("receipt_id", json!(receipt_id)),
            ],
        ),
    ];
    let edge = |relationship: &'static str, target: &str| CorrectionEdge {
        source: correction_node.clone(),
        target: target.to_string(),
        relationship,
    };
    let edges = vec![
        edge("yields_rule", &effect.rule_candidate.rule_id),
        edge("yields_outcome", &effect.outcome.outcome_id),
        edge("yields_case", &effect.eval_case.case_id),
    ];
    Ok(CorrectionCommit {
        effect,
        nodes,
        edges,
    })
}

/// The ids of the nearest prior corrections of `tenant`: the neighbours whose
/// `(class, tenant)` is `("Correction", tenant)`, by score descending then node
/// id ascending, at most `k` of them (`correction.nearest_k`).
pub fn nearest_corrections(
    neighbours: &[(String, f32)],
    class_and_tenant_of: impl Fn(&str) -> Option<(String, String)>,
    tenant: &str,
    k: usize,
) -> Vec<String> {
    let mut kept: Vec<&(String, f32)> = neighbours
        .iter()
        .filter(|(id, _)| {
            class_and_tenant_of(id)
                .is_some_and(|(class, owner)| class == CORRECTION_CLASS && owner == tenant)
        })
        .collect();
    kept.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    kept.into_iter().take(k).map(|(id, _)| id.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn correction(after: &str) -> Correction {
        Correction {
            correction_id: "c1".into(),
            target: "answer:7".into(),
            before: "wrong".into(),
            after: after.into(),
            produced_by: "agent:planner".into(),
            actor: "user:ada".into(),
            at_ms: 1_700_000_000_000,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R702.2
    #[test]
    fn batch_holds_all_three_effects_and_k_is_bounded() {
        let commit = commit_correction(&correction("right")).unwrap();
        let receipt = &commit.effect.receipt_id;
        assert_eq!(receipt.len(), 64);
        assert!(receipt
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        let ids: Vec<&str> = commit.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                format!("correction:{receipt}"),
                format!("rule:{receipt}"),
                format!("outcome:{receipt}"),
                format!("case:{receipt}"),
            ]
        );
        let classes: Vec<&str> = commit.nodes.iter().map(|n| n.class).collect();
        assert_eq!(
            classes,
            ["Correction", "RuleCandidate", "CorrectionOutcome", "EvalCase"]
        );
        let relationships: Vec<&str> = commit.edges.iter().map(|e| e.relationship).collect();
        assert_eq!(relationships, ["yields_rule", "yields_outcome", "yields_case"]);
        assert!(commit
            .edges
            .iter()
            .all(|edge| edge.source == format!("correction:{receipt}")));
        assert_eq!(commit.nodes[2].properties["tag"], "independent");
        assert_eq!(commit.nodes[2].properties["value"], 0.0);
        assert_eq!(commit.nodes[3].properties["input_digest"], Digest256::sha256(b"wrong").to_hex());

        // Equal input gives an equal receipt; a changed `after` gives another.
        assert_eq!(commit_correction(&correction("right")).unwrap(), commit);
        assert_ne!(
            commit_correction(&correction("righter")).unwrap().effect.receipt_id,
            *receipt
        );
        // The 8 KiB bound is the R702.1 shape error.
        let big = "x".repeat(8 * 1024 + 1);
        assert!(commit_correction(&correction(&big)).is_err());

        // Nearest: another tenant's node and another class drop out; at most
        // `k` remain, by score then id.
        let neighbours: Vec<(String, f32)> = vec![
            ("b".into(), 0.5),
            ("a".into(), 0.5),
            ("c".into(), 0.9),
            ("other".into(), 0.99),
            ("note".into(), 0.8),
            ("d".into(), 0.1),
        ];
        let lookup = |id: &str| match id {
            "other" => Some(("Correction".to_string(), "tenant-b".to_string())),
            "note" => Some(("Note".to_string(), "tenant-a".to_string())),
            "missing" => None,
            _ => Some(("Correction".to_string(), "tenant-a".to_string())),
        };
        assert_eq!(
            nearest_corrections(&neighbours, lookup, "tenant-a", 8),
            ["c", "a", "b", "d"]
        );
        assert_eq!(nearest_corrections(&neighbours, lookup, "tenant-a", 2), ["c", "a"]);
        let knob = &medulla_decision_types::correction::KNOBS[0];
        assert!(knob.admits(&knob.default));
    }
}
