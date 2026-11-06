//! openCypher over a named graph view (MEDULLA-UNIFIED-DATA-PLANE-R038.6.2 to .5).

use medulla_types::virtual_graph_view::{GraphView, NodeLabelMapping, RelationshipTypeMapping};

use crate::cypher::{compile_view_text, ViewLimits, ViewPlan, ViewRefusal};

const LIMITS: ViewLimits = ViewLimits { max_path_depth: 3 };

fn label(name: &str, class: &str, mapping: &str) -> NodeLabelMapping {
    NodeLabelMapping {
        label: name.to_string(),
        ontology_class: class.to_string(),
        mapping_id: mapping.to_string(),
    }
}

fn rel(name: &str, from: &str, to: &str, cond: &str) -> RelationshipTypeMapping {
    RelationshipTypeMapping {
        rel_type: name.to_string(),
        from_label: from.to_string(),
        to_label: to.to_string(),
        join_mapping_ids: vec![format!("{from}-map"), format!("{to}-map")],
        join_condition: cond.to_string(),
    }
}

/// Person -WORKS_FOR-> Org -LOCATED_IN-> City, all over one source.
fn view() -> GraphView {
    GraphView {
        mapping_digest: "sha256:view".to_string(),
        node_labels: vec![
            label("Person", "https://schema.org/Person", "person"),
            label("Org", "https://schema.org/Organization", "org"),
            label("City", "https://schema.org/City", "city"),
        ],
        relationship_types: vec![
            rel("WORKS_FOR", "Person", "Org", "person.org_id = org.id"),
            rel("LOCATED_IN", "Org", "City", "org.city_id = city.id"),
        ],
    }
}

/// A source that records every statement handed to it.
#[derive(Default)]
struct RecordingSource {
    statements: Vec<String>,
}

impl RecordingSource {
    fn run(plan: &ViewPlan) -> Self {
        let mut source = Self::default();
        plan.dispatch(&mut |statement| source.statements.push(statement.to_string()));
        source
    }
}

fn compile(text: &str) -> Result<ViewPlan, ViewRefusal> {
    compile_view_text(&view(), text, LIMITS)
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R038.6.2
#[test]
fn pattern_pushes_join() {
    const CASES: [(&str, usize); 3] = [
        ("MATCH (p:Person) RETURN p.name", 1),
        (
            "MATCH (p:Person)-[:WORKS_FOR]->(o:Org) RETURN p.name, o.name",
            1,
        ),
        (
            "MATCH (p:Person)-[:WORKS_FOR]->(o:Org)-[:LOCATED_IN]->(c:City) RETURN c.name",
            2,
        ),
    ];
    for (text, joins) in CASES {
        let source = RecordingSource::run(&compile(text).expect(text));
        assert_eq!(source.statements.len(), joins, "{text}");
    }

    let plan = compile(
        "MATCH (p:Person)-[:WORKS_FOR]->(o:Org) WHERE p.age > 30 AND o.name = 'Acme' \
         RETURN p.name LIMIT 5",
    )
    .unwrap();
    let source = RecordingSource::run(&plan);
    assert_eq!(
        source.statements,
        vec![
            "SELECT * FROM \"person\" JOIN \"org\" ON person.org_id = org.id \
             WHERE \"person\".\"age\" > 30 AND \"org\".\"name\" = 'Acme'"
        ]
    );
    assert!(plan.sparql.contains("?p a <https://schema.org/Person> ."));
    assert!(plan
        .sparql
        .contains("?p <https://schema.org/WORKS_FOR> ?o ."));
    assert!(plan.sparql.contains("FILTER(?p__age > 30)"));
    assert!(plan.sparql.ends_with("LIMIT 5"));

    let bounded = compile("MATCH (p:Person)-[:WORKS_FOR*1..2]->(o:Org) RETURN o.name").unwrap();
    assert!(bounded.sparql.contains("(<https://schema.org/WORKS_FOR>|"));
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R038.6.3
#[test]
fn served_read_resolves_view() {
    let plan = compile("MATCH (o:Org)<-[:WORKS_FOR]-(p) RETURN o.name, p").unwrap();
    assert!(plan
        .sparql
        .contains("?o a <https://schema.org/Organization> ."));
    // The anonymous-label endpoint resolves from the relationship's mapped label.
    assert!(plan.sparql.contains("?p a <https://schema.org/Person> ."));
    assert!(plan
        .sparql
        .contains("?p <https://schema.org/WORKS_FOR> ?o ."));
    assert_eq!(plan.pushed_statements.len(), 1);
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R038.6.4
#[test]
fn view_refusals() {
    const CASES: [(&str, &str, &str); 8] = [
        (
            "CREATE (n:Person {name: 'x'})",
            "VIRTUAL_VIEW_READ_ONLY",
            "CREATE",
        ),
        (
            "MATCH (n:Person) SET n.name = 'x'",
            "VIRTUAL_VIEW_READ_ONLY",
            "SET",
        ),
        ("MATCH (n:Ghost) RETURN n", "UNMAPPED_LABEL", "Ghost"),
        (
            "MATCH (p:Person)-[:KNOWS]->(q:Person) RETURN q",
            "UNMAPPED_LABEL",
            "KNOWS",
        ),
        (
            "MATCH (p:Person)-[:WORKS_FOR*1..9]->(o:Org) RETURN o",
            "PATH_DEPTH_EXCEEDED",
            "*1..9",
        ),
        (
            "MATCH (p:Person) WITH p RETURN p",
            "VIEW_CLAUSE_UNSUPPORTED",
            "WITH",
        ),
        (
            "MATCH (p:Person) WHERE p.age > 1 OR p.age < 0 RETURN p",
            "VIEW_CLAUSE_UNSUPPORTED",
            "OR",
        ),
        (
            "MATCH (p:Person) RETURN count(*)",
            "VIEW_CLAUSE_UNSUPPORTED",
            "count(*)",
        ),
    ];
    for (text, code, token) in CASES {
        let refusal = compile(text).expect_err(text);
        assert_eq!(refusal.code(), code, "{text}");
        assert_eq!(refusal.token(), token, "{text}");
        // A refusal is a compile error: no plan exists, so no source statement can run.
        assert!(refusal.to_string().starts_with(code), "{text}");
    }
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R038.6.5
#[test]
fn explain_names_mapping() {
    let plan =
        compile("MATCH (p:Person)-[:WORKS_FOR]->(o:Org)-[:LOCATED_IN]->(c:City) RETURN c.name")
            .unwrap();
    assert_eq!(plan.mapping_digests.len(), plan.pushed_statements.len());
    for digest in &plan.mapping_digests {
        assert_eq!(digest.len(), 64, "{digest}");
        assert!(digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    }
    assert_ne!(plan.mapping_digests[0], plan.mapping_digests[1]);
    for statement in &plan.pushed_statements {
        assert!(!statement.contains("password") && !statement.contains("://"));
    }
}
