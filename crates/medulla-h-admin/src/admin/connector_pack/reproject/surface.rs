//! The connector surface of a pack head, as triples (MEDULLA-TYPED-PACKS-R102.2,
//! R102.6).
//!
//! One node per tool, prompt, skill and server member. The engine stamps the
//! node's type, `cp:componentId`, entry revision, pack digest and lifecycle;
//! the declared surface links come from the pack's ontology documents.
//! The individuals those documents describe (MEDULLA-MEDIA-VOCAB-R655) ride the
//! same document, so served SPARQL answers questions about pack data.
//! Projection writes the result into the pack graph beside the schema source.

use std::sync::Arc;

use medulla_agent_types::agent_library::AgentLibraryLifecycle;
use medulla_catalog_types::connector_pack::{PackEntryKind, SeedImportReceipt};
use medulla_persistence_records::connector_pack::projection::{
    ConnectorPackProjectionPlan, PackSurfaceMember,
};
use medulla_rdf::oxrdf::{Literal, NamedNode, NamedOrBlankNode, Term, Triple};

use crate::admin::connector_pack::surface_vocab::{link_object, CP, RDF_TYPE};
use crate::graph_schema::PackSchemaDocument;

const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";

/// The class the engine types each surface member with.
fn surface_class(kind: PackEntryKind) -> &'static str {
    match kind {
        PackEntryKind::Tool => "SurfaceTool",
        PackEntryKind::Prompt => "SurfacePrompt",
        PackEntryKind::Skill => "SurfaceSkill",
        _ => "Connector",
    }
}

/// The lifecycle literal a node carries.
fn lifecycle_label(lifecycle: AgentLibraryLifecycle) -> &'static str {
    match lifecycle {
        AgentLibraryLifecycle::Published => "active",
        AgentLibraryLifecycle::Retired => "retired",
        AgentLibraryLifecycle::Withdrawn => "withdrawn",
    }
}

fn cp(local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{CP}{local}"))
}

fn node_of(member: &PackSurfaceMember) -> Result<NamedNode, String> {
    NamedNode::new(&member.uri).map_err(|error| {
        format!(
            "PACK_PROJECTION_SURFACE_IRI_INVALID: member '{}' is not an IRI: {error}",
            member.uri
        )
    })
}

fn stamp(subject: &NamedNode, predicate: &str, value: Literal) -> Triple {
    Triple::new(subject.clone(), cp(predicate), Term::from(value))
}

/// The triples the engine stamps on one member's node.
fn stamped(member: &PackSurfaceMember, digest: &str) -> Result<Vec<Triple>, String> {
    let node = node_of(member)?;
    let revision = Literal::new_typed_literal(
        member.entry_revision.to_string(),
        NamedNode::new_unchecked(XSD_INTEGER),
    );
    Ok(vec![
        Triple::new(
            node.clone(),
            NamedNode::new_unchecked(RDF_TYPE),
            Term::from(cp(surface_class(member.kind))),
        ),
        stamp(
            &node,
            "componentId",
            Literal::new_simple_literal(&member.component_id),
        ),
        stamp(&node, "entryRevision", revision),
        stamp(&node, "packDigest", Literal::new_simple_literal(digest)),
        stamp(
            &node,
            "lifecycle",
            Literal::new_simple_literal(lifecycle_label(member.lifecycle)),
        ),
    ])
}

/// The surface links the pack ontologies declare, in document order.
fn declared_links(ontologies: &[PackSchemaDocument]) -> Result<Vec<Triple>, String> {
    let mut links = Vec::new();
    for document in ontologies {
        let triples = medulla_rdf::mapping::parse_turtle(&document.body)?;
        links.extend(triples.into_iter().filter(|triple| {
            matches!(triple.subject, NamedOrBlankNode::NamedNode(_))
                && link_object(triple.predicate.as_str()).is_some()
        }));
    }
    Ok(links)
}

