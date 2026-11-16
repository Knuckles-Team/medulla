//! Served domain pack preview and run (MEDULLA-TYPED-PACKS-R097.700).
//!
//! `Method::DomainPack` carries a manifest, its source rows and markdown. The
//! loader checks the manifest's `integrity_sha256`, then its bounds and
//! references, then runs the manifest's own conformance cases; only then does
//! `evaluate_mapping` (a pure function in `medulla-core`) propose facts.
//! `Preview` returns them and writes nothing (served by the catalog admin
//! route). `Run` is gateway-routed (`Route::DomainPack`) and commits the entities
//! and relationships as one `BatchUpdate`, so the durable log, audit and CDC see
//! ordinary node and edge writes. A refusal writes nothing.

use super::*;
use medulla_catalog_types::domain_pack::{
    ComposedVocabulary, DomainPackInput, DomainPackManifest, DomainPackOp, DomainPackPreview,
    DomainPackRun, ManifestError, ProposedFact,
};
use medulla_core::domain_pack::{evaluate_mapping, fragment_markdown, run_cases, MappingError};
use medulla_isolation::isolation::AccessLevel;
use medulla_protocol_types::result_contract::graph::{
    DomainPackPreviewResult, DomainPackRunResult,
};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_server_auth::access::check_graph_access;
use medulla_types::contract::ServerErrorCode;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tokio::sync::RwLock;

/// The node property naming the opaque verified tenant scope of the writer.
const TENANT_SCOPE_KEY: &str = "domain_pack_tenant_scope";
/// Evidence of a promoted node: the digest of the manifest that produced it.
const EVIDENCE_INTEGRITY_KEY: &str = "domain_pack_integrity_sha256";
/// Evidence of a promoted node: the source row or markdown artifact it came from.
const EVIDENCE_SOURCE_KEY: &str = "domain_pack_source";
/// Node property names a pack property may not use.
const RESERVED: &[&str] = &[
    "type",
    "domain_pack",
    "domain_pack_version",
    EVIDENCE_INTEGRITY_KEY,
    EVIDENCE_SOURCE_KEY,
    TENANT_SCOPE_KEY,
];

fn refuse(code: ServerErrorCode, detail: impl std::fmt::Display) -> String {
    format!("{}: {detail}", code.as_str())
}

/// The wire refusal of a manifest error.
fn manifest_refusal(error: ManifestError) -> String {
    match error {
        ManifestError::IntegrityMismatch { declared, computed } => refuse(
            ServerErrorCode::DomainPackIntegrityMismatch,
            format!("declared {declared}, computed {computed}"),
        ),
        ManifestError::Invalid(detail) => refuse(ServerErrorCode::DomainPackInvalid, detail),
        ManifestError::MappingInvalid(detail) => {
            refuse(ServerErrorCode::DomainPackMappingInvalid, detail)
        }
        ManifestError::ClassUnknown(detail) => {
            refuse(ServerErrorCode::DomainPackClassUnknown, detail)
        }
    }
}

/// What admission yields: the proposed facts, how many cases passed, and the
/// source artifact (a source row id or a markdown artifact id) of each entity.
struct Admitted {
    facts: Vec<ProposedFact>,
    cases_passed: u32,
    sources: BTreeMap<String, String>,
}

/// The composed vocabulary of the request graph: its core plus admitted packs.
fn composed_vocabulary(core: &GraphCore) -> Result<ComposedVocabulary, String> {
    medulla_server_ops::api::admin::terms::composed_vocabulary(
        &medulla_server_ops::api::admin::schema_read::disclosed_sources(core),
    )
}

/// Load the manifest and evaluate the request: the proposed facts and how many
/// of the manifest's cases passed. Mappings resolve against `composed`, the
/// target graph's composed ontology, not the manifest's own classes alone.
fn admit(input: &DomainPackInput, composed: &ComposedVocabulary) -> Result<Admitted, String> {
    input
        .manifest
        .verify_integrity()
        .map_err(manifest_refusal)?;
    let manifest = input.manifest.resolve(composed).map_err(manifest_refusal)?;
    input.check_bounds().map_err(manifest_refusal)?;
    let cases_passed = run_cases(&manifest).map_err(|MappingError::CaseFailed { case }| {
        refuse(
            ServerErrorCode::DomainPackCaseFailed,
            format!("case '{case}' does not evaluate to its expected facts"),
        )
    })?;
    let mut facts = Vec::new();
    let mut sources = BTreeMap::new();
    let mut take = |source: &str, batch: Vec<ProposedFact>| {
        for fact in &batch {
            if let ProposedFact::Entity { entity_id, .. } = fact {
                sources.insert(entity_id.clone(), source.to_owned());
            }
        }
        facts.extend(batch);
    };
    for row in &input.rows {
        take(
            &row.row_id,
            evaluate_mapping(&manifest, std::slice::from_ref(row)),
        );
    }
    for source in &input.markdown {
        take(
            &source.artifact_id,
            fragment_markdown(&source.artifact_id, &source.text),
        );
    }
    Ok(Admitted {
        facts,
        cases_passed,
        sources,
    })
}

