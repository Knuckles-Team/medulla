//! The graph-scoped route of `SemanticEnrich` (MEDULLA-DECISION-ENGINE-R614.3).
//!
//! The call arrives after graph ACL, tenant binding, lazy-open and placement
//! lookup. Every op first proves it was scheduled: the request names a
//! committed `EnrichmentSchedule` decision that chose the work id
//! `semantic:<op>:<subject>` and that this caller may see; otherwise it refuses
//! `ENRICHMENT_NOT_SCHEDULED` before reading anything. The behaviours are the
//! pure functions of `medulla_enrich::semantic`; their results lower to one
//! digest-keyed write-set that commits through the existing ChangeEnvelope
//! authority, exactly like `EnrichDocument`. A refusal writes nothing.

#[cfg(all(feature = "query", feature = "modality-serving"))]
use super::repository_index::commit_context;
use super::*;

/// Serve one `SemanticEnrich` op.
pub async fn serve(
    ctx: GraphOpRouting<'_>,
    op: medulla_types::semantic_enrichment::SemanticEnrichOp,
) -> Response {
    imp::serve(ctx, op).await
}

#[cfg(all(feature = "query", feature = "modality-serving"))]
mod imp {
    use std::collections::{BTreeMap, BTreeSet};

    use medulla_decision_types::decision::statistical::log::{DecisionLogEntry, RecordVisibility};
    use medulla_decision_types::decision::statistical::outcome::StatisticalOutcome;
    use medulla_decision_types::decision::statistical::QuestionKind;
    use medulla_enrich::semantic::{
        classify_with_floor, project_edges, rank_candidates_top, store_card, CapabilityCard,
        ConceptInput, EnrichmentKnobs, ProjectionNode, ProjectionRule, TopicCentroid,
    };
    use medulla_enrich::{EnrichError, Id};
    use medulla_h_ingest::repository_index::IndexWriteSet;
    use medulla_h_ingest::semantic_enrichment as lower;
    use medulla_persistence_records::agent_library::AgentLibraryStateExt;
    use medulla_persistence_records::decision_jobs::{decode_artifact, record_key};
    use medulla_server_api::knob_config::knobs;
    use medulla_types::semantic_enrichment::{
        SemanticEnrichOp, SemanticEnrichReceipt, ENRICHMENT_NOT_SCHEDULED,
    };

    use super::super::*;
    use super::commit_context;

    fn visible(ctx: &GraphOpRouting<'_>, node_id: &str) -> bool {
        ctx.read_authority
            .as_ref()
            .is_none_or(|authority| authority.node_visible(ctx.core, node_id))
    }

    /// The visible node's properties as a JSON value, when it exists.
    fn visible_node(ctx: &GraphOpRouting<'_>, node_id: &str) -> Option<serde_json::Value> {
        let bytes = ctx.core.get_node_properties(node_id)?;
        if !visible(ctx, node_id) {
            return None;
        }
        medulla_types::msgpack::decode_property_value(&bytes).ok()
    }

    fn refuse(ctx: &GraphOpRouting<'_>, error: EnrichError) -> Response {
        Response::err(ctx.req_id, error.to_string())
    }

    fn id(ctx: &GraphOpRouting<'_>, value: &str) -> Result<Id, Response> {
        Id::new(value).map_err(|error| refuse(ctx, error))
    }

    fn not_scheduled(ctx: &GraphOpRouting<'_>, why: &str) -> Response {
        Response::err(ctx.req_id, format!("{ENRICHMENT_NOT_SCHEDULED}: {why}"))
    }

