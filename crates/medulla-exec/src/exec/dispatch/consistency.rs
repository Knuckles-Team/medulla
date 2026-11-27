//! The typed consistency-violation matrix (MEDULLA-FEDERATED-QUERY-R005.1).
//!
//! One more integrity shape on the R005.700 route: `VALIDATE SHAPE
//! 'integrity:consistency[:<ontology_revision_hex>]'`. The declared axioms come from the
//! `ontology_ttl` documents of the queried graph's composed schema sources (core, then
//! dynamic); the facts come from the same snapshot the rest of the plan reads. Five checks,
//! each reported once per independent occurrence:
//!
//! 1. `self_loop`: an `owl:IrreflexiveProperty` with a live edge `x -P-> x`.
//! 2. `asymmetry`: an `owl:AsymmetricProperty` with live edges `a -P-> b` and `b -P-> a`.
//! 3. `cardinality`: a class restriction (`owl:minCardinality`, `owl:maxCardinality`,
//!    `owl:qualifiedCardinality`) or an `owl:FunctionalProperty` whose count of distinct live
//!    undated targets lies outside the declared bound.
//! 4. `temporal_conflict`: an `owl:FunctionalProperty` with two live dated edges from one
//!    subject to different targets whose validity intervals overlap.
//! 5. `ontology_defect`: a node typed with two classes the ontology declares disjoint.
//!
//! The stage only reads. It never chooses a winner and never retracts a fact.

use std::collections::{BTreeMap, BTreeSet};

use medulla_graph_core::graph::{GraphSchemaSources, GraphView};
use medulla_rdf::oxrdf::{NamedOrBlankNode, Term, Triple};
use medulla_rdf::sparql::Projection;
use medulla_types::shape_report::{Violation, ViolationDetail};

use crate::exec::PlanCtx;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDFS_SUB_CLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const OWL_THING: &str = "http://www.w3.org/2002/07/owl#Thing";
const OWL_IRREFLEXIVE: &str = "http://www.w3.org/2002/07/owl#IrreflexiveProperty";
const OWL_ASYMMETRIC: &str = "http://www.w3.org/2002/07/owl#AsymmetricProperty";
const OWL_FUNCTIONAL: &str = "http://www.w3.org/2002/07/owl#FunctionalProperty";
const OWL_ALL_DISJOINT: &str = "http://www.w3.org/2002/07/owl#AllDisjointClasses";
const OWL_DISJOINT_WITH: &str = "http://www.w3.org/2002/07/owl#disjointWith";
const OWL_MEMBERS: &str = "http://www.w3.org/2002/07/owl#members";
const OWL_ON_PROPERTY: &str = "http://www.w3.org/2002/07/owl#onProperty";
const OWL_MIN_CARDINALITY: &str = "http://www.w3.org/2002/07/owl#minCardinality";
const OWL_MAX_CARDINALITY: &str = "http://www.w3.org/2002/07/owl#maxCardinality";
const OWL_QUALIFIED_CARDINALITY: &str = "http://www.w3.org/2002/07/owl#qualifiedCardinality";

/// The longest `owl:members` list read for one `owl:AllDisjointClasses` axiom.
const MAX_LIST_MEMBERS: usize = 10_000;

/// The composed ontology digest of the queried graph, lowercase hex: the revision every
/// violation is pinned to and the value a caller may pin in the shape reference.
pub(super) fn ontology_revision(ctx: &PlanCtx) -> String {
    ctx.view.schema_sources.composed_digest().to_hex()
}

/// A declared cardinality bound on one `(class, property)`.
#[derive(Debug, Default, Clone, Copy)]
struct Bounds {
    min: Option<u32>,
    max: Option<u32>,
}

/// The axioms the five checks read.
#[derive(Debug, Default)]
struct Axioms {
    irreflexive: BTreeSet<String>,
    asymmetric: BTreeSet<String>,
    functional: BTreeSet<String>,
    bounds: BTreeMap<(String, String), Bounds>,
    /// Disjoint class pairs, smaller IRI first.
    disjoint: BTreeSet<(String, String)>,
}

