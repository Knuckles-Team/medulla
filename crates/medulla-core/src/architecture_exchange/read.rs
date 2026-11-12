//! Open Exchange Format reader (MEDULLA-CQ-ORCH-R705.3): one streaming pass
//! with the entity rules of the process readers.

use std::collections::BTreeMap;

use medulla_types::archimate_exchange::{
    ArchimateExchange, ArchimateExchangeError, ExchangeConnection, ExchangeElement,
    ExchangeOrganization, ExchangePropertyDefinition, ExchangeRelationship, ExchangeView,
    ExchangeViewNode, ARCHIMATE_EXCHANGE_SCHEMA_VERSION,
};

use super::ARCHIMATE_NS;
use crate::process_exchange::xml_tree::{parse_tree, XmlElement, XmlLimits};
use crate::process_exchange::ProcessExchangeError;

fn lift(error: ProcessExchangeError) -> ArchimateExchangeError {
    match error {
        ProcessExchangeError::Malformed { line, column } => {
            ArchimateExchangeError::Malformed { line, column }
        }
        ProcessExchangeError::EntityRefused => ArchimateExchangeError::EntityRefused,
        ProcessExchangeError::TooLarge { bytes } => ArchimateExchangeError::TooLarge { bytes },
        ProcessExchangeError::NamespaceUnknown { namespace } => {
            ArchimateExchangeError::NamespaceUnknown { namespace }
        }
        ProcessExchangeError::NotDeployable { .. } => {
            ArchimateExchangeError::Malformed { line: 0, column: 0 }
        }
    }
}

const STRATEGY: &[&str] = &["Resource", "Capability", "ValueStream", "CourseOfAction"];
const ACTIVE: &[&str] = &[
    "BusinessActor",
    "BusinessRole",
    "BusinessCollaboration",
    "BusinessInterface",
    "ApplicationComponent",
    "ApplicationCollaboration",
    "ApplicationInterface",
    "Node",
    "Device",
    "SystemSoftware",
    "TechnologyCollaboration",
    "TechnologyInterface",
    "CommunicationNetwork",
    "Path",
    "Equipment",
    "Facility",
    "DistributionNetwork",
];
const BEHAVIOR: &[&str] = &[
    "BusinessProcess",
    "BusinessFunction",
    "BusinessInteraction",
    "BusinessEvent",
    "BusinessService",
    "ApplicationProcess",
    "ApplicationFunction",
    "ApplicationInteraction",
    "ApplicationEvent",
    "ApplicationService",
    "TechnologyProcess",
    "TechnologyFunction",
    "TechnologyInteraction",
    "TechnologyEvent",
    "TechnologyService",
    "Process",
    "Function",
    "Interaction",
    "Event",
    "Service",
];
const PASSIVE: &[&str] = &[
    "BusinessObject",
    "Contract",
    "Representation",
    "DataObject",
    "Artifact",
    "Material",
];
const MOTIVATION: &[&str] = &[
    "Stakeholder",
    "Driver",
    "Assessment",
    "Goal",
    "Outcome",
    "Principle",
    "Requirement",
    "Constraint",
    "Meaning",
    "Value",
];
const IMPLEMENTATION: &[&str] = &[
    "WorkPackage",
    "Deliverable",
    "ImplementationEvent",
    "Plateau",
    "Gap",
];
const OTHER: &[&str] = &[
    "Location",
    "Grouping",
    "Product",
    "Junction",
    "AndJunction",
    "OrJunction",
];

/// Every relationship type of the ArchiMate 3.x relationship table.
pub const RELATIONSHIP_TYPES: &[&str] = &[
    "Composition",
    "Aggregation",
    "Assignment",
    "Realization",
    "Serving",
    "Access",
    "Influence",
    "Association",
    "Triggering",
    "Flow",
    "Specialization",
];

fn is(set: &[&str], t: &str) -> bool {
    set.contains(&t)
}

fn known(t: &str) -> bool {
    [
        STRATEGY,
        ACTIVE,
        BEHAVIOR,
        PASSIVE,
        MOTIVATION,
        IMPLEMENTATION,
        OTHER,
    ]
    .iter()
    .any(|set| is(set, t))
}

