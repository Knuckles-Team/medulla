//! The graph-scoped routes of `EnrichDocument` (MEDULLA-UNIFIED-DATA-PLANE-R651.7)
//! and `SelectWebPages` (MEDULLA-DECISION-ENGINE-R802).
//!
//! Both arrive after graph ACL, tenant binding, lazy-open and placement
//! lookup. Enrichment reads one RLS-visible Document, embeds off the reactor
//! and commits its write-set through the existing ChangeEnvelope authority,
//! exactly like a scoped `IndexRepository`. Selection reads the visible
//! `Document` rows and answers; it writes nothing.

#[cfg(all(feature = "query", feature = "modality-serving"))]
use super::repository_index::commit_context;
use super::*;

/// Route the two web-ingest methods; hand any other method back.
pub async fn route_web_ingest(ctx: GraphOpRouting<'_>, method: Method) -> Result<Response, Method> {
    match method {
        Method::EnrichDocument { request } => Ok(enrich(ctx, *request).await),
        Method::SelectWebPages { request } => Ok(select(ctx, *request).await),
        Method::SemanticEnrich { op } => Ok(super::semantic_enrichment::serve(ctx, *op).await),
        other => Err(other),
    }
}

#[cfg(all(feature = "query", feature = "modality-serving"))]
fn visible(ctx: &GraphOpRouting<'_>, node_id: &str) -> bool {
    ctx.read_authority
        .as_ref()
        .is_none_or(|authority| authority.node_visible(ctx.core, node_id))
}

