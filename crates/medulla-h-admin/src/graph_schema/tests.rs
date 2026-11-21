use super::*;
use medulla_catalog_types::connector_pack::ids::pack_projection_graph_name;
use medulla_catalog_types::graph_schema::GraphSchemaOp;

/// The verified tenant every pack attach in these tests is recorded under.
const PACK_TENANT: &str = "tenant:test";

fn pack_document(uri: &str, body: Arc<str>) -> PackSchemaDocument {
    PackSchemaDocument {
        uri: uri.to_string(),
        body,
    }
}

#[test]
fn pack_projection_graph_identity_is_framed_opaque_and_tenant_safe() {
    let graph_os = medulla_types::contract::ResourceId::new("graph-os").unwrap();
    let utilities = medulla_types::contract::ResourceId::new("meristem").unwrap();
    let golden = pack_projection_graph_name("tenant-a", &graph_os).unwrap();
    assert_eq!(
        golden,
        "pack__710a40e26f5757a7d0ea74593af842cd8444bd62bc8f31e76d2d37f5d5611947"
    );
    assert_eq!(golden.len(), "pack__".len() + 64);
    assert!(!golden.contains("tenant-a") && !golden.contains("graph-os"));
    assert_ne!(
        golden,
        pack_projection_graph_name("tenant-b", &graph_os).unwrap()
    );
    assert_ne!(
        golden,
        pack_projection_graph_name("tenant-a", &utilities).unwrap()
    );
    assert!(pack_projection_graph_name("tenant-a ", &graph_os).is_err());
}

fn attach(source_id: &str, ontology_ttl: &str) -> GraphSchemaOp {
    GraphSchemaOp::Attach {
        source_id: source_id.to_string(),
        shapes_ttl: None,
        ontology_ttl: Some(ontology_ttl.to_string()),
        if_composed_digest: None,
    }
}

#[test]
fn attach_list_and_detach_share_one_authoritative_source_set() {
    let core = GraphCore::new();
    let initial = medulla_server_ops::api::admin::schema_read::list("g", &core).unwrap();
    let core_ids: std::collections::BTreeSet<_> = initial
        .core_sources
        .iter()
        .filter_map(|source| match &source.origin {
            medulla_catalog_types::graph_schema::SchemaSourceOriginView::Core { .. } => {
                Some(source.source_id.as_str())
            }
            _ => None,
        })
        .collect();
    // The listing reports the exact immutable catalog of 65 core sources (the state-machine vocabulary and its shapes included).
    // Twelve sources own both document kinds (the A2A vocabulary gained its team
    // shapes, and the identity metadata vocabulary ships with its shapes); every
    // other source owns exactly one document kind. `core:sdd@1` gained its shapes document with the
    // SDLC lifecycle kinds/edges/shapes model (ef517a72c), and the placement
    // target vocabulary joined as a both-kinds source with MEDULLA-PLACEMENT-R001.5.
    let catalog = core.schema_sources();
    let catalog_ids: std::collections::BTreeSet<_> =
        catalog.core.keys().map(String::as_str).collect();
    assert_eq!(core_ids, catalog_ids);
    assert_eq!(core_ids.len(), 65);
    for anchor in [
        "core:catalog@1",
        "core:foundation@1",
        "core:governance-shapes@1",
        "core:software@1",
    ] {
        assert!(core_ids.contains(anchor), "missing {anchor}");
    }
    assert_eq!(
        initial
            .core_sources
            .iter()
            .filter(|source| source.ontology_sha256.is_some())
            .count(),
        51
    );
    assert_eq!(
        initial
            .core_sources
            .iter()
            .filter(|source| source.shapes_sha256.is_some())
            .count(),
        26
    );
    for source in &initial.core_sources {
        let expected = if matches!(
            source.source_id.as_str(),
            "core:a2a@1"
                | "core:archimate@1"
                | "core:commercial@1"
                | "core:connector-pack@1"
                | "core:data-platform@1"
                | "core:decision@1"
                | "core:harness@1"
                | "core:identity_metadata@1"
                | "core:infrastructure@1"
                | "core:placement@1"
                | "core:sdd@1"
                | "core:sdlc_lifecycle@1"
        ) {
            (true, true)
        } else {
            assert!(source.shapes_sha256.is_some() ^ source.ontology_sha256.is_some());
            continue;
        };
        assert_eq!(
            (
                source.ontology_sha256.is_some(),
                source.shapes_sha256.is_some()
            ),
            expected
        );
    }
    let ontology = "@prefix owl: <http://www.w3.org/2002/07/owl#> . @prefix ex: <http://example/> . ex:Local a owl:Class .";
    apply(&core, "g", &attach("admin:local", ontology)).unwrap();
    let listed = medulla_server_ops::api::admin::schema_read::list("g", &core).unwrap();
    assert!(listed
        .dynamic_sources
        .iter()
        .any(|source| source.source_id == "admin:local"));
    assert!(listed
        .core_sources
        .iter()
        .any(|source| source.source_id == "core:foundation@1"));

    apply(
        &core,
        "g",
        &GraphSchemaOp::Detach {
            source_id: "admin:local".to_string(),
            if_composed_digest: None,
        },
    )
    .unwrap();
    assert!(!core.schema_sources().dynamic.contains_key("admin:local"));
    assert!(core.schema_sources().core.contains_key("core:foundation@1"));

    apply(&core, "g", &attach("admin:local", ontology)).unwrap();
    assert!(core.schema_sources().dynamic.contains_key("admin:local"));
}

