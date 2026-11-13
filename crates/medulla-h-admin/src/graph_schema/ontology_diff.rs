//! `GraphSchema.DiffOntology` behaviour (MEDULLA-TYPED-PACKS-R804.2): the
//! axiom diff between two ontology revisions and the impact of moving a graph
//! from one to the other.
//!
//! Both functions are pure over their inputs. Impact runs on an off-lock
//! snapshot of the graph with the candidate schema composed in memory; no
//! branch is created and nothing is written.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use medulla_graph_core::graph::{
    GraphSchemaSource, GraphSchemaSources, SchemaSourceOrigin, MAX_SCHEMA_DOCUMENT_TRIPLES,
};
use medulla_rdf::oxrdf::{Graph, NamedOrBlankNode, Term, Triple};
use medulla_shacl::Severity;
use medulla_types::ontology_diff::{ChangeImpact, ImpactItems, OntologyDiff};

const OWL_DEPRECATED: &str = "http://www.w3.org/2002/07/owl#deprecated";

/// One side of a diff: a revision label and its documents.
#[derive(Debug, Clone)]
pub struct SchemaSide {
    pub revision: String,
    pub ontology_ttl: String,
    pub shapes_ttl: Option<String>,
}

/// The off-lock graph snapshot an impact run reads.
pub struct ImpactSnapshot {
    /// The graph's data triples.
    pub data: Graph,
    /// `(mapping id, R2RML Turtle)` of every mapping in the graph.
    pub mappings: Vec<(String, String)>,
    /// `(stored query name, query text)` of every stored query.
    pub queries: Vec<(String, String)>,
    /// The schema-source set in force, and the key the candidate replaces.
    /// When present the candidate is composed in memory and must validate.
    pub compose: Option<(GraphSchemaSources, String)>,
}

fn parse(label: &str, document: &str) -> Result<Vec<Triple>, String> {
    medulla_rdf::mapping::parse_turtle_bounded(document, MAX_SCHEMA_DOCUMENT_TRIPLES)
        .map_err(|error| format!("ONTOLOGY_INVALID: {label}: {error}"))
}

/// Blank nodes are re-scoped on every parse, so their labels are not stable.
fn object_form(term: &Term) -> String {
    match term {
        Term::BlankNode(_) => "_:".to_string(),
        other => other.to_string(),
    }
}

/// The triples whose subject is an IRI, grouped by subject, each group sorted.
fn axioms(triples: &[Triple]) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for triple in triples {
        if let NamedOrBlankNode::NamedNode(subject) = &triple.subject {
            out.entry(subject.as_str().to_string())
                .or_default()
                .insert(format!(
                    "{} {}",
                    triple.predicate.as_str(),
                    object_form(&triple.object)
                ));
        }
    }
    out
}

fn deprecated_iris(triples: &[Triple]) -> BTreeSet<String> {
    triples
        .iter()
        .filter(|triple| triple.predicate.as_str() == OWL_DEPRECATED)
        .filter(|triple| matches!(&triple.object, Term::Literal(l) if l.value() == "true"))
        .filter_map(|triple| match &triple.subject {
            NamedOrBlankNode::NamedNode(subject) => Some(subject.as_str().to_string()),
            NamedOrBlankNode::BlankNode(_) => None,
        })
        .collect()
}

/// The canonical axiom changes from `from` to `to`.
pub fn diff_axioms(from: &SchemaSide, to: &SchemaSide) -> Result<OntologyDiff, String> {
    let from_triples = parse("from ontology", &from.ontology_ttl)?;
    let to_triples = parse("to ontology", &to.ontology_ttl)?;
    let (before, after) = (axioms(&from_triples), axioms(&to_triples));
    let added = after
        .keys()
        .filter(|iri| !before.contains_key(*iri))
        .cloned()
        .collect();
    let removed = before
        .keys()
        .filter(|iri| !after.contains_key(*iri))
        .cloned()
        .collect();
    let changed = before
        .iter()
        .filter(|(iri, set)| after.get(*iri).is_some_and(|other| other != *set))
        .map(|(iri, _)| iri.clone())
        .collect();
    let was_deprecated = deprecated_iris(&from_triples);
    let deprecated = deprecated_iris(&to_triples)
        .difference(&was_deprecated)
        .cloned()
        .collect();
    Ok(OntologyDiff {
        from_rev: from.revision.clone(),
        to_rev: to.revision.clone(),
        added,
        removed,
        changed,
        deprecated,
    })
}

