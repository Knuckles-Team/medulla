//! `text/html`: markup stripped to text, fragments over the extracted text.

use medulla_modality::artifact::ModalityKind;
use medulla_types::contract::Digest256;

use super::{build_fragments, cut_units, ReaderLimits, READER_CORRUPT};
use crate::fragment::{Fragment, Reader, ReaderError};

/// Reads the visible text of a page. A fragment span addresses the extracted
/// text, not the markup; the source digest is the digest of the markup.
pub(super) struct HtmlReader {
    limits: ReaderLimits,
}

impl HtmlReader {
    pub(super) fn new(limits: ReaderLimits) -> Self {
        Self { limits }
    }
}

impl Reader for HtmlReader {
    fn media_types(&self) -> &'static [&'static str] {
        &["text/html"]
    }

    fn read(&self, source: Digest256, bytes: &[u8]) -> Result<Vec<Fragment>, ReaderError> {
        self.limits.check_size(bytes)?;
        let text = crate::html::strip_to_text(bytes).ok_or_else(|| ReaderError {
            code: READER_CORRUPT,
            detail: "the page is empty, not UTF-8 or over the HTML scanner limit".into(),
        })?;
        let text = text.as_bytes();
        let cuts = cut_units(text, self.limits.unit_bytes(), |all, index| {
            all[index] == b'\n'
        });
        build_fragments(source, text, &cuts, ModalityKind::Document, &self.limits)
    }
}
