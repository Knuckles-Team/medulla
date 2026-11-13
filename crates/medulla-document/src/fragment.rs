//! Fragments and the reader registry (MEDULLA-INGEST-WORKER-R700.1).
//!
//! A fragment is a [`ChunkSpan`] plus a modality: the text one reader pulled out
//! of a source, with the position it came from. `ChunkSpan` is the only span
//! type. Readers register by media type; the registry reads a source by the
//! media type its caller names.

use std::collections::BTreeMap;
use std::sync::Arc;

use medulla_modality::artifact::ModalityKind;
use medulla_types::contract::Digest256;
use serde::{Deserialize, Serialize};

use crate::processing::ChunkSpan;

/// Stable code of a span refused at construction.
pub const FRAGMENT_SPAN_INVALID: &str = "FRAGMENT_SPAN_INVALID";
/// Stable code of a media type no reader is registered for.
pub const READER_UNSUPPORTED_MEDIA_TYPE: &str = "READER_UNSUPPORTED_MEDIA_TYPE";
/// Stable code of a second reader for a media type that already has one.
pub const READER_DUPLICATE_MEDIA_TYPE: &str = "READER_DUPLICATE_MEDIA_TYPE";

/// The media types the registry is built for. The Office Open XML wordprocessing,
/// presentation and spreadsheet types are part of the set.
pub const SUPPORTED_MEDIA_TYPES: &[&str] = &[
    "application/pdf",
    "message/rfc822",
    "text/html",
    "text/csv",
    "text/tab-separated-values",
    "text/vtt",
    "application/x-subrip",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
];

/// A refusal from fragment construction or the reader registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReaderError {
    /// The stable code: one of the constants of this module, or a reader's own.
    pub code: &'static str,
    /// What was refused.
    pub detail: String,
}

impl std::fmt::Display for ReaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for ReaderError {}

fn span_invalid(detail: impl Into<String>) -> ReaderError {
    ReaderError {
        code: FRAGMENT_SPAN_INVALID,
        detail: detail.into(),
    }
}

/// One piece of text a reader extracted, with where it came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Fragment {
    /// SHA-256 of the source bytes the fragment was read from.
    pub source: Digest256,
    /// Where the fragment sits in the extracted text.
    pub span: ChunkSpan,
    /// The fragment text.
    pub text: String,
    /// What kind of content the fragment holds.
    pub modality: ModalityKind,
    /// The 1-based page of a paged source; `None` for an unpaged one.
    pub page: Option<u32>,
}

#[derive(Deserialize)]
struct FragmentWire {
    source: Digest256,
    span: ChunkSpan,
    text: String,
    modality: ModalityKind,
    #[serde(default)]
    page: Option<u32>,
}

impl<'de> Deserialize<'de> for Fragment {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = FragmentWire::deserialize(deserializer)?;
        let fragment = Fragment::new(wire.source, wire.span, wire.text, wire.modality)
            .map_err(serde::de::Error::custom)?;
        match wire.page {
            Some(page) if page == 0 => Err(serde::de::Error::custom(span_invalid(
                "page numbers start at 1",
            ))),
            page => Ok(Fragment { page, ..fragment }),
        }
    }
}

impl Fragment {
    /// A fragment, or `FRAGMENT_SPAN_INVALID` when the span's end is not after
    /// its start, or when the span's character length exceeds its byte length.
    pub fn new(
        source: Digest256,
        span: ChunkSpan,
        text: String,
        modality: ModalityKind,
    ) -> Result<Self, ReaderError> {
        if span.end_byte <= span.start_byte {
            return Err(span_invalid(format!(
                "byte span {}..{} must end after it starts",
                span.start_byte, span.end_byte
            )));
        }
        if span.end_char <= span.start_char {
            return Err(span_invalid(format!(
                "char span {}..{} must end after it starts",
                span.start_char, span.end_char
            )));
        }
        if span.end_char - span.start_char > span.end_byte - span.start_byte {
            return Err(span_invalid(
                "a span cannot hold more characters than bytes",
            ));
        }
        Ok(Self {
            source,
            span,
            text,
            modality,
            page: None,
        })
    }

    /// Place the fragment on `page` (1-based) of a source with `page_count`
    /// pages, or `FRAGMENT_SPAN_INVALID` when the page lies outside it.
    pub fn on_page(mut self, page: u32, page_count: u32) -> Result<Self, ReaderError> {
        if page == 0 || page > page_count {
            return Err(span_invalid(format!(
                "page {page} is outside a document of {page_count} pages"
            )));
        }
        self.page = Some(page);
        Ok(self)
    }
}

/// Reads one family of media types into fragments.
pub trait Reader: Send + Sync {
    /// The media types this reader reads, lowercase and without parameters.
    fn media_types(&self) -> &'static [&'static str];

    /// Read `bytes` into fragments in source order. `source` is the digest of
    /// `bytes`; every fragment carries it.
    fn read(&self, source: Digest256, bytes: &[u8]) -> Result<Vec<Fragment>, ReaderError>;
}

/// Readers keyed by media type.
#[derive(Default)]
pub struct ReaderRegistry {
    readers: BTreeMap<String, Arc<dyn Reader>>,
}

