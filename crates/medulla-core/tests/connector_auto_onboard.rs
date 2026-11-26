//! Model fixture for one connector pack crossing into the shared ontology.
//!
//! This is deliberately limited to the R085.1 model seam.  Served import,
//! projection, source execution and query evidence belong to the later children
//! and are not inferred from this fixture.

use std::collections::{BTreeMap, BTreeSet};

use medulla_graph_core::graph::GraphSchemaSources;
use medulla_rdf::mapping::parse_turtle;
use medulla_rdf::oxrdf::{NamedNode, NamedOrBlankNode, Term, Triple};
use medulla_rdf::pack::{declared_classes_and_ontologies, ontology_orphan_classes};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS_SUBCLASS: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const CAT: &str = "https://knuckles-team.github.io/ontology/catalog#";
const CP: &str = "https://knuckles-team.github.io/ontology/connector-pack#";
const KG: &str = "https://knuckles-team.github.io/ontology/core#";
const SCHEMA: &str = "https://schema.org/";

fn has_iri(triples: &[Triple], subject: &str, predicate: &str, object: &str) -> bool {
    triples.contains(&Triple::new(
        NamedNode::new(subject).unwrap(),
        NamedNode::new(predicate).unwrap(),
        NamedNode::new(object).unwrap(),
    ))
}

fn typed_subjects<'a>(triples: &'a [Triple], class: &str) -> BTreeSet<&'a str> {
    triples
        .iter()
        .filter(|triple| triple.predicate.as_str() == RDF_TYPE)
        .filter(|triple| matches!(&triple.object, Term::NamedNode(node) if node.as_str() == class))
        .filter_map(|triple| match &triple.subject {
            NamedOrBlankNode::NamedNode(node) => Some(node.as_str()),
            NamedOrBlankNode::BlankNode(_) => None,
        })
        .collect()
}

fn parent_classes() -> BTreeSet<String> {
    let sources = GraphSchemaSources::default();
    sources
        .validate()
        .expect("the registered parent catalog validates");
    sources
        .core
        .values()
        .filter_map(|source| source.ontology_ttl.as_deref())
        .flat_map(|document| declared_classes_and_ontologies(document).0)
        .collect()
}

// spec: MEDULLA-CQ-CONN-R085.1
#[test]
fn fixture_parses_and_is_linked_to_the_parent() {
    let fixture = include_str!("fixtures/connector_auto_onboard/connector_pack.ttl");
    let triples = parse_turtle(fixture).expect("connector pack fixture parses");
    let parents = parent_classes();

    let orphans = ontology_orphan_classes(&[fixture], &parents);
    assert!(
        orphans.is_empty(),
        "fixture ontology has orphan classes: {orphans:?}"
    );

    let connector =
        "https://knuckles-team.github.io/ontology/test/connector-auto-onboard#connector";
    let connector_class =
        "https://knuckles-team.github.io/ontology/test/connector-auto-onboard#ConnectorIncident";
    let parent_class = format!("{KG}WorkItem");
    let instance =
        "https://knuckles-team.github.io/ontology/test/connector-auto-onboard#incident-1";
    let source = "https://knuckles-team.github.io/ontology/test/connector-auto-onboard#incidents";
    let operation =
        "https://knuckles-team.github.io/ontology/test/connector-auto-onboard#search-incidents";

    assert!(parents.contains(&parent_class));
    assert!(has_iri(
        &triples,
        connector_class,
        RDFS_SUBCLASS,
        &format!("{KG}Ticket")
    ));
    assert!(has_iri(&triples, instance, RDF_TYPE, connector_class));
    assert!(has_iri(
        &triples,
        instance,
        &format!("{CAT}fromSource"),
        source
    ));
    assert!(has_iri(
        &triples,
        source,
        &format!("{CP}ofConnector"),
        connector
    ));
    assert!(has_iri(
        &triples,
        source,
        &format!("{CP}servesClass"),
        connector_class
    ));
    assert!(has_iri(
        &triples,
        source,
        &format!("{CAT}hasField"),
        "https://knuckles-team.github.io/ontology/test/connector-auto-onboard#incident-id"
    ));
    assert!(has_iri(
        &triples,
        operation,
        &format!("{CP}ofConnector"),
        connector
    ));
    assert!(has_iri(
        &triples,
        operation,
        &format!("{CP}servesClass"),
        connector_class
    ));
    assert!(has_iri(
        &triples,
        operation,
        &format!("{CAT}actionType"),
        &format!("{SCHEMA}SearchAction")
    ));

    assert_eq!(
        typed_subjects(&triples, &format!("{CAT}PromptAsset")),
        ["https://knuckles-team.github.io/ontology/test/connector-auto-onboard#system-prompt"]
            .into_iter()
            .collect()
    );
    assert_eq!(
        typed_subjects(&triples, &format!("{CAT}SkillAsset")),
        ["https://knuckles-team.github.io/ontology/test/connector-auto-onboard#incident-skill"]
            .into_iter()
            .collect()
    );
    assert_eq!(
        typed_subjects(&triples, &format!("{CAT}ToolAsset")),
        ["https://knuckles-team.github.io/ontology/test/connector-auto-onboard#search-incidents"]
            .into_iter()
            .collect()
    );
    assert_eq!(
        typed_subjects(&triples, &format!("{CAT}DataResource")),
        [source].into_iter().collect()
    );

    let expected = include_str!("fixtures/connector_auto_onboard/expected.tsv");
    let row: Vec<&str> = expected.trim().split('\t').collect();
    assert_eq!(
        row,
        vec![parent_class.as_str(), connector_class, instance, source],
        "expected answer row is the fixture's parent/connector/instance/source contract"
    );
}

