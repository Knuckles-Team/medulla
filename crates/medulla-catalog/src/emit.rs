//! The one engine-side catalog emitter (MEDULLA-TYPED-PACKS-R108.2).
//!
//! `emit_asset` turns a record Medulla already holds into the catalog triples that
//! describe it. It catalogs; it copies no content: every node carries its class, a
//! `cat:sourceRecord` link back to the record id, and the owner, term and lineage
//! links the caller resolved. Prompts, role text, tool bodies and workflow
//! definitions stay in their own stores and are reached through the source id.

use medulla_agent_types::agent_library::AgentLibraryEntry;
use medulla_work_types::orchestration::org::OrgChart;
use medulla_work_types::orchestration::workflows::{WorkflowIdentity, WorkflowRegistrationEntry};

/// Catalog vocabulary namespace.
pub const CAT: &str = "https://knuckles-team.github.io/ontology/catalog#";
/// Core vocabulary namespace.
pub const CORE: &str = "https://knuckles-team.github.io/ontology/core#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const DCT_IS_PART_OF: &str = "http://purl.org/dc/terms/isPartOf";
const PROV_USED: &str = "http://www.w3.org/ns/prov#used";
/// Prefix of every asset node the emitter mints.
pub const ASSET_NODE_PREFIX: &str = "urn:medulla:asset:";
/// The flag a node carries when no owner was resolved for it.
pub const OWNER_MISSING_FLAG: &str = "owner_missing";

/// The object of one emitted triple.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Object {
    /// An IRI node.
    Iri(String),
    /// A plain string literal.
    Literal(String),
}

/// One emitted catalog triple. The caller converts it to its own RDF type or
/// serialises it with [`to_ntriples`] for `AddTriples`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Triple {
    /// Subject IRI.
    pub subject: String,
    /// Predicate IRI.
    pub predicate: String,
    /// Object.
    pub object: Object,
}

/// Why a record could not be emitted. Nothing is emitted for a refused record.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EmitError {
    /// A source record id was empty, so the node would link to nothing.
    #[error("CATALOG_SOURCE_ID_MISSING: {family} record has an empty id")]
    SourceIdMissing {
        /// The family whose record had no id.
        family: &'static str,
    },
    /// A resolved owner, term or lineage reference is not an absolute IRI.
    #[error("CATALOG_LINK_NOT_IRI: {role} reference {value:?} is not an absolute IRI")]
    LinkNotIri {
        /// Which link role carried it.
        role: &'static str,
        /// The offending value.
        value: String,
    },
}

/// Owner, glossary terms and lineage already resolved for one record by their
/// owning rows (stewardship R802, glossary R801, lineage R702). Each entry is an
/// absolute IRI; the emitter writes the links and nothing else about them.
#[derive(Debug, Clone, Copy, Default)]
pub struct AssetLinks<'a> {
    /// The accountable owner, when one is assigned.
    pub owner: Option<&'a str>,
    /// Glossary terms that define this asset.
    pub terms: &'a [String],
    /// Upstream assets this asset derives from.
    pub upstream: &'a [String],
}

/// One workflow run: the run id and the exact definition it ran.
#[derive(Debug, Clone, Copy)]
pub struct WorkflowRunRecord<'a> {
    /// The run id.
    pub run_id: &'a str,
    /// The definition identity the run executed.
    pub definition: &'a WorkflowIdentity,
}

/// One registered graph with the ontology revision, pack contract and sources it
/// is built from.
#[derive(Debug, Clone, Copy)]
pub struct GraphRegistration<'a> {
    /// The graph id.
    pub graph_id: &'a str,
    /// The composed ontology revision digest the graph is described by.
    pub ontology_revision: Option<&'a str>,
    /// The pack release digest (R107) the graph serves.
    pub contract_digest: Option<&'a str>,
    /// Source ids the graph is built from.
    pub sources: &'a [&'a str],
}