/// The media type without parameters, lowercased: `Text/HTML; charset=utf-8`
/// becomes `text/html`.
pub fn normalize_media_type(media_type: &str) -> String {
    media_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

impl ReaderRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `reader` for each of its media types. A media type that already
    /// has a reader is refused with `READER_DUPLICATE_MEDIA_TYPE` and nothing is
    /// registered.
    pub fn register(&mut self, reader: Arc<dyn Reader>) -> Result<(), ReaderError> {
        let types = reader.media_types();
        if let Some(taken) = types.iter().find(|t| self.readers.contains_key(**t)) {
            return Err(ReaderError {
                code: READER_DUPLICATE_MEDIA_TYPE,
                detail: format!("a reader is already registered for '{taken}'"),
            });
        }
        for media_type in types {
            self.readers
                .insert((*media_type).to_string(), Arc::clone(&reader));
        }
        Ok(())
    }

    /// The registered media types, sorted.
    pub fn media_types(&self) -> Vec<&str> {
        self.readers.keys().map(String::as_str).collect()
    }

    /// The reader for `media_type`, if one is registered.
    pub fn get(&self, media_type: &str) -> Option<&Arc<dyn Reader>> {
        self.readers.get(&normalize_media_type(media_type))
    }

    /// Read `bytes` with the reader for `media_type`, or refuse with
    /// `READER_UNSUPPORTED_MEDIA_TYPE`.
    pub fn read(&self, media_type: &str, bytes: &[u8]) -> Result<Vec<Fragment>, ReaderError> {
        let reader = self.get(media_type).ok_or_else(|| ReaderError {
            code: READER_UNSUPPORTED_MEDIA_TYPE,
            detail: format!(
                "no reader is registered for '{}'",
                normalize_media_type(media_type)
            ),
        })?;
        reader.read(Digest256::sha256(bytes), bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: usize, end: usize) -> ChunkSpan {
        ChunkSpan {
            ordinal: 0,
            heading_path: vec!["Intro".into()],
            start_byte: start,
            end_byte: end,
            start_char: start,
            end_char: end,
        }
    }

    struct Lines;
    impl Reader for Lines {
        fn media_types(&self) -> &'static [&'static str] {
            &["text/csv", "text/tab-separated-values"]
        }
        fn read(&self, source: Digest256, bytes: &[u8]) -> Result<Vec<Fragment>, ReaderError> {
            let text = String::from_utf8_lossy(bytes).to_string();
            Ok(vec![Fragment::new(
                source,
                span(0, text.len()),
                text,
                ModalityKind::Table,
            )?])
        }
    }

    // spec: MEDULLA-INGEST-WORKER-R700.1
    #[test]
    fn golden_fragment_round_trips_and_rejects_bad_spans() {
        let source = Digest256::sha256(b"a,b\n1,2\n");
        let fragment = Fragment::new(source, span(0, 8), "a,b\n1,2\n".into(), ModalityKind::Table)
            .unwrap()
            .on_page(2, 3)
            .unwrap();
        let json = serde_json::to_string(&fragment).unwrap();
        assert_eq!(serde_json::from_str::<Fragment>(&json).unwrap(), fragment);

        // End not after start, in bytes and in characters.
        for bad in [span(4, 4), span(5, 4)] {
            let err = Fragment::new(source, bad, "x".into(), ModalityKind::Document).unwrap_err();
            assert_eq!(err.code, "FRAGMENT_SPAN_INVALID");
        }
        let mut chars = span(0, 4);
        chars.end_char = 0;
        assert!(Fragment::new(source, chars, "x".into(), ModalityKind::Document).is_err());
        let mut wide = span(0, 4);
        wide.end_char = 9;
        assert!(Fragment::new(source, wide, "x".into(), ModalityKind::Document).is_err());

        // A page outside the document.
        let ok = Fragment::new(source, span(0, 4), "x".into(), ModalityKind::Document).unwrap();
        assert_eq!(
            ok.clone().on_page(0, 3).unwrap_err().code,
            "FRAGMENT_SPAN_INVALID"
        );
        assert_eq!(
            ok.clone().on_page(4, 3).unwrap_err().code,
            "FRAGMENT_SPAN_INVALID"
        );
        assert_eq!(ok.on_page(3, 3).unwrap().page, Some(3));

        // A bad span is refused when read back, too.
        let bad_json = json.replace("\"end_byte\":8", "\"end_byte\":0");
        assert!(serde_json::from_str::<Fragment>(&bad_json).is_err());

        // The registry is keyed by normalized media type, refuses duplicates and unknowns.
        let mut registry = ReaderRegistry::new();
        registry.register(Arc::new(Lines)).unwrap();
        assert_eq!(
            registry.media_types(),
            vec!["text/csv", "text/tab-separated-values"]
        );
        let read = registry.read("Text/CSV; charset=utf-8", b"a,b\n").unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].source, Digest256::sha256(b"a,b\n"));
        assert_eq!(
            registry.register(Arc::new(Lines)).unwrap_err().code,
            "READER_DUPLICATE_MEDIA_TYPE"
        );
        assert_eq!(
            registry.read("application/pdf", b"%PDF").unwrap_err().code,
            "READER_UNSUPPORTED_MEDIA_TYPE"
        );
        assert!(SUPPORTED_MEDIA_TYPES.contains(&"application/pdf"));
        assert_eq!(SUPPORTED_MEDIA_TYPES.len(), 10);
    }
}