impl Axioms {
    fn properties(&self) -> BTreeSet<&str> {
        self.irreflexive
            .iter()
            .chain(&self.asymmetric)
            .chain(&self.functional)
            .map(String::as_str)
            .chain(self.bounds.keys().map(|(_, property)| property.as_str()))
            .collect()
    }
}

/// One live edge of a property an axiom names.
#[derive(Debug, Clone)]
struct LiveEdge {
    /// `"{source}|{target}|{ord}"`, the id of the edge entry.
    id: String,
    source: String,
    target: String,
    property: String,
    /// `valid_from`, when the edge is dated.
    from: Option<i64>,
    /// `valid_until`, `i64::MAX` when absent.
    to: i64,
}

/// Every violation of the composed ontology's axioms over the snapshot behind `ctx`, grouped
/// by focus node and ordered `(kind, axiom_id)` within each focus.
pub(super) fn collect(
    ctx: &PlanCtx,
    projection: &Projection,
) -> Result<BTreeMap<String, Vec<Violation>>, String> {
    let revision = ontology_revision(ctx);
    let axioms = read_axioms(&ctx.view.schema_sources)?;
    let mut found: BTreeMap<String, Vec<Violation>> = BTreeMap::new();
    let mut push = |focus: &str, axiom_id: String, detail: ViolationDetail| {
        found.entry(focus.to_string()).or_default().push(Violation {
            focus: focus.to_string(),
            ontology_revision: revision.clone(),
            axiom_id,
            detail,
        });
    };

    let edges = live_edges(ctx.view, projection, &axioms.properties());
    let mut by_property_source: BTreeMap<(&str, &str), Vec<&LiveEdge>> = BTreeMap::new();
    let mut directed: BTreeMap<(&str, &str, &str), &LiveEdge> = BTreeMap::new();
    for edge in &edges {
        by_property_source
            .entry((edge.property.as_str(), edge.source.as_str()))
            .or_default()
            .push(edge);
        directed
            .entry((
                edge.property.as_str(),
                edge.source.as_str(),
                edge.target.as_str(),
            ))
            .or_insert(edge);
    }

    // (1) self_loop
    for edge in &edges {
        if edge.source == edge.target && axioms.irreflexive.contains(&edge.property) {
            push(
                &edge.source,
                format!("irreflexive:{}", edge.property),
                ViolationDetail::SelfLoop {
                    property: edge.property.clone(),
                    edge: edge.id.clone(),
                },
            );
        }
    }

    // (2) asymmetry, once per pair with a < b
    for ((property, a, b), forward) in &directed {
        if a < b && axioms.asymmetric.contains(*property) {
            if let Some(reverse) = directed.get(&(*property, *b, *a)) {
                push(
                    a,
                    format!("asymmetric:{property}"),
                    ViolationDetail::Asymmetry {
                        property: (*property).to_string(),
                        subject: (*a).to_string(),
                        object: (*b).to_string(),
                        forward_edge: forward.id.clone(),
                        reverse_edge: reverse.id.clone(),
                    },
                );
            }
        }
    }

    // (3) functional cardinality and (4) temporal conflict
    for ((property, source), group) in &by_property_source {
        if !axioms.functional.contains(*property) {
            continue;
        }
        let undated: Vec<&LiveEdge> = group.iter().copied().filter(|e| e.from.is_none()).collect();
        let targets: BTreeSet<&str> = undated.iter().map(|e| e.target.as_str()).collect();
        if targets.len() > 1 {
            push(
                source,
                format!("functional:{property}"),
                ViolationDetail::Cardinality {
                    class: OWL_THING.to_string(),
                    property: (*property).to_string(),
                    min: None,
                    max: Some(1),
                    observed: u32::try_from(targets.len()).unwrap_or(u32::MAX),
                    edges: undated.iter().map(|e| e.id.clone()).collect(),
                },
            );
        }
        let dated: Vec<&LiveEdge> = group.iter().copied().filter(|e| e.from.is_some()).collect();
        for (index, first) in dated.iter().enumerate() {
            for second in &dated[index + 1..] {
                let (Some(first_from), Some(second_from)) = (first.from, second.from) else {
                    continue;
                };
                let overlap = first_from < second.to && second_from < first.to;
                if first.target != second.target && overlap {
                    let (low, high) = if first.id <= second.id {
                        (first, second)
                    } else {
                        (second, first)
                    };
                    push(
                        source,
                        format!("functional:{property}"),
                        ViolationDetail::TemporalConflict {
                            property: (*property).to_string(),
                            first_edge: low.id.clone(),
                            second_edge: high.id.clone(),
                            first: (low.from.unwrap_or(i64::MIN), low.to),
                            second: (high.from.unwrap_or(i64::MIN), high.to),
                        },
                    );
                }
            }
        }
    }

    // (3) class cardinality and (5) ontology defect read the nodes' classes
    if !axioms.bounds.is_empty() || !axioms.disjoint.is_empty() {
        let classes = node_classes(ctx.view, projection)?;
        for id in ctx.view.node_map.keys() {
            let Some(own) = classes.get(&projection.node_iri(id)) else {
                continue;
            };
            for ((class, property), bounds) in &axioms.bounds {
                if !own.contains(class) {
                    continue;
                }
                let undated: Vec<&LiveEdge> = by_property_source
                    .get(&(property.as_str(), id.as_str()))
                    .map(|group| group.iter().copied().filter(|e| e.from.is_none()).collect())
                    .unwrap_or_default();
                let targets: BTreeSet<&str> = undated.iter().map(|e| e.target.as_str()).collect();
                let observed = u32::try_from(targets.len()).unwrap_or(u32::MAX);
                let below = bounds.min.is_some_and(|min| observed < min);
                let above = bounds.max.is_some_and(|max| observed > max);
                if below || above {
                    push(
                        id,
                        format!("cardinality:{class}:{property}"),
                        ViolationDetail::Cardinality {
                            class: class.clone(),
                            property: property.clone(),
                            min: bounds.min,
                            max: bounds.max,
                            observed,
                            edges: undated.iter().map(|e| e.id.clone()).collect(),
                        },
                    );
                }
            }
            let ordered: Vec<&String> = own.iter().collect();
            for (index, low) in ordered.iter().enumerate() {
                for high in &ordered[index + 1..] {
                    if axioms.disjoint.contains(&((*low).clone(), (*high).clone())) {
                        push(
                            id,
                            format!("disjoint:{low}:{high}"),
                            ViolationDetail::OntologyDefect {
                                affected_terms: vec![(*low).clone(), (*high).clone()],
                            },
                        );
                    }
                }
            }
        }
    }

    for violations in found.values_mut() {
        violations.sort_by(|a, b| {
            a.kind()
                .cmp(&b.kind())
                .then_with(|| a.axiom_id.cmp(&b.axiom_id))
                .then_with(|| a.detail.cmp(&b.detail))
        });
    }
    Ok(found)
}