/// Serve `Preview`: the proposed facts, nothing written. `Run` never reaches
/// this function; the router hands mutations to the gateway. Preview resolves
/// mappings against the request graph's composed ontology, so it reads the graph
/// under the caller's read access first.
pub async fn handle_domain_pack(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    graph: &str,
    verified: &VerifiedRequestContext,
    op: DomainPackOp,
) -> Response {
    let DomainPackOp::Preview { input } = op else {
        return Response::err(
            req_id,
            "INVALID_ARGUMENT: a domain pack run commits through the mutation gateway",
        );
    };
    let core = {
        let guard = state.read().await;
        let Some(entry) = guard.registry.get(graph) else {
            return Response::err(req_id, format!("INVALID_ARGUMENT: unknown graph {graph}"));
        };
        if let Err(denied) = check_graph_access(
            &guard.isolation,
            Some(verified.agent_id()),
            graph,
            entry.graph_type,
            entry.owner.as_deref(),
            AccessLevel::Read,
        ) {
            return Response::err(req_id, denied);
        }
        Arc::clone(&entry.core)
    };
    let admitted = composed_vocabulary(&core).and_then(|composed| admit(&input, &composed));
    match admitted {
        Ok(Admitted {
            facts,
            cases_passed,
            ..
        }) => Response::ok(
            req_id,
            ResultPayload::of::<DomainPackPreviewResult>(DomainPackPreview {
                pack_id: input.manifest.pack_id,
                version: input.manifest.version,
                cases_passed,
                facts,
            }),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

/// The tenant scope stored on a node, `None` when the node is absent.
fn stored_scope(core: &GraphCore, node_id: &str) -> Option<Option<String>> {
    let blob = core.get_node_properties(node_id)?;
    let props = medulla_types::msgpack::decode_property_object(&blob).ok()?;
    Some(
        props
            .get(TENANT_SCOPE_KEY)
            .and_then(|v| v.as_str())
            .map(str::to_owned),
    )
}

/// A node of another tenant is never read or overwritten; `Ok(true)` when the
/// node exists and is the writer's (or unscoped).
fn ensure_owned(core: &GraphCore, node_id: &str, tenant_scope: &str) -> Result<bool, String> {
    match stored_scope(core, node_id) {
        None => Ok(false),
        Some(Some(owner)) if owner != tenant_scope => Err(format!(
            "ACCESS_DENIED: {node_id} belongs to another tenant"
        )),
        Some(_) => Ok(true),
    }
}

/// The batch a request commits as, and its counts.
struct Plan {
    operations_msgpack: Vec<u8>,
    entities: u64,
    relationships: u64,
}

/// Group the facts into node upserts and edge upserts.
fn plan_batch(
    core: &GraphCore,
    manifest: &DomainPackManifest,
    tenant_scope: &str,
    admitted: &Admitted,
) -> Result<Plan, String> {
    let (pack_id, version) = (&manifest.pack_id, &manifest.version);
    let facts = &admitted.facts;
    let mut nodes: BTreeMap<&str, (&str, serde_json::Map<String, serde_json::Value>)> =
        BTreeMap::new();
    for fact in facts {
        if let ProposedFact::Entity { entity_id, class } = fact {
            nodes
                .entry(entity_id)
                .or_insert((class, serde_json::Map::new()));
        }
    }
    let mut edges = Vec::new();
    for fact in facts {
        match fact {
            ProposedFact::Entity { .. } => {}
            ProposedFact::Property {
                entity_id,
                property,
                value,
            } => {
                if RESERVED.contains(&property.as_str()) {
                    return Err(refuse(
                        ServerErrorCode::DomainPackInvalid,
                        format!("property '{property}' is reserved"),
                    ));
                }
                let Some((_, props)) = nodes.get_mut(entity_id.as_str()) else {
                    return Err(refuse(
                        ServerErrorCode::DomainPackInvalid,
                        format!("property of undeclared entity {entity_id}"),
                    ));
                };
                match props.remove(property) {
                    None => props.insert(property.clone(), value.clone()),
                    Some(serde_json::Value::Array(mut items)) => {
                        items.push(value.clone());
                        props.insert(property.clone(), serde_json::Value::Array(items))
                    }
                    Some(first) => props.insert(
                        property.clone(),
                        serde_json::Value::Array(vec![first, value.clone()]),
                    ),
                };
            }
            ProposedFact::Relationship { from, relation, to } => {
                if !nodes.contains_key(to.as_str()) && !ensure_owned(core, to, tenant_scope)? {
                    return Err(refuse(
                        ServerErrorCode::DomainPackInvalid,
                        format!("relation target {to} resolves to no entity or stored node"),
                    ));
                }
                edges.push((from.as_str(), relation.as_str(), to.as_str()));
            }
        }
    }
    let mut operations = Vec::new();
    for (id, (class, props)) in &nodes {
        ensure_owned(core, id, tenant_scope)?;
        let mut properties = props.clone();
        properties.insert("type".into(), (*class).into());
        properties.insert("domain_pack".into(), pack_id.into());
        properties.insert("domain_pack_version".into(), version.into());
        properties.insert(
            EVIDENCE_INTEGRITY_KEY.into(),
            manifest.integrity_sha256.as_str().into(),
        );
        if let Some(source) = admitted.sources.get(*id) {
            properties.insert(EVIDENCE_SOURCE_KEY.into(), source.as_str().into());
        }
        properties.insert(TENANT_SCOPE_KEY.into(), tenant_scope.into());
        operations.push(serde_json::json!({
            "op": "upsert_node", "id": id, "properties": properties,
        }));
    }
    let relationships = edges.len() as u64;
    let unique: BTreeSet<_> = edges.into_iter().collect();
    for (source, relation, target) in unique {
        operations.push(serde_json::json!({
            "op": "upsert_edge", "source": source, "target": target,
            "properties": {"relationship": relation},
        }));
    }
    let operations_msgpack = rmp_serde::to_vec_named(&operations)
        .map_err(|error| format!("INVALID_ARGUMENT: domain pack encoding failed: {error}"))?;
    Ok(Plan {
        operations_msgpack,
        entities: nodes.len() as u64,
        relationships,
    })
}

/// The `Route::DomainPack` entry: admit, then commit as one `BatchUpdate`.
pub(super) async fn handle_run(ctx: &MutationCtx<'_>, method: &Method) -> Option<Response> {
    let Method::DomainPack { op } = method else {
        return None;
    };
    let DomainPackOp::Run { input } = op.as_ref() else {
        return Some(Response::err(
            ctx.req_id,
            "INVALID_ARGUMENT: a domain pack preview is served by the catalog admin route",
        ));
    };
    let admitted = composed_vocabulary(ctx.core)
        .and_then(|composed| admit(input, &composed))
        .and_then(|admitted| plan_batch(ctx.core, &input.manifest, ctx.tenant_scope, &admitted));
    let plan = match admitted {
        Ok(plan) => plan,
        Err(error) => return Some(Response::err(ctx.req_id, error)),
    };
    let write = Method::BatchUpdate {
        operations_msgpack: plan.operations_msgpack.clone(),
    };
    let result = DomainPackRun {
        pack_id: input.manifest.pack_id.clone(),
        version: input.manifest.version.clone(),
        entities_written: plan.entities,
        relationships_written: plan.relationships,
    };
    let mutation_plan = MutationPlan::for_method(&write);
    let operations = plan.operations_msgpack;
    Some(
        medulla_server_ops::api::ingest::graph::commit_gateway(
            ctx,
            &mutation_plan,
            &write,
            move |core| {
                medulla_graph_algos::algorithms::batch_update(core, &operations)?;
                ResultPayload::of::<DomainPackRunResult>(result)
            },
        )
        .await,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_catalog_types::domain_pack::{
        DomainPackManifest, ManifestClass, ManifestMapping, SourceKind, SourceRow,
    };

    fn input(selector: &str) -> DomainPackInput {
        let mut manifest = DomainPackManifest {
            schema_version: 1,
            pack_id: "hr".into(),
            version: "1".into(),
            integrity_sha256: String::new(),
            classes: vec![ManifestClass {
                name: "Person".into(),
                properties: vec!["name".into()],
                relations: vec![],
            }],
            mappings: vec![ManifestMapping {
                source_kind: SourceKind::Column,
                selector: selector.into(),
                target_class: "Person".into(),
                target_property_or_relation: "name".into(),
            }],
            cases: vec![],
        };
        manifest.integrity_sha256 = manifest.compute_integrity();
        DomainPackInput {
            manifest,
            rows: vec![SourceRow {
                row_id: "r1".into(),
                source_kind: SourceKind::Column,
                fields: [("full_name".to_string(), serde_json::json!("Ada"))].into(),
            }],
            markdown: vec![],
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R097.700
    #[test]
    fn admission_refuses_a_tampered_manifest_and_plans_one_node() {
        let ok = input("full_name");
        let admitted = admit(&ok, &ComposedVocabulary::default()).unwrap();
        assert_eq!(admitted.cases_passed, 0);
        let plan = plan_batch(&GraphCore::new(), &ok.manifest, "tenant-a", &admitted).unwrap();
        assert_eq!((plan.entities, plan.relationships), (1, 0));

        let mut tampered = input("full_name");
        tampered.manifest.version = "2".into();
        assert!(admit(&tampered, &ComposedVocabulary::default())
            .unwrap_err()
            .starts_with("DOMAIN_PACK_INTEGRITY_MISMATCH"));
    }

    // spec: MEDULLA-TYPED-PACKS-R097.701
    #[test]
    fn refusals_carry_their_own_codes_and_evidence_names_the_source() {
        let mut ghost = input("full_name");
        ghost.manifest.mappings[0].target_class = "Ghost".into();
        ghost.manifest.integrity_sha256 = ghost.manifest.compute_integrity();
        assert!(admit(&ghost, &ComposedVocabulary::default())
            .unwrap_err()
            .starts_with("DOMAIN_PACK_CLASS_UNKNOWN"));

        let mut bad = input("full_name");
        bad.manifest.mappings[0].target_property_or_relation = "undeclared".into();
        bad.manifest.integrity_sha256 = bad.manifest.compute_integrity();
        assert!(admit(&bad, &ComposedVocabulary::default())
            .unwrap_err()
            .starts_with("DOMAIN_PACK_MAPPING_INVALID"));

        let ok = input("full_name");
        let admitted = admit(&ok, &ComposedVocabulary::default()).unwrap();
        assert_eq!(admitted.sources.values().collect::<Vec<_>>(), ["r1"]);
        let plan = plan_batch(&GraphCore::new(), &ok.manifest, "tenant-a", &admitted).unwrap();
        let operations: Vec<serde_json::Value> =
            rmp_serde::from_slice(&plan.operations_msgpack).unwrap();
        let properties = &operations[0]["properties"];
        assert_eq!(properties[EVIDENCE_SOURCE_KEY], "r1");
        assert_eq!(
            properties[EVIDENCE_INTEGRITY_KEY],
            ok.manifest.integrity_sha256.as_str()
        );
        assert_eq!(properties["domain_pack"], "hr");
        assert_eq!(properties["domain_pack_version"], "1");
    }

    // spec: MEDULLA-TYPED-PACKS-R097.701
    #[test]
    fn a_mapping_onto_a_composed_class_is_admitted_and_one_in_no_schema_is_refused() {
        use medulla_catalog_types::graph_schema::GraphSchemaTermKind as Kind;
        let composed = ComposedVocabulary::from_terms([
            (
                "https://knuckles-team.github.io/ontology/core#Document",
                Kind::Class,
            ),
            (
                "https://knuckles-team.github.io/ontology/core#title",
                Kind::DatatypeProperty,
            ),
        ]);
        let mut onto_core = input("full_name");
        onto_core.manifest.mappings[0].target_class = "Document".into();
        onto_core.manifest.mappings[0].target_property_or_relation = "title".into();
        onto_core.manifest.integrity_sha256 = onto_core.manifest.compute_integrity();
        let admitted = admit(&onto_core, &composed).unwrap();
        assert!(admitted.facts.iter().any(|fact| matches!(
            fact,
            ProposedFact::Property { property, .. } if property == "title"
        )));
        assert!(admit(&onto_core, &ComposedVocabulary::default())
            .unwrap_err()
            .starts_with("DOMAIN_PACK_CLASS_UNKNOWN"));
    }
}
