//! A skill package renders from graph content on every served catalog read
//! (MEDULLA-TYPED-PACKS-R100.3, R100.4, R100.5).
//!
//! The proof drives the real signed dispatch end to end: a connector pack is
//! imported through `ConnectorPack.Import`, the connector is bound by a
//! discovery observation through `FleetCatalog.record_discovery`, and every
//! assertion reads the served `FleetCatalog.list` page. The render sits in
//! `medulla-server-consensus`, which `medulla-capabilities` cannot reach, so
//! the L3 proofs live here beside the served pack harness.

use medulla_catalog_types::connector_pack::{PackAnnotations, PackEntryKind};
use medulla_catalog_types::fleet_catalog::{
    DiscoveryCounts, DiscoveryOutcome, DiscoveryScope, FleetCatalogKind, FleetCatalogListRequest,
    FleetCatalogOp, FleetCatalogPage, FleetCatalogRow, FleetDiscoveryRecordRequest, FleetSkillRow,
    FleetWriteReceipt,
};
use medulla_protocol_types::protocol::Method;
use medulla_types::contract::{BoundedVec, ResourceId};

use super::{bind, build_pack, head_of, imported, ok, skill, Content, Served, ADMIN};

const CONNECTOR: &str = "skill-render";
const OWL: &str = "http://www.w3.org/2002/07/owl#";

fn ontology_id() -> String {
    format!("mcp:{CONNECTOR}/ontology/core.ttl")
}

fn ontology() -> Content {
    let body = format!(
        "<https://example.org/A> a <{OWL}Class> ; \
         <http://www.w3.org/2000/01/rdf-schema#subClassOf> \
         <https://knuckles-team.github.io/ontology/core#DomainEntity> .\n"
    );
    Content {
        kind: PackEntryKind::Ontology,
        uri: format!("ontology://{CONNECTOR}/core.ttl"),
        name: "core.ttl".to_string(),
        media_type: "text/turtle",
        body: body.into_bytes(),
        input_schema: None,
        annotations: PackAnnotations::default(),
        references: Vec::new(),
    }
}

/// A skill whose `SKILL.md` declares `refs` and says `text`.
fn skill_text(name: &str, refs: &[&str], text: &str) -> Content {
    let mut content = skill(CONNECTOR, name, &[]);
    content.body = format!(
        "---\nname: {name}\ndescription: A test skill.\nontology_refs: {}\n---\n# {name}\n{text}\n",
        refs.join(" ")
    )
    .into_bytes();
    content
}

async fn observe(served: &Served) {
    let _: FleetWriteReceipt = ok(
        "record_discovery",
        served
            .call(
                "skill-render:discovery",
                Method::FleetCatalog {
                    op: Box::new(FleetCatalogOp::RecordDiscovery {
                        request: FleetDiscoveryRecordRequest {
                            server_name: "skill-render-server".to_string(),
                            scope: DiscoveryScope::TenantLocal,
                            connector: ResourceId::new(CONNECTOR).unwrap(),
                            outcome: DiscoveryOutcome::Reachable,
                            counts: DiscoveryCounts::default(),
                            expected_revision: None,
                        },
                    }),
                },
            )
            .await,
    );
}

/// The served skill rows.
async fn list_skills(served: &Served, key: &str) -> Vec<FleetSkillRow> {
    let page: FleetCatalogPage = ok(
        "list skills",
        served
            .call(
                key,
                Method::FleetCatalog {
                    op: Box::new(FleetCatalogOp::List {
                        request: FleetCatalogListRequest {
                            kind: FleetCatalogKind::Skills,
                            query: None,
                            grant_digests: BoundedVec::new(Vec::new()).unwrap(),
                            limit: None,
                            cursor: None,
                        },
                    }),
                },
            )
            .await,
    );
    page.rows
        .iter()
        .filter_map(|row| match row {
            FleetCatalogRow::Skill { row } => Some(row.clone()),
            _ => None,
        })
        .collect()
}

fn the_skill<'a>(rows: &'a [FleetSkillRow], name: &str) -> &'a FleetSkillRow {
    rows.iter()
        .find(|row| row.component.name == name)
        .unwrap_or_else(|| panic!("skill {name} is listed"))
}

