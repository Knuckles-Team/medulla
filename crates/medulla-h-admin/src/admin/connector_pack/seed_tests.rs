use std::collections::{BTreeMap, BTreeSet};

use medulla_catalog_types::{
    connector_pack::SeedGraphImport, connector_pack::SeedPart, connector_pack::SeedPartKey,
};
use medulla_graph_core::graph::{
    GraphSchemaSource, GraphSchemaSources, SchemaSourceOrigin, SchemaSourceRole,
};
use medulla_types::contract::{Digest256, ResourceId};

use super::seed::{
    canonical_digest, prepare_seed_import, SeedImportOutcome, MAX_SEED_BYTES, MAX_SEED_TRIPLES,
};

const SEED_NS: &str = "https://knuckles-team.github.io/ontology/connector-seed#";

fn digest(byte: u8) -> Digest256 {
    Digest256::from_bytes([byte; 32])
}

fn shapes() -> medulla_rdf::oxrdf::Graph {
    medulla_shacl::graph_from_turtle(&format!(
        "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
         @prefix seed: <{SEED_NS}> .\n\
         seed:SeedSubjectShape a sh:NodeShape ;\n\
             sh:targetClass seed:SeedSubject ;\n\
             sh:property [ sh:path seed:fromSeed ; sh:minCount 1 ; sh:maxCount 1 ; sh:nodeKind sh:IRI ] ."
    ))
    .expect("seed test shapes parse")
}

fn part(key: SeedPartKey, turtle: &str) -> SeedPart {
    SeedPart {
        key,
        turtle: turtle.to_string(),
        digest: canonical_digest(turtle).expect("canonical seed digest"),
    }
}

fn seed(catalog_name: &str, content_digest: Digest256) -> SeedGraphImport {
    let mut parts = BTreeMap::new();
    parts.insert(
        SeedPartKey::Identity,
        part(
            SeedPartKey::Identity,
            &format!(
                "<urn:seed:instance> a <{SEED_NS}SeedSubject> ; <{SEED_NS}fromSeed> <urn:seed:r1> ."
            ),
        ),
    );
    parts.insert(
        SeedPartKey::Catalog,
        part(
            SeedPartKey::Catalog,
            &format!("<urn:seed:{catalog_name}> a <{SEED_NS}SeedSubject> ; <{SEED_NS}fromSeed> <urn:seed:r1> ."),
        ),
    );
    parts.insert(
        SeedPartKey::References,
        part(
            SeedPartKey::References,
            "<urn:seed:item> <urn:seed:references> <urn:source:item> .",
        ),
    );
    SeedGraphImport {
        connector: ResourceId::new("demo").unwrap(),
        instance_iri: "urn:connector:demo:instance".to_string(),
        pack_digest: digest(1),
        parts,
        content_digest,
    }
}

// spec: MEDULLA-TYPED-PACKS-R104.2
/// MEDULLA-TYPED-PACKS-R104.2: a changed catalog part replaces only that part;
/// identity and references remain byte-identical in the one prepared state.
#[test]
fn changed_part_reimports_alone() {
    let shapes = shapes();
    let sources = GraphSchemaSources::default();
    let first = seed("old", digest(2));
    let initial =
        prepare_seed_import(&first, None, &shapes, &sources).expect("initial seed import");
    assert_eq!(initial.outcome, SeedImportOutcome::Inserted);
    assert_eq!(initial.content.parts.len(), 3);
    assert!(initial
        .content
        .parts
        .values()
        .all(|part| part.triple_count <= MAX_SEED_TRIPLES));

    let second = seed("new", digest(3));
    let changed = prepare_seed_import(&second, Some(&initial.content), &shapes, &sources)
        .expect("partial seed replacement");
    assert_eq!(
        changed.outcome,
        SeedImportOutcome::Replaced {
            changed_parts: BTreeSet::from([SeedPartKey::Catalog])
        }
    );
    assert_eq!(
        changed.content.parts[&SeedPartKey::Identity],
        initial.content.parts[&SeedPartKey::Identity]
    );
    assert_eq!(
        changed.content.parts[&SeedPartKey::References],
        initial.content.parts[&SeedPartKey::References]
    );
    assert_ne!(
        changed.content.parts[&SeedPartKey::Catalog],
        initial.content.parts[&SeedPartKey::Catalog]
    );
}

