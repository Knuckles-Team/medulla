//! Registered-source acceptance for the infrastructure-v1 term additions
//! (MEDULLA-CQ-CONN-R068.2 to R068.27).
//!
//! Each test loads the registered `core:infrastructure@1` source through the
//! CoreSpec registry and asserts the added property with its stated domain and
//! range. The pullCount test additionally proves — per T-AUD-002 — that a
//! valid fixture conforms to the SHACL companion while a violating fixture
//! (a negative pull count) fails the shape.

use medulla_graph_core::graph::GraphSchemaSources;
use medulla_graph_core::graph::SchemaSourceOrigin;
use medulla_rdf::mapping::parse_turtle;
use medulla_rdf::oxrdf::{NamedNode, Triple};

// The infrastructure module's default namespace (its `:` prefix): the spec
// rows' `:term` names expand here, not in the shared `kg#` namespace.
const KG: &str = "https://knuckles-team.github.io/ontology/infrastructure#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const OWL: &str = "http://www.w3.org/2002/07/owl#";
const RDFS_DOMAIN: &str = "http://www.w3.org/2000/01/rdf-schema#domain";
const RDFS_RANGE: &str = "http://www.w3.org/2000/01/rdf-schema#range";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

fn has_iri(triples: &[Triple], subject: &str, predicate: &str, object: &str) -> bool {
    triples.contains(&Triple::new(
        NamedNode::new(subject).unwrap(),
        NamedNode::new(predicate).unwrap(),
        NamedNode::new(object).unwrap(),
    ))
}

fn infrastructure_ontology() -> Vec<Triple> {
    let sources = GraphSchemaSources::default();
    let source = sources
        .core
        .get("core:infrastructure@1")
        .unwrap_or_else(|| panic!("registered source core:infrastructure@1 is missing"));
    match &source.origin {
        SchemaSourceOrigin::Core {
            module, version, ..
        } => {
            assert_eq!(module, "infrastructure", "source module ownership changed");
            assert_eq!(*version, 1, "unexpected source version");
        }
        origin => panic!("core:infrastructure@1 has non-core origin {origin:?}"),
    }
    parse_turtle(
        source
            .ontology_ttl
            .as_deref()
            .unwrap_or_else(|| panic!("registered infrastructure source has no ontology")),
    )
    .expect("registered infrastructure ontology parses")
}

/// The SHACL companion (MEDULLA-CQ-CONN-R068.14 pullCount lower bound).
fn infrastructure_shapes() -> &'static str {
    include_str!("../ontology/infrastructure-v1.shapes.ttl")
}

/// Shared kernel term namespace (`kg:` in the infrastructure module).
const CORE: &str = "https://knuckles-team.github.io/ontology/core#";

/// A valid fixture conforms to the SHACL companion; a violating fixture
/// fails it on `path`.
fn assert_shape(valid: &str, violating: &str, path: &str) {
    let prefixes = "@prefix : <https://knuckles-team.github.io/ontology/infrastructure#> .\n@prefix ex: <http://example.org/cq-conn/> .\n";
    let report =
        medulla_shacl::validate_turtle(infrastructure_shapes(), &format!("{prefixes}{valid}"))
            .expect("valid fixture parses and validates");
    assert!(
        report.conforms,
        "valid fixture conforms: {:?}",
        report.results
    );
    let report =
        medulla_shacl::validate_turtle(infrastructure_shapes(), &format!("{prefixes}{violating}"))
            .expect("violating fixture parses before validation");
    assert!(
        !report.conforms,
        "violating fixture fails the shape on {path}"
    );
    let expected = format!("<{KG}{path}>");
    assert!(
        report
            .results
            .iter()
            .any(|result| result.path.as_deref() == Some(expected.as_str())),
        "the violation is reported on {path}: {:?}",
        report.results
    );
}

#[allow(non_snake_case)]
mod ontology_audit_infrastructure_v1 {
    use super::*;

