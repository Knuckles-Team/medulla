//! The metadata-catalog core vocabulary: catalogued assets, owners and stewards,
//! lineage, the glossary and system-prompt candidates, aligned to W3C DCAT,
//! PROV-O, ORG and SKOS (MEDULLA-TYPED-PACKS-R106.1, MEDULLA-DECISION-ENGINE-R814.1.1).
#![cfg(feature = "owl")]

mod common;

use common::{has_iri, parse_all, BFO, KG, RDFS};
use medulla_rdf::oxrdf::Triple;
use medulla_reasoner::owl::Reasoner;

const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const OWL: &str = "http://www.w3.org/2002/07/owl#";
const CAT: &str = "https://knuckles-team.github.io/ontology/catalog#";
const CP: &str = "https://knuckles-team.github.io/ontology/connector-pack#";
const METADATA_CATALOG: &str = "https://knuckles-team.github.io/ontology/metadata_catalog";
const DCAT: &str = "http://www.w3.org/ns/dcat#";
const PROV: &str = "http://www.w3.org/ns/prov#";
const ORG: &str = "http://www.w3.org/ns/org#";
const SKOS: &str = "http://www.w3.org/2004/02/skos/core#";
const SHAPES: &str = include_str!("../../medulla-core/ontology/metadata_catalog-v1.shapes.ttl");

fn documents() -> Vec<Triple> {
    parse_all(&[
        include_str!("../../medulla-core/ontology/core-foundation-v1.ttl"),
        include_str!("../../medulla-core/ontology/capability-v1.ttl"),
        include_str!("../../medulla-core/ontology/company-v1.ttl"),
        include_str!("../../medulla-core/ontology/software-v1.ttl"),
        include_str!("../../medulla-core/ontology/sdlc_lifecycle-v1.ttl"),
        include_str!("../../medulla-core/ontology/infrastructure-v1.ttl"),
        include_str!("../../medulla-core/ontology/trm-v1.ttl"),
        include_str!("../../medulla-core/ontology/orchestration-v1.ttl"),
        include_str!("../../medulla-core/ontology/archimate-v1.ttl"),
        include_str!("../../medulla-core/ontology/sdd-v1.ttl"),
        include_str!("../../medulla-core/ontology/service_operations-v1.ttl"),
        include_str!("../../medulla-core/ontology/metadata_catalog-v1.ttl"),
    ])
}

// spec: MEDULLA-TYPED-PACKS-R106.1
// spec: MEDULLA-DECISION-ENGINE-R814.1
// spec: MEDULLA-DECISION-ENGINE-R814.1.1
#[test]
fn catalog_ontology_declares_root_and_cross_domain_imports() {
    let triples = documents();
    assert!(has_iri(
        &triples,
        METADATA_CATALOG,
        &format!("{RDF}type"),
        &format!("{OWL}Ontology")
    ));
    // Core is the root the classes extend, never an import
    // (MEDULLA-TYPED-PACKS-R071.2).
    assert!(!has_iri(
        &triples,
        METADATA_CATALOG,
        &format!("{OWL}imports"),
        "https://knuckles-team.github.io/ontology/core"
    ));
    for imported in [
        "https://knuckles-team.github.io/ontology/capability",
        "https://knuckles-team.github.io/ontology/orchestration",
        "https://knuckles-team.github.io/ontology/service_operations",
    ] {
        assert!(has_iri(
            &triples,
            METADATA_CATALOG,
            &format!("{OWL}imports"),
            imported
        ));
    }
}