#[test]
fn equal_content_digest_is_a_noop_after_the_existing_state_is_known() {
    let shapes = shapes();
    let sources = GraphSchemaSources::default();
    let first = seed("same", digest(4));
    let initial =
        prepare_seed_import(&first, None, &shapes, &sources).expect("initial seed import");
    let unchanged = prepare_seed_import(&first, Some(&initial.content), &shapes, &sources)
        .expect("equal digest should be unchanged");
    assert_eq!(unchanged.outcome, SeedImportOutcome::Unchanged);
    assert_eq!(unchanged.content, initial.content);
}

#[test]
fn canonical_digest_ignores_statement_order() {
    let first = "<urn:s> <urn:p> <urn:o> .\n<urn:s> <urn:q> \"value\" .";
    let second = "<urn:s> <urn:q> \"value\" .\n<urn:s> <urn:p> <urn:o> .";
    assert_eq!(canonical_digest(first), canonical_digest(second));
}

/// A pack source whose ontology declares `urn:seed:size` as a property.
fn pack_sources(role: SchemaSourceRole) -> GraphSchemaSources {
    let ontology = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
                    <urn:seed:size> a owl:DatatypeProperty .";
    let source = GraphSchemaSource::new(
        SchemaSourceOrigin::Pack {
            connector: "demo".to_string(),
            record_id: "record-1".to_string(),
            tenant: None,
        },
        None,
        Some(ontology.into()),
        0,
    )
    .expect("pack source")
    .with_role(role);
    GraphSchemaSources::with_dynamic(BTreeMap::from([("pack:demo".to_string(), source)]))
        .expect("composed sources")
}

fn refusal_code(error: &str) -> &str {
    error.split_once(':').map_or(error, |(code, _)| code)
}

/// MEDULLA-TYPED-PACKS-R104.4: each refusal returns its registered code as the
/// first token and yields no plan, so the prior seed state is untouched.
// spec: MEDULLA-TYPED-PACKS-R104.4
#[test]
fn seed_refusals_write_nothing() {
    let shapes = shapes();
    let sources = GraphSchemaSources::default();
    let initial = prepare_seed_import(&seed("old", digest(2)), None, &shapes, &sources)
        .expect("initial seed import");
    let before = initial.content.clone();

    let mut oversize = seed("old", digest(3));
    oversize
        .parts
        .get_mut(&SeedPartKey::References)
        .unwrap()
        .turtle = format!("# {}\n", "x".repeat(MAX_SEED_BYTES));
    let error = prepare_seed_import(&oversize, Some(&before), &shapes, &sources).unwrap_err();
    assert_eq!(refusal_code(&error), "SEED_TOO_LARGE");

    let mut violating = seed("old", digest(3));
    let bad = format!("<urn:seed:bad> a <{SEED_NS}SeedSubject> .");
    violating
        .parts
        .insert(SeedPartKey::Catalog, part(SeedPartKey::Catalog, &bad));
    let error = prepare_seed_import(&violating, Some(&before), &shapes, &sources).unwrap_err();
    assert_eq!(refusal_code(&error), "SEED_SHACL_VIOLATION");
    assert!(
        error.contains("urn:seed:bad"),
        "names the focus node: {error}"
    );

    let mut mismatched = seed("old", digest(3));
    mismatched
        .parts
        .get_mut(&SeedPartKey::Catalog)
        .unwrap()
        .digest = digest(9);
    let error = prepare_seed_import(&mismatched, Some(&before), &shapes, &sources).unwrap_err();
    assert_eq!(refusal_code(&error), "SEED_DIGEST_MISMATCH");

    let mut bulk = seed("old", digest(3));
    let sized = "<urn:seed:item> <urn:seed:size> \"10\" .";
    bulk.parts.insert(
        SeedPartKey::References,
        part(SeedPartKey::References, sized),
    );
    let domain = pack_sources(SchemaSourceRole::Domain);
    let error = prepare_seed_import(&bulk, Some(&before), &shapes, &domain).unwrap_err();
    assert_eq!(refusal_code(&error), "SEED_BULK_DATA");
    let catalog = pack_sources(SchemaSourceRole::Catalog);
    prepare_seed_import(&bulk, Some(&before), &shapes, &catalog)
        .expect("a catalog-role predicate is seed data");

    // The refused imports left the prior state exactly as it was.
    assert_eq!(initial.content, before);
}

