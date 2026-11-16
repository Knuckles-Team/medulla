//! The catalog write guard (MEDULLA-TYPED-PACKS-R108.4).
//!
//! Two refusals run over the triples of one catalog write before it commits, so a
//! refused write changes nothing. The guard is pure: the caller supplies the
//! class hierarchy test, the markings the writer holds and the marking table of
//! person fields, and the guard decides.
//!
//! * `ASSET_CLASS_ORPHAN`: a catalog node typed with a `cat:` class that is not
//!   under `cat:Asset`.
//! * `FIELD_MARKED`: a field of a `cat:Person` node carries a marking the writer
//!   does not hold.
//!
//! A node with no owner is not refused here: [`crate::emit`] commits it flagged
//! `owner_missing`, and the stewardship report lists it.

use std::collections::BTreeSet;

use medulla_identity_types::tenancy::Marking;

use crate::emit::{Object, Triple, ASSET_NODE_PREFIX, CAT};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
/// The wire code of an orphan asset class.
pub const ASSET_CLASS_ORPHAN: &str = "ASSET_CLASS_ORPHAN";
/// The wire code of a field the writer is not cleared for.
pub const FIELD_MARKED: &str = "FIELD_MARKED";

/// Why a catalog write was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogGuardError {
    /// A catalog node is typed with a class that is not under `cat:Asset`.
    #[error("ASSET_CLASS_ORPHAN: node {node} has class {class}, which is not under cat:Asset")]
    AssetClassOrphan {
        /// The refused node.
        node: String,
        /// Its class IRI.
        class: String,
    },
    /// A person field carries a marking the writer does not hold.
    #[error("FIELD_MARKED: field {predicate} of {node} carries marking {marking}, which the writer does not hold")]
    FieldMarked {
        /// The person node.
        node: String,
        /// The marked field.
        predicate: String,
        /// The marking the writer lacks.
        marking: String,
    },
}

impl CatalogGuardError {
    /// The registered `ServerErrorCode` token for this refusal.
    pub fn code(&self) -> &'static str {
        match self {
            Self::AssetClassOrphan { .. } => ASSET_CLASS_ORPHAN,
            Self::FieldMarked { .. } => FIELD_MARKED,
        }
    }
}

/// What the caller knows about the write: the class hierarchy, the writer's
/// clearance and the marking of each person field.
#[derive(Clone, Copy)]
pub struct CatalogGuardContext<'a> {
    /// True when the class IRI is `cat:Asset` or a subclass of it in the composed schema.
    pub is_asset_class: &'a dyn Fn(&str) -> bool,
    /// The markings the writer holds.
    pub held: &'a BTreeSet<Marking>,
    /// The marking of one field of a `cat:Person` node, when it has one.
    pub field_marking: &'a dyn Fn(&str) -> Option<Marking>,
}

/// Check the triples of one catalog write. The first refusal wins and nothing is
/// written by this function either way.
pub fn check_catalog_write(
    triples: &[Triple],
    context: &CatalogGuardContext<'_>,
) -> Result<(), CatalogGuardError> {
    let person = format!("{CAT}Person");
    let persons: BTreeSet<&str> = triples
        .iter()
        .filter(|t| t.predicate == RDF_TYPE && t.object == Object::Iri(person.clone()))
        .map(|t| t.subject.as_str())
        .collect();
    for triple in triples {
        if triple.predicate != RDF_TYPE || !triple.subject.starts_with(ASSET_NODE_PREFIX) {
            continue;
        }
        if let Object::Iri(class) = &triple.object {
            if class.starts_with(CAT) && !(context.is_asset_class)(class) {
                return Err(CatalogGuardError::AssetClassOrphan {
                    node: triple.subject.clone(),
                    class: class.clone(),
                });
            }
        }
    }
    for triple in triples {
        if triple.predicate == RDF_TYPE || !persons.contains(triple.subject.as_str()) {
            continue;
        }
        if let Some(marking) = (context.field_marking)(&triple.predicate) {
            if !context.held.contains(&marking) {
                return Err(CatalogGuardError::FieldMarked {
                    node: triple.subject.clone(),
                    predicate: triple.predicate.clone(),
                    marking: marking.as_str().to_string(),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marking(value: &str) -> Marking {
        Marking::new(value).expect("valid marking")
    }

    fn typed(subject: &str, class: &str) -> Triple {
        Triple {
            subject: subject.to_string(),
            predicate: RDF_TYPE.to_string(),
            object: Object::Iri(class.to_string()),
        }
    }

    fn field(subject: &str, predicate: &str) -> Triple {
        Triple {
            subject: subject.to_string(),
            predicate: predicate.to_string(),
            object: Object::Literal("value".to_string()),
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R108.4
    #[test]
    fn orphan_class_and_marked_person_field_are_refused_and_held_marking_passes() {
        let held: BTreeSet<Marking> = [marking("internal")].into();
        let email = format!("{CAT}email");
        let is_asset =
            |class: &str| class == format!("{CAT}Product") || class == format!("{CAT}Person");
        let field_marking = |predicate: &str| (predicate == email).then(|| marking("pii"));
        let context = CatalogGuardContext {
            is_asset_class: &is_asset,
            held: &held,
            field_marking: &field_marking,
        };
        let product = format!("{ASSET_NODE_PREFIX}product:p1");
        let person = format!("{ASSET_NODE_PREFIX}person:alice");

        let ok = vec![typed(&product, &format!("{CAT}Product"))];
        assert_eq!(check_catalog_write(&ok, &context), Ok(()));

        let orphan = vec![typed(&product, &format!("{CAT}Widget"))];
        let error = check_catalog_write(&orphan, &context).unwrap_err();
        assert_eq!(error.code(), ASSET_CLASS_ORPHAN);

        let marked = vec![
            typed(&person, &format!("{CAT}Person")),
            field(&person, &email),
        ];
        let error = check_catalog_write(&marked, &context).unwrap_err();
        assert_eq!(error.code(), FIELD_MARKED);

        let cleared_held: BTreeSet<Marking> = [marking("pii")].into();
        let cleared = CatalogGuardContext {
            held: &cleared_held,
            ..context
        };
        assert_eq!(check_catalog_write(&marked, &cleared), Ok(()));

        let unmarked = vec![
            typed(&person, &format!("{CAT}Person")),
            field(&person, &format!("{CAT}displayName")),
        ];
        assert_eq!(check_catalog_write(&unmarked, &context), Ok(()));
    }
}
