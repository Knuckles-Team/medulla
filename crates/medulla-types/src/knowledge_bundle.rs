//! The knowledge bundle: one scoped dataset that moves between tenants or deployments
//! (MEDULLA-DURABLE-KERNEL-R877.1, R877.3).
//!
//! A bundle is one tar stream whose first entry is the [`BundleManifest`]. For each graph it
//! holds `triples` (canonical N-Quads), `embeddings` (vector id, model id, dimension, float32
//! values), `provenance` (the property-graph rows that carry no RDF identity) and the
//! content-addressed source `documents`. The manifest digest is the SHA-256 of the canonical
//! manifest bytes; every part carries its own SHA-256, so changing one byte of any part fails
//! [`Bundle::unpack`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::contract::Digest256;

/// The only bundle version this engine reads and writes.
pub const BUNDLE_VERSION: u32 = 1;
/// The name of the first tar entry.
pub const MANIFEST_ENTRY: &str = "manifest.json";
/// Most triples one bundle holds.
pub const BUNDLE_TRIPLES_MAX: u64 = 5_000_000;
/// Most bytes of documents one bundle holds (4 GiB).
pub const BUNDLE_DOCUMENT_BYTES_MAX: u64 = 4 * 1024 * 1024 * 1024;
/// Most graphs one bundle holds.
pub const BUNDLE_GRAPHS_MAX: usize = 64;
/// The prefix of a content reference to a stored blob.
pub const CAS_PREFIX: &str = "cas:sha256:";

/// `BUNDLE_DIGEST_MISMATCH`: the manifest or a part does not match its digest.
pub const BUNDLE_DIGEST_MISMATCH: &str = "BUNDLE_DIGEST_MISMATCH";
/// `BUNDLE_VERSION_UNSUPPORTED`: the manifest names a `bundle_version` this engine does not read.
pub const BUNDLE_VERSION_UNSUPPORTED: &str = "BUNDLE_VERSION_UNSUPPORTED";
/// `BUNDLE_TOO_LARGE`: more triples, document bytes or graphs than the limits allow.
pub const BUNDLE_TOO_LARGE: &str = "BUNDLE_TOO_LARGE";
/// `BUNDLE_GRAPH_EXISTS`: a target graph exists and `on_conflict` is `refuse`.
pub const BUNDLE_GRAPH_EXISTS: &str = "BUNDLE_GRAPH_EXISTS";
/// `BUNDLE_EMBEDDING_MODEL_UNKNOWN`: an embedding names a model the target does not know.
pub const BUNDLE_EMBEDDING_MODEL_UNKNOWN: &str = "BUNDLE_EMBEDDING_MODEL_UNKNOWN";

/// The five bundle refusal codes.
pub const REFUSALS: [&str; 5] = [
    BUNDLE_DIGEST_MISMATCH,
    BUNDLE_VERSION_UNSUPPORTED,
    BUNDLE_TOO_LARGE,
    BUNDLE_GRAPH_EXISTS,
    BUNDLE_EMBEDDING_MODEL_UNKNOWN,
];

/// What an import does with a graph that already exists in the target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum BundleOnConflict {
    /// Refuse the whole import with `BUNDLE_GRAPH_EXISTS`.
    Refuse,
    /// Leave that graph as it is and import the others.
    Skip,
}

/// Why a bundle is refused: the code and the detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleError {
    /// One of [`REFUSALS`], or `PARAMETER_INVALID` for a stream that is no bundle.
    pub code: &'static str,
    /// What is wrong, without bundle content.
    pub detail: String,
}

impl BundleError {
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    fn malformed(detail: impl Into<String>) -> Self {
        Self::new("PARAMETER_INVALID", detail)
    }

    /// The wire text of the refusal: `CODE: detail`.
    pub fn message(&self) -> String {
        format!("{}: {}", self.code, self.detail)
    }
}

impl std::fmt::Display for BundleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

/// One part of one graph: its tar entry, SHA-256 and size.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PartRef {
    /// The tar entry that holds the part.
    pub entry: String,
    /// `sha256:<hex>` of the entry's bytes.
    pub sha256: String,
    /// Bytes of the entry.
    pub size: u64,
}

/// One content-addressed source document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DocumentRef {
    /// The reference the graph's rows hold: `cas:sha256:<manifest digest>`.
    pub cas_ref: String,
    /// `sha256:<hex>` of the document bytes.
    pub sha256: String,
    /// Bytes of the document.
    pub size: u64,
}

impl DocumentRef {
    /// The tar entry of the document.
    pub fn entry(&self) -> String {
        format!("documents/{}", self.sha256.trim_start_matches("sha256:"))
    }
}

