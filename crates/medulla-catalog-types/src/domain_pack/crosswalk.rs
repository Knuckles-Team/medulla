//! Crosswalk behaviour (MEDULLA-TYPED-PACKS-R071.1.2).
//!
//! Validates a set of [`VocabularyCrosswalk`] records against the pinned source
//! vocabulary and the approved target declarations each record carries. A
//! crosswalk is a reference between independently governed vocabularies: it
//! never implies `owl:sameAs`, class equivalence or a subclass assertion, so
//! nothing here writes or derives schema axioms. The importer never fetches
//! or adopts external definitions; every check reads only the supplied
//! context and records.

use std::collections::{BTreeMap, BTreeSet};

use super::{VocabularyCrosswalk, VocabularyCrosswalkError, VocabularyCrosswalkRelation};

/// Fixed bound: distinct source classes in one crosswalk set.
pub const MAX_CROSSWALK_SOURCE_CLASSES: usize = 4096;
/// Fixed bound: mappings in one crosswalk set.
pub const MAX_CROSSWALK_MAPPINGS: usize = 16384;
/// Fixed bound: bytes in one class IRI.
pub const MAX_CROSSWALK_IRI_BYTES: usize = 2048;

/// The pinned hierarchy fact between a source class and a target class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PinnedHierarchy {
    /// The target class is broader than the source class.
    TargetBroader,
    /// The target class is narrower than the source class.
    TargetNarrower,
}

/// The pinned source vocabulary a crosswalk set is validated against.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CrosswalkContext {
    /// The pinned source vocabulary revision.
    pub source_revision: String,
    /// The authorized source scope name, carried in refusal messages.
    pub scope: String,
    /// Every class of the pinned source revision, with its upper category
    /// (continuant, process, temporal region, ...).
    pub source_classes: BTreeMap<String, String>,
    /// The classes of the selected source set; a subset of `source_classes`.
    pub selected_sources: BTreeSet<String>,
    /// Pinned broader/narrower facts between source and target classes.
    pub hierarchy: BTreeMap<(String, String), PinnedHierarchy>,
}

/// Validate a crosswalk set. Every record passes its model check, resolves its
/// source in the pinned revision and selected scope, resolves its target in
/// its approved declarations with the same category, agrees with the pinned
/// hierarchy, and is not repeated. Every fixed bound refuses; nothing is
/// truncated.
pub fn validate_crosswalks(
    crosswalks: &[VocabularyCrosswalk],
    ctx: &CrosswalkContext,
) -> Result<(), VocabularyCrosswalkError> {
    if let Some(over) = crosswalks.get(MAX_CROSSWALK_MAPPINGS) {
        return Err(VocabularyCrosswalkError::LimitExceeded {
            bound: "mappings",
            iri: over.source_class_iri.clone(),
        });
    }
    let mut sources = BTreeSet::new();
    let mut pairs = BTreeSet::new();
    for crosswalk in crosswalks {
        for iri in [&crosswalk.source_class_iri, &crosswalk.target_class_iri] {
            if iri.len() > MAX_CROSSWALK_IRI_BYTES {
                return Err(VocabularyCrosswalkError::LimitExceeded {
                    bound: "iri_bytes",
                    iri: iri.clone(),
                });
            }
        }
        crosswalk.validate()?;
        if sources.insert(crosswalk.source_class_iri.as_str())
            && sources.len() > MAX_CROSSWALK_SOURCE_CLASSES
        {
            return Err(VocabularyCrosswalkError::LimitExceeded {
                bound: "source_classes",
                iri: crosswalk.source_class_iri.clone(),
            });
        }
        let source_category = resolve_source(crosswalk, ctx)?;
        let target = crosswalk
            .approved_target_declarations
            .iter()
            .find(|declaration| declaration.class_iri == crosswalk.target_class_iri)
            .ok_or_else(|| {
                VocabularyCrosswalkError::TargetUnknown(crosswalk.target_class_iri.clone())
            })?;
        if target.category != source_category {
            return Err(VocabularyCrosswalkError::CategoryConflict {
                target_iri: crosswalk.target_class_iri.clone(),
                source_category: source_category.to_string(),
                target_category: target.category.clone(),
            });
        }
        let conflict = || VocabularyCrosswalkError::Conflict {
            source_iri: crosswalk.source_class_iri.clone(),
            target_iri: crosswalk.target_class_iri.clone(),
        };
        let pinned = ctx.hierarchy.get(&(
            crosswalk.source_class_iri.clone(),
            crosswalk.target_class_iri.clone(),
        ));
        if !relation_agrees(crosswalk.relation, pinned.copied()) {
            return Err(conflict());
        }
        if !pairs.insert((
            crosswalk.source_class_iri.as_str(),
            crosswalk.target_class_iri.as_str(),
        )) {
            return Err(conflict());
        }
    }
    Ok(())
}

