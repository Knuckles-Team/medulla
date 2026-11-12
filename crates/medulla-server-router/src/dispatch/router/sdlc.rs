use super::*;

use std::collections::BTreeSet;
use std::sync::Arc;

use medulla_graph_core::graph::GraphCore;
use medulla_protocol_types::result_contract::ingestion::{
    RecordDeployment as RecordDeploymentResult, RecordRelease as RecordReleaseResult,
};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_server_graph::dispatch::graph_pipeline::dispatch_graph_op;
use medulla_server_graph::dispatch::shared::{
    authorized_graph_read, ensure_graph_resident, timed_read, typed_response,
};
use medulla_source_types::lifecycle::{
    deployment_node_id, handle_record_deployment, handle_record_release, release_node_id,
    Deployment, DeploymentInput, DeploymentReceipt, Release, ReleaseReceipt,
};
use serde_json::{json, Map, Value};
use tokio::sync::RwLock;

/// The node type of a recorded release (a sealed record class).
const RELEASE_TYPE: &str = "Release";
/// The node type of a recorded deployment.
const DEPLOYMENT_TYPE: &str = "Deployment";
/// The property holding the canonical JSON of the release record.
const RELEASE_RECORD_PROPERTY: &str = "release_record";
/// The node type of an indexed repository commit.
const COMMIT_TYPE: &str = "RepositoryCommit";
const MAX_RELEASES_READ: usize = 100_000;
const MAX_CHAIN: usize = 200_000;
const MAX_BATCH_OPERATIONS: usize = 50_000;

/// Served release and deployment records (MEDULLA-SDLC-R002.3 to R002.6): the
/// two methods self-route here. Each decides in
/// `medulla_source_types::lifecycle` and self-translates its records into one
/// gateway-routed `BatchUpdate` against the request graph, so the write is
/// ACL-checked, sealed, durable, audited and CDC-emitted. A refusal returns
/// before the batch is built, so nothing is written.
///
/// Hands a method it does not own back as `ControlFlow::Continue`.
pub async fn dispatch_sdlc_methods(
    ctx: DispatchCtx<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let target = SdlcTarget {
        graph: &ctx.req.graph,
        req_id: ctx.req.id,
        caller: ctx.req.agent_id.as_deref(),
        verified: ctx.verified_context,
    };
    let state = ctx.state;
    let response = match method {
        Method::RecordRelease { release } => {
            dispatch_boxed(record_release(state, target, release)).await
        }
        Method::RecordDeployment { deployment } => {
            dispatch_boxed(record_deployment(state, target, deployment)).await
        }
        other => return ControlFlow::Continue(other),
    };
    ControlFlow::Break(response)
}

struct SdlcTarget<'a> {
    graph: &'a str,
    req_id: u64,
    caller: Option<&'a str>,
    verified: &'a VerifiedRequestContext,
}

async fn record_release(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    release: Release,
) -> Response {
    let result = async {
        let (core, existing) = load_releases(state, &target).await?;
        let chain = commit_chain(&core, target.verified.tenant(), &release.changeset_head);
        let write = handle_record_release(&existing, &chain, release).map_err(|e| e.to_string())?;
        let operations = release_operations(&write, &target)?;
        commit(state, &target, operations).await?;
        Ok(ReleaseReceipt {
            release_id: write.release.id(),
            node_id: release_node_id(&write.release),
            seal: write.seal,
            contains: write.contains.len() as u64,
            replayed: write.replayed,
        })
    }
    .await;
    typed_response::<RecordReleaseResult>(target.req_id, result)
}

async fn record_deployment(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    input: DeploymentInput,
) -> Response {
    let result = async {
        let (_, releases) = load_releases(state, &target).await?;
        let deployment = handle_record_deployment(&releases, input);
        let operations = deployment_operations(&deployment, &target)?;
        commit(state, &target, operations).await?;
        Ok(DeploymentReceipt {
            deployment_id: deployment_node_id(&deployment),
            release_ref: deployment.release_ref,
            outcome: deployment.outcome,
        })
    }
    .await;
    typed_response::<RecordDeploymentResult>(target.req_id, result)
}

