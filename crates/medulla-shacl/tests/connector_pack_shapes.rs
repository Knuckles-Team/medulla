//! Native SHACL acceptance for the connector-pack surface shapes
//! (MEDULLA-TYPED-PACKS-R102.1).

use medulla_shacl::validate_turtle;

const SHAPES: &str = include_str!("../../medulla-core/ontology/connector_pack-v1.shapes.ttl");

const CP: &str = "https://knuckles-team.github.io/ontology/connector-pack#";

fn tool(component_id: Option<&str>, domain_link: Option<&str>) -> String {
    let mut parts = vec!["a cp:Tool".to_string()];
    if let Some(id) = component_id {
        parts.push(format!(r#"cp:componentId "{id}"^^xsd:string"#));
    }
    if let Some(link) = domain_link {
        parts.push(link.to_string());
    }
    format!(
        r#"@prefix cp: <https://knuckles-team.github.io/ontology/connector-pack#> .
@prefix kg: <https://knuckles-team.github.io/ontology/core#> .
@prefix ex: <https://knuckles-team.github.io/ontology/test/connector-pack#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

ex:checkoutTool {} .

ex:checkout a kg:Service .
"#,
        parts.join(" ;\n    ")
    )
}

fn feature(realized_by: Option<&str>) -> String {
    format!(
        r#"@prefix cp: <https://knuckles-team.github.io/ontology/connector-pack#> .
@prefix ex: <https://knuckles-team.github.io/ontology/test/connector-pack#> .

ex:billingFeature a cp:Feature{} .
"#,
        realized_by
            .map(|member| format!(" ; cp:realizedBy ex:{member}"))
            .unwrap_or_default()
    )
}

// spec: MEDULLA-TYPED-PACKS-R102.1
#[test]
fn surface_shapes_accept_valid_and_refuse_invalid() {
    // A tool with one component id and one operatesOn domain class conforms.
    let valid = validate_turtle(
        SHAPES,
        &tool(Some("tool:checkout"), Some("cp:operatesOn ex:checkout")),
    )
    .expect("the shapes and a valid tool parse");
    assert!(valid.conforms, "valid tool: {:?}", valid.results);

    // The providesCapability branch is accepted in place of operatesOn.
    let via_capability = validate_turtle(
        SHAPES,
        &tool(
            Some("tool:checkout"),
            Some("cp:providesCapability ex:billingCap"),
        ),
    )
    .expect("the shapes and a capability tool parse");
    assert!(
        via_capability.conforms,
        "capability tool: {:?}",
        via_capability.results
    );

    // A feature with one realised-by member conforms.
    let valid_feature = validate_turtle(SHAPES, &feature(Some("tool")))
        .expect("the shapes and a valid feature parse");
    assert!(
        valid_feature.conforms,
        "valid feature: {:?}",
        valid_feature.results
    );

    // A tool without its component id is refused.
    let no_component = validate_turtle(SHAPES, &tool(None, Some("cp:operatesOn ex:checkout")))
        .expect("the shapes and the componentless tool parse");
    assert!(
        !no_component.conforms,
        "a componentless tool must be refused"
    );
    assert!(
        no_component.results.iter().any(|result| {
            result
                .path
                .as_deref()
                .map(|path| path.trim_matches(['<', '>']))
                == Some(&format!("{CP}componentId"))
        }),
        "the refusal names the componentId path: {:?}",
        no_component.results
    );

    // A tool with a component id but no domain link is refused.
    let no_domain = validate_turtle(SHAPES, &tool(Some("tool:checkout"), None))
        .expect("the shapes and the domainless tool parse");
    assert!(
        !no_domain.conforms,
        "a tool without operatesOn/providesCapability must be refused"
    );

    // A feature without a realised-by member is refused.
    let bare_feature =
        validate_turtle(SHAPES, &feature(None)).expect("the shapes and the bare feature parse");
    assert!(
        !bare_feature.conforms,
        "a feature without a realised-by member must be refused"
    );
    assert!(
        bare_feature.results.iter().any(|result| {
            result
                .path
                .as_deref()
                .map(|path| path.trim_matches(['<', '>']))
                == Some(&format!("{CP}realizedBy"))
        }),
        "the refusal names the realizedBy path: {:?}",
        bare_feature.results
    );
}

const CONTRACT: &str = r#"@prefix cp: <https://knuckles-team.github.io/ontology/connector-pack#> .
@prefix c: <https://knuckles-team.github.io/ontology/connector-pack/contract/acme/> .
@prefix kg: <https://knuckles-team.github.io/ontology/core#> .

<https://knuckles-team.github.io/ontology/connector-pack/contract/acme> a cp:IngestContract ;
    cp:contractOf <https://knuckles-team.github.io/ontology/connector-pack/connector/acme> ;
    cp:contractVersion "1" ;
    cp:contractDigest "sha256:abc" ;
    cp:privacyClass "internal" ;
    cp:hasSourceObject c:ticket ;
    cp:hasCursorPolicy c:cursor ;
    cp:hasAccessPolicy c:access ;
    cp:requiresProvenance c:provenance .

c:ticket a cp:SourceObject ;
    cp:objectName "ticket" ;
    cp:ontologyClass kg:Ticket ;
    cp:hasKeyRule c:ticket%2Fkey ;
    cp:hasFieldMapping c:ticket%2Ffield%2Ftitle .

c:ticket%2Fkey a cp:KeyRule ; cp:keyField "id" .

c:ticket%2Ffield%2Ftitle a cp:FieldMapping ;
    cp:sourceField "title" ;
    cp:mapsPredicate "https://knuckles-team.github.io/ontology/core#title" .

c:cursor a cp:CursorPolicy ; cp:cursorKind "timestamp" .

c:access a cp:AccessPolicy ;
    cp:defaultAcl "quarantine" ;
    cp:aclField "acl" ;
    cp:tenantField "tenant" ;
    cp:readRole "reader" .

c:provenance a cp:ProvenanceRequirement ; cp:requiredField "source_id" .
"#;

fn refused(data: &str, why: &str) {
    let report = validate_turtle(SHAPES, data).expect("shapes and data parse");
    assert!(!report.conforms, "{why} must be refused");
}

// spec: MEDULLA-TYPED-PACKS-R101.2
#[test]
fn each_contract_shape_accepts_valid_and_refuses_invalid() {
    let valid = validate_turtle(SHAPES, CONTRACT).expect("shapes and contract parse");
    assert!(valid.conforms, "valid contract: {:?}", valid.results);

    // Contract: exactly one connector, version and digest; a closed privacy class.
    refused(
        &CONTRACT.replace("    cp:contractVersion \"1\" ;\n", ""),
        "a contract without a version",
    );
    refused(
        &CONTRACT.replace("    cp:contractDigest \"sha256:abc\" ;\n", ""),
        "a contract without a digest",
    );
    refused(
        &CONTRACT.replace(
            "cp:privacyClass \"internal\"",
            "cp:privacyClass \"classified\"",
        ),
        "an unknown privacy class",
    );
    refused(
        &CONTRACT.replace(
            "cp:contractOf <https://knuckles-team.github.io/ontology/connector-pack/connector/acme>",
            "cp:contractOf <https://knuckles-team.github.io/ontology/connector-pack/connector/acme>, <https://knuckles-team.github.io/ontology/connector-pack/connector/other>",
        ),
        "a contract naming two connectors",
    );
    for kind in ["opaque", "timestamp", "commit", "offset", "page_token"] {
        let ok = validate_turtle(
            SHAPES,
            &CONTRACT.replace(
                "cp:cursorKind \"timestamp\"",
                &format!("cp:cursorKind \"{kind}\""),
            ),
        )
        .unwrap();
        assert!(ok.conforms, "cursor kind {kind}: {:?}", ok.results);
    }
    refused(
        &CONTRACT.replace("cp:cursorKind \"timestamp\"", "cp:cursorKind \"watermark\""),
        "an unknown cursor kind",
    );
    for acl in ["quarantine", "inherit", "public"] {
        let ok = validate_turtle(
            SHAPES,
            &CONTRACT.replace(
                "cp:defaultAcl \"quarantine\"",
                &format!("cp:defaultAcl \"{acl}\""),
            ),
        )
        .unwrap();
        assert!(ok.conforms, "default ACL {acl}: {:?}", ok.results);
    }
    refused(
        &CONTRACT.replace("cp:defaultAcl \"quarantine\"", "cp:defaultAcl \"open\""),
        "an unknown default ACL",
    );

    // Source object: exactly one ontology class and one key rule.
    refused(
        &CONTRACT.replace("    cp:ontologyClass kg:Ticket ;\n", ""),
        "a source object without an ontology class",
    );
    refused(
        &CONTRACT.replace("    cp:hasKeyRule c:ticket%2Fkey ;\n", ""),
        "a source object without a key rule",
    );
    refused(
        &CONTRACT.replace("cp:keyField \"id\"", "cp:objectName \"id\""),
        "a key rule without a key field",
    );

    // Field mapping: one source field and one predicate.
    refused(
        &CONTRACT.replace("    cp:sourceField \"title\" ;\n", ""),
        "a field mapping without a source field",
    );
    refused(
        &CONTRACT.replace(
            "    cp:mapsPredicate \"https://knuckles-team.github.io/ontology/core#title\" .",
            "    cp:sourceField \"title\" .",
        ),
        "a field mapping without a predicate",
    );

    // Provenance requirement: at least one required field.
    refused(
        &CONTRACT.replace(
            "cp:requiredField \"source_id\"",
            "cp:keyField \"source_id\"",
        ),
        "a provenance requirement without a required field",
    );
}
