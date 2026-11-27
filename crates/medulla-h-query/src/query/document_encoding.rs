//! Served document-chunk encoding (MEDULLA-FEDERATED-QUERY-R039.2.5 to R039.2.7):
//! `EncodeDocument` and `EncodedDocumentSearch`.
//!
//! `EncodeDocument` is the one executor of the approved encoding recipe. It runs
//! `split_windows`, `encode_chunks` and `aggregate_mean_chunks` of
//! `medulla_core::compute::chunk_windows` and publishes the generation only after
//! every chunk has validated: a refused encoding returns before any node is written, so
//! the prior generation keeps answering. A published document is one node of label
//! `EncodedDocument` (`encoded_document:<tenant>:<policy digest>:<id>`) holding the single vector and
//! its receipt. One head node per tenant and policy digest (`encoded_index_head:<tenant>:<digest>`)
//! holds the vector generation, so vectors of two policies never mix in one ranking. Both are written through the staged graph of the
//! query gateway, so the document, the head and the generation commit together or not
//! at all. Ingestion and search share one policy digest and one generation: a search
//! whose policy digest has no generation refuses `ENCODE_POLICY_STALE` before ranking.

use std::time::Instant;

use medulla_core::compute::chunk_windows::{
    aggregate_mean_chunks, encode_chunks, split_windows, ChunkEncoder, ChunkRefusal, PositionMode,
};
use medulla_exec::exec::TextEmbedder;
use medulla_identity_types::identity::UnitPrincipal;
use medulla_index_types::embedding::{
    DocumentAggregation, DocumentEncodingPolicy, DocumentPositionMode, EmbeddingSpaceRef, Modality,
    DOCUMENT_ENCODING_POLICY_REVISION,
};
use medulla_protocol_types::result_contract::query as results;
use medulla_server_api::read_scope::ReadCaller;
use medulla_types::contract::ServerErrorCode;
use medulla_types::retrieval::{
    sha256_hex, EncodedDocument, EncodedHit, EncodedSearchRequest, EncodedSearchResult,
    EncodingReceipt,
};
use serde_json::{json, Value};

use super::*;

const DOCUMENT_LABEL: &str = "EncodedDocument";
const HEAD_LABEL: &str = "EncodedIndexHead";
/// The tokenizer identity of the default policy: the caller supplies token ids.
const TOKEN_ID_TOKENIZER: &str = "token-ids";
/// The registry id of the default chunk encoder (the pinned tiny encoder CI binds) and the
/// model revision label the default policy records in its receipts.
pub(crate) const BOUND_ENCODER_REVISION: &str = "builtin:test-hashed-token/1";
/// The space the default entry registers for.
const BOUND_ENCODER_SPACE: &str = "builtin-test-hashed-token";
/// The space revision the default entry registers for.
const BOUND_ENCODER_SPACE_REVISION: &str = "1";
/// Width of the default encoder's vectors.
const BOUND_ENCODER_DIM: usize = 8;
/// A window holding this token id makes the default encoder emit a non-finite vector.
pub(crate) const POISON_NON_FINITE_TOKEN: u32 = u32::MAX;
/// A window holding this token id makes the default encoder emit a vector of another width.
pub(crate) const POISON_SHORT_TOKEN: u32 = u32::MAX - 1;

/// The pinned tiny encoder CI binds: a position-aware hashed bag of token ids. It holds
/// no state, so the same window always gives the same vector. The two poison token ids
/// are part of its contract and let a served test make one chunk fail. It is a registered
/// entry of `ModalityEncoderRegistry` like every other encoder, so chunk encoding has one
/// encoder authority.
struct BoundEncoder;

impl BoundEncoder {
    fn encode_at(&self, window: &[u32], position_base: usize) -> Vec<f32> {
        if window.contains(&POISON_NON_FINITE_TOKEN) {
            return vec![f32::NAN; BOUND_ENCODER_DIM];
        }
        if window.contains(&POISON_SHORT_TOKEN) {
            return vec![1.0; BOUND_ENCODER_DIM / 2];
        }
        let mut vector = vec![0f32; BOUND_ENCODER_DIM];
        for (offset, token) in window.iter().enumerate() {
            let bucket = (token.wrapping_mul(2_654_435_761) >> 7) as usize % BOUND_ENCODER_DIM;
            let position = position_base + offset;
            vector[bucket] += 1.0 + (position % 5) as f32 * 0.25;
        }
        vector
    }
}

