//! Registration of the engine's query encoders into `ModalityEncoderRegistry` at server
//! bootstrap (MEDULLA-FEDERATED-QUERY-R039.1.3).
//!
//! The registry is process-wide and read by the served `UnifiedQuery` bind step, so the
//! bound text embedder must be in it before any request is served; a request then refuses
//! `QUERY_ENCODER_UNAVAILABLE` only when nothing is bound. Retrieval stays inside the
//! engine: no entry calls an external service on its own account.

use medulla_exec::exec::TextEmbedder;
use medulla_server_ops::api::query_txn::query_support::{
    uql_text_embedder, uql_text_encoder_identity,
};

/// The process-wide embedder as an owned encoder. The registry stores a leaked box, and the
/// bound embedder already lives for the process, so this only forwards.
struct BoundEncoder(&'static dyn TextEmbedder);

impl TextEmbedder for BoundEncoder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        self.0.embed(text)
    }
}

/// Register the bound text embedder under its identity. Idempotent: the registry is
/// process-wide and a server state may be composed more than once in one process. With no
/// embedder bound nothing is registered. A registration the registry refuses is a refusal
/// to boot (the same id pinned twice to different encoders is a configuration error).
pub fn register_query_encoders() -> Result<(), String> {
    static REGISTERED: std::sync::Once = std::sync::Once::new();
    let mut outcome = Ok(());
    REGISTERED.call_once(|| {
        let (Some(embedder), Some(identity)) = (uql_text_embedder(), uql_text_encoder_identity())
        else {
            return;
        };
        outcome = medulla_plan::embed_registry::modality_encoders().register(
            &identity.encoder_id,
            &identity.space_id,
            identity.dimensions,
            &identity.revision,
            Box::new(BoundEncoder(embedder)),
        );
    });
    outcome
}