/// Every live edge whose property (through `projection`) one of the axioms names. An edge
/// carrying `tx_to` is closed history. Entries are visited in `(source, target)` order, so
/// the result does not depend on hash order.
fn live_edges(
    view: &GraphView,
    projection: &Projection,
    properties: &BTreeSet<&str>,
) -> Vec<LiveEdge> {
    let sorted: BTreeMap<_, _> = view.edge_properties.iter().collect();
    let mut out = Vec::new();
    for ((source, target), blobs) in sorted {
        for (ord, blob) in blobs.iter().enumerate() {
            let Ok(props) = medulla_types::msgpack::decode_property_object(blob) else {
                continue;
            };
            let Some(relationship) = props.get("relationship").and_then(|v| v.as_str()) else {
                continue;
            };
            let property = projection.pred_iri(relationship);
            if !properties.contains(property.as_str()) {
                continue;
            }
            if props.get("tx_to").is_some_and(|v| !v.is_null()) {
                continue;
            }
            out.push(LiveEdge {
                id: format!("{source}|{target}|{ord}"),
                source: source.clone(),
                target: target.clone(),
                property,
                from: props.get("valid_from").and_then(serde_json::Value::as_i64),
                to: props
                    .get("valid_until")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(i64::MAX),
            });
        }
    }
    out
}

