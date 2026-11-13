//! EH-280 — durable projection of a branch-aware `IndexRepository` batch.
//!
//! A scoped result (symbols anchored on `:Blob`, `:FileVersion`, and
//! `:Branch -hasFileVersion-> :FileVersion` membership) is lowered to graph-row
//! methods and committed as ONE governed ChangeEnvelope through the existing
//! `ApplyChangeEnvelope` authority -- the same single commit path SourceIngest
//! uses, so there is no second graph writer. A tombstone lowers to the removal
//! of its membership edge. The envelope id is the digest of the lowered
//! write-set, so re-indexing an unchanged batch replays instead of committing
//! twice, and every node/edge write is an upsert. The envelope is classified
//! `MaterialClass::RepositorySnapshot`: symbol names, repository-relative paths
//! and code text are screened for HOST identity (absolute host paths, file URIs,
//! e-mail addresses), not blanket-rejected for `@` or a `/home/` path segment.

use std::collections::{BTreeMap, BTreeSet};

use medulla_protocol_types::{
    change_envelope::ChangeEnvelope, change_envelope::ContentVersion,
    change_envelope::ContentVersionPosition, change_envelope::MaterialClass,
    change_envelope::MaterialOperation, change_envelope::PolicyRecord,
    change_envelope::PrivacyAttestation, change_envelope::CHANGE_ENVELOPE_VERSION,
};
use medulla_types::contract::Digest256;
use medulla_types::ingestion_wire::{ExtractedEdge, ExtractedNode, IndexResult};

use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;

pub mod delta;
pub mod ecosystem;
pub mod enrich;
pub mod scope_facts;

/// The projection edge a tombstone is reported as; it lowers to a removal.
const TOMBSTONE_EDGE: &str = "removesFileVersion";
const ENVELOPE_PREFIX: &str = "repository-index:";
const VERSION_TYPE: &str = "repository-index-sha256";
const POLICY_VERSION: &str = "repository-index";

/// The verified request coordinates one scoped commit is compiled under.
pub type IndexCommitContext<'a> = medulla_server_api::batch_compile::GraphWriteScope<'a>;

/// A batch that lowers past the mutation batch's operation budget is refused
/// with this code BEFORE compilation: it commits as ONE atomic envelope or not
/// at all, and the source transport halves it and re-sends.
pub const BATCH_TOO_LARGE: &str = "REPOSITORY_BATCH_TOO_LARGE";

/// Encoded write-set bounds, under the durable ledger's per-record budget
/// (64 MiB / 1 000 000 MessagePack items) with room for the envelope.
const MAX_WRITE_SET_BYTES: usize = 48 * 1024 * 1024;
const MAX_WRITE_SET_ITEMS: usize = 900_000;
const RECORD_ITEMS_PER_OPERATION: usize = 10;

/// The lowered, digest-keyed write-set of one scoped result.
#[derive(Debug)]
pub struct IndexWriteSet {
    pub envelope_id: String,
    digest: Digest256,
    pub methods: Vec<Method>,
}

/// What a property value holding host identity is stored as.
pub const REDACTED_HOST_IDENTITY: &str = "[redacted: host identity]";

/// The value as stored: repository content may quote an e-mail address or an
/// absolute host path (a `pyproject.toml` author, a docstring). The
/// repository-snapshot rule refuses such text, and one quoted address must not
/// refuse a whole repository batch, so the VALUE is replaced; the rule itself
/// is unchanged and still screens every committed value.
pub(crate) fn stored_value(value: &str) -> &str {
    if medulla_protocol_types::change_envelope::admits_repository_text(value) {
        value
    } else {
        REDACTED_HOST_IDENTITY
    }
}