// spec: MEDULLA-TYPED-PACKS-R106.1
// spec: MEDULLA-DECISION-ENGINE-R814.1.1
#[test]
fn catalog_classes_roll_up_to_eg_core_and_align_to_w3c() {
    let triples = documents();
    let owl_class = format!("{OWL}Class");
    let sub = format!("{RDFS}subClassOf");
    let expected: [(String, &[String]); 18] = [
        (
            format!("{CAT}Catalog"),
            &[format!("{CAT}Asset"), format!("{DCAT}Catalog")],
        ),
        (
            format!("{CAT}Asset"),
            &[format!("{BFO}0000031"), format!("{DCAT}Resource")],
        ),
        (
            format!("{CAT}DataResource"),
            &[
                format!("{CAT}Asset"),
                format!("{KG}Dataset"),
                format!("{DCAT}Dataset"),
            ],
        ),
        (format!("{CAT}Field"), &[format!("{CAT}Asset")]),
        (format!("{CAT}ToolAsset"), &[format!("{CAT}Asset")]),
        (
            format!("{CAT}PromptAsset"),
            &[format!("{CAT}Asset"), format!("{KG}Prompt")],
        ),
        (
            format!("{KG}PromptCandidate"),
            &[format!("{CAT}PromptAsset")],
        ),
        (
            format!("{CAT}SkillAsset"),
            &[format!("{CAT}Asset"), format!("{KG}Document")],
        ),
        (
            format!("{CAT}AgentAsset"),
            &[format!("{CAT}Asset"), format!("{KG}Document")],
        ),
        (
            format!("{CP}Connector"),
            &[format!("{KG}Tool"), format!("{DCAT}DataService")],
        ),
        (
            format!("{CAT}StewardshipAssignment"),
            &[format!("{BFO}0000031"), format!("{PROV}Attribution")],
        ),
        (
            format!("{CAT}ChangeRequest"),
            &[format!("{BFO}0000031"), format!("{PROV}Activity")],
        ),
        (
            format!("{CAT}Certification"),
            &[format!("{BFO}0000031"), format!("{PROV}Attribution")],
        ),
        (
            format!("{CAT}StewardshipRole"),
            &[format!("{KG}Role"), format!("{ORG}Role")],
        ),
        (
            format!("{CAT}LineageEdge"),
            &[format!("{BFO}0000031"), format!("{PROV}Derivation")],
        ),
        (
            format!("{CAT}IngestRun"),
            &[format!("{BFO}0000015"), format!("{PROV}Activity")],
        ),
        (
            format!("{CAT}Glossary"),
            &[format!("{BFO}0000031"), format!("{SKOS}ConceptScheme")],
        ),
        (
            format!("{CAT}CatalogGlossaryTerm"),
            &[format!("{KG}Concept"), format!("{SKOS}Concept")],
        ),
    ];
    for (class, parents) in &expected {
        assert!(
            has_iri(&triples, class, &format!("{RDF}type"), &owl_class),
            "{class}"
        );
        for parent in *parents {
            assert!(
                has_iri(&triples, class, &sub, parent),
                "{class} under {parent}"
            );
        }
    }
    assert!(has_iri(
        &triples,
        &format!("{KG}AssignmentGroup"),
        &sub,
        &format!("{ORG}OrganizationalUnit")
    ));

    let classification = Reasoner::from_triples(&triples).classify();
    assert!(classification.consistent);
    assert!(
        classification.unsatisfiable.is_empty(),
        "unsatisfiable: {:?}",
        classification.unsatisfiable
    );
    // Every catalog class reaches the BFO root through EG core, never through a
    // W3C parent alone (packs and core modules roll up to EG core).
    let entity = format!("<{BFO}0000001>");
    for (class, _) in &expected {
        let subsumers = &classification.subsumers[&format!("<{class}>")];
        assert!(
            subsumers.contains(&entity),
            "{class} does not reach BFO: {subsumers:?}"
        );
    }
}

// spec: MEDULLA-TYPED-PACKS-R106.1
#[test]
fn owner_steward_and_lineage_properties_specialise_prov() {
    let triples = documents();
    let sub = format!("{RDFS}subPropertyOf");
    let expected = [
        (format!("{CAT}hasOwner"), format!("{KG}ownedBy")),
        (format!("{CAT}hasOwner"), format!("{PROV}wasAttributedTo")),
        (format!("{CAT}hasSteward"), format!("{PROV}wasAttributedTo")),
        (format!("{CP}declaredOwner"), format!("{KG}ownedBy")),
        (format!("{CAT}principal"), format!("{PROV}agent")),
        (format!("{CAT}role"), format!("{PROV}hadRole")),
        (format!("{KG}derivedFrom"), format!("{PROV}wasDerivedFrom")),
        (format!("{CAT}fromSource"), format!("{KG}derivedFrom")),
        (format!("{KG}memberOfGroup"), format!("{ORG}memberOf")),
    ];
    for (property, parent) in expected {
        assert!(
            has_iri(&triples, &property, &sub, &parent),
            "{property} under {parent}"
        );
    }
    assert!(has_iri(
        &triples,
        &format!("{CAT}lineageTo"),
        &format!("{OWL}inverseOf"),
        &format!("{KG}derivedFrom")
    ));
    for role in ["owner", "steward", "custodian"] {
        assert!(has_iri(
            &triples,
            &format!("{CAT}{role}"),
            &format!("{RDF}type"),
            &format!("{CAT}StewardshipRole")
        ));
    }
}

