//! The graph text-property source of a semantic index (MEDULLA-MEDIA-VOCAB-R001.102.2).
//!
//! A graph source is read through the same [`AuthorizedSqlSourceReadPort`] the S1 stage already
//! drives, so there is one admission, claim and completion path. The router, which owns the
//! caller's projected core, reads the visible nodes of the bound type once into a
//! [`GraphSourceSnapshot`]; the port pages that snapshot with the same cursor discipline as a SQL
//! snapshot: the page and its continuation are MAC-bound to the tenant, the selector and the
//! graph epoch, so a cursor never resumes against another snapshot.

use std::future::Future;
use std::pin::Pin;

pub use medulla_index_types::semantic_index::GraphTextPropertyRef;
use medulla_index_types::semantic_index::{SemanticSourceKind, SEMANTIC_GRAPH_SOURCE_ACL_REVISION};
use sha2::{Digest, Sha256};

use super::*;

/// The most nodes one graph source snapshot holds; a larger source is refused, never truncated.
pub const MAX_GRAPH_SOURCE_NODES: usize = 262_144;

/// Rows per source page: the bound of `SemanticSqlSourceReadPage`.
const PAGE_ROWS: usize = 256;

const CURSOR_DOMAIN: &[u8] = b"eg/semantic-graph-source-cursor/v1";
const COMPLETE_DOMAIN: &[u8] = b"eg/semantic-graph-source-complete/v1";
const DECISION_DOMAIN: &[u8] = b"eg/semantic-graph-source-decision/v1";

/// The nodes of one `(graph, type, property)` the caller may see, read once at one graph version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphSourceSnapshot {
    pub selector: GraphTextPropertyRef,
    /// `GraphCore::version()` read before the nodes were scanned.
    pub graph_version: u64,
    /// `(node id, text)` ordered by node id; nodes without the text property are absent.
    pub nodes: Vec<(String, String)>,
}

impl GraphSourceSnapshot {
    /// The source epoch: the graph version, never zero.
    pub fn epoch(&self) -> u64 {
        self.graph_version.max(1)
    }
}

pub type GraphSourceFuture<'a> =
    Pin<Box<dyn Future<Output = Result<GraphSourceSnapshot, String>> + Send + 'a>>;

/// The capability that reads a graph source snapshot for the verified caller. The router
/// implements it over the caller's projected core.
pub trait GraphSourceProvider: Sync {
    fn snapshot<'a>(&'a self, selector: &'a GraphTextPropertyRef) -> GraphSourceFuture<'a>;
}

/// The source entity id of one node under one tenant scope. The bookmark search maps a hit's
/// entity id back to its node id by computing this over the caller's visible nodes.
pub fn graph_source_entity_id(
    selector: &GraphTextPropertyRef,
    tenant_scope: &str,
    node_id: &str,
) -> String {
    SemanticSqlSourceIdentity::create_graph(
        selector,
        tenant_scope,
        GraphTextPropertyRef::record_identity_digest(node_id),
    )
    .source_entity_id()
}

impl AuthorizedSqlSourceReadPort {
    /// Serve the graph snapshot instead of a SQL table when the binding's selector is a graph
    /// text property.
    pub fn with_graph_source(mut self, snapshot: Option<Arc<GraphSourceSnapshot>>) -> Self {
        self.graph = snapshot;
        self
    }

