// MEDULLA-DECISION-ENGINE-R099.2: a fetched entity document lowers to alignment
// claims chosen by a connector mapping; it never merges entities.

use super::*;
use medulla_source_types::source_ingestion::web_document::EnrichDocumentRequest;

const WIKIDATA: &str = "http://www.wikidata.org/entity/";

fn request() -> EnrichDocumentRequest {
    EnrichDocumentRequest {
        document_id: "doc:fox".into(),
        text_property: None,
        fetch_backend: None,
        max_chunk_bytes: None,
        overlap_bytes: None,
        alignment: None,
    }
}

fn entity_document(body: &Value, media_type: &str) -> HeldDocument {
    let stored = rmp_serde::to_vec_named(&json!({
        "type": "Document",
        "text": body.to_string(),
        "media_type": media_type,
        "url": format!("{WIKIDATA}Q8332"),
    }))
    .unwrap();
    held_document(&request(), &stored).unwrap()
}

fn fox_body() -> Value {
    json!({
        "@id": format!("{WIKIDATA}Q8332"),
        "id": "Q8332",
        "curie": "NCBITaxon:9627",
        "sameAs": ["http://purl.obolibrary.org/obo/NCBITaxon_9627"],
    })
}

fn mapping() -> ConnectorSchemaMapping {
    ConnectorSchemaMapping {
        ontology_class: "Taxon".into(),
        fields: [
            ("@id", "@id"),
            ("id", "wikidataId"),
            ("sameAs", "skos:exactMatch"),
            ("curie", "termCurie"),
        ]
        .iter()
        .map(|(source, target)| (source.to_string(), target.to_string()))
        .collect(),
        datatypes: BTreeMap::new(),
        entity_iri_prefix: Some(WIKIDATA.into()),
    }
}

fn taxon(id: &str, curie: &str) -> LocalEntity {
    LocalEntity {
        node_id: id.into(),
        properties: json!({"type": "Taxon", "termCurie": curie, "scientificName": "Vulpes vulpes"}),
    }
}