/// Whether the relationship table allows `relationship` from an element of
/// type `source` to one of type `target`. Junctions and groupings connect
/// anything, as in the specification.
pub fn relationship_allowed(relationship: &str, source: &str, target: &str) -> bool {
    if !RELATIONSHIP_TYPES.contains(&relationship) || !known(source) || !known(target) {
        return false;
    }
    if relationship == "Association" {
        return true;
    }
    let free = |t: &str| {
        matches!(
            t,
            "Junction" | "AndJunction" | "OrJunction" | "Grouping" | "Location"
        )
    };
    if free(source) || free(target) {
        return true;
    }
    let core = |t: &str| is(ACTIVE, t) || is(BEHAVIOR, t) || is(PASSIVE, t) || t == "Product";
    match relationship {
        "Specialization" => source == target || (is(BEHAVIOR, source) && is(BEHAVIOR, target)),
        "Composition" | "Aggregation" => {
            (core(source) && core(target))
                || (is(MOTIVATION, source) && is(MOTIVATION, target))
                || (is(IMPLEMENTATION, source) && is(IMPLEMENTATION, target))
                || (is(STRATEGY, source) && is(STRATEGY, target))
        }
        "Assignment" => {
            (is(ACTIVE, source) && (is(BEHAVIOR, target) || is(ACTIVE, target)))
                || (is(STRATEGY, source) && is(STRATEGY, target))
                || (is(IMPLEMENTATION, source) && is(IMPLEMENTATION, target))
                || (source == "Stakeholder" && is(MOTIVATION, target))
        }
        "Realization" => {
            (!is(MOTIVATION, source) && !is(PASSIVE, target) && !is(ACTIVE, target))
                || (is(PASSIVE, source) && is(PASSIVE, target))
                || (is(ACTIVE, source) && is(ACTIVE, target))
        }
        "Serving" => {
            (is(BEHAVIOR, source) || is(ACTIVE, source))
                && !is(MOTIVATION, target)
                && !is(PASSIVE, target)
        }
        "Access" => (is(BEHAVIOR, source) || is(ACTIVE, source)) && is(PASSIVE, target),
        "Influence" => {
            is(MOTIVATION, target)
                && (is(MOTIVATION, source) || core(source) || is(STRATEGY, source))
        }
        "Triggering" | "Flow" => {
            is(BEHAVIOR, source) && is(BEHAVIOR, target) || is(ACTIVE, source) && is(ACTIVE, target)
        }
        _ => false,
    }
}

fn organization(item: &XmlElement) -> ExchangeOrganization {
    ExchangeOrganization {
        label: item
            .child("label")
            .map(|l| l.text.trim().to_owned())
            .unwrap_or_default(),
        item_refs: item
            .attr("identifierRef")
            .map(|r| vec![r.to_owned()])
            .unwrap_or_default(),
        children: item.children_named("item").map(organization).collect(),
    }
}

fn int(element: &XmlElement, key: &str) -> i64 {
    element
        .attr(key)
        .and_then(|v| v.trim().parse::<f64>().ok())
        .map(|v| v as i64)
        .unwrap_or(0)
}

fn name_of(element: &XmlElement) -> String {
    element
        .child("name")
        .map(|n| n.text.trim().to_owned())
        .unwrap_or_default()
}

