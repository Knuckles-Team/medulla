//! The competency question "what can happen next to X" (MEDULLA-ORCH-VOCAB-R811.7).
//!
//! The machine description is loaded as schema triples in the `__commons__` graph
//! (MEDULLA-ORCH-VOCAB-R811.5), not as nodes a `MATCH` can start from, so the question is
//! two served reads with no join between data and schema: the caller reads the record's
//! state text with `GetNode`, then sends one `Method::Sparql` against `__commons__` whose
//! statement text comes from [`next_events_sparql`]. This function is the only place that
//! statement text exists; a client does not write its own. A row of the answer is one
//! edge: its id, its event, its guard (unbound when it has none) and its target's stored
//! text. A state with no outgoing edge returns no row.

/// The core ontology namespace the `:` prefix of the statement is declared as.
pub const CORE_NAMESPACE: &str = "https://knuckles-team.github.io/ontology/core#";

/// The `SPARQL` statement that lists the edges leaving `state_text` in the machine
/// `machine_iri`, in edge id order.
pub fn next_events_sparql(machine_iri: &str, state_text: &str) -> Result<String, String> {
    if machine_iri.is_empty()
        || machine_iri
            .chars()
            .any(|c| "<>\"{}|\\^".contains(c) || c.is_whitespace())
    {
        return Err(
            "INVALID_ARGUMENT: machine_iri must be a non-empty IRI without any of <>\"{}|\\^ or whitespace"
                .to_string(),
        );
    }
    if state_text.is_empty() || state_text.contains('"') || state_text.contains('\\') {
        return Err(
            "INVALID_ARGUMENT: state_text must be non-empty and contain no quote or backslash"
                .to_string(),
        );
    }
    Ok(format!(
        "PREFIX : <{CORE_NAMESPACE}> \
         SELECT ?edge ?event ?guard ?to WHERE {{ \
         <{machine_iri}> :hasState ?from . \
         ?from :storedForm \"{state_text}\" . \
         ?t :fromState ?from ; :firedBy ?event ; :toState ?target ; :edgeId ?edge . \
         ?target :storedForm ?to . \
         OPTIONAL {{ ?t :guardedBy ?guard }} \
         }} ORDER BY ?edge"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MACHINE: &str = "https://knuckles-team.github.io/ontology/core#WorkItemMachine";

    // spec: MEDULLA-ORCH-VOCAB-R811.7
    #[test]
    fn statement_names_the_machine_and_state_and_orders_by_edge() {
        let text = next_events_sparql(MACHINE, "ready").unwrap();
        assert!(text.contains(&format!("<{MACHINE}> :hasState ?from")));
        assert!(text.contains("?from :storedForm \"ready\""));
        assert!(text.contains("OPTIONAL { ?t :guardedBy ?guard }"));
        assert!(text.ends_with("ORDER BY ?edge"));
        assert!(text.starts_with(&format!("PREFIX : <{CORE_NAMESPACE}>")));
        // Equal input gives equal text.
        assert_eq!(text, next_events_sparql(MACHINE, "ready").unwrap());
    }

    // spec: MEDULLA-ORCH-VOCAB-R811.7
    #[test]
    fn unsafe_machine_or_state_text_is_refused() {
        for machine in ["", "a b", "a<b", "a>b", "a\"b", "a{b", "a}b", "a|b", "a\\b", "a^b", "a\tb"] {
            let error = next_events_sparql(machine, "ready").unwrap_err();
            assert!(error.starts_with("INVALID_ARGUMENT"), "{machine:?}: {error}");
        }
        for state in ["", "re\"ady", "re\\ady"] {
            let error = next_events_sparql(MACHINE, state).unwrap_err();
            assert!(error.starts_with("INVALID_ARGUMENT"), "{state:?}: {error}");
        }
    }
}