fn props(method: &Method) -> Value {
    match method {
        Method::AddNode {
            properties_msgpack, ..
        } => medulla_types::msgpack::decode_property_value(properties_msgpack).unwrap(),
        other => panic!("not a node: {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R099.2
#[test]
fn entity_document_lowers_to_alignment_claims() {
    let document = entity_document(&fox_body(), "application/ld+json; charset=utf-8");
    assert!(document.is_entity_document());
    assert!(!entity_document(&fox_body(), "text/html").is_entity_document());

    // One local entity matches the document's identifying property.
    let local = [
        taxon("redFox", "NCBITaxon:9627"),
        taxon("kitFox", "NCBITaxon:1"),
    ];
    let (write_set, receipt) = lower_alignment("doc:fox", &document, &mapping(), &local).unwrap();
    let write_set = write_set.expect("a matched entity writes claims");
    let methods = write_set.methods();

    // The wikidataId fact: one claim node, class claim, provenance the document digest.
    let digest = document.content_digest();
    let claims: Vec<(&String, Value)> = methods
        .iter()
        .filter_map(|m| match m {
            Method::AddNode { node_id, .. } if props(m)["type"] == ALIGNMENT_CLAIM_TYPE => {
                Some((node_id, props(m)))
            }
            _ => None,
        })
        .collect();
    assert_eq!(claims.len(), 1);
    let (claim_id, claim) = &claims[0];
    assert_eq!(claim["wikidataId"], "Q8332");
    assert_eq!(claim["entity_id"], "redFox");
    assert_eq!(claim["evidence_class"], "claim");
    assert_eq!(claim["content_digest"], digest.as_str());
    assert_eq!(receipt.alignment_claims, vec![claim_id.to_string()]);
    assert!(!receipt.replayed && receipt.chunk_ids.is_empty());

    // One skos:exactMatch edge per exact-match IRI (the entity IRI and the listed one),
    // each carrying the claim class and the document digest.
    let exact: BTreeSet<String> = methods
        .iter()
        .filter_map(|m| match m {
            Method::AddEdge {
                source_id,
                target_id,
                properties_msgpack,
            } => {
                let edge =
                    medulla_types::msgpack::decode_property_value(properties_msgpack).ok()?;
                (edge["relationship"] == EXACT_MATCH).then(|| {
                    assert_eq!(source_id, "redFox");
                    assert_eq!(edge["evidence_class"], "claim");
                    assert_eq!(edge["content_digest"], digest.as_str());
                    target_id.clone()
                })
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        exact,
        BTreeSet::from([
            format!("external-entity:{WIKIDATA}Q8332"),
            "external-entity:http://purl.obolibrary.org/obo/NCBITaxon_9627".to_string(),
        ])
    );
    // Entity resolution decides every merge: nothing is removed or rewritten.
    assert!(methods
        .iter()
        .all(|m| matches!(m, Method::AddNode { .. } | Method::AddEdge { .. })));
    assert!(!methods
        .iter()
        .any(|m| matches!(m, Method::AddNode { node_id, .. } if node_id == "redFox")));

    // The same document and entities lower to the same write-set (a replay commits nothing new).
    let (again, _) = lower_alignment("doc:fox", &document, &mapping(), &local).unwrap();
    assert_eq!(again.unwrap().envelope_id, write_set.envelope_id);

    // A document that matches no local entity writes nothing.
    let (none, receipt) = lower_alignment(
        "doc:fox",
        &document,
        &mapping(),
        &[taxon("kitFox", "NCBITaxon:1")],
    )
    .unwrap();
    assert!(none.is_none() && receipt.alignment_claims.is_empty());

    // ALIGNMENT_NO_MAPPING: the IRI does not match the mapping's pattern.
    let mut elsewhere = fox_body();
    elsewhere["@id"] = json!("http://example.org/entity/Q8332");
    let refused = lower_alignment(
        "doc:fox",
        &entity_document(&elsewhere, "application/json"),
        &mapping(),
        &local,
    )
    .unwrap_err();
    assert!(refused.starts_with("ALIGNMENT_NO_MAPPING"), "{refused}");
    // ... the IRI's key differs from the key field ...
    let mut wrong_key = fox_body();
    wrong_key["id"] = json!("Q1");
    let refused = lower_alignment(
        "doc:fox",
        &entity_document(&wrong_key, "application/json"),
        &mapping(),
        &local,
    )
    .unwrap_err();
    assert!(refused.starts_with("ALIGNMENT_NO_MAPPING"), "{refused}");
    // ... or the mapping declares no IRI pattern.
    let mut patternless = mapping();
    patternless.entity_iri_prefix = None;
    let refused = lower_alignment("doc:fox", &document, &patternless, &local).unwrap_err();
    assert!(refused.starts_with("ALIGNMENT_NO_MAPPING"), "{refused}");

    // ALIGNMENT_AMBIGUOUS: two local entities match the identifying property ...
    let twins = [
        taxon("redFox", "NCBITaxon:9627"),
        taxon("redFoxCopy", "NCBITaxon:9627"),
    ];
    let refused = lower_alignment("doc:fox", &document, &mapping(), &twins).unwrap_err();
    assert!(refused.starts_with("ALIGNMENT_AMBIGUOUS"), "{refused}");
    assert!(refused.contains("redFox") && refused.contains("redFoxCopy"));
    // ... or another local entity already holds the key, by its own fact or by a claim.
    let mut holder = taxon("otherFox", "NCBITaxon:2");
    holder.properties["wikidataId"] = json!("Q8332");
    let refused = lower_alignment(
        "doc:fox",
        &document,
        &mapping(),
        &[taxon("redFox", "NCBITaxon:9627"), holder],
    )
    .unwrap_err();
    assert!(refused.starts_with("ALIGNMENT_AMBIGUOUS"), "{refused}");
    let claimed = LocalEntity {
        node_id: "alignment-claim:otherFox:abc".into(),
        properties: json!({"type": "AlignmentClaim", "entity_id": "otherFox", "wikidataId": "Q8332"}),
    };
    let refused = lower_alignment(
        "doc:fox",
        &document,
        &mapping(),
        &[taxon("redFox", "NCBITaxon:9627"), claimed],
    )
    .unwrap_err();
    assert!(refused.starts_with("ALIGNMENT_AMBIGUOUS"), "{refused}");
}