/// The classes of every RDF subject of the snapshot: the objects of its `rdf:type` triples,
/// keyed by the projected node key (`<iri>` or `_:label`).
fn node_classes(
    view: &GraphView,
    projection: &Projection,
) -> Result<BTreeMap<String, BTreeSet<String>>, String> {
    let triples = medulla_rdf::mapping::export_view_projected(view, projection)
        .map_err(|error| format!("INVALID_ARGUMENT: project the graph to RDF: {error}"))?;
    let mut classes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for triple in &triples {
        if triple.predicate.as_str() != RDF_TYPE {
            continue;
        }
        if let Term::NamedNode(class) = &triple.object {
            let subject = match &triple.subject {
                NamedOrBlankNode::NamedNode(node) => format!("<{}>", node.as_str()),
                NamedOrBlankNode::BlankNode(node) => format!("_:{}", node.as_str()),
            };
            classes
                .entry(subject)
                .or_default()
                .insert(class.as_str().to_string());
        }
    }
    Ok(classes)
}

/// Read the axioms of every `ontology_ttl` document, core then dynamic. Each document is
/// parsed once, with its blank nodes scoped to it.
fn read_axioms(sources: &GraphSchemaSources) -> Result<Axioms, String> {
    let mut axioms = Axioms::default();
    for source in sources.core.values().chain(sources.dynamic.values()) {
        let Some(document) = source.ontology_ttl.as_deref() else {
            continue;
        };
        let triples = medulla_rdf::mapping::parse_turtle(document)
            .map_err(|error| format!("INVALID_ARGUMENT: ontology document: {error}"))?;
        read_document(&triples, &mut axioms);
    }
    Ok(axioms)
}

fn subject_key(node: &NamedOrBlankNode) -> String {
    match node {
        NamedOrBlankNode::NamedNode(node) => node.as_str().to_string(),
        NamedOrBlankNode::BlankNode(node) => format!("_:{}", node.as_str()),
    }
}

fn object_key(term: &Term) -> Option<String> {
    match term {
        Term::NamedNode(node) => Some(node.as_str().to_string()),
        Term::BlankNode(node) => Some(format!("_:{}", node.as_str())),
        _ => None,
    }
}

fn is_blank(key: &str) -> bool {
    key.starts_with("_:")
}

fn ordered_pair(a: &str, b: &str) -> Option<(String, String)> {
    if a == b || is_blank(a) || is_blank(b) {
        return None;
    }
    Some(if a < b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    })
}

/// The members of an RDF list from `head`, bounded by [`MAX_LIST_MEMBERS`].
fn list_members(
    head: &str,
    first: &BTreeMap<String, String>,
    rest: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut members = Vec::new();
    let mut node = head;
    while members.len() < MAX_LIST_MEMBERS {
        let Some(member) = first.get(node) else {
            break;
        };
        members.push(member.clone());
        match rest.get(node) {
            Some(next) => node = next,
            None => break,
        }
    }
    members
}

