//! Turtle to [`ObjectModel`] extractor (MEDULLA-TYPED-PACKS-R650.7).
//!
//! The one reader of the object-model terms a pack ontology declares: value types,
//! link types, junction links, interfaces and implementations. It reads existing
//! terms only (`:ValueType`, `:LinkType`, `:JunctionLink`, `:Interface`,
//! `:hasInterfaceProperty`, `:implementsInterface` and the SHACL facets), adds no
//! ontology term, and returns the model `medulla-core` compiles. `medulla-core`
//! parses no pack Turtle.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;

use medulla_core::ontology::object_model::{
    Implementation, InterfaceDecl, LinkType, ObjectModel, PropertyDecl, ValueType,
};

use crate::oxrdf::{NamedOrBlankNode, Term};

const CORE: &str = medulla_types::ontology_iri!("core#");
const SH: &str = "http://www.w3.org/ns/shacl#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const OWL_DATATYPE_PROPERTY: &str = "http://www.w3.org/2002/07/owl#DatatypeProperty";

/// Why one declaration is not a well-formed object-model term. Every variant names
/// the subject IRI; the import reports it as `OBJECT_MODEL_INVALID`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectModelError {
    /// A value type has zero or several `sh:datatype` objects.
    ValueTypeBaseMissing { subject: String },
    /// A link type has not exactly one domain and one range.
    LinkEndpointMissing { subject: String },
    /// `sh:maxCount 0`, a non-integer count, or `min_card` above `max_card`.
    CardinalityInvalid { subject: String },
    /// A numeric facet is not numeric, or is stated twice.
    FacetInvalid { subject: String },
    /// `:implementsInterface` names no declared interface.
    InterfaceUnknown { subject: String },
}

impl ObjectModelError {
    /// The IRI of the declaration that is malformed.
    pub fn subject(&self) -> &str {
        match self {
            Self::ValueTypeBaseMissing { subject }
            | Self::LinkEndpointMissing { subject }
            | Self::CardinalityInvalid { subject }
            | Self::FacetInvalid { subject }
            | Self::InterfaceUnknown { subject } => subject,
        }
    }

    /// A fixed reason; it never echoes the pack's own values.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::ValueTypeBaseMissing { .. } => "a value type needs exactly one sh:datatype",
            Self::LinkEndpointMissing { .. } => {
                "a link type needs exactly one rdfs:domain and one rdfs:range"
            }
            Self::CardinalityInvalid { .. } => {
                "a link cardinality must be integers with 1 <= max and min <= max"
            }
            Self::FacetInvalid { .. } => "a range or length facet must be one numeric literal",
            Self::InterfaceUnknown { .. } => "implementsInterface names no declared interface",
        }
    }
}

/// Objects by subject, then predicate.
type Objects<'a> = BTreeMap<String, BTreeMap<&'a str, Vec<&'a Term>>>;

fn subject_key(subject: &NamedOrBlankNode) -> String {
    match subject {
        NamedOrBlankNode::NamedNode(node) => node.as_str().to_string(),
        NamedOrBlankNode::BlankNode(node) => format!("_:{}", node.as_str()),
    }
}

fn term_key(term: &Term) -> Option<String> {
    match term {
        Term::NamedNode(node) => Some(node.as_str().to_string()),
        Term::BlankNode(node) => Some(format!("_:{}", node.as_str())),
        _ => None,
    }
}

fn literal(term: &Term) -> Option<&str> {
    match term {
        Term::Literal(value) => Some(value.value()),
        _ => None,
    }
}

fn iri(term: &Term) -> Option<&str> {
    match term {
        Term::NamedNode(node) => Some(node.as_str()),
        _ => None,
    }
}