const PREFIXES: &str = "@prefix : <https://knuckles-team.github.io/ontology/core#> . \
     @prefix cat: <https://knuckles-team.github.io/ontology/catalog#> . \
     @prefix cp: <https://knuckles-team.github.io/ontology/connector-pack#> . \
     @prefix skos: <http://www.w3.org/2004/02/skos/core#> . ";

fn conforms(data: &str) -> bool {
    medulla_shacl::validate_turtle(SHAPES, &format!("{PREFIXES}{data}"))
        .unwrap()
        .conforms
}

const DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";

// spec: MEDULLA-TYPED-PACKS-R106.1
// spec: MEDULLA-DECISION-ENGINE-R814.1.1
#[test]
fn catalog_shapes_accept_a_complete_catalog() {
    let data = format!(
        "<urn:c:sn> a cp:Connector . \
         <urn:a:incidents> a cat:DataResource ; cp:ofConnector <urn:c:sn> ; cp:digest \"{DIGEST}\" ; cp:servesClass :Incident ; \
             cat:hasField <urn:a:incidents#group> . \
         <urn:a:incidents#group> a cat:Field ; cp:ofConnector <urn:c:sn> ; cp:digest \"{DIGEST}\" ; cp:servesProperty cat:hasSteward ; cat:flag \"owner_missing\" . \
         <urn:a:get_incidents> a cat:ToolAsset ; cp:ofConnector <urn:c:sn> ; cp:digest \"{DIGEST}\" ; \
             cat:flag \"owner_missing\" . \
         <urn:a:responder> a :PromptCandidate, cat:PromptAsset ; cp:ofConnector <urn:c:sn> ; cp:digest \"{DIGEST}\" ; \
             :systemPromptFor <urn:task:incident-triage> ; cat:flag \"owner_missing\" . \
         <urn:s:1> a cat:StewardshipAssignment ; cat:assignedTo <urn:a:incidents> ; \
             cat:principal <urn:g:service-desk> ; cat:role cat:owner . \
         <urn:l:1> a cat:LineageEdge ; cat:from <urn:a:get_incidents> ; cat:to <urn:a:incidents> ; \
             cat:hop \"resource\" . \
         <urn:a:get_incidents> cat:lineageTo <urn:a:incidents> . \
         <urn:t:incident> a cat:CatalogGlossaryTerm ; skos:prefLabel \"Incident\" ; cat:definesClass :Incident ."
    );
    assert!(conforms(&data));
}

