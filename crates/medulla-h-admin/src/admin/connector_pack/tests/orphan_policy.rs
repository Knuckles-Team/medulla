//! MEDULLA-CQ-CONN-R085.4: an orphaned connector ontology and a held source
//! abstain by name. The orphan check refuses with `ONTOLOGY_ORPHAN` and has no
//! environment switch (MEDULLA-TYPED-PACKS-R105.1 deleted `MEDULLA_ONTOLOGY_ORPHANS`);
//! a source whose mapping is not approved stays held, so the live part of an
//! answer abstains.

use medulla_catalog_types::connector_pack::{
    PackAnnotations, PackEntryKind, PackImportResult, PackSourceApproval, PackViolationCode,
};
#[cfg(feature = "redb")]
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_types::contract::ResourceId;

use super::{bind, build_pack, import, imported, ok, tool, Content, Served, ADMIN, TENANT};

const OWL: &str = "http://www.w3.org/2002/07/owl#";
const SUB: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const ACCESS: &str = "https://knuckles-team.github.io/ontology/access#";

fn ontology(connector: &str, body: String) -> Content {
    Content {
        kind: PackEntryKind::Ontology,
        uri: format!("ontology://{connector}/core.ttl"),
        name: "core.ttl".to_string(),
        media_type: "text/turtle",
        body: body.into_bytes(),
        input_schema: None,
        annotations: PackAnnotations::default(),
        references: Vec::new(),
    }
}

/// A class that reaches the parent catalog and, when `orphan`, one that does not.
fn classes(orphan: bool) -> String {
    let mut body = format!(
        "<https://example.org/conn#Incident> a <{OWL}Class> ;\n\
         <{SUB}> <https://knuckles-team.github.io/ontology/core#Ticket> .\n"
    );
    if orphan {
        body.push_str(&format!(
            "<https://example.org/conn#Stray> a <{OWL}Class> .\n"
        ));
    }
    body
}

// spec: MEDULLA-CQ-CONN-R085.4
#[tokio::test]
async fn orphan_and_held_variants_abstain_by_name() {
    // Orphan: the import refuses with ONTOLOGY_ORPHAN naming the unlinked class,
    // and the linked sibling is not named.
    let connector = "r085-orphan";
    let served = Served::new();
    bind(&served, connector, ADMIN).await;
    let pack = build_pack(
        connector,
        &[
            tool(connector, "a", "Tool a."),
            ontology(connector, classes(true)),
        ],
    );
    let PackImportResult::Rejected { violations, .. } =
        ok::<PackImportResult>("Import", import(&served, &pack, None).await)
    else {
        panic!("an orphaned ontology must refuse the import");
    };
    let violations = violations.as_slice();
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert_eq!(violations[0].code, PackViolationCode::OntologyOrphan);
    assert!(
        violations[0].detail.contains("conn#Stray"),
        "{violations:?}"
    );
    assert!(
        !violations[0].detail.contains("conn#Incident"),
        "{violations:?}"
    );

    // Held: a linked ontology with one access contract imports, and the source
    // is registered held until `ApproveSource` admits it.
    let connector = "r085-held";
    let served = Served::new();
    bind(&served, connector, ADMIN).await;
    let contract = format!(
        "@prefix ac: <{ACCESS}> .\n\
         <urn:r085-contract> a ac:AccessContract ; ac:classIri <urn:Incident> ;\n \
         ac:sourceId \"incidents\" ; ac:accessKind \"mcp_tool\" ;\n \
         ac:operation \"search_incidents\" ; ac:entity \"Incident\" ; ac:keyField \"id\" .\n"
    );
    let pack = build_pack(
        connector,
        &[ontology(connector, format!("{}{contract}", classes(false)))],
    );
    imported(&served, &pack, None).await;
    let store = served.state.write().await.ensure_agent_library().unwrap();
    let key = ResourceId::new(connector).unwrap();
    let rows = store.connector_pack_members(TENANT, &key).unwrap();
    let registrations: Vec<_> = rows
        .iter()
        .flat_map(|row| row.source_registrations.iter())
        .collect();
    assert_eq!(registrations.len(), 1, "{rows:?}");
    assert_eq!(registrations[0].approval, PackSourceApproval::Held);
}
