//! Spans over extracted text (MEDULLA-INGEST-WORKER-R700.7).
//!
//! A format whose bytes are not text has no source byte range to cite, so its
//! fragment spans address the extracted text: the texts of the kept fragments, in
//! order, joined by one `\n`. The joiner is the one `normalize_record` uses, so a
//! span also addresses the document text the atoms are cut from.

use medulla_modality::artifact::ModalityKind;
use medulla_types::contract::Digest256;

#[cfg(any(feature = "reader-pdf", feature = "reader-spreadsheet"))]
use super::{ReaderLimits, READER_CORRUPT};
use crate::fragment::{Fragment, ReaderError};
use crate::processing::ChunkSpan;

/// One unit of text a format reader pulled out of its source: a page, a
/// paragraph, a sheet row or a message part.
pub(super) struct ExtractedUnit {
    /// The unit text; a unit that is empty after `trim()` produces no fragment.
    pub text: String,
    /// The headings that enclose the unit, outermost first.
    pub heading_path: Vec<String>,
    /// The 1-based page (or slide) and the page count, for `Fragment::on_page`.
    pub page: Option<(u32, u32)>,
}

/// The fragments of `units`: every unit with text becomes one fragment whose
/// span addresses the `\n`-joined text of the kept fragments.
pub(super) fn extracted_fragments(
    source: Digest256,
    units: Vec<ExtractedUnit>,
    modality: ModalityKind,
) -> Result<Vec<Fragment>, ReaderError> {
    let mut fragments = Vec::new();
    let mut byte_offset = 0usize;
    let mut char_offset = 0usize;
    for unit in units {
        if unit.text.trim().is_empty() {
            continue;
        }
        let chars = unit.text.chars().count();
        let span = ChunkSpan {
            ordinal: fragments.len() as u32,
            heading_path: unit.heading_path,
            start_byte: byte_offset,
            end_byte: byte_offset + unit.text.len(),
            start_char: char_offset,
            end_char: char_offset + chars,
        };
        byte_offset += unit.text.len() + 1;
        char_offset += chars + 1;
        let fragment = Fragment::new(source, span, unit.text, modality)?;
        fragments.push(match unit.page {
            Some((page, count)) => fragment.on_page(page, count)?,
            None => fragment,
        });
    }
    Ok(fragments)
}

/// Run `work` over `items` on up to `limits.parallelism()` scoped threads and
/// return the results in the order of `items`.
#[cfg(any(feature = "reader-pdf", feature = "reader-spreadsheet"))]
pub(super) fn map_in_order<T: Sync, R: Send>(
    items: &[T],
    limits: &ReaderLimits,
    work: impl Fn(&T) -> Result<R, ReaderError> + Sync,
) -> Result<Vec<R>, ReaderError> {
    let per_thread = items.len().div_ceil(limits.parallelism()).max(1);
    let work = &work;
    let groups: Vec<Result<Vec<R>, ReaderError>> = std::thread::scope(|scope| {
        let handles: Vec<_> = items
            .chunks(per_thread)
            .map(|group| scope.spawn(move || group.iter().map(work).collect()))
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle.join().unwrap_or_else(|_| {
                    Err(ReaderError {
                        code: READER_CORRUPT,
                        detail: "a reader thread failed".into(),
                    })
                })
            })
            .collect()
    });
    let mut out = Vec::with_capacity(items.len());
    for group in groups {
        out.extend(group?);
    }
    Ok(out)
}
