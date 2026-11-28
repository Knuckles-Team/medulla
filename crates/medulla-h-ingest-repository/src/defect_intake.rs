//! Defect intake (MEDULLA-SDLC-R003.3): one symptom filed against the stored defects of its
//! repository. The decision is `DefectBook::file` (MEDULLA-SDLC-R003.2); this module only
//! seats the stored defects in a book and reads the touched defect back, so the served
//! `FileDefect`, the failed-gate path of `RecordGateVerdict` and the `IngestApi::file_defect`
//! seam all file through one function.

use medulla_core::lifecycle::defect::{DefectBook, FileOutcome};
use medulla_source_types::lifecycle::{DefectFileOutcome, DefectWrite, FileDefectRequest, Symptom};

/// File `request` against `existing`, the defects stored for `request.repo`. Returns the
/// touched defect with its full symptom list.
pub fn file_defect(
    existing: Vec<medulla_source_types::lifecycle::Defect>,
    request: FileDefectRequest,
) -> DefectWrite {
    let mut book = DefectBook::from_defects(existing);
    let outcome = book.file(
        &request.repo,
        request.intake,
        &request.title,
        request.severity,
        Symptom {
            intake: request.intake,
            source_record_id: request.source_record_id,
            observed_at: request.observed_at,
            text: request.text,
        },
    );
    let (id, outcome) = match outcome {
        FileOutcome::Created(id) => (id, DefectFileOutcome::Created),
        FileOutcome::Absorbed(id) => (id, DefectFileOutcome::Absorbed),
        FileOutcome::Reopened(id) => (id, DefectFileOutcome::Reopened),
    };
    let defect = book
        .defects()
        .iter()
        .find(|defect| defect.id == id)
        .cloned()
        .expect("the defect a filing touched is in the book");
    DefectWrite { defect, outcome }
}
