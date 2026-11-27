//! Graph integrity validation as named shapes (MEDULLA-FEDERATED-QUERY-R005.700).
//!
//! An integrity shape is a named structural rule over the queried graph that the
//! `VALIDATE SHAPE` stage runs in one pass. A shape is named by a reference that
//! starts with [`INTEGRITY_SHAPE_PREFIX`]; any other reference is a SHACL shape and
//! stays with the SHACL validator.

use serde::{Deserialize, Serialize};

/// Prefix that marks a `VALIDATE SHAPE` reference as an integrity shape.
pub const INTEGRITY_SHAPE_PREFIX: &str = "integrity:";

/// How serious a finding is. Only `Fatal` blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Worth knowing, never blocks.
    Info,
    /// Likely a defect, never blocks.
    Warn,
    /// A structural defect; the report blocks.
    Fatal,
}

impl Severity {
    /// The stable wire token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Fatal => "fatal",
        }
    }

    /// Whether a finding of this severity blocks.
    pub fn blocks(self) -> bool {
        self == Self::Fatal
    }
}

/// One named integrity shape, closed set.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrityShape {
    /// A node lacks the named property (or holds it null).
    RequiredProperty {
        /// The property key every node must carry.
        key: String,
    },
    /// A node has no incident edge.
    OrphanNode,
    /// An edge names an endpoint that is not a node of the graph.
    DanglingEdge,
    /// A node's reference property names a node that does not exist.
    InvalidReference {
        /// The property key whose string value (or string items) must name nodes.
        key: String,
    },
    /// The typed consistency-violation matrix over the composed ontology's axioms
    /// (MEDULLA-FEDERATED-QUERY-R005.1).
    Consistency {
        /// The ontology revision (composed digest, lowercase hex) the caller pins, if any.
        ontology_revision: Option<String>,
    },
}

impl IntegrityShape {
    /// Parse a shape reference. `Ok(None)` when it is not an integrity shape (a SHACL
    /// shape); `Err` when it carries the integrity prefix but names no integrity shape.
    pub fn parse(reference: &str) -> Result<Option<Self>, String> {
        let bare = reference
            .strip_prefix('<')
            .and_then(|rest| rest.strip_suffix('>'))
            .unwrap_or(reference);
        let Some(name) = bare.strip_prefix(INTEGRITY_SHAPE_PREFIX) else {
            return Ok(None);
        };
        let (kind, key) = match name.split_once(':') {
            Some((kind, key)) => (kind, Some(key)),
            None => (name, None),
        };
        let keyed = |key: Option<&str>| match key {
            Some(key) if !key.is_empty() => Ok(key.to_string()),
            _ => Err(format!(
                "SHAPE_UNKNOWN: integrity shape `{kind}` needs a property key, \
                 as `{INTEGRITY_SHAPE_PREFIX}{kind}:<key>`"
            )),
        };
        let unkeyed = |shape: Self| match key {
            None => Ok(Some(shape)),
            Some(_) => Err(format!(
                "SHAPE_UNKNOWN: integrity shape `{kind}` takes no property key"
            )),
        };
        match kind {
            "required_property" => Ok(Some(Self::RequiredProperty { key: keyed(key)? })),
            "invalid_reference" => Ok(Some(Self::InvalidReference { key: keyed(key)? })),
            "consistency" => Ok(Some(Self::Consistency {
                ontology_revision: match key {
                    None => None,
                    Some(revision) if !revision.is_empty() => Some(revision.to_string()),
                    Some(_) => {
                        return Err(
                            "SHAPE_UNKNOWN: integrity shape `consistency` has an empty ontology revision"
                                .to_string(),
                        )
                    }
                },
            })),
            "orphan_node" => unkeyed(Self::OrphanNode),
            "dangling_edge" => unkeyed(Self::DanglingEdge),
            other => Err(format!(
                "SHAPE_UNKNOWN: `{INTEGRITY_SHAPE_PREFIX}{other}` is not an integrity shape"
            )),
        }
    }

