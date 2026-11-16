//! Delimited text: `text/csv` and `text/tab-separated-values`.

use medulla_modality::artifact::ModalityKind;
use medulla_types::contract::Digest256;

use super::{build_fragments, cut_units, ReaderLimits};
use crate::fragment::{Fragment, Reader, ReaderError};

/// Reads rows into row-aligned fragments. A CSV quoted field may hold a newline;
/// a TSV has no quoting.
pub(super) struct DelimitedReader {
    limits: ReaderLimits,
    quoted: bool,
}

impl DelimitedReader {
    /// The `text/csv` reader: a quoted field may hold a newline.
    pub(super) fn csv(limits: ReaderLimits) -> Self {
        Self {
            limits,
            quoted: true,
        }
    }

    /// The `text/tab-separated-values` reader: every newline ends a row.
    pub(super) fn tsv(limits: ReaderLimits) -> Self {
        Self {
            limits,
            quoted: false,
        }
    }
}

impl Reader for DelimitedReader {
    fn media_types(&self) -> &'static [&'static str] {
        if self.quoted {
            &["text/csv"]
        } else {
            &["text/tab-separated-values"]
        }
    }

    fn read(&self, source: Digest256, bytes: &[u8]) -> Result<Vec<Fragment>, ReaderError> {
        self.limits.check_size(bytes)?;
        let mut in_quote = false;
        let cuts = cut_units(bytes, self.limits.unit_bytes(), |all, index| {
            match all[index] {
                b'"' if self.quoted => {
                    in_quote = !in_quote;
                    false
                }
                b'\n' => !in_quote,
                _ => false,
            }
        });
        build_fragments(source, bytes, &cuts, ModalityKind::Table, &self.limits)
    }
}
