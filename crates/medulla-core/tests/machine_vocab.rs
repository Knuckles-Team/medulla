//! The canonical machine vocabulary (MEDULLA-MACHINE-R001, R003, R004, R005.5).
//!
//! The registered `core:machine@1` source declares the one machine class, the seven part
//! classes, the observes link and the records of a probe and of a lifecycle transition. The
//! companion shapes validate data against them with the native SHACL engine. A scan proves the
//! six retired host and part classes are gone from the core ontology, the fixtures and the
//! Turtle of the tests.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use medulla_graph_core::graph::GraphSchemaSources;
use medulla_rdf::mapping::parse_turtle;
use medulla_rdf::oxrdf::{NamedNode, NamedOrBlankNode, Term, Triple};

const CORE: &str = "https://knuckles-team.github.io/ontology/core#";
const INFRA: &str = "https://knuckles-team.github.io/ontology/infrastructure#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS_LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";
const RDFS_COMMENT: &str = "http://www.w3.org/2000/01/rdf-schema#comment";
const RDFS_DOMAIN: &str = "http://www.w3.org/2000/01/rdf-schema#domain";
const RDFS_RANGE: &str = "http://www.w3.org/2000/01/rdf-schema#range";
const SUBCLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const OWL_CLASS: &str = "http://www.w3.org/2002/07/owl#Class";
const BFO_CONTINUANT: &str = "http://purl.obolibrary.org/obo/BFO_0000004";

const MACHINE_SHAPES: &str = include_str!("../ontology/machine-v1.shapes.ttl");

/// The six retired terms as full IRIs.
const RETIRED_IRIS: [&str; 6] = [
    "https://knuckles-team.github.io/ontology/infrastructure#Host",
    "https://knuckles-team.github.io/ontology/core#InfrastructureHost",
    "https://knuckles-team.github.io/ontology/core#HardwareNode",
    "https://knuckles-team.github.io/ontology/infrastructure#NetworkInterface",
    "https://knuckles-team.github.io/ontology/infrastructure#GPUAccelerator",
    "https://knuckles-team.github.io/ontology/infrastructure#hasInterface",
];

/// The six retired terms as they are written in text: prefixed or after a `#`.
const RETIRED_TOKENS: [&str; 14] = [
    "infra:Host",
    "infrastructure#Host",
    "core:InfrastructureHost",
    "kg:InfrastructureHost",
    "core#InfrastructureHost",
    "kg:HardwareNode",
    "core:HardwareNode",
    "core#HardwareNode",
    "infra:NetworkInterface",
    "infrastructure#NetworkInterface",
    "infra:GPUAccelerator",
    "infrastructure#GPUAccelerator",
    "infra:hasInterface",
    "infrastructure#hasInterface",
];

fn core(local: &str) -> String {
    format!("{CORE}{local}")
}

fn registered(id: &str) -> (Option<String>, Option<String>) {
    let sources = GraphSchemaSources::default();
    let source = sources
        .core
        .get(id)
        .unwrap_or_else(|| panic!("registered source {id} is missing"));
    (
        source.ontology_ttl.as_deref().map(str::to_owned),
        source.shapes_ttl.as_deref().map(str::to_owned),
    )
}

fn machine_ontology() -> Vec<Triple> {
    let (ontology, _) = registered("core:machine@1");
    parse_turtle(&ontology.expect("core:machine@1 carries an ontology")).expect("machine-v1 parses")
}

fn iri_of(term: &Term) -> Option<&str> {
    match term {
        Term::NamedNode(node) => Some(node.as_str()),
        _ => None,
    }
}

fn subject_iri(triple: &Triple) -> Option<&str> {
    match &triple.subject {
        NamedOrBlankNode::NamedNode(node) => Some(node.as_str()),
        _ => None,
    }
}

fn objects<'a>(triples: &'a [Triple], subject: &str, predicate: &str) -> Vec<&'a Term> {
    triples
        .iter()
        .filter(|t| subject_iri(t) == Some(subject) && t.predicate.as_str() == predicate)
        .map(|t| &t.object)
        .collect()
}

fn literal_of(term: &Term) -> String {
    match term {
        Term::Literal(literal) => literal.value().to_owned(),
        other => panic!("expected a literal, found {other:?}"),
    }
}

fn has_triple(triples: &[Triple], subject: &str, predicate: &str, object: &str) -> bool {
    triples.contains(&Triple::new(
        NamedNode::new(subject).unwrap(),
        NamedNode::new(predicate).unwrap(),
        NamedNode::new(object).unwrap(),
    ))
}

/// The class declarations (`a owl:Class`) of every registered core ontology document.
fn declarations_of(class_iri: &str) -> usize {
    let sources = GraphSchemaSources::default();
    sources
        .ontologies()
        .map(|(_, document)| {
            parse_turtle(document)
                .expect("a core ontology parses")
                .iter()
                .filter(|t| {
                    subject_iri(t) == Some(class_iri)
                        && t.predicate.as_str() == RDF_TYPE
                        && iri_of(&t.object) == Some(OWL_CLASS)
                })
                .count()
        })
        .sum()
}

