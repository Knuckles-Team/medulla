//! The item catalog of an `item_recommendation` question and the encoder a
//! `FitCatalogEncoder` write stores for it (MEDULLA-DECISION-ENGINE-R175.3,
//! R175.8).
//!
//! A catalog is the embedded nodes of one graph, as the caller may see them:
//! the item id is the node id, the vector is the node's stored embedding and
//! the time bucket is the node's `time_bucket` property (0 when absent). Its
//! digest is the SHA-256 over the sorted item ids and vectors; the encoder
//! fitted on a catalog is stored under that digest, so a changed catalog never
//! reads a stale encoder.

use sha2::{Digest, Sha256};

use medulla_decision_stats::decision::recommend::{fit_codebooks, CatalogItem, RecommendError};
use medulla_decision_types::decision::item_recommendation::{
    encoder_key, CatalogEncoderFitted, DIMENSION_MISMATCH, EMPTY_CATALOG,
};
use medulla_decision_types::decision::knob_families::recommend::RecommendKnobs;
use medulla_decision_types::decision::statistical::{DecideRequest, QuestionKind};
use medulla_agent_types::agent_component::DecisionPolicyRef;
use medulla_decision_outcome_types::decision::recommend::RecommendRequest;
use medulla_persistence_records::decision_jobs::encode_artifact;

use super::family::json_param;
use super::stat_executor::ExecutionContext;
use super::stat_support::{refusal, resolve_policy};
use super::stat_vectors::{graph_vectors, GraphVectors};
use super::SharedState;
use medulla_decision_types::decision::statistical::StatisticalErrorCode;

/// The param an `item_recommendation` question carries its [`RecommendRequest`] in, as JSON text.
pub(super) const PARAM_RECOMMEND: &str = "recommend";

/// The embedded items of one catalog graph and their digest.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct CatalogRead {
    /// The items in byte order of their ids.
    pub(super) items: Vec<CatalogItem>,
    /// SHA-256 over the sorted item ids and vectors.
    pub(super) digest: [u8; 32],
}

impl CatalogRead {
    /// Every embedded node of `graph` the caller may see.
    pub(super) fn read(graph: &GraphVectors) -> Self {
        let mut items = Vec::new();
        for id in graph.node_ids() {
            let Some(vector) = graph.stored_vector(&id) else {
                continue;
            };
            let time_bucket = graph
                .visible_properties(&id)
                .and_then(|node| node.get("time_bucket")?.as_u64())
                .and_then(|bucket| u16::try_from(bucket).ok())
                .unwrap_or(0);
            items.push(CatalogItem {
                item_id: id,
                time_bucket,
                vector,
            });
        }
        let mut hasher = Sha256::new();
        for item in &items {
            hasher.update((item.item_id.len() as u32).to_le_bytes());
            hasher.update(item.item_id.as_bytes());
            hasher.update((item.vector.len() as u32).to_le_bytes());
            for value in &item.vector {
                hasher.update(value.to_le_bytes());
            }
        }
        Self {
            items,
            digest: hasher.finalize().into(),
        }
    }

    /// The digest as 64 lowercase hex characters.
    pub(super) fn digest_hex(&self) -> String {
        hex::encode(self.digest)
    }

    /// The width of the item vectors; 0 for an empty catalog.
    pub(super) fn dimension(&self) -> usize {
        self.items.first().map_or(0, |item| item.vector.len())
    }
}

/// The catalog an `item_recommendation` question ranks, read from the graph its
/// `recommend` param names; `None` for every other kind.
pub(super) async fn read_for_question(
    state: &SharedState,
    agent_id: &str,
    request: &DecideRequest,
) -> Result<Option<CatalogRead>, String> {
    if request.question.kind != QuestionKind::ItemRecommendation {
        return Ok(None);
    }
    let recommend: RecommendRequest = json_param(request, PARAM_RECOMMEND)?;
    let graph = graph_vectors(state, agent_id, &recommend.catalog_ref).await?;
    Ok(Some(CatalogRead::read(&graph)))
}

fn refused(error: RecommendError) -> String {
    match error {
        RecommendError::EmptyCatalog => format!(
            "{}: the catalog graph holds no embedded item",
            EMPTY_CATALOG.as_str()
        ),
        RecommendError::DimensionMismatch => format!(
            "{}: the catalog's vectors differ in width",
            DIMENSION_MISMATCH.as_str()
        ),
        RecommendError::KnobOutOfRange => refusal(
            StatisticalErrorCode::ParameterInvalid,
            "a recommend knob is outside its range",
        ),
    }
}

/// Fit the encoder of `catalog` under the engine default policy's `recommend.*`
/// knobs and store it under the catalog's digest. Fitting the same catalog twice
/// stores the same bytes, which is a replay.
pub(super) fn fit_catalog_encoder(
    ctx: &ExecutionContext,
    catalog_ref: &str,
    catalog: &CatalogRead,
) -> Result<CatalogEncoderFitted, String> {
    let policy = resolve_policy(ctx.store, ctx.tenant_id, &DecisionPolicyRef::Default)?;
    let knobs = RecommendKnobs::from_policy(&policy.policy);
    let encoder = fit_codebooks(&catalog.items, &catalog.digest, &knobs).map_err(refused)?;
    let digest = catalog.digest_hex();
    ctx.store.put_decision_artifacts(
        ctx.tenant_id,
        &[(encoder_key(catalog_ref, &digest), encode_artifact(&encoder)?)],
    )?;
    Ok(CatalogEncoderFitted {
        catalog_ref: catalog_ref.to_string(),
        catalog_digest: digest,
        items: u32::try_from(catalog.items.len()).unwrap_or(u32::MAX),
        dimension: u32::try_from(encoder.dimension).unwrap_or(u32::MAX),
        levels: u8::try_from(encoder.codebooks.len()).unwrap_or(u8::MAX),
    })
}