// spec: MEDULLA-TYPED-PACKS-R100.3
#[tokio::test]
async fn catalog_listing_renders_current_content() {
    let served = Served::new();
    bind(&served, CONNECTOR, ADMIN).await;
    let ontology_ref = ontology_id();
    let refs = [ontology_ref.as_str()];
    let first = imported(
        &served,
        &build_pack(
            CONNECTOR,
            &[ontology(), skill_text("triage", &refs, "step one")],
        ),
        None,
    )
    .await;
    observe(&served).await;

    let listed = list_skills(&served, "skill-render:list:1").await;
    let before = the_skill(&listed, "triage");
    let package = before
        .package
        .as_ref()
        .expect("the listing renders a package");
    assert_eq!(before.package_refusal, None);
    assert_eq!(package.ontology_refs, vec![ontology_id()]);
    assert_eq!(package.digest.len(), 64);

    // Re-ingest changed content: the next listing renders the NEW content, so
    // nothing was cached between reads.
    imported(
        &served,
        &build_pack(
            CONNECTOR,
            &[ontology(), skill_text("triage", &refs, "step one, revised")],
        ),
        Some(head_of(&first)),
    )
    .await;
    let relisted = list_skills(&served, "skill-render:list:2").await;
    let after = the_skill(&relisted, "triage")
        .package
        .as_ref()
        .expect("the re-listing renders a package");
    assert_ne!(
        after.digest, package.digest,
        "the digest follows the content"
    );
}

// spec: MEDULLA-TYPED-PACKS-R100.4
#[tokio::test]
async fn dangling_reference_is_refused() {
    let served = Served::new();
    bind(&served, CONNECTOR, ADMIN).await;
    let missing = format!("mcp:{CONNECTOR}/ontology/absent.ttl");
    let ontology_ref = ontology_id();
    imported(
        &served,
        &build_pack(
            CONNECTOR,
            &[
                ontology(),
                skill_text("dangling", &[missing.as_str()], "refers to nothing"),
                skill_text("sound", &[ontology_ref.as_str()], "refers to the ontology"),
            ],
        ),
        None,
    )
    .await;
    observe(&served).await;

    let listed = list_skills(&served, "skill-render:list:dangling").await;
    // The listing never fails as a whole: the dangling skill stays listed
    // without a package and names the missing term; its sibling still renders.
    let dangling = the_skill(&listed, "dangling");
    assert!(dangling.package.is_none());
    let refusal = dangling.package_refusal.as_deref().expect("a refusal");
    assert!(
        refusal.starts_with("RENDER_DANGLING_ONTOLOGY_REF"),
        "{refusal}"
    );
    assert!(refusal.contains(&missing), "{refusal}");
    assert!(the_skill(&listed, "sound").package.is_some());
}

// spec: MEDULLA-TYPED-PACKS-R100.5
#[tokio::test]
async fn render_carries_revision_and_digest() {
    let served = Served::new();
    bind(&served, CONNECTOR, ADMIN).await;
    let ontology_ref = ontology_id();
    let refs = [ontology_ref.as_str()];
    let first = imported(
        &served,
        &build_pack(CONNECTOR, &[ontology(), skill_text("triage", &refs, "v1")]),
        None,
    )
    .await;
    observe(&served).await;

    let one = list_skills(&served, "skill-render:evidence:1").await;
    let two = list_skills(&served, "skill-render:evidence:2").await;
    let a = the_skill(&one, "triage").package.clone().expect("package");
    let b = the_skill(&two, "triage").package.clone().expect("package");
    assert_eq!(
        a, b,
        "two reads of unchanged content compare equal by both fields"
    );
    assert!(a.graph_revision > 0);

    imported(
        &served,
        &build_pack(CONNECTOR, &[ontology(), skill_text("triage", &refs, "v2")]),
        Some(head_of(&first)),
    )
    .await;
    let three = list_skills(&served, "skill-render:evidence:3").await;
    let c = the_skill(&three, "triage")
        .package
        .clone()
        .expect("package");
    assert_ne!(c.digest, a.digest);
    assert!(
        c.graph_revision > a.graph_revision,
        "the revision advances with the ingest"
    );
}
