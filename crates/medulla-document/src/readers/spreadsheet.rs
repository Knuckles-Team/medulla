//! Office Open XML spreadsheets (MEDULLA-INGEST-WORKER-R700.9): one fragment per
//! non-blank row, addressed by its A1 range.

use std::io::Cursor;

use calamine::{Data, Reader as _, Xlsx};
use medulla_modality::artifact::ModalityKind;
use medulla_types::contract::Digest256;

use super::extracted::{extracted_fragments, map_in_order, ExtractedUnit};
use super::{is_compound_file, ReaderLimits, READER_CORRUPT, READER_ENCRYPTED};
use crate::fragment::{Fragment, Reader, ReaderError};

const SHEET_TYPE: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/// Reads every sheet, hidden ones included, in workbook order.
pub(super) struct SpreadsheetReader {
    limits: ReaderLimits,
}

impl SpreadsheetReader {
    pub(super) fn new(limits: ReaderLimits) -> Self {
        Self { limits }
    }
}

fn corrupt(detail: impl Into<String>) -> ReaderError {
    ReaderError {
        code: READER_CORRUPT,
        detail: detail.into(),
    }
}

fn encrypted() -> ReaderError {
    ReaderError {
        code: READER_ENCRYPTED,
        detail: "workbook is an encrypted compound file".into(),
    }
}

/// The column letters of a 0-based column index: `0` is `A`, `26` is `AA`.
fn column_letters(index: u32) -> String {
    let mut letters = Vec::new();
    let mut rest = index + 1;
    while rest > 0 {
        let digit = ((rest - 1) % 26) as u8;
        letters.push(b'A' + digit);
        rest = (rest - 1) / 26;
    }
    letters.reverse();
    String::from_utf8_lossy(&letters).into_owned()
}

fn render(cell: &Data) -> String {
    match cell {
        Data::Bool(true) => "TRUE".into(),
        Data::Bool(false) => "FALSE".into(),
        Data::DateTime(date) => date.as_f64().to_string(),
        other => other.to_string(),
    }
}

/// The row fragments of one sheet.
fn sheet_units(
    workbook: &mut Xlsx<Cursor<&[u8]>>,
    name: &str,
) -> Result<Vec<ExtractedUnit>, ReaderError> {
    let range = workbook
        .worksheet_range(name)
        .map_err(|error| corrupt(format!("sheet {name}: {error}")))?;
    let Some((first_row, first_col)) = range.start() else {
        return Ok(Vec::new());
    };
    let mut units = Vec::new();
    for (offset, row) in range.rows().enumerate() {
        let Some(last) = row.iter().rposition(|cell| !cell.is_empty()) else {
            continue;
        };
        let text = row[..=last]
            .iter()
            .map(render)
            .collect::<Vec<_>>()
            .join("\t");
        let number = first_row as usize + offset + 1;
        let span = format!(
            "{}{number}:{}{number}",
            column_letters(first_col),
            column_letters(first_col + last as u32)
        );
        units.push(ExtractedUnit {
            text,
            heading_path: vec![name.to_string(), span],
            page: None,
        });
    }
    Ok(units)
}

/// An entry of the archive is encrypted. Only checked when a zip reader is
/// built in; otherwise the workbook parser reports what it cannot read.
#[cfg(feature = "reader-ooxml")]
fn has_encrypted_entry(bytes: &[u8]) -> bool {
    let Ok(mut archive) = zip::ZipArchive::new(Cursor::new(bytes)) else {
        return false;
    };
    (0..archive.len()).any(|index| {
        archive
            .by_index_raw(index)
            .map(|entry| entry.encrypted())
            .unwrap_or(false)
    })
}

#[cfg(not(feature = "reader-ooxml"))]
fn has_encrypted_entry(_bytes: &[u8]) -> bool {
    false
}

impl Reader for SpreadsheetReader {
    fn media_types(&self) -> &'static [&'static str] {
        &[SHEET_TYPE]
    }

    fn read(&self, source: Digest256, bytes: &[u8]) -> Result<Vec<Fragment>, ReaderError> {
        self.limits.check_size(bytes)?;
        if is_compound_file(bytes) || has_encrypted_entry(bytes) {
            return Err(encrypted());
        }
        let workbook = Xlsx::new(Cursor::new(bytes)).map_err(|error| corrupt(error.to_string()))?;
        let names = workbook.sheet_names();
        let per_thread = names.len().div_ceil(self.limits.parallelism()).max(1);
        let groups: Vec<&[String]> = names.chunks(per_thread).collect();
        // One workbook per group (at most `parallelism` groups, one thread each): a
        // sheet read needs `&mut`, and the groups are independent. The first parse
        // above only supplies the sheet order.
        let per_group = map_in_order(&groups, &self.limits, |group| {
            let mut workbook: Xlsx<Cursor<&[u8]>> =
                Xlsx::new(Cursor::new(bytes)).map_err(|error| corrupt(error.to_string()))?;
            let mut units = Vec::new();
            for name in group.iter() {
                units.extend(sheet_units(&mut workbook, name)?);
            }
            Ok(units)
        })?;
        extracted_fragments(
            source,
            per_group.into_iter().flatten().collect(),
            ModalityKind::Table,
        )
    }
}
