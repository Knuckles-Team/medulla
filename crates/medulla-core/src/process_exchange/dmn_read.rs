//! DMN 1.3 reader (MEDULLA-CQ-ORCH-R702.4): the same streaming and entity
//! rules as the BPMN reader. The reader keeps expression text verbatim and
//! evaluates nothing.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::xml_tree::{parse_tree, XmlElement, XmlLimits};
use super::{PreservedXml, ProcessExchangeError};

/// The DMN 1.3 model namespace; the root element must be in it.
pub const DMN_MODEL_NS: &str = "https://www.omg.org/spec/DMN/20191111/MODEL/";

/// A parsed DMN 1.3 document.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmnDocument {
    pub id: String,
    pub name: String,
    pub decisions: Vec<DmnDecision>,
    pub input_data: Vec<DmnInputData>,
    /// Elements the reader does not model, with the owner element id.
    pub preserved: Vec<PreservedXml>,
    pub namespaces: BTreeMap<String, String>,
}

/// One decision with its table and requirements.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmnDecision {
    pub id: String,
    pub name: String,
    pub table: Option<DmnTable>,
    pub requirements: Vec<DmnRequirement>,
}

/// One decision table.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmnTable {
    pub id: String,
    /// `UNIQUE` when the document states none, as DMN 1.3 defines.
    pub hit_policy: String,
    pub aggregation: Option<String>,
    pub inputs: Vec<DmnClause>,
    pub outputs: Vec<DmnClause>,
    pub rules: Vec<DmnRule>,
}

/// One input or output clause.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmnClause {
    pub id: String,
    pub label: String,
    /// The input expression text, or the output name.
    pub expression: String,
    pub type_ref: String,
}

/// One rule; entries keep their expression text verbatim.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmnRule {
    pub id: String,
    pub input_entries: Vec<String>,
    pub output_entries: Vec<String>,
}

/// One input data element.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmnInputData {
    pub id: String,
    pub name: String,
}

/// What a requirement points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DmnRequirementKind {
    Decision,
    Input,
}

/// One information requirement of a decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DmnRequirement {
    pub kind: DmnRequirementKind,
    /// The referenced element id, without the leading `#`.
    pub target: String,
}

fn id_of(element: &XmlElement) -> String {
    element.attr("id").unwrap_or_default().to_owned()
}

fn text_of(element: &XmlElement, child: &str) -> String {
    element
        .child(child)
        .and_then(|c| c.child("text"))
        .map(|t| t.text.clone())
        .unwrap_or_default()
}

fn clause(element: &XmlElement, expression: String) -> DmnClause {
    DmnClause {
        id: id_of(element),
        label: element.attr("label").unwrap_or_default().to_owned(),
        expression,
        type_ref: element.attr("typeRef").unwrap_or_default().to_owned(),
    }
}

fn table(element: &XmlElement) -> DmnTable {
    DmnTable {
        id: id_of(element),
        hit_policy: element.attr("hitPolicy").unwrap_or("UNIQUE").to_owned(),
        aggregation: element.attr("aggregation").map(str::to_owned),
        inputs: element
            .children_named("input")
            .map(|input| clause(input, text_of(input, "inputExpression")))
            .collect(),
        outputs: element
            .children_named("output")
            .map(|output| clause(output, output.attr("name").unwrap_or_default().to_owned()))
            .collect(),
        rules: element
            .children_named("rule")
            .map(|rule| DmnRule {
                id: id_of(rule),
                input_entries: rule
                    .children_named("inputEntry")
                    .map(|e| text_of_entry(e))
                    .collect(),
                output_entries: rule
                    .children_named("outputEntry")
                    .map(|e| text_of_entry(e))
                    .collect(),
            })
            .collect(),
    }
}

fn text_of_entry(entry: &XmlElement) -> String {
    entry
        .child("text")
        .map(|t| t.text.clone())
        .unwrap_or_default()
}

/// Parse one DMN 1.3 document. A DTD or an undeclared entity refuses
/// `EntityRefused`; a root outside the DMN model namespace refuses
/// `NamespaceUnknown`; elements the reader does not model keep their XML.
pub fn parse_dmn(bytes: &[u8], limits: XmlLimits) -> Result<DmnDocument, ProcessExchangeError> {
    let (root, namespaces) = parse_tree(bytes, limits)?;
    if root.ns != DMN_MODEL_NS || root.local != "definitions" {
        return Err(ProcessExchangeError::NamespaceUnknown { namespace: root.ns });
    }
    let mut doc = DmnDocument {
        id: id_of(&root),
        name: root.attr("name").unwrap_or_default().to_owned(),
        namespaces: namespaces.clone(),
        ..DmnDocument::default()
    };
    for child in &root.children {
        match child.local.as_str() {
            "decision" if child.ns == DMN_MODEL_NS => {
                let mut decision = DmnDecision {
                    id: id_of(child),
                    name: child.attr("name").unwrap_or_default().to_owned(),
                    ..DmnDecision::default()
                };
                for part in &child.children {
                    match part.local.as_str() {
                        "decisionTable" => decision.table = Some(table(part)),
                        "informationRequirement" => {
                            for reference in &part.children {
                                let kind = match reference.local.as_str() {
                                    "requiredDecision" => DmnRequirementKind::Decision,
                                    "requiredInput" => DmnRequirementKind::Input,
                                    _ => continue,
                                };
                                decision.requirements.push(DmnRequirement {
                                    kind,
                                    target: reference
                                        .attr("href")
                                        .unwrap_or_default()
                                        .trim_start_matches('#')
                                        .to_owned(),
                                });
                            }
                        }
                        _ => doc.preserved.push(PreservedXml {
                            owner: decision.id.clone(),
                            xml: part.to_xml(&BTreeMap::new()),
                        }),
                    }
                }
                doc.decisions.push(decision);
            }
            "inputData" if child.ns == DMN_MODEL_NS => doc.input_data.push(DmnInputData {
                id: id_of(child),
                name: child.attr("name").unwrap_or_default().to_owned(),
            }),
            _ => doc.preserved.push(PreservedXml {
                owner: doc.id.clone(),
                xml: child.to_xml(&namespaces),
            }),
        }
    }
    Ok(doc)
}