/// Extract every object-model declaration the pack ontology `documents` hold.
/// Malformed Turtle declares nothing (the import refuses it elsewhere). Every
/// output vector is sorted by name, so the result is deterministic; the first
/// malformed declaration, in subject order, is the error.
pub fn extract_object_model(documents: &[&str]) -> Result<ObjectModel, ObjectModelError> {
    let mut triples = Vec::new();
    for text in documents {
        triples.extend(crate::mapping::parse_turtle(text).unwrap_or_default());
    }
    let mut objects: Objects<'_> = BTreeMap::new();
    let mut subjects_of: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for triple in &triples {
        let subject = subject_key(&triple.subject);
        let predicate = triple.predicate.as_str();
        if predicate == format!("{RDF}type") {
            if let Some(class) = iri(&triple.object) {
                subjects_of
                    .entry(class.to_string())
                    .or_default()
                    .insert(subject.clone());
            }
        }
        objects
            .entry(subject)
            .or_default()
            .entry(predicate)
            .or_default()
            .push(&triple.object);
    }
    let of_type =
        |class: &str| -> BTreeSet<String> { subjects_of.get(class).cloned().unwrap_or_default() };
    let get = |subject: &str, predicate: &str| {
        let mut found: Vec<&Term> = objects
            .get(subject)
            .and_then(|by_predicate| by_predicate.get(predicate))
            .map(|terms| terms.to_vec())
            .unwrap_or_default();
        found.sort_by_key(|term| term.to_string());
        found
    };

    // Value types.
    let mut value_types = Vec::new();
    for subject in of_type(&format!("{CORE}ValueType")) {
        value_types.push(value_type(&subject, &get)?);
    }
    let value_type_names: BTreeSet<&str> = value_types.iter().map(|v| v.name.as_str()).collect();

    // Properties constrained by a declared value type.
    let mut properties = BTreeSet::new();
    for subject in of_type(OWL_DATATYPE_PROPERTY) {
        let ranges: Vec<&str> = get(&subject, &format!("{RDFS}range"))
            .into_iter()
            .filter_map(iri)
            .filter(|range| value_type_names.contains(range))
            .collect();
        for range in ranges {
            for domain in get(&subject, &format!("{RDFS}domain"))
                .into_iter()
                .filter_map(iri)
            {
                properties.insert((domain.to_string(), subject.clone(), range.to_string()));
            }
        }
    }

    // Link types.
    let junctions = of_type(&format!("{CORE}JunctionLink"));
    let mut link_subjects = of_type(&format!("{CORE}LinkType"));
    link_subjects.extend(junctions.iter().cloned());
    let mut link_types = Vec::new();
    for subject in &link_subjects {
        link_types.push(link_type(subject, junctions.contains(subject), &get)?);
    }
    let link_names: BTreeSet<&str> = link_types.iter().map(|l| l.name.as_str()).collect();

    // Interfaces and their implementations.
    let interface_subjects = of_type(&format!("{CORE}Interface"));
    let mut interfaces = Vec::new();
    for subject in &interface_subjects {
        let mut required_properties = BTreeSet::new();
        let mut required_links = BTreeSet::new();
        for member in get(subject, &format!("{CORE}hasInterfaceProperty")) {
            let Some(member) = term_key(member) else {
                continue;
            };
            if link_names.contains(member.as_str()) {
                required_links.insert(member);
            } else {
                required_properties.insert(member);
            }
        }
        interfaces.push(InterfaceDecl {
            name: subject.clone(),
            required_properties: required_properties.into_iter().collect(),
            required_links: required_links.into_iter().collect(),
        });
    }
    let mut implementations = BTreeSet::new();
    let implements = format!("{CORE}implementsInterface");
    for (subject, by_predicate) in &objects {
        let Some(terms) = by_predicate.get(implements.as_str()) else {
            continue;
        };
        for term in terms {
            let interface = term_key(term).unwrap_or_default();
            if !interface_subjects.contains(&interface) {
                return Err(ObjectModelError::InterfaceUnknown {
                    subject: subject.clone(),
                });
            }
            implementations.insert((subject.clone(), interface));
        }
    }

    value_types.sort_by(|a, b| a.name.cmp(&b.name));
    link_types.sort_by(|a, b| a.name.cmp(&b.name));
    interfaces.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(ObjectModel {
        value_types,
        link_types,
        interfaces,
        properties: properties
            .into_iter()
            .map(|(class, property, value_type)| PropertyDecl {
                class,
                property,
                value_type: Some(value_type),
            })
            .collect(),
        implementations: implementations
            .into_iter()
            .map(|(class, interface)| Implementation { class, interface })
            .collect(),
    })
}