/// Properties are re-keyed into a `BTreeMap` so the encoding -- and therefore
/// the envelope id -- never depends on hash-map iteration order.
fn encode_properties(
    properties: &std::collections::HashMap<String, String>,
    kind_key: &str,
    kind: &str,
) -> Result<Vec<u8>, String> {
    let mut ordered: BTreeMap<&str, &str> = properties
        .iter()
        .map(|(key, value)| (key.as_str(), stored_value(value)))
        .collect();
    ordered.insert(kind_key, kind);
    rmp_serde::to_vec_named(&ordered)
        .map_err(|error| format!("repository index property encoding failed: {error}"))
}

fn node_method(node: &ExtractedNode) -> Result<Method, String> {
    Ok(Method::AddNode {
        node_id: node.node_id.clone(),
        properties_msgpack: encode_properties(&node.properties, "type", &node.node_type)?,
    })
}

fn edge_method(edge: &ExtractedEdge) -> Result<Method, String> {
    if edge.edge_type == TOMBSTONE_EDGE {
        return Ok(Method::RemoveEdge {
            source_id: edge.source.clone(),
            target_id: edge.target.clone(),
        });
    }
    Ok(Method::AddEdge {
        source_id: edge.source.clone(),
        target_id: edge.target.clone(),
        properties_msgpack: encode_properties(&edge.properties, "relationship", &edge.edge_type)?,
    })
}

/// Lower a scoped result to its write-set. The graph admits parallel edges
/// between one endpoint pair when their `relationship` properties differ, so
/// keep one method per `(source, target, relation_type)` triple. This preserves
/// the semantic-bridge relations that repository hydration emits while still
/// suppressing duplicate resolver output for the same triple.
pub fn lower(result: &IndexResult) -> Result<IndexWriteSet, String> {
    let mut methods = result
        .nodes
        .iter()
        .map(node_method)
        .collect::<Result<Vec<_>, _>>()?;
    let mut edge_keys = BTreeSet::new();
    for edge in &result.edges {
        if edge_keys.insert((
            edge.source.as_str(),
            edge.target.as_str(),
            edge.edge_type.as_str(),
        )) {
            methods.push(edge_method(edge)?);
        }
    }
    IndexWriteSet::from_methods(ENVELOPE_PREFIX, b"eg/repository-index-batch", methods)
}

impl IndexWriteSet {
    /// The lowered methods, in commit order.
    pub fn methods(&self) -> &[Method] {
        &self.methods
    }

    /// Digest-key an engine-lowered write-set: the envelope id is `prefix`
    /// followed by the framed digest of the encoded methods, so an unchanged
    /// write-set replays. Refused whole past one commit's budget.
    pub(crate) fn from_methods(
        prefix: &str,
        domain: &[u8],
        methods: Vec<Method>,
    ) -> Result<Self, String> {
        let encoded = rmp_serde::to_vec_named(&methods)
            .map_err(|error| format!("engine write-set encoding failed: {error}"))?;
        check_commit_budget(&encoded, methods.len())?;
        let digest = Digest256::framed(domain, &[&encoded])?;
        Ok(IndexWriteSet {
            envelope_id: format!("{prefix}{}", digest.to_hex()),
            digest,
            methods,
        })
    }
}

impl IndexWriteSet {
    /// Key an engine-lowered write-set by an existing digest: the envelope id
    /// is `prefix` followed by `digest_hex`, so one source digest names one
    /// derived envelope and a replay commits nothing new. Refused whole past
    /// one commit's budget.
    pub(crate) fn with_digest(
        prefix: &str,
        digest_hex: &str,
        methods: Vec<Method>,
    ) -> Result<Self, String> {
        let encoded = rmp_serde::to_vec_named(&methods)
            .map_err(|error| format!("engine write-set encoding failed: {error}"))?;
        check_commit_budget(&encoded, methods.len())?;
        let digest = Digest256::parse(digest_hex)?;
        Ok(IndexWriteSet {
            envelope_id: format!("{prefix}{}", digest.to_hex()),
            digest,
            methods,
        })
    }
}

