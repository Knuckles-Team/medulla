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
    DomainPackInput, DomainPackOp, DomainPackPreview, DomainPackRun, ManifestError, ProposedFact,
};
use medulla_core::domain_pack::{evaluate_mapping, fragment_markdown, run_cases, MappingError};
use medulla_protocol_types::result_contract::graph::{
    DomainPackPreviewResult, DomainPackRunResult,
};
use medulla_types::contract::ServerErrorCode;
use std::collections::{BTreeMap, BTreeSet};

/// The node property naming the opaque verified tenant scope of the writer.
const TENANT_SCOPE_KEY: &str = "domain_pack_tenant_scope";
/// Node property names a pack property may not use.
const RESERVED: &[&str] = &[
    "type",
    "domain_pack",
    "domain_pack_version",
    TENANT_SCOPE_KEY,
];

fn refuse(code: ServerErrorCode, detail: impl std::fmt::Display) -> String {
    format!("{}: {detail}", code.as_str())
}

/// Load the manifest and evaluate the request: the proposed facts and how many
/// of the manifest's cases passed.
fn admit(input: &DomainPackInput) -> Result<(Vec<ProposedFact>, u32), String> {
    input
        .manifest
        .verify_integrity()
        .map_err(|error| match error {
            ManifestError::IntegrityMismatch { declared, computed } => refuse(
                ServerErrorCode::DomainPackIntegrityMismatch,
                format!("declared {declared}, computed {computed}"),
            ),
            ManifestError::Invalid(detail) => refuse(ServerErrorCode::DomainPackInvalid, detail),
        })?;
    let invalid = |error: ManifestError| match error {
        ManifestError::Invalid(detail) => refuse(ServerErrorCode::DomainPackInvalid, detail),
        ManifestError::IntegrityMismatch { .. } => refuse(
            ServerErrorCode::DomainPackIntegrityMismatch,
            "digest differs",
        ),
    };
    input.manifest.validate().map_err(invalid)?;
    input.check_bounds().map_err(invalid)?;
    let passed = run_cases(&input.manifest).map_err(|MappingError::CaseFailed { case }| {
        refuse(
            ServerErrorCode::DomainPackCaseFailed,
            format!("case '{case}' does not evaluate to its expected facts"),
        )
    })?;
    let mut facts = evaluate_mapping(&input.manifest, &input.rows);
    for source in &input.markdown {
        facts.extend(fragment_markdown(&source.artifact_id, &source.text));
    }
    Ok((facts, passed))
}

/// Serve `Preview`: the proposed facts, nothing written. `Run` never reaches
/// this function; the router hands mutations to the gateway.
pub async fn handle_domain_pack(req_id: u64, op: DomainPackOp) -> Response {
    let DomainPackOp::Preview { input } = op else {
        return Response::err(
            req_id,
            "INVALID_ARGUMENT: a domain pack run commits through the mutation gateway",
        );
    };
    match admit(&input) {
        Ok((facts, cases_passed)) => Response::ok(
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
    pack_id: &str,
    version: &str,
    tenant_scope: &str,
    facts: &[ProposedFact],
) -> Result<Plan, String> {
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
    let admitted = admit(input).and_then(|(facts, _)| {
        plan_batch(
            ctx.core,
            &input.manifest.pack_id,
            &input.manifest.version,
            ctx.tenant_scope,
            &facts,
        )
    });
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
        let (facts, passed) = admit(&ok).unwrap();
        assert_eq!(passed, 0);
        let plan = plan_batch(&GraphCore::new(), "hr", "1", "tenant-a", &facts).unwrap();
        assert_eq!((plan.entities, plan.relationships), (1, 0));

        let mut tampered = input("full_name");
        tampered.manifest.version = "2".into();
        assert!(admit(&tampered)
            .unwrap_err()
            .starts_with("DOMAIN_PACK_INTEGRITY_MISMATCH"));
    }
}