/// Every `rdfs:subClassOf` edge of the registered parent catalog plus `extra`.
fn subclass_edges(extra: &[Triple]) -> BTreeMap<String, BTreeSet<String>> {
    let sources = GraphSchemaSources::default();
    let mut edges: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let parent_triples = sources
        .core
        .values()
        .filter_map(|source| source.ontology_ttl.as_deref())
        .filter_map(|document| parse_turtle(document).ok())
        .flatten();
    for triple in parent_triples.chain(extra.iter().cloned()) {
        if triple.predicate.as_str() != RDFS_SUBCLASS {
            continue;
        }
        if let (NamedOrBlankNode::NamedNode(child), Term::NamedNode(parent)) =
            (&triple.subject, &triple.object)
        {
            edges
                .entry(child.as_str().to_owned())
                .or_default()
                .insert(parent.as_str().to_owned());
        }
    }
    edges
}

/// True when `class` is `ancestor` or reaches it through `rdfs:subClassOf`.
fn reaches(edges: &BTreeMap<String, BTreeSet<String>>, class: &str, ancestor: &str) -> bool {
    let mut seen = BTreeSet::new();
    let mut open = vec![class.to_owned()];
    while let Some(next) = open.pop() {
        if next == ancestor {
            return true;
        }
        if seen.insert(next.clone()) {
            open.extend(edges.get(&next).into_iter().flatten().cloned());
        }
    }
    false
}

/// One answer row of a parent-term question: class, instance, and the live
/// source (connector, source) that served it, or `None` for a native row.
type AnswerRow = (String, String, Option<(String, String)>);

/// The instances of `parent` a question naming only parent terms returns.
/// `connected` names the connectors whose live data is available.
fn parent_term_answer(
    triples: &[Triple],
    native: &[(&str, &str)],
    parent: &str,
    connected: &BTreeSet<String>,
) -> (Vec<AnswerRow>, Vec<String>) {
    let edges = subclass_edges(triples);
    let mut rows: Vec<AnswerRow> = native
        .iter()
        .filter(|(class, _)| reaches(&edges, class, parent))
        .map(|(class, id)| ((*class).to_owned(), (*id).to_owned(), None))
        .collect();
    let mut missing = BTreeSet::new();
    for triple in triples.iter().filter(|t| t.predicate.as_str() == RDF_TYPE) {
        let (NamedOrBlankNode::NamedNode(instance), Term::NamedNode(class)) =
            (&triple.subject, &triple.object)
        else {
            continue;
        };
        if !reaches(&edges, class.as_str(), parent) {
            continue;
        }
        let from_source = format!("{CAT}fromSource");
        let Some(source) = triples.iter().find_map(|t| match (&t.subject, &t.object) {
            (NamedOrBlankNode::NamedNode(s), Term::NamedNode(o))
                if s == instance && t.predicate.as_str() == from_source =>
            {
                Some(o.as_str().to_owned())
            }
            _ => None,
        }) else {
            continue;
        };
        let of_connector = format!("{CP}ofConnector");
        let connector = triples
            .iter()
            .find_map(|t| match (&t.subject, &t.object) {
                (NamedOrBlankNode::NamedNode(s), Term::NamedNode(o))
                    if s.as_str() == source && t.predicate.as_str() == of_connector =>
                {
                    Some(o.as_str().to_owned())
                }
                _ => None,
            })
            .expect("a live source names its connector");
        if connected.contains(&connector) {
            rows.push((
                class.as_str().to_owned(),
                instance.as_str().to_owned(),
                Some((connector, source)),
            ));
        } else {
            missing.insert(connector);
        }
    }
    (rows, missing.into_iter().collect())
}

// spec: MEDULLA-CQ-CONN-R085.2
#[test]
fn parent_term_question_returns_native_and_live_connector_rows() {
    let fixture = include_str!("fixtures/connector_auto_onboard/connector_pack.ttl");
    let triples = parse_turtle(fixture).expect("connector pack fixture parses");
    let parent = format!("{KG}WorkItem");
    let ticket = format!("{KG}Ticket");
    let connector =
        "https://knuckles-team.github.io/ontology/test/connector-auto-onboard#connector";
    let instance =
        "https://knuckles-team.github.io/ontology/test/connector-auto-onboard#incident-1";
    let source = "https://knuckles-team.github.io/ontology/test/connector-auto-onboard#incidents";
    let native = [(ticket.as_str(), "native:ticket-1")];

    // The question names only the parent class; the connector subclass answers
    // it live, and the native instance of a parent subclass stays in the answer.
    let connected: BTreeSet<String> = [connector.to_owned()].into_iter().collect();
    let (rows, missing) = parent_term_answer(&triples, &native, &parent, &connected);
    assert!(missing.is_empty(), "{missing:?}");
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows
        .iter()
        .any(|(_, id, live)| id == "native:ticket-1" && live.is_none()));
    let expected = include_str!("fixtures/connector_auto_onboard/expected.tsv");
    let row: Vec<&str> = expected.trim().split('\t').collect();
    let live: Vec<_> = rows.iter().filter(|(_, _, live)| live.is_some()).collect();
    assert_eq!(live.len(), 1, "{rows:?}");
    assert_eq!(live[0].1, instance);
    assert_eq!(live[0].2, Some((connector.to_owned(), source.to_owned())));
    assert_eq!(row[2], instance);

    // With the connector absent the answer holds native rows only and names it.
    let (rows, missing) = parent_term_answer(&triples, &native, &parent, &BTreeSet::new());
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].2.is_none());
    assert_eq!(missing, vec![connector.to_owned()]);
}
