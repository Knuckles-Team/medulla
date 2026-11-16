//! Bounded streaming readers (MEDULLA-INGEST-WORKER-R700.2).
//!
//! Every reader walks its source once, so time is O(bytes). A source is cut at
//! record boundaries into units of about `unit_bytes`; the units are independent,
//! so up to `parallelism` threads read them at once, and the fragments come out
//! in source order. A reader holds the source plus one copy of each fragment's
//! text, which keeps it within twice the byte limit.

use std::sync::Arc;

use medulla_modality::artifact::ModalityKind;
use medulla_types::contract::Digest256;

use crate::fragment::{Fragment, ReaderError, ReaderRegistry};
use crate::processing::ChunkSpan;

mod csv;
mod html;
mod subtitle;

/// Stable code of a source over the byte limit.
pub const READER_TOO_LARGE: &str = "READER_TOO_LARGE";
/// Stable code of a source the reader cannot parse.
pub const READER_CORRUPT: &str = "READER_CORRUPT";
/// A source is cut into this many units at the byte limit, so the unit size is
/// derived from `ingest.reader_max_bytes` and needs no second knob.
const UNITS_PER_LIMIT: usize = 1024;

/// The two limits every reader runs under: the values of the engine knobs
/// `ingest.reader_max_bytes` and `ingest.reader_parallelism`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReaderLimits {
    max_bytes: usize,
    parallelism: usize,
}

impl ReaderLimits {
    /// Limits for `max_bytes` of source and `parallelism` threads; a zero in
    /// either is refused with `READER_CORRUPT`.
    pub fn new(max_bytes: usize, parallelism: usize) -> Result<Self, ReaderError> {
        if max_bytes == 0 || parallelism == 0 {
            return Err(ReaderError {
                code: READER_CORRUPT,
                detail: "reader limits must be at least one byte and one thread".into(),
            });
        }
        Ok(Self {
            max_bytes,
            parallelism,
        })
    }

    /// The most source bytes a reader accepts.
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// The threads a reader may use.
    pub fn parallelism(&self) -> usize {
        self.parallelism
    }

    fn unit_bytes(&self) -> usize {
        (self.max_bytes / UNITS_PER_LIMIT).max(1)
    }

    fn check_size(&self, bytes: &[u8]) -> Result<(), ReaderError> {
        if bytes.len() > self.max_bytes {
            return Err(ReaderError {
                code: READER_TOO_LARGE,
                detail: format!(
                    "{} bytes exceed the reader limit of {}",
                    bytes.len(),
                    self.max_bytes
                ),
            });
        }
        Ok(())
    }
}

/// A registry holding the delimited-text, subtitle and HTML readers under `limits`.
pub fn text_registry(limits: ReaderLimits) -> Result<ReaderRegistry, ReaderError> {
    let mut registry = ReaderRegistry::new();
    registry.register(Arc::new(csv::DelimitedReader::csv(limits)))?;
    registry.register(Arc::new(csv::DelimitedReader::tsv(limits)))?;
    registry.register(Arc::new(subtitle::SubtitleReader::new(limits)))?;
    registry.register(Arc::new(html::HtmlReader::new(limits)))?;
    Ok(registry)
}

/// Read `text` as fragments cut at `cuts` (ascending end offsets, the last at
/// `text.len()`), in parallel, in order.
fn build_fragments(
    source: Digest256,
    text: &[u8],
    cuts: &[usize],
    modality: ModalityKind,
    limits: &ReaderLimits,
) -> Result<Vec<Fragment>, ReaderError> {
    let mut starts = Vec::with_capacity(cuts.len());
    let mut previous = 0;
    for &end in cuts {
        starts.push(previous);
        previous = end;
    }
    let per_thread = cuts.len().div_ceil(limits.parallelism()).max(1);
    let units: Vec<(usize, usize)> = starts.into_iter().zip(cuts.iter().copied()).collect();
    let counted: Vec<Result<Vec<usize>, ReaderError>> = std::thread::scope(|scope| {
        let handles: Vec<_> = units
            .chunks(per_thread)
            .map(|group| {
                scope.spawn(move || {
                    group
                        .iter()
                        .map(|&(start, end)| {
                            std::str::from_utf8(&text[start..end])
                                .map(|unit| unit.chars().count())
                                .map_err(|_| ReaderError {
                                    code: READER_CORRUPT,
                                    detail: format!("bytes {start}..{end} are not UTF-8"),
                                })
                        })
                        .collect::<Result<Vec<usize>, ReaderError>>()
                })
            })
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
    let mut fragments = Vec::with_capacity(units.len());
    let mut char_offset = 0;
    let mut ordinal = 0u32;
    let mut chars = Vec::with_capacity(units.len());
    for group in counted {
        chars.extend(group?);
    }
    for (&(start, end), count) in units.iter().zip(chars) {
        let span = ChunkSpan {
            ordinal,
            heading_path: Vec::new(),
            start_byte: start,
            end_byte: end,
            start_char: char_offset,
            end_char: char_offset + count,
        };
        let unit = std::str::from_utf8(&text[start..end]).map_err(|_| ReaderError {
            code: READER_CORRUPT,
            detail: format!("bytes {start}..{end} are not UTF-8"),
        })?;
        fragments.push(Fragment::new(source, span, unit.to_string(), modality)?);
        char_offset += count;
        ordinal += 1;
    }
    Ok(fragments)
}

/// Cut `bytes` after each record that ends once its unit holds `unit_bytes`.
/// `ends_record(bytes, i)` is called for every byte, in order, and says whether
/// a record ends at `i`.
fn cut_units(
    bytes: &[u8],
    unit_bytes: usize,
    mut ends_record: impl FnMut(&[u8], usize) -> bool,
) -> Vec<usize> {
    let mut cuts = Vec::new();
    let mut start = 0;
    for index in 0..bytes.len() {
        if ends_record(bytes, index) && index + 1 - start >= unit_bytes {
            cuts.push(index + 1);
            start = index + 1;
        }
    }
    if start < bytes.len() {
        cuts.push(bytes.len());
    }
    cuts
}