/// EH-355 ruling (c): the full-ABox tableau runs where schema ENTERS. An attached
/// individual in two disjoint classes passes the terminology-scoped composition
/// that restore uses, and is refused at attach with nothing published.
#[test]
fn attach_refuses_an_abox_contradiction_the_terminology_scope_admits() {
    let core = GraphCore::new();
    let before = core.schema_sources();
    let ontology = "@prefix owl: <http://www.w3.org/2002/07/owl#> . \
        @prefix ex: <http://example/> . \
        ex:A a owl:Class ; owl:disjointWith ex:B . ex:B a owl:Class . \
        ex:x a ex:A, ex:B .";
    let mut candidate = (*before).clone();
    candidate
        .attach_dynamic(
            "admin:contradiction".to_string(),
            medulla_graph_core::graph::GraphSchemaSource::new(
                medulla_graph_core::graph::SchemaSourceOrigin::Admin {
                    name: "contradiction".to_string(),
                },
                None,
                Some(Arc::from(ontology)),
                0,
            )
            .unwrap(),
        )
        .unwrap();
    medulla_server_api::schema_compose::validate_and_compose(&candidate).unwrap();

    let error = apply(&core, "g", &attach("admin:contradiction", ontology)).unwrap_err();
    assert!(error.starts_with("ONTOLOGY_INCONSISTENT"), "{error}");
    assert_eq!(core.schema_sources(), before);
}

/// Attaching `turtle` as source `admin:<name>` is refused as inconsistent
/// and leaves the schema catalog exactly as it was.
fn assert_attach_is_inconsistent(name: &str, turtle: &str) {
    let core = GraphCore::new();
    let before = core.schema_sources();
    let source = format!("admin:{name}");
    let error = apply(&core, "g", &attach(&source, turtle)).unwrap_err();
    assert!(
        error.starts_with("ONTOLOGY_INCONSISTENT"),
        "{name}: {error}"
    );
    assert_eq!(core.schema_sources(), before);
}

/// EH-363: the entry check sees the core's n-ary disjointness (`AllDisjointClasses`)
/// and BFO's `Continuant ⊥ Occurrent`. Each individual below sits in two disjoint
/// core classes: GDC and IC (a taxon typed as an organism), Person and Event, and a
/// specifically dependent continuant that is also a temporal region.
#[test]
fn attach_refuses_an_individual_in_two_disjoint_core_classes() {
    let bfo = "@prefix bfo: <http://purl.obolibrary.org/obo/BFO_> . \
        @prefix kg: <https://knuckles-team.github.io/ontology/core#> . @prefix ex: <http://example/> . ";
    for (name, individual) in [
        ("taxon", "ex:vulpes a bfo:0000031, bfo:0000004 ."),
        ("agent", "ex:ada a kg:Person, kg:Event ."),
        ("bfo", "ex:moment a bfo:0000020, bfo:0000008 ."),
    ] {
        assert_attach_is_inconsistent(name, &format!("{bfo}{individual}"));
    }
}

