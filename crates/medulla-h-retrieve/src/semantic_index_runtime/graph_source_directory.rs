//! The durable directory of graph-sourced semantic bindings (MEDULLA-MEDIA-VOCAB-R001.102.2).
//!
//! The graph commit chokepoint decides which commits wake a semantic index from the
//! bound-source registry of `medulla-index-types`, which lives in memory. An owner file is named
//! by a digest of its tenant and binding id, so the stored bindings cannot be listed by walking
//! the semantic-index directory: the tenant and binding id of every graph-sourced binding are
//! kept in this directory file, written when the binding is first registered. At startup
//! [`rebuild_graph_source_registry`] opens each recorded binding, reads the selector the owner
//! file stores (the directory is only where to look, never the authority) and registers it, so a
//! commit made before the first semantic operation of a restarted process still wakes its index.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use medulla_index_types::semantic_index::{bind_graph_source, SemanticSourceSelector};
use serde::{Deserialize, Serialize};

const DIRECTORY_FILE: &str = "graph-source-bindings.json";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectoryEntry {
    tenant: String,
    binding_id: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Directory {
    entries: BTreeSet<DirectoryEntry>,
}

fn directory_path(persist_dir: &Path) -> PathBuf {
    persist_dir.join("semantic-index").join(DIRECTORY_FILE)
}

fn write_lock() -> &'static Mutex<()> {
    static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(Default::default)
}

fn read_directory(persist_dir: &Path) -> Result<Directory, String> {
    match std::fs::read(directory_path(persist_dir)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|_| "graph source binding directory is unreadable".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Directory::default()),
        Err(_) => Err("graph source binding directory is unavailable".to_string()),
    }
}

/// Record that `binding_id` of `tenant` is built from a graph property. Idempotent: a binding
/// already recorded rewrites nothing.
pub(crate) fn record_graph_source_binding(
    persist_dir: &Path,
    tenant: &str,
    binding_id: &str,
) -> Result<(), String> {
    let _guard = write_lock()
        .lock()
        .map_err(|_| "graph source binding directory is locked".to_string())?;
    let mut directory = read_directory(persist_dir)?;
    let entry = DirectoryEntry {
        tenant: tenant.to_string(),
        binding_id: binding_id.to_string(),
    };
    if !directory.entries.insert(entry) {
        return Ok(());
    }
    let path = directory_path(persist_dir);
    let parent = path
        .parent()
        .ok_or_else(|| "graph source binding directory has no parent".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|_| "graph source binding directory is unavailable".to_string())?;
    let encoded = serde_json::to_vec(&directory)
        .map_err(|_| "graph source binding directory is unencodable".to_string())?;
    let staging = path.with_extension("json.tmp");
    let mut file = std::fs::File::create(&staging)
        .map_err(|_| "graph source binding directory is unavailable".to_string())?;
    file.write_all(&encoded)
        .and_then(|()| file.sync_all())
        .map_err(|_| "graph source binding directory is unavailable".to_string())?;
    std::fs::rename(&staging, &path)
        .map_err(|_| "graph source binding directory is unavailable".to_string())
}