// spec: MEDULLA-MACHINE-R001.1
#[test]
fn machine_class_is_declared_once_and_registered() {
    let (ontology, _) = registered("core:machine@1");
    assert!(ontology.is_some());
    let (_, shapes) = registered("core:machine-shapes@1");
    assert!(shapes
        .expect("machine-shapes carries shapes")
        .contains(":MachineShape"));
    let triples = machine_ontology();
    let machine = core("Machine");
    assert_eq!(
        declarations_of(&machine),
        1,
        "core:Machine is declared exactly once"
    );
    assert!(has_triple(&triples, &machine, SUBCLASS_OF, BFO_CONTINUANT));
    let labels = objects(&triples, &machine, RDFS_LABEL);
    assert_eq!(labels.len(), 1);
    assert_eq!(literal_of(labels[0]), "Machine");
    let comments = objects(&triples, &machine, RDFS_COMMENT);
    assert_eq!(comments.len(), 1);
    assert!(literal_of(comments[0]).contains("physical or virtual computing machine"));
    for part in [
        "MachineKey",
        "ProbeSnapshot",
        "MachineLifecycleTransitionRecord",
    ] {
        assert_eq!(declarations_of(&core(part)), 1, "{part} is declared once");
    }
}

// spec: MEDULLA-MACHINE-R001
#[test]
fn machine_is_the_only_machine_class() {
    // No other class is named Machine, and no retired host or part class is declared by any
    // registered core document.
    assert_eq!(declarations_of(&core("Machine")), 1);
    assert_eq!(declarations_of(&format!("{INFRA}Machine")), 0);
    for iri in RETIRED_IRIS {
        assert_eq!(declarations_of(iri), 0, "{iri} is still declared");
    }
    let sources = GraphSchemaSources::default();
    for (id, document) in sources.ontologies() {
        for triple in parse_turtle(document).expect("a core ontology parses") {
            let terms = [
                Some(triple.predicate.as_str()),
                subject_iri(&triple),
                iri_of(&triple.object),
            ];
            for iri in terms.into_iter().flatten() {
                assert!(!RETIRED_IRIS.contains(&iri), "{id} still names {iri}");
            }
        }
    }
}

fn collect_files(dir: &Path, extensions: &[&str], out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !name.starts_with("target") {
                collect_files(&path, extensions, out);
            }
        } else if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| extensions.contains(&ext))
        {
            out.push(path);
        }
    }
}

/// Whether `token` occurs in `text` not followed by an identifier character.
fn mentions(text: &str, token: &str) -> bool {
    text.match_indices(token).any(|(at, _)| {
        text[at + token.len()..]
            .chars()
            .next()
            .is_none_or(|next| !(next.is_alphanumeric() || next == '_'))
    })
}

fn crate_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(name)
}

// spec: MEDULLA-MACHINE-R001.2
// spec: MEDULLA-MACHINE-R003.3
#[test]
fn no_retired_machine_class_remains() {
    let this_file = Path::new(file!()).file_name().unwrap().to_owned();
    let mut ttl = Vec::new();
    collect_files(&crate_dir("medulla-core"), &["ttl"], &mut ttl);
    collect_files(&crate_dir("medulla-rdf").join("tests"), &["ttl"], &mut ttl);
    let mut text_files = Vec::new();
    collect_files(
        &crate_dir("medulla-core"),
        &["ttl", "rs", "sparql"],
        &mut text_files,
    );
    collect_files(
        &crate_dir("medulla-rdf").join("tests"),
        &["ttl", "rs", "sparql"],
        &mut text_files,
    );
    assert!(
        ttl.len() > 40,
        "the scan reached only {} Turtle files",
        ttl.len()
    );
    let mut offenders = BTreeSet::new();
    let mut templates = Vec::new();
    for path in &ttl {
        let text = std::fs::read_to_string(path).expect("a Turtle file reads");
        let triples = match parse_turtle(&text) {
            Ok(triples) => triples,
            // A fixture template carries a placeholder where an IRI or an object belongs. It
            // cannot be read as Turtle; the text scan below still covers it.
            Err(_) => {
                templates.push(path.display().to_string());
                continue;
            }
        };
        for triple in triples {
            let terms = [
                Some(triple.predicate.as_str()),
                subject_iri(&triple),
                iri_of(&triple.object),
            ];
            if terms
                .into_iter()
                .flatten()
                .any(|iri| RETIRED_IRIS.contains(&iri))
            {
                offenders.insert(path.display().to_string());
            }
        }
    }
    for path in &text_files {
        if path.file_name() == Some(&this_file) {
            continue;
        }
        let text = std::fs::read_to_string(path).unwrap_or_default();
        if RETIRED_TOKENS.iter().any(|token| mentions(&text, token)) {
            offenders.insert(path.display().to_string());
        }
    }
    assert!(
        templates.len() <= 8,
        "too many Turtle files do not parse: {templates:?}"
    );
    assert!(
        offenders.is_empty(),
        "retired machine classes remain in {offenders:?}"
    );
}