/// Parse one Open Exchange Format document. A relationship the ArchiMate 3.x
/// table forbids between its endpoint types refuses `RelationshipInvalid`.
pub fn parse_exchange(
    bytes: &[u8],
    limits: XmlLimits,
) -> Result<ArchimateExchange, ArchimateExchangeError> {
    let (root, namespaces) = parse_tree(bytes, limits).map_err(lift)?;
    if root.ns != ARCHIMATE_NS || root.local != "model" {
        return Err(ArchimateExchangeError::NamespaceUnknown { namespace: root.ns });
    }
    let mut doc = ArchimateExchange {
        schema_version: ARCHIMATE_EXCHANGE_SCHEMA_VERSION,
        model_id: root.attr("identifier").unwrap_or_default().to_owned(),
        name: name_of(&root),
        elements: Vec::new(),
        relationships: Vec::new(),
        organizations: Vec::new(),
        views: Vec::new(),
        property_definitions: Vec::new(),
        preserved: Vec::new(),
    };
    for child in &root.children {
        match child.local.as_str() {
            "name" => {}
            "elements" => {
                for e in child.children_named("element") {
                    let mut properties = BTreeMap::new();
                    if let Some(list) = e.child("properties") {
                        for p in list.children_named("property") {
                            let value =
                                p.child("value").map(|v| v.text.clone()).unwrap_or_default();
                            properties.insert(
                                p.attr("propertyDefinitionRef")
                                    .unwrap_or_default()
                                    .to_owned(),
                                value,
                            );
                        }
                    }
                    doc.elements.push(ExchangeElement {
                        schema_version: ARCHIMATE_EXCHANGE_SCHEMA_VERSION,
                        identifier: e.attr("identifier").unwrap_or_default().to_owned(),
                        element_type: e.attr("type").unwrap_or_default().to_owned(),
                        name: name_of(e),
                        properties,
                    });
                }
            }
            "relationships" => {
                for r in child.children_named("relationship") {
                    doc.relationships.push(ExchangeRelationship {
                        schema_version: ARCHIMATE_EXCHANGE_SCHEMA_VERSION,
                        identifier: r.attr("identifier").unwrap_or_default().to_owned(),
                        relationship_type: r.attr("type").unwrap_or_default().to_owned(),
                        source: r.attr("source").unwrap_or_default().to_owned(),
                        target: r.attr("target").unwrap_or_default().to_owned(),
                    });
                }
            }
            "organizations" => doc
                .organizations
                .extend(child.children_named("item").map(organization)),
            "propertyDefinitions" => {
                for d in child.children_named("propertyDefinition") {
                    doc.property_definitions.push(ExchangePropertyDefinition {
                        identifier: d.attr("identifier").unwrap_or_default().to_owned(),
                        name: name_of(d),
                        data_type: d.attr("type").unwrap_or("string").to_owned(),
                    });
                }
            }
            "views" => {
                for diagrams in child.children_named("diagrams") {
                    for v in diagrams.children_named("view") {
                        doc.views.push(ExchangeView {
                            identifier: v.attr("identifier").unwrap_or_default().to_owned(),
                            name: name_of(v),
                            viewpoint: v.attr("viewpoint").map(str::to_owned),
                            nodes: v
                                .children_named("node")
                                .map(|n| ExchangeViewNode {
                                    identifier: n.attr("identifier").unwrap_or_default().to_owned(),
                                    element_ref: n.attr("elementRef").map(str::to_owned),
                                    x: int(n, "x"),
                                    y: int(n, "y"),
                                    w: int(n, "w"),
                                    h: int(n, "h"),
                                })
                                .collect(),
                            connections: v
                                .children_named("connection")
                                .map(|c| ExchangeConnection {
                                    identifier: c.attr("identifier").unwrap_or_default().to_owned(),
                                    relationship_ref: c.attr("relationshipRef").map(str::to_owned),
                                    source: c.attr("source").unwrap_or_default().to_owned(),
                                    target: c.attr("target").unwrap_or_default().to_owned(),
                                })
                                .collect(),
                        });
                    }
                }
            }
            _ => doc.preserved.push(child.to_xml(&namespaces)),
        }
    }
    let types: BTreeMap<&str, &str> = doc
        .elements
        .iter()
        .map(|e| (e.identifier.as_str(), e.element_type.as_str()))
        .collect();
    let relationship_ids: Vec<&str> = doc
        .relationships
        .iter()
        .map(|r| r.identifier.as_str())
        .collect();
    for r in &doc.relationships {
        let invalid = || ArchimateExchangeError::RelationshipInvalid {
            identifier: r.identifier.clone(),
        };
        if !RELATIONSHIP_TYPES.contains(&r.relationship_type.as_str()) {
            return Err(invalid());
        }
        let source = types.get(r.source.as_str()).ok_or_else(invalid)?;
        match types.get(r.target.as_str()) {
            Some(target) => {
                if !relationship_allowed(&r.relationship_type, source, target) {
                    return Err(invalid());
                }
            }
            // A relationship may end on another relationship only by association.
            None if relationship_ids.contains(&r.target.as_str())
                && r.relationship_type == "Association" => {}
            None => return Err(invalid()),
        }
    }
    Ok(doc)
}
