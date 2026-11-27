//! Office Open XML wordprocessing and presentation packages
//! (MEDULLA-INGEST-WORKER-R700.8): one fragment per non-empty paragraph.

use std::io::{Cursor, Read};

use medulla_modality::artifact::ModalityKind;
use medulla_types::contract::Digest256;
use quick_xml::events::Event;
use zip::result::ZipError;
use zip::ZipArchive;

use super::extracted::{extracted_fragments, ExtractedUnit};
use super::{
    is_compound_file, ReaderLimits, READER_CORRUPT, READER_ENCRYPTED, READER_TOO_LARGE,
};
use crate::fragment::{Fragment, Reader, ReaderError};

const WORD_TYPE: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
const PRESENTATION_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.presentation";
const WORD_PART: &str = "word/document.xml";
const SLIDE_PREFIX: &str = "ppt/slides/slide";
const SLIDE_SUFFIX: &str = ".xml";

/// Which package family a reader instance reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OoxmlKind {
    Word,
    Presentation,
}

/// Reads wordprocessing or presentation packages. A reader receives no media
/// type, so each family has its own instance.
pub(super) struct OoxmlReader {
    limits: ReaderLimits,
    kind: OoxmlKind,
}

impl OoxmlReader {
    pub(super) fn word(limits: ReaderLimits) -> Self {
        Self {
            limits,
            kind: OoxmlKind::Word,
        }
    }

    pub(super) fn presentation(limits: ReaderLimits) -> Self {
        Self {
            limits,
            kind: OoxmlKind::Presentation,
        }
    }
}

fn corrupt(detail: impl Into<String>) -> ReaderError {
    ReaderError {
        code: READER_CORRUPT,
        detail: detail.into(),
    }
}

fn zip_error(error: ZipError) -> ReaderError {
    corrupt(error.to_string())
}

/// The number `N` of a part named `ppt/slides/slide<N>.xml`.
fn slide_number(name: &str) -> Option<u32> {
    name.strip_prefix(SLIDE_PREFIX)?
        .strip_suffix(SLIDE_SUFFIX)?
        .parse()
        .ok()
}

/// Refuse an archive that holds an encrypted entry.
fn refuse_encrypted_entries(archive: &mut ZipArchive<Cursor<&[u8]>>) -> Result<(), ReaderError> {
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(zip_error)?;
        if entry.encrypted() {
            return Err(ReaderError {
                code: READER_ENCRYPTED,
                detail: format!(
                    "ooxml part {} is encrypted",
                    entry.name().as_deref().unwrap_or("unnamed")
                ),
            });
        }
    }
    Ok(())
}

/// Read one part in full; the bytes of every part together stay within
/// `limits.max_bytes()`, so a zip bomb stays inside the knob.
fn read_part(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    name: &str,
    limits: &ReaderLimits,
    total: &mut usize,
) -> Result<Vec<u8>, ReaderError> {
    let entry = archive
        .by_name(name)
        .map_err(|error| corrupt(format!("part {name}: {error}")))?;
    let mut part = Vec::new();
    entry
        .take(limits.max_bytes() as u64 + 1)
        .read_to_end(&mut part)
        .map_err(|error| corrupt(format!("part {name}: {error}")))?;
    *total += part.len();
    if *total > limits.max_bytes() {
        return Err(ReaderError {
            code: READER_TOO_LARGE,
            detail: format!(
                "the parts of the package exceed the reader limit of {}",
                limits.max_bytes()
            ),
        });
    }
    Ok(part)
}