fn resolve_source<'a>(
    crosswalk: &VocabularyCrosswalk,
    ctx: &'a CrosswalkContext,
) -> Result<&'a str, VocabularyCrosswalkError> {
    let category = ctx
        .source_classes
        .get(&crosswalk.source_class_iri)
        .filter(|_| crosswalk.source_revision == ctx.source_revision)
        .ok_or_else(|| VocabularyCrosswalkError::SourceUnknown {
            source_iri: crosswalk.source_class_iri.clone(),
            revision: crosswalk.source_revision.clone(),
            scope: ctx.scope.clone(),
        })?;
    if !ctx.selected_sources.contains(&crosswalk.source_class_iri) {
        return Err(VocabularyCrosswalkError::SourceOutOfScope {
            source_iri: crosswalk.source_class_iri.clone(),
            revision: crosswalk.source_revision.clone(),
            scope: ctx.scope.clone(),
        });
    }
    Ok(category)
}

/// `SkosBroadMatch` means the target is broader than the source and
/// `SkosNarrowMatch` the target is narrower. An exact or close match on a pair
/// the pinned hierarchy orders is a conflict; `SeeAlso` asserts nothing.
fn relation_agrees(relation: VocabularyCrosswalkRelation, pinned: Option<PinnedHierarchy>) -> bool {
    use VocabularyCrosswalkRelation as R;
    match (relation, pinned) {
        (R::SeeAlso, _) => true,
        (R::SkosExactMatch | R::SkosCloseMatch, None) => true,
        (R::SkosExactMatch | R::SkosCloseMatch, Some(_)) => false,
        (R::SkosBroadMatch, Some(PinnedHierarchy::TargetBroader)) => true,
        (R::SkosNarrowMatch, Some(PinnedHierarchy::TargetNarrower)) => true,
        (R::SkosBroadMatch | R::SkosNarrowMatch, _) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain_pack::{ApprovedTargetDeclaration, VOCABULARY_CROSSWALK_SCHEMA_VERSION};

    const CORE: &str = "http://knuckles.team/kg#";
    const SCHEMA: &str = "http://schema.org/";
    const REV: &str = "core-crosswalk-fixture-v1";

    fn record(
        source: &str,
        target: &str,
        relation: VocabularyCrosswalkRelation,
        category: &str,
    ) -> VocabularyCrosswalk {
        let mut crosswalk = VocabularyCrosswalk {
            schema_version: VOCABULARY_CROSSWALK_SCHEMA_VERSION,
            source_class_iri: format!("{CORE}{source}"),
            target_class_iri: format!("{SCHEMA}{target}"),
            relation,
            source_revision: REV.to_string(),
            target_vocabulary_revision: "schemaorg-crosswalk-fixture-v1".to_string(),
            origin: "core-foundation-v1".to_string(),
            content_digest: String::new(),
            approved_target_declarations: vec![ApprovedTargetDeclaration {
                class_iri: format!("{SCHEMA}{target}"),
                category: category.to_string(),
            }],
        };
        crosswalk.content_digest = crosswalk.compute_content_digest();
        crosswalk
    }

    fn context() -> CrosswalkContext {
        let classes = [
            ("Person", "continuant"),
            ("Event", "process"),
            ("Document", "continuant"),
            ("Interval", "temporal_region"),
            ("Unselected", "continuant"),
        ];
        let mut hierarchy = BTreeMap::new();
        hierarchy.insert(
            (
                format!("{CORE}Document"),
                format!("{SCHEMA}DigitalDocument"),
            ),
            PinnedHierarchy::TargetNarrower,
        );
        hierarchy.insert(
            (format!("{CORE}Document"), format!("{SCHEMA}CreativeWork")),
            PinnedHierarchy::TargetBroader,
        );
        CrosswalkContext {
            source_revision: REV.to_string(),
            scope: "core".to_string(),
            source_classes: classes
                .iter()
                .map(|(name, category)| (format!("{CORE}{name}"), category.to_string()))
                .collect(),
            selected_sources: ["Person", "Event", "Document", "Interval"]
                .iter()
                .map(|name| format!("{CORE}{name}"))
                .collect(),
            hierarchy,
        }
    }

    fn golden() -> Vec<VocabularyCrosswalk> {
        use VocabularyCrosswalkRelation::SeeAlso;
        vec![
            record("Person", "Person", SeeAlso, "continuant"),
            record("Event", "Event", SeeAlso, "process"),
            record("Document", "CreativeWork", SeeAlso, "continuant"),
        ]
    }

    // spec: MEDULLA-TYPED-PACKS-R071.1.2
    #[test]
    fn golden_person_event_document_tuples() {
        let set = golden();
        validate_crosswalks(&set, &context()).expect("golden set validates");
        let tuples: Vec<(&str, &str, VocabularyCrosswalkRelation)> = set
            .iter()
            .map(|c| {
                (
                    c.source_class_iri.as_str(),
                    c.target_class_iri.as_str(),
                    c.relation,
                )
            })
            .collect();
        assert_eq!(
            tuples,
            vec![
                (
                    "http://knuckles.team/kg#Person",
                    "http://schema.org/Person",
                    VocabularyCrosswalkRelation::SeeAlso
                ),
                (
                    "http://knuckles.team/kg#Event",
                    "http://schema.org/Event",
                    VocabularyCrosswalkRelation::SeeAlso
                ),
                (
                    "http://knuckles.team/kg#Document",
                    "http://schema.org/CreativeWork",
                    VocabularyCrosswalkRelation::SeeAlso
                ),
            ]
        );

        // Document includes physical documents: never an exact match to DigitalDocument.
        let exact = record(
            "Document",
            "DigitalDocument",
            VocabularyCrosswalkRelation::SkosExactMatch,
            "continuant",
        );
        assert!(matches!(
            validate_crosswalks(&[exact], &context()),
            Err(VocabularyCrosswalkError::Conflict { .. })
        ));

        // Continuant, process and temporal-region separation is preserved.
        let crossed = record(
            "Person",
            "Event",
            VocabularyCrosswalkRelation::SeeAlso,
            "process",
        );
        assert!(matches!(
            validate_crosswalks(&[crossed], &context()),
            Err(VocabularyCrosswalkError::CategoryConflict { .. })
        ));
        let temporal = record(
            "Interval",
            "Event",
            VocabularyCrosswalkRelation::SeeAlso,
            "process",
        );
        assert!(matches!(
            validate_crosswalks(&[temporal], &context()),
            Err(VocabularyCrosswalkError::CategoryConflict { .. })
        ));

        // Source resolution: unknown class, stale revision, out of scope; unknown target.
        let unknown = record(
            "Nope",
            "Person",
            VocabularyCrosswalkRelation::SeeAlso,
            "continuant",
        );
        assert!(matches!(
            validate_crosswalks(&[unknown], &context()),
            Err(VocabularyCrosswalkError::SourceUnknown { .. })
        ));
        let mut stale = record(
            "Person",
            "Person",
            VocabularyCrosswalkRelation::SeeAlso,
            "continuant",
        );
        stale.source_revision = "core-crosswalk-fixture-v0".to_string();
        stale.content_digest = stale.compute_content_digest();
        assert!(matches!(
            validate_crosswalks(&[stale], &context()),
            Err(VocabularyCrosswalkError::SourceUnknown { .. })
        ));
        let out = record(
            "Unselected",
            "Person",
            VocabularyCrosswalkRelation::SeeAlso,
            "continuant",
        );
        assert!(matches!(
            validate_crosswalks(&[out], &context()),
            Err(VocabularyCrosswalkError::SourceOutOfScope { .. })
        ));
        let mut no_target = record(
            "Person",
            "Person",
            VocabularyCrosswalkRelation::SeeAlso,
            "continuant",
        );
        no_target.approved_target_declarations[0].class_iri = format!("{SCHEMA}Other");
        no_target.content_digest = no_target.compute_content_digest();
        assert!(matches!(
            validate_crosswalks(&[no_target], &context()),
            Err(VocabularyCrosswalkError::TargetUnknown(_))
        ));
    }

    // spec: MEDULLA-TYPED-PACKS-R071.1.2
    #[test]
    fn broad_and_narrow_direction_use_asymmetric_fixtures() {
        use VocabularyCrosswalkRelation::{SkosBroadMatch, SkosNarrowMatch};
        let ctx = context();
        let broad = record("Document", "CreativeWork", SkosBroadMatch, "continuant");
        validate_crosswalks(&[broad], &ctx).expect("CreativeWork is broader than Document");
        let narrow = record("Document", "DigitalDocument", SkosNarrowMatch, "continuant");
        validate_crosswalks(&[narrow], &ctx).expect("DigitalDocument is narrower than Document");

        let reversed_broad = record("Document", "DigitalDocument", SkosBroadMatch, "continuant");
        assert!(matches!(
            validate_crosswalks(&[reversed_broad], &ctx),
            Err(VocabularyCrosswalkError::Conflict { .. })
        ));
        let reversed_narrow = record("Document", "CreativeWork", SkosNarrowMatch, "continuant");
        assert!(matches!(
            validate_crosswalks(&[reversed_narrow], &ctx),
            Err(VocabularyCrosswalkError::Conflict { .. })
        ));
        let unordered = record("Person", "Person", SkosBroadMatch, "continuant");
        assert!(matches!(
            validate_crosswalks(&[unordered], &ctx),
            Err(VocabularyCrosswalkError::Conflict { .. })
        ));
    }

    // spec: MEDULLA-TYPED-PACKS-R071.1.2
    #[test]
    fn duplicate_input_and_every_bound() {
        let ctx = context();
        let mut set = golden();
        set.push(set[0].clone());
        assert!(matches!(
            validate_crosswalks(&set, &ctx),
            Err(VocabularyCrosswalkError::Conflict { .. })
        ));

        let mut long = golden()[0].clone();
        long.source_class_iri = format!("{CORE}{}", "a".repeat(MAX_CROSSWALK_IRI_BYTES));
        assert!(matches!(
            validate_crosswalks(&[long], &ctx),
            Err(VocabularyCrosswalkError::LimitExceeded {
                bound: "iri_bytes",
                ..
            })
        ));

        let too_many = vec![golden()[0].clone(); MAX_CROSSWALK_MAPPINGS + 1];
        assert!(matches!(
            validate_crosswalks(&too_many, &ctx),
            Err(VocabularyCrosswalkError::LimitExceeded {
                bound: "mappings",
                ..
            })
        ));

        let mut wide = ctx.clone();
        let mut many = Vec::new();
        for i in 0..=MAX_CROSSWALK_SOURCE_CLASSES {
            let name = format!("C{i}");
            wide.source_classes
                .insert(format!("{CORE}{name}"), "continuant".to_string());
            wide.selected_sources.insert(format!("{CORE}{name}"));
            many.push(record(
                &name,
                "Person",
                VocabularyCrosswalkRelation::SeeAlso,
                "continuant",
            ));
        }
        assert!(matches!(
            validate_crosswalks(&many, &wide),
            Err(VocabularyCrosswalkError::LimitExceeded {
                bound: "source_classes",
                ..
            })
        ));
        validate_crosswalks(&many[..MAX_CROSSWALK_SOURCE_CLASSES], &wide)
            .expect("exactly at the bound validates");
    }
}