/// EH-363: the entry check honours `rdfs:domain`/`rdfs:range`. `kg:memberOf` has
/// domain and range BFO:IndependentContinuant, and `kg:createdBy` range IC; a
/// process at either end contradicts Continuant ⊥ Occurrent (the shape of "a
/// process PARTICIPATED_IN an event" once participation is continuant-only).
#[test]
fn attach_refuses_a_process_at_a_continuant_only_end_of_an_edge() {
    let prefixes = "@prefix bfo: <http://purl.obolibrary.org/obo/BFO_> . \
        @prefix kg: <https://knuckles-team.github.io/ontology/core#> . @prefix ex: <http://example/> . ";
    for (name, data) in [
        (
            "domain",
            "ex:run a bfo:0000015 . ex:run kg:memberOf ex:team .",
        ),
        (
            "range",
            "ex:doc kg:createdBy ex:job . ex:job a bfo:0000015 .",
        ),
    ] {
        assert_attach_is_inconsistent(name, &format!("{prefixes}{data}"));
    }
    let core = GraphCore::new();
    let fine = format!("{prefixes}ex:ada a kg:Person . ex:ada kg:memberOf ex:team .");
    apply(&core, "g", &attach("admin:fine", &fine)).unwrap();
}

#[test]
fn invalid_attach_and_stale_cas_publish_nothing() {
    let core = GraphCore::new();
    let before = core.schema_sources();
    assert!(apply(&core, "g", &attach("admin:broken", "not turtle")).is_err());
    assert_eq!(core.schema_sources(), before);

    let fenced = GraphSchemaOp::Attach {
        source_id: "admin:fenced".to_string(),
        shapes_ttl: None,
        ontology_ttl: Some("@prefix owl: <http://www.w3.org/2002/07/owl#> .".to_string()),
        if_composed_digest: Some("0".repeat(64)),
    };
    assert!(apply(&core, "g", &fenced)
        .unwrap_err()
        .contains("COMPOSED_DIGEST_MISMATCH"));
    assert_eq!(core.schema_sources(), before);
}

#[test]
fn generic_graph_schema_cannot_detach_or_replace_core() {
    let core = GraphCore::new();
    let before = core.schema_sources();
    for op in [
        attach(
            "core:foundation@1",
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .",
        ),
        GraphSchemaOp::Detach {
            source_id: "core:foundation@1".to_string(),
            if_composed_digest: None,
        },
    ] {
        assert!(apply(&core, "g", &op)
            .unwrap_err()
            .contains("SCHEMA_SOURCE_RESERVED"));
    }
    assert_eq!(core.schema_sources(), before);
}

#[test]
fn internal_pack_projection_owns_identity_and_rejects_regression() {
    let core = GraphCore::new();
    let connector = medulla_types::contract::ResourceId::new("graph-os").unwrap();
    let ontology_a = Arc::<str>::from(
        "@prefix owl: <http://www.w3.org/2002/07/owl#> . @prefix ex: <http://example/> . ex:A a owl:Class .",
    );
    let ontology_b = Arc::<str>::from(
        "@prefix owl: <http://www.w3.org/2002/07/owl#> . @prefix ex: <http://example/> . ex:B a owl:Class .",
    );

    let first = apply_pack_source(
        &core,
        "pack__graph-os",
        PACK_TENANT,
        &connector,
        "pack:graph-os:1:aaaa",
        PackSourceDocuments {
            external_schemas: Vec::new(),
            shapes: Vec::new(),
            ontologies: vec![pack_document(
                "ontology://graph-os/a.ttl",
                Arc::clone(&ontology_a),
            )],
            crosswalks: Vec::new(),
        },
        None,
    )
    .unwrap();
    assert!(first.changed);
    let installed = core
        .schema_sources()
        .dynamic
        .get("pack:graph-os")
        .cloned()
        .unwrap();
    assert_eq!(
        installed.origin,
        medulla_graph_core::graph::SchemaSourceOrigin::Pack {
            connector: "graph-os".to_string(),
            record_id: "pack:graph-os:1:aaaa".to_string(),
            tenant: Some(PACK_TENANT.to_string()),
        }
    );

    let identical = apply_pack_source(
        &core,
        "pack__graph-os",
        PACK_TENANT,
        &connector,
        "pack:graph-os:1:aaaa",
        PackSourceDocuments {
            external_schemas: Vec::new(),
            shapes: Vec::new(),
            ontologies: vec![pack_document(
                "ontology://graph-os/a.ttl",
                Arc::clone(&ontology_a),
            )],
            crosswalks: Vec::new(),
        },
        Some(&first.composed_digest),
    )
    .unwrap();
    assert!(!identical.changed);
    assert_eq!(identical.composed_digest, first.composed_digest);

    let before = core.schema_sources();
    let error = apply_pack_source(
        &core,
        "pack__graph-os",
        PACK_TENANT,
        &connector,
        "pack:graph-os:0:bbbb",
        PackSourceDocuments {
            external_schemas: Vec::new(),
            shapes: Vec::new(),
            ontologies: vec![pack_document("ontology://graph-os/b.ttl", ontology_b)],
            crosswalks: Vec::new(),
        },
        None,
    )
    .unwrap_err();
    assert!(error.contains("SCHEMA_SOURCE_REGRESSION"));
    assert_eq!(core.schema_sources(), before);
}

