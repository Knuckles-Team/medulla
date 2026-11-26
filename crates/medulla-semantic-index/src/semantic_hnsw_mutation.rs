//! Validated embedding writes and derived-index maintenance for HNSW storage.

use super::*;

impl SemanticStore {
    /// Whether the resident index holds no tombstoned (superseded) slot: every row a walk
    /// reads is a live one (MEDULLA-FEDERATED-QUERY-R040.5.6). A rebuild or compaction
    /// clears the tombstones.
    pub fn ann_compacted(&self) -> bool {
        self.index.read().tombstones.is_empty()
    }

    /// Raw stored embedding for `node_id`, if present. The MMR reranker uses
    /// this owned value to compute pairwise diversity.
    pub fn get_embedding(&self, node_id: &str) -> Option<Vec<f32>> {
        self.embeddings.get(node_id).cloned()
    }

    /// Deterministic owned image used by the MutationBatch row-delta compiler.
    /// HNSW internals are derived and intentionally excluded.
    pub fn embeddings_snapshot(&self) -> Vec<(String, Vec<f32>)> {
        let mut rows: Vec<_> = self
            .embeddings
            .iter()
            .map(|(id, embedding)| (id.clone(), embedding.clone()))
            .collect();
        rows.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        rows
    }

    /// Validate an embedding against this store's declared space and backend
    /// width before any resident state is touched. Callers that already hold a
    /// read guard can use this admission check without cloning the corpus.
    pub fn validate_embedding(&self, embedding: &[f32]) -> Result<(), EmbeddingDimensionError> {
        let expected_dim = self
            .space
            .as_ref()
            .map(|space| space.dimensions)
            .unwrap_or_else(|| self.dim());
        check_embedding_dimension_bounded(embedding, expected_dim, MAX_GENERIC_DIMENSION)
            .map(|_| ())
    }

    /// Reject malformed data before touching either resident rows or HNSW.
    pub fn add_embedding(
        &mut self,
        node_id: String,
        embedding: Vec<f32>,
    ) -> Result<(), EmbeddingDimensionError> {
        self.validate_embedding(&embedding)?;

        let is_update = self.embeddings.contains_key(&node_id);
        self.embeddings.insert(node_id.clone(), embedding.clone());
        self.generation.bump();
        let live_len = self.embeddings.len();

        let mut idx = self.index.write();
        idx.compact = None;
        if idx.hnsw.is_none() {
            return Ok(());
        }
        // The guard above guarantees that the resident HNSW dimension is
        // unchanged; a decline here would indicate an internal invariant break.
        let internal = idx.order.len();
        idx.hnsw
            .as_mut()
            .expect("resident HNSW checked above")
            .insert(internal as u64, embedding);
        idx.order.push(node_id.clone());
        if is_update {
            if let Some(&old) = idx.id_to_internal.get(&node_id) {
                idx.tombstones.insert(old);
            }
        }
        idx.id_to_internal.insert(node_id, internal);
        idx.built_len = live_len;

        if idx.order.len() >= BRUTE_FORCE_THRESHOLD
            && idx.tombstones.len() * 100 >= idx.order.len() * COMPACT_TOMBSTONE_PCT
        {
            idx.hnsw = None;
        }
        Ok(())
    }

    /// Incrementally remove `node_id`'s embedding and tombstone its HNSW row.
    pub fn remove_embedding(&mut self, node_id: &str) -> bool {
        if self.embeddings.remove(node_id).is_none() {
            return false;
        }
        self.generation.bump();
        let live_len = self.embeddings.len();
        let mut idx = self.index.write();
        idx.compact = None;
        if idx.hnsw.is_some() {
            if let Some(&internal) = idx.id_to_internal.get(node_id) {
                idx.tombstones.insert(internal);
            }
            idx.id_to_internal.remove(node_id);
            idx.built_len = live_len;
            if idx.order.len() >= BRUTE_FORCE_THRESHOLD
                && idx.tombstones.len() * 100 >= idx.order.len() * COMPACT_TOMBSTONE_PCT
            {
                idx.hnsw = None;
            }
        }
        true
    }

    /// Force a clean rebuild that drops all tombstones.
    pub fn force_compact(&self) {
        let mut idx = self.index.write();
        if idx.hnsw.is_some() {
            self.rebuild(&mut idx);
        }
    }

    /// Record who built this index and from what. The identity is persisted with the
    /// store, and the compaction worker reads it from here, so no caller passes it in
    /// (MEDULLA-FEDERATED-QUERY-R040.5.6). A store already carrying a different identity
    /// refuses: an index belongs to one tenant, snapshot and source.
    pub fn declare_identity(&mut self, identity: IndexIdentity) -> Result<(), String> {
        match self.identity.as_ref() {
            Some(current) if current != &identity => {
                Err("the index already carries a different identity".to_string())
            }
            _ => {
                self.identity = Some(identity);
                Ok(())
            }
        }
    }

    /// The identity this index owns, if its build declared one.
    pub fn identity(&self) -> Option<&IndexIdentity> {
        self.identity.as_ref()
    }

    /// The compaction worker's low-storage step (MEDULLA-FEDERATED-QUERY-R040.5.6): drop
    /// every tombstone, then build the compact artifact with `HnswIndex::to_compact`.
    /// The manifest comes from the identity the index persisted when it was built and
    /// from its declared embedding space; the artifact measures its own graph and code
    /// bytes. An index with no identity, no declared space or no vectors refuses with
    /// `ANN_MANIFEST_INVALID`. The artifact stays resident until the next write.
    pub fn compact_low_storage(&self) -> Result<LowStorageArtifact, LowStorageError> {
        let refuse = |detail: &str| LowStorageError {
            code: medulla_ann::low_storage::LowStorageErrorCode::ManifestInvalid,
            detail: detail.to_string(),
        };
        let identity = self
            .identity
            .as_ref()
            .ok_or_else(|| refuse("the index carries no identity"))?;
        let space = self
            .space
            .as_ref()
            .ok_or_else(|| refuse("the index has no declared embedding space"))?;
        let mut idx = self.index.write();
        if idx.hnsw.is_none() || !idx.tombstones.is_empty() {
            self.rebuild(&mut idx);
        }
        let hnsw = idx
            .hnsw
            .as_ref()
            .ok_or_else(|| refuse("the index holds no vectors to compact"))?;
        let metadata_bytes: u64 = idx.order.iter().map(|id| id.len() as u64).sum();
        let request = ManifestRequest {
            tenant: identity.tenant.clone(),
            snapshot: identity.snapshot.clone(),
            source_id: identity.source_id.clone(),
            revision: identity.revision,
            text_digest: identity.text_digest,
            embedding_space: space.digest.clone(),
            encoding_policy: identity.encoding_policy.clone(),
            generation: identity.generation,
            build_policy: identity.build_policy.clone(),
            segments: vec![SegmentBinding {
                embedding_space: space.digest.clone(),
                encoding_policy: identity.encoding_policy.clone(),
            }],
            accounting: StorageAccounting {
                metadata_bytes,
                source_reference_bytes: identity.text_digest.len() as u64,
                ..StorageAccounting::default()
            },
            ceiling_bytes: identity.ceiling_bytes,
            pruning: None,
        };
        let artifact = hnsw.to_compact(request, &[])?;
        idx.compact = Some(artifact.clone());
        Ok(artifact)
    }

    /// The compact artifact the compaction worker last built, if no write has followed.
    pub fn low_storage_artifact(&self) -> Option<LowStorageArtifact> {
        self.index.read().compact.clone()
    }
}