/// A batch commits as ONE atomic envelope or is refused whole with
/// [`BATCH_TOO_LARGE`] so the source re-sizes it: past the mutation batch's
/// operation budget, or when the committed batch record would not decode
/// under the durable ledger's record budget (medulla-storage: 64 MiB / 1M items;
/// each stored operation wraps its method in about ten more items).
fn check_commit_budget(encoded: &[u8], operations: usize) -> Result<(), String> {
    let limit = medulla_protocol_types::mutation_batch::MAX_MUTATION_OPERATIONS;
    let items = MAX_WRITE_SET_ITEMS.saturating_sub(operations * RECORD_ITEMS_PER_OPERATION);
    let record_budget = medulla_types::msgpack::MsgpackLimits::new(
        MAX_WRITE_SET_BYTES,
        items,
        medulla_types::msgpack::DEFAULT_MAX_DEPTH,
    );
    if operations <= limit
        && medulla_types::msgpack::validate_single_value(encoded, record_budget).is_ok()
    {
        return Ok(());
    }
    Err(format!(
        "{BATCH_TOO_LARGE}: the batch lowers to {operations} graph operations, past one \
         atomic commit's budget ({limit} operations, the durable record size); submit \
         fewer blobs per batch"
    ))
}

/// The labels an engine-lowered envelope kind is sealed under.
pub struct EnvelopeLabels {
    pub version_type: &'static str,
    pub policy_version: &'static str,
}

pub const REPOSITORY_LABELS: EnvelopeLabels = EnvelopeLabels {
    version_type: VERSION_TYPE,
    policy_version: POLICY_VERSION,
};

fn policy(object_id: &str, tenant: &str, labels: &EnvelopeLabels) -> Result<PolicyRecord, String> {
    let subject = Digest256::framed(b"eg/repository-index-policy-subject", &[tenant.as_bytes()])?;
    Ok(PolicyRecord {
        policy_id: format!("policy:{object_id}"),
        operation: MaterialOperation::Upsert,
        object_id: object_id.into(),
        tenant: tenant.into(),
        classification: "tenant-private".into(),
        policy_version: labels.policy_version.into(),
        subject_set_digest: subject.to_hex(),
        retention_policy: labels.policy_version.into(),
        legal_hold: false,
    })
}

/// Compile repository rows and its bounded enrichment intent as one commit.
/// The compiler includes the outbox bytes in the canonical payload digest.
pub fn build_envelope_with_outbox(
    ctx: &IndexCommitContext<'_>,
    write_set: IndexWriteSet,
    outbox: Vec<medulla_mutation_types::mutation_scope::MutationOutboxIntent>,
) -> Result<ChangeEnvelope, String> {
    let IndexWriteSet {
        envelope_id,
        digest,
        methods,
    } = write_set;
    let mutation = ctx.compile_with_outbox(&envelope_id, methods, outbox)?;
    seal(
        mutation,
        envelope_id,
        &digest,
        ctx.tenant_scope,
        &REPOSITORY_LABELS,
    )
}

/// Compile an engine-lowered write-set as one envelope under `labels`. The
/// values were already screened with [`stored_value`], so the
/// repository-snapshot rule admits them.
pub fn build_labelled_envelope(
    ctx: &IndexCommitContext<'_>,
    write_set: IndexWriteSet,
    labels: &EnvelopeLabels,
) -> Result<ChangeEnvelope, String> {
    let IndexWriteSet {
        envelope_id,
        digest,
        methods,
    } = write_set;
    let mutation = ctx.compile_with_outbox(&envelope_id, methods, Vec::new())?;
    seal(mutation, envelope_id, &digest, ctx.tenant_scope, labels)
}