const BANNED_PRODUCTS: [&str; 5] = [
    "Home-Assistant",
    "tunnel-manager",
    "Kubernetes",
    "Swarm",
    "Docker",
];

/// The comment of every class a document declares, by class IRI.
fn class_comments(triples: &[Triple]) -> Vec<(String, String)> {
    triples
        .iter()
        .filter(|t| t.predicate.as_str() == RDF_TYPE && iri_of(&t.object) == Some(OWL_CLASS))
        .filter_map(subject_iri_owned)
        .flat_map(|class| {
            objects(triples, &class, RDFS_COMMENT)
                .into_iter()
                .map(|comment| (class.clone(), literal_of(comment)))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn subject_iri_owned(triple: &Triple) -> Option<String> {
    subject_iri(triple).map(str::to_owned)
}

fn comment_of(triples: &[Triple], iri: &str) -> String {
    let comments = objects(triples, iri, RDFS_COMMENT);
    assert_eq!(comments.len(), 1, "{iri} has one comment");
    literal_of(comments[0])
}

// spec: MEDULLA-MACHINE-R001.3
#[test]
fn server_is_a_software_server_and_core_comments_name_no_product() {
    let sources = GraphSchemaSources::default();
    let foundation = parse_turtle(
        sources
            .core
            .get("core:foundation@1")
            .unwrap()
            .ontology_ttl
            .as_deref()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        comment_of(&foundation, &core("Server")),
        "A running software server that offers a service. It runs on a :Machine."
    );
    let server_parents: Vec<&Term> = objects(&foundation, &core("Server"), SUBCLASS_OF);
    assert!(server_parents
        .iter()
        .any(|p| iri_of(p) == Some(BFO_CONTINUANT)));
    assert!(
        server_parents
            .iter()
            .any(|p| matches!(p, Term::BlankNode(_))),
        "the locatedIn restriction stays"
    );
    let infra = parse_turtle(
        sources
            .core
            .get("core:infrastructure@1")
            .unwrap()
            .ontology_ttl
            .as_deref()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        comment_of(&infra, &format!("{INFRA}Device")),
        "A physical or logical device that groups one or more :Entity states."
    );
    let node = comment_of(&infra, &format!("{INFRA}Node"));
    assert!(node.contains("a cluster member"), "{node}");
    for (document, triples) in [
        ("infrastructure-v1.ttl", infra),
        ("machine-v1.ttl", machine_ontology()),
    ] {
        for (class, comment) in class_comments(&triples) {
            for product in BANNED_PRODUCTS {
                assert!(
                    !comment.contains(product),
                    "{document}: the comment of {class} names {product}"
                );
            }
        }
    }
}

const PREFIXES: &str = "@prefix : <https://knuckles-team.github.io/ontology/core#> .\n\
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n";
const MACHINE: &str = "<urn:machine:serial:f3af1defd63343f5dd2ee44b56cb636d>";
const ANCHOR: &str = "<urn:machine-key:host:41097ff758a2bcf28635c4da73d9fa18>";
const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn validate(data: &str) -> medulla_shacl::ValidationReport {
    medulla_shacl::validate_turtle(MACHINE_SHAPES, &format!("{PREFIXES}{data}"))
        .expect("the fixture parses and validates")
}

fn assert_conforms(data: &str) {
    let report = validate(data);
    assert!(
        report.conforms,
        "expected a conforming fixture: {:?}",
        report.results
    );
}

/// The fixture violates the shape, and some result message contains `fragment`.
fn assert_refused(data: &str, fragment: &str) {
    let report = validate(data);
    assert!(
        !report.conforms,
        "expected a violation of '{fragment}' in:\n{data}"
    );
    let messages: Vec<String> = report
        .results
        .iter()
        .filter_map(|r| r.message.clone())
        .collect();
    assert!(
        messages.iter().any(|m| m.contains(fragment)),
        "no result mentions '{fragment}': {messages:?}"
    );
}

fn machine_node(extra: &str) -> String {
    format!(
        "{MACHINE} a :Machine ; :machineName \"alpha\" ; :identityTier \"serial\" ;\n\
         :machineKey \"serial:ABC 123\" ; :machineLifecycleState \"active\" {extra}.\n"
    )
}

// spec: MEDULLA-MACHINE-R002.3
#[test]
fn machine_shape_accepts_a_valid_machine_and_refuses_each_violation() {
    assert_conforms(&machine_node(""));
    assert_conforms(&machine_node(
        "; :systemUuid \"123e4567-e89b-12d3-a456-426614174000\" ; :machineKey \"host:alpha\" ",
    ));
    // A connector view of a machine carries no identityTier and no shape targets it.
    assert_conforms("<urn:view:ssh:alpha> a :Machine ; :machineName \"alpha\" .\n");
    let missing_name = format!("{MACHINE} a :Machine ; :identityTier \"serial\" ; :machineKey \"serial:A\" ; :machineLifecycleState \"active\" .");
    assert_refused(&missing_name, "exactly one non-empty :machineName");
    assert_refused(
        &machine_node("; :machineName \"beta\" "),
        "exactly one non-empty :machineName",
    );
    let empty_name = machine_node("").replace("\"alpha\"", "\"\"");
    assert_refused(&empty_name, "exactly one non-empty :machineName");
    assert_refused(
        &machine_node("").replace("\"serial\" ;\n", "\"cpu\" ;\n"),
        ":identityTier",
    );
    assert_refused(&machine_node("; :identityTier \"uuid\" "), ":identityTier");
    assert_refused(
        &machine_node("").replace(":machineKey \"serial:ABC 123\" ;", ""),
        "at least one :machineKey",
    );
    assert_refused(
        &machine_node("").replace("\"serial:ABC 123\"", "\"ABC 123\""),
        "at least one :machineKey",
    );
    assert_refused(
        &machine_node("; :systemUuid \"a\" , \"b\" "),
        "at most one :systemUuid",
    );
    assert_refused(
        &machine_node("").replace("\"active\"", "\"asleep\""),
        ":machineLifecycleState",
    );
    assert_refused(
        &machine_node("").replace(":machineLifecycleState \"active\" ", ""),
        ":machineLifecycleState",
    );
    let bad_iri = machine_node("").replace(MACHINE, "<urn:machine:serial:not-hex>");
    assert_refused(&bad_iri, "urn:machine:<tier>");
    let wrong_scheme = machine_node("").replace(MACHINE, "<https://example.org/machine/alpha>");
    assert_refused(&wrong_scheme, "urn:machine:<tier>");
}

/// The sub-property of `:hasComponent` a machine uses to own a part of `class`.
fn owner_property(class: &str) -> &'static str {
    match class {
        "CentralProcessor" => "hasProcessor",
        "MemoryModule" => "hasMemoryModule",
        "StorageDevice" => "hasStorageDevice",
        "NetworkInterface" => "hasNetworkInterface",
        "NetworkBond" => "hasNetworkBond",
        "Accelerator" => "hasAccelerator",
        "ManagementController" => "hasManagementController",
        other => panic!("{other} is not a part class"),
    }
}

fn owner_triple(class: &str) -> String {
    format!(
        "{MACHINE} :{} <urn:machine-part:{class}> .\n",
        owner_property(class)
    )
}

fn part(class: &str, props: &str) -> String {
    format!(
        "<urn:machine-part:{class}> a :{class} ; {props} .\n{}",
        owner_triple(class)
    )
}

const PART_FIXTURES: [(&str, &str); 7] = [
    ("CentralProcessor", ":socketIndex 0 ; :coreCount 8 ; :threadCount 16"),
    ("MemoryModule", ":componentSlot \"DIMM_A1\" ; :capacityBytes 17179869184"),
    ("StorageDevice", ":capacityBytes 1000204886016 ; :storageMedium \"nvme\" ; :deviceName \"nvme0n1\""),
    ("NetworkInterface", ":interfaceName \"eth0\" ; :macAddress \"aa:bb:cc:dd:ee:01\" ; :linkState \"up\" ; :speedMegabits 10000"),
    ("NetworkBond", ":bondName \"bond0\" ; :bondMode \"802.3ad\" ; :bondMember <urn:machine-part:NetworkInterface>"),
    ("Accelerator", ":acceleratorKind \"gpu\" ; :pciAddress \"0000:01:00.0\""),
    ("ManagementController", ":controllerId \"aa:bb:cc:dd:ee:02\" ; :firmwareVersion \"2.1\""),
];

// spec: MEDULLA-MACHINE-R003
#[test]
fn machine_with_all_seven_part_kinds_validates() {
    let mut data = machine_node("");
    for (class, props) in PART_FIXTURES {
        data.push_str(&part(class, props));
    }
    assert_conforms(&data);
    let triples = parse_turtle(&format!("{PREFIXES}{data}")).unwrap();
    let owners: Vec<String> = PART_FIXTURES
        .iter()
        .map(|(class, _)| core(owner_property(class)))
        .collect();
    let components = triples
        .iter()
        .filter(|t| owners.iter().any(|o| t.predicate.as_str() == o))
        .count();
    assert_eq!(components, 7);
}

// spec: MEDULLA-MACHINE-R003.2
#[test]
fn part_shapes_accept_valid_parts_and_refuse_each_violation() {
    for (class, props) in PART_FIXTURES {
        let mut data = machine_node("");
        if class == "NetworkBond" {
            data.push_str(&part("NetworkInterface", PART_FIXTURES[3].1));
        }
        data.push_str(&part(class, props));
        assert_conforms(&data);
    }
    let broken: [(&str, &str, &str); 14] = [
        (
            "CentralProcessor",
            ":socketIndex 0 ; :coreCount 8",
            "exactly one :threadCount",
        ),
        (
            "CentralProcessor",
            ":socketIndex 0 , 1 ; :coreCount 8 ; :threadCount 16",
            "exactly one :socketIndex",
        ),
        ("MemoryModule", ":capacityBytes 0", "at least 1"),
        (
            "MemoryModule",
            ":componentSlot \"A1\"",
            "exactly one :capacityBytes",
        ),
        (
            "StorageDevice",
            ":capacityBytes 10 ; :storageMedium \"tape\"",
            "hdd, ssd, nvme or other",
        ),
        (
            "StorageDevice",
            ":storageMedium \"ssd\"",
            "exactly one :capacityBytes",
        ),
        (
            "NetworkInterface",
            ":interfaceName \"eth0\" ; :linkState \"sideways\"",
            "up, down or unknown",
        ),
        (
            "NetworkInterface",
            ":interfaceName \"eth0\" ; :linkState \"up\" ; :macAddress \"AA:BB:CC:DD:EE:01\"",
            "lower-case colon form",
        ),
        (
            "NetworkInterface",
            ":linkState \"up\"",
            "exactly one :interfaceName",
        ),
        (
            "NetworkBond",
            ":bondName \"bond0\"",
            "at least one :bondMember",
        ),
        (
            "NetworkBond",
            ":bondMember <urn:machine-part:NetworkInterface>",
            "exactly one :bondName",
        ),
        (
            "Accelerator",
            ":acceleratorKind \"quantum\"",
            "gpu, fpga, tpu, dpu or other",
        ),
        (
            "ManagementController",
            ":controllerId \"aabbccddee02\"",
            "lower-case colon form",
        ),
        (
            "ManagementController",
            ":firmwareVersion \"1\"",
            "exactly one :controllerId",
        ),
    ];
    for (class, props, fragment) in broken {
        let mut data = machine_node("");
        if class != "NetworkInterface" {
            data.push_str(&part("NetworkInterface", PART_FIXTURES[3].1));
        }
        data.push_str(&part(class, props).replace(&owner_triple(class), ""));
        data.push_str(&owner_triple(class));
        assert_refused(&data, fragment);
    }
    // A part with no owner, and a part with two owners.
    let orphan = format!(
        "{}<urn:machine-part:x> a :Accelerator ; :acceleratorKind \"gpu\" .\n",
        machine_node("")
    );
    assert_refused(&orphan, "this part has none");
    let other = "<urn:machine:serial:106926c48469ff7da8c08bc3b320aeb8>";
    let shared = format!(
        "{}{other} a :Machine ; :machineName \"beta\" ; :identityTier \"serial\" ; :machineKey \"serial:SN-0001\" ; :machineLifecycleState \"active\" ; :hasAccelerator <urn:machine-part:Accelerator> .\n",
        part("Accelerator", ":acceleratorKind \"gpu\"")
    );
    assert_refused(
        &format!("{}{shared}", machine_node("")),
        "this part has several",
    );
}

// spec: MEDULLA-MACHINE-R003.1
#[test]
fn every_part_term_is_declared_with_label_comment_and_range() {
    let triples = machine_ontology();
    let classes = [
        "MachineComponent",
        "CentralProcessor",
        "MemoryModule",
        "StorageDevice",
        "NetworkInterface",
        "NetworkBond",
        "Accelerator",
        "ManagementController",
    ];
    for class in classes {
        let iri = core(class);
        assert_eq!(declarations_of(&iri), 1, "{class} is declared once");
        assert!(
            !objects(&triples, &iri, RDFS_LABEL).is_empty(),
            "{class} has a label"
        );
        assert!(
            !objects(&triples, &iri, RDFS_COMMENT).is_empty(),
            "{class} has a comment"
        );
        let parent = if class == "MachineComponent" {
            "http://purl.obolibrary.org/obo/BFO_0000030".to_string()
        } else {
            core("MachineComponent")
        };
        assert!(
            has_triple(&triples, &iri, SUBCLASS_OF, &parent),
            "{class} parent"
        );
    }
    // (term, domain, range): a domain only where one class owns the term.
    let properties: [(&str, Option<&str>, &str); 19] = [
        ("hasComponent", Some("Machine"), "MachineComponent"),
        ("hasProcessor", Some("Machine"), "CentralProcessor"),
        ("hasMemoryModule", Some("Machine"), "MemoryModule"),
        ("hasStorageDevice", Some("Machine"), "StorageDevice"),
        ("hasNetworkInterface", Some("Machine"), "NetworkInterface"),
        ("hasNetworkBond", Some("Machine"), "NetworkBond"),
        ("hasAccelerator", Some("Machine"), "Accelerator"),
        (
            "hasManagementController",
            Some("Machine"),
            "ManagementController",
        ),
        ("bondMember", Some("NetworkBond"), "NetworkInterface"),
        (
            "socketIndex",
            Some("CentralProcessor"),
            "xsd:nonNegativeInteger",
        ),
        ("capacityBytes", None, "xsd:nonNegativeInteger"),
        ("model", None, "xsd:string"),
        ("serialNumber", None, "xsd:string"),
        ("macAddress", Some("NetworkInterface"), "xsd:string"),
        ("storageMedium", Some("StorageDevice"), "xsd:string"),
        ("acceleratorKind", Some("Accelerator"), "xsd:string"),
        ("controllerId", Some("ManagementController"), "xsd:string"),
        ("bondName", Some("NetworkBond"), "xsd:string"),
        (
            "speedMegabits",
            Some("NetworkInterface"),
            "xsd:nonNegativeInteger",
        ),
    ];
    for (term, domain, range) in properties {
        let iri = core(term);
        assert!(
            !objects(&triples, &iri, RDFS_LABEL).is_empty(),
            "{term} has a label"
        );
        assert!(
            !objects(&triples, &iri, RDFS_COMMENT).is_empty(),
            "{term} has a comment"
        );
        let domains: Vec<&str> = objects(&triples, &iri, RDFS_DOMAIN)
            .into_iter()
            .filter_map(iri_of)
            .collect();
        match domain {
            Some(class) => assert_eq!(domains, [core(class).as_str()], "{term} domain"),
            None => assert!(domains.is_empty(), "{term} carries no domain"),
        }
        let want = match range.strip_prefix("xsd:") {
            Some(local) => format!("http://www.w3.org/2001/XMLSchema#{local}"),
            None => core(range),
        };
        let ranges: Vec<&str> = objects(&triples, &iri, RDFS_RANGE)
            .into_iter()
            .filter_map(iri_of)
            .collect();
        assert_eq!(ranges, [want.as_str()], "{term} range");
    }
    // Each part kind has its own owner property, a sub-property of `:hasComponent`.
    for (class, _) in PART_FIXTURES {
        assert!(
            has_triple(
                &triples,
                &core(owner_property(class)),
                "http://www.w3.org/2000/01/rdf-schema#subPropertyOf",
                &core("hasComponent")
            ),
            "{class} owner property is a sub-property of hasComponent"
        );
    }
    // Every datatype and object property of the module has a label, a comment and a range.
    let declared: BTreeSet<String> = triples
        .iter()
        .filter(|t| t.predicate.as_str() == RDF_TYPE)
        .filter(|t| matches!(iri_of(&t.object), Some(o) if o.ends_with("#DatatypeProperty") || o.ends_with("#ObjectProperty")))
        .filter_map(subject_iri_owned)
        .collect();
    assert!(
        declared.len() >= 55,
        "{} properties declared",
        declared.len()
    );
    for iri in declared {
        assert_eq!(objects(&triples, &iri, RDFS_LABEL).len(), 1, "{iri} label");
        assert_eq!(
            objects(&triples, &iri, RDFS_COMMENT).len(),
            1,
            "{iri} comment"
        );
        assert_eq!(objects(&triples, &iri, RDFS_RANGE).len(), 1, "{iri} range");
    }
}

// spec: MEDULLA-MACHINE-R004.1
#[test]
fn observes_accepts_a_machine_and_refuses_any_other_target() {
    let triples = machine_ontology();
    let observes = core("observes");
    assert!(
        objects(&triples, &observes, RDFS_DOMAIN).is_empty(),
        ":observes has no domain"
    );
    let range = objects(&triples, &observes, RDFS_RANGE);
    assert_eq!(range.len(), 1);
    assert!(
        matches!(range[0], Term::BlankNode(_)),
        "the range is a union class"
    );
    let anchor =
        format!("{ANCHOR} a :MachineKey ; :machineKey \"host:alpha\" ; :identifies {MACHINE} .\n");
    let base = format!("{}{anchor}", machine_node(""));
    assert_conforms(&format!(
        "{base}<urn:rec:ssh:alpha> :observes {MACHINE} .\n"
    ));
    assert_conforms(&format!("{base}<urn:rec:ssh:alpha> :observes {ANCHOR} .\n"));
    assert_conforms(&format!(
        "{base}<urn:rec:k8s:alpha> :observes {ANCHOR} , {MACHINE} .\n"
    ));
    let refused = "Every value of :observes is a :Machine or a :MachineKey";
    assert_refused(
        &format!("{base}<urn:rec:ssh:alpha> :observes <urn:somewhere:else> .\n"),
        refused,
    );
    assert_refused(
        &format!("{base}<urn:other> a :Server .\n<urn:rec:ssh:alpha> :observes <urn:other> .\n"),
        refused,
    );
    assert_refused(
        &format!("{base}<urn:rec:ssh:alpha> :observes {MACHINE} , <urn:other> .\n"),
        refused,
    );
}

fn probe(extra: &str) -> String {
    format!(
        "<urn:machine-probe:p1> a :ProbeSnapshot ; :probeKind \"system\" ; :connector \"shell\" ;\n\
         :tool \"inventory\" ; :probeDigest \"{DIGEST}\" ; :observedAt \"2026-11-27T10:00:00Z\"^^xsd:dateTime ;\n\
         :observes {MACHINE} {extra}.\n"
    )
}

// spec: MEDULLA-MACHINE-R004.2
#[test]
fn probe_snapshot_shape_requires_digest_and_exactly_one_machine() {
    let base = machine_node("");
    assert_conforms(&format!("{base}{}", probe("")));
    let broken: [(String, &str); 8] = [
        (
            probe("").replace(&format!("{DIGEST}\""), "sha256:abc\""),
            "sha256:<64 hex digits>",
        ),
        (
            probe("").replace(":probeDigest", ":tool"),
            "exactly one :probeDigest",
        ),
        (
            probe("; :probeDigest \"sha256:00\" "),
            "exactly one :probeDigest",
        ),
        (probe("").replace("^^xsd:dateTime", ""), "xsd:dateTime"),
        (probe("").replace("\"system\"", "\"gossip\""), ":probeKind"),
        (
            probe("").replace(" :connector \"shell\" ;", ""),
            "exactly one :connector",
        ),
        (
            probe("").replace(&format!(":observes {MACHINE}"), ""),
            "exactly one :Machine",
        ),
        (
            probe(&format!("; :observes {ANCHOR} ")),
            "exactly one :Machine",
        ),
    ];
    for (probe, fragment) in broken {
        let anchor = format!(
            "{ANCHOR} a :MachineKey ; :machineKey \"host:alpha\" ; :identifies {MACHINE} .\n"
        );
        assert_refused(&format!("{base}{anchor}{probe}"), fragment);
    }
    assert_refused(
        &format!(
            "{base}{}",
            probe("")
                .replace(MACHINE, "<urn:not-a-machine>")
                .replace("<urn:not-a-machine>", "<urn:elsewhere>")
        ),
        "exactly one :Machine",
    );
}

// spec: MEDULLA-MACHINE-R004.3
#[test]
fn machine_key_anchor_shape_requires_one_machine_and_a_matching_iri() {
    let base = machine_node("");
    let anchor = |body: &str| format!("{base}{body}");
    assert_conforms(&anchor(&format!(
        "{ANCHOR} a :MachineKey ; :machineKey \"host:alpha\" ; :identifies {MACHINE} .\n"
    )));
    let broken: [(String, &str); 6] = [
        (format!("{ANCHOR} a :MachineKey ; :machineKey \"host:alpha\" .\n"), "identifies exactly one :Machine"),
        (format!("{ANCHOR} a :MachineKey ; :machineKey \"host:alpha\" ; :identifies <urn:elsewhere> .\n"), "identifies exactly one :Machine"),
        (format!("{ANCHOR} a :MachineKey ; :machineKey \"host:alpha\" ; :identifies {MACHINE} , <urn:machine:host:00> .\n"), "identifies exactly one :Machine"),
        (format!("{ANCHOR} a :MachineKey ; :identifies {MACHINE} .\n"), "exactly one :machineKey"),
        (format!("{ANCHOR} a :MachineKey ; :machineKey \"alpha\" ; :identifies {MACHINE} .\n"), "exactly one :machineKey"),
        (format!("<urn:machine-key:host:short> a :MachineKey ; :machineKey \"host:alpha\" ; :identifies {MACHINE} .\n"), "urn:machine-key:<kind>"),
    ];
    for (body, fragment) in broken {
        assert_refused(&anchor(&body), fragment);
    }
}

const TRANSITION_IRI: &str = "<urn:machine-transition:t1>";

fn transition(extra: &str) -> String {
    format!(
        "{TRANSITION_IRI} a :MachineLifecycleTransitionRecord ; :previousState \"commissioned\" ; :nextState \"active\" ;\n\
         :appliedEdgeId \"H01\" ; :firedByEvent \"Ready\" ; :evidenceDigest \"{DIGEST}\" ;\n\
         :observedAt \"2026-11-27T10:00:00Z\"^^xsd:dateTime ; :transitionOf {MACHINE} {extra}.\n"
    )
}

// spec: MEDULLA-MACHINE-R005.5
#[test]
fn transition_record_shape_refuses_an_unknown_edge_and_a_missing_machine() {
    let base = machine_node("");
    assert_conforms(&format!("{base}{}", transition("")));
    let broken: [(String, &str); 9] = [
        (
            transition("").replace("\"H01\"", "\"H08\""),
            "from H01 to H07",
        ),
        (
            transition("").replace("\"H01\"", "\"X01\""),
            "from H01 to H07",
        ),
        (
            transition("").replace(&format!(":transitionOf {MACHINE}"), ""),
            "belongs to exactly one :Machine",
        ),
        (
            transition("").replace(MACHINE, "<urn:elsewhere>"),
            "belongs to exactly one :Machine",
        ),
        (
            transition("").replace("\"commissioned\"", "\"sleeping\""),
            ":previousState",
        ),
        (
            transition("").replace("\"active\" ;", "\"gone\" ;"),
            ":nextState",
        ),
        (
            transition("").replace("\"Ready\"", "\"Wake\""),
            "five events",
        ),
        (
            transition("").replace(&format!("\"{DIGEST}\""), "\"sha256:short\""),
            ":evidenceDigest",
        ),
        (transition("").replace("^^xsd:dateTime", ""), "xsd:dateTime"),
    ];
    for (record, fragment) in broken {
        assert_refused(&format!("{base}{record}"), fragment);
    }
}

const RECORDS_FIXTURE: &str = include_str!("../../../tests/fixtures/machine/records.json");

/// The stored form of a record set: one typed node per entity, one triple per relationship, one
/// triple per property value.
fn stored_graph(set: &medulla_source_types::machine::records::InventoryRecordSet) -> String {
    use medulla_source_types::machine::records::PropertyValue;
    let quote = |text: &str| format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""));
    let mut out = String::new();
    for entity in &set.entities {
        out.push_str(&format!("<{}> a :{} .\n", entity.id, entity.node_type));
        for (name, value) in &entity.properties {
            let values: Vec<String> = match value {
                PropertyValue::Text(text) if name == "observedAt" => {
                    vec![format!("{}^^xsd:dateTime", quote(text))]
                }
                PropertyValue::Text(text) => vec![quote(text)],
                PropertyValue::Integer(number) => vec![number.to_string()],
                PropertyValue::Boolean(flag) => vec![flag.to_string()],
                PropertyValue::List(items) => items.iter().map(|item| quote(item)).collect(),
            };
            for value in values {
                out.push_str(&format!("<{}> :{name} {value} .\n", entity.id));
            }
        }
    }
    for relationship in &set.relationships {
        out.push_str(&format!(
            "<{}> :{} <{}> .\n",
            relationship.source, relationship.relationship, relationship.target
        ));
    }
    out
}