impl TextEmbedder for BoundEncoder {
    /// Text reaches the default encoder as space-separated decimal token ids.
    fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        let window = text
            .split_ascii_whitespace()
            .map(|word| {
                word.parse::<u32>()
                    .map_err(|_| format!("`{word}` is not a token id"))
            })
            .collect::<Result<Vec<u32>, String>>()?;
        Ok(self.encode_at(&window, 0))
    }

    fn embed_window(&self, window: &[u32], position_base: usize) -> Result<Vec<f32>, String> {
        Ok(self.encode_at(window, position_base))
    }
}

/// Register the default chunk encoder in the process registry beside the bound query
/// encoders. Idempotent: a server state may be composed more than once in one process.
pub fn register_default_chunk_encoder() -> Result<(), String> {
    let registry = medulla_plan::embed_registry::modality_encoders();
    if registry.lookup_by_id(BOUND_ENCODER_REVISION).is_ok() {
        return Ok(());
    }
    registry.register(
        BOUND_ENCODER_REVISION,
        BOUND_ENCODER_SPACE,
        BOUND_ENCODER_DIM,
        BOUND_ENCODER_SPACE_REVISION,
        Box::new(BoundEncoder),
    )
}

/// The chunk encoder of one document: every window goes to the registry encoder named by
/// the policy. A document given as text holds the words in `words`, and a window's ids are
/// indices into it; a document given as token ids passes the ids through. An encoder
/// that refuses a window yields an empty vector, which the aggregate refuses.
struct RegistryChunkEncoder<'a> {
    encoder: &'static dyn TextEmbedder,
    words: Option<&'a [&'a str]>,
}

impl ChunkEncoder for RegistryChunkEncoder<'_> {
    fn encode(&self, window: &[u32]) -> Vec<f32> {
        self.encode_at(window, 0)
    }

    fn encode_at(&self, window: &[u32], position_base: usize) -> Vec<f32> {
        let embedded = match self.words {
            Some(words) => {
                let text = window
                    .iter()
                    .filter_map(|index| words.get(*index as usize).copied())
                    .collect::<Vec<&str>>()
                    .join(" ");
                self.encoder.embed(&text)
            }
            None => self.encoder.embed_window(window, position_base),
        };
        embedded.unwrap_or_default()
    }
}

/// A typed refusal: the code text and its detail.
#[derive(Debug)]
struct Refused(String);

fn refuse(code: ServerErrorCode, detail: impl std::fmt::Display) -> Refused {
    Refused(format!("{}: {detail}", code.as_str()))
}

fn chunk_refusal(refusal: ChunkRefusal) -> Refused {
    Refused(refusal.to_string())
}

fn internal(error: impl std::fmt::Display) -> Refused {
    Refused(format!("INTERNAL: {error}"))
}

fn reader_of(ctx: &QueryHandlerCtx<'_>) -> UnitPrincipal {
    let mut principal = ReadCaller::from_authority(ctx.read_authority, ctx.graph_name).principal;
    principal.id = ctx.caller.to_string();
    principal
}

fn head_node(tenant: &str, policy_digest: &str) -> String {
    format!("encoded_index_head:{tenant}:{policy_digest}")
}

fn document_prefix(tenant: &str, policy_digest: &str) -> String {
    format!("encoded_document:{tenant}:{policy_digest}:")
}

fn props_of(core: &GraphCore, node: &str) -> Option<serde_json::Map<String, Value>> {
    let blob = core.get_node_properties(node)?;
    medulla_types::msgpack::decode_property_object(&blob).ok()
}