/// The parts of one graph.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct GraphParts {
    /// The source graph name.
    pub graph: String,
    /// Canonical N-Quads of the graph's RDF triples.
    pub triples: PartRef,
    /// How many triples the part holds.
    pub triple_count: u64,
    /// The embeddings, when the export included them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embeddings: Option<PartRef>,
    /// The property-graph rows that carry no RDF identity.
    pub provenance: PartRef,
    /// The documents the graph's rows reference; empty when the export left them out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub documents: Vec<DocumentRef>,
}

/// The first entry of a bundle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BundleManifest {
    /// The integer 1.
    pub bundle_version: u32,
    /// The source graphs, in part order.
    pub graphs: Vec<String>,
    /// One entry per graph, in the order of `graphs`.
    pub parts: Vec<GraphParts>,
    /// Unix milliseconds the export read its snapshot.
    pub created_at: u64,
    /// `sha256:<hex>` over the canonical manifest bytes (this field empty).
    pub digest: String,
}

/// One embedding of the `embeddings` part.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct EmbeddingRecord {
    /// The node the vector belongs to.
    pub vector_id: String,
    /// The embedding model of the vector.
    pub model_id: String,
    /// The vector width.
    pub dimension: u32,
    /// The float32 values.
    pub values: Vec<f32>,
}

/// The `embeddings` part of one graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingsPart {
    /// The embedding space the source graph declares, as the engine's own space record.
    pub space: Option<serde_json::Value>,
    /// The vectors, in node-id order.
    pub records: Vec<EmbeddingRecord>,
}

/// What `BundleExport` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BundleExportReceipt {
    /// `cas:sha256:<digest>` of the stored bundle blob.
    pub bundle_ref: String,
    /// The manifest digest.
    pub manifest_digest: String,
    /// Bytes of the bundle blob.
    pub size_bytes: u64,
}

/// What one graph of an import did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ImportedGraph {
    /// The target graph name: the prefix and the source name.
    pub graph: String,
    /// Whether the graph was left alone because it existed and `on_conflict` is `skip`.
    pub skipped: bool,
}

/// What `BundleImport` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BundleImportReceipt {
    /// One entry per bundle graph.
    pub graphs: Vec<ImportedGraph>,
    /// Triples written.
    pub triples: u64,
    /// Embeddings written.
    pub embeddings: u64,
    /// Provenance rows written.
    pub provenance: u64,
    /// Documents stored.
    pub documents: u64,
}

/// `sha256:<hex>` of `bytes`.
pub fn digest_text(bytes: &[u8]) -> String {
    format!("sha256:{}", Digest256::sha256(bytes).to_hex())
}

impl BundleManifest {
    /// The canonical manifest bytes: the manifest with an empty `digest`, as compact JSON in
    /// declaration order.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut unsealed = self.clone();
        unsealed.digest = String::new();
        serde_json::to_vec(&unsealed).expect("a manifest is plain data")
    }

    /// The digest the manifest must carry.
    pub fn computed_digest(&self) -> String {
        digest_text(&self.canonical_bytes())
    }

    /// Set `digest` to the computed one.
    pub fn seal(mut self) -> Self {
        self.digest = self.computed_digest();
        self
    }

    /// Check the version, the shape and the digest.
    pub fn check(&self) -> Result<(), BundleError> {
        if self.bundle_version != BUNDLE_VERSION {
            return Err(BundleError::new(
                BUNDLE_VERSION_UNSUPPORTED,
                format!("bundle_version {} is not {BUNDLE_VERSION}", self.bundle_version),
            ));
        }
        if self.digest != self.computed_digest() {
            return Err(BundleError::new(
                BUNDLE_DIGEST_MISMATCH,
                "the manifest does not match its digest",
            ));
        }
        if self.graphs.len() != self.parts.len()
            || self.graphs.iter().zip(&self.parts).any(|(name, part)| name != &part.graph)
        {
            return Err(BundleError::malformed("the manifest graphs and parts disagree"));
        }
        self.check_limits()
    }

    /// Check the size limits.
    pub fn check_limits(&self) -> Result<(), BundleError> {
        let triples: u64 = self.parts.iter().map(|part| part.triple_count).sum();
        let mut seen = std::collections::BTreeSet::new();
        let document_bytes: u64 = self
            .parts
            .iter()
            .flat_map(|part| part.documents.iter())
            .filter(|document| seen.insert(document.sha256.as_str()))
            .map(|document| document.size)
            .sum();
        if self.graphs.len() > BUNDLE_GRAPHS_MAX
            || triples > BUNDLE_TRIPLES_MAX
            || document_bytes > BUNDLE_DOCUMENT_BYTES_MAX
        {
            return Err(BundleError::new(
                BUNDLE_TOO_LARGE,
                format!(
                    "{} graphs, {triples} triples and {document_bytes} document bytes exceed the \
                     bundle limits",
                    self.graphs.len()
                ),
            ));
        }
        Ok(())
    }
}

