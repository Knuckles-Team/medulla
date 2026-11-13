//! The pure mapping evaluator of a domain pack manifest
//! (MEDULLA-TYPED-PACKS-R097.700).
//!
//! `evaluate_mapping` turns source rows into proposed entities, properties and
//! relationships. It is linear in rows times mappings, allocates only the
//! facts it returns, and writes nothing. Markdown becomes `Fragment` entities
//! of an `Artifact` (`fragmentOf`, `fragmentOrdinal`).

use std::collections::BTreeSet;

use medulla_catalog_types::domain_pack::{
    DomainPackManifest, ManifestMapping, ProposedFact, SourceRow,
};
use serde_json::Value;

/// Why an evaluation is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MappingError {
    /// A manifest case evaluated to other facts than it declares.
    CaseFailed { case: String },
}

/// The entity id one row yields for one class.
fn entity_id(manifest: &DomainPackManifest, class: &str, row: &SourceRow) -> String {
    format!("{}:{}:{}", manifest.pack_id, class, row.row_id)
}

/// The scalar values a selector reads from a row: the value itself, or each
/// scalar of an array. Objects and nulls yield nothing.
fn selected(mapping: &ManifestMapping, row: &SourceRow) -> Vec<Value> {
    let found: Option<Value> = if mapping.selector.starts_with('/') {
        Value::Object(row.fields.clone().into_iter().collect())
            .pointer(&mapping.selector)
            .cloned()
    } else {
        row.fields.get(&mapping.selector).cloned()
    };
    match found {
        Some(Value::Array(items)) => items.into_iter().filter(is_scalar).collect(),
        Some(value) if is_scalar(&value) => vec![value],
        _ => Vec::new(),
    }
}

fn is_scalar(value: &Value) -> bool {
    matches!(value, Value::String(_) | Value::Number(_) | Value::Bool(_))
}

