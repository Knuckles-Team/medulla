//! Connector-pack competency proofs (MEDULLA-CQ-CONN-R084).
//!
//! The pack competency questions (R084.1 to R084.4) are answered by SPARQL over
//! imported connector packs. This file carries the vocabulary proofs those
//! questions depend on, starting with the finance method vocabulary (R084.5):
//! the terms are declared in the registered `finance` core source, and the
//! shapes companion registered as `finance-shapes` rejects an algorithm record
//! missing either applicability axis.

use std::collections::BTreeSet;

use medulla_graph_core::graph::GraphCore;
use medulla_graph_core::graph::GraphSchemaSources;
use medulla_graph_core::graph::SchemaSourceOrigin;
use medulla_rdf::mapping::{load_triples, parse_turtle, IriStore};
use medulla_rdf::oxrdf::{NamedNode, Triple};
use medulla_rdf::sparql::{execute, Dataset, Projection, QueryOutcome};

const KG: &str = "https://knuckles-team.github.io/ontology/core#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS_DOMAIN: &str = "http://www.w3.org/2000/01/rdf-schema#domain";
const RDFS_RANGE: &str = "http://www.w3.org/2000/01/rdf-schema#range";
const OWL_CLASS: &str = "http://www.w3.org/2002/07/owl#Class";
const OWL_OBJECT_PROPERTY: &str = "http://www.w3.org/2002/07/owl#ObjectProperty";
const SHACL_NODE_SHAPE: &str = "http://www.w3.org/ns/shacl#NodeShape";

fn has_iri(triples: &[Triple], subject: &str, predicate: &str, object: &str) -> bool {
    triples.contains(&Triple::new(
        NamedNode::new(subject).unwrap(),
        NamedNode::new(predicate).unwrap(),
        NamedNode::new(object).unwrap(),
    ))
}

fn finance_documents() -> (std::sync::Arc<str>, std::sync::Arc<str>) {
    let sources = GraphSchemaSources::default();
    sources
        .validate()
        .expect("the default core catalog validates");
    for (source_id, expected_module) in [
        ("core:finance@1", "finance"),
        ("core:finance-shapes@1", "finance-shapes"),
    ] {
        let source = sources
            .core
            .get(source_id)
            .unwrap_or_else(|| panic!("registered source {source_id} is missing"));
        match &source.origin {
            SchemaSourceOrigin::Core {
                module, version, ..
            } => {
                assert_eq!(module, expected_module, "source module ownership changed");
                assert_eq!(*version, 1, "unexpected source version");
            }
            origin => panic!("{source_id} has non-core origin {origin:?}"),
        }
    }
    let ontology = sources
        .core
        .get("core:finance@1")
        .unwrap()
        .ontology_ttl
        .clone()
        .expect("the finance module carries an ontology");
    let shapes = sources
        .core
        .get("core:finance-shapes@1")
        .unwrap()
        .shapes_ttl
        .clone()
        .expect("the finance module carries its shapes companion");
    (ontology, shapes)
}