/// An unpacked bundle: the manifest and every entry by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bundle {
    /// The verified manifest.
    pub manifest: BundleManifest,
    /// The part and document bytes by tar entry name.
    pub entries: BTreeMap<String, Vec<u8>>,
}

impl Bundle {
    /// The bytes of one entry.
    pub fn entry(&self, name: &str) -> Result<&[u8], BundleError> {
        self.entries
            .get(name)
            .map(Vec::as_slice)
            .ok_or_else(|| BundleError::malformed(format!("the bundle holds no entry {name}")))
    }

    /// Pack a sealed manifest and its entries into one tar stream, manifest first.
    pub fn pack(manifest: &BundleManifest, entries: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
        let mut out = Vec::new();
        let manifest_bytes = serde_json::to_vec(manifest).expect("a manifest is plain data");
        write_entry(&mut out, MANIFEST_ENTRY, &manifest_bytes);
        for (name, bytes) in entries {
            write_entry(&mut out, name, bytes);
        }
        out.extend_from_slice(&[0u8; 1024]);
        out
    }

    /// Read a tar stream and verify it: the version first, then the manifest digest, the limits,
    /// and the SHA-256 and size of every part and document it names.
    pub fn unpack(bytes: &[u8]) -> Result<Self, BundleError> {
        let mut entries = read_entries(bytes)?;
        let (first, manifest_bytes) = entries
            .first()
            .cloned()
            .ok_or_else(|| BundleError::malformed("the bundle holds no entry"))?;
        if first != MANIFEST_ENTRY {
            return Err(BundleError::malformed("the first entry is not the manifest"));
        }
        entries.remove(0);
        let value: serde_json::Value = serde_json::from_slice(&manifest_bytes)
            .map_err(|error| BundleError::malformed(format!("the manifest is not JSON: {error}")))?;
        let version = value.get("bundle_version").and_then(serde_json::Value::as_u64);
        if version != Some(u64::from(BUNDLE_VERSION)) {
            return Err(BundleError::new(
                BUNDLE_VERSION_UNSUPPORTED,
                "the manifest names an unknown bundle_version",
            ));
        }
        let manifest: BundleManifest = serde_json::from_value(value)
            .map_err(|error| BundleError::malformed(format!("the manifest is malformed: {error}")))?;
        manifest.check()?;
        let entries: BTreeMap<String, Vec<u8>> = entries.into_iter().collect();
        let bundle = Self { manifest, entries };
        bundle.verify_parts()?;
        Ok(bundle)
    }

    /// Check every part and document against the manifest.
    pub fn verify_parts(&self) -> Result<(), BundleError> {
        let mismatch = |what: &str| {
            Err(BundleError::new(
                BUNDLE_DIGEST_MISMATCH,
                format!("{what} does not match its digest"),
            ))
        };
        for part in &self.manifest.parts {
            let named = [Some(&part.triples), part.embeddings.as_ref(), Some(&part.provenance)];
            for reference in named.into_iter().flatten() {
                let bytes = self.entry(&reference.entry)?;
                if bytes.len() as u64 != reference.size || digest_text(bytes) != reference.sha256 {
                    return mismatch(&reference.entry);
                }
            }
            for document in &part.documents {
                let entry = document.entry();
                let bytes = self.entry(&entry)?;
                if bytes.len() as u64 != document.size || digest_text(bytes) != document.sha256 {
                    return mismatch(&entry);
                }
            }
        }
        Ok(())
    }
}

const BLOCK: usize = 512;

fn write_octal(field: &mut [u8], value: u64) {
    let width = field.len() - 1;
    let text = format!("{value:0width$o}");
    field[..width].copy_from_slice(text.as_bytes());
    field[width] = 0;
}

fn write_entry(out: &mut Vec<u8>, name: &str, data: &[u8]) {
    let mut header = [0u8; BLOCK];
    let name_bytes = name.as_bytes();
    let length = name_bytes.len().min(100);
    header[..length].copy_from_slice(&name_bytes[..length]);
    write_octal(&mut header[100..108], 0o644);
    write_octal(&mut header[108..116], 0);
    write_octal(&mut header[116..124], 0);
    write_octal(&mut header[124..136], data.len() as u64);
    write_octal(&mut header[136..148], 0);
    header[148..156].fill(b' ');
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum: u64 = header.iter().map(|byte| u64::from(*byte)).sum();
    let text = format!("{checksum:06o}");
    header[148..154].copy_from_slice(text.as_bytes());
    header[154] = 0;
    header[155] = b' ';
    out.extend_from_slice(&header);
    out.extend_from_slice(data);
    let padding = (BLOCK - data.len() % BLOCK) % BLOCK;
    out.extend(std::iter::repeat(0u8).take(padding));
}

