//! Source authority acceptance (MEDULLA-SRC-AUTH-R004.5, R009.1, R009.2): authority
//! rows are graph data. The same question text selects a different source when
//! only the ranks in the graph change.
#![cfg(feature = "rdf")]

use std::sync::Arc;

use medulla_graph_core::graph::GraphSchemaSource;
use medulla_graph_core::graph::GraphSchemaSources;
use medulla_graph_core::graph::SchemaSourceOrigin;
use medulla_virtual_graph::source_selection::{
    audit_from, fixture_from_schema_sources, select_sources, select_sources_with, AuthorityOrigin,
    AuthorityRow, CoverageFixture, CoverageResult, SelectionQuery,
};

const TEMPLATE: &str = include_str!("fixtures/source_authority_incident.ttl");
const SUPPLIER_SEED: &str = include_str!("fixtures/xsrc_authority_supplier.ttl");
const COMPONENT_SEED: &str = include_str!("fixtures/xsrc_authority_component.ttl");
const SERVICE_SEED: &str = include_str!("fixtures/xsrc_authority_service.ttl");
const WORKLOAD_SEED: &str = include_str!("fixtures/xsrc_authority_workload.ttl");
const CAPABILITY_SEED: &str = include_str!("fixtures/xsrc_authority_capability.ttl");
const SHAPES: &str = include_str!("../../medulla-core/ontology/virtual_graph-v1.shapes.ttl");
const INCIDENT: &str = "https://knuckles-team.github.io/ontology/core#Incident";
const SNOW_ROW: &str = "urn:eg:fixture:incident:incident-servicenow";
const PD_ROW: &str = "urn:eg:fixture:incident:incident-pagerduty";

fn document(servicenow: u8, pagerduty: u8) -> String {
    TEMPLATE
        .replace("RANK_SERVICENOW", &servicenow.to_string())
        .replace("RANK_PAGERDUTY", &pagerduty.to_string())
}

/// The question: the ontology class only, never a source or connector name.
fn question() -> (Vec<String>, SelectionQuery) {
    (vec![INCIDENT.to_string()], SelectionQuery::default())
}

fn incident_verdict(fixture: &CoverageFixture) -> CoverageResult {
    let (roots, _) = question();
    select_sources(fixture, &roots, 1)
        .into_iter()
        .find(|v| v.class() == INCIDENT)
        .expect("an Incident verdict")
}

fn attached(document: &str) -> GraphSchemaSources {
    attached_under(document, SchemaSourceOrigin::Operator)
}

fn attached_under(document: &str, origin: SchemaSourceOrigin) -> GraphSchemaSources {
    let mut sources = GraphSchemaSources::default();
    let source = GraphSchemaSource::new(origin, None, Some(Arc::from(document)), 0).unwrap();
    sources
        .attach_dynamic("operator".to_string(), source)
        .expect("the source attaches");
    sources
}

// spec: MEDULLA-SRC-AUTH-R009.1
#[test]
fn the_incident_fixture_parses_passes_the_shapes_and_selects_rank_one() {
    let doc = document(1, 2);
    let report = medulla_shacl::validate_turtle(SHAPES, &doc).expect("validates");
    assert!(report.conforms, "{:?}", report.results);
    let fixture = fixture_from_schema_sources(&attached(&doc)).unwrap();
    assert_eq!(fixture.rows().count(), 2);
    match incident_verdict(&fixture) {
        CoverageResult::Covered {
            source, alternates, ..
        } => {
            assert_eq!(source, "servicenow-mcp");
            assert_eq!(alternates, ["pagerduty-mcp"]);
        }
        other => panic!("expected ServiceNow, got {other:?}"),
    }
}