/// The request graph and the verified tenant's stored releases visible to the caller.
async fn load_releases(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
) -> Result<(Arc<GraphCore>, Vec<Release>), String> {
    let tenant = target.verified.tenant();
    if tenant.trim().is_empty() {
        return Err("TENANT_SCOPE_REQUIRED".to_string());
    }
    ensure_graph_resident(state, target.graph).await;
    let (core, authority) = {
        let current = timed_read(state).await;
        authorized_graph_read(
            &current,
            target.graph,
            target.verified,
            &format!("GRAPH_NOT_FOUND: {}", target.graph),
        )?
    };
    let releases = core
        .get_nodes_by_label(RELEASE_TYPE, MAX_RELEASES_READ)
        .into_iter()
        .filter(|(node_id, properties)| {
            authority.can_see_node(properties, core.is_schema_node(node_id))
        })
        .filter_map(|(_, properties)| release_record(&properties, tenant))
        .collect();
    Ok((core, releases))
}

fn release_record(properties: &[u8], tenant: &str) -> Option<Release> {
    let node = medulla_types::msgpack::decode_property_value(properties).ok()?;
    if node.get("tenant_id").and_then(Value::as_str) != Some(tenant) {
        return None;
    }
    serde_json::from_str(node.get(RELEASE_RECORD_PROPERTY)?.as_str()?).ok()
}

/// The commits reachable from `head` through `parents`, oldest first (parents
/// before children), ending at `head`. Empty when `head` is not an indexed
/// commit, so the release refuses `RELEASE_HEAD_UNKNOWN`.
fn commit_chain(core: &GraphCore, tenant: &str, head: &str) -> Vec<String> {
    let parents_of = |digest: &str| -> Option<Vec<String>> {
        let properties = core.get_node_properties(&format!("commit:{digest}"))?;
        let node = medulla_types::msgpack::decode_property_value(&properties).ok()?;
        if node.get("type").and_then(Value::as_str) != Some(COMMIT_TYPE) {
            return None;
        }
        // A commit stamped for another tenant is not on this tenant's chain.
        if node
            .get("tenant_id")
            .and_then(Value::as_str)
            .is_some_and(|owner| owner != tenant)
        {
            return None;
        }
        let parents = node.get("parents").and_then(Value::as_str).unwrap_or("");
        Some(
            parents
                .split(',')
                .filter(|p| !p.is_empty())
                .map(str::to_owned)
                .collect(),
        )
    };
    let mut chain = Vec::new();
    let mut seen = BTreeSet::new();
    // Iterative post-order walk: (digest, parents still to visit).
    let Some(parents) = parents_of(head) else {
        return chain;
    };
    seen.insert(head.to_owned());
    let mut stack = vec![(head.to_owned(), parents.into_iter())];
    while let Some((digest, remaining)) = stack.last_mut() {
        match remaining.next() {
            Some(parent) => {
                if chain.len() + stack.len() > MAX_CHAIN || !seen.insert(parent.clone()) {
                    continue;
                }
                if let Some(grandparents) = parents_of(&parent) {
                    stack.push((parent, grandparents.into_iter()));
                }
            }
            None => {
                chain.push(digest.clone());
                stack.pop();
            }
        }
    }
    chain
}

fn stamp(properties: &mut Map<String, Value>, target: &SdlcTarget<'_>) {
    properties.insert("tenant_id".into(), target.verified.tenant().into());
    properties.insert("_owner".into(), target.verified.agent_id().into());
}

fn edge(source: &str, relationship: &str, destination: &str, target: &SdlcTarget<'_>) -> Value {
    let mut properties = Map::from_iter([("relationship".to_string(), relationship.into())]);
    stamp(&mut properties, target);
    json!({
        "op": "upsert_edge",
        "source": source,
        "target": destination,
        "properties": properties,
    })
}

