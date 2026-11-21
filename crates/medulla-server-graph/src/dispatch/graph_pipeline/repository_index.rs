//! EH-280 — the graph-scoped route of a branch-aware `IndexRepository`.
//!
//! An unscoped `IndexRepository` is a stateless parse answered by the control
//! plane. A scoped one reaches this route only after graph ACL (Write), tenant
//! binding, lazy-open and placement lookup, exactly like `SourceIngest`: it
//! parses each unique blob once, lowers the projection, and delegates the one
//! durable commit to the existing ChangeEnvelope authority.

use super::*;
#[cfg(all(feature = "ast", feature = "blob"))]
use medulla_server_blob::blob::BlobStateExt;

#[cfg(all(feature = "ast", feature = "redb"))]
mod consumer;
#[cfg(all(feature = "ast", feature = "redb"))]
mod enrichment;
#[cfg(all(
    feature = "ast",
    feature = "redb",
    feature = "blob",
    feature = "raft",
    feature = "security"
))]
pub use consumer::decode_pending_intent as decode_pending_enrichment_intent;
#[cfg(all(feature = "ast", feature = "redb", feature = "blob"))]
pub use consumer::drain_once as drain_repository_enrichment_once;
#[cfg(all(feature = "ast", feature = "redb", feature = "blob", feature = "raft"))]
pub use consumer::plan_held_underfunded_park;
#[cfg(all(
    feature = "ast",
    feature = "redb",
    feature = "blob",
    feature = "raft",
    feature = "security"
))]
pub use enrichment::intent_for_snapshot as enrichment_intent_for_snapshot;

type IndexScope = Box<medulla_ingestion_types::ingestion_wire::IndexRepositoryScope>;

/// Every scope path obeys the same portable logical-path rule as a source name.
#[cfg(feature = "ast")]
fn validate_scope_paths(
    scope: &medulla_ingestion_types::ingestion_wire::IndexRepositoryScope,
) -> Result<(), String> {
    use super::super::request_boundary::preflight::validate_ast_logical_path;
    let versions = scope.file_versions.iter().map(|item| item.path.as_str());
    let removed = scope
        .tombstones
        .iter()
        .flat_map(|item| std::iter::once(item.path.as_str()).chain(item.successor_path.as_deref()));
    versions
        .chain(removed)
        .try_for_each(validate_ast_logical_path)
}

/// The changesets of one branch: the whole chain and the delta after the last cited one,
/// both oldest first.
#[cfg(feature = "ast")]
#[derive(Default)]
struct ChangesetChain {
    chain: Vec<medulla_source_types::source_changeset::Changeset>,
    delta: Vec<medulla_source_types::source_changeset::Changeset>,
    /// The id of the newest commit of the branch (empty when it has none): the base
    /// the next changeset chains from (MEDULLA-SCM-R006).
    head: String,
    /// The `seq` of each changeset of the chain by changeset id, from the branch history
    /// index (MEDULLA-SCM-R005.1).
    seqs: std::collections::BTreeMap<String, u64>,
    /// The branch the chain was read from.
    #[cfg(all(feature = "ast", feature = "redb", feature = "blob"))]
    source: Option<crate::dispatch::changeset_lifecycle::BranchChain>,
}

/// What the route does after the index commits (MEDULLA-SDLC-R001.3): the lifecycle accept of
/// each changeset the chain delta cites, through the one accept `SubmitChangeset` runs.
#[derive(Default)]
struct AcceptFollow {
    #[cfg(all(feature = "ast", feature = "redb", feature = "blob"))]
    delta: Vec<(medulla_source_types::source_changeset::Changeset, u64)>,
    #[cfg(all(feature = "ast", feature = "redb", feature = "blob"))]
    source: Option<crate::dispatch::changeset_lifecycle::BranchChain>,
    branch: String,
}

