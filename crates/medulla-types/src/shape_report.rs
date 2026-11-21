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
        }
    }

    /// The severity every finding of this shape carries.
    pub fn severity(&self) -> Severity {
        match self {
            Self::OrphanNode => Severity::Info,
            Self::RequiredProperty { .. } => Severity::Warn,
            Self::DanglingEdge | Self::InvalidReference { .. } => Severity::Fatal,
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
}
