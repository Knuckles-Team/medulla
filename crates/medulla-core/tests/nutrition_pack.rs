//! MEDULLA-MEDIA-VOCAB-R655: food composition and nutrient reference types.
//! The nutrition ontology extends core classes, declares the nutrient identifier
//! property, and the nutrient crosswalk maps identifiers to chemical-entity terms
//! and refuses the rest.

use medulla_catalog_types::domain_pack::{
    chemical_entity_for_nutrient, NutrientIdUnmapped, NUTRIENT_CROSSWALK,
};
use medulla_rdf::mapping::parse_turtle;
use medulla_rdf::oxrdf::{NamedNode, Term, Triple};

const KG: &str = "https://knuckles-team.github.io/ontology/core#";
const RDFS_SUBCLASS: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const RDFS_DOMAIN: &str = "http://www.w3.org/2000/01/rdf-schema#domain";
const NUTRITION: &str = include_str!("../ontology/nutrition-v1.ttl");

fn has_iri(triples: &[Triple], subject: &str, predicate: &str, object: &str) -> bool {
    triples.contains(&Triple::new(
        NamedNode::new(subject).unwrap(),
        NamedNode::new(predicate).unwrap(),
        NamedNode::new(object).unwrap(),
    ))
}

fn extends_something(triples: &[Triple], class: &str) -> bool {
    triples.iter().any(|t| {
        t.subject.to_string() == format!("<{class}>")
            && t.predicate.as_str() == RDFS_SUBCLASS
            && matches!(t.object, Term::NamedNode(_))
    })
}

// spec: MEDULLA-MEDIA-VOCAB-R655
#[test]
fn nutrient_ids_map_and_records_conform() {
    let triples = parse_turtle(NUTRITION).expect("nutrition ontology parses");

    for class in [
        "FoodTerm",
        "FoodCompositionRecord",
        "NutrientAmount",
        "NutrientReferenceIntake",
    ] {
        assert!(
            extends_something(&triples, &format!("{KG}{class}")),
            "{class} extends a core class"
        );
    }
    assert!(has_iri(
        &triples,
        &format!("{KG}nutrientId"),
        RDFS_DOMAIN,
        &format!("{KG}NutrientAmount")
    ));

    for (id, iri) in NUTRIENT_CROSSWALK {
        assert_eq!(chemical_entity_for_nutrient(id), Ok(*iri));
        assert!(NamedNode::new(*iri).is_ok(), "{id} maps to an IRI");
    }
    assert_eq!(
        chemical_entity_for_nutrient("not-a-nutrient"),
        Err(NutrientIdUnmapped("not-a-nutrient".to_string()))
    );
}
