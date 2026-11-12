//! ArchiMate Open Exchange Format (MEDULLA-CQ-ORCH-R705).
//!
//! `read::parse_exchange` (R705.3) returns the records of
//! `medulla_types::archimate_exchange`; `graph::exchange_mutations` (R705.4)
//! maps a document onto the graph; `write::write_exchange` (R705.5) writes the
//! visible architecture nodes back as a document.

use std::collections::BTreeMap;

use medulla_graph_core::graph::GraphCore;

pub use medulla_types::archimate_exchange::ArchimateExchangeError;

pub mod graph;
pub mod read;
pub mod write;

/// The Open Exchange Format root namespace.
pub const ARCHIMATE_NS: &str = "http://www.opengroup.org/xsd/archimate/3.0/";

/// The `:archimateType` and `:archimateRelationshipType` annotations of
/// `archimate-v1.ttl`, resolved to plain maps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArchimateTypeMap {
    /// ArchiMate element type to the class label that carries it.
    pub classes: BTreeMap<String, String>,
    /// ArchiMate relationship type to the edge predicate that carries it.
    pub edges: BTreeMap<String, String>,
    /// Class label to its parent class label, for nearest-annotation lookup.
    pub parents: BTreeMap<String, String>,
}

impl ArchimateTypeMap {
    /// The ArchiMate element type of a class label: the nearest annotated
    /// class on the way up.
    pub fn element_type_of(&self, label: &str) -> Option<&str> {
        let mut current = label;
        for _ in 0..64 {
            if let Some((archimate, _)) = self.classes.iter().find(|(_, class)| *class == current) {
                return Some(archimate.as_str());
            }
            current = self.parents.get(current)?;
        }
        None
    }

    /// The class label an element type maps to.
    pub fn class_of(&self, element_type: &str) -> Option<&str> {
        self.classes.get(element_type).map(String::as_str)
    }
}

/// What the mapping and the writer read: the visible graph and the type map.
pub struct ExchangeView<'a> {
    pub core: &'a GraphCore,
    pub types: &'a ArchimateTypeMap,
}

/// The export scope: the tenant and an optional named graph.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportScope {
    pub tenant: String,
    pub graph: Option<String>,
}
