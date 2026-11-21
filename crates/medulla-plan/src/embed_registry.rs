//! The engine-local modality encoder registry (MEDULLA-FEDERATED-QUERY-R039.1.3).
//!
//! Retrieval stays inside the engine: the encoder that turns a query in one modality into a
//! vector of a multimodal space is an entry of this registry, never an external service. An
//! entry is registered under the encoder id a space's `ModalityBinding` names, and records the
//! `(space id, dimensions, model revision)` tuple of the space it encodes for; a lookup whose
//! space tuple differs from the entry's finds nothing. Encoders implement the existing
//! [`TextEmbedder`] shape (the query reaches an encoder as text: the text itself, or the
//! reference an image, audio or video query carries).

use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};

use medulla_exec::exec::TextEmbedder;
use medulla_types::contract::ServerErrorCode;

struct Entry {
    space_id: String,
    dimensions: usize,
    revision: String,
    encoder: &'static dyn TextEmbedder,
}

/// The process registry of modality encoders, by registry id.
#[derive(Default)]
pub struct ModalityEncoderRegistry {
    entries: RwLock<BTreeMap<String, Entry>>,
}

impl ModalityEncoderRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `encoder` under `encoder_id` for the space `(space_id, dimensions, revision)`.
    /// The encoder lives for the rest of the process (it is leaked, like the process-wide
    /// text embedder). A repeated id refuses.
    pub fn register(
        &self,
        encoder_id: &str,
        space_id: &str,
        dimensions: usize,
        revision: &str,
        encoder: Box<dyn TextEmbedder>,
    ) -> Result<(), String> {
        let mut entries = self
            .entries
            .write()
            .map_err(|_| "modality encoder registry is poisoned".to_string())?;
        if entries.contains_key(encoder_id) {
            return Err(format!(
                "modality encoder `{encoder_id}` is already registered"
            ));
        }
        entries.insert(
            encoder_id.to_string(),
            Entry {
                space_id: space_id.to_string(),
                dimensions,
                revision: revision.to_string(),
                encoder: Box::leak(encoder),
            },
        );
        Ok(())
    }

    /// The encoder registered as `encoder_id` for the space `(space_id, dimensions,
    /// revision)`, or `QUERY_ENCODER_UNAVAILABLE` when no such entry exists for that exact
    /// space tuple.
    pub fn lookup(
        &self,
        encoder_id: &str,
        space_id: &str,
        dimensions: usize,
        revision: &str,
    ) -> Result<&'static dyn TextEmbedder, String> {
        let unavailable = |why: &str| {
            format!(
                "{}: modality encoder `{encoder_id}` for space `{space_id}` ({dimensions} dimensions, revision `{revision}`) {why}",
                ServerErrorCode::QueryEncoderUnavailable
            )
        };
        let entries = self
            .entries
            .read()
            .map_err(|_| unavailable("is unreadable"))?;
        let entry = entries
            .get(encoder_id)
            .ok_or_else(|| unavailable("is not registered"))?;
        if entry.space_id != space_id || entry.dimensions != dimensions || entry.revision != revision
        {
            return Err(unavailable("is registered for a different space tuple"));
        }
        Ok(entry.encoder)
    }
}

/// The one process-wide registry the served query path binds from.
pub fn modality_encoders() -> &'static ModalityEncoderRegistry {
    static REGISTRY: OnceLock<ModalityEncoderRegistry> = OnceLock::new();
    REGISTRY.get_or_init(ModalityEncoderRegistry::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_exec::exec::HashEmbedder;

    // spec: MEDULLA-FEDERATED-QUERY-R039.1.3
    #[test]
    fn lookup_finds_only_the_exact_space_tuple() {
        let registry = ModalityEncoderRegistry::new();
        registry
            .register("enc-text", "space-a", 8, "r1", Box::new(HashEmbedder::new(8)))
            .unwrap();
        assert!(registry
            .register("enc-text", "space-a", 8, "r1", Box::new(HashEmbedder::new(8)))
            .is_err());
        let encoder = registry.lookup("enc-text", "space-a", 8, "r1").unwrap();
        assert_eq!(encoder.embed("x").unwrap().len(), 8);
        for (id, space, dim, revision) in [
            ("enc-image", "space-a", 8, "r1"),
            ("enc-text", "space-b", 8, "r1"),
            ("enc-text", "space-a", 16, "r1"),
            ("enc-text", "space-a", 8, "r2"),
        ] {
            let error = registry.lookup(id, space, dim, revision).err().unwrap();
            assert!(error.starts_with("QUERY_ENCODER_UNAVAILABLE"), "{error}");
        }
    }
}