/// The changesets of the repository's branch chain that follow the last one cited
/// (MEDULLA-SDLC-R014.3), oldest first. The last cited changeset id is the `digest` of the
/// stored artifact entry named `changesets`. A branch with no chain gives no changesets and
/// no error; a payload that is not a changeset (a branch write) is not one and is skipped.
/// The whole chain comes back too, for the derived row status (MEDULLA-SDLC-R014.12).
#[cfg(all(feature = "ast", feature = "redb"))]
async fn branch_changesets(
    ctx: &GraphOpRouting<'_>,
    branch: &str,
    stored: &std::collections::BTreeMap<
        String,
        medulla_h_ingest::source_ingestion::spec_projection::StoredArtifact,
    >,
) -> Result<ChangesetChain, String> {
    use medulla_source_types::source_changeset::decode_changeset;
    let registry = {
        let guard = ctx.state.read().await;
        medulla_persistence::branch_registry_store::shared_branch_registry(&guard)?
    };
    let reference = medulla_mutation_types::branch::BranchRef {
        tenant: ctx.tenant_scope.to_string(),
        graph: ctx.graph_name.to_string(),
        name: branch.to_string(),
    };
    let last_cited = stored
        .get(medulla_h_ingest::source_ingestion::spec_projection::CHANGESETS_ARTIFACT)
        .map(|artifact| artifact.digest.clone());
    #[cfg(feature = "blob")]
    let source = Some(crate::dispatch::changeset_lifecycle::BranchChain::new(
        std::sync::Arc::clone(&registry),
        reference.clone(),
    ));
    tokio::task::spawn_blocking(move || {
        let mut chain = Vec::new();
        let mut seqs = std::collections::BTreeMap::new();
        let mut head = String::new();
        // `chain` answers newest first. The delta stops at the last changeset already
        // cited; the whole chain is kept for the row status (MEDULLA-SDLC-R014.12).
        let mut delta_len = None;
        for (position, commit) in registry.chain(&reference)?.into_iter().enumerate() {
            if position == 0 {
                head = commit.id.to_string();
            }
            let Some(bytes) = registry.commit_payload(&reference, &commit.id)? else {
                continue;
            };
            let Ok(changeset) = decode_changeset(&bytes) else {
                continue;
            };
            if delta_len.is_none() && last_cited.as_deref() == Some(changeset.id.as_str()) {
                delta_len = Some(chain.len());
            }
            if let Some(bytes) = registry.history_row(
                &reference,
                &medulla_compute::source_history::seq_key(&commit.id),
            )? {
                seqs.insert(
                    changeset.id.clone(),
                    medulla_compute::source_history::decode_seq(&bytes)?,
                );
            }
            chain.push(changeset);
        }
        let delta_len = delta_len.unwrap_or(chain.len());
        // Newest first becomes oldest first; the delta is the newest `delta_len` entries.
        let mut delta: Vec<_> = chain[..delta_len].to_vec();
        delta.reverse();
        chain.reverse();
        Ok::<_, String>(ChangesetChain {
            chain,
            delta,
            head,
            seqs,
            #[cfg(feature = "blob")]
            source,
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Without the branch registry there is no changeset chain to read.
#[cfg(all(feature = "ast", not(feature = "redb")))]
async fn branch_changesets(
    _ctx: &GraphOpRouting<'_>,
    _branch: &str,
    _stored: &std::collections::BTreeMap<
        String,
        medulla_h_ingest::source_ingestion::spec_projection::StoredArtifact,
    >,
) -> Result<ChangesetChain, String> {
    Ok(ChangesetChain::default())
}

/// Decode, validate and parse+project one scoped batch (off the reactor).
#[cfg(feature = "ast")]
async fn index_scoped(
    ctx: GraphOpRouting<'_>,
    files_msgpack: Vec<u8>,
    scope: IndexScope,
) -> Result<(medulla_ingestion_types::ingestion_wire::IndexResult, AcceptFollow), Response> {
    use super::super::request_boundary::preflight::{ast_input_limits, decode_ast_sources};
    let req_id = ctx.req_id;
    // Entries are content-keyed: one path may name several blobs (EH-280).
    let files = decode_ast_sources(&files_msgpack, ast_input_limits())
        .and_then(|files| validate_scope_paths(&scope).map(|()| files))
        .map_err(|error| Response::err(req_id, error))?;
    // MEDULLA-SDLC-R014.2: the spec artifact digests this repository already holds.
    let stored_specs = medulla_h_ingest::source_ingestion::spec_projection::stored_artifacts(
        ctx.core,
        &scope.repository_id,
    );
    // MEDULLA-SDLC-R014.3: the changeset chain of the repository's default branch (or the
    // branch the scope names) cites the requirement rows it delivers.
    let branch_name = scope
        .branch
        .clone()
        .unwrap_or_else(|| medulla_mutation_types::branch::RESERVED_BRANCH_NAME.to_string());
    let ChangesetChain {
        chain,
        delta: changesets,
        head,
        seqs,
        #[cfg(all(feature = "redb", feature = "blob"))]
        source,
        ..
    } = branch_changesets(&ctx, &branch_name, &stored_specs)
        .await
        .map_err(|error| Response::err(req_id, error))?;
    #[cfg(all(feature = "redb", feature = "blob"))]
    let follow = AcceptFollow {
        delta: changesets
            .iter()
            .filter_map(|changeset| Some((changeset.clone(), *seqs.get(&changeset.id)?)))
            .collect(),
        source,
        branch: branch_name.clone(),
    };
    #[cfg(not(all(feature = "redb", feature = "blob")))]
    let follow = {
        let _ = &seqs;
        AcceptFollow { branch: branch_name.clone() }
    };
    // MEDULLA-SCM-R006: the generation this full index leaves; the next changeset chains
    // from the branch head.
    let generation = ctx
        .core
        .get_node_properties(&medulla_h_ingest::repository_index::changeset::generation_node_id(
            &scope.repository_id,
        ))
        .and_then(|blob| medulla_h_ingest::repository_index::changeset::read_generation(&blob));
    // MEDULLA-ORCH-VOCAB-R012.7.2: the commit a policy candidate cites is the revision of
    // the indexed branch's ref (its first ref when no ref carries the branch name).
    let commit = scope
        .refs
        .as_slice()
        .iter()
        .find(|r| r.ref_name == branch_name || r.ref_name == format!("refs/heads/{branch_name}"))
        .or_else(|| scope.refs.as_slice().first())
        .map(|r| r.revision_id.clone())
        .unwrap_or_default();
    // Validate the complete scope and parse before acquiring CAS holders. A
    // malformed membership must never pin unreferenced source bytes forever.
    let (mut result, files, repository_id) =
        medulla_server_api::compute::compute_off_lock(req_id, move || {
            let mut result =
                medulla_compute::parser::branch_index::index_branches(files.clone(), &scope)?;
            // MEDULLA-REPO-INGEST-R801: packages and layers lower beside the
            // entity nodes; a layer cycle refuses the batch before any commit.
            medulla_h_ingest::repository_index::ecosystem::project(&files, &mut result)?;
            // MEDULLA-REPO-INGEST-R601.3 / R602.3: commits, routes, IaC resources
            // and service prefixes lower beside them for the enrichment hooks.
            medulla_h_ingest::repository_index::scope_facts::project(&files, &scope, &mut result)?;
            // MEDULLA-SDLC-R014.2: changed spec artifact files lower beside them as
            // requirement records, so they commit in the same envelope.
            medulla_h_ingest::source_ingestion::spec_projection::project_spec_artifacts(
                &scope.repository_id,
                &files,
                &stored_specs,
                &chain,
                &changesets,
                &branch_name,
                &commit,
                &mut result,
            )?;
            // MEDULLA-SCM-R006: a full index is the baseline of the changeset chain.
            medulla_h_ingest::repository_index::changeset::stamp_generation(
                &mut result,
                &scope.repository_id,
                generation.as_ref(),
                &head,
            );
            // MEDULLA-SCM-R006: one bucket node per unresolved name, so a changeset
            // reads exactly the references that wait on a name it defines.
            let (buckets, references) = medulla_compute::parser::resolve::unresolved_buckets(
                &scope.repository_id,
                &result.nodes,
            );
            result.nodes.extend(buckets);
            result.edges.extend(references);
            Ok::<_, String>((result, files, scope.repository_id.clone()))
        })
        .await?
        .map_err(|error| Response::err(req_id, error))?;
    let refs = admit_repository_sources(ctx, &repository_id, &files).await?;
    attach_content_refs(&mut result, &refs);
    Ok((result, follow))
}

#[cfg(feature = "ast")]
fn attach_content_refs(
    result: &mut medulla_ingestion_types::ingestion_wire::IndexResult,
    refs: &std::collections::BTreeMap<String, (String, u64)>,
) {
    for node in &mut result.nodes {
        if node.node_type == "Blob" {
            if let Some(digest) = node.properties.get("content_digest") {
                if let Some((content_ref, length)) = refs.get(digest) {
                    node.properties
                        .insert("content_ref".into(), content_ref.clone());
                    node.properties
                        .insert("content_length".into(), length.to_string());
                }
            }
        }
    }
}

#[cfg(not(feature = "ast"))]
async fn index_scoped(
    ctx: GraphOpRouting<'_>,
    files_msgpack: Vec<u8>,
    scope: IndexScope,
) -> Result<(medulla_ingestion_types::ingestion_wire::IndexResult, AcceptFollow), Response> {
    let req_id = ctx.req_id;
    let _ = (files_msgpack, scope);
    Err(Response::err(req_id, "AST feature not enabled".to_string()))
}

/// Admit source bytes as one tenant/repository-scoped CAS owner transaction.
/// The graph envelope later pins each submitted Blob's content_ref; a parse or
/// graph refusal leaves only set-like CAS holders, never a partial graph batch.
#[cfg(all(feature = "ast", feature = "blob"))]
async fn admit_repository_sources(
    ctx: GraphOpRouting<'_>,
    repository_id: &str,
    files: &[(String, Vec<u8>)],
) -> Result<std::collections::BTreeMap<String, (String, u64)>, Response> {
    use sha2::{Digest, Sha256};
    let Some(blob) = ctx.state.read().await.blob_cursors().cloned() else {
        return Ok(Default::default());
    };
    let tenant = ctx.tenant_scope.to_string();
    let repository = repository_id.to_string();
    let bodies: Vec<_> = files
        .iter()
        .map(
            |(_, bytes)| medulla_server_blob::blob::engine_bodies::EngineBody {
                sha256: medulla_types::contract::Digest256::from_bytes(
                    Sha256::digest(bytes).into(),
                ),
                body: bytes.clone(),
            },
        )
        .collect();
    let stored = tokio::task::spawn_blocking(move || {
        blob.store.put_repository_bodies(
            &tenant,
            &repository,
            &bodies,
            medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
        )
    })
    .await
    .map_err(|error| Response::err(ctx.req_id, error.to_string()))?
    .map_err(|error| Response::err(ctx.req_id, error))?;
    Ok(stored
        .into_iter()
        .map(|body| {
            (
                format!("sha256:{}", body.sha256.to_hex()),
                (format!("cas:sha256:{}", body.manifest_digest), body.length),
            )
        })
        .collect())
}

#[cfg(all(feature = "ast", not(feature = "blob")))]
async fn admit_repository_sources(
    _ctx: GraphOpRouting<'_>,
    _repository_id: &str,
    _files: &[(String, Vec<u8>)],
) -> Result<std::collections::BTreeMap<String, (String, u64)>, Response> {
    Ok(Default::default())
}

/// Whether this exact write-set was already committed to the graph.
async fn already_committed(ctx: &GraphOpRouting<'_>, envelope_id: &str) -> Result<bool, Response> {
    let Some(persistence) = ctx.persistence.as_ref() else {
        return Err(Response::err(
            ctx.req_id,
            "branch-aware repository indexing requires durable persistence",
        ));
    };
    persistence
        .read_change_envelope(
            &medulla_kernel::persist::sanitize(ctx.graph_name),
            envelope_id,
        )
        .await
        .map(|record| record.is_some())
        .map_err(|error| Response::err(ctx.req_id, error))
}

/// The verified coordinates an engine-lowered envelope commits under, shared
/// by the repository-index and document-enrichment routes.
pub fn commit_context<'a>(
    ctx: &GraphOpRouting<'a>,
    placement_epoch: u64,
    fencing_token: Option<u64>,
) -> medulla_h_ingest::repository_index::IndexCommitContext<'a> {
    medulla_h_ingest::repository_index::IndexCommitContext {
        request_id: ctx.req_id,
        graph_name: ctx.graph_name,
        tenant_scope: ctx.tenant_scope,
        verified: ctx.verified_context,
        graph_version: ctx.core.version(),
        placement_epoch,
        fencing_token,
    }
}

/// Parse, project and durably commit one scoped `IndexRepository` batch.
pub async fn route_repository_index(
    ctx: GraphOpRouting<'_>,
    files_msgpack: Vec<u8>,
    scope: IndexScope,
    placement_epoch: u64,
    fencing_token: Option<u64>,
) -> Response {
    let follow_on_scope = medulla_h_ingest::repository_index::enrich::FollowOnScope {
        tenant: ctx.tenant_scope.to_string(),
        graph: ctx.graph_name.to_string(),
        repository_id: scope.repository_id.clone(),
    };
    let repository_id = scope.repository_id.clone();
    let (result, follow) = match index_scoped(ctx, files_msgpack, scope).await {
        Ok(indexed) => indexed,
        Err(response) => return response,
    };
    // The requirement records this index writes: references waiting on them resolve once the
    // batch commits.
    let requirement_prefix = format!("requirement:{repository_id}:");
    let requirements: Vec<String> = result
        .nodes
        .iter()
        .filter(|node| node.node_id.starts_with(&requirement_prefix))
        .map(|node| node.node_id.clone())
        .collect();
    let write_set = match medulla_h_ingest::repository_index::lower(&result) {
        Ok(write_set) => write_set,
        Err(error) => return Response::err(ctx.req_id, error),
    };
    let envelope_id = write_set.envelope_id.clone();
    match already_committed(&ctx, &envelope_id).await {
        Ok(true) => return medulla_h_ingest::repository_index::respond(ctx.req_id, result),
        Ok(false) => {}
        Err(response) => return response,
    }
    #[cfg(all(feature = "ast", feature = "redb"))]
    let pending = match enrichment::pending_intent(ctx, &result, &envelope_id).await {
        Ok(pending) => pending,
        Err(error) => return Response::err(ctx.req_id, error),
    };
    // MEDULLA-REPO-INGEST-R600.3: the durable follow-on work item commits in
    // this batch's own outbox and is claimed after the index commits.
    let follow_on = match medulla_h_ingest::repository_index::enrich::follow_on_intent(
        &envelope_id,
        follow_on_scope,
    ) {
        Ok(intent) => intent,
        Err(error) => return Response::err(ctx.req_id, error),
    };
    #[cfg(all(feature = "ast", feature = "redb"))]
    let outbox = pending
        .as_ref()
        .map(|entry| entry.intent.clone())
        .into_iter()
        .chain(std::iter::once(follow_on))
        .collect();
    #[cfg(all(feature = "ast", feature = "redb"))]
    let source_budget = pending_source_budget(pending.as_ref());
    #[cfg(not(all(feature = "ast", feature = "redb")))]
    let outbox = vec![follow_on];
    // The engine lowers this envelope, so its graph version is read under the
    // graph write lock: a commit landing between the read and the apply (the
    // previous batch's follow-on enrichment) must not refuse this batch.
    let held = crate::dispatch::change_envelope::lock_graph_for_lowering(ctx.graph_name).await;
    let commit_ctx = commit_context(&ctx, placement_epoch, fencing_token);
    let envelope = match medulla_h_ingest::repository_index::build_envelope_with_outbox(
        &commit_ctx,
        write_set,
        outbox,
    ) {
        Ok(envelope) => envelope,
        Err(error) => return Response::err(ctx.req_id, error),
    };
    #[cfg(all(feature = "ast", feature = "redb"))]
    let commit = crate::dispatch::change_envelope::apply_repository_change_envelope(
        ctx,
        held,
        envelope,
        source_budget,
    )
    .await;
    #[cfg(not(all(feature = "ast", feature = "redb")))]
    let commit =
        crate::dispatch::change_envelope::apply_lowered_change_envelope(ctx, held, envelope).await;
    let response = medulla_h_ingest::repository_index::finish(result, commit);
    if response.refusal_text().is_some() {
        return response;
    }
    match accept_followers(&ctx, &repository_id, follow, &requirements).await {
        Ok(()) => response,
        Err(error) => Response::err(ctx.req_id, error),
    }
}

/// MEDULLA-SDLC-R001.3: the lifecycle accept of every changeset the index followed, through
/// the accept `SubmitChangeset` runs (the same derivation and the same
/// `CHANGESET_NODE_CONFLICT` check), then the resolution of the references that waited on the
/// requirement records this index wrote.
#[cfg(all(feature = "ast", feature = "redb", feature = "blob"))]
async fn accept_followers(
    ctx: &GraphOpRouting<'_>,
    repository_id: &str,
    follow: AcceptFollow,
    requirements: &[String],
) -> Result<(), String> {
    use crate::dispatch::changeset_lifecycle::{accept_edges, resolve_waiting};
    use medulla_server_blob::blob::BlobStateExt;
    let store = ctx
        .state
        .read()
        .await
        .blob_cursors()
        .map(|cursors| std::sync::Arc::clone(&cursors.store));
    if let (Some(source), Some(store)) = (follow.source, store) {
        let owner = medulla_server_api::carrier::CarrierAuthority::from_verified(
            ctx.verified_context,
        )?
        .owner_scope()
        .to_string();
        for (changeset, seq) in &follow.delta {
            Box::pin(accept_edges(
                ctx.state,
                ctx.verified_context,
                ctx.req_id,
                std::sync::Arc::clone(&store),
                owner.clone(),
                changeset,
                &follow.branch,
                *seq,
                source.clone(),
            ))
            .await?;
        }
    }
    Box::pin(resolve_waiting(
        ctx.state,
        ctx.verified_context,
        ctx.req_id,
        repository_id,
        requirements,
    ))
    .await
}

#[cfg(not(all(feature = "ast", feature = "redb", feature = "blob")))]
async fn accept_followers(
    _ctx: &GraphOpRouting<'_>,
    _repository_id: &str,
    _follow: AcceptFollow,
    _requirements: &[String],
) -> Result<(), String> {
    Ok(())
}

#[cfg(all(feature = "ast", feature = "redb"))]
fn pending_source_budget(
    pending: Option<&enrichment::PendingEnrichment>,
) -> Option<medulla_store::redb_store::enrichment_budget::SourceBudgetAuthority> {
    pending.map(|entry| entry.budget.clone())
}

#[cfg(test)]
mod tests {
    use super::super::super::request_boundary::preflight::preflight_request_msgpack;
    use medulla_protocol_types::protocol::Method;

    #[cfg(feature = "ast")]
    #[test]
    fn only_cas_admitted_blob_rows_receive_source_refs() {
        let blob = |digest: &str| medulla_ingestion_types::ingestion_wire::ExtractedNode {
            node_id: format!("blob:{digest}"),
            node_type: "Blob".into(),
            properties: std::collections::HashMap::from([("content_digest".into(), digest.into())]),
        };
        let mut result = medulla_ingestion_types::ingestion_wire::IndexResult {
            nodes: vec![blob("sha256:admitted"), blob("sha256:prior-tombstone")],
            ..Default::default()
        };
        super::attach_content_refs(
            &mut result,
            &std::collections::BTreeMap::from([(
                "sha256:admitted".into(),
                ("cas:sha256:manifest".into(), 12),
            )]),
        );
        assert_eq!(
            result.nodes[0]
                .properties
                .get("content_ref")
                .map(String::as_str),
            Some("cas:sha256:manifest")
        );
        assert_eq!(
            result.nodes[0]
                .properties
                .get("content_length")
                .map(String::as_str),
            Some("12")
        );
        assert!(!result.nodes[1].properties.contains_key("content_ref"));
    }

    /// A caller cannot claim the repository-snapshot class on the public
    /// envelope method to reach the repository-content screening rule.
    #[test]
    fn callers_cannot_submit_repository_snapshot_material() {
        let result = medulla_ingestion_types::ingestion_wire::IndexResult {
            nodes: vec![medulla_ingestion_types::ingestion_wire::ExtractedNode {
                node_id: "blob:x".to_string(),
                node_type: "Blob".to_string(),
                properties: std::collections::HashMap::new(),
            }],
            ..Default::default()
        };
        let envelope = medulla_h_ingest::repository_index::repository_envelope(&result)
            .expect("repository envelope");
        let error = preflight_request_msgpack(&Method::ApplyChangeEnvelope {
            envelope: Box::new(envelope),
        })
        .expect_err("engine-attested only");
        assert!(error.contains("engine-attested"), "{error}");
    }

    #[cfg(all(feature = "ast", feature = "redb"))]
    #[test]
    fn repository_index_forwards_enrichment_budget_with_pending_intent() {
        let pending = super::enrichment::PendingEnrichment {
            intent: medulla_mutation_types::mutation_scope::MutationOutboxIntent {
                topic: "repository.enrichment.pending".into(),
                key: "source-envelope".into(),
                payload: Vec::new(),
                headers: Default::default(),
            },
            budget: medulla_store::redb_store::enrichment_budget::SourceBudgetAuthority {
                tenant_id: "tenant".into(),
                source_envelope: "source-envelope".into(),
                snapshot_digest: "a".repeat(64),
                repository_id: "repo".into(),
                policy_digest: "b".repeat(64),
                total_budget_units: 7,
                max_total_units: 9,
            },
        };
        let authority = super::pending_source_budget(Some(&pending)).expect("authority");
        assert_eq!(authority.source_envelope, "source-envelope");
        assert_eq!(authority.total_budget_units, 7);
        assert_eq!(authority.max_total_units, 9);
        assert!(super::pending_source_budget(None).is_none());
    }
}