/// A record the engine already holds, by source family.
#[derive(Debug, Clone, Copy)]
pub enum AssetRecord<'a> {
    /// An Agent Library entry revision.
    AgentVersion(&'a AgentLibraryEntry),
    /// An organization and the roles it staffs.
    Organization(&'a OrgChart),
    /// A registered workflow definition.
    WorkflowDefinition(&'a WorkflowRegistrationEntry),
    /// One run of a workflow definition.
    WorkflowRun(WorkflowRunRecord<'a>),
    /// A registered graph.
    Graph(GraphRegistration<'a>),
}

fn encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The node IRI of a record in a family, stable for the record id.
pub fn asset_node(family: &str, id: &str) -> String {
    format!("{ASSET_NODE_PREFIX}{family}:{}", encode(id))
}

fn require_id(family: &'static str, id: &str) -> Result<(), EmitError> {
    if id.trim().is_empty() {
        Err(EmitError::SourceIdMissing { family })
    } else {
        Ok(())
    }
}

fn require_iri(role: &'static str, value: &str) -> Result<(), EmitError> {
    let absolute = value
        .split_once(':')
        .is_some_and(|(scheme, rest)| !scheme.is_empty() && !rest.is_empty());
    if absolute
        && !value
            .chars()
            .any(|c| c.is_whitespace() || c == '<' || c == '>')
    {
        Ok(())
    } else {
        Err(EmitError::LinkNotIri {
            role,
            value: value.to_string(),
        })
    }
}

fn iri(subject: &str, predicate: &str, object: &str) -> Triple {
    Triple {
        subject: subject.to_string(),
        predicate: predicate.to_string(),
        object: Object::Iri(object.to_string()),
    }
}

fn literal(subject: &str, predicate: &str, object: &str) -> Triple {
    Triple {
        subject: subject.to_string(),
        predicate: predicate.to_string(),
        object: Object::Literal(object.to_string()),
    }
}

/// The class, node and link triples common to every asset.
fn node(
    out: &mut Vec<Triple>,
    class: &str,
    family: &'static str,
    id: &str,
    source: &str,
    links: &AssetLinks<'_>,
) -> Result<String, EmitError> {
    require_id(family, id)?;
    if let Some(owner) = links.owner {
        require_iri("owner", owner)?;
    }
    for term in links.terms {
        require_iri("term", term)?;
    }
    for upstream in links.upstream {
        require_iri("lineage", upstream)?;
    }
    let subject = asset_node(family, id);
    out.push(iri(&subject, RDF_TYPE, &format!("{CAT}{class}")));
    out.push(literal(&subject, &format!("{CAT}sourceRecord"), source));
    match links.owner {
        Some(owner) => out.push(iri(&subject, &format!("{CAT}hasOwner"), owner)),
        None => out.push(literal(&subject, &format!("{CAT}flag"), OWNER_MISSING_FLAG)),
    }
    for term in links.terms {
        out.push(iri(term, &format!("{CORE}definesAsset"), &subject));
    }
    for upstream in links.upstream {
        out.push(iri(upstream, &format!("{CAT}lineageTo"), &subject));
    }
    Ok(subject)
}

/// Emit the catalog triples for one record the engine already holds.
///
/// One `match` per source family. The output carries the source record id and the
/// resolved owner, term and lineage links; no record content is copied.
pub fn emit_asset(
    record: AssetRecord<'_>,
    links: &AssetLinks<'_>,
) -> Result<Vec<Triple>, EmitError> {
    let mut out = Vec::new();
    match record {
        AssetRecord::AgentVersion(entry) => {
            let id = format!("{}@{}", entry.agent_id, entry.entry_revision);
            let source = format!("agent-library:{}:{}", entry.agent_id, entry.entry_revision);
            node(
                &mut out,
                "AgentVersion",
                "agent-version",
                &id,
                &source,
                links,
            )?;
        }
        AssetRecord::Organization(chart) => {
            let unit = node(
                &mut out,
                "OrgUnit",
                "org-unit",
                &chart.org_id,
                &format!("org:{}", chart.org_id),
                links,
            )?;
            for role in &chart.roles {
                let id = format!("{}/{}", chart.org_id, role.role_id);
                let source = format!("org:{}:role:{}", chart.org_id, role.role_id);
                let role_node = node(
                    &mut out,
                    "RoleAsset",
                    "role",
                    &id,
                    &source,
                    &AssetLinks {
                        owner: links.owner,
                        ..AssetLinks::default()
                    },
                )?;
                out.push(iri(&role_node, DCT_IS_PART_OF, &unit));
            }
        }
        AssetRecord::WorkflowDefinition(entry) => {
            let identity = &entry.identity;
            let id = WorkflowRegistrationEntry::node_id(identity);
            node(
                &mut out,
                "WorkflowDefinitionAsset",
                "workflow",
                &id,
                &id,
                links,
            )?;
        }
        AssetRecord::WorkflowRun(run) => {
            let run_node = node(
                &mut out,
                "WorkflowRun",
                "workflow-run",
                run.run_id,
                &format!("workflow-run:{}", run.run_id),
                links,
            )?;
            let definition = asset_node(
                "workflow",
                &WorkflowRegistrationEntry::node_id(run.definition),
            );
            out.push(iri(&run_node, PROV_USED, &definition));
        }
        AssetRecord::Graph(graph) => {
            let graph_node = node(
                &mut out,
                "KnowledgeGraph",
                "graph",
                graph.graph_id,
                &format!("graph:{}", graph.graph_id),
                links,
            )?;
            if let Some(ontology) = graph.ontology_revision {
                require_id("ontology", ontology)?;
                out.push(iri(
                    &graph_node,
                    &format!("{CAT}hasOntology"),
                    &asset_node("ontology", ontology),
                ));
            }
            if let Some(contract) = graph.contract_digest {
                require_id("contract", contract)?;
                out.push(iri(
                    &graph_node,
                    &format!("{CAT}hasContract"),
                    &asset_node("contract", contract),
                ));
            }
            for source in graph.sources {
                require_id("source", source)?;
                out.push(iri(
                    &graph_node,
                    &format!("{CAT}hasSource"),
                    &asset_node("source", source),
                ));
            }
        }
    }
    Ok(out)
}

fn escape_literal(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

/// Serialise emitted triples as N-Triples, the form `AddTriples` accepts.
pub fn to_ntriples(triples: &[Triple]) -> String {
    let mut out = String::new();
    for triple in triples {
        out.push_str(&format!("<{}> <{}> ", triple.subject, triple.predicate));
        match &triple.object {
            Object::Iri(value) => out.push_str(&format!("<{value}>")),
            Object::Literal(value) => out.push_str(&format!("\"{}\"", escape_literal(value))),
        }
        out.push_str(" .\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_agent_types::agent_component::{AgentComponentKind, ComponentDependency};
    use medulla_agent_types::agent_library::AgentLibraryEntryDraft;
    use medulla_work_types::orchestration::org::RoleSpec;
    use medulla_work_types::orchestration::workflows::WorkflowLifecycle;

    fn digest(c: char) -> String {
        format!("sha256:{}", c.to_string().repeat(64))
    }

    fn dependency(id: &str, kind: AgentComponentKind, c: char) -> ComponentDependency {
        ComponentDependency {
            component_id: id.into(),
            kind,
            definition_digest: digest(c),
        }
    }

    fn agent_entry() -> AgentLibraryEntry {
        AgentLibraryEntry::publish(
            AgentLibraryEntryDraft {
                agent_id: "agent:1".into(),
                package_id: "package:1".into(),
                version: "1.0.0".into(),
                role: "SECRET-ROLE-TEXT".into(),
                role_digest: digest('a'),
                system_prompt: dependency("prompt:1", AgentComponentKind::SystemPrompt, 'b'),
                tools: vec![dependency("tool:1", AgentComponentKind::Tool, 'c')],
                skills: vec![dependency("skill:1", AgentComponentKind::Skill, 'd')],
                model_profile: dependency("model-profile:1", AgentComponentKind::ModelProfile, 'e'),
                model_identity: "model:1".into(),
                ontologies: vec![dependency("ontology:1", AgentComponentKind::Ontology, 'f')],
                tenant_id: "tenant:1".into(),
                actor_scope: "tenant:1/agent:1".into(),
                purpose_id: "delegation.execute".into(),
                policy_digest: digest('0'),
                source_revision: "library-source:7".into(),
                source_revision_digest: digest('1'),
                runtime: Default::default(),
                instantiated_from: None,
            },
            3,
            10,
        )
        .expect("a valid draft publishes")
    }

    fn workflow() -> WorkflowRegistrationEntry {
        WorkflowRegistrationEntry {
            identity: WorkflowIdentity {
                workflow_id: "summarize".into(),
                version: "1.2.0".into(),
                content_digest: digest('a'),
            },
            owner_scope: "tenant:1".into(),
            lifecycle: WorkflowLifecycle::Active,
        }
    }

    fn has(triples: &[Triple], subject: &str, predicate: &str, object: &Object) -> bool {
        triples
            .iter()
            .any(|t| t.subject == subject && t.predicate == predicate && &t.object == object)
    }

    // spec: MEDULLA-TYPED-PACKS-R108.2
    #[test]
    fn existing_records_emit_with_owner_term_and_lineage_without_copies() {
        let owner = "urn:medulla:principal:alice".to_string();
        let terms = vec!["urn:medulla:term:billing".to_string()];
        let upstream = vec!["urn:medulla:asset:source:crm".to_string()];
        let links = AssetLinks {
            owner: Some(&owner),
            terms: &terms,
            upstream: &upstream,
        };
        let entry = agent_entry();
        let agent = emit_asset(AssetRecord::AgentVersion(&entry), &links).unwrap();
        let agent_node = asset_node("agent-version", "agent:1@3");
        assert!(has(
            &agent,
            &agent_node,
            RDF_TYPE,
            &Object::Iri(format!("{CAT}AgentVersion"))
        ));
        assert!(has(
            &agent,
            &agent_node,
            &format!("{CAT}sourceRecord"),
            &Object::Literal("agent-library:agent:1:3".into())
        ));
        assert!(has(
            &agent,
            &agent_node,
            &format!("{CAT}hasOwner"),
            &Object::Iri(owner.clone())
        ));
        assert!(has(
            &agent,
            &terms[0],
            &format!("{CORE}definesAsset"),
            &Object::Iri(agent_node.clone())
        ));
        assert!(has(
            &agent,
            &upstream[0],
            &format!("{CAT}lineageTo"),
            &Object::Iri(agent_node.clone())
        ));
        // Nothing the record holds is copied: no role text, no prompt digest.
        let serialised = to_ntriples(&agent);
        assert!(!serialised.contains("SECRET-ROLE-TEXT"));
        assert!(!serialised.contains(&entry.system_prompt.definition_digest));
        assert!(!serialised.contains(&entry.definition_digest));

        let chart = OrgChart {
            org_id: "acme".into(),
            roles: vec![RoleSpec {
                role_id: "analyst".into(),
                required_capabilities: vec!["SECRET-CAPABILITY".into()],
                headcount: 2,
            }],
        };
        let org = emit_asset(AssetRecord::Organization(&chart), &links).unwrap();
        let unit = asset_node("org-unit", "acme");
        let role = asset_node("role", "acme/analyst");
        assert!(has(
            &org,
            &unit,
            RDF_TYPE,
            &Object::Iri(format!("{CAT}OrgUnit"))
        ));
        assert!(has(
            &org,
            &role,
            RDF_TYPE,
            &Object::Iri(format!("{CAT}RoleAsset"))
        ));
        assert!(has(&org, &role, DCT_IS_PART_OF, &Object::Iri(unit)));
        assert!(!to_ntriples(&org).contains("SECRET-CAPABILITY"));

        let definition = workflow();
        let flow = emit_asset(AssetRecord::WorkflowDefinition(&definition), &links).unwrap();
        let flow_id = WorkflowRegistrationEntry::node_id(&definition.identity);
        assert!(has(
            &flow,
            &asset_node("workflow", &flow_id),
            RDF_TYPE,
            &Object::Iri(format!("{CAT}WorkflowDefinitionAsset"))
        ));
        let run = emit_asset(
            AssetRecord::WorkflowRun(WorkflowRunRecord {
                run_id: "run-9",
                definition: &definition.identity,
            }),
            &links,
        )
        .unwrap();
        assert!(has(
            &run,
            &asset_node("workflow-run", "run-9"),
            PROV_USED,
            &Object::Iri(asset_node("workflow", &flow_id))
        ));

        let sources = ["crm", "erp"];
        let graph = emit_asset(
            AssetRecord::Graph(GraphRegistration {
                graph_id: "g1",
                ontology_revision: Some("rev-4"),
                contract_digest: Some("release-2"),
                sources: &sources,
            }),
            &links,
        )
        .unwrap();
        let graph_node = asset_node("graph", "g1");
        assert!(has(
            &graph,
            &graph_node,
            &format!("{CAT}hasOntology"),
            &Object::Iri(asset_node("ontology", "rev-4"))
        ));
        assert!(has(
            &graph,
            &graph_node,
            &format!("{CAT}hasContract"),
            &Object::Iri(asset_node("contract", "release-2"))
        ));
        assert_eq!(
            graph
                .iter()
                .filter(|t| t.predicate == format!("{CAT}hasSource"))
                .count(),
            2
        );

        // No owner resolved: the node commits flagged, never ownerless-and-silent.
        let flagged =
            emit_asset(AssetRecord::AgentVersion(&entry), &AssetLinks::default()).unwrap();
        assert!(has(
            &flagged,
            &agent_node,
            &format!("{CAT}flag"),
            &Object::Literal(OWNER_MISSING_FLAG.into())
        ));
        assert!(!flagged
            .iter()
            .any(|t| t.predicate == format!("{CAT}hasOwner")));
    }

    #[test]
    fn a_relative_link_or_empty_id_emits_nothing() {
        let entry = agent_entry();
        let bad = AssetLinks {
            owner: Some("alice"),
            ..AssetLinks::default()
        };
        assert!(matches!(
            emit_asset(AssetRecord::AgentVersion(&entry), &bad),
            Err(EmitError::LinkNotIri { role: "owner", .. })
        ));
        let empty = OrgChart {
            org_id: " ".into(),
            roles: vec![],
        };
        assert_eq!(
            emit_asset(AssetRecord::Organization(&empty), &AssetLinks::default()),
            Err(EmitError::SourceIdMissing { family: "org-unit" })
        );
    }
}