fn term_iri(term: &Term) -> Option<&str> {
    match term {
        Term::NamedNode(node) => Some(node.as_str()),
        _ => None,
    }
}

fn triple_iris(triple: &Triple) -> Vec<&str> {
    let mut out = vec![triple.predicate.as_str()];
    if let NamedOrBlankNode::NamedNode(subject) = &triple.subject {
        out.push(subject.as_str());
    }
    out.extend(term_iri(&triple.object));
    out
}

/// Shapes of `shapes_ttl` that name a removed IRI, by their nearest IRI shape.
fn broken_shapes(shapes_ttl: &str, removed: &BTreeSet<String>) -> Result<BTreeSet<String>, String> {
    let triples = parse("to shapes", shapes_ttl)?;
    let mut parent: BTreeMap<String, NamedOrBlankNode> = BTreeMap::new();
    for triple in &triples {
        if let Term::BlankNode(child) = &triple.object {
            parent.insert(child.as_str().to_string(), triple.subject.clone());
        }
    }
    let owner = |mut node: NamedOrBlankNode| {
        for _ in 0..=triples.len() {
            match node {
                NamedOrBlankNode::NamedNode(iri) => return iri.as_str().to_string(),
                NamedOrBlankNode::BlankNode(blank) => match parent.get(blank.as_str()) {
                    Some(up) => node = up.clone(),
                    None => return "_:anonymous".to_string(),
                },
            }
        }
        "_:anonymous".to_string()
    };
    Ok(triples
        .iter()
        .filter(|triple| triple_iris(triple).iter().any(|iri| removed.contains(*iri)))
        .map(|triple| owner(triple.subject.clone()))
        .collect())
}

/// Ids of mappings whose parsed IRIs include a removed IRI.
fn broken_mappings(
    mappings: &[(String, String)],
    removed: &BTreeSet<String>,
) -> Result<BTreeSet<String>, String> {
    let mut out = BTreeSet::new();
    for (id, ttl) in mappings {
        let triples = parse(&format!("mapping {id}"), ttl)?;
        if triples
            .iter()
            .any(|triple| triple_iris(triple).iter().any(|iri| removed.contains(*iri)))
        {
            out.insert(id.clone());
        }
    }
    Ok(out)
}

/// Every `<iri>` written in a query text.
fn query_iris(text: &str) -> BTreeSet<&str> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        let tail = &rest[open + 1..];
        match tail.find('>') {
            Some(close) if !tail[..close].contains(char::is_whitespace) => {
                out.insert(&tail[..close]);
                rest = &tail[close + 1..];
            }
            _ => rest = tail,
        }
    }
    out
}

fn shapes_graph(shapes_ttl: Option<&str>) -> Result<Graph, String> {
    let mut graph = Graph::new();
    if let Some(document) = shapes_ttl {
        for triple in parse("shapes", document)? {
            graph.insert(&triple);
        }
    }
    Ok(graph)
}

/// Focus nodes with a `Violation` under `shapes_ttl`.
fn failing_nodes(shapes_ttl: Option<&str>, data: &Graph) -> Result<BTreeSet<String>, String> {
    let report = medulla_shacl::validate(&shapes_graph(shapes_ttl)?, data)?;
    Ok(report
        .results
        .into_iter()
        .filter(|result| result.severity == Severity::Violation)
        .map(|result| {
            result
                .focus_node
                .trim_start_matches('<')
                .trim_end_matches('>')
                .to_string()
        })
        .collect())
}

fn compose_candidate(
    sources: &GraphSchemaSources,
    key: &str,
    to: &SchemaSide,
) -> Result<(), String> {
    let source = GraphSchemaSource::new(
        SchemaSourceOrigin::Ontology {
            connector: "diff".to_string(),
            ontology_iri: key.to_string(),
            revision: 0,
            importer: String::new(),
            history: Vec::new(),
        },
        to.shapes_ttl.as_deref().map(Arc::from),
        Some(Arc::from(to.ontology_ttl.as_str())),
        0,
    )?;
    let mut staged = sources.clone();
    staged.attach_dynamic(key.to_string(), source)?;
    medulla_server_api::schema_compose::validate_entering_schema(&staged).map(|_| ())
}