/// The sealed release node, then one `contains` edge per changeset it contains.
fn release_operations(
    write: &medulla_source_types::lifecycle::ReleaseWrite,
    target: &SdlcTarget<'_>,
) -> Result<Vec<Value>, String> {
    let release = &write.release;
    let id = release_node_id(release);
    let body =
        serde_json::to_string(release).map_err(|_| "release record encoding failed".to_string())?;
    let mut properties = Map::new();
    properties.insert("type".into(), RELEASE_TYPE.into());
    properties.insert(RELEASE_RECORD_PROPERTY.into(), body.into());
    properties.insert("repo".into(), release.repo.clone().into());
    properties.insert("version".into(), release.version.clone().into());
    properties.insert("git_ref".into(), release.git_ref.clone().into());
    properties.insert(
        "changeset_head".into(),
        release.changeset_head.clone().into(),
    );
    properties.insert("published_at".into(), release.published_at.into());
    properties.insert("release_seal".into(), write.seal.clone().into());
    stamp(&mut properties, target);
    let mut operations = vec![json!({"op": "upsert_node", "id": id, "properties": properties})];
    for changeset in &write.contains {
        operations.push(edge(
            &id,
            "contains",
            &format!("commit:{changeset}"),
            target,
        ));
    }
    bounded(operations)
}

/// The deployment node and, when a release names its image, the `deploys` edge.
fn deployment_operations(
    deployment: &Deployment,
    target: &SdlcTarget<'_>,
) -> Result<Vec<Value>, String> {
    let id = deployment_node_id(deployment);
    let mut properties = Map::new();
    properties.insert("type".into(), DEPLOYMENT_TYPE.into());
    properties.insert("environment".into(), deployment.environment.clone().into());
    properties.insert("workload".into(), deployment.workload.clone().into());
    properties.insert(
        "image_digest".into(),
        deployment.image_digest.clone().into(),
    );
    properties.insert("started_at".into(), deployment.started_at.into());
    properties.insert("finished_at".into(), deployment.finished_at.into());
    properties.insert("outcome".into(), json!(deployment.outcome));
    if let Some(release_ref) = &deployment.release_ref {
        properties.insert("release_ref".into(), release_ref.clone().into());
    }
    stamp(&mut properties, target);
    let mut operations = vec![json!({"op": "upsert_node", "id": id, "properties": properties})];
    if let Some(release_ref) = &deployment.release_ref {
        // `Release::id` is `repo@version`; the node id is `release:repo:version`.
        let node = release_ref.replacen('@', ":", 1);
        operations.push(edge(&format!("release:{node}"), "deploys", &id, target));
    }
    bounded(operations)
}

fn bounded(operations: Vec<Value>) -> Result<Vec<Value>, String> {
    if operations.len() > MAX_BATCH_OPERATIONS {
        return Err("INVALID_ARGUMENT: the release exceeds one BatchUpdate".to_string());
    }
    Ok(operations)
}

/// Commit `operations` as ONE `BatchUpdate` through the ordinary graph gateway.
async fn commit(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
    operations: Vec<Value>,
) -> Result<(), String> {
    let operations_msgpack = rmp_serde::to_vec_named(&operations)
        .map_err(|_| "lifecycle batch encoding failed".to_string())?;
    let response = dispatch_graph_op(
        state,
        target.graph,
        target.req_id,
        target.caller,
        target.verified,
        Method::BatchUpdate { operations_msgpack },
    )
    .await;
    match response.refusal_text() {
        Some(refusal) => Err(refusal),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_source_types::lifecycle::DeploymentOutcome;

    fn deployment(release_ref: Option<&str>) -> Deployment {
        Deployment {
            environment: "prod".into(),
            workload: "engine".into(),
            image_digest: "sha256:aa".into(),
            release_ref: release_ref.map(str::to_owned),
            started_at: 1,
            finished_at: 2,
            outcome: DeploymentOutcome::Succeeded,
        }
    }

    #[test]
    fn a_release_ref_maps_to_its_release_node_id() {
        let release = Release {
            repo: "medulla".into(),
            version: "1.0.0".into(),
            git_ref: "refs/tags/v1.0.0".into(),
            changeset_head: "c".into(),
            artifact_digests: vec![],
            published_at: 1,
        };
        assert_eq!(
            format!("release:{}", release.id().replacen('@', ":", 1)),
            release_node_id(&release)
        );
        assert!(deployment(None).release_ref.is_none());
    }
}