// spec: MEDULLA-MACHINE-R006.4
#[test]
fn the_graph_after_ingest_conforms_to_the_machine_shapes() {
    let set: medulla_source_types::machine::records::InventoryRecordSet =
        serde_json::from_str(RECORDS_FIXTURE).expect("the fixture record set parses");
    // The deterministic normalizer accepts it, and the engine shapes accept what it produced.
    set.check_shapes()
        .expect("the fixture passes the Rust check");
    assert_conforms(&stored_graph(&set));

    // The Rust check and the SHACL shapes refuse the same broken records.
    type Mutation = fn(&mut medulla_source_types::machine::records::InventoryRecordSet);
    let mutations: [(&str, Mutation, &str); 3] = [
        (
            "an unknown lifecycle state",
            |s| {
                let machine = s
                    .entities
                    .iter_mut()
                    .find(|e| e.node_type == "Machine")
                    .unwrap();
                machine.properties.insert(
                    "machineLifecycleState".into(),
                    medulla_source_types::machine::records::PropertyValue::Text("asleep".into()),
                );
            },
            ":machineLifecycleState",
        ),
        (
            "a part with no owner",
            |s| {
                s.relationships
                    .retain(|r| r.relationship != "hasAccelerator");
            },
            "this part has none",
        ),
        (
            "a part with two owners",
            |s| {
                let owner = s
                    .relationships
                    .iter()
                    .find(|r| r.relationship == "hasProcessor")
                    .unwrap()
                    .clone();
                let mut second = owner.clone();
                second.source = "urn:machine:host:c9faaf568305e0a90e726ca4b4807c73".into();
                s.relationships.push(second);
            },
            "this part has several",
        ),
    ];
    for (label, mutate, fragment) in mutations {
        let mut broken = set.clone();
        mutate(&mut broken);
        let mut data = stored_graph(&broken);
        if label == "a part with two owners" {
            data.push_str(
                &machine_node("")
                    .replace(
                        MACHINE,
                        "<urn:machine:host:c9faaf568305e0a90e726ca4b4807c73>",
                    )
                    .replace("serial", "host")
                    .replace("serial:ABC 123", "host:beta"),
            );
        }
        assert_refused(&data, fragment);
        assert!(
            broken.check_shapes().is_err(),
            "{label}: the Rust check refuses it too"
        );
    }
}