/// The vector generation of one tenant's index under one policy digest.
fn load_generation(core: &GraphCore, tenant: &str, policy_digest: &str) -> Option<u64> {
    props_of(core, &head_node(tenant, policy_digest))?
        .get("generation")?
        .as_u64()
}

fn write_node(core: &GraphCore, node: String, props: &Value) -> Result<(), Refused> {
    core.add_node(node, msgpack_bytes(props).map_err(internal)?);
    Ok(())
}

fn position_label(mode: DocumentPositionMode) -> &'static str {
    match mode {
        DocumentPositionMode::LocalReset => "local_reset",
        DocumentPositionMode::AbsoluteOffset => "absolute_offset",
    }
}

fn token_bytes(tokens: &[u32]) -> Vec<u8> {
    tokens.iter().flat_map(|t| t.to_le_bytes()).collect()
}

fn vector_bytes(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|c| c.to_le_bytes()).collect()
}

/// The space of the default registered encoder.
fn default_space() -> Result<EmbeddingSpaceRef, Refused> {
    EmbeddingSpaceRef::pinned(
        BOUND_ENCODER_SPACE,
        BOUND_ENCODER_SPACE_REVISION,
        "builtin-test-hashed-token-weights",
        "builtin-test-hashed-token-preprocessing",
        BOUND_ENCODER_DIM,
        false,
    )
    .map_err(|error| refuse(ServerErrorCode::EncodePolicyStale, error))
}

/// The policy a request without one runs under: the registered window knobs, the
/// bound model, `local_reset` positions and `mean_chunks`.
fn default_policy() -> Result<DocumentEncodingPolicy, Refused> {
    let knobs = medulla_server_api::knob_config::knobs();
    let size = |value: u64| usize::try_from(value).unwrap_or(usize::MAX);
    DocumentEncodingPolicy::new(
        TOKEN_ID_TOKENIZER,
        size(knobs.retrieval_encode_window_tokens),
        size(knobs.retrieval_encode_window_overlap),
        DocumentPositionMode::LocalReset,
        DocumentAggregation::MeanChunks,
        BOUND_ENCODER_REVISION,
        default_space()?,
    )
    .map_err(|error| refuse(ServerErrorCode::EncodeOverlapInvalid, error))
}

/// The SHA-256 of the canonical policy record.
fn policy_digest(policy: &DocumentEncodingPolicy) -> Result<String, Refused> {
    Ok(sha256_hex(&serde_json::to_vec(policy).map_err(internal)?))
}

/// Refuse a policy this server cannot run before any work is done, and resolve its
/// encoder by the policy's space: a multimodal space names its text binding's encoder,
/// which must be registered for exactly the space's `(id, dimensions, revision)` tuple;
/// a single-modality space takes the entry registered for that tuple. A space with no
/// registered encoder is a model that is not bound.
fn check_policy(policy: &DocumentEncodingPolicy) -> Result<&'static dyn TextEmbedder, Refused> {
    if policy.policy_revision != DOCUMENT_ENCODING_POLICY_REVISION {
        return Err(refuse(
            ServerErrorCode::EncodePolicyStale,
            format!(
                "policy revision {} is not served (served: {DOCUMENT_ENCODING_POLICY_REVISION})",
                policy.policy_revision
            ),
        ));
    }
    let space = &policy.space;
    let registry = medulla_plan::embed_registry::modality_encoders();
    let resolved = match space
        .modalities
        .iter()
        .find(|binding| binding.modality == Modality::Text)
    {
        Some(binding) => registry.lookup(
            &binding.encoder,
            &space.id,
            space.dimensions,
            &space.revision,
        ),
        None => registry.lookup_by_space(&space.id, space.dimensions, &space.revision),
    };
    resolved.map_err(|error| {
        refuse(
            ServerErrorCode::EncodePolicyStale,
            format!(
                "space `{}` ({} dimensions, revision `{}`) has no registered encoder ({error})",
                space.id, space.dimensions, space.revision
            ),
        )
    })
}

