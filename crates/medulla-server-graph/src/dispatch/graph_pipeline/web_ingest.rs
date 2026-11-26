//! The graph-scoped routes of `EnrichDocument` (MEDULLA-UNIFIED-DATA-PLANE-R651.7)
//! and `SelectWebPages` (MEDULLA-DECISION-ENGINE-R802).
//!
//! Both arrive after graph ACL, tenant binding, lazy-open and placement
//! lookup. Enrichment reads one RLS-visible Document, embeds off the reactor
//! and commits its write-set through the existing ChangeEnvelope authority,
//! exactly like a scoped `IndexRepository`. Selection reads the visible
//! `Document` rows and answers; it writes nothing. `OntologySlice`
//! (MEDULLA-INGEST-WORKER-R913.1) reads the request graph's admitted ontology and answers the
//! classes and properties one text unit needs; it reads no row and writes nothing.

#[cfg(all(feature = "query", feature = "modality-serving"))]
use super::repository_index::commit_context;
use super::*;

/// Route the two web-ingest methods; hand any other method back.
pub async fn route_web_ingest(ctx: GraphOpRouting<'_>, method: Method) -> Result<Response, Method> {
    match method {
        Method::EnrichDocument { request } => Ok(enrich(ctx, *request).await),
        Method::SelectWebPages { request } => Ok(select(ctx, *request).await),
        Method::SemanticEnrich { op } => Ok(super::semantic_enrichment::serve(ctx, *op).await),
        Method::OntologySlice { request } => Ok(slice_ontology(&ctx, &request)),
        other => Err(other),
    }
}

/// Serve `OntologySlice` over the request graph's schema sources, embedding class texts with the
/// one text embedder the Uql path binds when the request carries the unit's embedding.
#[cfg(all(feature = "query", feature = "decide", feature = "blob"))]
fn slice_ontology(
    ctx: &GraphOpRouting<'_>,
    request: &medulla_ingestion_types::ingestion_wire::OntologySliceRequest,
) -> Response {
    let embedder = medulla_server_ops::api::query_txn::query_support::uql_text_embedder();
    let embed = |text: &str| match embedder {
        Some(embedder) => embedder.embed(text),
        None => Err("no text embedder is bound".to_string()),
    };
    let bound: Option<medulla_h_ingest::ontology_slice::SliceEmbedder<'_>> = if embedder.is_some() {
        Some(&embed)
    } else {
        None
    };
    match medulla_h_ingest::ontology_slice::slice_for_unit(
        ctx.core.schema_sources().as_ref(),
        request,
        bound,
    ) {
        Ok(slice) => Response::ok(
            ctx.req_id,
            ResultPayload::of::<medulla_protocol_types::result_contract::ingestion::OntologySlice>(
                slice,
            ),
        ),
        Err(refusal) => Response::err(ctx.req_id, refusal.message()),
    }
}

