//! Typed knowledge atoms with mandatory provenance (MEDULLA-INGEST-WORKER-R003.1).
//!
//! Decomposition, selection and storage are later rows (R003.2 to R003.6). The stored
//! shape of an atom (R003.3) is a node of type [`ATOM_NODE_TYPE`] named by
//! [`atom_node_id`], linked to its document node by an [`ATOM_OF`] edge; the served read
//! `AtomContext` answers an [`AtomContextResult`].

use serde::{Deserialize, Serialize};

/// The `type` property of a stored knowledge atom node.
pub const ATOM_NODE_TYPE: &str = "KnowledgeAtom";

/// The `relationship` of the edge from an atom node to its document node.
pub const ATOM_OF: &str = "atom_of";

/// The node id of the stored atom `atom_id`.
pub fn atom_node_id(atom_id: &str) -> String {
    format!("knowledge_atom:{atom_id}")
}

/// One atom the context read selected (MEDULLA-INGEST-WORKER-R003.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AtomContextAtom {
    pub atom_id: String,
    pub text: String,
    pub provenance_key: String,
}

/// The answer of `Method::AtomContext` (MEDULLA-INGEST-WORKER-R003.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AtomContextResult {
    pub atoms: Vec<AtomContextAtom>,
    pub atoms_selected: u64,
    pub tokens_used: u64,
}

/// The closed set of atom kinds. An unknown kind fails deserialization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum AtomKind {
    Fact,
    Event,
    Preference,
    Procedure,
}

/// One typed unit of knowledge decomposed from a document or interaction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct KnowledgeAtom {
    pub atom_id: String,
    pub kind: AtomKind,
    pub text: String,
    pub provenance_key: String,
    pub tenant: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // spec: MEDULLA-INGEST-WORKER-R003.1
    #[test]
    fn atom_round_trips_and_rejects_unknown_kind() {
        let atom = KnowledgeAtom {
            atom_id: "atom-1".into(),
            kind: AtomKind::Procedure,
            text: "Restart the worker after upgrade.".into(),
            provenance_key: "doc:runbook#p3".into(),
            tenant: "t1".into(),
        };
        let encoded = serde_json::to_value(&atom).unwrap();
        assert_eq!(encoded["kind"], "procedure");
        assert_eq!(
            serde_json::from_value::<KnowledgeAtom>(encoded.clone()).unwrap(),
            atom
        );
        for kind in ["fact", "event", "preference", "procedure"] {
            let mut value = encoded.clone();
            value["kind"] = json!(kind);
            assert!(serde_json::from_value::<KnowledgeAtom>(value).is_ok());
        }
        let mut unknown = encoded;
        unknown["kind"] = json!("opinion");
        assert!(serde_json::from_value::<KnowledgeAtom>(unknown).is_err());
    }
}
