//! Subtitle transcripts: `text/vtt` and `application/x-subrip`.

use medulla_modality::artifact::ModalityKind;
use medulla_types::contract::Digest256;

use super::{build_fragments, cut_units, ReaderLimits};
use crate::fragment::{Fragment, Reader, ReaderError};

/// Reads cues into cue-aligned fragments; a blank line ends a cue.
pub(super) struct SubtitleReader {
    limits: ReaderLimits,
}

impl SubtitleReader {
    pub(super) fn new(limits: ReaderLimits) -> Self {
        Self { limits }
    }
}

/// Whether the newline at `index` closes a blank line.
pub(super) fn closes_blank_line(bytes: &[u8], index: usize) -> bool {
    if bytes[index] != b'\n' {
        return false;
    }
    match index.checked_sub(1).map(|i| bytes[i]) {
        None => true,
        Some(b'\n') => true,
        Some(b'\r') => index == 1 || bytes[index - 2] == b'\n',
        Some(_) => false,
    }
}

impl Reader for SubtitleReader {
    fn media_types(&self) -> &'static [&'static str] {
        &["text/vtt", "application/x-subrip"]
    }

    fn read(&self, source: Digest256, bytes: &[u8]) -> Result<Vec<Fragment>, ReaderError> {
        self.limits.check_size(bytes)?;
        let cuts = cut_units(bytes, self.limits.unit_bytes(), closes_blank_line);
        build_fragments(source, bytes, &cuts, ModalityKind::Audio, &self.limits)
    }
}