    // spec: MEDULLA-CQ-CONN-R068.13
    #[test]
    fn lastPushedAt_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}lastPushedAt");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}DatatypeProperty")),
            "lastPushedAt is declared as a datatype property"
        );
        assert!(
            has_iri(&triples, &term, RDFS_DOMAIN, &format!("{KG}ContainerImage")),
            "lastPushedAt's domain is ContainerImage"
        );
        assert!(
            has_iri(&triples, &term, RDFS_RANGE, &format!("{XSD}dateTime")),
            "lastPushedAt's range is xsd:dateTime"
        );
    }

    // spec: MEDULLA-CQ-CONN-R068.14
    #[test]
    fn pullCount_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}pullCount");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}DatatypeProperty")),
            "pullCount is declared as a datatype property"
        );
        assert!(
            has_iri(&triples, &term, RDFS_DOMAIN, &format!("{KG}ContainerImage")),
            "pullCount's domain is ContainerImage"
        );
        assert!(
            has_iri(&triples, &term, RDFS_RANGE, &format!("{XSD}integer")),
            "pullCount's range is xsd:integer"
        );

        // The non-negative bound lives in the SHACL companion: a valid fixture
        // conforms, a violating fixture fails the shape.
        let valid = r#"
@prefix : <https://knuckles-team.github.io/ontology/infrastructure#> .
@prefix ex: <http://example.org/cq-conn/> .
ex:img-1 a :ContainerImage ;
    :pullCount 1200 .
"#;
        let report = medulla_shacl::validate_turtle(infrastructure_shapes(), valid)
            .expect("valid pullCount fixture parses and validates");
        assert!(
            report.conforms,
            "a non-negative pullCount conforms: {:?}",
            report.results
        );

        let violating = r#"
@prefix : <https://knuckles-team.github.io/ontology/infrastructure#> .
@prefix ex: <http://example.org/cq-conn/> .
ex:img-2 a :ContainerImage ;
    :pullCount -1 .