    /// The call must name a committed, visible `EnrichmentSchedule` decision
    /// that acted on (or explored) exactly this op's work id.
    async fn check_schedule(
        ctx: &GraphOpRouting<'_>,
        op: &SemanticEnrichOp,
    ) -> Result<(), Response> {
        let store = ctx.state.read().await.agent_library();
        let Some(store) = store else {
            return Err(not_scheduled(ctx, "no committed schedule decision exists"));
        };
        let tenant = ctx.verified_context.tenant();
        let principal = ctx.verified_context.principal_persistence_id();
        let bytes = store
            .decision_artifact(tenant, &record_key(op.schedule_record_id()))
            .map_err(|error| Response::err(ctx.req_id, error))?;
        let Some(bytes) = bytes else {
            return Err(not_scheduled(
                ctx,
                &format!("no decision '{}' is committed", op.schedule_record_id()),
            ));
        };
        let entry: DecisionLogEntry = decode_artifact(&bytes, "decision log entry")
            .map_err(|error| Response::err(ctx.req_id, error))?;
        let seen = match &entry.visibility {
            RecordVisibility::Tenant => true,
            RecordVisibility::Principal { principal: owner } => *owner == principal,
        };
        if !seen {
            return Err(not_scheduled(
                ctx,
                &format!("no decision '{}' is committed", op.schedule_record_id()),
            ));
        }
        if entry.record.question.kind != QuestionKind::EnrichmentSchedule {
            return Err(not_scheduled(
                ctx,
                &format!(
                    "decision '{}' is not an enrichment schedule",
                    op.schedule_record_id()
                ),
            ));
        }
        let chosen = match &entry.record.outcome {
            StatisticalOutcome::Acted { option_id, .. }
            | StatisticalOutcome::Explored { option_id, .. } => Some(option_id.as_str()),
            StatisticalOutcome::Advisory { .. } | StatisticalOutcome::Abstained { .. } => None,
        };
        if chosen != Some(op.work_id().as_str()) {
            return Err(not_scheduled(
                ctx,
                &format!(
                    "decision '{}' did not choose '{}'",
                    op.schedule_record_id(),
                    op.work_id()
                ),
            ));
        }
        Ok(())
    }

    fn receipt(op: &SemanticEnrichOp) -> SemanticEnrichReceipt {
        SemanticEnrichReceipt {
            op: op.name().to_string(),
            subject: op.subject().to_string(),
            schedule_record_id: op.schedule_record_id().to_string(),
            node_ids: Vec::new(),
            edge_count: 0,
            unresolved: 0,
            topic: None,
            confidence_bp: None,
            more: false,
        }
    }

    fn enrichment_knobs(ctx: &GraphOpRouting<'_>) -> Result<EnrichmentKnobs, Response> {
        let knobs = knobs();
        let narrow = |value: u64| u32::try_from(value).unwrap_or(u32::MAX);
        EnrichmentKnobs::new(
            narrow(knobs.enrichment_proposal_top_n),
            narrow(knobs.enrichment_topic_min_confidence_bp),
            narrow(knobs.enrichment_projection_batch),
        )
        .map_err(|error| refuse(ctx, error))
    }

