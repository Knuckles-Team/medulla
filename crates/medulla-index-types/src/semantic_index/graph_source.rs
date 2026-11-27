//! The binding coordinates of a graph text-property source.
//!
//! A SQL source reads its schema, field-set and ACL digests from the SQL catalog. A graph source
//! has no catalog, so its coordinates are one deterministic function of the selector: the author
//! of a binding and the engine's graph source read compute the same values from the same
//! [`GraphTextPropertyRef`], and a binding made over a different selector cannot match.
//!
//! The row ACL of a graph source is the caller's row visibility, evaluated when the graph is
//! read, so the ACL revision is the constant [`SEMANTIC_GRAPH_SOURCE_ACL_REVISION`].

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{OnceLock, RwLock};

use super::digest::{cbor, domain_digest, SemanticDigest};
use super::selector_ref::GraphTextPropertyRef;

/// The ACL revision of every graph source binding.
pub const SEMANTIC_GRAPH_SOURCE_ACL_REVISION: u64 = 1;

const COORDINATES_DOMAIN: &[u8] = b"au-eg/semantic-graph-source-coordinates/v1";
const AUTHORITY_DOMAIN: &[u8] = b"au-eg/semantic-graph-source-authority/v1";
const RECORD_DOMAIN: &[u8] = b"au-eg/semantic-graph-source-record/v1";

/// The schema, field-set and ACL digests a graph source binding carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphSourceCoordinates {
    pub schema_digest: String,
    pub field_set_digest: String,
    pub acl_digest: String,
}

impl GraphTextPropertyRef {
    fn coordinate(&self, part: &'static str) -> String {
        domain_digest(
            COORDINATES_DOMAIN,
            cbor::map([
                ("part", cbor::text(part)),
                ("graph_id", cbor::text(&self.graph_id)),
                ("type_id", cbor::text(&self.type_id)),
                ("property_id", cbor::text(&self.property_id)),
            ]),
        )
        .to_string()
    }

    /// The `source_schema_digest`, `source_field_set_digest` and ACL digest of a binding over
    /// this selector.
    pub fn coordinates(&self) -> GraphSourceCoordinates {
        GraphSourceCoordinates {
            schema_digest: self.coordinate("schema"),
            field_set_digest: self.coordinate("field_set"),
            acl_digest: self.coordinate("acl"),
        }
    }

    /// The digest of the source authority of this selector under one tenant scope: the lineage
    /// of every `graph-source:<digest>:epoch:<n>` revision of a binding over it.
    pub fn authority_digest(&self, tenant_scope: &str) -> SemanticDigest {
        domain_digest(
            AUTHORITY_DOMAIN,
            cbor::map([
                ("tenant_scope", cbor::text(tenant_scope)),
                ("graph_id", cbor::text(&self.graph_id)),
                ("type_id", cbor::text(&self.type_id)),
                ("property_id", cbor::text(&self.property_id)),
            ]),
        )
    }

    /// The stable record identity of one node: the digest of its node id, so a source entity id
    /// is deterministic from the node id.
    pub fn record_identity_digest(node_id: &str) -> SemanticDigest {
        domain_digest(RECORD_DOMAIN, cbor::text(node_id))
    }
}

/// The `(type, property)` pairs bound to a semantic index, by `(tenant, graph)`.
type BoundSources = BTreeMap<(String, String), BTreeSet<(String, String)>>;

fn registry() -> &'static RwLock<BoundSources> {
    static REGISTRY: OnceLock<RwLock<BoundSources>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

/// Record that a semantic index of `tenant` is built from `selector`, so the graph commit
/// chokepoint knows which commits to wake it for. A binding that is later dropped may leave its
/// entry behind: the wakeup is a hint and the reconcile reads the source, so an extra wakeup
/// costs one read and never changes an answer.
pub fn bind_graph_source(tenant: &str, selector: &GraphTextPropertyRef) {
    if let Ok(mut bound) = registry().write() {
        bound
            .entry((tenant.to_string(), selector.graph_id.clone()))
            .or_default()
            .insert((selector.type_id.clone(), selector.property_id.clone()));
    }
}

/// The `(type, property)` pairs of `graph` that a semantic index of `tenant` is built from.
pub fn bound_graph_sources(tenant: &str, graph: &str) -> Vec<(String, String)> {
    registry()
        .read()
        .map(|bound| {
            bound
                .get(&(tenant.to_string(), graph.to_string()))
                .map(|pairs| pairs.iter().cloned().collect())
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic_index::SemanticSqlSourceIdentity;

    #[test]
    fn a_bound_source_is_reported_for_its_tenant_and_graph_only() {
        let selector = GraphTextPropertyRef {
            graph_id: "registry-test-graph".into(),
            type_id: "Bookmark".into(),
            property_id: "text".into(),
        };
        assert!(bound_graph_sources("registry-test-a", "registry-test-graph").is_empty());
        bind_graph_source("registry-test-a", &selector);
        bind_graph_source("registry-test-a", &selector);
        assert_eq!(
            bound_graph_sources("registry-test-a", "registry-test-graph"),
            vec![("Bookmark".to_string(), "text".to_string())]
        );
        assert!(bound_graph_sources("registry-test-b", "registry-test-graph").is_empty());
        assert!(bound_graph_sources("registry-test-a", "other-graph").is_empty());
    }

    fn selector(property: &str) -> GraphTextPropertyRef {
        GraphTextPropertyRef {
            graph_id: "media".into(),
            type_id: "Bookmark".into(),
            property_id: property.into(),
        }
    }

    #[test]
    fn coordinates_are_deterministic_and_selector_bound() {
        assert_eq!(
            selector("text").coordinates(),
            selector("text").coordinates()
        );
        assert_ne!(
            selector("text").coordinates(),
            selector("body").coordinates()
        );
        let coordinates = selector("text").coordinates();
        assert_ne!(coordinates.schema_digest, coordinates.field_set_digest);
        assert_ne!(coordinates.schema_digest, coordinates.acl_digest);
        assert!(coordinates.schema_digest.starts_with("sha256:"));
    }

    #[test]
    fn authority_is_tenant_and_selector_bound() {
        let text = selector("text");
        assert_ne!(text.authority_digest("a"), text.authority_digest("b"));
        assert_ne!(
            text.authority_digest("a"),
            selector("body").authority_digest("a")
        );
    }

    #[test]
    fn a_node_id_names_exactly_one_source_entity() {
        let text = selector("text");
        let entity = |node: &str| {
            SemanticSqlSourceIdentity::create_graph(
                &text,
                "a",
                GraphTextPropertyRef::record_identity_digest(node),
            )
            .source_entity_id()
        };
        assert_eq!(entity("bookmark:1"), entity("bookmark:1"));
        assert_ne!(entity("bookmark:1"), entity("bookmark:2"));
    }
}
