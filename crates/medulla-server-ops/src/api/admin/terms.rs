//! `Method::GraphSchemaClasses` (EH-389): the declared vocabulary of the
//! request graph's composed schema.
//!
//! Terms are read from the same keyed sources composition reads, in the same
//! order (core before dynamic, then by key), so a term's `source_id` is the
//! source that owns it: a dynamic source may only restate a core declaration
//! verbatim (`compose::validate_dynamic_declarations`), never take it over.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use medulla_catalog_types::{
    graph_schema::term_local_name, graph_schema::validate_term_page_limit,
    graph_schema::GraphSchemaClassesView, graph_schema::GraphSchemaInterfaceView,
    graph_schema::GraphSchemaTermKind, graph_schema::GraphSchemaTermPosition,
    graph_schema::GraphSchemaTermView, graph_schema::GRAPH_SCHEMA_RESULT_SCHEMA_VERSION,
    graph_schema::MAX_GRAPH_SCHEMA_INTERFACE_REPORTS,
};
use medulla_rdf::oxrdf::{NamedOrBlankNode, Term, Triple};

use medulla_graph_core::graph::GraphCore;
use medulla_graph_core::graph::GraphSchemaSources;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const INTERFACE_TYPE: &str = medulla_types::ontology_iri!("core#Interface");
const RDFS_SUBCLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";

/// The declared terms of a composition and the direct superclasses of its classes.
struct DeclaredVocabulary {
    terms: BTreeMap<GraphSchemaTermPosition, String>,
    superclasses: BTreeMap<String, BTreeSet<String>>,
}

/// The interface markers of the composed schema: which classes are interfaces and which
/// interfaces each class implements (MEDULLA-TYPED-PACKS-R650.8).
struct InterfaceMarks {
    interfaces: BTreeSet<String>,
    implements: BTreeMap<String, Vec<String>>,
}

impl InterfaceMarks {
    fn of(model: &medulla_core::ontology::object_model::ObjectModel) -> Self {
        let mut implements: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for implementation in &model.implementations {
            implements
                .entry(implementation.class.clone())
                .or_default()
                .insert(implementation.interface.clone());
        }
        Self {
            interfaces: model
                .interfaces
                .iter()
                .map(|interface| interface.name.clone())
                .collect(),
            implements: implements
                .into_iter()
                .map(|(class, interfaces)| (class, interfaces.into_iter().collect()))
                .collect(),
        }
    }
}

/// One page request, as the method carries it.
pub struct TermPageRequest<'a> {
    /// Restrict the page to one term kind.
    pub kind: Option<GraphSchemaTermKind>,
    /// Position after which the page starts.
    pub cursor: Option<&'a str>,
    /// Maximum number of terms in the page.
    pub limit: u32,
}

