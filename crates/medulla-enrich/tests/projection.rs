use std::collections::{BTreeMap, BTreeSet};

use medulla_enrich::semantic::{
    project_edges, ProjectionNode, ProjectionRule, PROJECTION_RULE_UNKNOWN_TAXONOMY,
};
use medulla_enrich::Id;

fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}

fn rule(name: &str, taxonomy: &str, property: &str, relation: &str) -> ProjectionRule {
    ProjectionRule {
        id: id(name),
        taxonomy: id(taxonomy),
        property: id(property),
        relation: id(relation),
    }
}

fn node(properties: &[(&str, &str)]) -> ProjectionNode {
    ProjectionNode {
        id: id("node:1"),
        properties: properties
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<BTreeMap<_, _>>(),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R614.2.4
#[test]
fn projection_emits_stable_edges_and_skips_unresolved() {
    let taxonomies: BTreeSet<Id> = [id("taxonomy:code")].into_iter().collect();
    let rules = [
        rule("rule:owner", "taxonomy:code", "owner", "eg:edge/owned_by"),
        rule(
            "rule:parent",
            "taxonomy:code",
            "parent",
            "eg:edge/contained_in",
        ),
        rule(
            "rule:absent",
            "taxonomy:code",
            "missing_property",
            "eg:edge/none",
        ),
    ];
    let existing: BTreeSet<Id> = [id("node:team")].into_iter().collect();
    let subject = node(&[("owner", "node:team"), ("parent", "node:ghost")]);

    let first = project_edges(&subject, &rules, &taxonomies, |n| existing.contains(n)).unwrap();
    assert_eq!(first.edges.len(), 1);
    assert_eq!(first.unresolved, 1, "the ghost parent counts as unresolved");
    let edge = &first.edges[0];
    assert_eq!(edge.source.as_str(), "node:1");
    assert_eq!(edge.relation.as_str(), "eg:edge/owned_by");
    assert_eq!(edge.target.as_str(), "node:team");
    assert_eq!(edge.id.len(), 64);

    // A replay writes the same edge id.
    let replay = project_edges(&subject, &rules, &taxonomies, |n| existing.contains(n)).unwrap();
    assert_eq!(replay, first);

    // The id depends on all three parts.
    let other_relation = [rule(
        "rule:owner",
        "taxonomy:code",
        "owner",
        "eg:edge/other",
    )];
    let moved = project_edges(&subject, &other_relation, &taxonomies, |n| {
        existing.contains(n)
    })
    .unwrap();
    assert_ne!(moved.edges[0].id, edge.id);

    // A value that is not even a valid identifier is unresolved, not an error.
    let spaced = node(&[("owner", "not an id")]);
    let none = project_edges(&spaced, &rules, &taxonomies, |_| true).unwrap();
    assert!(none.edges.is_empty());
    assert_eq!(none.unresolved, 1);

    // An undeclared taxonomy refuses the projection.
    let bad = [rule(
        "rule:x",
        "taxonomy:unknown",
        "owner",
        "eg:edge/owned_by",
    )];
    assert_eq!(
        project_edges(&subject, &bad, &taxonomies, |_| true)
            .unwrap_err()
            .code,
        PROJECTION_RULE_UNKNOWN_TAXONOMY
    );
}