/// The counts and ids of data and consumers the move from `from` to `to`
/// breaks. Reads only `snapshot`.
pub fn impact(
    from: &SchemaSide,
    to: &SchemaSide,
    snapshot: &ImpactSnapshot,
) -> Result<(ChangeImpact, ImpactItems), String> {
    if let Some((sources, key)) = &snapshot.compose {
        compose_candidate(sources, key, to)?;
    }
    let diff = diff_axioms(from, to)?;
    let removed: BTreeSet<String> = diff.removed.into_iter().collect();

    let old_failing = failing_nodes(from.shapes_ttl.as_deref(), &snapshot.data)?;
    let new_failing = failing_nodes(to.shapes_ttl.as_deref(), &snapshot.data)?;
    let instances: Vec<String> = new_failing.difference(&old_failing).cloned().collect();

    let shapes: Vec<String> = match &to.shapes_ttl {
        Some(document) => broken_shapes(document, &removed)?.into_iter().collect(),
        None => Vec::new(),
    };
    let mappings: Vec<String> = broken_mappings(&snapshot.mappings, &removed)?
        .into_iter()
        .collect();
    let queries: BTreeSet<String> = snapshot
        .queries
        .iter()
        .filter(|(_, text)| query_iris(text).iter().any(|iri| removed.contains(*iri)))
        .map(|(name, _)| name.clone())
        .collect();
    let counts = ChangeImpact {
        instances_invalid: instances.len() as u64,
        shapes_broken: shapes.len() as u64,
        mappings_broken: mappings.len() as u64,
        queries_broken: queries.len() as u64,
    };
    // MEDULLA-TYPED-PACKS-R804.5: the counts above are exact; only the listed
    // ids are cut, after sorting, to `ontology_diff.max_listed` each.
    let listed = usize::try_from(medulla_server_api::knob_config::knobs().ontology_diff_max_listed)
        .unwrap_or(usize::MAX);
    let cut = |mut ids: Vec<String>| {
        ids.truncate(listed);
        ids
    };
    let items = ImpactItems {
        instances: cut(instances),
        shapes: cut(shapes),
        mappings: cut(mappings),
        queries: cut(queries.into_iter().collect()),
    };
    Ok((counts, items))
}

const R2RML_NS: &str = "http://www.w3.org/ns/r2rml#";
const STORED_QUERY_LABEL: &str = "StoredQuery";

/// The off-lock snapshot of `core` an impact run reads: the data triples, the
/// R2RML mapping documents carried by the other attached schema sources, the
/// newest version of every stored query. `key` is the source being replaced;
/// its own documents are never read as mappings. The caller sets `compose`.
pub fn snapshot_from_core(
    core: &medulla_graph_core::graph::GraphCore,
    key: &str,
) -> Result<ImpactSnapshot, String> {
    let mut data = Graph::new();
    // Only RDF resources project; engine bookkeeping nodes (edits, change
    // requests, stored queries) carry no RDF identity and contribute nothing.
    for triple in medulla_rdf::mapping::export_view_triples(&core.analysis_snapshot())? {
        data.insert(&triple);
    }
    let sources = core.schema_sources();
    let mut mappings = Vec::new();
    for (id, source) in &sources.dynamic {
        if id == key {
            continue;
        }
        for document in [&source.ontology_ttl, &source.shapes_ttl]
            .into_iter()
            .flatten()
        {
            if document.contains(R2RML_NS) {
                mappings.push((id.clone(), document.to_string()));
            }
        }
    }
    let mut newest: BTreeMap<String, (u64, String)> = BTreeMap::new();
    for (_, bytes) in core.get_nodes_by_label(STORED_QUERY_LABEL, 0) {
        let Ok(value) = rmp_serde::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        let query = &value["query"];
        let (Some(name), Some(text)) = (query["name"].as_str(), query["text"].as_str()) else {
            continue;
        };
        let version = query["version"].as_u64().unwrap_or(0);
        let entry = newest.entry(name.to_string()).or_insert((0, String::new()));
        if version >= entry.0 {
            *entry = (version, text.to_string());
        }
    }
    let queries = newest
        .into_iter()
        .map(|(name, (_, text))| (name, text))
        .collect();
    Ok(ImpactSnapshot {
        data,
        mappings,
        queries,
        compose: None,
    })
}