// spec: MEDULLA-TYPED-PACKS-R106.1
// spec: MEDULLA-DECISION-ENGINE-R814.1.1
#[test]
fn catalog_shapes_refuse_incomplete_or_open_vocabulary_records() {
    // An asset with no connector, or two.
    assert!(!conforms("<urn:a:t> a cat:ToolAsset ."));
    assert!(!conforms(
        "<urn:a:t> a cat:ToolAsset ; cp:ofConnector <urn:c:a>, <urn:c:b> ."
    ));
    // A field also belongs to exactly one connector-backed catalog.
    assert!(!conforms(
        "<urn:a:f> a cat:Field ; cp:servesProperty cat:hasSteward ."
    ));
    assert!(!conforms(
        "<urn:a:f> a cat:Field ; cp:ofConnector <urn:c:a>, <urn:c:b> ; \
         cp:servesProperty cat:hasSteward ."
    ));
    // A data resource that serves no class.
    assert!(!conforms(
        "<urn:a:r> a cat:DataResource ; cp:ofConnector <urn:c:a> ."
    ));
    // A digest that is not sha256 hex.
    assert!(!conforms(
        "<urn:a:t> a cat:ToolAsset ; cp:ofConnector <urn:c:a> ; cp:digest \"abc\" ."
    ));
    // A system-prompt candidate that frames no task.
    assert!(!conforms(
        "<urn:a:p> a :PromptCandidate, cat:PromptAsset ; cp:ofConnector <urn:c:a> ."
    ));
    // A stewardship role outside the closed set.
    assert!(!conforms(
        "<urn:s:1> a cat:StewardshipAssignment ; cat:assignedTo <urn:a:r> ; \
         cat:principal <urn:g:x> ; cat:role cat:approver ."
    ));
    // A lineage hop outside the closed set.
    assert!(!conforms(
        "<urn:l:1> a cat:LineageEdge ; cat:from <urn:a:x> ; cat:to <urn:a:y> ; cat:hop \"copy\" ."
    ));
    // A glossary term that names no class.
    assert!(!conforms(
        "<urn:t:x> a cat:CatalogGlossaryTerm ; skos:prefLabel \"X\" ."
    ));
    // An unknown asset flag.
    assert!(!conforms(
        "<urn:a:t> a cat:ToolAsset ; cp:ofConnector <urn:c:a> ; cat:flag \"stale\" ."
    ));
    // The owner_missing flag is part of the closed set.
    assert!(conforms(&format!(
        "<urn:a:t> a cat:ToolAsset ; cp:ofConnector <urn:c:a> ; cp:digest \"{DIGEST}\" ; \
         cat:flag \"owner_missing\" ."
    )));
    // An asset without its digest cannot be integrity-checked (MEDULLA-TYPED-PACKS-R106.3).
    assert!(!conforms(
        "<urn:a:t> a cat:ToolAsset ; cp:ofConnector <urn:c:a> ; cat:flag \"owner_missing\" ."
    ));
}

// spec: MEDULLA-TYPED-PACKS-R106.2
#[test]
fn every_asset_has_owner_version_lineage_pair_and_class_link() {
    // A resource and a field name the class and the property they serve.
    assert!(!conforms(
        "<urn:a:r> a cat:DataResource ; cp:ofConnector <urn:c:a> ."
    ));
    assert!(!conforms(
        "<urn:a:f> a cat:Field ; cp:ofConnector <urn:c:a> ."
    ));
    // An asset with neither an assignment nor the owner_missing flag is refused; either one
    // satisfies the shape.
    let asset = &format!(
        "<urn:a:t> a cat:ToolAsset ; cp:ofConnector <urn:c:a> ; cp:digest \"{DIGEST}\" . "
    );
    assert!(!conforms(asset));
    assert!(conforms(&format!(
        "{asset}<urn:s:1> a cat:StewardshipAssignment ; cat:assignedTo <urn:a:t> ; \
         cat:principal <urn:g:x> ; cat:role cat:owner ."
    )));
    assert!(conforms(&format!(
        "{asset}<urn:a:t> cat:flag \"owner_missing\" ."
    )));
    assert!(!conforms(&format!(
        "{asset}<urn:a:t> cat:flag \"deprecated\" ."
    )));
    // The owner is a stewardship assignment with the owner role, one principal and one asset.
    assert!(conforms(
        "<urn:s:1> a cat:StewardshipAssignment ; cat:assignedTo <urn:a:r> ; \
         cat:principal <urn:g:x> ; cat:role cat:owner ."
    ));
    assert!(!conforms(
        "<urn:s:1> a cat:StewardshipAssignment ; cat:assignedTo <urn:a:r> ; cat:role cat:owner ."
    ));
    // A lineage edge is paired with the direct edge between its ends.
    let edge = "<urn:l:1> a cat:LineageEdge ; cat:from <urn:a:x> ; cat:to <urn:a:y> ; \
                cat:hop \"resource\" . ";
    assert!(!conforms(edge));
    assert!(conforms(&format!(
        "{edge}<urn:a:x> cat:lineageTo <urn:a:y> ."
    )));
    // The vocabulary the batch uses is declared.
    let triples = documents();
    for (term, kind) in [
        ("LineageEdge", "Class"),
        ("lineageTo", "ObjectProperty"),
        ("assignedTo", "ObjectProperty"),
    ] {
        assert!(has_iri(
            &triples,
            &format!("{CAT}{term}"),
            &format!("{RDF}type"),
            &format!("{OWL}{kind}")
        ));
    }
    for term in [
        "declaredOwner",
        "ofConnector",
        "packVersion",
        "servesClass",
        "servesProperty",
    ] {
        assert!(triples
            .iter()
            .any(|t| t.subject.to_string() == format!("<{CP}{term}>")));
    }
}

