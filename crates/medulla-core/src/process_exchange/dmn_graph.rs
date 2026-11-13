//! DMN document to graph mapping (MEDULLA-CQ-ORCH-R702.5).
//!
//! One pure function turns a parsed DMN document into the graph writes the served
//! `ProcessDocumentImport` handler commits as one batch. Each element becomes one node of its
//! `dmn-v1.ttl` class; the rules of a table keep their declared order in `dmnRuleIndex`.

use std::collections::BTreeMap;

use super::dmn_read::{DmnClause, DmnDocument, DmnRequirementKind};
use crate::architecture_exchange::graph::ExchangeMutation;

fn node(label: &str, id: String, mut properties: BTreeMap<String, String>) -> ExchangeMutation {
    properties.insert("type".to_owned(), label.to_owned());
    ExchangeMutation::UpsertNode {
        id,
        label: label.to_owned(),
        properties,
    }
}

fn clause_node(
    label: &str,
    model_iri: &str,
    table_id: &str,
    clause: &DmnClause,
) -> ExchangeMutation {
    node(
        label,
        element_iri(model_iri, &clause.id),
        BTreeMap::from([
            ("dmnTable".to_owned(), element_iri(model_iri, table_id)),
            ("name".to_owned(), clause.label.clone()),
            ("dmnExpression".to_owned(), clause.expression.clone()),
            ("dmnTypeRef".to_owned(), clause.type_ref.clone()),
        ]),
    )
}

/// The node id of one element of the model `model_iri`.
pub fn element_iri(model_iri: &str, element_id: &str) -> String {
    format!("{model_iri}#{element_id}")
}

/// Map a DMN document onto graph writes. The decision requirements graph is one node of class
/// `DmnDecisionRequirements`; each decision, table, input clause, output clause, rule and input
/// data element is one node of its class. A requirement becomes a `dmnRequiresDecision` or
/// `dmnRequiresInput` edge from the requiring decision. Rules carry their zero-based declared
/// position in `dmnRuleIndex`. The list is ordered by element id (nodes first, then edges), so
/// equal input gives equal output. Nothing is written here.
pub fn dmn_mutations(doc: &DmnDocument, model_iri: &str) -> Vec<ExchangeMutation> {
    let mut nodes: Vec<(String, ExchangeMutation)> = Vec::new();
    let mut edges: Vec<(String, ExchangeMutation)> = Vec::new();
    let mut push_node = |id: &str, mutation: ExchangeMutation| {
        nodes.push((id.to_owned(), mutation));
    };

    push_node(
        &doc.id,
        node(
            "DmnDecisionRequirements",
            element_iri(model_iri, &doc.id),
            BTreeMap::from([("name".to_owned(), doc.name.clone())]),
        ),
    );
    for input in &doc.input_data {
        push_node(
            &input.id,
            node(
                "DmnInputData",
                element_iri(model_iri, &input.id),
                BTreeMap::from([("name".to_owned(), input.name.clone())]),
            ),
        );
    }
    for decision in &doc.decisions {
        let decision_iri = element_iri(model_iri, &decision.id);
        push_node(
            &decision.id,
            node(
                "DmnDecision",
                decision_iri.clone(),
                BTreeMap::from([
                    ("name".to_owned(), decision.name.clone()),
                    ("dmnModel".to_owned(), element_iri(model_iri, &doc.id)),
                ]),
            ),
        );
        for requirement in &decision.requirements {
            let predicate = match requirement.kind {
                DmnRequirementKind::Decision => "dmnRequiresDecision",
                DmnRequirementKind::Input => "dmnRequiresInput",
            };
            edges.push((
                format!("{}\u{0}{predicate}\u{0}{}", decision.id, requirement.target),
                ExchangeMutation::Link {
                    source: decision_iri.clone(),
                    target: element_iri(model_iri, &requirement.target),
                    predicate: predicate.to_owned(),
                },
            ));
        }
        let Some(table) = &decision.table else {
            continue;
        };
        let mut properties = BTreeMap::from([
            ("dmnDecision".to_owned(), decision_iri.clone()),
            ("dmnHitPolicy".to_owned(), table.hit_policy.clone()),
        ]);
        if let Some(aggregation) = &table.aggregation {
            properties.insert("dmnAggregation".to_owned(), aggregation.clone());
        }
        push_node(
            &table.id,
            node(
                "DmnDecisionTable",
                element_iri(model_iri, &table.id),
                properties,
            ),
        );
        for input in &table.inputs {
            push_node(
                &input.id,
                clause_node("DmnInputClause", model_iri, &table.id, input),
            );
        }
        for output in &table.outputs {
            push_node(
                &output.id,
                clause_node("DmnOutputClause", model_iri, &table.id, output),
            );
        }
        for (index, rule) in table.rules.iter().enumerate() {
            let mut properties = BTreeMap::from([
                ("dmnTable".to_owned(), element_iri(model_iri, &table.id)),
                ("dmnRuleIndex".to_owned(), index.to_string()),
            ]);
            for (position, entry) in rule.input_entries.iter().enumerate() {
                properties.insert(format!("dmnInputEntry:{position}"), entry.clone());
            }
            for (position, entry) in rule.output_entries.iter().enumerate() {
                properties.insert(format!("dmnOutputEntry:{position}"), entry.clone());
            }
            push_node(
                &rule.id,
                node("DmnRule", element_iri(model_iri, &rule.id), properties),
            );
        }
    }
    nodes.sort_by(|a, b| a.0.cmp(&b.0));
    edges.sort_by(|a, b| a.0.cmp(&b.0));
    nodes
        .into_iter()
        .chain(edges)
        .map(|(_, mutation)| mutation)
        .collect()
}
