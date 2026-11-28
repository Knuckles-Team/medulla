//! Served policy source binding (MEDULLA-SRC-AUTH-R801).
//!
//! `PolicySourceBind` is gateway-routed and admin-scoped (`kg:admin`). After a
//! human review, the caller names the reviewed source revision and digest and
//! the head `version` of the `DecisionPolicy` component it saw. [`bind`] is
//! pure over the already-read head component and decides, in order, whether the
//! review still matches; the gateway arm ([`handle_policy_source_bind`]) does
//! the `current_component` read and the one node commit. Medulla never writes
//! the source: a requested source change goes through the governed write-back.
//!
//! The activation record is the graph node `policy_source:<policy_iri>` with the
//! label `PolicySourceBinding`: created when absent, replaced by one
//! `CompareAndSetNodeFields` on the stored `source_revision` otherwise. Every
//! refusal writes nothing, so the active policy is unchanged.

use super::*;

use medulla_agent_types::agent_component::{AgentComponentEntry, AgentComponentKind};
use medulla_types::contract::ServerErrorCode;
use medulla_types::policy_source::{
    policy_source_node_id, PolicySourceBind, PolicySourceBinding, POLICY_SOURCE_LABEL,
};

/// A refused bind: the registered code and what it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicySourceRefusal {
    pub code: &'static str,
    pub detail: String,
}