/// Answer one `GraphSchemaClasses` page for the request graph.
pub async fn handle_classes(
    req_id: u64,
    graph_name: &str,
    core: &Arc<GraphCore>,
    request: TermPageRequest<'_>,
) -> Response {
    match classes_page(
        graph_name,
        &super::schema_read::disclosed_sources(core),
        &request,
    )
    .and_then(|view| with_node_counts(view, core))
    {
        Ok(view) => Response::ok(
            req_id,
            ResultPayload::of::<
                medulla_protocol_types::result_contract::reasoning::GraphSchemaClasses,
            >(view),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

/// The class, property and relation terms of the request graph's composed
/// schema (the core plus every admitted pack): the one vocabulary a domain
/// pack's mappings resolve against, read through the same sources and
/// declarations `GraphSchemaClasses` serves.
pub fn composed_vocabulary(
    sources: &GraphSchemaSources,
) -> Result<medulla_catalog_types::domain_pack::ComposedVocabulary, String> {
    let terms = declared_terms(sources)?;
    Ok(
        medulla_catalog_types::domain_pack::ComposedVocabulary::from_terms(
            terms
                .keys()
                .map(|position| (position.iri.as_str(), position.kind)),
        ),
    )
}

/// The nodes of the graph typed with each class term of the page (MEDULLA-TYPED-PACKS-R106.5).
fn with_node_counts(
    mut view: GraphSchemaClassesView,
    core: &GraphCore,
) -> Result<GraphSchemaClassesView, String> {
    const COUNT_PAGE: usize = 1000;
    let mut terms = view.terms.as_slice().to_vec();
    for term in terms
        .iter_mut()
        .filter(|term| term.kind == GraphSchemaTermKind::Class)
    {
        let mut count = 0u64;
        let mut after: Option<String> = None;
        loop {
            let page = core.get_nodes_by_label_page(&term.iri, after.as_deref(), COUNT_PAGE);
            count += page.len() as u64;
            if page.len() < COUNT_PAGE {
                break;
            }
            after = page.last().map(|(id, _)| id.clone());
        }
        term.node_count = Some(count);
    }
    view.terms = medulla_types::contract::BoundedVec::new(terms)?;
    Ok(view)
}

fn classes_page(
    graph_name: &str,
    sources: &GraphSchemaSources,
    request: &TermPageRequest<'_>,
) -> Result<GraphSchemaClassesView, String> {
    let limit = validate_term_page_limit(request.limit)?;
    let after = request
        .cursor
        .map(GraphSchemaTermPosition::parse)
        .transpose()?;
    let DeclaredVocabulary {
        terms,
        superclasses,
    } = scan_vocabulary(sources)?;
    let model = object_model(sources)?;
    let marks = InterfaceMarks::of(&model);
    let matching: Vec<(&GraphSchemaTermPosition, &String)> = terms
        .iter()
        .filter(|(position, _)| request.kind.is_none_or(|kind| kind == position.kind))
        .collect();
    let total_terms = matching.len() as u64;
    let mut page: Vec<GraphSchemaTermView> = matching
        .into_iter()
        .filter(|(position, _)| after.as_ref().is_none_or(|after| *position > after))
        .take(limit + 1)
        .map(|(position, source_id)| term_view(position, source_id, sources, &superclasses, &marks))
        .collect();
    let next_cursor = (page.len() > limit).then(|| {
        page.truncate(limit);
        let last = page.last().expect("a page over its limit is non-empty");
        GraphSchemaTermPosition {
            iri: last.iri.clone(),
            kind: last.kind,
        }
        .cursor()
    });
    Ok(GraphSchemaClassesView {
        schema_version: GRAPH_SCHEMA_RESULT_SCHEMA_VERSION,
        graph: graph_name.to_string(),
        composed_digest: sources.composed_digest().to_hex(),
        total_terms,
        terms: medulla_types::contract::BoundedVec::new(page)?,
        interfaces: interface_reports(&model)?,
        next_cursor,
    })
}

/// The object model of the composed schema: one extraction path, the model the pack import
/// compiles its shapes from (MEDULLA-TYPED-PACKS-R650.7).
fn object_model(
    sources: &GraphSchemaSources,
) -> Result<medulla_core::ontology::object_model::ObjectModel, String> {
    let documents: Vec<&str> = sources.ontologies().map(|(_, text)| text).collect();
    medulla_rdf::pack::extract_object_model(&documents).map_err(|error| {
        format!(
            "OBJECT_MODEL_INVALID: {}: {}",
            error.subject(),
            error.reason()
        )
    })
}

/// Every `:implementsInterface` the composed schema declares, with the required
/// members the implementing class lacks (MEDULLA-TYPED-PACKS-R650.3).
fn interface_reports(
    model: &medulla_core::ontology::object_model::ObjectModel,
) -> Result<Vec<GraphSchemaInterfaceView>, String> {
    use medulla_core::ontology::object_model::interface_gaps;

    let mut missing: BTreeMap<(String, String), Vec<String>> = model
        .implementations
        .iter()
        .map(|implementation| {
            (
                (
                    implementation.class.clone(),
                    implementation.interface.clone(),
                ),
                Vec::new(),
            )
        })
        .collect();
    for gap in interface_gaps(model) {
        missing
            .entry((gap.class, gap.interface))
            .or_default()
            .push(gap.member);
    }
    if missing.len() > MAX_GRAPH_SCHEMA_INTERFACE_REPORTS {
        return Err(format!(
            "GRAPH_SCHEMA_TOO_LARGE: more than {MAX_GRAPH_SCHEMA_INTERFACE_REPORTS} interface implementations"
        ));
    }
    Ok(missing
        .into_iter()
        .map(|((class, interface), mut missing)| {
            missing.sort();
            missing.dedup();
            GraphSchemaInterfaceView {
                class,
                interface,
                missing,
            }
        })
        .collect())
}

fn term_view(
    position: &GraphSchemaTermPosition,
    source_id: &str,
    sources: &GraphSchemaSources,
    superclasses: &BTreeMap<String, BTreeSet<String>>,
    marks: &InterfaceMarks,
) -> GraphSchemaTermView {
    let is_class = position.kind == GraphSchemaTermKind::Class;
    let superclasses = match position.kind {
        GraphSchemaTermKind::Class => superclasses
            .get(&position.iri)
            .map(|direct| direct.iter().cloned().collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    GraphSchemaTermView {
        iri: position.iri.clone(),
        local_name: term_local_name(&position.iri).to_string(),
        kind: position.kind,
        source_id: source_id.to_string(),
        ontology_revision: active_ontology_revision(sources, source_id),
        node_count: None,
        superclasses,
        interface: is_class && marks.interfaces.contains(&position.iri),
        implements: if is_class {
            marks
                .implements
                .get(&position.iri)
                .cloned()
                .unwrap_or_default()
        } else {
            Vec::new()
        },
    }
}

/// The active runtime-ontology revision of `source_id` (MEDULLA-TYPED-PACKS-R103.5).
/// Only the active revision composes, so a term a later revision dropped is
/// never listed with an older revision number.
fn active_ontology_revision(sources: &GraphSchemaSources, source_id: &str) -> Option<u64> {
    match sources.dynamic.get(source_id).map(|source| &source.origin) {
        Some(medulla_graph_core::graph::SchemaSourceOrigin::Ontology { revision, .. }) => {
            Some(*revision)
        }
        _ => None,
    }
}

/// Every named term the sources' ontology documents declare, keyed by
/// `(iri, kind)`, mapped to the first source declaring it.
fn declared_terms(
    sources: &GraphSchemaSources,
) -> Result<BTreeMap<GraphSchemaTermPosition, String>, String> {
    scan_vocabulary(sources).map(|vocabulary| vocabulary.terms)
}

/// One parse of every source: the declared terms and, per subject, the named
/// `rdfs:subClassOf` objects any source states (MEDULLA-TYPED-PACKS-R812.1). A
/// restriction (a blank-node object) is not a named superclass and is skipped.
fn scan_vocabulary(sources: &GraphSchemaSources) -> Result<DeclaredVocabulary, String> {
    let mut terms = BTreeMap::new();
    let mut superclasses: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (source_id, document) in sources.ontologies() {
        let triples = medulla_rdf::mapping::parse_turtle(document)
            .map_err(|error| format!("ONTOLOGY_INVALID: source '{source_id}': {error}"))?;
        for triple in &triples {
            if let Some(position) = declared_position(triple) {
                terms
                    .entry(position)
                    .or_insert_with(|| source_id.to_string());
            } else if let Some((subject, object)) = subclass_pair(triple) {
                superclasses.entry(subject).or_default().insert(object);
            }
        }
    }
    Ok(DeclaredVocabulary {
        terms,
        superclasses,
    })
}

fn subclass_pair(triple: &Triple) -> Option<(String, String)> {
    if triple.predicate.as_str() != RDFS_SUBCLASS_OF {
        return None;
    }
    let (NamedOrBlankNode::NamedNode(subject), Term::NamedNode(object)) =
        (&triple.subject, &triple.object)
    else {
        return None;
    };
    Some((subject.as_str().to_string(), object.as_str().to_string()))
}

fn declared_position(triple: &Triple) -> Option<GraphSchemaTermPosition> {
    if triple.predicate.as_str() != RDF_TYPE {
        return None;
    }
    let (NamedOrBlankNode::NamedNode(subject), Term::NamedNode(object)) =
        (&triple.subject, &triple.object)
    else {
        return None;
    };
    // A subject typed `:Interface` is a class term even when it carries no `owl:Class` typing,
    // so every interface the object model names is a row a client can list.
    let kind = if object.as_str() == INTERFACE_TYPE {
        GraphSchemaTermKind::Class
    } else {
        GraphSchemaTermKind::declared_by(object.as_str())?
    };
    Some(GraphSchemaTermPosition {
        iri: subject.as_str().to_string(),
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_graph_core::graph::GraphSchemaSource;
    use medulla_graph_core::graph::SchemaSourceOrigin;

    const ADMIN_ONTOLOGY: &str = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
        <https://example.org/pack#Widget> a owl:Class .\n\
        <https://example.org/pack#Gadget> a rdfs:Class .\n\
        <https://example.org/pack#partOf> a owl:ObjectProperty .\n";

    fn with_admin_source() -> GraphSchemaSources {
        let source = GraphSchemaSource::new(
            SchemaSourceOrigin::Admin {
                name: "widgets".to_string(),
            },
            None,
            Some(Arc::from(ADMIN_ONTOLOGY)),
            0,
        )
        .expect("a bounded admin source");
        GraphSchemaSources::with_dynamic(BTreeMap::from([("admin:widgets".to_string(), source)]))
            .expect("the admin source composes with the core catalog")
    }

    fn request(
        kind: Option<GraphSchemaTermKind>,
        cursor: Option<&str>,
        limit: u32,
    ) -> TermPageRequest<'_> {
        TermPageRequest {
            kind,
            cursor,
            limit,
        }
    }

    fn walk(
        sources: &GraphSchemaSources,
        kind: Option<GraphSchemaTermKind>,
        limit: u32,
    ) -> Vec<GraphSchemaClassesView> {
        let mut pages = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let page = classes_page("g", sources, &request(kind, cursor.as_deref(), limit))
                .expect("every page of a valid walk answers");
            cursor = page.next_cursor.clone();
            pages.push(page);
            if cursor.is_none() {
                return pages;
            }
        }
    }

    #[test]
    fn the_core_catalog_declares_the_canonical_classes_under_core_sources() {
        let sources = GraphSchemaSources::default();
        let terms = declared_terms(&sources).expect("the core catalog parses");
        let classes: Vec<_> = terms
            .iter()
            .filter(|(position, _)| position.kind == GraphSchemaTermKind::Class)
            .collect();
        assert!(classes.len() > 100, "the core catalog declares its classes");
        assert!(classes
            .iter()
            .all(|(_, source)| source.starts_with("core:")));
        assert!(classes
            .iter()
            .any(|(position, _)| term_local_name(&position.iri) == "Document"));
    }

    const SUBCLASS_ONTOLOGY: &str = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
        <https://example.org/pack#Base> a owl:Class .\n\
        <https://example.org/pack#Other> a owl:Class .\n\
        <https://example.org/pack#Leaf> a owl:Class ;\n\
            rdfs:subClassOf <https://example.org/pack#Other> , <https://example.org/pack#Base> ,\n\
                [ a owl:Restriction ; owl:onProperty <https://example.org/pack#partOf> ;\n\
                  owl:someValuesFrom <https://example.org/pack#Base> ] .\n\
        <https://example.org/pack#partOf> a owl:ObjectProperty .\n";

    // spec: MEDULLA-TYPED-PACKS-R812.1
    #[test]
    fn a_class_term_lists_its_direct_named_superclasses_sorted() {
        let source = GraphSchemaSource::new(
            SchemaSourceOrigin::Admin {
                name: "tree".to_string(),
            },
            None,
            Some(Arc::from(SUBCLASS_ONTOLOGY)),
            0,
        )
        .expect("a bounded admin source");
        let sources =
            GraphSchemaSources::with_dynamic(BTreeMap::from([("admin:tree".to_string(), source)]))
                .expect("the source composes with the core catalog");
        let page = classes_page("g", &sources, &request(None, None, 1000))
            .expect("a first page")
            .terms
            .as_slice()
            .to_vec();
        let term = |iri: &str| {
            page.iter()
                .find(|term| term.iri == iri)
                .unwrap_or_else(|| panic!("{iri} is listed"))
        };
        assert_eq!(
            term("https://example.org/pack#Leaf").superclasses,
            [
                "https://example.org/pack#Base".to_string(),
                "https://example.org/pack#Other".to_string()
            ],
            "the restriction is not a named superclass"
        );
        assert!(term("https://example.org/pack#Base")
            .superclasses
            .is_empty());
        assert!(
            term("https://example.org/pack#partOf")
                .superclasses
                .is_empty(),
            "a property term carries no superclasses"
        );
    }

    const INTERFACE_ONTOLOGY: &str = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
        @prefix kg: <https://knuckles-team.github.io/ontology/core#> .\n\
        <https://example.org/pack#Named> a kg:Interface .\n\
        <https://example.org/pack#Person> a owl:Class ;\n\
            kg:implementsInterface <https://example.org/pack#Named> .\n\
        <https://example.org/pack#Robot> a owl:Class ;\n\
            kg:implementsInterface <https://example.org/pack#Named> .\n\
        <https://example.org/pack#Plain> a owl:Class .\n\
        <https://example.org/pack#partOf> a owl:ObjectProperty .\n";

    // spec: MEDULLA-TYPED-PACKS-R650.8
    #[test]
    fn a_class_row_marks_its_interface_and_lists_the_interfaces_it_implements() {
        let source = GraphSchemaSource::new(
            SchemaSourceOrigin::Admin {
                name: "ifaces".to_string(),
            },
            None,
            Some(Arc::from(INTERFACE_ONTOLOGY)),
            0,
        )
        .expect("a bounded admin source");
        let sources = GraphSchemaSources::with_dynamic(BTreeMap::from([(
            "admin:ifaces".to_string(),
            source,
        )]))
        .expect("the source composes with the core catalog");
        let page = classes_page("g", &sources, &request(None, None, 1000))
            .expect("a first page")
            .terms
            .as_slice()
            .to_vec();
        let term = |iri: &str| {
            page.iter()
                .find(|term| term.iri == iri)
                .unwrap_or_else(|| panic!("{iri} is listed"))
        };
        let named = term("https://example.org/pack#Named");
        assert!(
            named.interface,
            "an :Interface is a class row marked interface"
        );
        assert_eq!(named.kind, GraphSchemaTermKind::Class);
        assert!(named.implements.is_empty());
        for implementer in ["Person", "Robot"] {
            let row = term(&format!("https://example.org/pack#{implementer}"));
            assert!(!row.interface);
            assert_eq!(
                row.implements,
                ["https://example.org/pack#Named".to_string()]
            );
        }
        let plain = term("https://example.org/pack#Plain");
        assert!(!plain.interface && plain.implements.is_empty());
        let property = term("https://example.org/pack#partOf");
        assert!(!property.interface && property.implements.is_empty());
    }

    // spec: MEDULLA-TYPED-PACKS-R812.1
    #[test]
    fn a_core_class_rolls_up_through_its_declared_superclass() {
        let sources = GraphSchemaSources::default();
        let page = classes_page(
            "g",
            &sources,
            &request(Some(GraphSchemaTermKind::Class), None, 1000),
        )
        .expect("a first page");
        let terms = page.terms.as_slice();
        assert!(
            terms.iter().any(|term| !term.superclasses.is_empty()),
            "the core catalog states at least one rdfs:subClassOf"
        );
        let iris: std::collections::BTreeSet<&str> =
            terms.iter().map(|term| term.iri.as_str()).collect();
        assert!(
            terms
                .iter()
                .flat_map(|term| term.superclasses.iter())
                .any(|superclass| iris.contains(superclass.as_str())),
            "a superclass IRI names a class the vocabulary declares"
        );
    }

    // spec: MEDULLA-TYPED-PACKS-R077
    #[test]
    fn an_attached_source_contributes_its_terms_with_its_own_source_id() {
        let sources = with_admin_source();
        let page = classes_page(
            "g",
            &sources,
            &request(Some(GraphSchemaTermKind::Class), None, 1000),
        )
        .expect("a first page");
        let widget = page
            .terms
            .as_slice()
            .iter()
            .find(|term| term.local_name == "Widget")
            .expect("the attached class is listed");
        assert_eq!(widget.source_id, "admin:widgets");
        assert!(page
            .terms
            .as_slice()
            .iter()
            .any(|term| term.iri == "https://example.org/pack#Gadget"));
        assert!(page
            .terms
            .as_slice()
            .iter()
            .all(|term| term.kind == GraphSchemaTermKind::Class));
        assert_eq!(page.composed_digest, sources.composed_digest().to_hex());
    }

    // spec: MEDULLA-TYPED-PACKS-R077
    #[test]
    fn a_paged_walk_returns_every_term_once_in_order_at_one_digest() {
        let sources = with_admin_source();
        let pages = walk(&sources, None, 97);
        assert!(
            pages.len() > 1,
            "a 97-term page cannot hold the core catalog"
        );
        let walked: Vec<_> = pages
            .iter()
            .flat_map(|page| page.terms.as_slice().iter().cloned())
            .collect();
        let total = pages[0].total_terms;
        assert_eq!(walked.len() as u64, total);
        assert!(pages.iter().all(|page| page.total_terms == total));
        assert!(pages
            .iter()
            .all(|page| page.composed_digest == pages[0].composed_digest));
        let keys: Vec<_> = walked
            .iter()
            .map(|term| (term.iri.clone(), term.kind))
            .collect();
        let mut sorted = keys.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            keys, sorted,
            "strictly ordered by (iri, kind), no duplicates"
        );
        let classes: u64 = walk(&sources, Some(GraphSchemaTermKind::Class), 1000)
            .iter()
            .map(|page| page.terms.len() as u64)
            .sum();
        let walked_classes = walked
            .iter()
            .filter(|term| term.kind == GraphSchemaTermKind::Class)
            .count() as u64;
        assert_eq!(
            classes, walked_classes,
            "the kind filter keeps exactly one kind"
        );
    }

    #[test]
    fn a_foreign_cursor_and_an_unbounded_limit_are_refused_by_code() {
        let sources = GraphSchemaSources::default();
        let cursor =
            classes_page("g", &sources, &request(None, Some("not a cursor"), 10)).unwrap_err();
        assert!(cursor.starts_with("SCHEMA_TERM_CURSOR_INVALID"), "{cursor}");
        let limit = classes_page("g", &sources, &request(None, None, 0)).unwrap_err();
        assert!(limit.starts_with("SCHEMA_TERM_PAGE_INVALID"), "{limit}");
    }
}