    /// The canonical reference of this shape (the `shape_id` of its findings).
    pub fn shape_id(&self) -> String {
        match self {
            Self::RequiredProperty { key } => {
                format!("{INTEGRITY_SHAPE_PREFIX}required_property:{key}")
            }
            Self::OrphanNode => format!("{INTEGRITY_SHAPE_PREFIX}orphan_node"),
            Self::DanglingEdge => format!("{INTEGRITY_SHAPE_PREFIX}dangling_edge"),
            Self::InvalidReference { key } => {
                format!("{INTEGRITY_SHAPE_PREFIX}invalid_reference:{key}")
            }
            Self::Consistency { .. } => format!("{INTEGRITY_SHAPE_PREFIX}consistency"),
        }
    }

    /// The severity every finding of this shape carries.
    pub fn severity(&self) -> Severity {
        match self {
            Self::OrphanNode => Severity::Info,
            Self::RequiredProperty { .. } => Severity::Warn,
            Self::DanglingEdge | Self::InvalidReference { .. } | Self::Consistency { .. } => {
                Severity::Fatal
            }
        }
    }
}

/// One finding of an integrity shape.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ShapeReport {
    /// The shape that produced the finding.
    pub shape_id: String,
    /// The finding's severity.
    pub severity: Severity,
    /// The node the finding is about.
    pub focus: String,
    /// What is wrong, in one sentence.
    pub message: String,
    /// True when `severity` is `Fatal`.
    pub blocks: bool,
}

impl ShapeReport {
    /// A finding of `shape` about `focus`.
    pub fn new(shape: &IntegrityShape, focus: String, message: String) -> Self {
        let severity = shape.severity();
        Self {
            shape_id: shape.shape_id(),
            severity,
            focus,
            message,
            blocks: severity.blocks(),
        }
    }
}

/// The kind of one consistency violation (MEDULLA-FEDERATED-QUERY-R005.1), a closed set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViolationKind {
    /// Two live edges of a functional property from one subject, with overlapping validity.
    TemporalConflict,
    /// A subject's count of live targets lies outside a declared cardinality.
    Cardinality,
    /// A live edge from a node to itself over an irreflexive property.
    SelfLoop,
    /// Live edges in both directions between two nodes over an asymmetric property.
    Asymmetry,
    /// A node typed with two classes the ontology declares disjoint.
    OntologyDefect,
}

impl ViolationKind {
    /// The stable wire token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TemporalConflict => "temporal_conflict",
            Self::Cardinality => "cardinality",
            Self::SelfLoop => "self_loop",
            Self::Asymmetry => "asymmetry",
            Self::OntologyDefect => "ontology_defect",
        }
    }
}

/// The kind-specific fields of one violation. The kind tag is the serde tag, so an unknown
/// kind does not decode and every field of a kind is required.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ViolationDetail {
    /// Overlapping validity of two live edges of a functional property.
    TemporalConflict {
        /// The functional property IRI.
        property: String,
        /// The first edge id.
        first_edge: String,
        /// The second edge id.
        second_edge: String,
        /// `[valid_from, valid_until)` of the first edge.
        first: (i64, i64),
        /// `[valid_from, valid_until)` of the second edge.
        second: (i64, i64),
    },
    /// A count outside a declared cardinality.
    Cardinality {
        /// The class whose restriction declares the bound (`owl:Thing` for a functional property).
        class: String,
        /// The property IRI.
        property: String,
        /// The declared minimum, if any.
        min: Option<u32>,
        /// The declared maximum, if any.
        max: Option<u32>,
        /// The observed count of distinct live targets.
        observed: u32,
        /// The edge ids that were counted.
        edges: Vec<String>,
    },
    /// An edge from a node to itself over an irreflexive property.
    SelfLoop {
        /// The irreflexive property IRI.
        property: String,
        /// The edge id.
        edge: String,
    },
    /// A reverse pair over an asymmetric property.
    Asymmetry {
        /// The asymmetric property IRI.
        property: String,
        /// The smaller node id of the pair.
        subject: String,
        /// The larger node id of the pair.
        object: String,
        /// The edge id of `subject -> object`.
        forward_edge: String,
        /// The edge id of `object -> subject`.
        reverse_edge: String,
    },
    /// A node typed with two disjoint classes.
    OntologyDefect {
        /// The two disjoint class IRIs, smaller first.
        affected_terms: Vec<String>,
    },
}

/// One consistency violation, pinned to the ontology revision it was found under.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Violation {
    /// The node the violation is about.
    pub focus: String,
    /// The composed ontology digest as lowercase hex.
    pub ontology_revision: String,
    /// The violated axiom id, for example `asymmetric:<property>`.
    pub axiom_id: String,
    /// The kind-specific fields.
    pub detail: ViolationDetail,
}