/// The text of every non-empty paragraph of one XML part. `prefix` is the
/// namespace prefix of the format: `w` (word) or `a` (presentation).
fn paragraphs(part: &[u8], prefix: &str) -> Result<Vec<String>, ReaderError> {
    let paragraph = format!("{prefix}:p");
    let text = format!("{prefix}:t");
    let tab = format!("{prefix}:tab");
    let line_break = format!("{prefix}:br");
    let properties = format!("{prefix}:pPr");
    let mut reader = quick_xml::Reader::from_reader(part);
    let mut found = Vec::new();
    let mut current: Option<String> = None;
    let mut in_text = false;
    let mut in_properties = 0usize;
    loop {
        let event = reader
            .read_event()
            .map_err(|error| corrupt(error.to_string()))?;
        match event {
            Event::Eof => break,
            Event::Start(start) => {
                let name = start.name();
                let name = name.as_ref();
                if name == paragraph.as_bytes() {
                    current = Some(String::new());
                } else if name == properties.as_bytes() {
                    in_properties += 1;
                } else if name == text.as_bytes() {
                    in_text = true;
                } else if let Some(open) = current.as_mut() {
                    add_break(name, &tab, &line_break, in_properties, open);
                }
            }
            Event::Empty(start) => {
                if let Some(open) = current.as_mut() {
                    add_break(start.name().as_ref(), &tab, &line_break, in_properties, open);
                }
            }
            Event::End(end) => {
                let name = end.name();
                let name = name.as_ref();
                if name == paragraph.as_bytes() {
                    found.extend(current.take());
                } else if name == properties.as_bytes() {
                    in_properties = in_properties.saturating_sub(1);
                } else if name == text.as_bytes() {
                    in_text = false;
                }
            }
            Event::Text(content) if in_text => {
                if let Some(open) = current.as_mut() {
                    open.push_str(&content.decode().map_err(|error| corrupt(error.to_string()))?);
                }
            }
            Event::CData(content) if in_text => {
                if let Some(open) = current.as_mut() {
                    open.push_str(&content.decode().map_err(|error| corrupt(error.to_string()))?);
                }
            }
            Event::GeneralRef(reference) if in_text => {
                if let Some(open) = current.as_mut() {
                    push_reference(open, &reference)?;
                }
            }
            _ => {}
        }
    }
    Ok(found)
}

/// A tab inside a paragraph adds `\t` (a tab-stop definition inside the
/// paragraph properties is not text), a break adds `\n`.
fn add_break(name: &[u8], tab: &str, line_break: &str, in_properties: usize, open: &mut String) {
    if name == tab.as_bytes() && in_properties == 0 {
        open.push('\t');
    } else if name == line_break.as_bytes() {
        open.push('\n');
    }
}

/// Resolve one character or predefined entity reference.
fn push_reference(
    open: &mut String,
    reference: &quick_xml::events::BytesRef<'_>,
) -> Result<(), ReaderError> {
    if let Some(character) = reference
        .resolve_char_ref()
        .map_err(|error| corrupt(error.to_string()))?
    {
        open.push(character);
        return Ok(());
    }
    match &**reference {
        b"lt" => open.push('<'),
        b"gt" => open.push('>'),
        b"amp" => open.push('&'),
        b"quot" => open.push('"'),
        b"apos" => open.push('\''),
        other => {
            return Err(corrupt(format!(
                "unknown entity &{};",
                String::from_utf8_lossy(other)
            )))
        }
    }
    Ok(())
}

impl Reader for OoxmlReader {
    fn media_types(&self) -> &'static [&'static str] {
        match self.kind {
            OoxmlKind::Word => &[WORD_TYPE],
            OoxmlKind::Presentation => &[PRESENTATION_TYPE],
        }
    }

    fn read(&self, source: Digest256, bytes: &[u8]) -> Result<Vec<Fragment>, ReaderError> {
        self.limits.check_size(bytes)?;
        if is_compound_file(bytes) {
            return Err(ReaderError {
                code: READER_ENCRYPTED,
                detail: "ooxml package is an encrypted compound file".into(),
            });
        }
        let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(zip_error)?;
        refuse_encrypted_entries(&mut archive)?;
        let mut total = 0usize;
        let mut units = Vec::new();
        match self.kind {
            OoxmlKind::Word => {
                let part = read_part(&mut archive, WORD_PART, &self.limits, &mut total)?;
                for text in paragraphs(&part, "w")? {
                    units.push(ExtractedUnit {
                        text,
                        heading_path: Vec::new(),
                        page: None,
                    });
                }
            }
            OoxmlKind::Presentation => {
                let mut slides: Vec<(u32, String)> = archive
                    .file_names()
                    .filter_map(|name| {
                        // A part whose name is not valid text cannot be a slide part.
                        let name = name.ok()?;
                        slide_number(&name).map(|n| (n, name.into_owned()))
                    })
                    .collect();
                slides.sort();
                if slides.is_empty() {
                    return Err(corrupt("the package holds no slide part"));
                }
                let slide_count = slides.len() as u32;
                for (position, (_, name)) in slides.iter().enumerate() {
                    let part = read_part(&mut archive, name, &self.limits, &mut total)?;
                    for text in paragraphs(&part, "a")? {
                        units.push(ExtractedUnit {
                            text,
                            heading_path: Vec::new(),
                            page: Some((position as u32 + 1, slide_count)),
                        });
                    }
                }
            }
        }
        extracted_fragments(source, units, ModalityKind::Document)
    }
}
