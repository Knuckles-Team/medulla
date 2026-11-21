//! Media memory planning and recall matching (`MEDULLA-MEDIA-VOCAB-R650.2`).
//!
//! Two pure functions. [`plan_media_write`] turns one blob reference and its derived
//! text (a transcript or a caption, with the embedding of that text) into a
//! [`MediaWritePlan`]: the blob is planned only when the tenant ledger does not hold it
//! yet, every derivation gets a content-addressed id, and the output order is fixed.
//! [`match_derivation`] ranks stored derivation embeddings against a query embedding
//! by cosine. Neither reads a store: the caller (`MemoryRemember` and `MemoryRecall` in
//! `medulla-h-graph`) supplies the ledger fact and the embeddings. The only lineage edge
//! is `prov:wasDerivedFrom`, drawn from the derivation node to the blob node.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The blob a media memory cites: its SHA-256 digest, MIME type and byte length.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaBlobRef {
    /// SHA-256 of the blob bytes: 64 lowercase hexadecimal characters.
    pub digest: String,
    /// The MIME type of the blob.
    pub mime: String,
    /// The byte length of the blob.
    pub byte_len: u64,
}

/// What a derivation is: the value of `:derivationKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DerivationKind {
    /// Text spoken in the blob.
    Transcript,
    /// A description of what the blob shows.
    Caption,
}

impl DerivationKind {
    /// The kind string, as stored in `:derivationKind`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transcript => "transcript",
            Self::Caption => "caption",
        }
    }
}

/// One derivation a caller offers for a blob.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DerivationInput {
    /// Transcript or caption.
    pub kind: DerivationKind,
    /// The derived text; never empty.
    pub text: String,
    /// The embedding of `text`.
    pub embedding: Vec<f32>,
}

/// One derivation node to write.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedDerivation {
    /// Lowercase hex SHA-256 of the blob digest, NUL, the kind string, NUL and the text
    /// bytes, so the same derivation of the same blob is the same node.
    pub id: String,
    /// Transcript or caption.
    pub kind: DerivationKind,
    /// The derived text.
    pub text: String,
    /// The embedding of `text`.
    pub embedding: Vec<f32>,
    /// The blob digest: the target of the `prov:wasDerivedFrom` edge.
    pub derived_from: String,
}

/// What one media write adds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaWritePlan {
    /// The blob to write, or `None` when the ledger already holds this digest.
    pub blob: Option<MediaBlobRef>,
    /// The derivation nodes, sorted by `id` ascending, one per distinct id.
    pub derivations: Vec<PlannedDerivation>,
}

/// Why a media write is refused before any effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaRefusal {
    /// The blob digest is not 64 lowercase hexadecimal characters.
    DigestMalformed,
    /// A derivation has empty text.
    DerivationTextEmpty,
}

fn digest_is_well_formed(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn derivation_id(blob_digest: &str, kind: DerivationKind, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(blob_digest.as_bytes());
    hasher.update([0u8]);
    hasher.update(kind.as_str().as_bytes());
    hasher.update([0u8]);
    hasher.update(text.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Plan one media write. `blob_exists` is the tenant blob ledger's answer for
/// `blob.digest`; the blob is planned only when it is false. Each input becomes one
/// derivation, two inputs with the same id collapse to one, and the output is sorted by
/// id. A malformed digest or an empty text refuses.
pub fn plan_media_write(
    blob: &MediaBlobRef,
    blob_exists: bool,
    derivations: &[DerivationInput],
) -> Result<MediaWritePlan, MediaRefusal> {
    if !digest_is_well_formed(&blob.digest) {
        return Err(MediaRefusal::DigestMalformed);
    }
    let mut planned: Vec<PlannedDerivation> = Vec::with_capacity(derivations.len());
    for input in derivations {
        if input.text.is_empty() {
            return Err(MediaRefusal::DerivationTextEmpty);
        }
        let id = derivation_id(&blob.digest, input.kind, &input.text);
        if planned.iter().any(|existing| existing.id == id) {
            continue;
        }
        planned.push(PlannedDerivation {
            id,
            kind: input.kind,
            text: input.text.clone(),
            embedding: input.embedding.clone(),
            derived_from: blob.digest.clone(),
        });
    }
    planned.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(MediaWritePlan {
        blob: (!blob_exists).then(|| blob.clone()),
        derivations: planned,
    })
}

/// Bytes of a blob the magic-type sniff reads.
pub const SNIFF_HEAD_BYTES: usize = 16;

/// One magic-type rule: the bytes that must sit at `offset`, and the type they name.
type SniffRule = (usize, &'static [u8], &'static str);

/// The sniff table of MEDULLA-MEDIA-VOCAB-R650.4. `RIFF` containers are told apart by the form
/// bytes at offset 8, and an `ftyp` box at offset 4 names `audio/mp4`.
pub const SNIFF_TABLE: &[SniffRule] = &[
    (0, b"\x89PNG\r\n\x1a\n", "image/png"),
    (0, b"\xff\xd8\xff", "image/jpeg"),
    (8, b"WEBP", "image/webp"),
    (0, b"GIF87a", "image/gif"),
    (0, b"GIF89a", "image/gif"),
    (0, b"ID3", "audio/mpeg"),
    (0, b"\xff\xfb", "audio/mpeg"),
    (0, b"\xff\xf3", "audio/mpeg"),
    (0, b"\xff\xf2", "audio/mpeg"),
    (0, b"OggS", "audio/ogg"),
    (8, b"WAVE", "audio/wav"),
    (4, b"ftyp", "audio/mp4"),
    (0, b"%PDF-", "application/pdf"),
];

/// The type the first bytes of a blob name, or `None` when no table rule matches. A `RIFF` rule
/// needs the `RIFF` tag at offset 0 as well as its form bytes.
pub fn sniff_mime(head: &[u8]) -> Option<&'static str> {
    let head = &head[..head.len().min(SNIFF_HEAD_BYTES)];
    SNIFF_TABLE.iter().find_map(|(offset, magic, mime)| {
        let riff = matches!(*mime, "image/webp" | "audio/wav");
        let fits = head.get(*offset..*offset + magic.len()) == Some(*magic)
            && (!riff || head.get(..4) == Some(b"RIFF".as_slice()));
        fits.then_some(*mime)
    })
}

/// One recalled derivation: its id and the cosine score against the query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DerivationHit {
    /// The derivation id.
    pub id: String,
    /// Cosine similarity of the query and the stored embedding.
    pub score: f32,
}

/// Cosine similarity, accumulated in `f64`; `None` for a length mismatch, an empty vector
/// or a zero norm. This crate sits below the semantic index in the layer order, so it
/// keeps its own copy of the standard formula.
fn cosine(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let (mut dot, mut norm_a, mut norm_b) = (0f64, 0f64, 0f64);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (f64::from(*x), f64::from(*y));
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return None;
    }
    Some((dot / (norm_a.sqrt() * norm_b.sqrt())) as f32)
}

/// Rank `candidates` (derivation id, stored embedding) against `query` by cosine, best
/// first, equal scores in id order, and keep the first `limit`. A candidate with no
/// defined cosine is not a hit. The order does not depend on thread count.
pub fn match_derivation(
    query: &[f32],
    candidates: &[(String, Vec<f32>)],
    limit: usize,
) -> Vec<DerivationHit> {
    let mut hits: Vec<DerivationHit> = candidates
        .iter()
        .filter_map(|(id, embedding)| {
            cosine(query, embedding).map(|score| DerivationHit {
                id: id.clone(),
                score,
            })
        })
        .collect();
    hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    hits.truncate(limit);
    hits
}