impl Violation {
    /// The kind of this violation.
    pub fn kind(&self) -> ViolationKind {
        match self.detail {
            ViolationDetail::TemporalConflict { .. } => ViolationKind::TemporalConflict,
            ViolationDetail::Cardinality { .. } => ViolationKind::Cardinality,
            ViolationDetail::SelfLoop { .. } => ViolationKind::SelfLoop,
            ViolationDetail::Asymmetry { .. } => ViolationKind::Asymmetry,
            ViolationDetail::OntologyDefect { .. } => ViolationKind::OntologyDefect,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-FEDERATED-QUERY-R005.700
    #[test]
    fn parse_is_closed_and_round_trips_shape_ids() {
        for reference in [
            "integrity:required_property:name",
            "integrity:orphan_node",
            "integrity:dangling_edge",
            "integrity:invalid_reference:owner",
            "integrity:consistency",
        ] {
            let shape = IntegrityShape::parse(reference).unwrap().unwrap();
            assert_eq!(shape.shape_id(), reference);
            let bracketed = format!("<{reference}>");
            assert_eq!(IntegrityShape::parse(&bracketed).unwrap(), Some(shape));
        }
        assert_eq!(IntegrityShape::parse("http://ex/Named").unwrap(), None);
        for bad in [
            "integrity:nonsense",
            "integrity:required_property",
            "integrity:orphan_node:x",
        ] {
            assert!(IntegrityShape::parse(bad)
                .unwrap_err()
                .starts_with("SHAPE_UNKNOWN"));
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R005.1
    #[test]
    fn violation_round_trips_and_rejects_unknown_kind() {
        let pinned = IntegrityShape::parse("integrity:consistency:ab12")
            .unwrap()
            .unwrap();
        assert_eq!(
            pinned,
            IntegrityShape::Consistency {
                ontology_revision: Some("ab12".to_string())
            }
        );
        assert_eq!(pinned.shape_id(), "integrity:consistency");
        assert!(pinned.severity().blocks());
        let details = [
            ViolationDetail::TemporalConflict {
                property: "p".into(),
                first_edge: "a|b|0".into(),
                second_edge: "a|c|0".into(),
                first: (1, 5),
                second: (3, i64::MAX),
            },
            ViolationDetail::Cardinality {
                class: "C".into(),
                property: "p".into(),
                min: Some(1),
                max: None,
                observed: 0,
                edges: Vec::new(),
            },
            ViolationDetail::SelfLoop {
                property: "p".into(),
                edge: "a|a|0".into(),
            },
            ViolationDetail::Asymmetry {
                property: "p".into(),
                subject: "a".into(),
                object: "b".into(),
                forward_edge: "a|b|0".into(),
                reverse_edge: "b|a|0".into(),
            },
            ViolationDetail::OntologyDefect {
                affected_terms: vec!["C1".into(), "C2".into()],
            },
        ];
        let kinds = [
            ViolationKind::TemporalConflict,
            ViolationKind::Cardinality,
            ViolationKind::SelfLoop,
            ViolationKind::Asymmetry,
            ViolationKind::OntologyDefect,
        ];
        for (detail, kind) in details.into_iter().zip(kinds) {
            let violation = Violation {
                focus: "a".into(),
                ontology_revision: "00".into(),
                axiom_id: "x:p".into(),
                detail,
            };
            assert_eq!(violation.kind(), kind);
            let json = serde_json::to_value(&violation).unwrap();
            assert_eq!(json["detail"]["kind"], kind.as_str());
            let back: Violation = serde_json::from_value(json.clone()).unwrap();
            assert_eq!(back, violation);
            // A kind-specific field is required.
            let mut missing = json.clone();
            let fields = missing["detail"].as_object_mut().unwrap();
            let key = fields.keys().find(|key| *key != "kind").unwrap().clone();
            fields.remove(&key);
            assert!(serde_json::from_value::<Violation>(missing).is_err());
        }
        // An unknown kind does not decode.
        let unknown = serde_json::json!({
            "focus": "a", "ontology_revision": "00", "axiom_id": "x",
            "detail": {"kind": "teleport"}
        });
        assert!(serde_json::from_value::<Violation>(unknown).is_err());
    }
}
