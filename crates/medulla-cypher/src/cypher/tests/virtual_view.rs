//! Tests of the graph-view compiler (MEDULLA-UNIFIED-DATA-PLANE-R038.6.2 to R038.6.4).

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use medulla_types::virtual_graph_view::{GraphView, NodeLabelMapping, RelationshipTypeMapping};
use medulla_virtual_graph::obda::{
    apply_view_joins, run_virtual, ForeignRow, JoinScan, ObdaFilter, ObdaSource,
    ObdaSourceRegistry, SourceIdentity, TriplesMap, VirtualGraph, JOIN_RIGHT_PREFIX,
};

use super::super::{compile_view_statement, resolve_view_plan, ViewRefusal, ViewResolver};

const NS: &str = "http://example.org/";
const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";

fn view() -> GraphView {
    GraphView {
        mapping_digest: "sha256:abc".to_string(),
        node_labels: vec![
            NodeLabelMapping {
                label: "Person".to_string(),
                ontology_class: format!("{NS}Person"),
                mapping_id: "m-person@3".to_string(),
            },
            NodeLabelMapping {
                label: "Org".to_string(),
                ontology_class: format!("{NS}Org"),
                mapping_id: "m-org@1".to_string(),
            },
        ],
        relationship_types: vec![RelationshipTypeMapping {
            rel_type: "WORKS_FOR".to_string(),
            from_label: "Person".to_string(),
            to_label: "Org".to_string(),
            join_mapping_ids: vec!["m-person@3".to_string(), "m-org@1".to_string()],
            join_condition: "person.org_id = org.id".to_string(),
        }],
    }
}