#[test]
fn internal_pack_projection_withdrawal_retains_record_high_water() {
    let core = GraphCore::new();
    let connector = medulla_types::contract::ResourceId::new("meristem").unwrap();
    let ontology = Arc::<str>::from(
        "@prefix owl: <http://www.w3.org/2002/07/owl#> . @prefix ex: <http://example/> . ex:A a owl:Class .",
    );
    apply_pack_source(
        &core,
        "pack__test",
        PACK_TENANT,
        &connector,
        "pack:meristem:1:aaaa",
        PackSourceDocuments {
            external_schemas: Vec::new(),
            shapes: Vec::new(),
            ontologies: vec![pack_document(
                "ontology://meristem/a.ttl",
                Arc::clone(&ontology),
            )],
            crosswalks: Vec::new(),
        },
        None,
    )
    .unwrap();
    let withdrawal = apply_pack_source(
        &core,
        "pack__test",
        PACK_TENANT,
        &connector,
        "pack:meristem:2:bbbb",
        PackSourceDocuments {
            external_schemas: Vec::new(),
            shapes: Vec::new(),
            ontologies: Vec::new(),
            crosswalks: Vec::new(),
        },
        None,
    )
    .unwrap();
    assert!(withdrawal.changed);
    let tombstone = core
        .schema_sources()
        .dynamic
        .get("pack:meristem")
        .cloned()
        .unwrap();
    assert!(tombstone.shapes_ttl.is_none() && tombstone.ontology_ttl.is_none());
    let persisted = core.to_msgpack().unwrap();
    let restarted = GraphCore::new();
    restarted.from_msgpack(&persisted).unwrap();
    assert_eq!(restarted.schema_sources(), core.schema_sources());
    assert!(apply_pack_source(
        &restarted,
        "pack__test",
        PACK_TENANT,
        &connector,
        "pack:meristem:1:cccc",
        PackSourceDocuments {
            external_schemas: Vec::new(),
            shapes: Vec::new(),
            ontologies: vec![pack_document("ontology://meristem/a.ttl", ontology)],
            crosswalks: Vec::new()
        },
        None
    )
    .unwrap_err()
    .contains("SCHEMA_SOURCE_REGRESSION"));
}

