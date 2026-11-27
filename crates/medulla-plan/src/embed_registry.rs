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
        if entry.space_id != space_id
            || entry.dimensions != dimensions
            || entry.revision != revision
        {
            return Err(unavailable("is registered for a different space tuple"));
        }
        Ok(entry.encoder)
    }
}

/// One registry entry as a caller that knows only the encoder id sees it.
pub struct RegisteredEncoder {
    /// The space id the entry encodes for.
    pub space_id: String,
    /// The space's vector width.
    pub dimensions: usize,
    /// The space's revision.
    pub revision: String,
    /// The encoder.
    pub encoder: &'static dyn TextEmbedder,
}

impl ModalityEncoderRegistry {
    /// The entry registered as `encoder_id`, with the space tuple it encodes for
    /// (MEDULLA-FEDERATED-QUERY-R039.2.5: served `EncodeDocument` resolves its encoder
    /// here, the one encoder authority). No such entry is `QUERY_ENCODER_UNAVAILABLE`.
    pub fn lookup_by_id(&self, encoder_id: &str) -> Result<RegisteredEncoder, String> {
        let entries = self.entries.read().map_err(|_| {
            format!(
                "{}: modality encoder `{encoder_id}` is unreadable",
                ServerErrorCode::QueryEncoderUnavailable
            )
        })?;
        entries
            .get(encoder_id)
            .map(|entry| RegisteredEncoder {
                space_id: entry.space_id.clone(),
                dimensions: entry.dimensions,
                revision: entry.revision.clone(),
                encoder: entry.encoder,
            })
            .ok_or_else(|| {
                format!(
                    "{}: modality encoder `{encoder_id}` is not registered",
                    ServerErrorCode::QueryEncoderUnavailable
                )
            })
    }
}

impl ModalityEncoderRegistry {
    /// The text encoder of the space `(space_id, dimensions, revision)` for a caller that
    /// knows the space and not an encoder id (MEDULLA-FEDERATED-QUERY-R077.5: a column
    /// binding names its model, never an encoder). Every entry embeds a query that reaches
    /// it as text, so the entry registered under the space's own id is taken when there is
    /// one, else the first entry of the exact tuple in id order. No entry of that tuple is
    /// `QUERY_ENCODER_UNAVAILABLE`.
    pub fn lookup_by_space(
        &self,
        space_id: &str,
        dimensions: usize,
        revision: &str,
    ) -> Result<&'static dyn TextEmbedder, String> {
        let unavailable = |why: &str| {
            format!(
                "{}: space `{space_id}` ({dimensions} dimensions, revision `{revision}`) {why}",
                ServerErrorCode::QueryEncoderUnavailable
            )
        };
        let entries = self
            .entries
            .read()
            .map_err(|_| unavailable("has an unreadable encoder registry"))?;
        let matches = |entry: &Entry| {
            entry.space_id == space_id
                && entry.dimensions == dimensions
                && entry.revision == revision
        };
        entries
            .get(space_id)
            .filter(|entry| matches(entry))
            .or_else(|| entries.values().find(|entry| matches(entry)))
            .map(|entry| entry.encoder)
            .ok_or_else(|| unavailable("has no registered encoder"))
    }
}

/// MEDULLA-FEDERATED-QUERY-R039.1.6: one pinned encoder file's contract.
const ENCODER_FILE_SUFFIX: &str = ".encoder.json";
/// The one format revision of a pinned encoder file.
const ENCODER_FILE_FORMAT: u64 = 1;
/// The largest pinned encoder file the loader reads.
const ENCODER_FILE_MAX_BYTES: u64 = 16 * 1024 * 1024;
/// The most tokens one pinned encoder holds.
const ENCODER_MAX_TOKENS: usize = 65_536;
/// The widest space a pinned encoder file declares.
const ENCODER_MAX_DIMENSIONS: usize = 4096;

/// The space tuple a pinned encoder file registers for.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PinnedSpace {
    id: String,
    dimensions: usize,
    revision: String,
}

/// One `<anything>.encoder.json` file.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PinnedEncoderFile {
    format: u64,
    id: String,
    modality: String,
    space: PinnedSpace,
    tokens: BTreeMap<String, Vec<f64>>,
}

/// The encoder a pinned file defines: a whitespace token lookup, summed and L2-normalized.
struct PinnedEncoder {
    id: String,
    dimensions: usize,
    tokens: BTreeMap<String, Vec<f32>>,
}

impl TextEmbedder for PinnedEncoder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        let mut sum = vec![0f32; self.dimensions];
        let mut known = false;
        for token in text.split_ascii_whitespace() {
            if let Some(vector) = self.tokens.get(token) {
                known = true;
                for (component, value) in sum.iter_mut().zip(vector) {
                    *component += value;
                }
            }
        }
        let norm = sum.iter().map(|c| c * c).sum::<f32>().sqrt();
        if !known || norm == 0.0 || !norm.is_finite() {
            return Err(format!(
                "EMBEDDING_UNAVAILABLE: no token of the input is known to encoder `{}`",
                self.id
            ));
        }
        Ok(sum.into_iter().map(|c| c / norm).collect())
    }
}

