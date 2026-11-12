//! Object-model declarations a connector pack ships: value types, link types and
//! interfaces (MEDULLA-TYPED-PACKS-R650.1).
//!
//! The ontology terms are `:ValueType` (`core-foundation-v1.ttl`) and `:LinkType`,
//! `:Interface`, `:implementsInterface` (`system-v1.ttl`). The records below are
//! closed serde types: an unknown field refuses the declaration.

use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

/// A named constrained datatype. `base` is an XSD datatype IRI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueType {
    pub name: String,
    pub base: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_len: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub one_of: Vec<String>,
}

/// A typed link between two classes. `max_card` of 0 is invalid; absent means unbounded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkType {
    pub name: String,
    pub from_class: String,
    pub to_class: String,
    #[serde(default)]
    pub min_card: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_card: Option<NonZeroU32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub junction: Option<String>,
}

/// An interface: the properties and links an implementing class must carry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceDecl {
    pub name: String,
    #[serde(default)]
    pub required_properties: Vec<String>,
    #[serde(default)]
    pub required_links: Vec<String>,
}

/// One property a class carries, constrained by a declared [`ValueType`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropertyDecl {
    /// The class IRI the property belongs to.
    pub class: String,
    /// The property IRI.
    pub property: String,
    /// The name of the [`ValueType`] that constrains its values, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_type: Option<String>,
}

/// A class that declares `:implementsInterface`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Implementation {
    pub class: String,
    pub interface: String,
}

/// Every object-model declaration one pack ships.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectModel {
    #[serde(default)]
    pub value_types: Vec<ValueType>,
    #[serde(default)]
    pub link_types: Vec<LinkType>,
    #[serde(default)]
    pub interfaces: Vec<InterfaceDecl>,
    #[serde(default)]
    pub properties: Vec<PropertyDecl>,
    #[serde(default)]
    pub implementations: Vec<Implementation>,
}

/// An object of a compiled shape triple.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ShapeTerm {
    Iri(String),
    Literal {
        lexical: String,
        datatype: Option<String>,
    },
}

/// One compiled SHACL triple. `medulla-core` does not depend on `medulla-rdf`,
/// so the caller converts it to an RDF triple.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ShapeTriple {
    pub subject: String,
    pub predicate: String,
    pub object: ShapeTerm,
}

/// A required interface member a class does not declare.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct InterfaceGap {
    pub class: String,
    pub interface: String,
    /// The missing property IRI or link name.
    pub member: String,
}

const SH: &str = "http://www.w3.org/ns/shacl#";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
/// Namespace of every compiled shape and of link names that are not IRIs.
pub const OBJECT_MODEL_NS: &str = medulla_types::ontology_iri!("object-model#");

/// The property IRI a link name stands for: an IRI stays, a bare name is
/// minted under [`OBJECT_MODEL_NS`].
pub fn link_iri(name: &str) -> String {
    if name.contains(':') {
        name.to_string()
    } else {
        format!("{OBJECT_MODEL_NS}link/{}", segment(name))
    }
}

fn segment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

struct Out(Vec<ShapeTriple>);

impl Out {
    fn iri(&mut self, subject: &str, predicate: &str, object: &str) {
        self.0.push(ShapeTriple {
            subject: subject.to_string(),
            predicate: predicate.to_string(),
            object: ShapeTerm::Iri(object.to_string()),
        });
    }

    fn lit(&mut self, subject: &str, predicate: &str, lexical: String, datatype: &str) {
        self.0.push(ShapeTriple {
            subject: subject.to_string(),
            predicate: predicate.to_string(),
            object: ShapeTerm::Literal {
                lexical,
                datatype: Some(format!("{XSD}{datatype}")),
            },
        });
    }

    /// One `sh:NodeShape` on `class` with one property shape on `path`;
    /// returns the property shape IRI.
    fn property_shape(&mut self, class: &str, path: &str) -> String {
        self.scoped_property_shape("shape", class, path)
    }

    fn scoped_property_shape(&mut self, scope: &str, class: &str, path: &str) -> String {
        let node = format!("{OBJECT_MODEL_NS}{scope}/{}", segment(class));
        let property = format!("{node}/property/{}", segment(path));
        self.iri(&node, &format!("{RDF}type"), &format!("{SH}NodeShape"));
        self.iri(&node, &format!("{SH}targetClass"), class);
        self.iri(&node, &format!("{SH}property"), &property);
        self.iri(&property, &format!("{SH}path"), path);
        property
    }
}

