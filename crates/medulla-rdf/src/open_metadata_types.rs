//! The open metadata type mapping read from the ontology
//! (MEDULLA-TYPED-PACKS-R110.5).
//!
//! The `cat:openMetadataType` annotations of the metadata catalog ontology
//! name which catalog class stands for which open metadata type. The Turtle is
//! the one source of truth: the sync adapter reads this view and keeps no
//! second table.

use std::sync::OnceLock;

use medulla_catalog_types::open_metadata::OpenMetadataTypeView;
use oxrdf::Term;

use crate::mapping::parse_turtle;

const OPEN_METADATA_TYPE: &str =
    "https://knuckles-team.github.io/ontology/catalog#openMetadataType";

/// The ontology document that carries the annotations.
const ONTOLOGY: &str = include_str!("../../medulla-core/ontology/metadata_catalog-v1.ttl");

/// Resolve the type view from a Turtle document.
pub fn type_view_from_turtle(document: &str) -> Result<OpenMetadataTypeView, String> {
    let mut annotations = Vec::new();
    for triple in parse_turtle(document)? {
        if triple.predicate.as_str() != OPEN_METADATA_TYPE {
            continue;
        }
        let Term::Literal(literal) = &triple.object else {
            continue;
        };
        let class = triple.subject.to_string();
        annotations.push((
            literal.value().to_owned(),
            class.trim_matches(['<', '>']).to_owned(),
        ));
    }
    Ok(OpenMetadataTypeView::from_annotations(annotations))
}

/// The type view of the shipped metadata catalog ontology, read once.
pub fn open_metadata_type_view() -> &'static OpenMetadataTypeView {
    static VIEW: OnceLock<OpenMetadataTypeView> = OnceLock::new();
    VIEW.get_or_init(|| {
        type_view_from_turtle(ONTOLOGY).expect("the shipped metadata catalog ontology parses")
    })
}