// spec: MEDULLA-TYPED-PACKS-R810.5
#[test]
fn no_writer_sets_content_outside_construction() {
    // A seed plan's content is built once by `prepare_seed_import`: nothing
    // assigns the field afterwards. The needles are built in halves so this
    // file does not match its own scan.
    let assign = [".con", "tent="].concat();
    let compare = [".con", "tent=="].concat();
    for (name, source) in [
        ("seed.rs", include_str!("seed.rs")),
        ("seed_tests.rs", include_str!("seed_tests.rs")),
    ] {
        for (number, line) in source.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            let compact: String = code.split_whitespace().collect();
            assert!(
                !compact.contains(&assign) || compact.contains(&compare),
                "{name}:{}: seed plan content assigned after construction",
                number + 1
            );
        }
    }
}

/// The served import of seed part entries (MEDULLA-TYPED-PACKS-R104.3).
#[cfg(all(feature = "redb", feature = "blob"))]
mod served {
    use std::sync::atomic::AtomicBool;

    use medulla_catalog_types::connector_pack::{PackAnnotations, PackEntryKind, PackImportResult};
    use medulla_protocol_types::protocol::Method;
    use medulla_types::contract::ResourceId;

    use crate::admin::connector_pack::tests::{
        bind, build_pack, head_of, import, imported, ok, Content, Served, ADMIN, TENANT,
    };
    use crate::connector_pack_projection::worker::sweep;

    const CONNECTOR: &str = "seed-served";

    fn seed_part(key: &str, turtle: &str) -> Content {
        Content {
            kind: PackEntryKind::SeedPart,
            uri: format!("seed://{key}"),
            name: key.to_string(),
            media_type: "text/turtle",
            body: turtle.as_bytes().to_vec(),
            input_schema: None,
            annotations: PackAnnotations::default(),
            references: Vec::new(),
        }
    }

    fn parts() -> Vec<Content> {
        vec![
            seed_part(
                "identity",
                "<urn:seed:instance> a <urn:seed:Identity> .\n",
            ),
            seed_part("catalog", "<urn:seed:item> a <urn:seed:CatalogEntry> .\n"),
            seed_part(
                "references",
                "<urn:seed:item> <urn:seed:references> <urn:source:item> .\n",
            ),
        ]
    }

    async fn pack_graph_rdf(served: &Served, key: &str) -> String {
        let graph = medulla_catalog_types::connector_pack::ids::pack_projection_graph_name(
            TENANT,
            &ResourceId::new(CONNECTOR).unwrap(),
        )
        .unwrap();
        ok::<String>("GetRdf", served.call_on(&graph, key, Method::GetRdf).await)
    }