fn value_type<'a>(
    subject: &str,
    get: &dyn Fn(&str, &str) -> Vec<&'a Term>,
) -> Result<ValueType, ObjectModelError> {
    let facet_error = || ObjectModelError::FacetInvalid {
        subject: subject.to_string(),
    };
    let bases: Vec<&str> = get(subject, &format!("{SH}datatype"))
        .into_iter()
        .filter_map(iri)
        .collect();
    let [base] = bases.as_slice() else {
        return Err(ObjectModelError::ValueTypeBaseMissing {
            subject: subject.to_string(),
        });
    };
    let single = |predicate: &str| -> Result<Option<String>, ObjectModelError> {
        let values = get(subject, &format!("{SH}{predicate}"));
        match values.as_slice() {
            [] => Ok(None),
            [one] => literal(one)
                .map(|text| Some(text.to_string()))
                .ok_or_else(facet_error),
            _ => Err(facet_error()),
        }
    };
    let number = |predicate: &str| -> Result<Option<f64>, ObjectModelError> {
        single(predicate)?
            .map(|text| {
                text.trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite())
                    .ok_or_else(facet_error)
            })
            .transpose()
    };
    let max_len = single("maxLength")?
        .map(|text| text.trim().parse::<u32>().map_err(|_| facet_error()))
        .transpose()?;
    let pattern = match get(subject, &format!("{SH}pattern")).as_slice() {
        [] => None,
        [one] => Some(literal(one).ok_or_else(facet_error)?.to_string()),
        _ => return Err(facet_error()),
    };
    Ok(ValueType {
        name: subject.to_string(),
        base: (*base).to_string(),
        pattern,
        min: number("minInclusive")?,
        max: number("maxInclusive")?,
        max_len,
        one_of: enumeration(subject, get)?,
    })
}

/// The lexical forms of the RDF list `sh:in` names, in list order.
fn enumeration<'a>(
    subject: &str,
    get: &dyn Fn(&str, &str) -> Vec<&'a Term>,
) -> Result<Vec<String>, ObjectModelError> {
    let facet_error = || ObjectModelError::FacetInvalid {
        subject: subject.to_string(),
    };
    let heads = get(subject, &format!("{SH}in"));
    let head = match heads.as_slice() {
        [] => return Ok(Vec::new()),
        [one] => term_key(one).ok_or_else(facet_error)?,
        _ => return Err(facet_error()),
    };
    let nil = format!("{RDF}nil");
    let mut items = Vec::new();
    let mut cell = head;
    let mut seen = BTreeSet::new();
    while cell != nil {
        if !seen.insert(cell.clone()) {
            return Err(facet_error());
        }
        let first = get(&cell, &format!("{RDF}first"));
        let [first] = first.as_slice() else {
            return Err(facet_error());
        };
        items.push(match first {
            Term::Literal(value) => value.value().to_string(),
            Term::NamedNode(node) => node.as_str().to_string(),
            _ => return Err(facet_error()),
        });
        let rest = get(&cell, &format!("{RDF}rest"));
        let [rest] = rest.as_slice() else {
            return Err(facet_error());
        };
        cell = term_key(rest).ok_or_else(facet_error)?;
    }
    Ok(items)
}

fn link_type<'a>(
    subject: &str,
    junction: bool,
    get: &dyn Fn(&str, &str) -> Vec<&'a Term>,
) -> Result<LinkType, ObjectModelError> {
    let endpoint = || ObjectModelError::LinkEndpointMissing {
        subject: subject.to_string(),
    };
    let cardinality = || ObjectModelError::CardinalityInvalid {
        subject: subject.to_string(),
    };
    let one_iri = |predicate: &str| -> Result<String, ObjectModelError> {
        let values = get(subject, &format!("{RDFS}{predicate}"));
        match values.as_slice() {
            [one] => iri(one).map(str::to_string).ok_or_else(endpoint),
            _ => Err(endpoint()),
        }
    };
    let from_class = one_iri("domain")?;
    let to_class = one_iri("range")?;
    let count = |predicate: &str| -> Result<Option<u32>, ObjectModelError> {
        let values = get(subject, &format!("{SH}{predicate}"));
        match values.as_slice() {
            [] => Ok(None),
            [one] => literal(one)
                .and_then(|text| text.trim().parse::<u32>().ok())
                .map(Some)
                .ok_or_else(cardinality),
            _ => Err(cardinality()),
        }
    };
    let min_card = count("minCount")?.unwrap_or(0);
    let max_card = match count("maxCount")? {
        None => None,
        Some(max) => Some(NonZeroU32::new(max).ok_or_else(cardinality)?),
    };
    if max_card.is_some_and(|max| min_card > max.get()) {
        return Err(cardinality());
    }
    Ok(LinkType {
        name: subject.to_string(),
        from_class,
        to_class,
        min_card,
        max_card,
        junction: junction.then(|| subject.to_string()),
    })
}