    fn string_properties(value: &serde_json::Value) -> BTreeMap<String, String> {
        value
            .as_object()
            .map(|map| {
                map.iter()
                    .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) async fn serve(ctx: GraphOpRouting<'_>, op: SemanticEnrichOp) -> Response {
        if let Err(response) = check_schedule(&ctx, &op).await {
            return response;
        }
        let mut receipt = receipt(&op);
        let write_set = match &op {
            SemanticEnrichOp::Card { .. } => card(&ctx, &op, &mut receipt),
            SemanticEnrichOp::RankProposals { .. } => proposals(&ctx, &op, &mut receipt),
            SemanticEnrichOp::Classify { .. } => classify(&ctx, &op, &mut receipt).await,
            SemanticEnrichOp::Project { .. } => project(&ctx, &op, &mut receipt),
        };
        match write_set {
            Ok(Some(write_set)) => commit(ctx, write_set, receipt).await,
            Ok(None) => respond(ctx.req_id, receipt),
            Err(response) => response,
        }
    }

    fn card(
        ctx: &GraphOpRouting<'_>,
        op: &SemanticEnrichOp,
        receipt: &mut SemanticEnrichReceipt,
    ) -> Result<Option<IndexWriteSet>, Response> {
        let SemanticEnrichOp::Card {
            schedule_record_id,
            subject,
            summary,
            model,
            prompt_digest,
            source_fact_ids,
        } = op
        else {
            unreachable!("card is called for a Card op")
        };
        let subject_id = id(ctx, subject)?;
        let mut cited = Vec::with_capacity(source_fact_ids.len());
        let mut known = BTreeSet::new();
        for fact in source_fact_ids {
            let fact_id = id(ctx, fact)?;
            if visible_node(ctx, fact).is_some() {
                known.insert(fact_id.clone());
            }
            cited.push(fact_id);
        }
        let card = CapabilityCard {
            id: id(ctx, &format!("card:{subject}"))?,
            subject: subject_id,
            summary: summary.clone(),
            model: id(ctx, model)?,
            prompt_digest: prompt_digest.clone(),
            source_fact_ids: cited,
        };
        let stored = store_card(&card, &known).map_err(|error| refuse(ctx, error))?;
        receipt.node_ids = vec![stored.id.as_str().to_string()];
        receipt.edge_count = 1;
        lower::lower_card(&stored, schedule_record_id)
            .map_err(|error| Response::err(ctx.req_id, error))
    }

    fn proposals(
        ctx: &GraphOpRouting<'_>,
        op: &SemanticEnrichOp,
        receipt: &mut SemanticEnrichReceipt,
    ) -> Result<Option<IndexWriteSet>, Response> {
        let SemanticEnrichOp::RankProposals {
            schedule_record_id,
            subject,
            concepts,
        } = op
        else {
            unreachable!("proposals is called for a RankProposals op")
        };
        let knobs = enrichment_knobs(ctx)?;
        let inputs = concepts
            .iter()
            .map(|concept| {
                Ok(ConceptInput {
                    concept: id(ctx, &concept.concept)?,
                    impact_bp: concept.impact_bp,
                    confidence_bp: concept.confidence_bp,
                    cost_units: concept.cost_units,
                })
            })
            .collect::<Result<Vec<_>, Response>>()?;
        let ranked = rank_candidates_top(&inputs, knobs.proposal_top_n)
            .map_err(|error| refuse(ctx, error))?;
        receipt.node_ids = ranked
            .iter()
            .map(|candidate| lower::proposal_id(subject, candidate.concept.as_str()))
            .collect();
        lower::lower_proposals(subject, &ranked, concepts, schedule_record_id)
            .map_err(|error| Response::err(ctx.req_id, error))
    }

    async fn classify(
        ctx: &GraphOpRouting<'_>,
        op: &SemanticEnrichOp,
        receipt: &mut SemanticEnrichReceipt,
    ) -> Result<Option<IndexWriteSet>, Response> {
        let SemanticEnrichOp::Classify {
            schedule_record_id,
            subject,
            taxonomy,
        } = op
        else {
            unreachable!("classify is called for a Classify op")
        };
        let knobs = enrichment_knobs(ctx)?;
        let document = id(ctx, subject)?;
        let Some(properties) = visible_node(ctx, subject) else {
            return Err(Response::err(
                ctx.req_id,
                format!("DOCUMENT_NOT_FOUND: no Document '{subject}'"),
            ));
        };
        let embedder = medulla_server_ops::api::ingest::embedder::EmbedderConfig::from_env();
        let held = ctx.core.semantic_store.read().get_embedding(subject);
        let vector = match held {
            Some(vector) => vector,
            None => {
                let text = ["text", "content", "body"]
                    .iter()
                    .find_map(|name| properties.get(*name).and_then(|v| v.as_str()))
                    .filter(|text| !text.trim().is_empty())
                    .ok_or_else(|| {
                        Response::err(
                            ctx.req_id,
                            "DOCUMENT_TEXT_MISSING: the Document holds no text to classify",
                        )
                    })?
                    .to_string();
                let worker = embedder.clone();
                let embedded =
                    medulla_server_api::compute::compute_off_lock(ctx.req_id, move || {
                        worker.embed(&[text.as_str()])
                    })
                    .await;
                match embedded {
                    Ok(Ok(mut vectors)) if vectors.len() == 1 => vectors.remove(0),
                    Ok(Ok(_)) => {
                        return Err(Response::err(
                            ctx.req_id,
                            "EMBEDDING_UNAVAILABLE: the embedder returned no vector",
                        ))
                    }
                    Ok(Err(error)) => return Err(Response::err(ctx.req_id, error)),
                    Err(response) => return Err(response),
                }
            }
        };
        let mut topics = Vec::new();
        {
            let store = ctx.core.semantic_store.read();
            for (topic_id, bytes) in ctx.core.get_nodes_by_label("Topic", 0) {
                if !visible(ctx, &topic_id) {
                    continue;
                }
                let Ok(value) = medulla_types::msgpack::decode_property_value(&bytes) else {
                    continue;
                };
                if value.get("taxonomy").and_then(|v| v.as_str()) != Some(taxonomy.as_str()) {
                    continue;
                }
                let Some(centroid) = store.get_embedding(&topic_id) else {
                    continue;
                };
                topics.push(TopicCentroid {
                    topic: id(ctx, &topic_id)?,
                    centroid,
                });
            }
        }
        topics.sort_by(|a, b| a.topic.cmp(&b.topic));
        let assignment =
            classify_with_floor(&document, &vector, &topics, knobs.topic_min_confidence_bp)
                .map_err(|error| refuse(ctx, error))?;
        let Some(assignment) = assignment else {
            return Ok(None);
        };
        receipt.node_ids = vec![lower::assignment_id(subject, taxonomy)];
        receipt.edge_count = 1;
        receipt.topic = Some(assignment.topic.as_str().to_string());
        receipt.confidence_bp = Some(assignment.confidence.basis_points());
        lower::lower_assignment(&assignment, taxonomy, embedder.name(), schedule_record_id)
            .map_err(|error| Response::err(ctx.req_id, error))
    }

    fn project(
        ctx: &GraphOpRouting<'_>,
        op: &SemanticEnrichOp,
        receipt: &mut SemanticEnrichReceipt,
    ) -> Result<Option<IndexWriteSet>, Response> {
        let SemanticEnrichOp::Project { subject, rules, .. } = op else {
            unreachable!("project is called for a Project op")
        };
        let knobs = enrichment_knobs(ctx)?;
        let rules = rules
            .iter()
            .map(|rule| {
                Ok(ProjectionRule {
                    id: id(ctx, &rule.id)?,
                    taxonomy: id(ctx, &rule.taxonomy)?,
                    property: id(ctx, &rule.property)?,
                    relation: id(ctx, &rule.relation)?,
                })
            })
            .collect::<Result<Vec<_>, Response>>()?;
        let mut known_taxonomies = BTreeSet::new();
        for (topic_id, bytes) in ctx.core.get_nodes_by_label("Topic", 0) {
            if !visible(ctx, &topic_id) {
                continue;
            }
            if let Some(name) = medulla_types::msgpack::decode_property_value(&bytes)
                .ok()
                .and_then(|value| value.get("taxonomy")?.as_str().map(str::to_string))
            {
                if let Ok(taxonomy) = Id::new(name) {
                    known_taxonomies.insert(taxonomy);
                }
            }
        }
        // The rules are checked once up front, so a node-less label still
        // refuses an undeclared taxonomy.
        let probe = ProjectionNode {
            id: id(ctx, subject)?,
            properties: BTreeMap::new(),
        };
        project_edges(&probe, &rules, &known_taxonomies, |_| false)
            .map_err(|error| refuse(ctx, error))?;
        let mut nodes: Vec<(String, Vec<u8>)> = ctx
            .core
            .get_nodes_by_label(subject, 0)
            .into_iter()
            .filter(|(node_id, _)| visible(ctx, node_id))
            .collect();
        nodes.sort_by(|a, b| a.0.cmp(&b.0));
        let batch = usize::try_from(knobs.projection_batch).unwrap_or(usize::MAX);
        receipt.more = nodes.len() > batch;
        nodes.truncate(batch);
        let mut edges = Vec::new();
        let mut seen = BTreeSet::new();
        for (node_id, bytes) in nodes {
            let Ok(value) = medulla_types::msgpack::decode_property_value(&bytes) else {
                continue;
            };
            let node = ProjectionNode {
                id: id(ctx, &node_id)?,
                properties: string_properties(&value),
            };
            let projection = project_edges(&node, &rules, &known_taxonomies, |target| {
                visible_node(ctx, target.as_str()).is_some()
            })
            .map_err(|error| refuse(ctx, error))?;
            receipt.unresolved += projection.unresolved;
            for edge in projection.edges {
                if seen.insert(edge.id.clone()) {
                    edges.push(edge);
                }
            }
        }
        receipt.edge_count = u32::try_from(edges.len()).unwrap_or(u32::MAX);
        lower::lower_projection(&edges).map_err(|error| Response::err(ctx.req_id, error))
    }

    async fn commit(
        ctx: GraphOpRouting<'_>,
        write_set: IndexWriteSet,
        receipt: SemanticEnrichReceipt,
    ) -> Response {
        let (placement_epoch, fencing_token) = match source_ingestion_route_fence(&ctx).await {
            Ok(fence) => fence,
            Err(response) => return response,
        };
        let commit_ctx = commit_context(&ctx, placement_epoch, fencing_token);
        let envelope = match medulla_h_ingest::repository_index::build_labelled_envelope(
            &commit_ctx,
            write_set,
            &lower::LABELS,
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

    fn respond(req_id: u64, receipt: SemanticEnrichReceipt) -> Response {
        Response::ok(
            req_id,
            ResultPayload::of::<medulla_protocol_types::result_contract::ingestion::SemanticEnrich>(
                receipt,
            ),
        )
    }
}

#[cfg(not(all(feature = "query", feature = "modality-serving")))]
mod imp {
    use super::super::*;

    pub(super) async fn serve(
        ctx: GraphOpRouting<'_>,
        _op: medulla_types::semantic_enrichment::SemanticEnrichOp,
    ) -> Response {
        Response::err(
            ctx.req_id,
            "ENRICHMENT_UNAVAILABLE: semantic enrichment requires the query and modality-serving features",
        )
    }
}
