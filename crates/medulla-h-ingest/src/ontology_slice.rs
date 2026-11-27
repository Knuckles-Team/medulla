//! The ontology slice one extraction unit needs (MEDULLA-INGEST-WORKER-R913.1, R913.2).
//!
//! The SDK worker sends one text unit and the engine answers the classes and properties most
//! relevant to it, so the worker's prompt stays small. Selection is the engine's choice: each
//! class (label and comment) is scored against the unit by candidate-local BM25, and, when the
//! request carries the unit's embedding, by cosine against the class text embedded with the one
//! text embedder the Uql path binds; the two ranks fuse by reciprocal rank. A tenant ontology of
//! `max_classes` classes or fewer is returned whole (`bypassed`). Nothing here writes or reads a
//! clock.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_stats::decision::bm25;
use medulla_graph_core::graph::GraphSchemaSources;
use medulla_ingestion_types::ingest_rules::subclass_closure;
use medulla_ingestion_types::ingestion_wire::{
    OntologySlice, OntologySliceRequest, SliceClass, SliceProperty,
};
use medulla_rdf::oxrdf::{NamedOrBlankNode, Term};
use medulla_types::contract::{EngineErrorCode, ServerErrorCode};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS_LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";
const RDFS_COMMENT: &str = "http://www.w3.org/2000/01/rdf-schema#comment";
const RDFS_DOMAIN: &str = "http://www.w3.org/2000/01/rdf-schema#domain";
const RDFS_RANGE: &str = "http://www.w3.org/2000/01/rdf-schema#range";
const RDFS_SUBCLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const CLASS_TYPES: [&str; 2] = [
    "http://www.w3.org/2002/07/owl#Class",
    "http://www.w3.org/2000/01/rdf-schema#Class",
];
/// The reciprocal-rank constant of the two scoring arms.
const RRF_K: f64 = 60.0;

/// A slice refusal: the wire code and its detail. Nothing was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SliceRefusal {
    /// The wire code of the refusal.
    pub code: String,
    /// The detail of the refusal.
    pub detail: String,
}

impl SliceRefusal {
    fn invalid(detail: impl Into<String>) -> Self {
        Self {
            code: EngineErrorCode::InvalidArgument.as_str().to_string(),
            detail: detail.into(),
        }
    }

    /// The wire error text `CODE: detail`.
    pub fn message(&self) -> String {
        format!("{}: {}", self.code, self.detail)
    }
}

/// The text embedder the unit's class texts are embedded with: `text` to a vector, or the
/// embedder's failure.
pub type SliceEmbedder<'a> = &'a dyn Fn(&str) -> Result<Vec<f32>, String>;

#[derive(Default)]
struct ClassEntry {
    label: String,
    description: String,
}

#[derive(Default)]
struct PropertyEntry {
    label: String,
    domains: BTreeSet<String>,
    ranges: BTreeSet<String>,
}

/// The classes, properties and direct subclass edges of the tenant ontology.
#[derive(Default)]
struct TenantOntology {
    classes: BTreeMap<String, ClassEntry>,
    properties: BTreeMap<String, PropertyEntry>,
    direct: BTreeMap<String, BTreeSet<String>>,
}

fn local_name(iri: &str) -> &str {
    iri.rsplit(['#', '/']).next().unwrap_or(iri)
}