// spec: MEDULLA-SRC-AUTH-R009.2
#[test]
fn swapping_two_ranks_changes_sources_used_with_equal_query_text() {
    let before_question = question();
    let before = fixture_from_schema_sources(&attached(&document(1, 2))).unwrap();
    let before_audit = audit_from(&[incident_verdict(&before)]);
    let after_question = question();
    let after = fixture_from_schema_sources(&attached(&document(2, 1))).unwrap();
    let after_audit = audit_from(&[incident_verdict(&after)]);
    assert_eq!(before_question, after_question, "the query text is equal");
    let used = |audit: &medulla_identity_types::authority_audit::AuthorityAudit| {
        audit
            .used
            .iter()
            .map(|e| e.source.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(used(&before_audit), ["servicenow-mcp"]);
    assert_eq!(used(&after_audit), ["pagerduty-mcp"]);
    assert!(!before_audit.used[0].premises.is_empty());
    assert!(!after_audit.used[0].premises.is_empty());
}

// spec: MEDULLA-SRC-AUTH-R004.5
#[test]
fn the_live_read_returns_the_same_verdict_as_the_fixture() {
    let live = fixture_from_schema_sources(&attached(&document(1, 2))).unwrap();
    let row = |id: &str, mapping: &str, source: &str, rank: u64| AuthorityRow {
        row: id.to_string(),
        mapping_name: format!("urn:eg:fixture:incident:{mapping}"),
        contract: Some("mcp".to_string()),
        ..AuthorityRow::new(INCIDENT, mapping, source, 0, rank).origin(AuthorityOrigin::Operator)
    };
    let hand = CoverageFixture::new()
        .with_row(row(SNOW_ROW, "snow-incidents", "servicenow-mcp", 1))
        .with_row(row(PD_ROW, "pd-incidents", "pagerduty-mcp", 2))
        .with_source_kind("servicenow-mcp", "mcp")
        .with_source_kind("pagerduty-mcp", "mcp");
    let live_verdict = incident_verdict(&live);
    let hand_verdict = incident_verdict(&hand);
    let strip_subclass = |v: CoverageResult| match v {
        CoverageResult::Covered {
            source,
            mapping_name,
            alternates,
            premises,
            mode,
            ..
        } => (source, mapping_name, alternates, premises, mode),
        other => panic!("expected Covered, got {other:?}"),
    };
    assert_eq!(strip_subclass(live_verdict), strip_subclass(hand_verdict));
    // A pack-origin read of the same text proposes only.
    let pack = SchemaSourceOrigin::Pack {
        connector: "incident-pack".to_string(),
        record_id: "pack-1".to_string(),
        tenant: None,
    };
    let fixture = fixture_from_schema_sources(&attached_under(&document(1, 2), pack)).unwrap();
    assert!(matches!(
        incident_verdict(&fixture),
        CoverageResult::Uncovered { .. }
    ));
}

const CORE: &str = "https://knuckles-team.github.io/ontology/core#";
const INFRA: &str = "https://knuckles-team.github.io/ontology/infrastructure#";
const RDFS_LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";

/// Validates a seed against the shapes, then reads its rows under `origin`.
fn seed_fixture(seed: &str, origin: SchemaSourceOrigin) -> CoverageFixture {
    let report = medulla_shacl::validate_turtle(SHAPES, seed).expect("validates");
    assert!(report.conforms, "{:?}", report.results);
    fixture_from_schema_sources(&attached_under(seed, origin)).expect("the seed attaches")
}

fn verdicts(
    fixture: &CoverageFixture,
    classes: &[String],
    facets: &[(String, String)],
) -> Vec<CoverageResult> {
    let query = SelectionQuery {
        facets: facets.to_vec(),
        ..SelectionQuery::default()
    };
    select_sources_with(fixture, classes, 1, &query)
}

/// The (source, alternates) of the covered verdict for `class` and `facet`.
fn covered(verdicts: &[CoverageResult], class: &str, facet: Option<&str>) -> (String, Vec<String>) {
    verdicts
        .iter()
        .find_map(|v| match v {
            CoverageResult::Covered {
                class: c,
                facet: f,
                source,
                alternates,
                ..
            } if c == class && f.as_deref() == facet => Some((source.clone(), alternates.clone())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("a covered verdict for {class} {facet:?}: {verdicts:?}"))
}

fn names(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// A pack-origin read of the same seed only proposes: nothing is covered.
fn assert_pack_proposes_only(seed: &str, classes: &[String]) {
    let pack = SchemaSourceOrigin::Pack {
        connector: "seed-pack".to_string(),
        record_id: "pack-1".to_string(),
        tenant: None,
    };
    let fixture = fixture_from_schema_sources(&attached_under(seed, pack)).unwrap();
    assert!(select_sources(&fixture, classes, 1)
        .iter()
        .all(|v| !matches!(v, CoverageResult::Covered { .. })));
}

// spec: MEDULLA-CQ-XSRC-R002.5
#[test]
fn supplier_seed_passes_shape() {
    let fixture = seed_fixture(SUPPLIER_SEED, SchemaSourceOrigin::Operator);
    assert_eq!(fixture.rows().count(), 6);
    let classes: Vec<String> = [
        "Supplier",
        "SupplyChainItem",
        "RiskScenario",
        "PrivacyAssessment",
    ]
    .iter()
    .map(|name| format!("{CORE}{name}"))
    .collect();
    let got = verdicts(&fixture, &classes, &[]);
    let supplier = format!("{CORE}Supplier");
    let item = format!("{CORE}SupplyChainItem");
    let risk = format!("{CORE}RiskScenario");
    let privacy = format!("{CORE}PrivacyAssessment");
    // ERPNext ranks first for suppliers and items, LeanIX second.
    assert_eq!(
        covered(&got, &supplier, None),
        ("erpnext-mcp".into(), names(&["leanix-mcp"]))
    );
    assert_eq!(
        covered(&got, &item, None),
        ("erpnext-mcp".into(), names(&["leanix-mcp"]))
    );
    assert_eq!(
        covered(&got, &risk, None),
        ("ciso-assistant-mcp".into(), Vec::new())
    );
    assert_eq!(
        covered(&got, &privacy, None),
        ("onetrust-mcp".into(), Vec::new())
    );
    assert_pack_proposes_only(SUPPLIER_SEED, &classes);
}

// spec: MEDULLA-CQ-XSRC-R002.6
#[test]
fn component_seed_passes_shape() {
    let fixture = seed_fixture(COMPONENT_SEED, SchemaSourceOrigin::Operator);
    assert_eq!(fixture.rows().count(), 5);
    let image = format!("{INFRA}ContainerImage");
    let vulnerability = format!("{CORE}Vulnerability");
    let built_from = format!("{INFRA}builtFrom");
    let classes = vec![image.clone(), vulnerability.clone()];
    let facets = vec![(image.clone(), built_from.clone())];
    let got = verdicts(&fixture, &classes, &facets);
    // The registry says what an image is; the code host says what is wrong with it.
    assert_eq!(
        covered(&got, &image, None),
        ("dockerhub-mcp".into(), Vec::new())
    );
    assert_eq!(
        covered(&got, &vulnerability, None),
        ("gitlab-mcp".into(), names(&["github-mcp"]))
    );
    // The source repository an image is built from: the code host, GitHub second.
    assert_eq!(
        covered(&got, &image, Some(&built_from)),
        ("gitlab-mcp".into(), names(&["github-mcp"]))
    );
    assert_pack_proposes_only(COMPONENT_SEED, &classes);
}

// spec: MEDULLA-CQ-XSRC-R003.1
#[test]
fn service_seed_passes_shape() {
    let fixture = seed_fixture(SERVICE_SEED, SchemaSourceOrigin::Operator);
    assert_eq!(fixture.rows().count(), 3);
    let service = format!("{INFRA}Service");
    let health = format!("{INFRA}serviceStatus");
    let facets = vec![
        (service.clone(), health.clone()),
        (service.clone(), RDFS_LABEL.to_string()),
    ];
    let got = verdicts(&fixture, std::slice::from_ref(&service), &facets);
    // Health: uptime first, lgtm second. Identity: the CMDB.
    assert_eq!(
        covered(&got, &service, Some(&health)),
        ("uptime-mcp".into(), names(&["lgtm-mcp"]))
    );
    assert_eq!(
        covered(&got, &service, Some(RDFS_LABEL)),
        ("servicenow-mcp".into(), Vec::new())
    );
    assert_pack_proposes_only(SERVICE_SEED, &[service]);
}

// spec: MEDULLA-CQ-XSRC-R003.2
#[test]
fn workload_seed_passes_shape() {
    let fixture = seed_fixture(WORKLOAD_SEED, SchemaSourceOrigin::Operator);
    assert_eq!(fixture.rows().count(), 5);
    let image = format!("{INFRA}ContainerImage");
    let host = format!("{INFRA}Host");
    let temperature = format!("{INFRA}temperatureCelsius");
    let classes = vec![image.clone(), host.clone()];
    let facets = vec![(host.clone(), temperature.clone())];
    let got = verdicts(&fixture, &classes, &facets);
    let first = (
        "container-manager-mcp".to_string(),
        names(&["portainer-mcp"]),
    );
    assert_eq!(covered(&got, &image, None), first);
    assert_eq!(covered(&got, &host, None), first);
    // Hardware facts come from systems-manager as a facet row.
    assert_eq!(
        covered(&got, &host, Some(&temperature)),
        ("systems-manager-mcp".into(), Vec::new())
    );
    assert_pack_proposes_only(WORKLOAD_SEED, &classes);
}

// spec: MEDULLA-CQ-XSRC-R003.3
#[test]
fn capability_seed_passes_shape() {
    let fixture = seed_fixture(CAPABILITY_SEED, SchemaSourceOrigin::Operator);
    assert_eq!(fixture.rows().count(), 2);
    let capability = format!("{CORE}BusinessCapability");
    let classes = vec![capability.clone()];
    let got = verdicts(&fixture, &classes, &[]);
    // LeanIX is the maintained catalogue, ArchiMate the modelled view.
    assert_eq!(
        covered(&got, &capability, None),
        ("leanix-mcp".into(), names(&["archimate-mcp"]))
    );
    assert_pack_proposes_only(CAPABILITY_SEED, &classes);
}

const DATA_LOCATIONS: &str = r#"
@prefix sc: <https://knuckles-team.github.io/ontology/surface-contract#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix ex: <urn:eg:fixture:locations:> .

ex:orders-east a sc:DataLocation ;
    sc:instance "east" ;
    sc:store "orders-db" ;
    sc:holdsClass <https://knuckles-team.github.io/ontology/core#Order> ,
        <https://knuckles-team.github.io/ontology/core#Customer> ;
    sc:readTool "list_orders" ;
    sc:routeParam "region" ;
    sc:federationKey "order_id" .

ex:orders-nowhere a sc:DataLocation ;
    sc:holdsClass <https://knuckles-team.github.io/ontology/core#Order> .
"#;

// spec: MEDULLA-SRC-AUTH-R013
#[test]
fn data_location_yields_a_proposed_pack_row() {
    let pack = SchemaSourceOrigin::Pack {
        connector: "orders-pack".to_string(),
        record_id: "pack-1".to_string(),
        tenant: None,
    };
    let fixture = fixture_from_schema_sources(&attached_under(DATA_LOCATIONS, pack)).unwrap();
    let rows: Vec<&AuthorityRow> = fixture.rows().collect();
    // One row per held class of the complete location; the incomplete one yields none.
    assert_eq!(rows.len(), 2, "{rows:?}");
    for row in &rows {
        assert_eq!(row.origin, AuthorityOrigin::Pack.as_str());
        assert_eq!(row.source, "orders-pack:east:orders-db");
        assert_eq!(row.contract.as_deref(), Some("live_truth"));
        assert!(!row.approved, "a pack row stays a proposal");
        let route = row.route.as_ref().expect("the route is kept");
        assert_eq!(route.read_tool.as_deref(), Some("list_orders"));
        assert_eq!(route.route_param.as_deref(), Some("region"));
        assert_eq!(route.federation_key.as_deref(), Some("order_id"));
    }
    let classes: std::collections::BTreeSet<&str> =
        rows.iter().map(|row| row.class.as_str()).collect();
    assert_eq!(
        classes,
        [
            "https://knuckles-team.github.io/ontology/core#Customer",
            "https://knuckles-team.github.io/ontology/core#Order",
        ]
        .into_iter()
        .collect()
    );
    // The location with no instance is audited by name.
    let audit = fixture.audit_entries();
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert!(
        audit[0].starts_with("DATA_LOCATION_INCOMPLETE"),
        "{audit:?}"
    );
    assert!(audit[0].contains("orders-nowhere"), "{audit:?}");
}
