//! The nutrient identifier crosswalk (MEDULLA-MEDIA-VOCAB-R655): food-composition
//! sources identify a nutrient by their own number (USDA FoodData Central
//! nutrient numbers); the graph names it by a chemical-entity term. This table is
//! the one place the two meet. A source identifier with no row here is refused,
//! never guessed.

macro_rules! chebi {
    ($id:literal) => {
        concat!("http://purl.obolibrary.org/obo/CHEBI_", $id)
    };
}

/// `(source nutrient number, chemical-entity term IRI)`, sorted by number.
pub const NUTRIENT_CROSSWALK: &[(&str, &str)] = &[
    ("1003", chebi!("36080")), // protein
    ("1004", chebi!("18059")), // total lipid
    ("1005", chebi!("16646")), // carbohydrate
    ("1087", chebi!("22984")), // calcium
    ("1089", chebi!("18248")), // iron
    ("1092", chebi!("26216")), // potassium
    ("1093", chebi!("26708")), // sodium
    ("1105", chebi!("17336")), // retinol
    ("1162", chebi!("29073")), // L-ascorbic acid
    ("1253", chebi!("16113")), // cholesterol
    ("255", chebi!("15377")),  // water
];

/// A source nutrient identifier without a crosswalk entry.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("nutrient identifier {0:?} has no chemical-entity crosswalk entry")]
pub struct NutrientIdUnmapped(pub String);

/// The chemical-entity term IRI of one source nutrient identifier.
pub fn chemical_entity_for_nutrient(nutrient_id: &str) -> Result<&'static str, NutrientIdUnmapped> {
    NUTRIENT_CROSSWALK
        .iter()
        .find(|(id, _)| *id == nutrient_id)
        .map(|(_, iri)| *iri)
        .ok_or_else(|| NutrientIdUnmapped(nutrient_id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chemical-entity namespace the table targets.
    const CHEBI: &str = "http://purl.obolibrary.org/obo/CHEBI_";

    #[test]
    fn every_row_targets_a_chemical_entity_and_ids_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for (id, iri) in NUTRIENT_CROSSWALK {
            assert!(iri.starts_with(CHEBI), "{id}");
            assert!(seen.insert(*id), "{id} repeated");
        }
    }

    #[test]
    fn an_unlisted_identifier_is_refused() {
        assert!(chemical_entity_for_nutrient("1003").is_ok());
        assert_eq!(
            chemical_entity_for_nutrient("9999"),
            Err(NutrientIdUnmapped("9999".to_string()))
        );
    }
}