    fn graph_cursor_mac(
        &self,
        authority: SemanticDigest,
        epoch: u64,
        offset: u64,
    ) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(CURSOR_DOMAIN);
        hasher.update(self.cursor_auth_secret);
        hasher.update(authority.to_string().as_bytes());
        hasher.update(epoch.to_be_bytes());
        hasher.update(offset.to_be_bytes());
        hasher.finalize().into()
    }

    pub(super) fn read_graph_snapshot(
        &self,
        binding: &SemanticBinding,
        selector: &GraphTextPropertyRef,
        cursor: Option<[u8; 40]>,
    ) -> Result<AuthorizedSqlSourcePage, SemanticIndexError> {
        let mismatch = SemanticIndexError::SourceManifestMismatch;
        let snapshot = self.graph.as_deref().ok_or_else(|| mismatch.clone())?;
        let coordinates = selector.coordinates();
        let acl = &binding.policy_identity.components;
        if &snapshot.selector != selector
            || coordinates.schema_digest != binding.source_schema_digest
            || coordinates.field_set_digest != binding.source_field_set_digest
            || coordinates.acl_digest != acl.source_acl_digest
            || acl.source_acl_revision != SEMANTIC_GRAPH_SOURCE_ACL_REVISION
        {
            return Err(mismatch);
        }
        let authority = selector.authority_digest(&binding.tenant_id);
        let epoch = snapshot.epoch();
        let offset = match cursor {
            None => 0usize,
            Some(bytes) => {
                let (offset_bytes, mac) = bytes.split_at(8);
                let offset = u64::from_be_bytes(
                    offset_bytes
                        .try_into()
                        .map_err(|_| SemanticIndexError::SourceManifestMismatch)?,
                );
                if self.graph_cursor_mac(authority, epoch, offset)[..] != *mac {
                    return Err(SemanticIndexError::SourceManifestMismatch);
                }
                usize::try_from(offset).map_err(|_| SemanticIndexError::SourceManifestMismatch)?
            }
        };
        if offset > snapshot.nodes.len() {
            return Err(SemanticIndexError::SourceManifestMismatch);
        }
        let source_revision = SemanticSourceKind::Graph.revision_for_epoch(authority, epoch)?;
        let end = (offset + PAGE_ROWS).min(snapshot.nodes.len());
        let authorization_receipt_digest = {
            let mut hasher = Sha256::new();
            hasher.update(DECISION_DOMAIN);
            hasher.update(binding.tenant_id.as_bytes());
            hasher.update([0]);
            hasher.update(binding.effective_actor_scope.as_bytes());
            hasher.update([0]);
            hasher.update(authority.to_string().as_bytes());
            hasher.update(epoch.to_be_bytes());
            SemanticDigest::from_bytes(hasher.finalize().into())
        };
        let sources = snapshot.nodes[offset..end]
            .iter()
            .map(|(node_id, text)| SemanticSqlSourceRecord {
                source_identity: SemanticSqlSourceIdentity::create_graph(
                    selector,
                    binding.tenant_id.clone(),
                    GraphTextPropertyRef::record_identity_digest(node_id),
                ),
                source_revision: source_revision.clone(),
                value: SemanticSqlSourceValue::Present {
                    source_bytes: text.clone().into_bytes(),
                },
                source_schema_revision: 1,
                source_schema_digest: coordinates.schema_digest.clone(),
                source_field_set_digest: coordinates.field_set_digest.clone(),
                source_acl_revision: SEMANTIC_GRAPH_SOURCE_ACL_REVISION,
                source_acl_digest: coordinates.acl_digest.clone(),
                authorization_receipt_digest,
            })
            .collect();
        let complete = end == snapshot.nodes.len();
        let next_cursor = (!complete).then(|| {
            let mut bytes = Vec::with_capacity(40);
            bytes.extend_from_slice(&(end as u64).to_be_bytes());
            bytes.extend_from_slice(&self.graph_cursor_mac(authority, epoch, end as u64));
            bytes
        });
        let complete_snapshot_receipt_digest = complete.then(|| {
            let mut hasher = Sha256::new();
            hasher.update(COMPLETE_DOMAIN);
            hasher.update(authority.to_string().as_bytes());
            hasher.update(epoch.to_be_bytes());
            for (node_id, _) in &snapshot.nodes {
                hasher.update(node_id.as_bytes());
                hasher.update([0]);
            }
            SemanticDigest::from_bytes(hasher.finalize().into())
        });
        Ok(AuthorizedSqlSourcePage {
            page: SemanticSqlSourceReadPage {
                source_revision,
                complete_snapshot_receipt_digest,
                sources,
                next_cursor,
                complete,
            },
            decision_at_ms: medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
            source_schema_revision: 1,
            authorization_receipt_digest,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(count: usize) -> GraphSourceSnapshot {
        GraphSourceSnapshot {
            selector: GraphTextPropertyRef {
                graph_id: "media".into(),
                type_id: "Bookmark".into(),
                property_id: "text".into(),
            },
            graph_version: 0,
            nodes: (0..count)
                .map(|index| (format!("bookmark:{index:05}"), format!("text {index}")))
                .collect(),
        }
    }

    #[test]
    fn the_epoch_is_the_graph_version_and_never_zero() {
        let mut snapshot = snapshot(1);
        assert_eq!(snapshot.epoch(), 1);
        snapshot.graph_version = 9;
        assert_eq!(snapshot.epoch(), 9);
    }

    #[test]
    fn a_node_id_names_one_entity_per_tenant() {
        let selector = snapshot(0).selector;
        assert_eq!(
            graph_source_entity_id(&selector, "a", "bookmark:1"),
            graph_source_entity_id(&selector, "a", "bookmark:1")
        );
        assert_ne!(
            graph_source_entity_id(&selector, "a", "bookmark:1"),
            graph_source_entity_id(&selector, "b", "bookmark:1")
        );
        assert!(graph_source_entity_id(&selector, "a", "bookmark:1")
            .starts_with("semantic-graph-source:"));
    }
}