fn row(pairs: &[(&str, &str)]) -> ForeignRow {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// A source that counts every statement it is asked to run.
struct Recording {
    source_id: &'static str,
    table: &'static str,
    rows: Vec<ForeignRow>,
    right_rows: Vec<ForeignRow>,
    scans: Arc<AtomicUsize>,
    joins: Arc<Mutex<Vec<JoinScan>>>,
    filters: Arc<Mutex<Vec<ObdaFilter>>>,
}

impl ObdaSource for Recording {
    fn identity(&self) -> Option<SourceIdentity> {
        Some(SourceIdentity {
            source_id: self.source_id.to_string(),
            table: self.table.to_string(),
        })
    }

    fn scan(
        &self,
        needed: &BTreeSet<String>,
        filters: &[ObdaFilter],
    ) -> Result<Vec<ForeignRow>, String> {
        self.scans.fetch_add(1, Ordering::SeqCst);
        self.filters.lock().unwrap().extend(filters.iter().cloned());
        Ok(self
            .rows
            .iter()
            .filter(|row| {
                filters
                    .iter()
                    .all(|f| row.get(&f.column).is_some_and(|cell| f.matches(cell)))
            })
            .map(|row| {
                row.iter()
                    .filter(|(k, _)| needed.is_empty() || needed.contains(*k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .collect())
    }

    fn scan_join(
        &self,
        join: &JoinScan,
        needed: &BTreeSet<String>,
    ) -> Option<Result<Vec<ForeignRow>, String>> {
        self.joins.lock().unwrap().push(join.clone());
        let mut out = Vec::new();
        for left in &self.rows {
            for right in &self.right_rows {
                if left.get(&join.on_left) == right.get(&join.on_right) {
                    let mut joined: ForeignRow = left.clone();
                    for (k, v) in right {
                        joined.insert(format!("{JOIN_RIGHT_PREFIX}{k}"), v.clone());
                    }
                    joined.retain(|k, _| needed.contains(k));
                    out.push(joined);
                }
            }
        }
        Some(Ok(out))
    }
}

struct Fixture {
    vg: VirtualGraph,
    registry: ObdaSourceRegistry,
    scans: Arc<AtomicUsize>,
    joins: Arc<Mutex<Vec<JoinScan>>>,
    filters: Arc<Mutex<Vec<ObdaFilter>>>,
}

/// People and organisations; `same_source` puts both tables in one database.
fn fixture(same_source: bool) -> Fixture {
    let scans = Arc::new(AtomicUsize::new(0));
    let joins = Arc::new(Mutex::new(Vec::new()));
    let filters = Arc::new(Mutex::new(Vec::new()));
    let people = vec![
        row(&[
            ("id", "1"),
            ("name", "Ann"),
            ("age", "41"),
            ("org_id", "10"),
        ]),
        row(&[
            ("id", "2"),
            ("name", "Bob"),
            ("age", "25"),
            ("org_id", "10"),
        ]),
        row(&[("id", "3"), ("name", "Cy"), ("age", "52"), ("org_id", "11")]),
    ];
    let orgs = vec![
        row(&[("id", "10"), ("name", "Acme")]),
        row(&[("id", "11"), ("name", "Globex")]),
    ];
    let mut registry = ObdaSourceRegistry::new();
    registry.register(
        "people",
        Arc::new(Recording {
            source_id: "db-a",
            table: "people",
            rows: people,
            right_rows: orgs.clone(),
            scans: scans.clone(),
            joins: joins.clone(),
            filters: filters.clone(),
        }),
    );
    registry.register(
        "orgs",
        Arc::new(Recording {
            source_id: if same_source { "db-a" } else { "db-b" },
            table: "orgs",
            rows: orgs,
            right_rows: Vec::new(),
            scans: scans.clone(),
            joins: joins.clone(),
            filters: filters.clone(),
        }),
    );
    let mut person = TriplesMap::new("people", format!("{NS}person/{{id}}"))
        .with_class(format!("{NS}Person"))
        .add_column(format!("{NS}name"), "name")
        .add_typed_column(format!("{NS}age"), "age", XSD_INTEGER);
    if !same_source {
        // Across databases the from-endpoint map declares the relationship itself.
        person = person.add_ref(format!("{NS}WORKS_FOR"), format!("{NS}org/{{org_id}}"));
    }
    let org = TriplesMap::new("orgs", format!("{NS}org/{{id}}"))
        .with_class(format!("{NS}Org"))
        .add_column(format!("{NS}name"), "name");
    Fixture {
        vg: VirtualGraph::new().with_map(person).with_map(org),
        registry,
        scans,
        joins,
        filters,
    }
}

fn answer(fixture: &mut Fixture, text: &str) -> Vec<Vec<String>> {
    let plan = compile_view_statement(&view(), text, 8).expect("compiles");
    apply_view_joins(&mut fixture.vg, &mut fixture.registry, &plan.joins).expect("joins apply");
    let result = run_virtual(&fixture.vg, &fixture.registry, &plan.sparql).expect("evaluates");
    let (_, rows) = result.to_rows();
    let mut rows: Vec<Vec<String>> = rows
        .into_iter()
        .map(|row| row.into_iter().map(Option::unwrap_or_default).collect())
        .collect();
    rows.sort();
    rows
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R038.6.2
#[test]
fn pattern_pushes_join() {
    let mut same = fixture(true);
    let rows = answer(
        &mut same,
        "MATCH (p:Person)-[:WORKS_FOR]->(o:Org) RETURN p.name, o.name",
    );
    assert_eq!(
        rows,
        vec![
            vec!["Ann".to_string(), "Acme".to_string()],
            vec!["Bob".to_string(), "Acme".to_string()],
            vec!["Cy".to_string(), "Globex".to_string()],
        ]
    );
    // One pushed statement for the one same-source join, naming the join columns.
    let joins = same.joins.lock().unwrap().clone();
    assert_eq!(joins.len(), 1);
    assert_eq!(
        joins[0],
        JoinScan {
            right_table: "orgs".to_string(),
            on_left: "org_id".to_string(),
            on_right: "id".to_string(),
        }
    );

    // Across two databases no join is pushed; the answer is the same.
    let mut apart = fixture(false);
    let across = answer(
        &mut apart,
        "MATCH (p:Person)-[:WORKS_FOR]->(o:Org) RETURN p.name, o.name",
    );
    assert_eq!(across, rows);
    assert!(apart.joins.lock().unwrap().is_empty());

    // A comparison on a typed column reaches the source as a row-level filter.
    let mut filtered = fixture(true);
    let older = answer(
        &mut filtered,
        "MATCH (p:Person)-[:WORKS_FOR]->(o:Org) WHERE p.age > 30 AND o.name = 'Acme' \
         RETURN p.name LIMIT 5",
    );
    assert_eq!(older, vec![vec!["Ann".to_string()]]);
    assert!(filtered
        .filters
        .lock()
        .unwrap()
        .iter()
        .any(|f| f.column == "age"));

    // A bounded variable-length path compiles to alternatives of fixed paths.
    let plan = compile_view_statement(
        &view(),
        "MATCH (p:Person)-[:WORKS_FOR*1..2]->(o:Org) RETURN o",
        8,
    )
    .unwrap();
    assert!(plan.sparql.contains(&format!(
        "(<{NS}WORKS_FOR>|<{NS}WORKS_FOR>/<{NS}WORKS_FOR>)"
    )));
    assert_eq!(plan.mapping_digests, vec!["sha256:abc".to_string()]);
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R038.6.4
#[test]
fn view_refusals() {
    let source = fixture(true);
    let cases = [
        (
            "CREATE (n:Person {name: 'x'})",
            "VIRTUAL_VIEW_READ_ONLY",
            "CREATE",
        ),
        (
            "MATCH (p:Person) SET p.name = 'x'",
            "VIRTUAL_VIEW_READ_ONLY",
            "SET",
        ),
        ("MATCH (g:Ghost) RETURN g", "UNMAPPED_LABEL", "Ghost"),
        (
            "MATCH (p:Person)-[:WORKS_FOR*1..9]->(o:Org) RETURN o",
            "PATH_DEPTH_EXCEEDED",
            "*1..9",
        ),
        (
            "MATCH (p:Person) RETURN p ORDER BY p.name",
            "VIEW_CLAUSE_UNSUPPORTED",
            "ORDER BY",
        ),
        (
            "MATCH (p:Person) OPTIONAL MATCH (o:Org) RETURN p",
            "VIEW_CLAUSE_UNSUPPORTED",
            "OPTIONAL MATCH",
        ),
    ];
    for (text, code, token) in cases {
        let refusal = compile_view_statement(&view(), text, 8).unwrap_err();
        assert_eq!(refusal.code(), code, "{text}");
        assert_eq!(refusal.token(), token, "{text}");
    }
    // A read of an undeclared relationship type names the type.
    let refusal = compile_view_statement(&view(), "MATCH (p:Person)-[:OWNS]->(o:Org) RETURN o", 8)
        .unwrap_err();
    assert_eq!(refusal, ViewRefusal::UnmappedLabel("OWNS".to_string()));
    // No refusal reached a source.
    assert_eq!(source.scans.load(Ordering::SeqCst), 0);
    assert!(source.joins.lock().unwrap().is_empty());
}

struct Views(HashMap<String, GraphView>);

impl ViewResolver for Views {
    fn view(&self, name: &str) -> Option<GraphView> {
        self.0.get(name).cloned()
    }
}

#[test]
fn served_read_resolves_view() {
    let views = Views(HashMap::from([("people".to_string(), view())]));
    let plan = resolve_view_plan(&views, "people", "MATCH (p:Person) RETURN p.name", 8).unwrap();
    assert_eq!(plan.mapping_digests, vec!["sha256:abc".to_string()]);
    assert_eq!(
        plan.projection,
        vec![("p__name".to_string(), "name".to_string())]
    );
    // The plan the named view yields is the plan compiled against that view directly.
    assert_eq!(
        plan,
        compile_view_statement(&view(), "MATCH (p:Person) RETURN p.name", 8).unwrap()
    );
    // A name the caller has not registered resolves to nothing.
    let refusal =
        resolve_view_plan(&views, "other", "MATCH (p:Person) RETURN p.name", 8).unwrap_err();
    assert_eq!(refusal, ViewRefusal::UnknownView("other".to_string()));
}