impl PolicySourceRefusal {
    fn new(code: ServerErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code: code.as_str(),
            detail: detail.into(),
        }
    }

    fn invalid(detail: impl Into<String>) -> Self {
        Self {
            code: "INVALID_ARGUMENT",
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for PolicySourceRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

/// The stored digest of a head: its `content_digest` without a leading `sha256:`.
fn stored_digest(head: &AgentComponentEntry) -> &str {
    head.content_digest
        .strip_prefix("sha256:")
        .unwrap_or(&head.content_digest)
}

/// The stored source and revision of a head: its `content_ref` split at the
/// last `@`. A head without a reviewed source has neither.
fn stored_source(head: &AgentComponentEntry) -> (Option<&str>, Option<&str>) {
    match head.content_ref.as_deref().and_then(|r| r.rsplit_once('@')) {
        Some((source, revision)) => (Some(source), Some(revision)),
        None => (None, None),
    }
}

/// Decide a bind over the head component the caller read. The rules run in
/// order: an empty `approver` is `POLICY_SOURCE_UNREVIEWED`; a malformed
/// revision or digest, or no such `DecisionPolicy` component, is
/// `INVALID_ARGUMENT`; a head `version` other than the one reviewed is
/// `POLICY_SOURCE_STALE`; a digest, source or revision other than the head's is
/// `POLICY_SOURCE_DIGEST_MISMATCH`. Otherwise the activation record.
pub fn bind(
    request: &PolicySourceBind,
    head: Option<&AgentComponentEntry>,
    now_ms: u64,
) -> Result<PolicySourceBinding, PolicySourceRefusal> {
    if request.approver.trim().is_empty() {
        return Err(PolicySourceRefusal::new(
            ServerErrorCode::PolicySourceUnreviewed,
            "the bind names no approver",
        ));
    }
    if !request.shape_valid() {
        return Err(PolicySourceRefusal::invalid(
            "source_revision must be 40 hex characters and source_digest 64 lowercase hex characters",
        ));
    }
    let head = head
        .filter(|head| head.kind == AgentComponentKind::DecisionPolicy)
        .ok_or_else(|| {
            PolicySourceRefusal::invalid(format!(
                "{} is not a published DecisionPolicy component",
                request.policy_iri
            ))
        })?;
    if head.version != request.component_version {
        return Err(PolicySourceRefusal::new(
            ServerErrorCode::PolicySourceStale,
            format!(
                "the head of {} is version {}, not the reviewed {}",
                request.policy_iri, head.version, request.component_version
            ),
        ));
    }
    let (source, revision) = stored_source(head);
    if request.source_digest != stored_digest(head)
        || source != Some(request.source_ref.as_str())
        || revision != Some(request.source_revision.as_str())
    {
        return Err(PolicySourceRefusal::new(
            ServerErrorCode::PolicySourceDigestMismatch,
            format!(
                "the reviewed source does not match the head of {}",
                request.policy_iri
            ),
        ));
    }
    Ok(PolicySourceBinding::from_request(request, now_ms))
}

/// The node properties of a record: its fields flat, under the label.
fn record_properties(
    binding: &PolicySourceBinding,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let serde_json::Value::Object(mut fields) = serde_json::to_value(binding)
        .map_err(|error| format!("INVALID_ARGUMENT: policy source encoding failed: {error}"))?
    else {
        return Err("INVALID_ARGUMENT: policy source record is not an object".to_string());
    };
    fields.insert(
        "type".to_string(),
        serde_json::Value::String(POLICY_SOURCE_LABEL.to_string()),
    );
    Ok(fields)
}

/// The stored record of one policy, when its node exists and decodes.
fn stored_record(core: &GraphCore, node_id: &str) -> Option<PolicySourceBinding> {
    let blob = core.get_node_properties(node_id)?;
    let mut fields = medulla_types::msgpack::decode_property_object(&blob).ok()?;
    fields.remove("type");
    serde_json::from_value(serde_json::Value::Object(fields)).ok()
}

/// The write one accepted bind needs: a create when no record exists, else a
/// compare-and-set conditioned on the stored `source_revision`.
fn write_method(
    node_id: &str,
    binding: &PolicySourceBinding,
    stored: Option<&PolicySourceBinding>,
) -> Result<Method, String> {
    let encode = |value: &serde_json::Map<String, serde_json::Value>| {
        rmp_serde::to_vec_named(value)
            .map_err(|error| format!("INVALID_ARGUMENT: policy source encoding failed: {error}"))
    };
    let properties = record_properties(binding)?;
    Ok(match stored {
        None => Method::CreateNodeIfAbsent {
            node_id: node_id.to_string(),
            properties_msgpack: encode(&properties)?,
        },
        Some(stored) => {
            let mut conditions = serde_json::Map::new();
            conditions.insert(
                "source_revision".to_string(),
                serde_json::Value::String(stored.source_revision.clone()),
            );
            Method::CompareAndSetNodeFields {
                node_id: node_id.to_string(),
                conditions_msgpack: encode(&conditions)?,
                updates_msgpack: encode(&properties)?,
            }
        }
    })
}

fn answer(req_id: u64, binding: PolicySourceBinding) -> Response {
    match ResultPayload::of::<medulla_protocol_types::result_contract::graph::PolicySourceBind>(
        binding,
    ) {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => Response::err(req_id, error),
    }
}

/// The head revision of the policy component under the tenant, when one exists.
#[cfg(feature = "redb")]
async fn read_head(
    state: &Arc<RwLock<ServerState>>,
    tenant_id: &str,
    policy_iri: &str,
) -> Result<Option<AgentComponentEntry>, String> {
    use medulla_persistence_records::agent_library::AgentLibraryStateExt;
    let Some(store) = state.read().await.agent_library() else {
        return Ok(None);
    };
    store.current_component(tenant_id, policy_iri)
}

#[cfg(not(feature = "redb"))]
async fn read_head(
    _state: &Arc<RwLock<ServerState>>,
    _tenant_id: &str,
    _policy_iri: &str,
) -> Result<Option<AgentComponentEntry>, String> {
    Ok(None)
}

/// Serve `PolicySourceBind` on the gateway: read the head, decide, and commit
/// the one activation node. A re-bind of the same review answers the stored
/// record and writes nothing.
pub async fn handle_policy_source_bind(
    state: &Arc<RwLock<ServerState>>,
    ctx: &MutationCtx<'_>,
    tenant_id: &str,
    method: &Method,
) -> Response {
    let Method::PolicySourceBind { request } = method else {
        return Response::err(ctx.req_id, "INVALID_ARGUMENT: not a PolicySourceBind");
    };
    let head = match read_head(state, tenant_id, &request.policy_iri).await {
        Ok(head) => head,
        Err(error) => return Response::err(ctx.req_id, error),
    };
    let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let binding = match bind(request, head.as_ref(), now_ms) {
        Ok(binding) => binding,
        Err(refusal) => return Response::err(ctx.req_id, refusal.to_string()),
    };
    let node_id = policy_source_node_id(&request.policy_iri);
    let stored = stored_record(ctx.core, &node_id);
    if let Some(stored) = stored
        .as_ref()
        .filter(|stored| stored.same_review(&binding))
    {
        return answer(ctx.req_id, stored.clone());
    }
    let write = match write_method(&node_id, &binding, stored.as_ref()) {
        Ok(write) => write,
        Err(error) => return Response::err(ctx.req_id, error),
    };
    let plan = MutationPlan::for_method(&write);
    let apply_write = write.clone();
    medulla_server_ops::api::ingest::graph::commit_gateway(ctx, &plan, &write, move |core| {
        if !apply_node_write(core, &apply_write)? {
            return Err(PolicySourceRefusal::new(
                ServerErrorCode::PolicySourceStale,
                "the activation record changed since the bind was read",
            )
            .to_string());
        }
        ResultPayload::of::<medulla_protocol_types::result_contract::graph::PolicySourceBind>(
            binding,
        )
    })
    .await
}

/// Apply the one node write under the graph lane. `false` means a competing
/// bind committed first: the create found the node, or the compare-and-set
/// found another `source_revision`.
fn apply_node_write(core: &GraphCore, write: &Method) -> Result<bool, String> {
    match write {
        Method::CreateNodeIfAbsent {
            node_id,
            properties_msgpack,
        } => Ok(core.create_node_if_absent(node_id.clone(), properties_msgpack.clone())),
        Method::CompareAndSetNodeFields {
            node_id,
            conditions_msgpack,
            updates_msgpack,
        } => {
            let conditions = medulla_types::msgpack::decode_property_object(conditions_msgpack)
                .map_err(|_| {
                    "INVALID_ARGUMENT: policy source condition is malformed".to_string()
                })?;
            let updates = medulla_types::msgpack::decode_property_object(updates_msgpack)
                .map_err(|_| "INVALID_ARGUMENT: policy source record is malformed".to_string())?;
            Ok(core.compare_and_set_fields(node_id, &conditions, &updates))
        }
        _ => Err("INVALID_ARGUMENT: not a policy source node write".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";
    const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn head(version: &str) -> AgentComponentEntry {
        serde_json::from_value(serde_json::json!({
            "schema_version": 3,
            "component_id": "policy-a",
            "kind": "decision_policy",
            "version": version,
            "content_digest": format!("sha256:{DIGEST}"),
            "content_ref": format!("source-a@{REVISION}"),
            "facts": {"facts": "opaque"},
            "provenance": {"origin": "native"},
            "summary": "policy",
            "tenant_id": "tenant-a",
            "actor_scope": "principal:sha256:importer",
            "purpose_id": "purpose",
            "policy_digest": format!("sha256:{DIGEST}"),
            "source_revision": "connector-pack",
            "source_revision_digest": format!("sha256:{DIGEST}"),
            "entry_revision": 1,
            "lifecycle": "published",
            "definition_digest": format!("sha256:{DIGEST}"),
            "created_at_ms": 1,
            "updated_at_ms": 2,
        }))
        .expect("fixture is a valid component entry")
    }

    fn request() -> PolicySourceBind {
        PolicySourceBind {
            source_ref: "source-a".to_string(),
            source_revision: REVISION.to_string(),
            source_digest: DIGEST.to_string(),
            policy_iri: "policy-a".to_string(),
            component_version: "2".to_string(),
            approver: "reviewer".to_string(),
        }
    }

    fn code(result: Result<PolicySourceBinding, PolicySourceRefusal>) -> &'static str {
        result.expect_err("the bind is refused").code
    }

    // proves R801: Reviewed policy source bound to the active policy
    // spec: MEDULLA-SRC-AUTH-R801
    #[test]
    fn bind_refuses_in_order() {
        let head = head("2");
        // Every refusal at once: the unreviewed one wins.
        let mut all_wrong = request();
        all_wrong.approver = String::new();
        all_wrong.component_version = "1".to_string();
        all_wrong.source_digest = "b".repeat(64);
        assert_eq!(code(bind(&all_wrong, None, 5)), "POLICY_SOURCE_UNREVIEWED");

        // No component, or a component of another kind: invalid.
        assert_eq!(code(bind(&request(), None, 5)), "INVALID_ARGUMENT");
        let mut other_kind = head.clone();
        other_kind.kind = AgentComponentKind::Tool;
        assert_eq!(
            code(bind(&request(), Some(&other_kind), 5)),
            "INVALID_ARGUMENT"
        );

        // A head newer than the reviewed one is stale, even with a wrong digest.
        let mut stale = request();
        stale.component_version = "1".to_string();
        stale.source_digest = "b".repeat(64);
        assert_eq!(code(bind(&stale, Some(&head), 5)), "POLICY_SOURCE_STALE");

        // A digest, source or revision the head does not hold is a mismatch.
        let mut digest = request();
        digest.source_digest = "b".repeat(64);
        assert_eq!(
            code(bind(&digest, Some(&head), 5)),
            "POLICY_SOURCE_DIGEST_MISMATCH"
        );
        let mut source = request();
        source.source_ref = "source-b".to_string();
        assert_eq!(
            code(bind(&source, Some(&head), 5)),
            "POLICY_SOURCE_DIGEST_MISMATCH"
        );
        let mut revision = request();
        revision.source_revision = "f".repeat(40);
        assert_eq!(
            code(bind(&revision, Some(&head), 5)),
            "POLICY_SOURCE_DIGEST_MISMATCH"
        );

        // A match activates, stamping the engine time.
        let binding = bind(&request(), Some(&head), 7).expect("the review matches the head");
        assert_eq!(binding.policy_iri, "policy-a");
        assert_eq!(binding.approver, "reviewer");
        assert_eq!(binding.bound_at_ms, 7);
    }

    // spec: MEDULLA-SRC-AUTH-R801
    #[test]
    fn rebind_of_the_same_review_is_the_same_record() {
        let head = head("2");
        let first = bind(&request(), Some(&head), 7).unwrap();
        let again = bind(&request(), Some(&head), 99).unwrap();
        assert!(first.same_review(&again));
        let mut other = again.clone();
        other.approver = "someone-else".to_string();
        assert!(!first.same_review(&other));
    }
}
