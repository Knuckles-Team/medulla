//! Embedding versioning — tag stored vectors with the model/version that produced
//! them (CONCEPT:MEDULLA-KG.sharding.semantic-embedding-store-backed depth: per-modality vector versioning).
//!
//! `medulla-ann` stores compressed PQ/SQ8 codes keyed by an external `u64` id (see
//! [`crate::ivfpq::IvfPq`]); it does NOT track which embedding model (or which
//! version of that model) produced each stored vector. That provenance matters the
//! moment a KG re-embeds a corpus with a new model: mixed-version rows in the same
//! index silently degrade recall (old and new vectors are not comparable), and
//! nothing today can even tell you it happened.
//!
//! [`EmbeddingVersion`] is the minimal tag (`model_id` + a monotonic `model_version`
//! integer); [`EmbeddingVersionStore`] is an in-memory side-table
//! `id -> EmbeddingVersion`, independent of the code buffers.
//!
//! RF-ADR-002: its `save`/`open` pair wrote a `versions.bin` file "beside the
//! index's own `meta.bin`" -- a directory format this crate no longer produces
//! and that had no reader anywhere in `crates/**` or `src/**`. Deleted with it.
//! A durable tag table belongs in the semantic owner file alongside the codes,
//! written by the holder of a mutation capability, not by this leaf crate.

use serde::de::{self, SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::fmt;

const MAX_VERSION_TAGS: usize = 5_000_000;
const MAX_MODEL_ID_BYTES: usize = 1_024;

/// The model identity + version that produced one stored embedding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingVersion {
    /// The embedding model's identifier (e.g. `"text-embedding-3-large"`).
    pub model_id: String,
    /// A monotonic version counter for that model (bump on any re-embed that
    /// changes the vector space — a fine-tune, a dimension change, a provider
    /// upgrade). Caller-assigned; this crate only compares/stores it.
    pub model_version: u64,
}

impl EmbeddingVersion {
    pub fn new(model_id: impl Into<String>, model_version: u64) -> Self {
        Self {
            model_id: model_id.into(),
            model_version,
        }
    }
}

/// A passing promotion decision for one model and version
/// (MEDULLA-UNIFIED-DATA-PLANE-R040.3). [`EmbeddingVersionStore::tag_batch`]
/// tags nothing without one; the gate that judged the candidate supplies it.
pub trait PromotionApproval {
    fn promoted(&self) -> bool;
    fn model_id(&self) -> &str;
    fn model_version(&self) -> u64;
}

/// Why [`EmbeddingVersionStore::tag_batch`] tagged nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TagRefused {
    /// The verdict did not promote the candidate.
    NotPromoted,
    /// The verdict judged another model or version than the tag names.
    VersionMismatch,
}

impl fmt::Display for TagRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPromoted => f.write_str("embedding version tag requires a passing verdict"),
            Self::VersionMismatch => {
                f.write_str("embedding version tag names another model or version than its verdict")
            }
        }
    }
}

impl std::error::Error for TagRefused {}

/// A side-table mapping stored embedding id → the [`EmbeddingVersion`] that
/// produced it. Independent of the PQ/SQ8 code buffers — tagging (or re-tagging)
/// never touches the vector data itself.
#[derive(Clone, Debug, Default)]
pub struct EmbeddingVersionStore {
    tags: HashMap<u64, EmbeddingVersion>,
}

impl Serialize for EmbeddingVersionStore {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut entries: Vec<_> = self.tags.iter().collect();
        entries.sort_unstable_by_key(|(id, _)| **id);
        let mut sequence = serializer.serialize_seq(Some(entries.len()))?;
        for (&id, version) in entries {
            sequence.serialize_element(&(id, version))?;
        }
        sequence.end()
    }
}

impl<'de> Deserialize<'de> for EmbeddingVersionStore {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct StoreVisitor;

