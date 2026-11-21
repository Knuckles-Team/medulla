//! `application/pdf` (MEDULLA-INGEST-WORKER-R700.7): one fragment per page that
//! has a text layer.

use medulla_modality::artifact::ModalityKind;
use medulla_types::contract::Digest256;

use super::extracted::{extracted_fragments, map_in_order, ExtractedUnit};
use super::{ReaderLimits, READER_CORRUPT, READER_ENCRYPTED};
use crate::fragment::{Fragment, Reader, ReaderError};

/// The token a PDF trailer uses to name its encryption dictionary.
const ENCRYPT_TOKEN: &[u8] = b"/Encrypt";
/// How many trailing bytes are searched for the token when the load fails.
const TRAILER_WINDOW: usize = 1024;

/// Reads the text layer of every page.
pub(super) struct PdfReader {
    limits: ReaderLimits,
}

impl PdfReader {
    pub(super) fn new(limits: ReaderLimits) -> Self {
        Self { limits }
    }
}

fn encrypted() -> ReaderError {
    ReaderError {
        code: READER_ENCRYPTED,
        detail: "pdf trailer names an /Encrypt dictionary".into(),
    }
}

fn trailer_names_encrypt(bytes: &[u8]) -> bool {
    let tail = &bytes[bytes.len().saturating_sub(TRAILER_WINDOW)..];
    tail.windows(ENCRYPT_TOKEN.len())
        .any(|window| window == ENCRYPT_TOKEN)
}

impl Reader for PdfReader {
    fn media_types(&self) -> &'static [&'static str] {
        &["application/pdf"]
    }

    fn read(&self, source: Digest256, bytes: &[u8]) -> Result<Vec<Fragment>, ReaderError> {
        self.limits.check_size(bytes)?;
        let document = match lopdf::Document::load_mem(bytes) {
            Ok(document) => document,
            Err(error) => {
                if trailer_names_encrypt(bytes) {
                    return Err(encrypted());
                }
                return Err(ReaderError {
                    code: READER_CORRUPT,
                    detail: error.to_string(),
                });
            }
        };
        if document.is_encrypted() {
            return Err(encrypted());
        }
        let page_numbers: Vec<u32> = document.get_pages().keys().copied().collect();
        let page_count = page_numbers.len() as u32;
        let texts = map_in_order(&page_numbers, &self.limits, |&page| {
            document
                .extract_text(&[page])
                .map(|text| text.replace("\r\n", "\n").trim().to_string())
                .map_err(|error| ReaderError {
                    code: READER_CORRUPT,
                    detail: format!("page {page}: {error}"),
                })
        })?;
        let units = page_numbers
            .into_iter()
            .zip(texts)
            .map(|(page, text)| ExtractedUnit {
                text,
                heading_path: Vec::new(),
                page: Some((page, page_count)),
            })
            .collect();
        extracted_fragments(source, units, ModalityKind::Document)
    }
}