/// Compile the object model to SHACL triples, sorted and de-duplicated
/// (MEDULLA-TYPED-PACKS-R650.2). A value type becomes one property shape per
/// property that uses it; a link type becomes `sh:class` with `sh:minCount` and
/// `sh:maxCount` on its source class; a junction link adds a shape on the
/// junction class with two required links; each implementing class gets a
/// required-member shape per interface member. Linear in the declarations.
pub fn compile_shapes(model: &ObjectModel) -> Vec<ShapeTriple> {
    let mut out = Out(Vec::new());
    for property in &model.properties {
        let Some(value) = property
            .value_type
            .as_deref()
            .and_then(|name| model.value_types.iter().find(|v| v.name == name))
        else {
            continue;
        };
        let shape = out.property_shape(&property.class, &property.property);
        out.iri(&shape, &format!("{SH}datatype"), &value.base);
        if let Some(pattern) = &value.pattern {
            out.lit(&shape, &format!("{SH}pattern"), pattern.clone(), "string");
        }
        if let Some(min) = value.min {
            out.lit(
                &shape,
                &format!("{SH}minInclusive"),
                min.to_string(),
                "double",
            );
        }
        if let Some(max) = value.max {
            out.lit(
                &shape,
                &format!("{SH}maxInclusive"),
                max.to_string(),
                "double",
            );
        }
        if let Some(max_len) = value.max_len {
            out.lit(
                &shape,
                &format!("{SH}maxLength"),
                max_len.to_string(),
                "integer",
            );
        }
        if !value.one_of.is_empty() {
            let mut head = format!("{RDF}nil");
            for (index, item) in value.one_of.iter().enumerate().rev() {
                let cell = format!("{shape}/in/{index}");
                out.0.push(ShapeTriple {
                    subject: cell.clone(),
                    predicate: format!("{RDF}first"),
                    object: ShapeTerm::Literal {
                        lexical: item.clone(),
                        datatype: Some(value.base.clone()),
                    },
                });
                out.iri(&cell, &format!("{RDF}rest"), &head);
                head = cell;
            }
            out.iri(&shape, &format!("{SH}in"), &head);
        }
    }
    for link in &model.link_types {
        let path = link_iri(&link.name);
        let shape = out.property_shape(&link.from_class, &path);
        out.iri(&shape, &format!("{SH}class"), &link.to_class);
        if link.min_card > 0 {
            out.lit(
                &shape,
                &format!("{SH}minCount"),
                link.min_card.to_string(),
                "integer",
            );
        }
        if let Some(max) = link.max_card {
            out.lit(
                &shape,
                &format!("{SH}maxCount"),
                max.get().to_string(),
                "integer",
            );
        }
        if let Some(junction) = &link.junction {
            for (side, class) in [("from", &link.from_class), ("to", &link.to_class)] {
                let side_path = format!("{path}/{side}");
                let side_shape = out.property_shape(junction, &side_path);
                out.iri(&side_shape, &format!("{SH}class"), class);
                out.lit(
                    &side_shape,
                    &format!("{SH}minCount"),
                    "1".to_string(),
                    "integer",
                );
                out.lit(
                    &side_shape,
                    &format!("{SH}maxCount"),
                    "1".to_string(),
                    "integer",
                );
            }
        }
    }
    for implementation in &model.implementations {
        let Some(interface) = model
            .interfaces
            .iter()
            .find(|i| i.name == implementation.interface)
        else {
            continue;
        };
        let members = interface
            .required_properties
            .iter()
            .cloned()
            .chain(interface.required_links.iter().map(|l| link_iri(l)));
        for member in members {
            let scope = format!("interface/{}", segment(&implementation.interface));
            let shape = out.scoped_property_shape(&scope, &implementation.class, &member);
            out.lit(&shape, &format!("{SH}minCount"), "1".to_string(), "integer");
        }
    }
    out.0.sort();
    out.0.dedup();
    out.0
}

/// Per class with `:implementsInterface`, each required property or link the
/// model does not declare on that class, sorted.
pub fn interface_gaps(model: &ObjectModel) -> Vec<InterfaceGap> {
    let mut gaps = Vec::new();
    for implementation in &model.implementations {
        let Some(interface) = model
            .interfaces
            .iter()
            .find(|i| i.name == implementation.interface)
        else {
            continue;
        };
        let declared_property = |iri: &str| {
            model
                .properties
                .iter()
                .any(|p| p.class == implementation.class && p.property == iri)
        };
        let declared_link = |name: &str| {
            model.link_types.iter().any(|l| {
                l.from_class == implementation.class && link_iri(&l.name) == link_iri(name)
            })
        };
        let missing = interface
            .required_properties
            .iter()
            .filter(|p| !declared_property(p))
            .chain(
                interface
                    .required_links
                    .iter()
                    .filter(|l| !declared_link(l)),
            );
        for member in missing {
            gaps.push(InterfaceGap {
                class: implementation.class.clone(),
                interface: implementation.interface.clone(),
                member: member.clone(),
            });
        }
    }
    gaps.sort();
    gaps.dedup();
    gaps
}