// spec: MEDULLA-CQ-CONN-R084.5
#[test]
fn finance_method_terms_and_shape() {
    let (ontology, shapes) = finance_documents();
    let ontology: &str = &ontology;
    let shapes: &str = &shapes;
    let triples = parse_turtle(ontology).expect("the registered finance ontology parses");

    // The two method classes are declared owl:Class in the registered module.
    for class in ["FinanceAlgorithm", "AnalysisType"] {
        assert!(
            has_iri(&triples, &format!("{KG}{class}"), RDF_TYPE, OWL_CLASS),
            "{class} is declared as an owl:Class"
        );
    }
    // The applicability and input properties are declared owl:ObjectProperty
    // with the finance-method domain and their declared ranges.
    for property in [
        "appliesToAnalysis",
        "appliesToInstrumentType",
        "requiresInput",
    ] {
        let subject = format!("{KG}{property}");
        assert!(
            has_iri(&triples, &subject, RDF_TYPE, OWL_OBJECT_PROPERTY),
            "{property} is declared as an owl:ObjectProperty"
        );
        assert!(
            has_iri(
                &triples,
                &subject,
                RDFS_DOMAIN,
                &format!("{KG}FinanceAlgorithm")
            ),
            "{property} carries the FinanceAlgorithm domain"
        );
    }
    assert!(has_iri(
        &triples,
        &format!("{KG}appliesToAnalysis"),
        RDFS_RANGE,
        &format!("{KG}AnalysisType")
    ));

    // The registered shapes companion carries the algorithm shape.
    let shape_triples = parse_turtle(shapes).expect("the registered finance shapes parse");
    assert!(has_iri(
        &shape_triples,
        &format!("{KG}FinanceAlgorithmShape"),
        RDF_TYPE,
        SHACL_NODE_SHAPE
    ));

    // A complete algorithm conforms; dropping either applicability axis
    // violates the shape.
    let complete = "@prefix : <https://knuckles-team.github.io/ontology/core#> .
        :method a :FinanceAlgorithm ; :appliesToAnalysis :riskAnalysis ; :appliesToInstrumentType :Position .
        :riskAnalysis a :AnalysisType .";
    let report = medulla_shacl::validate_turtle(shapes, complete).expect("validation runs");
    assert!(report.conforms, "a complete algorithm conforms: {report:?}");

    let missing_analysis = "@prefix : <https://knuckles-team.github.io/ontology/core#> .
        :method a :FinanceAlgorithm ; :appliesToInstrumentType :Position .";
    let report = medulla_shacl::validate_turtle(shapes, missing_analysis).expect("validation runs");
    assert!(
        !report.conforms,
        "an algorithm naming no analysis type violates the shape: {report:?}"
    );

    let missing_instrument = "@prefix : <https://knuckles-team.github.io/ontology/core#> .
        :method a :FinanceAlgorithm ; :appliesToAnalysis :riskAnalysis .";
    let report =
        medulla_shacl::validate_turtle(shapes, missing_instrument).expect("validation runs");
    assert!(
        !report.conforms,
        "an algorithm naming no instrument or position class violates the shape: {report:?}"
    );
}

// ── The four pack competency questions (R084.1 to R084.4) ─────────────────────
//
// Each question is a directory under `fixtures/connector_pack_competency/` holding
// `query.rq` and `expected.tsv`. The pack Turtle (`finance_pack.ttl`,
// `document_pack.ttl`) is the same text the served L4 test imports as a pack
// ontology entry; `l2_stand_in.ttl` stands in for the facts the served path
// stamps itself (projection stamps and SourceIngest metadata). `{c}` is the
// connector name, substituted by this test and by the served test.

const CONNECTOR: &str = "competency-pack";
const CP_NS: &str = "https://knuckles-team.github.io/ontology/connector-pack#";

/// SourceIngest stamps these properties on every ingested node
/// (`insert_source_metadata` in `medulla-h-ingest`); they are engine metadata
/// of the record, not ontology terms, so the registry check skips them.
const ENGINE_STAMPED: [&str; 3] = [
    "sourceRecordId",
    "sourceUpdatedAt",
    "sourceMappingReference",
];

struct Question {
    pack: &'static str,
    query: &'static str,
    expected: &'static str,
}

const FINANCE_PACK: &str = include_str!("fixtures/connector_pack_competency/finance_pack.ttl");
const DOCUMENT_PACK: &str = include_str!("fixtures/connector_pack_competency/document_pack.ttl");
const STAND_IN: &str = include_str!("fixtures/connector_pack_competency/l2_stand_in.ttl");

