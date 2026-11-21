//! The production `bookmark` semantic binding (MEDULLA-MEDIA-VOCAB-R001.102.2).
//!
//! The connector pack that declares the `Bookmark` class activates a graph-sourced semantic index
//! over the `text` property of `Bookmark` nodes. The router reads what the binding needs when
//! the pack activates (the activating request's graph, the bound text embedder's registered
//! space and the isolation layer's policy revision) and calls [`author_bookmark_binding`], which
//! admits the binding once and records it in the graph-source registry. Authoring is idempotent:
//! a tenant that already has a `bookmark` binding keeps it.

use std::sync::Arc;

use medulla_index_types::embedding::EmbeddingSpaceRef;
use medulla_index_types::semantic_index::{
    GraphTextPropertyRef, SemanticAnnIndexMethod, SemanticAnnIndexSpec, SemanticBinding,
    SemanticBindingDraft, SemanticLexicalIndexSpec, SemanticModelIdentity,
    SemanticPolicyComponents, SemanticSourceKind, SemanticSourceSelector, SemanticVectorMetric,
    SEMANTIC_GRAPH_SOURCE_ACL_REVISION,
};
use medulla_server_api::carrier::CarrierAuthority;
use medulla_server_api::state::ServerState;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::bookmark::BOOKMARK_INDEX_BINDING_ID;

/// The node type the bookmark index is built from.
pub const BOOKMARK_NODE_TYPE: &str = "Bookmark";
/// The node property the bookmark index embeds.
pub const BOOKMARK_TEXT_PROPERTY: &str = "text";

/// What the router read at pack activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookmarkBindingInputs {
    /// The graph of the activating request: the index reads `Bookmark` nodes of this graph.
    pub graph_id: String,
    /// The bound text embedder's pinned space, from `pin_embedding_space`: its `digest` is the
    /// binding's `model_digest`, which the S6 image's own space must equal.
    pub space: EmbeddingSpaceRef,
    /// The isolation layer's policy revision at authoring time.
    pub policy_revision: u64,
    /// The engine clock at authoring time.
    pub now_ms: u64,
}

fn sha256_label(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

/// The `bookmark` binding of `authority`'s tenant over `inputs`.
pub fn bookmark_binding(
    authority: &CarrierAuthority,
    inputs: &BookmarkBindingInputs,
) -> Result<SemanticBinding, String> {
    let tenant = authority.tenant_scope();
    let selector = GraphTextPropertyRef {
        graph_id: inputs.graph_id.clone(),
        type_id: BOOKMARK_NODE_TYPE.to_string(),
        property_id: BOOKMARK_TEXT_PROPERTY.to_string(),
    };
    let coordinates = selector.coordinates();
    let source_revision = SemanticSourceKind::Graph
        .revision_for_epoch(selector.authority_digest(tenant), 1)
        .map_err(|_| "ENGINE_UNAVAILABLE: bookmark source revision".to_string())?;
    let dimension = u32::try_from(inputs.space.dimensions)
        .map_err(|_| "INVALID_ARGUMENT: bookmark embedding dimension".to_string())?;
    // One policy image backs both policy components; the digests name the image revision.
    let policy_revision = inputs.policy_revision.max(1);
    let revision_text = policy_revision.to_string();
    let space = &inputs.space;
    let draft = SemanticBindingDraft {
        binding_id: BOOKMARK_INDEX_BINDING_ID.to_string(),
        tenant_id: tenant.to_string(),
        actor_scope: authority.actor_scope().to_string(),
        effective_actor_scope: authority.agent_id().to_string(),
        purpose_id: "retrieval".to_string(),
        policy: SemanticPolicyComponents {
            rbac_policy_revision: policy_revision,
            rbac_policy_digest: sha256_label(&["isolation-rbac-policy", tenant, &revision_text]),
            row_policy_revision: policy_revision,
            row_policy_digest: sha256_label(&["isolation-row-policy", tenant, &revision_text]),
            source_acl_revision: SEMANTIC_GRAPH_SOURCE_ACL_REVISION,
            source_acl_digest: coordinates.acl_digest,
        },
        source_selector: SemanticSourceSelector::GraphTextPropertyRef(selector),
        source_schema_digest: coordinates.schema_digest,
        source_revision,
        source_field_set_digest: coordinates.field_set_digest,
        dimension,
        metric: SemanticVectorMetric::Cosine,
        model: SemanticModelIdentity {
            model_id: space.id.clone(),
            model_revision: space.revision.clone(),
            preprocess_digest: space.preprocessing_hash.clone(),
            model_digest: space.digest.clone(),
        },
        generation: 1,
        maintenance_policy_id: "semantic-maintenance".to_string(),
        lexical_index: SemanticLexicalIndexSpec {
            analyzer_id: "standard".to_string(),
            analyzer_revision: "1".to_string(),
            analyzer_config_digest: sha256_label(&["lexical-analyzer", "standard", "1"]),
        },
        ann_index: SemanticAnnIndexSpec {
            method: SemanticAnnIndexMethod::IvfPq,
            parameters_digest: sha256_label(&["ann-parameters", "ivf_pq", "default"]),
        },
        created_at: format!("epoch-ms:{}", inputs.now_ms),
    };
    SemanticBinding::create(draft)
        .map_err(|_| "INVALID_ARGUMENT: bookmark binding rejected".to_string())
}

/// Admit the `bookmark` binding of `authority`'s tenant when it has none, and record it in the
/// graph-source registry so a commit of a `Bookmark` node wakes its index. Returns whether this
/// call created the binding.
pub async fn author_bookmark_binding(
    state: &Arc<RwLock<ServerState>>,
    authority: &CarrierAuthority,
    inputs: &BookmarkBindingInputs,
) -> Result<bool, String> {
    let persist_dir = {
        let guard = state.read().await;
        guard.persist_dir.as_deref().map(std::path::PathBuf::from).ok_or_else(|| {
            "ENGINE_UNAVAILABLE: semantic index storage is unavailable".to_string()
        })?
    };
    let service = crate::semantic_index_runtime::open_semantic_service(
        &persist_dir,
        authority.tenant_scope(),
        BOOKMARK_INDEX_BINDING_ID,
    )
    .map_err(|_| "ENGINE_UNAVAILABLE: semantic index storage is unavailable".to_string())?;
    let existing = {
        let service = Arc::clone(&service);
        tokio::task::spawn_blocking(move || service.binding())
            .await
            .map_err(|_| "ENGINE_UNAVAILABLE: semantic index task failed".to_string())?
            .map_err(|error| error.to_string())?
    };
    let (binding, created) = match existing {
        Some(binding) => (binding, false),
        None => {
            let binding = bookmark_binding(authority, inputs)?;
            let admitted = binding.clone();
            let now_ms = inputs.now_ms;
            let service = Arc::clone(&service);
            tokio::task::spawn_blocking(move || service.admit_binding(&admitted, now_ms))
                .await
                .map_err(|_| "ENGINE_UNAVAILABLE: semantic index task failed".to_string())?
                .map_err(|error| error.to_string())?;
            (binding, true)
        }
    };
    if let SemanticSourceSelector::GraphTextPropertyRef(selector) = &binding.source_selector {
        {
            let guard = state.read().await;
            crate::semantic_index_runtime::record_graph_source_binding(
                &guard,
                &binding.tenant_id,
                &binding.binding_id,
                selector,
            )?;
        }
        medulla_index_types::semantic_index::bind_graph_source(&binding.tenant_id, selector);
    }
    Ok(created)
}