#[test]
fn pack_members_keep_file_local_blank_nodes_and_uri_order_is_stable() {
    let first = PackSchemaDocument {
        uri: "shapes://test/a.ttl".to_string(),
        body: Arc::from(
            "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
             @prefix ex: <http://example/> .\n\
             ex:A a sh:NodeShape ; sh:property _:b0 .\n\
             _:b0 sh:path ex:a .\n",
        ),
    };
    let second = PackSchemaDocument {
        uri: "shapes://test/b.ttl".to_string(),
        body: Arc::from(
            "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
             @prefix ex: <http://example/> .\n\
             ex:B a sh:NodeShape ; sh:property _:b0 .\n\
             _:b0 sh:path ex:b .\n",
        ),
    };
    let forward = compose_pack_documents("SHAPES", vec![first.clone(), second.clone()])
        .unwrap()
        .unwrap();
    let reverse = compose_pack_documents("SHAPES", vec![second, first])
        .unwrap()
        .unwrap();
    assert_eq!(
        forward, reverse,
        "archive member order cannot change identity"
    );
    assert_eq!(
        medulla_types::contract::Digest256::sha256(forward.as_bytes()),
        medulla_types::contract::Digest256::sha256(reverse.as_bytes())
    );

    let triples = medulla_rdf::mapping::parse_turtle(&forward).unwrap();
    let blank_subjects: std::collections::BTreeSet<_> = triples
        .iter()
        .filter_map(|triple| match &triple.subject {
            medulla_rdf::oxrdf::NamedOrBlankNode::BlankNode(node) => {
                Some(node.as_str().to_string())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        blank_subjects.len(),
        2,
        "two local _:b0 labels must not merge"
    );
}

#[test]
fn pack_member_failures_name_the_exact_uri_and_enforce_each_bound() {
    let error = compose_pack_documents(
        "ONTOLOGY",
        vec![PackSchemaDocument {
            uri: "ontology://test/broken.ttl".to_string(),
            body: Arc::from("not turtle"),
        }],
    )
    .unwrap_err();
    assert!(error.contains("ontology://test/broken.ttl"));

    let error = compose_pack_documents(
        "SHAPES",
        vec![PackSchemaDocument {
            uri: "shapes://test/oversized.ttl".to_string(),
            body: Arc::from(
                "x".repeat(medulla_catalog_types::graph_schema::MAX_SCHEMA_DOCUMENT_BYTES + 1),
            ),
        }],
    )
    .unwrap_err();
    assert!(error.contains("shapes://test/oversized.ttl"));
    assert!(error.contains("TOO_LARGE"));
}

// spec: MEDULLA-TYPED-PACKS-R053
/// A pack schema attached to a graph that is not its reserved projection graph
/// copies a static snapshot: the installed bytes are fixed at attach time, and
/// later pack head updates -- which only the reserved projection graph follows
/// -- never enter the snapshot graph.
#[test]
fn attach_pack_into_a_spare_graph_copies_a_static_snapshot() {
    let connector = medulla_types::contract::ResourceId::new("graph-os").unwrap();
    let v1 = Arc::<str>::from(
        "@prefix owl: <http://www.w3.org/2002/07/owl#> . @prefix ex: <http://example/> . ex:A a owl:Class .",
    );
    let v2 = Arc::<str>::from(
        "@prefix owl: <http://www.w3.org/2002/07/owl#> . @prefix ex: <http://example/> . ex:A a owl:Class . ex:B a owl:Class .",
    );

    // The snapshot taken from pack head v1 is installed into a graph other
    // than the connector's reserved projection graph.
    let spare = GraphCore::new();
    apply_pack_source(
        &spare,
        "tenant-a-spare",
        PACK_TENANT,
        &connector,
        "pack:graph-os:1:aaaa",
        PackSourceDocuments {
            external_schemas: Vec::new(),
            shapes: Vec::new(),
            ontologies: vec![pack_document("ontology://graph-os/a.ttl", Arc::clone(&v1))],
            crosswalks: Vec::new(),
        },
        None,
    )
    .unwrap();
    let snapshot = spare
        .schema_sources()
        .dynamic
        .get("pack:graph-os")
        .cloned()
        .unwrap();
    let snapshot_digest = spare.schema_sources().composed_digest().to_hex();

    // The pack head moves to v2; the reserved projection graph follows it.
    let reserved = GraphCore::new();
    let reserved_name = pack_projection_graph_name("tenant-a", &connector).unwrap();
    apply_pack_source(
        &reserved,
        &reserved_name,
        PACK_TENANT,
        &connector,
        "pack:graph-os:2:bbbb",
        PackSourceDocuments {
            external_schemas: Vec::new(),
            shapes: Vec::new(),
            ontologies: vec![pack_document("ontology://graph-os/a.ttl", Arc::clone(&v2))],
            crosswalks: Vec::new(),
        },
        None,
    )
    .unwrap();

    // The snapshot graph still serves the v1 bytes under the v1 record...
    assert_eq!(
        spare.schema_sources().composed_digest().to_hex(),
        snapshot_digest
    );
    let still = spare
        .schema_sources()
        .dynamic
        .get("pack:graph-os")
        .cloned()
        .unwrap();
    assert_eq!(still.ontology_ttl, snapshot.ontology_ttl);
    assert_eq!(
        still.origin,
        medulla_graph_core::graph::SchemaSourceOrigin::Pack {
            connector: "graph-os".to_string(),
            record_id: "pack:graph-os:1:aaaa".to_string(),
            tenant: Some(PACK_TENANT.to_string()),
        }
    );
    // ...while the reserved projection graph serves the newer head.
    let followed = reserved
        .schema_sources()
        .dynamic
        .get("pack:graph-os")
        .cloned()
        .unwrap();
    assert_ne!(followed.ontology_ttl, snapshot.ontology_ttl);
    // The composed projection is an N-Triples union, so the v2 addition shows
    // up under its expanded IRI, not the author's `ex:` prefix.
    assert!(followed.ontology_ttl.unwrap().contains("http://example/B"));
}

// spec: MEDULLA-TYPED-PACKS-R059
/// Schema attach validates the staged document and the writes it introduces,
/// but never scans or revalidates the graph's existing data: a node that
/// already violates the stricter shape being attached does not block the
/// attach, while a self-contradicting attached document is refused.
#[test]
fn attach_validates_its_own_writes_and_never_scans_existing_data() {
    let core = GraphCore::new();
    // An existing graph node that violates the shape being attached: a
    // `Person` without the `name` the shape requires.
    core.add_node(
        "http://example/ada".to_string(),
        rmp_serde::to_vec_named(&serde_json::json!({
            "type": "http://example/Person"
        }))
        .unwrap(),
    );
    let shape = "@prefix sh: <http://www.w3.org/ns/shacl#> . \
                 @prefix ex: <http://example/> . \
                 ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ; \
                 sh:property [ sh:path ex:name ; sh:minCount 1 ] .";
    let attach_strict = GraphSchemaOp::Attach {
        source_id: "admin:strict".to_string(),
        shapes_ttl: Some(shape.to_string()),
        ontology_ttl: None,
        if_composed_digest: None,
    };
    apply(&core, "g", &attach_strict).unwrap();
    assert!(core.schema_sources().dynamic.contains_key("admin:strict"));

    // Writes the attach introduces are still validated: a self-contradicting
    // document is refused with the source set unchanged.
    let before = core.schema_sources();
    let error = apply(
        &core,
        "g",
        &attach(
            "admin:bad",
            "@prefix owl: <http://www.w3.org/2002/07/owl#> . \
             @prefix ex: <http://example/> . \
             ex:A a owl:Class ; owl:disjointWith ex:B . ex:B a owl:Class . ex:x a ex:A, ex:B .",
        ),
    )
    .unwrap_err();
    assert!(error.starts_with("ONTOLOGY_INCONSISTENT"), "{error}");
    assert_eq!(core.schema_sources(), before);
}

/// BUG-009: a pack projection decides its entering-schema check on the blocking
/// pool before the commit kernel runs, and the verdict lands on the
/// content-keyed compiled entry, so the in-commit check of the same sources is a
/// cache hit rather than a second tableau on a runtime worker.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pack_projection_prevalidation_memoizes_the_entering_schema_verdict() {
    let core = Arc::new(GraphCore::new());
    let connector = medulla_types::contract::ResourceId::new("bug009-prevalidate").unwrap();
    let ontology = Arc::<str>::from(
        "@prefix owl: <http://www.w3.org/2002/07/owl#> . @prefix ex: <http://example/bug009/> . ex:Prevalidated a owl:Class .",
    );
    let documents = |ontologies: Vec<PackSchemaDocument>| PackProjectionDocuments {
        external_schemas: Vec::new(),
        shapes: Vec::new(),
        ontologies,
        crosswalks: Vec::new(),
        seed: Vec::new(),
        seed_receipt: None,
        surface: None,
    };
    let source = |ontologies: Vec<PackSchemaDocument>| PackSourceDocuments {
        external_schemas: Vec::new(),
        shapes: Vec::new(),
        ontologies,
        crosswalks: Vec::new(),
    };
    let good = vec![pack_document(
        "ontology://bug009-prevalidate/a.ttl",
        Arc::clone(&ontology),
    )];
    let record_id = "pack:bug009-prevalidate:1:aaaa";

    prevalidate_pack_projection(
        &core,
        PACK_TENANT,
        &connector,
        record_id,
        &documents(good.clone()),
    )
    .await
    .unwrap();
    let (candidate, changed, _) = pack_candidate_sources(
        &core.schema_sources(),
        PACK_TENANT,
        &connector,
        record_id,
        source(good.clone()),
        None,
    )
    .unwrap();
    assert!(changed);
    let composed = medulla_server_api::schema_compose::validate_and_compose(&candidate).unwrap();
    assert!(
        composed.abox_verified(),
        "prevalidation must leave the ABox verdict on the shared compiled entry"
    );
    let committed = apply_pack_source(
        &core,
        "pack__bug009",
        PACK_TENANT,
        &connector,
        record_id,
        source(good),
        None,
    )
    .unwrap();
    assert!(committed.changed);
    assert_eq!(
        committed.composed_digest,
        candidate.composed_digest().to_hex()
    );

    // A refusal is the refusal the commit would give for the same bytes.
    let broken = vec![pack_document(
        "ontology://bug009-prevalidate/broken.ttl",
        Arc::from("this is not turtle"),
    )];
    let error = prevalidate_pack_projection(
        &core,
        PACK_TENANT,
        &connector,
        record_id,
        &documents(broken.clone()),
    )
    .await
    .unwrap_err();
    let commit_error = apply_pack_source(
        &core,
        "pack__bug009",
        PACK_TENANT,
        &connector,
        record_id,
        source(broken),
        None,
    )
    .unwrap_err();
    assert_eq!(error, commit_error);
}

/// A core-metadata crosswalk record for `local`, pinned to the live core revision.
fn core_crosswalk_body(local: &str, target: &str, category: &str) -> Arc<str> {
    use medulla_catalog_types::domain_pack::{
        ApprovedTargetDeclaration, VocabularyCrosswalk, VocabularyCrosswalkRelation,
        VOCABULARY_CROSSWALK_SCHEMA_VERSION,
    };
    let mut record = VocabularyCrosswalk {
        schema_version: VOCABULARY_CROSSWALK_SCHEMA_VERSION,
        source_class_iri: format!("https://knuckles-team.github.io/ontology/core#{local}"),
        target_class_iri: format!("http://schema.org/{target}"),
        relation: VocabularyCrosswalkRelation::SkosExactMatch,
        source_revision: medulla_graph_core::graph::GraphSchemaSources::default()
            .composed_digest()
            .to_hex(),
        target_vocabulary_revision: "ab".repeat(32),
        origin: "core-metadata-v1".to_string(),
        content_digest: String::new(),
        approved_target_declarations: vec![ApprovedTargetDeclaration {
            class_iri: format!("http://schema.org/{target}"),
            category: category.to_string(),
        }],
    };
    record.content_digest = record.compute_content_digest();
    Arc::<str>::from(serde_json::to_string(&record).unwrap())
}

// spec: MEDULLA-TYPED-PACKS-R097.2.3
#[test]
fn pack_activation_stores_the_revision_bound_crosswalk_coverage() {
    let core = GraphCore::new();
    let connector = medulla_types::contract::ResourceId::new("coverage-pack").unwrap();
    let documents = || PackSourceDocuments {
        external_schemas: Vec::new(),
        shapes: Vec::new(),
        ontologies: Vec::new(),
        crosswalks: vec![pack_document(
            "crosswalk://coverage-pack/person.json",
            core_crosswalk_body("Person", "Person", "continuant"),
        )],
    };
    let first = apply_pack_source(
        &core,
        "pack__coverage-pack",
        PACK_TENANT,
        &connector,
        "record-1",
        documents(),
        None,
    )
    .unwrap();
    assert!(first.changed);

    let sources = core.schema_sources();
    let stored = sources.dynamic.get("pack:coverage-pack").unwrap();
    let bound = stored.crosswalk_coverage.as_ref().expect("coverage stored");
    assert_eq!(bound.binding.tenant_id, PACK_TENANT);
    assert_eq!(bound.binding.graph, "pack__coverage-pack");
    assert_eq!(bound.binding.pack_record_id, "record-1");
    assert_eq!(bound.binding.target_vocabulary_revisions.len(), 1);
    assert_eq!(bound.binding.schema_digest, sources.composed_digest().to_hex());
    let coverage = &bound.coverage;
    let person = "https://knuckles-team.github.io/ontology/core#Person".to_string();
    assert_eq!(coverage.mapped, vec![person.clone()]);
    assert!(coverage
        .unmapped
        .iter()
        .all(|entry| entry.class_iri != person));
    assert_eq!(
        coverage.declared as usize,
        coverage.mapped.len() + coverage.unmapped.len()
    );
    assert!(
        coverage.unmapped.len() > 10,
        "the core catalog has many classes"
    );
    assert_eq!(coverage.source_digest.len(), 64);

    // The stored report is exactly what the import-side function computes over
    // the same set and the same pin: one function, one answer.
    let pinned = crate::admin::connector_pack::import::vocabulary::pin_composed(
        &medulla_graph_core::graph::GraphSchemaSources::default(),
        &sources,
    )
    .unwrap();
    let recomputed = crate::admin::connector_pack::import::crosswalk::coverage_of_set(
        &stored.crosswalks,
        &pinned,
    )
    .unwrap();
    assert_eq!(&recomputed, coverage);

    // A repeat activation of the same head changes nothing.
    let again = apply_pack_source(
        &core,
        "pack__coverage-pack",
        PACK_TENANT,
        &connector,
        "record-1",
        documents(),
        Some(&first.composed_digest),
    )
    .unwrap();
    assert!(!again.changed);

    // A crosswalk naming a class outside the core revision refuses activation
    // and leaves the stored coverage untouched.
    let before = core.schema_sources();
    let refused = apply_pack_source(
        &core,
        "pack__coverage-pack",
        PACK_TENANT,
        &connector,
        "record-2",
        PackSourceDocuments {
            external_schemas: Vec::new(),
            shapes: Vec::new(),
            ontologies: Vec::new(),
            crosswalks: vec![pack_document(
                "crosswalk://coverage-pack/unknown.json",
                core_crosswalk_body("NoSuchClass", "Person", "continuant"),
            )],
        },
        None,
    );
    assert!(refused.is_err());
    assert_eq!(core.schema_sources().dynamic, before.dynamic);
}

/// One external schema body: a mapped class and an unmapped property, both outside the
/// platform vocabulary.
fn external_schema_body(extra: serde_json::Value) -> Arc<str> {
    let digest = medulla_types::contract::Digest256::sha256(b"external schema");
    let mut value = serde_json::json!({
        "schema_version": 1,
        "source_schema_digest": digest,
        "source_revision": "ext-2026-01",
        "mapping_revision": "map-1",
        "terms": [
            {"external_iri": "http://example.org/ext#Patient", "kind": "class"},
            {"external_iri": "http://example.org/ext#status", "kind": "datatype_property"}
        ],
        "mappings": [{
            "schema_version": 1,
            "source_schema_digest": digest,
            "source_revision": "ext-2026-01",
            "external_iri": "http://example.org/ext#Patient",
            "external_kind": "class",
            "platform_iri": "https://knuckles-team.github.io/ontology/core#Person",
            "platform_kind": "class",
            "mapping_revision": "map-1"
        }]
    });
    if let (Some(target), Some(extra)) = (value.as_object_mut(), extra.as_object()) {
        target.extend(extra.clone());
    }
    Arc::<str>::from(value.to_string())
}

// spec: MEDULLA-TYPED-PACKS-R097.2.7.3
#[test]
fn activation_stores_external_coverage_for_inspection() {
    let core = GraphCore::new();
    let connector = medulla_types::contract::ResourceId::new("external-pack").unwrap();
    let documents = |body: Arc<str>| PackSourceDocuments {
        external_schemas: vec![pack_document(
            "external-schema://external-pack/ext.json",
            body,
        )],
        shapes: Vec::new(),
        ontologies: Vec::new(),
        crosswalks: Vec::new(),
    };
    let first = apply_pack_source(
        &core,
        "pack__external-pack",
        PACK_TENANT,
        &connector,
        "record-1",
        documents(external_schema_body(serde_json::json!({}))),
        None,
    )
    .unwrap();
    assert!(first.changed);

    let sources = core.schema_sources();
    let stored = sources.dynamic.get("pack:external-pack").unwrap();
    assert_eq!(stored.external_coverage.len(), 1, "one coverage per external schema");
    let coverage = &stored.external_coverage[0];
    assert_eq!(coverage.pack_revision, "record-1");
    assert_eq!(
        coverage.platform_vocabulary_revision,
        medulla_graph_core::graph::GraphSchemaSources::default()
            .composed_digest()
            .to_hex()
    );
    assert_eq!(coverage.schema_revision, sources.composed_digest().to_hex());
    assert_eq!(coverage.terms.len(), 2);
    let mapped = coverage
        .terms
        .iter()
        .filter(|term| {
            matches!(
                term.result,
                medulla_catalog_types::external_schema::ExternalTermResult::Mapped { .. }
            )
        })
        .count();
    assert_eq!(mapped, 1, "the declared mapping is adopted; the other term is not");

    // The same head activated again changes nothing.
    let again = apply_pack_source(
        &core,
        "pack__external-pack",
        PACK_TENANT,
        &connector,
        "record-1",
        documents(external_schema_body(serde_json::json!({}))),
        None,
    )
    .unwrap();
    assert!(!again.changed);

    // A body whose carried report is not the computed one is refused with its code.
    let refused = apply_pack_source(
        &core,
        "pack__external-pack-forged",
        PACK_TENANT,
        &connector,
        "record-2",
        documents(external_schema_body(
            serde_json::json!({"coverage": {"schema_version": 1}}),
        )),
        None,
    )
    .unwrap_err();
    assert!(
        refused.starts_with("EXTERNAL_REPORT_FORGED") || refused.starts_with("MALFORMED_BODY"),
        "{refused}"
    );
}
