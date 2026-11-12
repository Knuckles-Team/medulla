//! Food-composition records in a pack ontology (MEDULLA-MEDIA-VOCAB-R655): every
//! `:NutrientAmount` individual names its source nutrient identifier
//! (`:nutrientId`) and the chemical-entity term it measures
//! (`:amountOfNutrient`). The identifier must have a crosswalk entry, and the
//! term must be the one the crosswalk names.

use medulla_catalog_types::domain_pack::chemical_entity_for_nutrient;
use medulla_rdf::oxrdf::{NamedNodeRef, Term};

const KG: &str = medulla_types::ontology_iri!("core#");
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// The violations of the `:NutrientAmount` individuals of one Turtle document.
/// An unparseable document yields none: the RDF validation reports it.
pub(super) fn nutrient_amount_violations(text: &str) -> Vec<String> {
    let Ok(triples) = medulla_rdf::mapping::parse_turtle(text) else {
        return Vec::new();
    };
    let amount_class = format!("{KG}NutrientAmount");
    let id_property = format!("{KG}nutrientId");
    let nutrient_property = format!("{KG}amountOfNutrient");
    let is_amount = |triple: &medulla_rdf::oxrdf::Triple| {
        triple.predicate.as_str() == RDF_TYPE
            && matches!(&triple.object, Term::NamedNode(node) if node.as_str() == amount_class)
    };
    let mut violations = Vec::new();
    for amount in triples.iter().filter(|triple| is_amount(triple)) {
        let object_of = |property: &str| {
            triples
                .iter()
                .find(|t| t.subject == amount.subject && t.predicate.as_str() == property)
                .map(|t| t.object.clone())
        };
        let subject = amount.subject.to_string();
        let Some(Term::Literal(id)) = object_of(&id_property) else {
            violations.push(format!(
                "NUTRIENT_ID_UNMAPPED: {subject} has no :nutrientId"
            ));
            continue;
        };
        match chemical_entity_for_nutrient(id.value()) {
            Err(error) => violations.push(format!("NUTRIENT_ID_UNMAPPED: {subject}: {error}")),
            Ok(expected) => {
                let measured = object_of(&nutrient_property);
                let agrees = matches!(
                    &measured,
                    Some(Term::NamedNode(node))
                        if NamedNodeRef::new(expected).is_ok_and(|e| e == node.as_ref())
                );
                if !agrees {
                    violations.push(format!(
                        "NUTRIENT_ID_UNMAPPED: {subject} measures a term other than {expected}"
                    ));
                }
            }
        }
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFIXES: &str = "@prefix : <https://knuckles-team.github.io/ontology/core#> .\n\
        @prefix obo: <http://purl.obolibrary.org/obo/> .\n";

    fn amount(id: &str, nutrient: &str) -> String {
        format!(
            "{PREFIXES}:a1 a :NutrientAmount ; :nutrientId \"{id}\" ; :amountOfNutrient obo:{nutrient} .\n"
        )
    }

    #[test]
    fn a_mapped_identifier_with_its_term_passes() {
        assert!(nutrient_amount_violations(&amount("1003", "CHEBI_36080")).is_empty());
    }

    #[test]
    fn an_unmapped_identifier_is_refused() {
        let found = nutrient_amount_violations(&amount("9999", "CHEBI_36080"));
        assert_eq!(found.len(), 1);
        assert!(found[0].starts_with("NUTRIENT_ID_UNMAPPED"));
    }

    #[test]
    fn a_term_other_than_the_crosswalk_target_is_refused() {
        assert_eq!(
            nutrient_amount_violations(&amount("1003", "CHEBI_18059")).len(),
            1
        );
    }
}