#[cfg(not(all(feature = "query", feature = "decide", feature = "blob")))]
fn slice_ontology(
    ctx: &GraphOpRouting<'_>,
    _request: &medulla_ingestion_types::ingestion_wire::OntologySliceRequest,
) -> Response {
    Response::err(
        ctx.req_id,
        "METHOD_NOT_YET_SERVED: the ontology slice requires the query, decide and blob features",
    )
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
    let document = match web::held_document(&request, &properties) {
        Ok(document) => document,
        Err(error) => return Response::err(req_id, error),
    };
    if document.is_entity_document() {
        return enrich_alignment(ctx, &request, &doc_id, document).await;
    }
    let config = match web::chunking(&request) {
        Ok(config) => config,
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
    let chunk_classes = medulla_h_ingest::ingestion_admission::chunk_classes(&records);
    let (current, prior) = web::prior_enrichments(&doc_id, &document.digest_hex, records);
    if let Some(record) = current {
        let receipt = web::replay_receipt(&doc_id, &document.digest_hex, &record);
        return respond(req_id, receipt);
    }
    let knobs = medulla_decision_types::decision::ingestion_admission::AdmissionKnobs::from_policy(
        &medulla_decision_types::decision::policy::DecisionPolicy::engine_default(),
    );
    let demoted = match demoted_classes(&ctx, &chunk_classes, &knobs).await {
        Ok(demoted) => demoted,
        Err(error) => return Response::err(req_id, error),
    };
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
    let demoted_labels: Vec<String> = demoted
        .iter()
        .map(|class| medulla_h_ingest::ingestion_admission::class_label(*class))
        .collect();
    let max_bytes = medulla_server_api::knob_config::knobs().scheduler_document_max_bytes;
    let embedded = medulla_server_api::compute::compute_off_lock(req_id, move || {
        web::check_document_size(&document, max_bytes)?;
        let spans = web::chunks(&document, &config)?;
        let (sections, tree_check) = web::sections(&document);
        web::check_tree(tree_check)?;
        let links = medulla_document::processing::extract_link_anchors(&document.text);
        let known: std::collections::BTreeSet<String> = reusable.keys().cloned().collect();
        let decided = web::decide_spans(&document, spans, &knobs, &known, &demoted);
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
        Ok::<_, String>((
            document,
            decided,
            vectors,
            embedded_units,
            embedder,
            sections,
            tree_check,
            links,
        ))
    })
    .await;
    let (document, decided, vectors, embedded_units, embedder, sections, tree_check, links) = match embedded {
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
    let fact_supersession = {
        let read = |node_id: &str| {
            let bytes = ctx.core.get_node_properties(node_id)?;
            medulla_types::msgpack::decode_property_value(&bytes).ok()
        };
        // The same ChunkFacts scope key the facts batch handler derives.
        let scope = medulla_h_ingest::source_ingestion::facts::chunk_facts_scope(ctx.verified_context);
        match web::prior_fact_supersession(&scope, &prior, &read) {
            Ok(methods) => methods,
            Err(error) => return Response::err(req_id, error),
        }
    };
    // The chunk scan: one `proposed` claim per passing detector, written with the chunks
    // (MEDULLA-TYPED-PACKS-R803.7). The detector set is validated before the first chunk.
    let claims = {
        use medulla_core::sensitive_scan::{scan_chunks, NoResources};
        use medulla_h_jobs::jobs::sensitive_claims::{claim_methods, load_detectors};
        let (detectors, limits) = match load_detectors(ctx.core, ctx.verified_context.tenant()) {
            Ok(loaded) => loaded,
            Err(refusal) => {
                return Response::err(req_id, format!("{}: {}", refusal.code, refusal.detail))
            }
        };
        let chunk_texts: Vec<&str> = spans
            .iter()
            .map(|span| span.text(&document.text))
            .collect();
        let detections = match scan_chunks(
            &doc_id,
            &chunk_texts,
            &detectors,
            &limits,
            medulla_server_api::knob_config::knobs().sensitive_max_chunks as usize,
            &NoResources,
            i64::try_from(medulla_kernel::kernel::dispatch_support::authoritative_now_ms())
                .unwrap_or(i64::MAX),
        ) {
            Ok(detections) => detections,
            Err(error) => return Response::err(req_id, format!("{}: {error}", error.code())),
        };
        match claim_methods(
            ctx.core,
            ctx.verified_context.tenant(),
            ctx.verified_context.agent_id(),
            &detectors,
            &detections,
        ) {
            Ok(methods) => methods,
            Err(error) => return Response::err(req_id, error),
        }
    };
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
        demoted_classes: demoted_labels,
        sections: &sections,
        tree_check,
        links,
        fact_supersession,
        claims,
    };
    match web::lower(plan) {
        Ok((write_set, receipt)) => {
            commit(
                ctx,
                write_set,
                &web::LABELS,
                receipt,
                Some(&document.digest_hex),
            )
            .await
        }
        Err(error) => Response::err(req_id, error),
    }
}

/// The content classes retrieval feedback has demoted for this tenant
/// (MEDULLA-DECISION-ENGINE-R079.5): the cited span ids the decision log
/// recorded inside the window, joined to the class of each embedded chunk.
/// With no Agent Library open there is no usage, so nothing is demoted.
#[cfg(all(feature = "query", feature = "modality-serving"))]
async fn demoted_classes(
    ctx: &GraphOpRouting<'_>,
    chunk_classes: &std::collections::BTreeMap<
        String,
        medulla_decision_types::decision::ingestion_admission::ContentClass,
    >,
    knobs: &medulla_decision_types::decision::ingestion_admission::AdmissionKnobs,
) -> Result<
    std::collections::BTreeSet<medulla_decision_types::decision::ingestion_admission::ContentClass>,
    String,
> {
    use medulla_persistence_records::agent_library::AgentLibraryStateExt;
    let Some(store) = ctx.state.read().await.agent_library() else {
        return Ok(Default::default());
    };
    let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let cited = store.cited_span_ids_since(
        ctx.verified_context.tenant(),
        now_ms.saturating_sub(knobs.demote_window_ms),
    )?;
    let usage = medulla_h_ingest::ingestion_admission::class_usage(chunk_classes, &cited);
    Ok(medulla_h_ingest::ingestion_admission::demoted_classes(
        &usage, knobs,
    ))
}

/// An entity document (`application/json` or `application/ld+json`) lowers to
/// alignment claims instead of chunks (MEDULLA-DECISION-ENGINE-R099.2). The
/// mapping is resolved from the caller's imported connector pack, never taken
/// from the request body; the local entities are the RLS-visible nodes of the
/// mapping's class and the visible `AlignmentClaim` nodes.
#[cfg(all(feature = "query", feature = "modality-serving"))]
async fn enrich_alignment(
    ctx: GraphOpRouting<'_>,
    request: &medulla_source_types::source_ingestion::web_document::EnrichDocumentRequest,
    doc_id: &str,
    document: medulla_h_ingest::web_ingest::HeldDocument,
) -> Response {
    use medulla_h_ingest::web_ingest as web;
    let req_id = ctx.req_id;
    let Some(selection) = request.alignment.as_ref() else {
        return Response::err(
            req_id,
            format!(
                "{}: the entity document names no alignment mapping",
                web::ALIGNMENT_NO_MAPPING
            ),
        );
    };
    let mapping = match resolve_alignment_mapping(&ctx, selection).await {
        Ok(mapping) => mapping,
        Err(error) => return Response::err(req_id, error),
    };
    let local: Vec<web::LocalEntity> = [mapping.ontology_class.as_str(), web::ALIGNMENT_CLAIM_TYPE]
        .into_iter()
        .flat_map(|label| ctx.core.get_nodes_by_label(label, 0))
        .filter(|(id, _)| visible(&ctx, id))
        .filter_map(|(id, bytes)| web::LocalEntity::decode(id, &bytes))
        .collect();
    match web::lower_alignment(doc_id, &document, &mapping, &local) {
        Ok((Some(write_set), receipt)) => {
            commit(ctx, write_set, &web::ALIGNMENT_LABELS, receipt, None).await
        }
        Ok((None, receipt)) => respond(req_id, receipt),
        Err(error) => Response::err(req_id, error),
    }
}

#[cfg(all(feature = "query", feature = "modality-serving", feature = "redb"))]
async fn resolve_alignment_mapping(
    ctx: &GraphOpRouting<'_>,
    selection: &medulla_source_types::source_ingestion::web_document::AlignmentMappingRef,
) -> Result<medulla_catalog_types::connector_pack::ConnectorSchemaMapping, String> {
    use medulla_persistence_records::agent_library::AgentLibraryStateExt;
    let Some(agent_library) = ctx.state.read().await.agent_library() else {
        return Err(
            "CONNECTOR_SCHEMA_MAPPING_UNAVAILABLE: no connector pack is imported for this tenant"
                .to_string(),
        );
    };
    let pack_tenant =
        medulla_persistence_records::connector_pack::pack_tenant_key(ctx.verified_context);
    agent_library
        .resolve_connector_schema_mapping(
            pack_tenant,
            &selection.connector,
            &selection.mapping_reference,
        )
        .map(|resolved| resolved.schema_mapping)
}

#[cfg(all(feature = "query", feature = "modality-serving", not(feature = "redb")))]
async fn resolve_alignment_mapping(
    _ctx: &GraphOpRouting<'_>,
    _selection: &medulla_source_types::source_ingestion::web_document::AlignmentMappingRef,
) -> Result<medulla_catalog_types::connector_pack::ConnectorSchemaMapping, String> {
    Err("ENRICHMENT_UNAVAILABLE: alignment mappings require the redb feature".to_string())
}

#[cfg(all(feature = "query", feature = "modality-serving"))]
async fn commit(
    ctx: GraphOpRouting<'_>,
    write_set: medulla_h_ingest::repository_index::IndexWriteSet,
    labels: &medulla_h_ingest::repository_index::EnvelopeLabels,
    mut receipt: medulla_source_types::source_ingestion::web_document::EnrichDocumentReceipt,
    extraction_digest: Option<&str>,
) -> Response {
    let (placement_epoch, fencing_token) = match source_ingestion_route_fence(&ctx).await {
        Ok(fence) => fence,
        Err(response) => return response,
    };
    let commit_ctx = commit_context(&ctx, placement_epoch, fencing_token);
    let envelope = match medulla_h_ingest::repository_index::build_labelled_envelope(
        &commit_ctx,
        write_set,
        labels,
    ) {
        Ok(envelope) => envelope,
        Err(error) => return Response::err(ctx.req_id, error),
    };
    let committed =
        crate::dispatch::change_envelope::apply_one_change_envelope(ctx, envelope).await;
    match committed.refusal_text() {
        Some(refusal) => Response::err(committed.id, refusal),
        None => {
            if let Some(digest) = extraction_digest {
                let chunks = receipt.chunk_ids.clone();
                receipt.warnings =
                    enqueue_extraction(ctx, &receipt.document_id, digest, &chunks).await;
            }
            respond(committed.id, receipt)
        }
    }
}

/// Enqueue one extraction job per chunk of the enriched Document, after the enrich commit
/// (MEDULLA-UNIFIED-DATA-PLANE-R651.7.7). Every job goes through the one ingest enqueue path
/// (`enqueue_ingest_job`) on the `ingest` queue; its id is
/// `<document_id>:<digest_hex>:<chunk_ordinal>`, so a repeat of the same enrich enqueues
/// nothing new. The caller is the verified caller of `EnrichDocument`. A refusal other than a
/// duplicate comes back as a warning and never undoes the enrich.
#[cfg(all(feature = "query", feature = "modality-serving", feature = "jobs"))]
async fn enqueue_extraction(
    ctx: GraphOpRouting<'_>,
    document_id: &str,
    digest_hex: &str,
    chunks: &[String],
) -> Vec<String> {
    use medulla_source_types::ingest_job::{IngestJobEnvelope, IngestJobKind, IngestRetryPolicy};
    if chunks.is_empty() {
        return Vec::new();
    }
    let unavailable = |detail: String| vec![format!("EXTRACTION_ENQUEUE_FAILED: {detail}")];
    let (persist_dir, config_store) = {
        let guard = ctx.state.read().await;
        let store = match medulla_persistence::scheduler_config_store::shared_scheduler_config_store(
            &guard,
        ) {
            Ok(store) => store,
            Err(error) => return unavailable(error.to_string()),
        };
        (guard.persist_dir.clone(), store)
    };
    let config = match medulla_h_jobs::scheduler_config::effective_config(&config_store) {
        Ok(config) => config,
        Err(error) => return unavailable(error),
    };
    let jobs = medulla_server_ops::api::ingest::jobs_seam(ctx.state).await;
    let tenant = ctx.verified_context.tenant();
    let queue = medulla_source_types::ingest_job::lane_for(IngestJobKind::Extraction).queue_name();
    let mut depth = match jobs.as_ref() {
        Some(jobs) => match jobs.task_queue_load(&persist_dir, tenant, queue) {
            Ok((queued, running, _)) => queued.saturating_add(running),
            Err(error) => return unavailable(error),
        },
        None => 0,
    };
    let caller = medulla_h_ingest::ingest_queue::IngestCaller {
        persist_dir,
        tenant: tenant.to_string(),
        principal: ctx.verified_context.principal().to_string(),
    };
    let policy = IngestRetryPolicy::from_limits(&config.ingest_retry);
    let mut warnings = Vec::new();
    for (ordinal, chunk) in chunks.iter().enumerate() {
        let envelope = IngestJobEnvelope {
            job_id: format!("{document_id}:{digest_hex}:{ordinal}"),
            tenant: tenant.to_string(),
            corpus_key: document_id.to_string(),
            kind: IngestJobKind::Extraction,
            target: chunk.clone(),
            attempt: 0,
            page: None,
        };
        match medulla_h_ingest::ingest_queue::enqueue_ingest_job(
            jobs.clone(),
            &caller,
            &envelope,
            &policy,
            depth,
            config.max_queue_depth,
        ) {
            Ok(receipt) => {
                if !receipt.deduplicated {
                    depth = depth.saturating_add(1);
                }
            }
            Err(error) => warnings.push(format!(
                "EXTRACTION_ENQUEUE_REFUSED: chunk '{chunk}': {}: {}",
                error.code.as_str(),
                error.detail
            )),
        }
    }
    warnings
}

/// A build without the `jobs` feature has no ingest queue to enqueue on.
#[cfg(all(feature = "query", feature = "modality-serving", not(feature = "jobs")))]
async fn enqueue_extraction(
    _ctx: GraphOpRouting<'_>,
    _document_id: &str,
    _digest_hex: &str,
    chunks: &[String],
) -> Vec<String> {
    if chunks.is_empty() {
        return Vec::new();
    }
    vec!["EXTRACTION_ENQUEUE_FAILED: UNAVAILABLE: ingest enqueue requires the `jobs` feature".to_string()]
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