    /// The pack graph read through a served `Method::Sparql`, one `<s> <p> "<o>"` line per
    /// solution (the object is quoted whatever its kind, so a line is matched by substring).
    async fn pack_graph_sparql(served: &Served, key: &str) -> String {
        let graph = medulla_catalog_types::connector_pack::ids::pack_projection_graph_name(
            TENANT,
            &ResourceId::new(CONNECTOR).unwrap(),
        )
        .unwrap();
        let result: medulla_protocol_types::protocol::SparqlResult = ok(
            "Sparql",
            served
                .call_on(
                    &graph,
                    key,
                    Method::Sparql {
                        query: "SELECT ?s ?p ?o WHERE { ?s ?p ?o }".to_string(),
                        base_iri: String::new(),
                        type_convention: String::new(),
                        explain: false,
                        requires_packs: Vec::new(),
                        latency_class: Default::default(),
                        graph_media_type: None,
                    },
                )
                .await,
        );
        result
            .rows
            .iter()
            .map(|row| {
                let cell = |index: usize| row[index].clone().unwrap_or_default();
                format!("<{}> <{}> \"{}\"", cell(0), cell(1), cell(2))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    // spec: MEDULLA-TYPED-PACKS-R104.3
    #[tokio::test]
    async fn served_import_projects_seed() {
        let served = Served::new();
        let _env = served.with_graph_persistence().await;
        bind(&served, CONNECTOR, ADMIN).await;
        let pack = build_pack(CONNECTOR, &parts());
        let receipt = imported(&served, &pack, None).await;
        let subscribed = AtomicBool::new(false);
        while sweep(&served.state, &subscribed).await.unwrap() {}

        // The pack graph holds the seed triples, the revision node and one
        // `seed:fromSeed` link per seed subject.
        let projected = pack_graph_rdf(&served, "seed:first").await;
        for subject in ["urn:seed:instance", "urn:seed:item"] {
            assert!(projected.contains(subject), "{subject} is projected");
        }
        assert!(projected.contains("urn:source:item"), "{projected}");
        assert!(
            projected.contains("urn:medulla:seed:seed-served:"),
            "the revision node is projected: {projected}"
        );
        assert_eq!(
            projected.matches("connector-seed#fromSeed").count(),
            2,
            "one fromSeed link per seed subject: {projected}"
        );

        // A repeat with equal entries leaves the head and the graph unchanged.
        let repeated: PackImportResult = ok(
            "repeat import",
            import(&served, &pack, Some(head_of(&receipt))).await,
        );
        assert!(matches!(repeated, PackImportResult::Unchanged { .. }));
        while sweep(&served.state, &subscribed).await.unwrap() {}
        assert_eq!(pack_graph_rdf(&served, "seed:second").await, projected);
    }

    // spec: MEDULLA-TYPED-PACKS-R104.5
    #[tokio::test]
    async fn receipt_names_part_outcomes() {
        use medulla_catalog_types::connector_pack::{SeedPartKey, SeedPartOutcome};

        let served = Served::new();
        let _env = served.with_graph_persistence().await;
        bind(&served, CONNECTOR, ADMIN).await;
        let first = imported(&served, &build_pack(CONNECTOR, &parts()), None).await;
        let seed = first.seed.clone().expect("a pack with seed parts carries a receipt");
        assert_eq!(seed.connector.as_str(), CONNECTOR);
        assert!(seed
            .outcomes
            .values()
            .all(|outcome| *outcome == SeedPartOutcome::Inserted));
        assert_eq!(seed.outcomes.len(), 3);

        // Only the changed part is `replaced`; the others are `unchanged`.
        let mut changed = parts();
        changed[1] = seed_part("catalog", "<urn:seed:item2> a <urn:seed:CatalogEntry> .\n");
        let second = imported(
            &served,
            &build_pack(CONNECTOR, &changed),
            Some(head_of(&first)),
        )
        .await;
        let reimport = second.seed.clone().expect("the reimport carries a receipt");
        assert_eq!(reimport.outcomes[&SeedPartKey::Catalog], SeedPartOutcome::Replaced);
        assert_eq!(reimport.outcomes[&SeedPartKey::Identity], SeedPartOutcome::Unchanged);
        assert_eq!(reimport.outcomes[&SeedPartKey::References], SeedPartOutcome::Unchanged);
        assert_ne!(reimport.content_digest, seed.content_digest);
        assert_ne!(reimport.graph_revision, seed.graph_revision);

        // The graph holds the receipt's revision node, and each seed subject links to it.
        let subscribed = AtomicBool::new(false);
        while sweep(&served.state, &subscribed).await.unwrap() {}
        let projected = pack_graph_sparql(&served, "seed:receipt").await;
        assert!(projected.contains(&reimport.graph_revision), "{projected}");
        assert!(!projected.contains(&seed.graph_revision), "{projected}");
        assert_eq!(
            projected
                .lines()
                .filter(|line| line.contains("connector-seed#fromSeed")
                    && line.contains(&reimport.graph_revision))
                .count(),
            3,
            "one fromSeed link per seed subject: {projected}"
        );

        // The receipt facts are in the graph: one node per part with the part, the outcome
        // and the revision node of the last import.
        for (part, outcome) in [
            ("identity", "unchanged"),
            ("catalog", "replaced"),
            ("references", "unchanged"),
        ] {
            let node = format!(
                "urn:medulla:seed-receipt:{CONNECTOR}:{}:{part}",
                reimport.content_digest.to_hex()
            );
            let lines: Vec<&str> = projected.lines().filter(|line| line.contains(&node)).collect();
            assert!(
                lines.iter().any(|line| line.contains("connector-seed#outcome")
                    && line.contains(&format!("\"{outcome}\""))),
                "{part} is {outcome}: {projected}"
            );
            assert!(
                lines.iter().any(|line| line.contains("connector-seed#graphRevision")
                    && line.contains(&reimport.graph_revision)),
                "{part} names the revision node: {projected}"
            );
            assert!(
                lines.iter().any(|line| line.contains("connector-seed#digest")
                    && line.contains("sha256:")),
                "{part} has a digest: {projected}"
            );
        }
        assert!(
            !projected.contains(&format!(
                "urn:medulla:seed-receipt:{CONNECTOR}:{}",
                seed.content_digest.to_hex()
            )),
            "an older receipt never lingers: {projected}"
        );
    }
}