fn encode(
    ctx: &QueryHandlerCtx<'_>,
    document_id: String,
    token_ids: Vec<u32>,
    text: Option<String>,
    policy: Option<DocumentEncodingPolicy>,
) -> Result<EncodedDocument, Refused> {
    let started = Instant::now();
    if document_id.is_empty() || document_id.contains(':') {
        return Err(refuse(
            ServerErrorCode::NoEmbedding,
            "document_id must be non-empty and hold no `:`",
        ));
    }
    let policy = match policy {
        Some(policy) => policy,
        None => default_policy()?,
    };
    let encoder = check_policy(&policy)?;
    if text.is_some() && !token_ids.is_empty() {
        return Err(refuse(
            ServerErrorCode::NoEmbedding,
            "a document is `token_ids` or `text`, not both",
        ));
    }
    // A text document is its space-separated words; a window's ids index into them.
    let words: Option<Vec<&str>> = text
        .as_deref()
        .map(|text| text.split_ascii_whitespace().collect());
    let token_ids = match &words {
        Some(words) => (0..u32::try_from(words.len()).unwrap_or(u32::MAX)).collect(),
        None => token_ids,
    };
    let mode = match policy.position_mode {
        DocumentPositionMode::LocalReset => PositionMode::LocalReset,
        DocumentPositionMode::AbsoluteOffset => PositionMode::Absolute,
    };
    let windows = split_windows(token_ids.len(), policy.chunk_size, policy.overlap, mode)
        .map_err(chunk_refusal)?;
    policy
        .validate()
        .map_err(|error| refuse(ServerErrorCode::EncodePolicyStale, error))?;
    let budget =
        usize::try_from(medulla_server_api::knob_config::knobs().retrieval_encode_budget_chunks)
            .unwrap_or(usize::MAX);
    if windows.len() > budget {
        return Err(refuse(
            ServerErrorCode::EncodeBudgetExhausted,
            format!(
                "{} chunks exceed the per-document budget of {budget}",
                windows.len()
            ),
        ));
    }
    let digest = policy_digest(&policy)?;
    let tenant = reader_of(ctx).tenant;
    let prior_generation = load_generation(ctx.core, &tenant, &digest);
    let chunk_encoder = RegistryChunkEncoder {
        encoder,
        words: words.as_deref(),
    };
    let encoding = encode_chunks(&chunk_encoder, &policy.model_revision, &token_ids, &windows);
    // A failed chunk blocks the aggregate, and nothing is written before it.
    let vector = aggregate_mean_chunks(&encoding.vectors).map_err(chunk_refusal)?;
    let generation = prior_generation.unwrap_or(0) + 1;
    let receipt = EncodingReceipt {
        document_digest: match &text {
            Some(text) => sha256_hex(text.as_bytes()),
            None => sha256_hex(&token_bytes(&token_ids)),
        },
        token_ranges: windows
            .iter()
            .map(|w| [w.start as u64, w.end as u64])
            .collect(),
        position_mode: position_label(policy.position_mode).to_string(),
        chunk_model_revisions: windows
            .iter()
            .map(|_| policy.model_revision.clone())
            .collect(),
        aggregate_digest: sha256_hex(&vector_bytes(&vector)),
        policy_digest: digest.clone(),
        index_generation: generation,
        measured_cost_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    };
    receipt.validate().map_err(internal)?;
    let node = json!({
        "type": DOCUMENT_LABEL,
        "document_id": document_id,
        "author_tenant": tenant,
        "vector": vector,
        "receipt": serde_json::to_value(&receipt).map_err(internal)?,
    });
    write_node(
        ctx.core,
        format!("{}{document_id}", document_prefix(&tenant, &digest)),
        &node,
    )?;
    let head = json!({
        "type": HEAD_LABEL,
        "author_tenant": tenant,
        "policy_digest": digest,
        "generation": generation,
    });
    write_node(ctx.core, head_node(&tenant, &digest), &head)?;
    Ok(EncodedDocument {
        document_id,
        receipt,
        vector,
    })
}

fn reply<M: MethodResult>(req_id: u64, out: Result<M::Body, Refused>) -> Response {
    match out
        .map_err(|refusal| refusal.0)
        .and_then(ResultPayload::of::<M>)
    {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => Response::err(req_id, error),
    }
}