/// Every triple the pack ontologies declare: the class-level (TBox) triples,
/// the contract `cp:` triples and the named individuals alike
/// (MEDULLA-CQ-CONN-R084.1, MEDULLA-MEDIA-VOCAB-R655).
fn pack_triples(ontologies: &[PackSchemaDocument]) -> Result<Vec<Triple>, String> {
    let mut triples = Vec::new();
    for document in ontologies {
        triples.extend(medulla_rdf::mapping::parse_turtle(&document.body)?);
    }
    Ok(triples)
}

/// The `cp:` triples of the connector declaration's contract (the mapping the
/// import validated, MEDULLA-TYPED-PACKS-R101.2), or none when the declaration
/// carries no contract.
fn contract_triples(connector: &str, declaration: Option<&str>) -> Result<Vec<Triple>, String> {
    let Some(text) = declaration else {
        return Ok(Vec::new());
    };
    let Ok(body) = serde_json::from_str::<serde_json::Value>(text) else {
        return Ok(Vec::new());
    };
    if body.get("contract").is_none() {
        return Ok(Vec::new());
    }
    let graph =
        crate::admin::connector_pack::import::contract_rdf::contract_graph(connector, &body)
            .map_err(|_| {
                "PACK_PROJECTION_FAILED: contract declaration is not mappable to RDF".to_string()
            })?;
    Ok(graph.iter().map(|triple| triple.into_owned()).collect())
}

/// The declared surface links of each tool and skill member of `plan`'s head
/// (MEDULLA-DECISION-ENGINE-R172.12.3): the classes it applies to or operates
/// on, and the classes of the methods it executes. These are the facts the
/// capability feed upserts beside the component record.
pub(super) fn surface_facts(
    plan: &ConnectorPackProjectionPlan,
    ontologies: &[PackSchemaDocument],
) -> Result<Vec<medulla_catalog_types::connector_pack::PackSurfaceFacts>, String> {
    let links = declared_links(ontologies)?;
    let applies = [cp("appliesTo"), cp("operatesOn")];
    let executes = cp("executes");
    let mut facts = Vec::new();
    for member in &plan.surface {
        if !matches!(member.kind, PackEntryKind::Tool | PackEntryKind::Skill) {
            continue;
        }
        let mut entry = medulla_catalog_types::connector_pack::PackSurfaceFacts {
            component_id: member.component_id.clone(),
            applies_to: Default::default(),
            executes: Default::default(),
        };
        for link in &links {
            let NamedOrBlankNode::NamedNode(subject) = &link.subject else {
                continue;
            };
            let Term::NamedNode(object) = &link.object else {
                continue;
            };
            if subject.as_str() != member.uri {
                continue;
            }
            if applies.contains(&link.predicate) {
                entry.applies_to.insert(object.as_str().to_string());
            } else if link.predicate == executes {
                entry.executes.insert(object.as_str().to_string());
            }
        }
        facts.push(entry);
    }
    Ok(facts)
}

/// The stamped surface of `plan`'s head and every triple its ontologies
/// declare as one N-Triples document, or `None` when the head has neither.
pub(super) fn surface_triples(
    plan: &ConnectorPackProjectionPlan,
    ontologies: &[PackSchemaDocument],
    declaration: Option<&str>,
) -> Result<Option<Arc<str>>, String> {
    let mut triples = Vec::new();
    if !plan.surface.is_empty() {
        let digest = plan.pack_digest().to_hex();
        for member in &plan.surface {
            triples.extend(stamped(member, &digest)?);
        }
    }
    triples.extend(super::capability::persist_descriptors(
        &plan.tenant_id,
        pack_triples(ontologies)?,
    )?);
    triples.extend(contract_triples(plan.connector.as_str(), declaration)?);
    if triples.is_empty() {
        return Ok(None);
    }
    let document = medulla_rdf::mapping::to_ntriples(&triples)?;
    Ok(Some(Arc::from(document)))
}