// spec: MEDULLA-TYPED-PACKS-R109.1
#[test]
fn dashboard_asset_needs_one_revision_and_an_owner_or_flag() {
    let owned = "<urn:d:1> a cat:DashboardAsset ; cat:hasOwner <urn:g:x> ; \
                 cat:revision \"1\"^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger> .";
    assert!(conforms(owned));
    assert!(!conforms(
        "<urn:d:1> a cat:DashboardAsset ; cat:hasOwner <urn:g:x> ."
    ));
    assert!(!conforms(
        "<urn:d:1> a cat:DashboardAsset ; \
         cat:revision \"1\"^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger> ."
    ));
}

// spec: MEDULLA-TYPED-PACKS-R108.14
#[test]
fn report_and_template_need_an_owner_and_the_template_a_stored_query() {
    let report =
        "<urn:r:1> a cat:ReportAsset ; cat:hasOwner <urn:g:x> ; cat:readsAsset <urn:a:t> . ";
    assert!(conforms(report));
    assert!(!conforms("<urn:r:1> a cat:ReportAsset ."));
    let template = "<urn:r:2> a cat:ReportTemplate ; cat:hasOwner <urn:g:x> . ";
    assert!(!conforms(template));
    let saved = "<urn:q:1> a cat:SavedQueryAsset ; cat:hasOwner <urn:g:x> ; \
                 cat:parameterName \"region\" . ";
    assert!(conforms(&format!(
        "{saved}{template}<urn:r:2> cat:usesStoredQuery <urn:q:1> ."
    )));
    // A parameter on the template itself no longer stands for a stored query.
    assert!(!conforms(&format!(
        "{template}<urn:r:2> cat:parameterName \"region\" ."
    )));
}

// spec: MEDULLA-TYPED-PACKS-R108.13
#[test]
fn article_needs_author_version_status_and_an_owner_or_flag() {
    let xsd = "http://www.w3.org/2001/XMLSchema#nonNegativeInteger";
    let article = format!(
        "<urn:art:1> a cat:Article ; cat:hasOwner <urn:g:x> ; cat:author \"ada\" ; \
         cat:articleVersion \"1\"^^<{xsd}> ; cat:articleStatus \"draft\" ; \
         cat:aboutAsset <urn:a:orders> ; cat:inBusinessDomain <urn:dom:sales> ."
    );
    assert!(conforms(&article));
    // No author, a status outside the lifecycle, or no owner each fail.
    assert!(!conforms(&article.replace("cat:author \"ada\" ; ", "")));
    assert!(!conforms(&article.replace("\"draft\"", "\"published\"")));
    assert!(!conforms(&article.replace("cat:hasOwner <urn:g:x> ; ", "")));
}