fn as_id(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Evaluate `manifest`'s mappings over `rows`. Each row yields one entity per
/// target class it feeds (announced once, before its first property), then its
/// properties and relationships in mapping order.
pub fn evaluate_mapping(manifest: &DomainPackManifest, rows: &[SourceRow]) -> Vec<ProposedFact> {
    let mut facts = Vec::new();
    for row in rows {
        let mut announced = BTreeSet::new();
        for mapping in manifest
            .mappings
            .iter()
            .filter(|mapping| mapping.source_kind == row.source_kind)
        {
            for value in selected(mapping, row) {
                let id = entity_id(manifest, &mapping.target_class, row);
                if announced.insert(mapping.target_class.clone()) {
                    facts.push(ProposedFact::Entity {
                        entity_id: id.clone(),
                        class: mapping.target_class.clone(),
                    });
                }
                let target = &mapping.target_property_or_relation;
                facts.push(if manifest.is_relation(&mapping.target_class, target) {
                    ProposedFact::Relationship {
                        from: id,
                        relation: target.clone(),
                        to: as_id(&value),
                    }
                } else {
                    ProposedFact::Property {
                        entity_id: id,
                        property: target.clone(),
                        value,
                    }
                });
            }
        }
    }
    facts
}

/// Evaluate every conformance case of the manifest; returns how many passed or
/// the first case that did not.
pub fn run_cases(manifest: &DomainPackManifest) -> Result<u32, MappingError> {
    let mut passed = 0;
    for case in &manifest.cases {
        if evaluate_mapping(manifest, &case.rows) != case.expected {
            return Err(MappingError::CaseFailed {
                case: case.name.clone(),
            });
        }
        passed += 1;
    }
    Ok(passed)
}

/// Split markdown at headings into `Fragment` entities of one `Artifact`.
/// Text before the first heading is fragment 0. Ids are `<artifact>#<ordinal>`.
pub fn fragment_markdown(artifact_id: &str, text: &str) -> Vec<ProposedFact> {
    let mut sections: Vec<(Option<String>, String)> = vec![(None, String::new())];
    for line in text.lines() {
        if let Some(heading) = line.strip_prefix('#') {
            sections.push((
                Some(heading.trim_start_matches('#').trim().to_string()),
                String::new(),
            ));
        } else if let Some((_, body)) = sections.last_mut() {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(line);
        }
    }
    let mut facts = vec![ProposedFact::Entity {
        entity_id: artifact_id.to_string(),
        class: "Artifact".into(),
    }];
    let mut ordinal = 0_u64;
    for (heading, body) in sections {
        if heading.is_none() && body.trim().is_empty() {
            continue;
        }
        let id = format!("{artifact_id}#{ordinal}");
        facts.push(ProposedFact::Entity {
            entity_id: id.clone(),
            class: "Fragment".into(),
        });
        facts.push(ProposedFact::Property {
            entity_id: id.clone(),
            property: "fragmentOrdinal".into(),
            value: Value::from(ordinal),
        });
        if let Some(heading) = heading {
            facts.push(ProposedFact::Property {
                entity_id: id.clone(),
                property: "heading".into(),
                value: Value::String(heading),
            });
        }
        facts.push(ProposedFact::Property {
            entity_id: id.clone(),
            property: "text".into(),
            value: Value::String(body),
        });
        facts.push(ProposedFact::Relationship {
            from: id,
            relation: "fragmentOf".into(),
            to: artifact_id.to_string(),
        });
        ordinal += 1;
    }
    facts
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_catalog_types::domain_pack::{ManifestCase, ManifestClass, SourceKind};
    use std::collections::BTreeMap;

    fn row(id: &str, kind: SourceKind, fields: &[(&str, Value)]) -> SourceRow {
        SourceRow {
            row_id: id.into(),
            source_kind: kind,
            fields: fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    fn manifest(cases: Vec<ManifestCase>) -> DomainPackManifest {
        let mapping = |kind, selector: &str, target: &str| ManifestMapping {
            source_kind: kind,
            selector: selector.into(),
            target_class: "Person".into(),
            target_property_or_relation: target.into(),
        };
        let mut manifest = DomainPackManifest {
            schema_version: 1,
            pack_id: "hr".into(),
            version: "1".into(),
            integrity_sha256: String::new(),
            classes: vec![ManifestClass {
                name: "Person".into(),
                properties: vec!["name".into(), "grade".into()],
                relations: vec!["reportsTo".into()],
            }],
            mappings: vec![
                mapping(SourceKind::Column, "full_name", "name"),
                mapping(SourceKind::Column, "manager", "reportsTo"),
                mapping(SourceKind::JsonField, "/job/grade", "grade"),
            ],
            cases,
        };
        manifest.integrity_sha256 = manifest.compute_integrity();
        manifest
    }

    // spec: MEDULLA-TYPED-PACKS-R097.700
    #[test]
    fn mapping_cases_evaluate_to_expected_entities() {
        let rows = vec![
            row(
                "r1",
                SourceKind::Column,
                &[
                    ("full_name", Value::from("Ada")),
                    ("manager", Value::from("hr:Person:r0")),
                    ("ignored", Value::from("x")),
                ],
            ),
            row(
                "r2",
                SourceKind::JsonField,
                &[("job", serde_json::json!({"grade": 7}))],
            ),
            // a kind no mapping reads, and a selector the row lacks: no facts.
            row(
                "r3",
                SourceKind::Heading,
                &[("full_name", Value::from("Z"))],
            ),
            row("r4", SourceKind::Column, &[("other", Value::from("y"))]),
        ];
        let expected = vec![
            ProposedFact::Entity {
                entity_id: "hr:Person:r1".into(),
                class: "Person".into(),
            },
            ProposedFact::Property {
                entity_id: "hr:Person:r1".into(),
                property: "name".into(),
                value: Value::from("Ada"),
            },
            ProposedFact::Relationship {
                from: "hr:Person:r1".into(),
                relation: "reportsTo".into(),
                to: "hr:Person:r0".into(),
            },
            ProposedFact::Entity {
                entity_id: "hr:Person:r2".into(),
                class: "Person".into(),
            },
            ProposedFact::Property {
                entity_id: "hr:Person:r2".into(),
                property: "grade".into(),
                value: Value::from(7),
            },
        ];
        let case = ManifestCase {
            name: "people".into(),
            rows: rows.clone(),
            expected: expected.clone(),
        };
        let manifest = manifest(vec![case]);
        assert_eq!(evaluate_mapping(&manifest, &rows), expected);
        assert_eq!(run_cases(&manifest), Ok(1));

        // a case whose expectation is wrong is named, not skipped.
        let broken = ManifestCase {
            name: "broken".into(),
            rows,
            expected: vec![],
        };
        assert_eq!(
            run_cases(&self::manifest(vec![broken])),
            Err(MappingError::CaseFailed {
                case: "broken".into()
            })
        );

        // linear bound: 1,000 rows evaluate to a fixed number of facts per row.
        let many: Vec<SourceRow> = (0..1_000)
            .map(|i| {
                row(
                    &format!("n{i}"),
                    SourceKind::Column,
                    &[("full_name", Value::from("p"))],
                )
            })
            .collect();
        assert_eq!(evaluate_mapping(&manifest, &many).len(), 2_000);

        // markdown becomes fragments of one artifact.
        let facts = fragment_markdown("doc:1", "intro\n# One\nbody\n## Two\nmore");
        let fragments = facts
            .iter()
            .filter(|f| matches!(f, ProposedFact::Entity { class, .. } if class == "Fragment"))
            .count();
        assert_eq!(fragments, 3);
        assert!(facts.contains(&ProposedFact::Relationship {
            from: "doc:1#2".into(),
            relation: "fragmentOf".into(),
            to: "doc:1".into(),
        }));
    }
}
