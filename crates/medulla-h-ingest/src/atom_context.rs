//! The context read over stored knowledge atoms (MEDULLA-INGEST-WORKER-R003.3).
//!
//! `Method::AtomContext{query, token_budget}` lists the atoms visible to the caller, ranks
//! them against the query with [`select_atoms`] and answers each selected atom with its
//! provenance key. The read writes nothing.

use medulla_ingestion_types::ingest_rules::select_atoms;
use medulla_ingestion_types::knowledge_atom::{AtomContextAtom, AtomContextResult, KnowledgeAtom};

/// Select the atoms of `visible` that answer `query` inside `token_budget` words. The caller
/// has already narrowed `visible` to the atoms its tenant and policy admit.
pub fn atom_context(
    query: &str,
    token_budget: usize,
    visible: &[KnowledgeAtom],
) -> AtomContextResult {
    let selected = select_atoms(query, visible, token_budget);
    let tokens_used = selected
        .iter()
        .map(|atom| atom.text.split_whitespace().count() as u64)
        .sum();
    AtomContextResult {
        atoms_selected: selected.len() as u64,
        tokens_used,
        atoms: selected
            .into_iter()
            .map(|atom| AtomContextAtom {
                atom_id: atom.atom_id.clone(),
                text: atom.text.clone(),
                provenance_key: atom.provenance_key.clone(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_ingestion_types::knowledge_atom::AtomKind;

    fn atom(id: &str, text: &str) -> KnowledgeAtom {
        KnowledgeAtom {
            atom_id: id.into(),
            kind: AtomKind::Fact,
            text: text.into(),
            provenance_key: format!("doc-1#{id}"),
            tenant: "t1".into(),
        }
    }

    // spec: MEDULLA-INGEST-WORKER-R003.3
    #[test]
    fn selected_atoms_cite_their_provenance_inside_the_budget() {
        let atoms = [
            atom("a", "the worker restarts after upgrade"),
            atom("b", "unrelated sentence about lunch"),
            atom("c", "restarts restarts need a drain"),
        ];
        let result = atom_context("restarts", 7, &atoms);
        assert_eq!(result.atoms_selected, 1);
        assert_eq!(result.atoms[0].atom_id, "c");
        assert_eq!(result.atoms[0].provenance_key, "doc-1#c");
        assert_eq!(result.tokens_used, 5);
        assert!(atom_context("absent", 100, &atoms).atoms.is_empty());
    }
}
