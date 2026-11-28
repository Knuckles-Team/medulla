//! The machine competency question (MEDULLA-MACHINE-R004, R009).
//!
//! Three connector records, an SSH host record, an orchestrator member record and a managed-host
//! record, each point `:observes` at the key anchor of a key they hold. The canonical machine
//! is the one node that carries `:identityTier`. One question asks which records observe each
//! machine and what its lifecycle state is, and answers one machine in state `active` with
//! three observers and one probe snapshot.
#![cfg(feature = "sparql")]

mod common;

use common::parse_all;
use medulla_graph_core::graph::GraphCore;
use medulla_graph_core::graph::GraphView;
use medulla_rdf::mapping::{load_triples, parse_turtle, IriStore};
use medulla_rdf::sparql::{execute, Dataset, Projection, QueryOutcome};

const MACHINE_ONTOLOGY: &str = include_str!("../../medulla-core/ontology/machine-v1.ttl");

/// One canonical machine, its two key anchors, one probe snapshot and three connector views.
/// Each view is a subclass of `:Machine` declared by its connector, with no `:identityTier`.
const FIXTURE: &str = r#"
@prefix : <https://knuckles-team.github.io/ontology/core#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex: <urn:eg:fixture:machine:> .

ex:SshHost rdfs:subClassOf :Machine .
ex:ClusterMember rdfs:subClassOf :Machine .
ex:ManagedHost rdfs:subClassOf :Machine .

<urn:machine:serial:f3af1defd63343f5dd2ee44b56cb636d> a :Machine ;
    :machineName "alpha" ; :identityTier "serial" ; :machineKey "serial:ABC 123" , "host:alpha" ;
    :machineLifecycleState "active" .

<urn:machine-key:serial:ce4057006ad7685b67a799f0743e2911> a :MachineKey ;
    :machineKey "serial:ABC 123" ;
    :identifies <urn:machine:serial:f3af1defd63343f5dd2ee44b56cb636d> .
<urn:machine-key:host:41097ff758a2bcf28635c4da73d9fa18> a :MachineKey ;
    :machineKey "host:alpha" ;
    :identifies <urn:machine:serial:f3af1defd63343f5dd2ee44b56cb636d> .

<urn:machine-probe:p1> a :ProbeSnapshot ;
    :probeKind "system" ; :connector "shell" ; :tool "inventory" ;
    :probeDigest "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef" ;
    :observedAt "2026-11-27T10:00:00Z"^^xsd:dateTime ;
    :observes <urn:machine:serial:f3af1defd63343f5dd2ee44b56cb636d> .

ex:ssh-alpha a ex:SshHost ; :observes <urn:machine-key:host:41097ff758a2bcf28635c4da73d9fa18> .
ex:node-alpha a ex:ClusterMember ; :observes <urn:machine-key:host:41097ff758a2bcf28635c4da73d9fa18> .
ex:managed-alpha a ex:ManagedHost ; :observes <urn:machine-key:serial:ce4057006ad7685b67a799f0743e2911> .

# A second machine nobody observes still answers, with no observer.
<urn:machine:host:c9faaf568305e0a90e726ca4b4807c73> a :Machine ;
    :machineName "idle" ; :identityTier "host" ; :machineKey "host:idle" ;
    :machineLifecycleState "commissioned" .
"#;

fn graph() -> GraphView {
    let core = GraphCore::new();
    let mut iris = IriStore::default();
    load_triples(&core, &mut iris, "schema", parse_all(&[MACHINE_ONTOLOGY])).unwrap();
    load_triples(&core, &mut iris, "fixture", parse_turtle(FIXTURE).unwrap()).unwrap();
    core.analysis_snapshot()
}

fn select(view: &GraphView, query: &str) -> Vec<Vec<String>> {
    let outcome = execute(
        &Dataset::new(view, Vec::new()),
        query,
        &Projection::raw(),
        None,
    )
    .expect("the query executes");
    let QueryOutcome::Solutions(table) = outcome else {
        panic!("expected a SELECT solution table");
    };
    let (_, rows) = table.to_text_rows();
    rows.into_iter()
        .map(|row| {
            row.into_iter()
                .map(|cell| cell.unwrap_or_default())
                .collect()
        })
        .collect()
}

/// Which records observe each machine, directly or through a key anchor, and the state of the
/// machine. A canonical machine is a node that carries `:identityTier`; a probe snapshot is
/// not a connector record.
const QUESTION: &str = r#"
PREFIX : <https://knuckles-team.github.io/ontology/core#>
SELECT ?machine ?state ?observer WHERE {
  ?machine :identityTier ?tier ; :machineLifecycleState ?state .
  OPTIONAL {
    { ?observer :observes ?machine } UNION { ?observer :observes ?anchor . ?anchor :identifies ?machine }
    OPTIONAL { ?observer a ?probe . FILTER(?probe = :ProbeSnapshot) }
    FILTER(!BOUND(?probe))
  }
}"#;

const SNAPSHOTS: &str = r#"
PREFIX : <https://knuckles-team.github.io/ontology/core#>
SELECT ?machine ?probe WHERE {
  ?machine :identityTier ?tier .
  ?probe a :ProbeSnapshot ; :observes ?machine .
}"#;

fn clean(cell: &str) -> &str {
    cell.trim_matches(|c| c == '<' || c == '>' || c == '"')
}

// spec: MEDULLA-MACHINE-R004
// spec: MEDULLA-MACHINE-R009
#[test]
fn three_connector_records_answer_one_machine_question() {
    let view = graph();
    let rows = select(&view, QUESTION);
    let alpha = "urn:machine:serial:f3af1defd63343f5dd2ee44b56cb636d";
    let mut observers: Vec<String> = rows
        .iter()
        .filter(|row| clean(&row[0]) == alpha)
        .filter(|row| !row[2].is_empty())
        .map(|row| clean(&row[2]).to_string())
        .collect();
    observers.sort();
    observers.dedup();
    assert_eq!(
        observers,
        [
            "urn:eg:fixture:machine:managed-alpha",
            "urn:eg:fixture:machine:node-alpha",
            "urn:eg:fixture:machine:ssh-alpha",
        ],
        "{rows:?}"
    );
    for row in rows.iter().filter(|row| clean(&row[0]) == alpha) {
        assert_eq!(clean(&row[1]).trim_matches('"'), "active", "{row:?}");
    }
    // Only canonical machines answer: the three connector views carry no identityTier.
    let machines: std::collections::BTreeSet<&str> =
        rows.iter().map(|row| clean(&row[0])).collect();
    assert_eq!(
        machines,
        std::collections::BTreeSet::from([
            alpha,
            "urn:machine:host:c9faaf568305e0a90e726ca4b4807c73"
        ])
    );
    let idle: Vec<&Vec<String>> = rows
        .iter()
        .filter(|row| clean(&row[0]).ends_with("c9faaf568305e0a90e726ca4b4807c73"))
        .collect();
    assert_eq!(idle.len(), 1);
    assert!(
        idle[0][2].is_empty(),
        "an unobserved machine has no observer"
    );
    let snapshots = select(&view, SNAPSHOTS);
    assert_eq!(snapshots.len(), 1, "{snapshots:?}");
    assert_eq!(clean(&snapshots[0][0]), alpha);
}