/// Wrap a compiled batch as the repository-snapshot envelope of `digest`.
pub fn seal(
    mutation: medulla_protocol_types::mutation_batch::MutationBatch,
    envelope_id: String,
    digest: &Digest256,
    tenant: &str,
    labels: &EnvelopeLabels,
) -> Result<ChangeEnvelope, String> {
    let digest = digest.to_hex();
    let envelope = ChangeEnvelope {
        schema_version: CHANGE_ENVELOPE_VERSION,
        policies: vec![policy(&envelope_id, tenant, labels)?],
        envelope_id: envelope_id.clone(),
        mutation,
        content_version: ContentVersion {
            object_id: envelope_id,
            digest_algorithm: "sha256".into(),
            digest: digest.clone(),
            previous_digest: None,
            source_version: ContentVersionPosition::Opaque {
                version_type: labels.version_type.into(),
                value: digest.clone(),
            },
        },
        cursor: None,
        blobs: Vec::new(),
        features: Vec::new(),
        evidence: Vec::new(),
        lineage: Vec::new(),
        privacy: PrivacyAttestation {
            policy_version: labels.policy_version.into(),
            sanitizer_version: labels.policy_version.into(),
            sanitized_payload_digest: digest,
        },
        material_class: MaterialClass::RepositorySnapshot,
        commit_seq: None,
        commit_descriptor_ref: None,
    };
    envelope.validate()?;
    Ok(envelope)
}

/// Answer a batch whose write-set is durable (committed now, or already).
pub fn respond(request_id: u64, result: IndexResult) -> Response {
    Response::ok(
        request_id,
        ResultPayload::of::<medulla_protocol_types::result_contract::ingestion::IndexRepository>(
            result,
        ),
    )
}

/// Answer the commit of a batch: its index result, or the commit's refusal.
pub fn finish(result: IndexResult, commit: Response) -> Response {
    match commit.refusal_text() {
        Some(refusal) => Response::err(commit.id, refusal),
        None => respond(commit.id, result),
    }
}

#[cfg(any(test, feature = "test-support"))]
use medulla_mutation_types::mutation_scope::MutationSurface;
#[cfg(any(test, feature = "test-support"))]
use medulla_server_api::batch_compile::compile_methods;
#[cfg(any(test, feature = "test-support"))]
use medulla_server_api::batch_compile::CompileBatch;

#[cfg(any(test, feature = "test-support"))]
/// The repository-snapshot envelope the durable route commits for `result`.
pub fn repository_envelope(result: &IndexResult) -> Result<ChangeEnvelope, String> {
    let IndexWriteSet {
        envelope_id,
        digest,
        methods,
    } = lower(result)?;
    let mutation = compile_methods(
        CompileBatch {
            batch_id: &envelope_id,
            request_id: 1,
            attempt_nonce: None,
            principal: Some("test-principal"),
            tenant: "tenant-a",
            graph: "graph-a",
            placement_epoch: 0,
            idempotency_key: "repository-index-test",
            expected_graph_version: Some(0),
            fencing_token: None,
            created_at_ms: 1,
            default_surface: MutationSurface::Graph,
            authoritative_state: None,
        },
        methods,
    )?;
    seal(
        mutation,
        envelope_id,
        &digest,
        "tenant-a",
        &REPOSITORY_LABELS,
    )
}

#[cfg(test)]
mod tests {
    use medulla_protocol_types::mutation_batch::MAX_MUTATION_OPERATIONS;
    use medulla_protocol_types::protocol::Method;
    use medulla_types::ingestion_wire::{ExtractedEdge, ExtractedNode, IndexResult};

    use super::*;

    fn edge(source: &str, target: &str, relation: &str) -> ExtractedEdge {
        ExtractedEdge {
            source: source.into(),
            target: target.into(),
            edge_type: relation.into(),
            properties: std::collections::HashMap::new(),
        }
    }

    fn relation_of(method: &Method) -> Option<String> {
        match method {
            Method::AddEdge {
                properties_msgpack, ..
            } => rmp_serde::from_slice::<std::collections::BTreeMap<String, String>>(
                properties_msgpack,
            )
            .ok()
            .and_then(|properties| properties.get("relationship").cloned()),
            _ => None,
        }
    }