/// `EncodeDocument`: encode one document and publish its vector generation.
pub(crate) fn handle_encode_document(
    ctx: &QueryHandlerCtx<'_>,
    document_id: String,
    token_ids: Vec<u32>,
    text: Option<String>,
    policy: Option<DocumentEncodingPolicy>,
) -> Response {
    reply::<results::EncodeDocument>(
        ctx.req_id,
        encode(ctx, document_id, token_ids, text, policy),
    )
}

fn cosine(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    (na.is_normal() && nb.is_normal()).then(|| dot / (na * nb))
}

fn search(
    ctx: &QueryHandlerCtx<'_>,
    request: EncodedSearchRequest,
) -> Result<EncodedSearchResult, Refused> {
    if request.query_embedding.iter().any(|c| !c.is_finite()) {
        return Err(refuse(
            ServerErrorCode::EncodeNonFinite,
            "the query vector holds a non-finite component",
        ));
    }
    let tenant = reader_of(ctx).tenant;
    // A policy digest with no generation under it is stale: it is refused before ranking.
    let Some(generation) = load_generation(ctx.core, &tenant, &request.policy_digest) else {
        return Err(refuse(
            ServerErrorCode::EncodePolicyStale,
            "no vector generation exists under the query policy digest",
        ));
    };
    let prefix = document_prefix(&tenant, &request.policy_digest);
    let mut scored: Vec<(String, f32, EncodingReceipt)> = Vec::new();
    for id in ctx.core.node_ids() {
        let Some(document_id) = id.strip_prefix(&prefix) else {
            continue;
        };
        let Some(props) = props_of(ctx.core, &id) else {
            continue;
        };
        let Some(vector) = props.get("vector").and_then(Value::as_array).map(|items| {
            items
                .iter()
                .filter_map(|c| c.as_f64().map(|c| c as f32))
                .collect::<Vec<f32>>()
        }) else {
            continue;
        };
        let Some(receipt) = props
            .get("receipt")
            .and_then(|r| serde_json::from_value::<EncodingReceipt>(r.clone()).ok())
        else {
            continue;
        };
        if let Some(score) = cosine(&request.query_embedding, &vector) {
            scored.push((document_id.to_string(), score, receipt));
        }
    }
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    scored.truncate(usize::try_from(request.k).unwrap_or(usize::MAX));
    Ok(EncodedSearchResult {
        hits: scored
            .into_iter()
            .map(|(document_id, score, receipt)| EncodedHit {
                document_id,
                score,
                receipt,
            })
            .collect(),
        policy_digest: request.policy_digest,
        index_generation: generation,
    })
}

/// `EncodedDocumentSearch`: rank the caller's encoded documents against one vector.
pub(crate) fn handle_encoded_search(
    ctx: &QueryHandlerCtx<'_>,
    request: EncodedSearchRequest,
) -> Response {
    reply::<results::EncodedDocumentSearch>(ctx.req_id, search(ctx, request))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bound_encoder_is_position_aware_and_stateless() {
        let window = [1u32, 2, 3, 4, 5];
        let first = BoundEncoder.encode_at(&window, 0);
        assert_eq!(first, BoundEncoder.encode_at(&window, 0));
        assert_ne!(first, BoundEncoder.encode_at(&window, 1));
        assert_eq!(first.len(), BOUND_ENCODER_DIM);
        assert!(BoundEncoder.encode_at(&[POISON_NON_FINITE_TOKEN], 0)[0].is_nan());
        assert_eq!(
            BoundEncoder.encode_at(&[POISON_SHORT_TOKEN], 0).len(),
            BOUND_ENCODER_DIM / 2
        );
    }

    #[test]
    fn unsupported_policy_is_stale() {
        register_default_chunk_encoder().unwrap();
        let mut policy = default_policy().unwrap();
        check_policy(&policy).unwrap();
        policy.space.id = "other".into();
        assert!(check_policy(&policy)
            .err()
            .unwrap()
            .0
            .starts_with("ENCODE_POLICY_STALE"));
    }
}