fn question(name: &str) -> Question {
    match name {
        "q1_finance_algorithms" => Question {
            pack: FINANCE_PACK,
            query: include_str!(
                "fixtures/connector_pack_competency/q1_finance_algorithms/query.rq"
            ),
            expected: include_str!(
                "fixtures/connector_pack_competency/q1_finance_algorithms/expected.tsv"
            ),
        },
        "q2_positions_risk" => Question {
            pack: FINANCE_PACK,
            query: include_str!("fixtures/connector_pack_competency/q2_positions_risk/query.rq"),
            expected: include_str!(
                "fixtures/connector_pack_competency/q2_positions_risk/expected.tsv"
            ),
        },
        "q3_capability_surface" => Question {
            pack: DOCUMENT_PACK,
            query: include_str!(
                "fixtures/connector_pack_competency/q3_capability_surface/query.rq"
            ),
            expected: include_str!(
                "fixtures/connector_pack_competency/q3_capability_surface/expected.tsv"
            ),
        },
        "q4_record_governance" => Question {
            pack: DOCUMENT_PACK,
            query: include_str!("fixtures/connector_pack_competency/q4_record_governance/query.rq"),
            expected: include_str!(
                "fixtures/connector_pack_competency/q4_record_governance/expected.tsv"
            ),
        },
        other => panic!("unknown question {other}"),
    }
}

fn substitute(text: &str) -> String {
    text.replace("{c}", CONNECTOR)
}

/// One answer cell reduced to a comparable word: an IRI to its local name, a
/// literal to its lexical form; the connector name is shown as `{c}`.
fn comparable(cell: &str) -> String {
    let word = if let Some(rest) = cell.strip_prefix('"') {
        rest.split('"').next().unwrap_or_default().to_string()
    } else {
        cell.trim_matches(|c| c == '<' || c == '>')
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
    };
    word.replace(CONNECTOR, "{c}")
}

/// Run one question over its pack and the L2 stand-in facts. Returns the
/// stable columns of the sorted answer rows and asserts that every remaining
/// column (source, record id, as-of) is bound and non-empty on every row.
fn answer(question: &Question, stable: usize) -> Vec<String> {
    let core = GraphCore::new();
    let mut iris = IriStore::default();
    for (graph, text) in [("pack", question.pack), ("facts", STAND_IN)] {
        let triples = parse_turtle(&substitute(text)).expect("fixture parses");
        load_triples(&core, &mut iris, graph, triples).expect("fixture loads");
    }
    let view = core.analysis_snapshot();
    let QueryOutcome::Solutions(table) = execute(
        &Dataset::new(&view, Vec::new()),
        &substitute(question.query),
        &Projection::raw(),
        None,
    )
    .expect("the question query runs") else {
        panic!("a SELECT question returns solutions");
    };
    let mut rows: Vec<String> = Vec::new();
    for row in table.to_rows().1 {
        for (index, cell) in row.iter().enumerate().skip(stable) {
            let value = cell.as_deref().unwrap_or_default();
            assert!(
                !value.is_empty(),
                "column {index} (source, record id or as-of) is unbound"
            );
        }
        rows.push(
            row.iter()
                .take(stable)
                .map(|cell| comparable(cell.as_deref().unwrap_or("-")))
                .collect::<Vec<_>>()
                .join("\t"),
        );
    }
    rows.sort();
    rows
}

fn expected_rows(question: &Question) -> (usize, Vec<String>) {
    let mut rows: Vec<String> = question.expected.lines().map(str::to_owned).collect();
    let stable = rows[0].split('\t').count();
    rows.sort();
    (stable, rows)
}

fn assert_answers(name: &str) {
    let question = question(name);
    let (stable, expected) = expected_rows(&question);
    assert_eq!(
        answer(&question, stable),
        expected,
        "{name} answered wrongly"
    );
}

