//! The graph-scoped route of `DocumentDelete` (MEDULLA-UNIFIED-DATA-PLANE-R651.3..R651.5).
//!
//! Arrives after graph ACL, tenant binding, lazy-open and placement lookup,
//! exactly like `route_web_ingest`. The handler reads only RLS-visible nodes,
//! plans the cascade with `medulla_document::cascade::plan_cascade`, and
//! commits the removals plus one content-free `DocumentDeletion` record as one
//! ChangeEnvelope. A refusal writes nothing.

#[cfg(all(feature = "query", feature = "modality-serving"))]
use super::repository_index::commit_context;
use super::*;

/// Default for `knob document.max_cascade_nodes`.
#[cfg(all(feature = "query", feature = "modality-serving"))]
pub const DEFAULT_MAX_CASCADE_NODES: usize = 100_000;
/// Environment override of `knob document.max_cascade_nodes`.
#[cfg(all(feature = "query", feature = "modality-serving"))]
pub const MAX_CASCADE_NODES_ENV: &str = "MEDULLA_DOCUMENT_MAX_CASCADE_NODES";

/// Route the document lifecycle methods; hand any other method back.
pub async fn route_document_ops(
    ctx: GraphOpRouting<'_>,
    method: Method,
) -> Result<Response, Method> {
    match method {
        Method::DocumentDelete { request } => Ok(handle_document_delete(ctx, *request).await),
        other => Err(other),
    }
}

/// `knob document.max_cascade_nodes`: the environment value, else the default.
#[cfg(all(feature = "query", feature = "modality-serving"))]
pub fn max_cascade_nodes() -> usize {
    std::env::var(MAX_CASCADE_NODES_ENV)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .filter(|value: &usize| *value > 0)
        .unwrap_or(DEFAULT_MAX_CASCADE_NODES)
}

/// The RLS-visible edge view the cascade planner reads.
#[cfg(all(feature = "query", feature = "modality-serving"))]
struct VisibleGraph<'a, 'b> {
    ctx: &'a GraphOpRouting<'b>,
}

#[cfg(all(feature = "query", feature = "modality-serving"))]
impl VisibleGraph<'_, '_> {
    fn visible(&self, node_id: &str) -> bool {
        self.ctx
            .read_authority
            .as_ref()
            .is_none_or(|authority| authority.node_visible(self.ctx.core, node_id))
    }

    fn key(&self, node_id: &str) -> medulla_document::cascade::NodeKey {
        let label = self
            .ctx
            .core
            .get_node_properties(node_id)
            .and_then(|blob| medulla_types::msgpack::decode_property_value(&blob).ok())
            .and_then(|value| {
                value
                    .get("type")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        medulla_document::cascade::NodeKey {
            label,
            id: node_id.to_string(),
        }
    }

    fn edges(
        &self,
        source: &str,
        target: &str,
    ) -> impl Iterator<Item = medulla_document::cascade::EdgeKey> + '_ {
        let from = self.key(source);
        let to = self.key(target);
        self.ctx
            .core
            .typed_edges_between(source, target)
            .into_iter()
            .map(move |(label, _)| medulla_document::cascade::EdgeKey {
                from: from.clone(),
                label,
                to: to.clone(),
            })
    }
}

#[cfg(all(feature = "query", feature = "modality-serving"))]
impl medulla_document::cascade::CascadeReader for VisibleGraph<'_, '_> {
    fn in_edges(
        &self,
        node: &medulla_document::cascade::NodeKey,
    ) -> Vec<medulla_document::cascade::EdgeKey> {
        let sources = self.ctx.core.get_predecessors(&node.id).unwrap_or_default();
        sources
            .iter()
            .filter(|source| self.visible(source))
            .flat_map(|source| self.edges(source, &node.id).collect::<Vec<_>>())
            .collect()
    }

    fn out_edges(
        &self,
        node: &medulla_document::cascade::NodeKey,
    ) -> Vec<medulla_document::cascade::EdgeKey> {
        let targets = self.ctx.core.get_successors(&node.id).unwrap_or_default();
        targets
            .iter()
            .filter(|target| self.visible(target))
            .flat_map(|target| self.edges(&node.id, target).collect::<Vec<_>>())
            .collect()
    }
}

/// Plan the cascade of one visible Document, then apply it.
#[cfg(all(feature = "query", feature = "modality-serving"))]
pub async fn handle_document_delete(
    ctx: GraphOpRouting<'_>,
    request: medulla_query_types::document_ops::DocumentDeleteRequest,
) -> Response {
    use medulla_document::cascade::{plan_cascade, CascadeRefusal, DOCUMENT_LABEL};
    let req_id = ctx.req_id;
    let graph = VisibleGraph { ctx: &ctx };
    let root = graph.key(&request.document_id);
    // A Document of another tenant is indistinguishable from an absent one.
    if root.label != DOCUMENT_LABEL
        || !graph.visible(&request.document_id)
        || request.tenant != ctx.verified_context.tenant()
    {
        return Response::err(req_id, CascadeRefusal::NotFound.to_string());
    }
    let plan = match plan_cascade(
        &graph,
        &request.tenant,
        &request.document_id,
        request.max_nodes.map_or(max_cascade_nodes(), |cap| {
            usize::try_from(cap).map_or(max_cascade_nodes(), |cap| cap.min(max_cascade_nodes()))
        }),
    ) {
        Ok(plan) => plan,
        Err(refusal) => return Response::err(req_id, refusal.to_string()),
    };
    apply_cascade(ctx, plan).await
}

/// Commit the plan's removals and its deletion record as one ChangeEnvelope.
#[cfg(all(feature = "query", feature = "modality-serving"))]
pub async fn apply_cascade(
    ctx: GraphOpRouting<'_>,
    plan: medulla_document::cascade::CascadePlan,
) -> Response {
    use medulla_h_ingest::document_delete as delete;
    let deleted_at = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let (record, receipt) = delete::deletion_record(&plan, deleted_at, ctx.verified_actor);
    let write_set = match delete::lower(&plan, &record) {
        Ok(write_set) => write_set,
        Err(error) => return Response::err(ctx.req_id, error),
    };
    let (placement_epoch, fencing_token) = match source_ingestion_route_fence(&ctx).await {
        Ok(fence) => fence,
        Err(response) => return response,
    };
    let commit_ctx = commit_context(&ctx, placement_epoch, fencing_token);
    let envelope = match medulla_h_ingest::repository_index::build_labelled_envelope(
        &commit_ctx,
        write_set,
        &delete::LABELS,
    ) {
        Ok(envelope) => envelope,
        Err(error) => return Response::err(ctx.req_id, error),
    };
    let committed =
        crate::dispatch::change_envelope::apply_one_change_envelope(ctx, envelope).await;
    match committed.refusal_text() {
        Some(refusal) => Response::err(committed.id, refusal),
        None => Response::ok(
            committed.id,
            ResultPayload::of::<medulla_protocol_types::result_contract::ingestion::DocumentDelete>(
                receipt,
            ),
        ),
    }
}

#[cfg(not(all(feature = "query", feature = "modality-serving")))]
pub async fn handle_document_delete(
    ctx: GraphOpRouting<'_>,
    _request: medulla_query_types::document_ops::DocumentDeleteRequest,
) -> Response {
    Response::err(
        ctx.req_id,
        "DOCUMENT_DELETE_UNAVAILABLE: document delete requires the query and modality-serving features",
    )
}
