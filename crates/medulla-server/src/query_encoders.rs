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
        // The built-in chunk encoder is a registered default entry, so served
        // `EncodeDocument` resolves every encoder through the registry.
        if let Err(error) = medulla_h_query::query::document_encoding::register_default_chunk_encoder() {
            outcome = Err(error);
            return;
        }
        // Each non-text modality bound in the same model-binding config registers beside
        // the text embedder; an unbound modality is simply not registered.
        #[cfg(feature = "modality-serving")]
        for (identity, encoder) in
            medulla_server_ops::api::query_txn::query_support::bound_modality_encoders()
        {
            if let Err(error) = medulla_plan::embed_registry::modality_encoders().register(
                &identity.encoder_id,
                &identity.space_id,
                identity.dimensions,
                &identity.revision,
                encoder,
            ) {
                outcome = Err(error);
                return;
            }
        }
        // MEDULLA-FEDERATED-QUERY-R039.1.6: pinned encoder files an operator names, read once
        // before any request is served. A load error refuses to boot.
        if let Some(dir) = std::env::var_os("MEDULLA_EMBEDDING_ENCODER_DIR").filter(|d| !d.is_empty())
        {
            if let Err(error) =
                medulla_plan::embed_registry::modality_encoders().load_dir(std::path::Path::new(&dir))
            {
                outcome = Err(error);
                return;
            }
        }
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