/// Write the projection document into the pack graph, inside the graph gateway.
///
/// The projection of a newer head replaces the whole projected set in this one
/// commit (MEDULLA-MEDIA-VOCAB-R655): every triple the pack graph held goes and the
/// new head's document is written, so a changed value never leaves its old value
/// beside the new one and a triple the new head dropped disappears. A head with
/// nothing to project (`None`) leaves the graph empty. The pack graph holds only
/// projection output; the attached schema sources are kept apart from its triples.
///
/// The seed graph (MEDULLA-TYPED-PACKS-R104.3) is written in the same replacement:
/// its triples, the `seed:SeedRevision` node of this head's seed content and one
/// `seed:fromSeed` link per seed subject. This is the one writer of the pack graph.
pub(crate) fn write_projection_triples(
    core: &medulla_graph_core::graph::GraphCore,
    graph: &str,
    surface: Option<&str>,
    seed: &[PackSchemaDocument],
    receipt: Option<&SeedImportReceipt>,
    connector: &str,
) -> Result<(), String> {
    let mut fresh = match surface {
        Some(document) => medulla_rdf::mapping::parse_ntriples(document)?,
        None => Vec::new(),
    };
    fresh.extend(seed_triples(seed, connector, receipt)?);
    let stale = medulla_rdf::mapping::export_triples(core, graph)?;
    medulla_rdf::update::remove_triples(core, &stale);
    medulla_rdf::update::insert_triples(core, &fresh)?;
    Ok(())
}