"#;
        let report = medulla_shacl::validate_turtle(infrastructure_shapes(), violating)
            .expect("violating pullCount fixture parses before validation");
        assert!(!report.conforms, "a negative pullCount must fail the shape");
        // Report terms carry their N-Triples lexical form: an IRI path is
        // `<iri>`.
        assert!(
            report
                .results
                .iter()
                .any(|result| { result.path.as_deref() == Some(&format!("<{KG}pullCount>")) }),
            "the violation is reported on the pullCount path: {:?}",
            report.results
        );
    }

    // spec: MEDULLA-CQ-CONN-R068.15
    #[test]
    fn entityState_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}entityState");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}DatatypeProperty")),
            "entityState is declared as a datatype property"
        );
        assert!(
            has_iri(&triples, &term, RDFS_DOMAIN, &format!("{KG}Entity")),
            "entityState's domain is Entity"
        );
        assert!(
            has_iri(&triples, &term, RDFS_RANGE, &format!("{XSD}string")),
            "entityState's range is xsd:string"
        );
    }

    // spec: MEDULLA-CQ-CONN-R068.16
    #[test]
    fn unavailableSince_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}unavailableSince");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}DatatypeProperty")),
            "unavailableSince is declared as a datatype property"
        );
        assert!(
            has_iri(&triples, &term, RDFS_DOMAIN, &format!("{KG}Entity")),
            "unavailableSince's domain is Entity"
        );
        assert!(
            has_iri(&triples, &term, RDFS_RANGE, &format!("{XSD}dateTime")),
            "unavailableSince's range is xsd:dateTime"
        );
    }

    /// Verifies MEDULLA-CQ-CONN-R068.2: AutomationJob
    // spec: MEDULLA-CQ-CONN-R068.2
    #[test]
    fn AutomationJob_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}AutomationJob");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}Class")),
            "AutomationJob is declared as owl:Class"
        );
        assert_shape(
            r#"ex:j a :AutomationJob ; :jobStatus "successful" ."#,
            r#"ex:j a :AutomationJob ."#,
            "jobStatus",
        );
    }

    /// Verifies MEDULLA-CQ-CONN-R068.3: AutomationJobTemplate
    // spec: MEDULLA-CQ-CONN-R068.3
    #[test]
    fn AutomationJobTemplate_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}AutomationJobTemplate");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}Class")),
            "AutomationJobTemplate is declared as owl:Class"
        );
    }

    /// Verifies MEDULLA-CQ-CONN-R068.4: jobStatus
    // spec: MEDULLA-CQ-CONN-R068.4
    #[test]
    fn jobStatus_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}jobStatus");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}DatatypeProperty")),
            "jobStatus is declared as owl:DatatypeProperty"
        );
        assert!(
            has_iri(&triples, &term, RDFS_DOMAIN, &format!("{KG}AutomationJob")),
            "jobStatus's domain is KG:AutomationJob"
        );
        assert!(
            has_iri(&triples, &term, RDFS_RANGE, &format!("{XSD}string")),
            "jobStatus's range is XSD:string"
        );
        assert_shape(
            r#"ex:j a :AutomationJob ; :jobStatus "failed" ."#,
            r#"ex:j a :AutomationJob ; :jobStatus "exploded" ."#,
            "jobStatus",
        );
    }

    /// Verifies MEDULLA-CQ-CONN-R068.5: ranAgainstHost
    // spec: MEDULLA-CQ-CONN-R068.5
    #[test]
    fn ranAgainstHost_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}ranAgainstHost");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}ObjectProperty")),
            "ranAgainstHost is declared as owl:ObjectProperty"
        );
        assert!(
            has_iri(&triples, &term, RDFS_DOMAIN, &format!("{KG}AutomationJob")),
            "ranAgainstHost's domain is KG:AutomationJob"
        );
        assert!(
            has_iri(&triples, &term, RDFS_RANGE, &format!("{KG}Host")),
            "ranAgainstHost's range is KG:Host"
        );
    }

    /// Verifies MEDULLA-CQ-CONN-R068.6: routeHostname
    // spec: MEDULLA-CQ-CONN-R068.6
    #[test]
    fn routeHostname_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}routeHostname");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}DatatypeProperty")),
            "routeHostname is declared as owl:DatatypeProperty"
        );
        assert!(
            has_iri(&triples, &term, RDFS_DOMAIN, &format!("{KG}ProxyRoute")),
            "routeHostname's domain is KG:ProxyRoute"
        );
        assert!(
            has_iri(&triples, &term, RDFS_RANGE, &format!("{XSD}string")),
            "routeHostname's range is XSD:string"
        );
        assert_shape(
            r#"ex:r a :ProxyRoute ; :routeHostname "a.example" ."#,
            r#"ex:r a :ProxyRoute ."#,
            "routeHostname",
        );
    }

    /// Verifies MEDULLA-CQ-CONN-R068.7: upstreamAddress
    // spec: MEDULLA-CQ-CONN-R068.7
    #[test]
    fn upstreamAddress_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}upstreamAddress");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}DatatypeProperty")),
            "upstreamAddress is declared as owl:DatatypeProperty"
        );
        assert!(
            has_iri(&triples, &term, RDFS_DOMAIN, &format!("{KG}ProxyRoute")),
            "upstreamAddress's domain is KG:ProxyRoute"
        );
        assert!(
            has_iri(&triples, &term, RDFS_RANGE, &format!("{XSD}string")),
            "upstreamAddress's range is XSD:string"
        );
    }

    /// Verifies MEDULLA-CQ-CONN-R068.9: alertSeverity
    // spec: MEDULLA-CQ-CONN-R068.9
    #[test]
    fn alertSeverity_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}alertSeverity");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}DatatypeProperty")),
            "alertSeverity is declared as owl:DatatypeProperty"
        );
        assert!(
            has_iri(&triples, &term, RDFS_DOMAIN, &format!("{CORE}Alert")),
            "alertSeverity's domain is CORE:Alert"
        );
        assert!(
            has_iri(&triples, &term, RDFS_RANGE, &format!("{XSD}string")),
            "alertSeverity's range is XSD:string"
        );
    }

    /// Verifies MEDULLA-CQ-CONN-R068.10: alertState
    // spec: MEDULLA-CQ-CONN-R068.10
    #[test]
    fn alertState_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}alertState");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}DatatypeProperty")),
            "alertState is declared as owl:DatatypeProperty"
        );
        assert!(
            has_iri(&triples, &term, RDFS_DOMAIN, &format!("{CORE}Alert")),
            "alertState's domain is CORE:Alert"
        );
        assert!(
            has_iri(&triples, &term, RDFS_RANGE, &format!("{XSD}string")),
            "alertState's range is XSD:string"
        );
    }

    /// Verifies MEDULLA-CQ-CONN-R068.11: firingSince
    // spec: MEDULLA-CQ-CONN-R068.11
    #[test]
    fn firingSince_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}firingSince");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}DatatypeProperty")),
            "firingSince is declared as owl:DatatypeProperty"
        );
        assert!(
            has_iri(&triples, &term, RDFS_DOMAIN, &format!("{CORE}Alert")),
            "firingSince's domain is CORE:Alert"
        );
        assert!(
            has_iri(&triples, &term, RDFS_RANGE, &format!("{XSD}dateTime")),
            "firingSince's range is XSD:dateTime"
        );
    }

    /// Verifies MEDULLA-CQ-CONN-R068.17: desiredReplicas
    // spec: MEDULLA-CQ-CONN-R068.17
    #[test]
    fn desiredReplicas_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}desiredReplicas");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}DatatypeProperty")),
            "desiredReplicas is declared as owl:DatatypeProperty"
        );
        assert!(
            has_iri(&triples, &term, RDFS_DOMAIN, &format!("{KG}ContainerStack")),
            "desiredReplicas's domain is KG:ContainerStack"
        );
        assert!(
            has_iri(&triples, &term, RDFS_RANGE, &format!("{XSD}integer")),
            "desiredReplicas's range is XSD:integer"
        );
        assert_shape(
            r#"ex:s a :ContainerStack ; :desiredReplicas 3 ."#,
            r#"ex:s a :ContainerStack ; :desiredReplicas -1 ."#,
            "desiredReplicas",
        );
    }

    /// Verifies MEDULLA-CQ-CONN-R068.18: runningReplicas
    // spec: MEDULLA-CQ-CONN-R068.18
    #[test]
    fn runningReplicas_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}runningReplicas");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}DatatypeProperty")),
            "runningReplicas is declared as owl:DatatypeProperty"
        );
        assert!(
            has_iri(&triples, &term, RDFS_DOMAIN, &format!("{KG}ContainerStack")),
            "runningReplicas's domain is KG:ContainerStack"
        );
        assert!(
            has_iri(&triples, &term, RDFS_RANGE, &format!("{XSD}integer")),
            "runningReplicas's range is XSD:integer"
        );
        assert_shape(
            r#"ex:s a :ContainerStack ; :runningReplicas 0 ."#,
            r#"ex:s a :ContainerStack ; :runningReplicas -2 ."#,
            "runningReplicas",
        );
    }

    /// Verifies MEDULLA-CQ-CONN-R068.19: stackSourceRepository
    // spec: MEDULLA-CQ-CONN-R068.19
    #[test]
    fn stackSourceRepository_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}stackSourceRepository");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}ObjectProperty")),
            "stackSourceRepository is declared as owl:ObjectProperty"
        );
        assert!(
            has_iri(&triples, &term, RDFS_DOMAIN, &format!("{KG}ContainerStack")),
            "stackSourceRepository's domain is KG:ContainerStack"
        );
        assert!(
            has_iri(&triples, &term, RDFS_RANGE, &format!("{CORE}Repository")),
            "stackSourceRepository's range is CORE:Repository"
        );
    }

    /// Verifies MEDULLA-CQ-CONN-R068.20: ManagementEnvironment
    // spec: MEDULLA-CQ-CONN-R068.20
    #[test]
    fn ManagementEnvironment_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}ManagementEnvironment");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}Class")),
            "ManagementEnvironment is declared as owl:Class"
        );
    }

    /// Verifies MEDULLA-CQ-CONN-R068.21: pendingUpdateCount
    // spec: MEDULLA-CQ-CONN-R068.21
    #[test]
    fn pendingUpdateCount_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}pendingUpdateCount");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}DatatypeProperty")),
            "pendingUpdateCount is declared as owl:DatatypeProperty"
        );
        assert!(
            has_iri(
                &triples,
                &term,
                RDFS_DOMAIN,
                &format!("{KG}OperatingSystem")
            ),
            "pendingUpdateCount's domain is KG:OperatingSystem"
        );
        assert!(
            has_iri(&triples, &term, RDFS_RANGE, &format!("{XSD}integer")),
            "pendingUpdateCount's range is XSD:integer"
        );
        assert_shape(
            r#"ex:o a :OperatingSystem ; :pendingUpdateCount 4 ."#,
            r#"ex:o a :OperatingSystem ; :pendingUpdateCount -4 ."#,
            "pendingUpdateCount",
        );
    }

    /// Verifies MEDULLA-CQ-CONN-R068.22: uptimeSeconds
    // spec: MEDULLA-CQ-CONN-R068.22
    #[test]
    fn uptimeSeconds_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}uptimeSeconds");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}DatatypeProperty")),
            "uptimeSeconds is declared as owl:DatatypeProperty"
        );
        assert!(
            has_iri(
                &triples,
                &term,
                RDFS_DOMAIN,
                &format!("{KG}OperatingSystem")
            ),
            "uptimeSeconds's domain is KG:OperatingSystem"
        );
        assert!(
            has_iri(&triples, &term, RDFS_RANGE, &format!("{XSD}integer")),
            "uptimeSeconds's range is XSD:integer"
        );
    }

    /// Verifies MEDULLA-CQ-CONN-R068.23: recordType
    // spec: MEDULLA-CQ-CONN-R068.23
    #[test]
    fn recordType_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}recordType");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}DatatypeProperty")),
            "recordType is declared as owl:DatatypeProperty"
        );
        assert!(
            has_iri(&triples, &term, RDFS_DOMAIN, &format!("{KG}DnsRecord")),
            "recordType's domain is KG:DnsRecord"
        );
        assert!(
            has_iri(&triples, &term, RDFS_RANGE, &format!("{XSD}string")),
            "recordType's range is XSD:string"
        );
    }

    /// Verifies MEDULLA-CQ-CONN-R068.24: recordValue
    // spec: MEDULLA-CQ-CONN-R068.24
    #[test]
    fn recordValue_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}recordValue");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}DatatypeProperty")),
            "recordValue is declared as owl:DatatypeProperty"
        );
        assert!(
            has_iri(&triples, &term, RDFS_DOMAIN, &format!("{KG}DnsRecord")),
            "recordValue's domain is KG:DnsRecord"
        );
        assert!(
            has_iri(&triples, &term, RDFS_RANGE, &format!("{XSD}string")),
            "recordValue's range is XSD:string"
        );
    }

    /// Verifies MEDULLA-CQ-CONN-R068.25: lastConnectionStatus
    // spec: MEDULLA-CQ-CONN-R068.25
    #[test]
    fn lastConnectionStatus_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}lastConnectionStatus");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}DatatypeProperty")),
            "lastConnectionStatus is declared as owl:DatatypeProperty"
        );
        assert!(
            has_iri(&triples, &term, RDFS_DOMAIN, &format!("{KG}Host")),
            "lastConnectionStatus's domain is KG:Host"
        );
        assert!(
            has_iri(&triples, &term, RDFS_RANGE, &format!("{XSD}string")),
            "lastConnectionStatus's range is XSD:string"
        );
    }

    /// Verifies MEDULLA-CQ-CONN-R068.26: lastConnectionError
    // spec: MEDULLA-CQ-CONN-R068.26
    #[test]
    fn lastConnectionError_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}lastConnectionError");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}DatatypeProperty")),
            "lastConnectionError is declared as owl:DatatypeProperty"
        );
        assert!(
            has_iri(&triples, &term, RDFS_DOMAIN, &format!("{KG}Host")),
            "lastConnectionError's domain is KG:Host"
        );
        assert!(
            has_iri(&triples, &term, RDFS_RANGE, &format!("{XSD}string")),
            "lastConnectionError's range is XSD:string"
        );
    }

    /// Verifies MEDULLA-CQ-CONN-R068.27: stackInEnvironment
    // spec: MEDULLA-CQ-CONN-R068.27
    #[test]
    fn stackInEnvironment_declared() {
        let triples = infrastructure_ontology();
        let term = format!("{KG}stackInEnvironment");
        assert!(
            has_iri(&triples, &term, RDF_TYPE, &format!("{OWL}ObjectProperty")),
            "stackInEnvironment is declared as owl:ObjectProperty"
        );
        assert!(
            has_iri(&triples, &term, RDFS_DOMAIN, &format!("{KG}ContainerStack")),
            "stackInEnvironment's domain is KG:ContainerStack"
        );
        assert!(
            has_iri(
                &triples,
                &term,
                RDFS_RANGE,
                &format!("{KG}ManagementEnvironment")
            ),
            "stackInEnvironment's range is KG:ManagementEnvironment"
        );
    }
}