// spec: MEDULLA-TYPED-PACKS-R108.1
#[test]
fn six_families_validate_and_share_one_parent_asset_class() {
    let triples = documents();
    let sub = format!("{RDFS}subClassOf");
    // Every family class is a cat:Asset subclass. A family whose subject is a
    // core entity links it with `cat:describes some :<Class>` and never
    // subclasses it: a catalog record about a person is not the person.
    let families: [(&str, Option<&str>); 16] = [
        ("AgentTemplateAsset", Some("AgentTemplate")),
        ("A2AAgentAsset", Some("Agent")),
        ("AgentVersion", Some("Agent")),
        ("OrgUnit", Some("Organization")),
        ("TeamAsset", Some("Team")),
        ("PersonAsset", Some("Person")),
        ("RoleAsset", Some("Role")),
        ("BusinessDomain", None),
        ("Product", None),
        ("ServiceAsset", Some("Service")),
        ("WorkflowDefinitionAsset", Some("Workflow")),
        ("WorkflowRun", Some("DurableExecutionUnit")),
        ("WorkflowStepAsset", Some("WorkflowStep")),
        ("KnowledgeGraph", Some("Dataset")),
        ("SpecialistPackage", None),
        ("ProviderAsset", None),
    ];
    for (family, core) in families {
        let class = format!("{CAT}{family}");
        assert!(
            has_iri(&triples, &class, &sub, &format!("{CAT}Asset")),
            "{family} must be a cat:Asset subclass"
        );
        if let Some(core) = core {
            assert!(
                !has_iri(&triples, &class, &sub, &format!("{KG}{core}")),
                "{family} must not subclass core {core}"
            );
            assert!(
                describes_some(&triples, &class, &format!("{KG}{core}")),
                "{family} must be cat:describes some core {core}"
            );
        }
    }
    let describes = format!("{CAT}describes");
    for sub_property in ["describesAgent", "describesSkill"] {
        assert!(
            has_iri(
                &triples,
                &format!("{CAT}{sub_property}"),
                &format!("{RDFS}subPropertyOf"),
                &describes
            ),
            "cat:{sub_property} must specialise cat:describes"
        );
    }
    // The families are satisfiable alongside the BFO-rooted core.
    let classification = Reasoner::from_triples(&triples).classify();
    assert!(classification.consistent);
    for (family, _) in families {
        let class = format!("<{CAT}{family}>");
        assert!(
            !classification.unsatisfiable.contains(&class),
            "{family} is unsatisfiable"
        );
    }

    // A complete asset of each family validates.
    let owned =
        |class: &str| format!("<urn:a:{class}> a cat:{class} ; cat:hasOwner <urn:g:owner> .");
    for (family, _) in families {
        if matches!(family, "SpecialistPackage" | "ProviderAsset") {
            continue;
        }
        assert!(conforms(&owned(family)), "{family}");
    }
    for family in ["SpecialistPackage", "ProviderAsset"] {
        assert!(conforms(&format!(
            "<urn:a:{family}> a cat:{family} ; cat:hasOwner <urn:g:owner> ; cp:digest \"{DIGEST}\" ."
        )));
        // A digest is required and must be sha256 hex.
        assert!(!conforms(&owned(family)));
        assert!(!conforms(&format!(
            "<urn:a:{family}> a cat:{family} ; cat:hasOwner <urn:g:owner> ; cp:digest \"abc\" ."
        )));
    }
    // An owner is named, or the asset carries owner_missing; neither refuses.
    assert!(conforms(
        "<urn:a:p> a cat:Product ; cat:flag \"owner_missing\" ."
    ));
    assert!(!conforms("<urn:a:p> a cat:Product ."));
    assert!(!conforms(
        "<urn:a:p> a cat:Product ; cat:hasOwner <urn:g:a>, <urn:g:b> ."
    ));
}

/// True when `class` is `rdfs:subClassOf [ a owl:Restriction ; owl:onProperty
/// cat:describes ; owl:someValuesFrom <filler> ]`.
fn describes_some(triples: &[Triple], class: &str, filler: &str) -> bool {
    use medulla_rdf::oxrdf::{NamedNode, NamedOrBlankNode, Term};
    let named = |iri: &str| NamedNode::new(iri).unwrap();
    let sub = named(&format!("{RDFS}subClassOf"));
    let on_property = named(&format!("{OWL}onProperty"));
    let some = named(&format!("{OWL}someValuesFrom"));
    let restriction = named(&format!("{OWL}Restriction"));
    let rdf_type = named(&format!("{RDF}type"));
    let describes = Term::from(named(&format!("{CAT}describes")));
    let filler = Term::from(named(filler));
    let class = NamedOrBlankNode::from(named(class));
    triples
        .iter()
        .filter(|t| t.subject == class && t.predicate == sub)
        .filter_map(|t| match &t.object {
            Term::BlankNode(node) => Some(NamedOrBlankNode::from(node.clone())),
            _ => None,
        })
        .any(|node| {
            let holds = |predicate: &NamedNode, object: &Term| {
                triples
                    .iter()
                    .any(|t| t.subject == node && t.predicate == *predicate && t.object == *object)
            };
            holds(&rdf_type, &Term::from(restriction.clone()))
                && holds(&on_property, &describes)
                && holds(&some, &filler)
        })
}