        impl<'de> Visitor<'de> for StoreVisitor {
            type Value = EmbeddingVersionStore;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a bounded sequence of embedding-version tags")
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut tags = HashMap::new();
                while let Some((id, version)) =
                    sequence.next_element::<(u64, EmbeddingVersion)>()?
                {
                    if tags.len() >= MAX_VERSION_TAGS {
                        return Err(de::Error::custom("embedding-version tag limit exceeded"));
                    }
                    validate_version(&version).map_err(de::Error::custom)?;
                    if tags.insert(id, version).is_some() {
                        return Err(de::Error::custom("duplicate embedding-version tag"));
                    }
                }
                Ok(EmbeddingVersionStore { tags })
            }
        }

        deserializer.deserialize_seq(StoreVisitor)
    }
}

fn validate_version(version: &EmbeddingVersion) -> Result<(), &'static str> {
    if version.model_id.is_empty() || version.model_id.len() > MAX_MODEL_ID_BYTES {
        return Err("embedding model identifier is empty or too long");
    }
    Ok(())
}

impl EmbeddingVersionStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild the side-table from its durable rows (the semantic owner's
    /// `embedding_version_tags`). Loading is not tagging: no verdict applies.
    pub fn restore(tags: impl IntoIterator<Item = (u64, EmbeddingVersion)>) -> Self {
        Self {
            tags: tags.into_iter().collect(),
        }
    }

    /// Tag `id` with `version` (overwrites any prior tag for that id).
    pub fn tag(&mut self, id: u64, version: EmbeddingVersion) {
        self.tags.insert(id, version);
    }

    /// Tag a whole batch with the SAME version — the common re-embed-a-corpus
    /// case. Only a passing verdict for exactly that model and version tags:
    /// otherwise nothing is tagged.
    pub fn tag_batch(
        &mut self,
        ids: &[u64],
        version: &EmbeddingVersion,
        verdict: &dyn PromotionApproval,
    ) -> Result<(), TagRefused> {
        if !verdict.promoted() {
            return Err(TagRefused::NotPromoted);
        }
        if (verdict.model_id(), verdict.model_version())
            != (version.model_id.as_str(), version.model_version)
        {
            return Err(TagRefused::VersionMismatch);
        }
        for &id in ids {
            self.tags.insert(id, version.clone());
        }
        Ok(())
    }

    pub fn version_of(&self, id: u64) -> Option<&EmbeddingVersion> {
        self.tags.get(&id)
    }

    pub fn remove(&mut self, id: u64) -> Option<EmbeddingVersion> {
        self.tags.remove(&id)
    }

    pub fn len(&self) -> usize {
        self.tags.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }

    /// Every stored id currently tagged with exactly `model_id`/`model_version`.
    pub fn ids_for_version(&self, model_id: &str, model_version: u64) -> Vec<u64> {
        self.tags
            .iter()
            .filter(|(_, v)| v.model_id == model_id && v.model_version == model_version)
            .map(|(&id, _)| id)
            .collect()
    }

    /// The distinct `(model_id, model_version)` pairs present — a quick "is this
    /// index mixed-version?" check (`distinct_versions().len() > 1`).
    pub fn distinct_versions(&self) -> Vec<(String, u64)> {
        let mut out: Vec<(String, u64)> = self
            .tags
            .values()
            .map(|v| (v.model_id.clone(), v.model_version))
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        out.sort();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Verdict {
        promoted: bool,
        model_id: &'static str,
        model_version: u64,
    }

    impl PromotionApproval for Verdict {
        fn promoted(&self) -> bool {
            self.promoted
        }
        fn model_id(&self) -> &str {
            self.model_id
        }
        fn model_version(&self) -> u64 {
            self.model_version
        }
    }

    fn pass(model_id: &'static str, model_version: u64) -> Verdict {
        Verdict {
            promoted: true,
            model_id,
            model_version,
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R040.3
    #[test]
    fn tag_requires_passing_verdict() {
        let mut store = EmbeddingVersionStore::new();
        let v2 = EmbeddingVersion::new("m1", 2);
        let failed = Verdict {
            promoted: false,
            ..pass("m1", 2)
        };
        assert_eq!(
            store.tag_batch(&[1, 2], &v2, &failed),
            Err(TagRefused::NotPromoted)
        );
        assert_eq!(
            store.tag_batch(&[1, 2], &v2, &pass("m1", 3)),
            Err(TagRefused::VersionMismatch)
        );
        assert!(store.is_empty());
        store.tag_batch(&[1, 2], &v2, &pass("m1", 2)).unwrap();
        assert_eq!(store.ids_for_version("m1", 2).len(), 2);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R040.4
    #[test]
    fn blocked_promotion_tags_nothing() {
        let mut store = EmbeddingVersionStore::new();
        let v1 = EmbeddingVersion::new("m1", 1);
        store.tag_batch(&[1, 2, 3], &v1, &pass("m1", 1)).unwrap();
        let candidate = EmbeddingVersion::new("m1", 2);
        let blocked = Verdict {
            promoted: false,
            ..pass("m1", 2)
        };
        assert!(store.tag_batch(&[1, 2, 3], &candidate, &blocked).is_err());
        assert!(store.ids_for_version("m1", 2).is_empty());
        assert_eq!(store.distinct_versions(), vec![("m1".to_string(), 1)]);
        assert!([1u64, 2, 3]
            .iter()
            .all(|&id| store.version_of(id) == Some(&v1)));
    }

    #[test]
    fn tag_and_lookup_roundtrip() {
        let mut store = EmbeddingVersionStore::new();
        let v1 = EmbeddingVersion::new("text-embedding-3-large", 1);
        store.tag(10, v1.clone());
        assert_eq!(store.version_of(10), Some(&v1));
        assert_eq!(store.version_of(999), None);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn tag_batch_applies_same_version_to_all_ids() {
        let mut store = EmbeddingVersionStore::new();
        let v = EmbeddingVersion::new("m1", 2);
        store.tag_batch(&[1, 2, 3], &v, &pass("m1", 2)).unwrap();
        assert_eq!(store.len(), 3);
        assert!([1u64, 2, 3]
            .iter()
            .all(|&id| store.version_of(id) == Some(&v)));
    }

    #[test]
    fn ids_for_version_filters_correctly_on_reembed() {
        let mut store = EmbeddingVersionStore::new();
        store
            .tag_batch(&[1, 2], &EmbeddingVersion::new("m1", 1), &pass("m1", 1))
            .unwrap();
        store
            .tag_batch(&[3, 4], &EmbeddingVersion::new("m1", 2), &pass("m1", 2))
            .unwrap(); // re-embedded
        let old = store.ids_for_version("m1", 1);
        let new = store.ids_for_version("m1", 2);
        assert_eq!(old.len(), 2);
        assert_eq!(new.len(), 2);
        assert!(old.iter().all(|id| [1u64, 2].contains(id)));
        assert!(new.iter().all(|id| [3u64, 4].contains(id)));
    }

    #[test]
    fn distinct_versions_detects_mixed_version_index() {
        let mut store = EmbeddingVersionStore::new();
        assert!(store.distinct_versions().is_empty());
        store
            .tag_batch(&[1], &EmbeddingVersion::new("m1", 1), &pass("m1", 1))
            .unwrap();
        assert_eq!(store.distinct_versions(), vec![("m1".to_string(), 1)]);
        store
            .tag_batch(&[2], &EmbeddingVersion::new("m1", 2), &pass("m1", 2))
            .unwrap();
        assert_eq!(
            store.distinct_versions(),
            vec![("m1".to_string(), 1), ("m1".to_string(), 2)],
            "a re-embedded-but-not-yet-migrated index is detectably mixed-version"
        );
    }
}