#[cfg(all(feature = "query", feature = "modality-serving"))]
async fn enrich(
    ctx: GraphOpRouting<'_>,
    request: medulla_source_types::source_ingestion::web_document::EnrichDocumentRequest,
) -> Response {
    use medulla_h_ingest::web_ingest as web;
    let req_id = ctx.req_id;
    let doc_id = request.document_id.clone();
    let properties = match ctx.core.get_node_properties(&doc_id) {
        Some(properties) if visible(&ctx, &doc_id) => properties,
        _ => {
            return Response::err(
                req_id,
                format!("DOCUMENT_NOT_FOUND: no Document '{doc_id}'"),
            )
        }
    };
    let (document, config) = match web::held_document(&request, &properties)
        .and_then(|document| web::chunking(&request).map(|config| (document, config)))
    {
        Ok(pair) => pair,
        Err(error) => return Response::err(req_id, error),
    };
    let records = ctx.core.get_nodes_by_label(web::ENRICHMENT_TYPE, 0);
    if let Some(stored) = web::outranking_rung(&doc_id, &records) {
        return Response::err(
            req_id,
            format!(
                "RUNG_OUTRANKED: Document '{doc_id}' holds a {} fact; an embedding candidate does not replace it",
                medulla_h_ingest::ingestion_rung::rung_label(stored)
            ),
        );
    }
    let (current, prior) = web::prior_enrichments(&doc_id, &document.digest_hex, records);
    if let Some(record) = current {
        let receipt = web::replay_receipt(&doc_id, &document.digest_hex, &record);
        return respond(req_id, receipt);
    }
    let embedder = medulla_server_ops::api::ingest::embedder::EmbedderConfig::from_env();
    // Vectors already held for this Document's earlier spans, by exact span
    // digest: an identical span reuses its vector instead of re-embedding.
    let reusable: std::collections::BTreeMap<String, Vec<f32>> = {
        let store = ctx.core.semantic_store.read();
        prior
            .iter()
            .flat_map(|earlier| earlier.embedded.iter())
            .filter_map(|(digest, chunk)| Some((digest.clone(), store.get_embedding(chunk)?)))
            .collect()
    };
    let knobs = medulla_decision_types::decision::ingestion_admission::AdmissionKnobs::from_policy(
        &medulla_decision_types::decision::policy::DecisionPolicy::engine_default(),
    );
    let embedded = medulla_server_api::compute::compute_off_lock(req_id, move || {
        let spans = web::chunks(&document, &config)?;
        let known: std::collections::BTreeSet<String> = reusable.keys().cloned().collect();
        let decided = web::decide_spans(&document, spans, &knobs, &known);
        let texts: Vec<&str> = decided
            .iter()
            .filter(|unit| {
                unit.admission.outcome
                    == medulla_source_types::source_ingestion::web_document::AdmissionOutcome::Admitted
            })
            .map(|unit| unit.span.text(&document.text))
            .collect();
        let embedded_units = texts.len() as u32;
        let fresh = if texts.is_empty() {
            Vec::new()
        } else {
            embedder.embed(&texts)?
        };
        let vectors = web::kept_vectors(&decided, fresh, &reusable)?;
        Ok::<_, String>((document, decided, vectors, embedded_units, embedder))
    })
    .await;
    let (document, decided, vectors, embedded_units, embedder) = match embedded {
        Ok(Ok(parts)) => parts,
        Ok(Err(error)) => return Response::err(req_id, error),
        Err(response) => return response,
    };
    let admissions = decided.iter().map(|u| u.admission.clone()).collect();
    let (spans, digests): (Vec<_>, Vec<_>) = decided
        .into_iter()
        .filter(web::DecidedSpan::kept)
        .map(|unit| (unit.span, unit.digest))
        .unzip();
    let plan = web::EnrichmentPlan {
        document_id: &doc_id,
        document: &document,
        config,
        spans: &spans,
        vectors,
        embedder: embedder.name(),
        embedding_model: embedder.model.clone(),
        dimensions: embedder.dims,
        prior,
        admissions,
        digests,
        embedded_units,
    };
    match web::lower(plan) {
        Ok((write_set, receipt)) => commit(ctx, write_set, receipt).await,
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(all(feature = "query", feature = "modality-serving"))]
async fn commit(
    ctx: GraphOpRouting<'_>,
    write_set: medulla_h_ingest::repository_index::IndexWriteSet,
    receipt: medulla_source_types::source_ingestion::web_document::EnrichDocumentReceipt,
) -> Response {
    let (placement_epoch, fencing_token) = match source_ingestion_route_fence(&ctx).await {
        Ok(fence) => fence,
        Err(response) => return response,
    };
    let commit_ctx = commit_context(&ctx, placement_epoch, fencing_token);
    let envelope = match medulla_h_ingest::repository_index::build_labelled_envelope(
        &commit_ctx,
        write_set,
        &medulla_h_ingest::web_ingest::LABELS,
    ) {
        Ok(envelope) => envelope,
        Err(error) => return Response::err(ctx.req_id, error),
    };
    let committed =
        crate::dispatch::change_envelope::apply_one_change_envelope(ctx, envelope).await;
    match committed.refusal_text() {
        Some(refusal) => Response::err(committed.id, refusal),
        None => respond(committed.id, receipt),
    }
}

#[cfg(all(feature = "query", feature = "modality-serving"))]
fn respond(
    req_id: u64,
    receipt: medulla_source_types::source_ingestion::web_document::EnrichDocumentReceipt,
) -> Response {
    Response::ok(
        req_id,
        ResultPayload::of::<medulla_protocol_types::result_contract::ingestion::EnrichDocument>(
            receipt,
        ),
    )
}

#[cfg(not(all(feature = "query", feature = "modality-serving")))]
async fn enrich(
    ctx: GraphOpRouting<'_>,
    _request: medulla_source_types::source_ingestion::web_document::EnrichDocumentRequest,
) -> Response {
    Response::err(
        ctx.req_id,
        "ENRICHMENT_UNAVAILABLE: document enrichment requires the query and modality-serving features",
    )
}

#[cfg(all(feature = "query", feature = "modality-serving", feature = "decide"))]
async fn select(
    ctx: GraphOpRouting<'_>,
    request: medulla_decision_outcome_types::decision::web_fetch_selection::SelectWebPagesRequest,
) -> Response {
    use medulla_h_ingest::web_ingest as web;
    let documents: Vec<_> = ctx
        .core
        .get_nodes_by_label("Document", 0)
        .into_iter()
        .filter(|(id, _)| visible(&ctx, id))
        .collect();
    let (held, digests) = web::held_pages(documents);
    let now_ms = request
        .now_ms
        .unwrap_or_else(medulla_kernel::kernel::dispatch_support::authoritative_now_ms);
    let digest = web::query_digest(&request.query);
    match medulla_decision_stats::decision::web_fetch_selection::select_web_pages(
        &request, &held, &digests, now_ms, digest,
    ) {
        Ok(answer) => Response::ok(
            ctx.req_id,
            ResultPayload::of::<medulla_protocol_types::result_contract::query::SelectWebPages>(
                answer,
            ),
        ),
        Err(error) => Response::err(ctx.req_id, error),
    }
}

#[cfg(not(all(feature = "query", feature = "modality-serving", feature = "decide")))]
async fn select(
    ctx: GraphOpRouting<'_>,
    _request: medulla_decision_outcome_types::decision::web_fetch_selection::SelectWebPagesRequest,
) -> Response {
    Response::err(
        ctx.req_id,
        "SELECTION_UNAVAILABLE: web page selection requires the decide feature",
    )
}
