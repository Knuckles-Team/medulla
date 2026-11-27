//! DMN document to graph mapping (MEDULLA-CQ-ORCH-R702.5).
//!
//! One pure function turns a parsed DMN document into the graph writes the served
//! `ProcessDocumentImport` handler commits as one batch. Each element becomes one node of its
//! `dmn-v1.ttl` class; the rules of a table keep their declared order in `dmnRuleIndex`.

use std::collections::BTreeMap;

use super::dmn_read::{
    DmnClause, DmnDecision, DmnDocument, DmnInputData, DmnRequirement, DmnRequirementKind, DmnRule,
    DmnTable,
};
use super::PreservedXml;
use crate::architecture_exchange::graph::ExchangeMutation;
use medulla_types::process_exchange::ProcessExchangeError;

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
    index: usize,
) -> ExchangeMutation {
    node(
        label,
        element_iri(model_iri, &clause.id),
        BTreeMap::from([
            ("dmnTable".to_owned(), element_iri(model_iri, table_id)),
            ("name".to_owned(), clause.label.clone()),
            ("dmnExpression".to_owned(), clause.expression.clone()),
            ("dmnTypeRef".to_owned(), clause.type_ref.clone()),
            ("dmnClauseIndex".to_owned(), index.to_string()),
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
        for (index, input) in table.inputs.iter().enumerate() {
            push_node(
                &input.id,
                clause_node("DmnInputClause", model_iri, &table.id, input, index),
            );
        }
        for (index, output) in table.outputs.iter().enumerate() {
            push_node(
                &output.id,
                clause_node("DmnOutputClause", model_iri, &table.id, output, index),
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
    // The root's prefixed namespaces stay on the requirements node, so a written document
    // keeps its preserved fragments well formed.
    // A kept fragment sits on its owner node; one whose owner is not a node stays on the root.
    let mut kept: BTreeMap<String, String> = BTreeMap::new();
    for fragment in &doc.preserved {
        let owner = if nodes.iter().any(|(id, _)| *id == fragment.owner) {
            fragment.owner.as_str()
        } else {
            doc.id.as_str()
        };
        kept.entry(owner.to_owned())
            .or_default()
            .push_str(&fragment.xml);
    }
    for (id, mutation) in &mut nodes {
        if let ExchangeMutation::UpsertNode { properties, .. } = mutation {
            if let Some(xml) = kept.get(id.as_str()) {
                properties.insert("dmnPreservedXml".to_owned(), xml.clone());
            }
            if *id == doc.id {
                for (prefix, iri) in &doc.namespaces {
                    properties.insert(format!("dmnNamespace:{prefix}"), iri.clone());
                }
            }
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

/// The element id of a node id of the model `model_iri`, or `None` for a foreign node.
fn element_of<'a>(model_iri: &str, node_id: &'a str) -> Option<&'a str> {
    node_id
        .strip_prefix(model_iri)
        .and_then(|rest| rest.strip_prefix('#'))
}

/// The positional entries `<prefix><n>` of a rule node, in position order.
fn entries(properties: &BTreeMap<String, String>, prefix: &str) -> Vec<String> {
    let mut found: Vec<(usize, String)> = properties
        .iter()
        .filter_map(|(key, value)| {
            let position = key.strip_prefix(prefix)?.parse::<usize>().ok()?;
            Some((position, value.clone()))
        })
        .collect();
    found.sort_by_key(|(position, _)| *position);
    found.into_iter().map(|(_, value)| value).collect()
}

fn index_of(properties: &BTreeMap<String, String>, key: &str) -> usize {
    properties
        .get(key)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(usize::MAX)
}

/// The inverse of [`dmn_mutations`]: rebuild the DMN document of the model `model_iri` from the
/// node and edge writes that stored it (MEDULLA-CQ-ORCH-R702.11). A model without one
/// `DmnDecisionRequirements` node refuses `NotDeployable`. Clauses come back in their
/// `dmnClauseIndex` order and rules in their `dmnRuleIndex` order.
pub fn dmn_document(
    mutations: &[ExchangeMutation],
    model_iri: &str,
) -> Result<DmnDocument, ProcessExchangeError> {
    let mut nodes: BTreeMap<&str, (&str, &BTreeMap<String, String>)> = BTreeMap::new();
    let mut edges: Vec<(&str, &str, &str)> = Vec::new();
    for mutation in mutations {
        match mutation {
            ExchangeMutation::UpsertNode {
                id,
                label,
                properties,
            } => {
                if let Some(element) = element_of(model_iri, id) {
                    nodes.insert(element, (label.as_str(), properties));
                }
            }
            ExchangeMutation::Link {
                source,
                target,
                predicate,
            } => {
                if let (Some(from), Some(to)) =
                    (element_of(model_iri, source), element_of(model_iri, target))
                {
                    edges.push((from, predicate.as_str(), to));
                }
            }
        }
    }
    let mut roots = nodes
        .iter()
        .filter(|(_, (label, _))| *label == "DmnDecisionRequirements");
    let (Some((root_id, (_, root))), None) = (roots.next(), roots.next()) else {
        return Err(ProcessExchangeError::NotDeployable {
            reason: "the model has no single DmnDecisionRequirements node".into(),
        });
    };
    let name_of =
        |properties: &BTreeMap<String, String>| properties.get("name").cloned().unwrap_or_default();
    let mut doc = DmnDocument {
        id: (*root_id).to_owned(),
        name: name_of(root),
        namespaces: root
            .iter()
            .filter_map(|(key, value)| {
                key.strip_prefix("dmnNamespace:")
                    .map(|prefix| (prefix.to_owned(), value.clone()))
            })
            .collect(),
        ..DmnDocument::default()
    };
    for (id, (_, properties)) in &nodes {
        if let Some(xml) = properties.get("dmnPreservedXml") {
            doc.preserved.push(PreservedXml {
                owner: (*id).to_owned(),
                xml: xml.clone(),
            });
        }
    }
    let clauses = |label: &str, table_iri: &str| -> Vec<DmnClause> {
        let mut found: Vec<(usize, DmnClause)> = nodes
            .iter()
            .filter(|(_, (l, p))| {
                *l == label && p.get("dmnTable").map(String::as_str) == Some(table_iri)
            })
            .map(|(id, (_, p))| {
                (
                    index_of(p, "dmnClauseIndex"),
                    DmnClause {
                        id: (*id).to_owned(),
                        label: name_of(p),
                        expression: p.get("dmnExpression").cloned().unwrap_or_default(),
                        type_ref: p.get("dmnTypeRef").cloned().unwrap_or_default(),
                    },
                )
            })
            .collect();
        found.sort_by_key(|(index, _)| *index);
        found.into_iter().map(|(_, clause)| clause).collect()
    };
    for (id, (label, properties)) in &nodes {
        match *label {
            "DmnInputData" => doc.input_data.push(DmnInputData {
                id: (*id).to_owned(),
                name: name_of(properties),
            }),
            "DmnDecision" => {
                let decision_iri = element_iri(model_iri, id);
                let table = nodes
                    .iter()
                    .find(|(_, (l, p))| {
                        *l == "DmnDecisionTable"
                            && p.get("dmnDecision").map(String::as_str)
                                == Some(decision_iri.as_str())
                    })
                    .map(|(table_id, (_, p))| {
                        let table_iri = element_iri(model_iri, table_id);
                        let mut rules: Vec<(usize, DmnRule)> = nodes
                            .iter()
                            .filter(|(_, (l, rp))| {
                                *l == "DmnRule"
                                    && rp.get("dmnTable").map(String::as_str)
                                        == Some(table_iri.as_str())
                            })
                            .map(|(rule_id, (_, rp))| {
                                (
                                    index_of(rp, "dmnRuleIndex"),
                                    DmnRule {
                                        id: (*rule_id).to_owned(),
                                        input_entries: entries(rp, "dmnInputEntry:"),
                                        output_entries: entries(rp, "dmnOutputEntry:"),
                                    },
                                )
                            })
                            .collect();
                        rules.sort_by_key(|(index, _)| *index);
                        DmnTable {
                            id: (*table_id).to_owned(),
                            hit_policy: p.get("dmnHitPolicy").cloned().unwrap_or_default(),
                            aggregation: p.get("dmnAggregation").cloned(),
                            inputs: clauses("DmnInputClause", &table_iri),
                            outputs: clauses("DmnOutputClause", &table_iri),
                            rules: rules.into_iter().map(|(_, rule)| rule).collect(),
                        }
                    });
                let mut requirements: Vec<DmnRequirement> = edges
                    .iter()
                    .filter(|(from, _, _)| from == id)
                    .filter_map(|(_, predicate, to)| {
                        let kind = match *predicate {
                            "dmnRequiresDecision" => DmnRequirementKind::Decision,
                            "dmnRequiresInput" => DmnRequirementKind::Input,
                            _ => return None,
                        };
                        Some(DmnRequirement {
                            kind,
                            target: (*to).to_owned(),
                        })
                    })
                    .collect();
                requirements.sort_by(|a, b| a.target.cmp(&b.target));
                doc.decisions.push(DmnDecision {
                    id: (*id).to_owned(),
                    name: name_of(properties),
                    table,
                    requirements,
                });
            }
            _ => {}
        }
    }
    Ok(doc)
}