/// Read the admitted ontology: the ontology documents of the graph's dynamic schema sources. The
/// engine's own core vocabulary is not the tenant's ontology and is not read.
fn read_ontology(sources: &GraphSchemaSources) -> Result<TenantOntology, SliceRefusal> {
    let mut labels: BTreeMap<String, String> = BTreeMap::new();
    let mut comments: BTreeMap<String, String> = BTreeMap::new();
    let mut class_iris: BTreeSet<String> = BTreeSet::new();
    let mut ontology = TenantOntology::default();
    for source in sources.dynamic.values() {
        let Some(document) = source.ontology_ttl.as_deref() else {
            continue;
        };
        let triples = medulla_rdf::mapping::parse_turtle(document).map_err(|error| SliceRefusal {
            code: ServerErrorCode::Internal.as_str().to_string(),
            detail: error,
        })?;
        for triple in triples {
            let NamedOrBlankNode::NamedNode(subject) = &triple.subject else {
                continue;
            };
            let subject = subject.as_str().to_string();
            match (triple.predicate.as_str(), &triple.object) {
                (RDF_TYPE, Term::NamedNode(class)) if CLASS_TYPES.contains(&class.as_str()) => {
                    class_iris.insert(subject);
                }
                (RDFS_LABEL, Term::Literal(label)) => {
                    labels.insert(subject, label.value().to_string());
                }
                (RDFS_COMMENT, Term::Literal(comment)) => {
                    comments.insert(subject, comment.value().to_string());
                }
                (RDFS_DOMAIN, Term::NamedNode(class)) => {
                    ontology
                        .properties
                        .entry(subject)
                        .or_default()
                        .domains
                        .insert(class.as_str().to_string());
                }
                (RDFS_RANGE, Term::NamedNode(class)) => {
                    ontology
                        .properties
                        .entry(subject)
                        .or_default()
                        .ranges
                        .insert(class.as_str().to_string());
                }
                (RDFS_SUBCLASS_OF, Term::NamedNode(parent)) => {
                    class_iris.insert(subject.clone());
                    class_iris.insert(parent.as_str().to_string());
                    ontology
                        .direct
                        .entry(subject)
                        .or_default()
                        .insert(parent.as_str().to_string());
                }
                _ => {}
            }
        }
    }
    for iri in class_iris {
        let label = labels
            .get(&iri)
            .cloned()
            .unwrap_or_else(|| local_name(&iri).to_string());
        let description = comments.get(&iri).cloned().unwrap_or_default();
        ontology.classes.insert(iri, ClassEntry { label, description });
    }
    for (iri, property) in &mut ontology.properties {
        property.label = labels
            .get(iri)
            .cloned()
            .unwrap_or_else(|| local_name(iri).to_string());
    }
    Ok(ontology)
}

fn slice_class(iri: &str, entry: &ClassEntry) -> SliceClass {
    SliceClass {
        iri: iri.to_string(),
        label: entry.label.clone(),
        description: entry.description.clone(),
    }
}

/// The 1-based ranks of the `(key, score)` pairs, best first, ties by key. Only a positive score
/// has a rank.
fn ranks(mut scored: Vec<(String, f64)>) -> BTreeMap<String, u32> {
    scored.retain(|(_, score)| *score > 0.0);
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    scored
        .into_iter()
        .enumerate()
        .map(|(index, (key, _))| (key, u32::try_from(index + 1).unwrap_or(u32::MAX)))
        .collect()
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    let (mut dot, mut norm_a, mut norm_b) = (0.0f64, 0.0f64, 0.0f64);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (f64::from(*x), f64::from(*y));
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a.sqrt() * norm_b.sqrt())
    }
}