fn parse_octal(field: &[u8]) -> Result<u64, BundleError> {
    let text: String = field
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as char)
        .collect();
    u64::from_str_radix(text.trim(), 8)
        .map_err(|_| BundleError::malformed("a tar header field is not octal"))
}

fn read_entries(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, BundleError> {
    let mut entries = Vec::new();
    let mut at = 0usize;
    while at + BLOCK <= bytes.len() {
        let header = &bytes[at..at + BLOCK];
        if header.iter().all(|byte| *byte == 0) {
            return Ok(entries);
        }
        let mut probe = header.to_vec();
        probe[148..156].fill(b' ');
        let expected: u64 = probe.iter().map(|byte| u64::from(*byte)).sum();
        if parse_octal(&header[148..156])? != expected {
            return Err(BundleError::malformed("a tar header fails its checksum"));
        }
        let name: String = header[..100]
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| *byte as char)
            .collect();
        let size = usize::try_from(parse_octal(&header[124..136])?)
            .map_err(|_| BundleError::malformed("a tar entry is too large"))?;
        let start = at + BLOCK;
        let end = start
            .checked_add(size)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| BundleError::malformed("a tar entry is cut short"))?;
        entries.push((name, bytes[start..end].to_vec()));
        at = start + size.div_ceil(BLOCK) * BLOCK;
    }
    Err(BundleError::malformed("the tar stream has no end marker"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (BundleManifest, BTreeMap<String, Vec<u8>>) {
        let triples = b"<a> <b> <c> <g> .\n".to_vec();
        let embeddings = br#"{"space":null,"records":[]}"#.to_vec();
        let provenance = b"[]".to_vec();
        let document = b"a source document".to_vec();
        let reference = |entry: &str, bytes: &[u8]| PartRef {
            entry: entry.to_string(),
            sha256: digest_text(bytes),
            size: bytes.len() as u64,
        };
        let doc = DocumentRef {
            cas_ref: format!("{CAS_PREFIX}{}", "ab".repeat(32)),
            sha256: digest_text(&document),
            size: document.len() as u64,
        };
        let mut entries = BTreeMap::new();
        entries.insert("graphs/0000/triples.nq".to_string(), triples.clone());
        entries.insert("graphs/0000/embeddings.json".to_string(), embeddings.clone());
        entries.insert("graphs/0000/provenance.json".to_string(), provenance.clone());
        entries.insert(doc.entry(), document);
        let manifest = BundleManifest {
            bundle_version: BUNDLE_VERSION,
            graphs: vec!["g".into()],
            parts: vec![GraphParts {
                graph: "g".into(),
                triples: reference("graphs/0000/triples.nq", &triples),
                triple_count: 1,
                embeddings: Some(reference("graphs/0000/embeddings.json", &embeddings)),
                provenance: reference("graphs/0000/provenance.json", &provenance),
                documents: vec![doc],
            }],
            created_at: 7,
            digest: String::new(),
        }
        .seal();
        (manifest, entries)
    }

    // spec: MEDULLA-DURABLE-KERNEL-R877.1
    #[test]
    fn manifest_round_trips_and_digest_covers_every_part() {
        let (manifest, entries) = sample();
        let archive = Bundle::pack(&manifest, &entries);
        let bundle = Bundle::unpack(&archive).expect("a sealed bundle unpacks");
        assert_eq!(bundle.manifest, manifest);
        assert_eq!(bundle.entries, entries);
        let again = serde_json::from_slice::<BundleManifest>(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        assert_eq!(again, manifest);
        assert_eq!(manifest.digest, manifest.computed_digest());

        // Changing one byte of any part fails the digest check.
        for name in entries.keys() {
            let mut changed = entries.clone();
            changed.get_mut(name).unwrap()[0] ^= 1;
            let error = Bundle::unpack(&Bundle::pack(&manifest, &changed)).unwrap_err();
            assert_eq!(error.code, BUNDLE_DIGEST_MISMATCH, "{name}");
        }
        // Changing one byte of the manifest fails it too.
        let mut tampered = manifest.clone();
        tampered.created_at += 1;
        let error = Bundle::unpack(&Bundle::pack(&tampered, &entries)).unwrap_err();
        assert_eq!(error.code, BUNDLE_DIGEST_MISMATCH);
        // An unknown version is its own refusal, before any digest is read.
        let mut future = manifest.clone();
        future.bundle_version = 2;
        let error = Bundle::unpack(&Bundle::pack(&future, &entries)).unwrap_err();
        assert_eq!(error.code, BUNDLE_VERSION_UNSUPPORTED);
        // A count over the limit is refused as too large.
        let mut large = manifest.clone();
        large.parts[0].triple_count = BUNDLE_TRIPLES_MAX + 1;
        let error = Bundle::unpack(&Bundle::pack(&large.seal(), &entries)).unwrap_err();
        assert_eq!(error.code, BUNDLE_TOO_LARGE);
    }
}
