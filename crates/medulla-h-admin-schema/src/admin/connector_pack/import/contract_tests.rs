//! Served acceptance of the pack ontology and surface refusals
//! (MEDULLA-TYPED-PACKS-R102.3, R102.5): every refusal comes back as a named
//! violation from `ConnectorPack.Import` and leaves the connector without a
//! head.

use medulla_catalog_types::connector_pack::{
    PackAnnotations, PackEntryKind, PackImportResult, PackViolationCode,
};
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_types::contract::ResourceId;

use super::{bind, build_pack, import, ok, tool, Content, Served, ADMIN, TENANT};

const PREFIXES: &str = "@prefix cp: <https://knuckles-team.github.io/ontology/connector-pack#> .\n\
    @prefix kg: <https://knuckles-team.github.io/ontology/core#> .\n\
    @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
    @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
    @prefix ex: <https://example.org/> .\n";

/// One class under a core class, so the ontology itself is always well formed.
const ORDER_CLASS: &str = "ex:Order a owl:Class ; rdfs:subClassOf kg:Agreement .\n";

fn ontology(connector: &str, statements: &str) -> Content {
    Content {
        kind: PackEntryKind::Ontology,
        uri: format!("ontology://{connector}/surface.ttl"),
        name: "surface.ttl".to_string(),
        media_type: "text/turtle",
        body: format!("{PREFIXES}{statements}").into_bytes(),
        input_schema: None,
        annotations: PackAnnotations::default(),
        references: Vec::new(),
    }
}

/// Import `tool a` and the ontology of `statements` as `connector`.
async fn import_statements(served: &Served, connector: &str, statements: &str) -> PackImportResult {
    bind(served, connector, ADMIN).await;
    let pack = build_pack(
        connector,
        &[
            tool(connector, "a", "Tool a."),
            ontology(connector, statements),
        ],
    );
    ok("Import", import(served, &pack, None).await)
}

/// The violation codes of a refused import; fails if the pack was imported or
/// if the refusal left a head behind.
async fn refusal_codes(connector: &str, statements: &str) -> Vec<PackViolationCode> {
    let served = Served::new();
    let result = import_statements(&served, connector, statements).await;
    let PackImportResult::Rejected { violations, .. } = result else {
        panic!("{connector}: the pack must be refused: {result:?}");
    };
    let store = served.state.write().await.ensure_agent_library().unwrap();
    let status = store
        .connector_pack_status(TENANT, &ResourceId::new(connector).unwrap())
        .unwrap();
    assert!(status.head.is_none(), "{connector}: a refusal wrote a head");
    violations.iter().map(|violation| violation.code).collect()
}

async fn imports(connector: &str, statements: &str) -> bool {
    let served = Served::new();
    let result = import_statements(&served, connector, statements).await;
    matches!(result, PackImportResult::Imported { .. })
}

// spec: MEDULLA-TYPED-PACKS-R102.3
#[tokio::test]
async fn core_class_redeclaration_is_refused() {
    let redeclared = format!("{ORDER_CLASS}kg:Agreement rdfs:subClassOf ex:Thing .\n");
    assert_eq!(
        refusal_codes("r102-core-conflict", &redeclared).await,
        [PackViolationCode::OntologyCoreConflict]
    );
    assert!(
        imports("r102-core-reuse", ORDER_CLASS).await,
        "reusing a core class by IRI is accepted"
    );
}

// spec: MEDULLA-TYPED-PACKS-R102.3
#[tokio::test]
async fn pack_class_without_core_path_is_refused() {
    let codes = refusal_codes("r102-orphan", "ex:Widget a owl:Class .\n").await;
    assert_eq!(codes, [PackViolationCode::OntologyOrphan]);
}

// spec: MEDULLA-TYPED-PACKS-R102.5
#[tokio::test]
async fn surface_refusals_leave_state_unchanged() {
    // (connector, surface statements, the one code the pack is refused with)
    let cases = [
        (
            "r102-ghost",
            "<tool://r102-ghost/ghost> cp:operatesOn ex:Order .\n",
            PackViolationCode::UnresolvedReference,
        ),
        (
            "r102-class",
            "<tool://r102-class/a> cp:operatesOn ex:Missing .\n",
            PackViolationCode::ContractShaclViolation,
        ),
        (
            "r102-foreign-tool",
            "<tool://other-tenant-connector/a> cp:operatesOn ex:Order .\n",
            PackViolationCode::ImporterMismatch,
        ),
        (
            "r102-foreign-node",
            "<https://knuckles-team.github.io/ontology/connector-pack/connector/other-tenant-connector> \
             cp:providesFeature ex:Feature .\n",
            PackViolationCode::ImporterMismatch,
        ),
    ];
    for (connector, surface, expected) in cases {
        let codes = refusal_codes(connector, &format!("{ORDER_CLASS}{surface}")).await;
        assert_eq!(codes, [expected], "{connector}");
    }
    let linked = format!("{ORDER_CLASS}<tool://r102-linked/a> cp:operatesOn ex:Order .\n");
    assert!(
        imports("r102-linked", &linked).await,
        "a resolved link imports"
    );
}