/// One encoder a [`ModalityEncoderRegistry::load_dir`] call registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedEncoder {
    /// The registry id.
    pub id: String,
    /// The lowercase modality name the file declares.
    pub modality: String,
    /// The space id the entry is registered for.
    pub space_id: String,
    /// The space's vector width.
    pub dimensions: usize,
    /// The space's revision.
    pub revision: String,
    /// The file name the entry came from.
    pub file: String,
}

fn invalid_file(file: &str, reason: impl std::fmt::Display) -> String {
    format!("ENCODER_FILE_INVALID: {file}: {reason}")
}

/// Read and validate one pinned encoder file.
fn read_pinned(
    path: &std::path::Path,
    file: &str,
) -> Result<(LoadedEncoder, PinnedEncoder), String> {
    let size = std::fs::metadata(path)
        .map_err(|error| invalid_file(file, error))?
        .len();
    if size > ENCODER_FILE_MAX_BYTES {
        return Err(invalid_file(
            file,
            format!("{size} bytes exceed the {ENCODER_FILE_MAX_BYTES} byte limit"),
        ));
    }
    let bytes = std::fs::read(path).map_err(|error| invalid_file(file, error))?;
    let parsed: PinnedEncoderFile =
        serde_json::from_slice(&bytes).map_err(|error| invalid_file(file, error))?;
    if parsed.format != ENCODER_FILE_FORMAT {
        return Err(invalid_file(
            file,
            format!("format {} is not {ENCODER_FILE_FORMAT}", parsed.format),
        ));
    }
    if parsed.id.is_empty() || parsed.id.chars().count() > 128 {
        return Err(invalid_file(file, "id must be 1 to 128 characters"));
    }
    if !matches!(
        parsed.modality.as_str(),
        "text" | "image" | "audio" | "video"
    ) {
        return Err(invalid_file(
            file,
            format!(
                "modality `{}` is not text, image, audio or video",
                parsed.modality
            ),
        ));
    }
    let dimensions = parsed.space.dimensions;
    if !(1..=ENCODER_MAX_DIMENSIONS).contains(&dimensions) {
        return Err(invalid_file(
            file,
            format!("space dimensions {dimensions} are outside [1, {ENCODER_MAX_DIMENSIONS}]"),
        ));
    }
    if parsed.tokens.is_empty() || parsed.tokens.len() > ENCODER_MAX_TOKENS {
        return Err(invalid_file(
            file,
            format!("tokens must hold 1 to {ENCODER_MAX_TOKENS} entries"),
        ));
    }
    let mut tokens = BTreeMap::new();
    for (token, vector) in parsed.tokens {
        if vector.len() != dimensions || vector.iter().any(|c| !c.is_finite()) {
            return Err(invalid_file(
                file,
                format!("token `{token}` must be {dimensions} finite numbers"),
            ));
        }
        tokens.insert(token, vector.into_iter().map(|c| c as f32).collect());
    }
    let loaded = LoadedEncoder {
        id: parsed.id.clone(),
        modality: parsed.modality,
        space_id: parsed.space.id,
        dimensions,
        revision: parsed.space.revision,
        file: file.to_string(),
    };
    let encoder = PinnedEncoder {
        id: parsed.id,
        dimensions,
        tokens,
    };
    Ok((loaded, encoder))
}