/// The slice of the tenant ontology `request.unit_text` needs.
///
/// Refusals, all before any write: `INVALID_ARGUMENT` for an empty or oversized `unit_text`, a
/// `max_classes` outside 1 to 256, a malformed `unit_embedding`, a `unit_embedding` with no bound
/// `embed`, and an embedding of another width than the embedder's; `ONTOLOGY_NOT_ADMITTED` when
/// the graph holds no admitted ontology class; `ONTOLOGY_HIERARCHY_CYCLE` for a cyclic hierarchy.
pub fn slice_for_unit(
    sources: &GraphSchemaSources,
    request: &OntologySliceRequest,
    embed: Option<SliceEmbedder<'_>>,
) -> Result<OntologySlice, SliceRefusal> {
    request.validate().map_err(SliceRefusal::invalid)?;
    let ontology = read_ontology(sources)?;
    if ontology.classes.is_empty() {
        return Err(SliceRefusal {
            code: ServerErrorCode::OntologyNotAdmitted.as_str().to_string(),
            detail: "the graph holds no admitted ontology class".to_string(),
        });
    }
    let closure = subclass_closure(&ontology.direct).map_err(|refusal| SliceRefusal {
        code: refusal.code(),
        detail: refusal.to_string(),
    })?;
    let max = usize::try_from(request.max_classes).unwrap_or(usize::MAX);
    let bypassed = ontology.classes.len() <= max;
    let chosen: Vec<&String> = if bypassed {
        ontology.classes.keys().collect()
    } else {
        select_classes(&ontology, request, embed, max)?
    };
    let classes: Vec<SliceClass> = chosen
        .iter()
        .map(|iri| slice_class(iri, &ontology.classes[*iri]))
        .collect();
    // A property is connected when its domain or range is a returned class or one of its
    // superclasses.
    let mut reached: BTreeSet<&str> = BTreeSet::new();
    for iri in &chosen {
        reached.insert(iri.as_str());
        if let Some(supers) = closure.get(*iri) {
            reached.extend(supers.iter().map(String::as_str));
        }
    }
    let properties = ontology
        .properties
        .iter()
        .filter(|(_, property)| {
            bypassed
                || property
                    .domains
                    .iter()
                    .chain(&property.ranges)
                    .any(|class| reached.contains(class.as_str()))
        })
        .map(|(iri, property)| SliceProperty {
            iri: iri.clone(),
            label: property.label.clone(),
            domains: property.domains.iter().cloned().collect(),
            ranges: property.ranges.iter().cloned().collect(),
        })
        .collect();
    Ok(OntologySlice {
        classes,
        properties,
        bypassed,
    })
}

/// The `max` best classes by fused rank, best first.
fn select_classes<'a>(
    ontology: &'a TenantOntology,
    request: &OntologySliceRequest,
    embed: Option<SliceEmbedder<'_>>,
    max: usize,
) -> Result<Vec<&'a String>, SliceRefusal> {
    let iris: Vec<&String> = ontology.classes.keys().collect();
    let documents: Vec<String> = iris
        .iter()
        .map(|iri| {
            let entry = &ontology.classes[*iri];
            format!("{} {}", entry.label, entry.description)
        })
        .collect();
    let documents: Vec<&str> = documents.iter().map(String::as_str).collect();
    let lexical = ranks(
        iris.iter()
            .zip(bm25::scores(&documents, &request.unit_text))
            .map(|(iri, score)| ((*iri).clone(), score))
            .collect(),
    );
    let semantic = match request.unit_embedding.as_deref() {
        None => None,
        Some(unit) => {
            let embed = embed.ok_or_else(|| {
                SliceRefusal::invalid("unit_embedding needs a bound text embedder")
            })?;
            let mut scored = Vec::with_capacity(iris.len());
            for (iri, document) in iris.iter().zip(&documents) {
                let vector = embed(document).map_err(|error| SliceRefusal {
                    code: ServerErrorCode::Internal.as_str().to_string(),
                    detail: error,
                })?;
                if vector.len() != unit.len() {
                    return Err(SliceRefusal::invalid(format!(
                        "unit_embedding holds {} values; the text embedder answers {}",
                        unit.len(),
                        vector.len()
                    )));
                }
                scored.push(((*iri).clone(), cosine(unit, &vector)));
            }
            Some(ranks(scored))
        }
    };
    let mut fused: Vec<(&String, f64)> = iris
        .iter()
        .map(|iri| {
            let score: f64 = [lexical.get(*iri), semantic.as_ref().and_then(|r| r.get(*iri))]
                .into_iter()
                .flatten()
                .map(|rank| 1.0 / (RRF_K + f64::from(*rank)))
                .sum();
            (*iri, score)
        })
        .filter(|(_, score)| *score > 0.0)
        .collect();
    fused.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    fused.truncate(max);
    Ok(fused.into_iter().map(|(iri, _)| iri).collect())
}
