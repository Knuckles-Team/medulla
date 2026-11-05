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

const SEED_NS: &str = "http://knuckles.team/kg/connector-seed#";

fn digest(byte: u8) -> Digest256 {
    Digest256::from_bytes([byte; 32])
}

fn shapes() -> medulla_shacl::Graph {
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
    assert_eq!(initial.state.parts.len(), 3);
    assert!(initial
        .state
        .parts
        .values()
        .all(|part| part.triple_count <= MAX_SEED_TRIPLES));

    let second = seed("new", digest(3));
    let changed = prepare_seed_import(&second, Some(&initial.state), &shapes, &sources)
        .expect("partial seed replacement");
    assert_eq!(
        changed.outcome,
        SeedImportOutcome::Replaced {
            changed_parts: BTreeSet::from([SeedPartKey::Catalog])
        }
    );
    assert_eq!(
        changed.state.parts[&SeedPartKey::Identity],
        initial.state.parts[&SeedPartKey::Identity]
    );
    assert_eq!(
        changed.state.parts[&SeedPartKey::References],
        initial.state.parts[&SeedPartKey::References]
    );
    assert_ne!(
        changed.state.parts[&SeedPartKey::Catalog],
        initial.state.parts[&SeedPartKey::Catalog]
    );
}

#[test]
fn equal_content_digest_is_a_noop_after_the_existing_state_is_known() {
    let shapes = shapes();
    let sources = GraphSchemaSources::default();
    let first = seed("same", digest(4));
    let initial =
        prepare_seed_import(&first, None, &shapes, &sources).expect("initial seed import");
    let unchanged = prepare_seed_import(&first, Some(&initial.state), &shapes, &sources)
        .expect("equal digest should be unchanged");
    assert_eq!(unchanged.outcome, SeedImportOutcome::Unchanged);
    assert_eq!(unchanged.state, initial.state);
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
    let before = initial.state.clone();

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
    assert_eq!(initial.state, before);
}