/// Every `:Term` and `cp:Term` of a query, with strings and IRIs removed.
fn query_terms(query: &str) -> BTreeSet<(bool, String)> {
    let mut plain = String::new();
    let mut skip_until: Option<char> = None;
    for ch in query.chars() {
        match skip_until {
            Some(end) if ch == end => skip_until = None,
            Some(_) => {}
            None if ch == '<' => skip_until = Some('>'),
            None if ch == '"' => skip_until = Some('"'),
            None => plain.push(ch),
        }
    }
    let chars: Vec<char> = plain.chars().collect();
    let mut terms = BTreeSet::new();
    for (index, ch) in chars.iter().enumerate() {
        if *ch != ':' {
            continue;
        }
        let before: String = chars[..index]
            .iter()
            .rev()
            .take_while(|c| c.is_ascii_alphanumeric() || **c == '_')
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let name: String = chars[index + 1..]
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric() || **c == '_')
            .collect();
        if name.is_empty() || !(before.is_empty() || before == "cp") {
            continue;
        }
        terms.insert((before == "cp", name));
    }
    terms
}

/// The local names every registered core ontology module declares as a class
/// or property.
fn registered_terms() -> BTreeSet<String> {
    let sources = GraphSchemaSources::default();
    sources
        .validate()
        .expect("the default core catalog validates");
    let declaring = [
        "http://www.w3.org/2002/07/owl#Class",
        "http://www.w3.org/2002/07/owl#ObjectProperty",
        "http://www.w3.org/2002/07/owl#DatatypeProperty",
    ];
    let mut terms = BTreeSet::new();
    for source in sources.core.values() {
        let Some(document) = source.ontology_ttl.as_deref() else {
            continue;
        };
        for triple in parse_turtle(document).expect("a registered ontology parses") {
            if triple.predicate.as_str() == RDF_TYPE
                && declaring.contains(
                    &triple
                        .object
                        .to_string()
                        .trim_matches(|c| c == '<' || c == '>'),
                )
            {
                if let medulla_rdf::oxrdf::NamedOrBlankNode::NamedNode(node) = &triple.subject {
                    let iri = node.as_str();
                    if let Some(local) = iri.strip_prefix(KG).or_else(|| iri.strip_prefix(CP_NS)) {
                        terms.insert(format!(
                            "{}{local}",
                            if iri.starts_with(CP_NS) { "cp:" } else { ":" }
                        ));
                    }
                }
            }
        }
    }
    terms
}

fn assert_registered(name: &str) {
    let question = question(name);
    let registered = registered_terms();
    let terms = query_terms(question.query);
    assert!(!terms.is_empty(), "{name} names no ontology terms");
    for (is_cp, local) in terms {
        if ENGINE_STAMPED.contains(&local.as_str()) {
            continue;
        }
        let key = format!("{}{local}", if is_cp { "cp:" } else { ":" });
        assert!(
            registered.contains(&key),
            "{name} uses unregistered term {key}"
        );
    }
}

// spec: MEDULLA-CQ-CONN-R084.1
#[test]
fn finance_algorithm_selection_answers() {
    assert_answers("q1_finance_algorithms");
}

// spec: MEDULLA-CQ-CONN-R084.1
#[test]
fn finance_algorithm_selection_query_uses_registered_terms() {
    assert_registered("q1_finance_algorithms");
}

// spec: MEDULLA-CQ-CONN-R084.2
#[test]
fn positions_and_applicable_risk_analyses_answer() {
    assert_answers("q2_positions_risk");
}

// spec: MEDULLA-CQ-CONN-R084.2
#[test]
fn positions_and_risk_analyses_query_uses_registered_terms() {
    assert_registered("q2_positions_risk");
}

// spec: MEDULLA-CQ-CONN-R084.3
#[test]
fn capability_to_surface_and_contract_answers() {
    assert_answers("q3_capability_surface");
}

// spec: MEDULLA-CQ-CONN-R084.3
#[test]
fn capability_to_surface_query_uses_registered_terms() {
    assert_registered("q3_capability_surface");
}

// spec: MEDULLA-CQ-CONN-R084.4
#[test]
fn record_governance_answers() {
    assert_answers("q4_record_governance");
}

// spec: MEDULLA-CQ-CONN-R084.4
#[test]
fn record_governance_query_uses_registered_terms() {
    assert_registered("q4_record_governance");
}