/// Register every recorded graph-sourced binding with the bound-source registry, reading each
/// binding's selector from its owner file. Returns how many bindings were registered. A recorded
/// binding whose owner no longer opens, or is no longer graph sourced, is skipped: the directory
/// only says where to look. Run at startup, after the semantic owner activation is installed.
pub fn rebuild_graph_source_registry(persist_dir: &Path) -> Result<usize, String> {
    let directory = read_directory(persist_dir)?;
    let mut registered = 0;
    for entry in directory.entries {
        let Ok(service) =
            super::existing_semantic_service(persist_dir, &entry.tenant, &entry.binding_id)
        else {
            continue;
        };
        let Ok(Some(binding)) = service.binding() else {
            continue;
        };
        if let SemanticSourceSelector::GraphTextPropertyRef(selector) = &binding.source_selector {
            bind_graph_source(&binding.tenant_id, selector);
            registered += 1;
        }
    }
    Ok(registered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_index_types::semantic_index::{
        bound_graph_sources, GraphTextPropertyRef, SemanticAnnIndexMethod, SemanticAnnIndexSpec,
        SemanticBinding, SemanticBindingDraft, SemanticLexicalIndexSpec,
        SemanticModelIdentity, SemanticPolicyComponents, SemanticSourceKind,
        SemanticVectorMetric, SEMANTIC_GRAPH_SOURCE_ACL_REVISION,
    };

    const TENANT: &str = "restart-registry-tenant";
    const BINDING_ID: &str = "bookmark";

    fn selector() -> GraphTextPropertyRef {
        GraphTextPropertyRef {
            graph_id: "restart-registry-graph".into(),
            type_id: "Bookmark".into(),
            property_id: "text".into(),
        }
    }

    fn graph_binding() -> SemanticBinding {
        let selector = selector();
        let coordinates = selector.coordinates();
        let revision = SemanticSourceKind::Graph
            .revision_for_epoch(selector.authority_digest(TENANT), 1)
            .unwrap();
        SemanticBinding::create(SemanticBindingDraft {
            binding_id: BINDING_ID.into(),
            tenant_id: TENANT.into(),
            actor_scope: "semantic:index-maintainer".into(),
            effective_actor_scope: "semantic:agent:index-maintainer".into(),
            purpose_id: "retrieval".into(),
            policy: SemanticPolicyComponents {
                rbac_policy_revision: 1,
                rbac_policy_digest: "sha256:rbac".into(),
                row_policy_revision: 1,
                row_policy_digest: "sha256:row-policy".into(),
                source_acl_revision: SEMANTIC_GRAPH_SOURCE_ACL_REVISION,
                source_acl_digest: coordinates.acl_digest,
            },
            source_selector: SemanticSourceSelector::GraphTextPropertyRef(selector),
            source_schema_digest: coordinates.schema_digest,
            source_revision: revision,
            source_field_set_digest: coordinates.field_set_digest,
            dimension: 3,
            metric: SemanticVectorMetric::Cosine,
            model: SemanticModelIdentity {
                model_id: "embedding-model".into(),
                model_revision: "revision-1".into(),
                preprocess_digest: "sha256:preprocess".into(),
                model_digest: "sha256:model".into(),
            },
            generation: 1,
            maintenance_policy_id: "semantic-maintenance".into(),
            lexical_index: SemanticLexicalIndexSpec {
                analyzer_id: "standard".into(),
                analyzer_revision: "1".into(),
                analyzer_config_digest: "sha256:analyzer".into(),
            },
            ann_index: SemanticAnnIndexSpec {
                method: SemanticAnnIndexMethod::IvfPq,
                parameters_digest: "sha256:ann-parameters".into(),
            },
            created_at: "2026-11-21T00:00:00Z".into(),
        })
        .unwrap()
    }

    // spec: MEDULLA-MEDIA-VOCAB-R001.102.2
    #[test]
    fn a_restarted_process_rebuilds_the_bound_source_registry_from_the_stored_binding() {
        let dir = tempfile::tempdir().unwrap();
        let service = super::super::open_semantic_service(dir.path(), TENANT, BINDING_ID).unwrap();
        service.admit_binding(&graph_binding(), 1).unwrap();
        record_graph_source_binding(dir.path(), TENANT, BINDING_ID).unwrap();
        // Recording twice leaves one entry.
        record_graph_source_binding(dir.path(), TENANT, BINDING_ID).unwrap();
        assert_eq!(read_directory(dir.path()).unwrap().entries.len(), 1);

        // A new process starts with an empty registry; nothing above registered the source in
        // memory, so the registry is empty exactly as it is after a restart.
        assert!(bound_graph_sources(TENANT, "restart-registry-graph").is_empty());
        assert_eq!(rebuild_graph_source_registry(dir.path()).unwrap(), 1);
        assert_eq!(
            bound_graph_sources(TENANT, "restart-registry-graph"),
            vec![("Bookmark".to_string(), "text".to_string())]
        );
    }

    // spec: MEDULLA-MEDIA-VOCAB-R001.102.2
    #[test]
    fn a_recorded_binding_that_does_not_open_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        record_graph_source_binding(dir.path(), "restart-missing-tenant", "bookmark").unwrap();
        assert_eq!(rebuild_graph_source_registry(dir.path()).unwrap(), 0);
        assert_eq!(rebuild_graph_source_registry(Path::new("/nonexistent-persist")).unwrap(), 0);
    }
}