/// The seed parts' triples plus the revision node and one `seed:fromSeed` link per
/// seed subject. The revision IRI is `urn:medulla:seed:<escaped connector>:<content
/// digest hex>`, so a changed part is a new revision and an equal head the same one.
///
/// With the head's import `receipt` it also appends one `seed:SeedImportReceipt` node per
/// seed part (MEDULLA-TYPED-PACKS-R104.5) in the same batch: the part token, its outcome,
/// its canonical digest as `sha256:<hex>` and `seed:graphRevision` to the revision node.
/// The receipt must name the content the documents hold.
fn seed_triples(
    seed: &[PackSchemaDocument],
    connector: &str,
    receipt: Option<&SeedImportReceipt>,
) -> Result<Vec<Triple>, String> {
    use crate::admin::connector_pack::seed::{canonical_digest, seed_content_digest, MAX_SEED_TRIPLES};
    use medulla_catalog_types::connector_pack::{seed_receipt_iri, seed_revision_iri, SeedPartKey, SeedPartOutcome};

    if seed.is_empty() {
        return Ok(Vec::new());
    }
    const SEED_NS: &str = medulla_types::ontology_iri!("connector-seed#");
    let node = |iri: &str| {
        NamedNode::new(iri).map_err(|error| format!("seed projection IRI is invalid: {error}"))
    };
    let mut digests = std::collections::BTreeMap::new();
    let mut triples = Vec::new();
    for document in seed {
        let key = SeedPartKey::from_uri(&document.uri).ok_or_else(|| {
            format!("CORRUPT_CONNECTOR_PACK: '{}' is not a seed part", document.uri)
        })?;
        digests.insert(key, canonical_digest(&document.body)?);
        triples.extend(
            medulla_rdf::mapping::parse_turtle_bounded(&document.body, MAX_SEED_TRIPLES)
                .map_err(|error| format!("SEED_TURTLE_INVALID: {error}"))?,
        );
    }
    let content_digest = seed_content_digest(&digests);
    let revision = node(&seed_revision_iri(connector, &content_digest))?;
    let subjects: std::collections::BTreeSet<NamedNode> = triples
        .iter()
        .filter_map(|triple| match &triple.subject {
            NamedOrBlankNode::NamedNode(subject) => Some(subject.clone()),
            _ => None,
        })
        .collect();
    triples.push(Triple::new(
        revision.clone(),
        node(RDF_TYPE)?,
        node(&format!("{SEED_NS}SeedRevision"))?,
    ));
    let from_seed = node(&format!("{SEED_NS}fromSeed"))?;
    for subject in subjects {
        triples.push(Triple::new(subject, from_seed.clone(), revision.clone()));
    }
    if let Some(receipt) = receipt {
        if receipt.content_digest != content_digest {
            return Err(
                "CORRUPT_CONNECTOR_PACK: the seed import receipt names other seed content than the head holds"
                    .into(),
            );
        }
        let receipt_class = node(&format!("{SEED_NS}SeedImportReceipt"))?;
        let rdf_type = node(RDF_TYPE)?;
        let part_property = node(&format!("{SEED_NS}part"))?;
        let outcome_property = node(&format!("{SEED_NS}outcome"))?;
        let digest_property = node(&format!("{SEED_NS}digest"))?;
        let revision_property = node(&format!("{SEED_NS}graphRevision"))?;
        for (key, digest) in &digests {
            let outcome = receipt.outcomes.get(key).ok_or_else(|| {
                format!(
                    "CORRUPT_CONNECTOR_PACK: the seed import receipt has no outcome for part '{}'",
                    key.token()
                )
            })?;
            let outcome = match outcome {
                SeedPartOutcome::Inserted => "inserted",
                SeedPartOutcome::Replaced => "replaced",
                SeedPartOutcome::Unchanged => "unchanged",
            };
            let part_node = node(&seed_receipt_iri(connector, &content_digest, *key))?;
            triples.push(Triple::new(part_node.clone(), rdf_type.clone(), receipt_class.clone()));
            triples.push(Triple::new(
                part_node.clone(),
                part_property.clone(),
                Literal::new_simple_literal(key.token()),
            ));
            triples.push(Triple::new(
                part_node.clone(),
                outcome_property.clone(),
                Literal::new_simple_literal(outcome),
            ));
            triples.push(Triple::new(
                part_node.clone(),
                digest_property.clone(),
                Literal::new_simple_literal(format!("sha256:{}", digest.to_hex())),
            ));
            triples.push(Triple::new(part_node, revision_property.clone(), revision.clone()));
        }
    }
    Ok(triples)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::contract::Digest256;

    #[test]
    fn class_level_triples_and_individuals_are_both_projected() {
        let body = "@prefix : <https://knuckles-team.github.io/ontology/core#> .\n\
            @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
            :Extra a owl:Class ; :appliesToAnalysis :Risk .\n\
            :amount_a a :NutrientAmount ; :nutrientId \"1003\" .\n";
        let documents = [PackSchemaDocument {
            uri: "ontology://food/foods".to_string(),
            body: Arc::from(body),
        }];
        let triples = pack_triples(&documents).unwrap();
        assert_eq!(triples.len(), 4, "{triples:?}");
        let subjects: std::collections::BTreeSet<String> =
            triples.iter().map(|t| t.subject.to_string()).collect();
        assert!(subjects.contains("<https://knuckles-team.github.io/ontology/core#Extra>"));
        assert!(subjects.contains("<https://knuckles-team.github.io/ontology/core#amount_a>"));
    }

    fn individuals(ids: &[&str]) -> String {
        let mut document = String::new();
        for id in ids {
            document.push_str(&format!(
                "<https://knuckles-team.github.io/ontology/core#{id}> <{RDF_TYPE}> <https://knuckles-team.github.io/ontology/core#NutrientAmount> .\n\
                 <https://knuckles-team.github.io/ontology/core#{id}> <https://knuckles-team.github.io/ontology/core#nutrientId> \"1003\" .\n"
            ));
        }
        document
    }

    fn subjects_of(core: &medulla_graph_core::graph::GraphCore) -> Vec<String> {
        let mut out: Vec<String> = medulla_rdf::mapping::export_triples(core, "pack")
            .unwrap()
            .iter()
            .map(|t| t.subject.to_string())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    // spec: MEDULLA-TYPED-PACKS-R104.5
    #[test]
    fn seed_triples_write_one_receipt_node_per_part() {
        use crate::admin::connector_pack::seed::{canonical_digest, seed_content_digest};
        use medulla_catalog_types::connector_pack::{
            seed_receipt_iri, seed_revision_iri, SeedPartKey, SeedPartOutcome,
        };

        let bodies = [
            ("seed://identity", "<urn:seed:instance> a <urn:seed:Identity> .\n"),
            ("seed://catalog", "<urn:seed:item> a <urn:seed:CatalogEntry> .\n"),
            (
                "seed://references",
                "<urn:seed:item> <urn:seed:references> <urn:source:item> .\n",
            ),
        ];
        let documents: Vec<PackSchemaDocument> = bodies
            .iter()
            .map(|(uri, body)| PackSchemaDocument {
                uri: (*uri).to_string(),
                body: Arc::from(*body),
            })
            .collect();
        let digests: std::collections::BTreeMap<_, _> = bodies
            .iter()
            .map(|(uri, body)| {
                (SeedPartKey::from_uri(uri).unwrap(), canonical_digest(body).unwrap())
            })
            .collect();
        let content_digest = seed_content_digest(&digests);
        let receipt = SeedImportReceipt {
            connector: medulla_types::contract::ResourceId::new("demo").unwrap(),
            content_digest,
            outcomes: [
                (SeedPartKey::Identity, SeedPartOutcome::Unchanged),
                (SeedPartKey::Catalog, SeedPartOutcome::Replaced),
                (SeedPartKey::References, SeedPartOutcome::Inserted),
            ]
            .into_iter()
            .collect(),
            graph_revision: seed_revision_iri("demo", &content_digest),
        };
        let triples = seed_triples(&documents, "demo", Some(&receipt)).unwrap();
        let seed_ns = medulla_types::ontology_iri!("connector-seed#");
        let of = |subject: &str, property: &str| -> Vec<String> {
            triples
                .iter()
                .filter(|t| {
                    t.subject.to_string() == format!("<{subject}>")
                        && t.predicate.as_str() == format!("{seed_ns}{property}")
                })
                .map(|t| t.object.to_string())
                .collect()
        };
        let receipt_nodes = triples
            .iter()
            .filter(|t| t.object.to_string() == format!("<{seed_ns}SeedImportReceipt>"))
            .count();
        assert_eq!(receipt_nodes, 3);
        for (key, outcome) in [
            (SeedPartKey::Identity, "unchanged"),
            (SeedPartKey::Catalog, "replaced"),
            (SeedPartKey::References, "inserted"),
        ] {
            let node = seed_receipt_iri("demo", &content_digest, key);
            assert_eq!(of(&node, "part"), vec![format!("\"{}\"", key.token())]);
            assert_eq!(of(&node, "outcome"), vec![format!("\"{outcome}\"")]);
            assert_eq!(
                of(&node, "digest"),
                vec![format!("\"sha256:{}\"", digests[&key].to_hex())]
            );
            assert_eq!(
                of(&node, "graphRevision"),
                vec![format!("<{}>", receipt.graph_revision)]
            );
        }

        // A receipt that names other content than the documents hold is refused.
        let mut other = receipt.clone();
        other.content_digest = Digest256::from_bytes([9; 32]);
        assert!(seed_triples(&documents, "demo", Some(&other)).is_err());
        // No receipt, no receipt nodes.
        let bare = seed_triples(&documents, "demo", None).unwrap();
        assert!(bare.iter().all(|t| t.object.to_string() != format!("<{seed_ns}SeedImportReceipt>")));
    }

    // spec: MEDULLA-MEDIA-VOCAB-R655
    #[test]
    fn a_newer_head_drops_the_triples_it_no_longer_names() {
        let core = medulla_graph_core::graph::GraphCore::new();
        write_projection_triples(&core, "pack", Some(&individuals(&["amount_a", "amount_b"])), &[], None, "demo").unwrap();
        assert_eq!(subjects_of(&core).len(), 2);
        write_projection_triples(&core, "pack", Some(&individuals(&["amount_a"])), &[], None, "demo").unwrap();
        assert_eq!(
            subjects_of(&core),
            vec!["<https://knuckles-team.github.io/ontology/core#amount_a>".to_string()],
            "amount_b was dropped from the newer head"
        );
        write_projection_triples(&core, "pack", None, &[], None, "demo").unwrap();
        assert!(
            subjects_of(&core).is_empty(),
            "a head with nothing to project clears the graph"
        );
    }
}