    // spec: MEDULLA-REPO-INGEST-R008
    //
    // Past one atomic commit's budget the batch is refused whole with the
    // stable retry code before anything commits: the sender halves and
    // re-sends, and no write-set, envelope or graph operation exists after
    // the refusal.
    #[test]
    fn oversized_batch_is_refused_with_the_stable_retry_code_and_commits_nothing() {
        let oversized = IndexResult {
            nodes: Vec::new(),
            edges: (0..=MAX_MUTATION_OPERATIONS)
                .map(|i| edge(&format!("n{i:06}"), "t", "calls"))
                .collect(),
            ..Default::default()
        };
        let refusal = match lower(&oversized) {
            Err(refusal) => refusal,
            Ok(_) => panic!("past the budget the batch must refuse"),
        };
        assert!(
            refusal.starts_with(BATCH_TOO_LARGE),
            "the stable retry code leads the refusal, got: {refusal}"
        );
        // The commit path refuses the same way: no envelope is built.
        match repository_envelope(&oversized) {
            Err(refusal) => assert!(
                refusal.starts_with(BATCH_TOO_LARGE),
                "the commit path must refuse the same batch, got: {refusal}"
            ),
            Ok(_) => panic!("the commit path must refuse the same batch"),
        }

        // At the budget the batch still commits: the ceiling is enforced, not a
        // blanket refusal.
        let within_budget = IndexResult {
            nodes: Vec::new(),
            edges: (0..8)
                .map(|i| edge(&format!("n{i}"), "t", "calls"))
                .collect(),
            ..Default::default()
        };
        let write_set = lower(&within_budget).expect("within budget the batch lowers");
        assert_eq!(write_set.methods().len(), 8);
    }

    // spec: MEDULLA-REPO-INGEST-R009
    // Rollup: hydration links indexed entities into the specification bridge
    // as typed edges committed in one envelope — proven by the child test
    // below (R009.2), the only test on main that exercises this path.
    //
    // spec: MEDULLA-REPO-INGEST-R009.2
    //
    // Two relations between the same endpoints stay two distinct edges: each
    // bridge relation stores as its own `(source, target, relation_type)`
    // edge in ONE envelope, the edge identity carries the relation type, and
    // only a duplicate triple collapses.
    #[test]
    fn two_relations_between_the_same_endpoints_stay_distinct_edges() {
        let bridge = IndexResult {
            nodes: vec![
                ExtractedNode {
                    node_id: "code::A".into(),
                    node_type: "symbol".into(),
                    properties: std::collections::HashMap::new(),
                },
                ExtractedNode {
                    node_id: "spec::B".into(),
                    node_type: "specification".into(),
                    properties: std::collections::HashMap::new(),
                },
            ],
            edges: vec![
                edge("code::A", "spec::B", "governsSpecification"),
                edge("code::A", "spec::B", "testedBy"),
                // A duplicate resolver output for the same triple collapses.
                edge("code::A", "spec::B", "governsSpecification"),
            ],
            ..Default::default()
        };
        let write_set = lower(&bridge).expect("the batch lowers");
        let methods = write_set.methods();
        // Two nodes and exactly two edges — one per distinct relation type.
        assert_eq!(methods.len(), 4, "{methods:?}");
        let edges: Vec<_> = methods
            .iter()
            .filter(|m| matches!(m, Method::AddEdge { .. }))
            .collect();
        assert_eq!(edges.len(), 2);
        let relations: Vec<_> = edges
            .iter()
            .map(|m| relation_of(m).expect("the edge identity carries its relation"))
            .collect();
        assert_eq!(
            relations,
            vec!["governsSpecification".to_string(), "testedBy".to_string()]
        );
        for method in &edges {
            match method {
                Method::AddEdge {
                    source_id,
                    target_id,
                    ..
                } => {
                    assert_eq!(source_id, "code::A");
                    assert_eq!(target_id, "spec::B");
                }
                _ => unreachable!("collected only AddEdge methods"),
            }
        }
        // One envelope commits the entity nodes and the bridge edges together:
        // no second graph writer.
        let nodes: Vec<_> = methods
            .iter()
            .filter(|m| matches!(m, Method::AddNode { .. }))
            .collect();
        assert_eq!(nodes.len(), 2);
    }
}