impl ModalityEncoderRegistry {
    /// MEDULLA-FEDERATED-QUERY-R039.1.6: register every `*.encoder.json` file of `dir`, in
    /// ascending file-name order, and return one [`LoadedEncoder`] per file. Every file is
    /// validated before anything registers: a directory that cannot be read, a file that
    /// does not parse or validate and a repeated id (within the directory or against an
    /// entry already registered) each refuse `ENCODER_FILE_INVALID` and register nothing
    /// from this call. An empty directory is `Ok(vec![])`.
    pub fn load_dir(&self, dir: &std::path::Path) -> Result<Vec<LoadedEncoder>, String> {
        let shown = dir.display().to_string();
        let mut names = Vec::new();
        for entry in std::fs::read_dir(dir).map_err(|error| invalid_file(&shown, error))? {
            let entry = entry.map_err(|error| invalid_file(&shown, error))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(ENCODER_FILE_SUFFIX) {
                names.push(name);
            }
        }
        names.sort();
        let mut files = Vec::with_capacity(names.len());
        let mut seen = std::collections::BTreeSet::new();
        for name in &names {
            let (loaded, encoder) = read_pinned(&dir.join(name), name)?;
            if !seen.insert(loaded.id.clone()) {
                return Err(invalid_file(
                    name,
                    format!("id `{}` is repeated in the directory", loaded.id),
                ));
            }
            let registered = self
                .entries
                .read()
                .map_err(|_| invalid_file(name, "the registry is poisoned"))?
                .contains_key(&loaded.id);
            if registered {
                return Err(invalid_file(
                    name,
                    format!("id `{}` is already registered", loaded.id),
                ));
            }
            files.push((loaded, encoder));
        }
        let mut loaded = Vec::with_capacity(files.len());
        for (entry, encoder) in files {
            self.register(
                &entry.id,
                &entry.space_id,
                entry.dimensions,
                &entry.revision,
                Box::new(encoder),
            )
            .map_err(|error| invalid_file(&entry.file, error))?;
            loaded.push(entry);
        }
        Ok(loaded)
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
            .register(
                "enc-text",
                "space-a",
                8,
                "r1",
                Box::new(HashEmbedder::new(8)),
            )
            .unwrap();
        assert!(registry
            .register(
                "enc-text",
                "space-a",
                8,
                "r1",
                Box::new(HashEmbedder::new(8))
            )
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

    // spec: MEDULLA-FEDERATED-QUERY-R077.5
    #[test]
    fn lookup_by_space_finds_the_entry_of_the_exact_tuple() {
        let registry = ModalityEncoderRegistry::new();
        registry
            .register("enc-a", "space-a", 8, "r1", Box::new(HashEmbedder::new(8)))
            .unwrap();
        assert_eq!(
            registry
                .lookup_by_space("space-a", 8, "r1")
                .unwrap()
                .embed("x")
                .unwrap()
                .len(),
            8
        );
        for (space, dim, revision) in [
            ("space-b", 8, "r1"),
            ("space-a", 4, "r1"),
            ("space-a", 8, "r2"),
        ] {
            let error = registry
                .lookup_by_space(space, dim, revision)
                .err()
                .unwrap();
            assert!(error.starts_with("QUERY_ENCODER_UNAVAILABLE"), "{error}");
        }
    }

    fn write_encoder(dir: &std::path::Path, file: &str, id: &str, token: &str, vector: &str) {
        let body = format!(
            r#"{{"format":1,"id":"{id}","modality":"text","space":{{"id":"s","dimensions":2,"revision":"1"}},"tokens":{{"{token}":{vector}}}}}"#
        );
        std::fs::write(dir.join(file), body).unwrap();
    }

    // spec: MEDULLA-FEDERATED-QUERY-R039.1.6
    #[test]
    fn load_dir_registers_pinned_encoders_in_file_order() {
        let dir = std::env::temp_dir().join(format!("encoders-ok-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_encoder(&dir, "b.encoder.json", "enc-b", "beta", "[0.0, 3.0]");
        write_encoder(&dir, "a.encoder.json", "enc-a", "alpha", "[2.0, 0.0]");
        std::fs::write(dir.join("ignored.json"), "not an encoder").unwrap();
        let registry = ModalityEncoderRegistry::new();
        let loaded = registry.load_dir(&dir).unwrap();
        assert_eq!(
            loaded
                .iter()
                .map(|l| (l.id.as_str(), l.file.as_str()))
                .collect::<Vec<_>>(),
            vec![("enc-a", "a.encoder.json"), ("enc-b", "b.encoder.json")]
        );
        let encoder = registry.lookup("enc-a", "s", 2, "1").unwrap();
        assert_eq!(
            encoder.embed("alpha alpha unknown").unwrap(),
            vec![1.0, 0.0]
        );
        let error = encoder.embed("beta").unwrap_err();
        assert!(error.starts_with("EMBEDDING_UNAVAILABLE"), "{error}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // spec: MEDULLA-FEDERATED-QUERY-R039.1.6
    #[test]
    fn load_dir_refuses_everything_or_nothing() {
        let dir = std::env::temp_dir().join(format!("encoders-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_encoder(&dir, "a.encoder.json", "enc-a", "alpha", "[1.0, 0.0]");
        write_encoder(&dir, "b.encoder.json", "enc-a", "beta", "[0.0, 1.0]");
        let registry = ModalityEncoderRegistry::new();
        let error = registry.load_dir(&dir).unwrap_err();
        assert!(
            error.starts_with("ENCODER_FILE_INVALID: b.encoder.json"),
            "{error}"
        );
        assert!(registry.lookup("enc-a", "s", 2, "1").is_err());
        // A wrong-width token and an unknown key each refuse.
        write_encoder(&dir, "b.encoder.json", "enc-b", "beta", "[0.0]");
        assert!(registry
            .load_dir(&dir)
            .unwrap_err()
            .contains("b.encoder.json"));
        std::fs::write(dir.join("b.encoder.json"), r#"{"format":1,"extra":1}"#).unwrap();
        assert!(registry
            .load_dir(&dir)
            .unwrap_err()
            .contains("b.encoder.json"));
        std::fs::remove_dir_all(&dir).unwrap();
        let missing = registry.load_dir(&dir).unwrap_err();
        assert!(missing.starts_with("ENCODER_FILE_INVALID"), "{missing}");
        let empty = std::env::temp_dir().join(format!("encoders-empty-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(registry.load_dir(&empty), Ok(vec![]));
        std::fs::remove_dir_all(&empty).unwrap();
    }
}