fn read_document(triples: &[Triple], axioms: &mut Axioms) {
    let mut types: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut on_property: BTreeMap<String, String> = BTreeMap::new();
    let mut cardinalities: BTreeMap<String, Vec<(String, u32)>> = BTreeMap::new();
    let mut sub_class_of: Vec<(String, String)> = Vec::new();
    let mut first: BTreeMap<String, String> = BTreeMap::new();
    let mut rest: BTreeMap<String, String> = BTreeMap::new();
    let mut members: Vec<(String, String)> = Vec::new();
    let mut disjoint_with: Vec<(String, String)> = Vec::new();

    for triple in triples {
        let subject = subject_key(&triple.subject);
        let object = object_key(&triple.object);
        match (triple.predicate.as_str(), object) {
            (RDF_TYPE, Some(object)) => {
                types.entry(subject).or_default().insert(object);
            }
            (OWL_ON_PROPERTY, Some(object)) => {
                on_property.insert(subject, object);
            }
            (RDFS_SUB_CLASS_OF, Some(object)) => sub_class_of.push((subject, object)),
            (RDF_FIRST, Some(object)) => {
                first.insert(subject, object);
            }
            (RDF_REST, Some(object)) => {
                rest.insert(subject, object);
            }
            (OWL_MEMBERS, Some(object)) => members.push((subject, object)),
            (OWL_DISJOINT_WITH, Some(object)) => disjoint_with.push((subject, object)),
            (
                predicate @ (OWL_MIN_CARDINALITY | OWL_MAX_CARDINALITY | OWL_QUALIFIED_CARDINALITY),
                None,
            ) => {
                if let Term::Literal(literal) = &triple.object {
                    if let Ok(count) = literal.value().trim().parse::<u32>() {
                        cardinalities
                            .entry(subject)
                            .or_default()
                            .push((predicate.to_string(), count));
                    }
                }
            }
            _ => {}
        }
    }

    for (node, kinds) in &types {
        if is_blank(node) {
            continue;
        }
        for kind in kinds {
            match kind.as_str() {
                OWL_IRREFLEXIVE => {
                    axioms.irreflexive.insert(node.clone());
                }
                OWL_ASYMMETRIC => {
                    axioms.asymmetric.insert(node.clone());
                }
                OWL_FUNCTIONAL => {
                    axioms.functional.insert(node.clone());
                }
                _ => {}
            }
        }
    }

    for (class, restriction) in &sub_class_of {
        if is_blank(class) {
            continue;
        }
        let (Some(property), Some(counts)) =
            (on_property.get(restriction), cardinalities.get(restriction))
        else {
            continue;
        };
        let bounds = axioms
            .bounds
            .entry((class.clone(), property.clone()))
            .or_default();
        for (predicate, count) in counts {
            if predicate.as_str() != OWL_MAX_CARDINALITY {
                bounds.min = Some(bounds.min.map_or(*count, |min| min.max(*count)));
            }
            if predicate.as_str() != OWL_MIN_CARDINALITY {
                bounds.max = Some(bounds.max.map_or(*count, |max| max.min(*count)));
            }
        }
    }

    for (a, b) in &disjoint_with {
        if let Some(pair) = ordered_pair(a, b) {
            axioms.disjoint.insert(pair);
        }
    }
    for (node, kinds) in &types {
        if !kinds.contains(OWL_ALL_DISJOINT) {
            continue;
        }
        for (owner, head) in &members {
            if owner != node {
                continue;
            }
            let classes = list_members(head, &first, &rest);
            for (index, a) in classes.iter().enumerate() {
                for b in &classes[index + 1..] {
                    if let Some(pair) = ordered_pair(a, b) {
                        axioms.disjoint.insert(pair);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use medulla_graph_core::graph::{GraphCore, GraphSchemaSource, SchemaSourceOrigin};
    use medulla_semantic_index::semantic::SemanticStore;
    use medulla_types::shape_report::ViolationKind;

    use super::*;

    const ONTOLOGY: &str = r#"
@prefix ex: <http://ex/> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
ex:irr a owl:ObjectProperty, owl:IrreflexiveProperty .
ex:asym a owl:ObjectProperty, owl:AsymmetricProperty .
ex:func a owl:ObjectProperty, owl:FunctionalProperty .
ex:member a owl:ObjectProperty .
ex:Team a owl:Class ; rdfs:subClassOf [ a owl:Restriction ;
    owl:onProperty ex:member ; owl:minCardinality "1"^^xsd:nonNegativeInteger ] .
ex:A a owl:Class ; owl:disjointWith ex:B .
ex:B a owl:Class .
"#;

    fn id(name: &str) -> String {
        format!("<http://ex/{name}>")
    }

    fn props(value: serde_json::Value) -> Vec<u8> {
        rmp_serde::to_vec_named(&value).unwrap()
    }

    struct Fixture {
        core: GraphCore,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                core: GraphCore::new(),
            }
        }

        fn node(&self, name: &str) {
            self.core.add_node(id(name), props(serde_json::json!({})));
        }

        fn edge(&self, from: &str, property: &str, to: &str, extra: serde_json::Value) {
            let mut value = serde_json::json!({"relationship": format!("http://ex/{property}")});
            if let (Some(map), Some(extra)) = (value.as_object_mut(), extra.as_object()) {
                map.extend(extra.clone());
            }
            self.core.add_edge(id(from), id(to), props(value)).unwrap();
        }

        fn typed(&self, name: &str, class: &str) {
            self.core
                .add_edge(
                    id(name),
                    id(class),
                    props(serde_json::json!({
                        "relationship": "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
                    })),
                )
                .unwrap();
        }

        fn view(&self) -> GraphView {
            let mut view = self.core.analysis_snapshot();
            let mut sources = GraphSchemaSources::default();
            sources
                .attach_dynamic(
                    "admin:consistency".to_string(),
                    GraphSchemaSource::new(
                        SchemaSourceOrigin::Admin {
                            name: "consistency".to_string(),
                        },
                        None,
                        Some(Arc::from(ONTOLOGY)),
                        0,
                    )
                    .unwrap(),
                )
                .unwrap();
            view.schema_sources = Arc::new(sources);
            view
        }
    }

    fn collect_for(view: &GraphView) -> BTreeMap<String, Vec<Violation>> {
        let semantic = SemanticStore::new();
        let ctx = PlanCtx::new(view, &semantic);
        collect(&ctx, &Projection::raw()).unwrap()
    }

    fn kinds(found: &BTreeMap<String, Vec<Violation>>, name: &str) -> Vec<ViolationKind> {
        found
            .get(&id(name))
            .map(|violations| violations.iter().map(Violation::kind).collect())
            .unwrap_or_default()
    }

    // spec: MEDULLA-FEDERATED-QUERY-R005.1
    #[test]
    fn five_kinds_and_their_controls() {
        let fx = Fixture::new();
        for name in [
            "loop", "calm", "a1", "b1", "a2", "b2", "x", "t1", "t2", "y", "full", "empty",
            "member", "bad", "good", "both", "A", "B", "Team", "other",
        ] {
            fx.node(name);
        }
        let none = serde_json::json!({});
        // self_loop and its control
        fx.edge("loop", "irr", "loop", none.clone());
        fx.edge("calm", "irr", "other", none.clone());
        // asymmetry: both directions is the violation, one direction alone the control
        fx.edge("a1", "asym", "b1", none.clone());
        fx.edge("b1", "asym", "a1", none.clone());
        fx.edge("a2", "asym", "b2", none.clone());
        // temporal conflict: overlapping intervals, and a back-to-back control
        fx.edge(
            "x",
            "func",
            "t1",
            serde_json::json!({"valid_from": 1, "valid_until": 10}),
        );
        fx.edge(
            "x",
            "func",
            "t2",
            serde_json::json!({"valid_from": 5, "valid_until": 20}),
        );
        fx.edge(
            "y",
            "func",
            "t1",
            serde_json::json!({"valid_from": 1, "valid_until": 4}),
        );
        fx.edge(
            "y",
            "func",
            "t2",
            serde_json::json!({"valid_from": 4, "valid_until": 9}),
        );
        // cardinality: a Team with no member violates, one with a member is the control
        fx.typed("empty", "Team");
        fx.typed("full", "Team");
        fx.edge("full", "member", "member", none.clone());
        // ontology defect: disjoint classes on one node, one class on the control
        fx.typed("bad", "A");
        fx.typed("bad", "B");
        fx.typed("good", "A");
        // two independent defects on one node
        fx.typed("both", "A");
        fx.typed("both", "B");
        fx.edge("both", "irr", "both", none);

        let view = fx.view();
        let found = collect_for(&view);
        let revision = view.schema_sources.composed_digest().to_hex();

        assert_eq!(kinds(&found, "loop"), [ViolationKind::SelfLoop]);
        assert_eq!(kinds(&found, "a1"), [ViolationKind::Asymmetry]);
        assert_eq!(kinds(&found, "x"), [ViolationKind::TemporalConflict]);
        assert_eq!(kinds(&found, "empty"), [ViolationKind::Cardinality]);
        assert_eq!(kinds(&found, "bad"), [ViolationKind::OntologyDefect]);
        assert_eq!(
            kinds(&found, "both"),
            [ViolationKind::SelfLoop, ViolationKind::OntologyDefect]
        );
        for control in ["calm", "a2", "b1", "b2", "y", "full", "good"] {
            assert!(kinds(&found, control).is_empty(), "{control}");
        }

        let asymmetry = &found[&id("a1")][0];
        assert_eq!(asymmetry.ontology_revision, revision);
        assert_eq!(asymmetry.axiom_id, "asymmetric:http://ex/asym");
        assert_eq!(
            asymmetry.detail,
            ViolationDetail::Asymmetry {
                property: "http://ex/asym".to_string(),
                subject: id("a1"),
                object: id("b1"),
                forward_edge: format!("{}|{}|0", id("a1"), id("b1")),
                reverse_edge: format!("{}|{}|0", id("b1"), id("a1")),
            }
        );
        assert_eq!(
            found[&id("empty")][0].detail,
            ViolationDetail::Cardinality {
                class: "http://ex/Team".to_string(),
                property: "http://ex/member".to_string(),
                min: Some(1),
                max: None,
                observed: 0,
                edges: Vec::new(),
            }
        );
        assert_eq!(
            found[&id("bad")][0].detail,
            ViolationDetail::OntologyDefect {
                affected_terms: vec!["http://ex/A".to_string(), "http://ex/B".to_string()],
            }
        );
        assert_eq!(
            found[&id("bad")][0].axiom_id,
            "disjoint:http://ex/A:http://ex/B"
        );

        // Reading is not writing: the snapshot is unchanged by validation.
        assert_eq!(
            view.node_map.len(),
            fx.core.analysis_snapshot().node_map.len()
        );
        assert_eq!(
            view.edge_properties.len(),
            fx.core.analysis_snapshot().edge_properties.len()
        );
    }

    // spec: MEDULLA-FEDERATED-QUERY-R005.1
    #[test]
    fn stale_pinned_revision_is_refused_and_current_is_accepted() {
        use crate::rowset::RowSet;
        use medulla_query_types::wire::ShapeKeep;
        use medulla_types::shape_report::IntegrityShape;

        let fx = Fixture::new();
        fx.node("loop");
        fx.edge("loop", "irr", "loop", serde_json::json!({}));
        let view = fx.view();
        let semantic = SemanticStore::new();
        let ctx = PlanCtx::new(&view, &semantic);
        let input = || RowSet::from_ids([id("loop")]);

        let stale = IntegrityShape::parse(&format!("integrity:consistency:{}", "0".repeat(64)))
            .unwrap()
            .unwrap();
        let refused = super::super::shape_integrity::run_named_shapes(
            &ctx,
            input(),
            &stale,
            "",
            ShapeKeep::Violating,
        )
        .unwrap_err();
        assert!(refused.starts_with("ONTOLOGY_REVISION_STALE"), "{refused}");

        let current = IntegrityShape::parse(&format!(
            "integrity:consistency:{}",
            view.schema_sources.composed_digest().to_hex()
        ))
        .unwrap()
        .unwrap();
        let rows = super::super::shape_integrity::run_named_shapes(
            &ctx,
            input(),
            &current,
            "",
            ShapeKeep::Violating,
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        let violations = rows.text(&id("loop"), "shape.violations").unwrap();
        let decoded: Vec<Violation> = serde_json::from_str(violations).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].kind(), ViolationKind::SelfLoop);
    }
}
