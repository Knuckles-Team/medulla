//! Format readers: PDF, Office Open XML, spreadsheet and email
//! (MEDULLA-INGEST-WORKER-R700.7 to R700.10).

use std::path::PathBuf;

use medulla_document::fragment::Fragment;
use medulla_document::readers::{
    text_registry, ReaderLimits, READER_CORRUPT, READER_ENCRYPTED, READER_TOO_LARGE,
};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/readers")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn limits() -> ReaderLimits {
    ReaderLimits::new(64 << 20, 4).expect("limits")
}

fn read(media_type: &str, bytes: &[u8]) -> Result<Vec<Fragment>, &'static str> {
    text_registry(limits())
        .expect("registry")
        .read(media_type, bytes)
        .map_err(|error| error.code)
}

/// `(text, start_byte, end_byte, page)` of each fragment.
fn shape(fragments: &[Fragment]) -> Vec<(&str, usize, usize, Option<u32>)> {
    fragments
        .iter()
        .map(|f| (f.text.as_str(), f.span.start_byte, f.span.end_byte, f.page))
        .collect()
}

#[cfg(feature = "reader-pdf")]
mod pdf {
    use super::*;
    const PDF: &str = "application/pdf";

    // spec: MEDULLA-INGEST-WORKER-R700.7
    #[test]
    fn pdf_pages_become_paged_fragments() {
        let fragments = read(PDF, &fixture("sample.pdf")).expect("read");
        assert_eq!(
            shape(&fragments),
            vec![
                ("Quarterly report", 0, 16, Some(1)),
                ("Appendix table", 17, 31, Some(2))
            ]
        );
        assert_eq!(fragments[1].span.start_char, 17);
        assert_eq!(fragments[1].span.end_char, 31);
        assert_eq!(fragments[1].span.ordinal, 1);
    }

    // spec: MEDULLA-INGEST-WORKER-R700.7
    #[test]
    fn pdf_truncated_to_100_bytes_is_corrupt() {
        let bytes = fixture("sample.pdf");
        assert_eq!(read(PDF, &bytes[..100]).unwrap_err(), READER_CORRUPT);
    }

    // spec: MEDULLA-INGEST-WORKER-R700.7
    #[test]
    fn encrypted_pdf_is_refused() {
        assert_eq!(
            read(PDF, &fixture("encrypted.pdf")).unwrap_err(),
            READER_ENCRYPTED
        );
    }

    // spec: MEDULLA-INGEST-WORKER-R700.7
    #[test]
    fn pdf_over_the_limit_is_too_large() {
        let registry = text_registry(ReaderLimits::new(100, 1).expect("limits")).expect("registry");
        let error = registry.read(PDF, &fixture("sample.pdf")).unwrap_err();
        assert_eq!(error.code, READER_TOO_LARGE);
    }
}

#[cfg(feature = "reader-ooxml")]
mod ooxml {
    use super::*;
    use std::io::Write;

    const DOCX: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
    const PPTX: &str = "application/vnd.openxmlformats-officedocument.presentationml.presentation";

    // spec: MEDULLA-INGEST-WORKER-R700.8
    #[test]
    fn docx_paragraphs_become_fragments() {
        let fragments = read(DOCX, &fixture("sample.docx")).expect("read");
        assert_eq!(
            shape(&fragments),
            vec![
                ("Engine readers", 0, 14, None),
                ("Every fragment carries a span.", 15, 45, None),
                ("Fish & chips", 46, 58, None)
            ]
        );
    }

    // spec: MEDULLA-INGEST-WORKER-R700.8
    #[test]
    fn pptx_slides_become_paged_fragments() {
        let fragments = read(PPTX, &fixture("sample.pptx")).expect("read");
        assert_eq!(
            shape(&fragments),
            vec![
                ("Roadmap", 0, 7, Some(1)),
                ("Readers ship in Q4", 8, 26, Some(1)),
                ("Risks", 27, 32, Some(2))
            ]
        );
    }

    // spec: MEDULLA-INGEST-WORKER-R700.8
    #[test]
    fn encrypted_docx_is_refused() {
        let bytes = fixture("encrypted.docx");
        assert_eq!(read(DOCX, &bytes).unwrap_err(), READER_ENCRYPTED);
        assert_eq!(read(PPTX, &bytes).unwrap_err(), READER_ENCRYPTED);
    }

    // spec: MEDULLA-INGEST-WORKER-R700.8
    #[test]
    fn ooxml_not_a_zip_is_corrupt() {
        assert_eq!(read(DOCX, b"not a zip").unwrap_err(), READER_CORRUPT);
        assert_eq!(read(PPTX, b"not a zip").unwrap_err(), READER_CORRUPT);
    }

    // spec: MEDULLA-INGEST-WORKER-R700.8
    #[test]
    fn ooxml_part_bomb_is_too_large() {
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            writer
                .start_file(
                    "word/document.xml",
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .expect("start");
            let paragraph = "<w:p><w:r><w:t>aaaaaaaaaaaaaaaa</w:t></w:r></w:p>";
            let body = format!(
                "<w:document><w:body>{}</w:body></w:document>",
                paragraph.repeat(200)
            );
            writer.write_all(body.as_bytes()).expect("write");
            writer.finish().expect("finish");
        }
        let bytes = buffer.into_inner();
        assert!(bytes.len() < 1024, "the archive stays under the limit");
        let registry =
            text_registry(ReaderLimits::new(1024, 1).expect("limits")).expect("registry");
        let error = registry.read(DOCX, &bytes).unwrap_err();
        assert_eq!(error.code, READER_TOO_LARGE);
    }
}

#[cfg(feature = "reader-spreadsheet")]
mod spreadsheet {
    use super::*;
    const XLSX: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

    // spec: MEDULLA-INGEST-WORKER-R700.9
    #[test]
    fn xlsx_rows_become_cell_range_fragments() {
        let fragments = read(XLSX, &fixture("sample.xlsx")).expect("read");
        assert_eq!(
            shape(&fragments),
            vec![
                ("region\tamount", 0, 13, None),
                ("north\t120", 14, 23, None),
                ("south\t75.5", 24, 34, None),
                ("checked", 35, 42, None)
            ]
        );
        let paths: Vec<&[String]> = fragments
            .iter()
            .map(|f| f.span.heading_path.as_slice())
            .collect();
        assert_eq!(paths[0], ["Revenue".to_string(), "A1:B1".to_string()]);
        assert_eq!(paths[2], ["Revenue".to_string(), "A3:B3".to_string()]);
        assert_eq!(paths[3], ["Notes".to_string(), "A1:A1".to_string()]);
    }

    // spec: MEDULLA-INGEST-WORKER-R700.9
    #[test]
    fn encrypted_xlsx_is_refused() {
        assert_eq!(
            read(XLSX, &fixture("encrypted.xlsx")).unwrap_err(),
            READER_ENCRYPTED
        );
    }

    // spec: MEDULLA-INGEST-WORKER-R700.9
    #[test]
    fn xlsx_not_a_zip_is_corrupt() {
        assert_eq!(read(XLSX, b"not a zip").unwrap_err(), READER_CORRUPT);
    }
}

#[cfg(feature = "reader-email")]
mod email {
    use super::*;
    const EML: &str = "message/rfc822";

    // spec: MEDULLA-INGEST-WORKER-R700.10
    #[test]
    fn eml_subject_and_body_become_fragments() {
        let fragments = read(EML, &fixture("sample.eml")).expect("read");
        assert_eq!(
            shape(&fragments),
            vec![
                ("Subject: Reader fixture", 0, 23, None),
                (
                    "The readers ship together.\nEvery fragment keeps its span.",
                    24,
                    81,
                    None
                )
            ]
        );
    }

    // spec: MEDULLA-INGEST-WORKER-R700.10
    #[test]
    fn encrypted_eml_is_refused() {
        assert_eq!(
            read(EML, &fixture("encrypted.eml")).unwrap_err(),
            READER_ENCRYPTED
        );
    }

    // spec: MEDULLA-INGEST-WORKER-R700.10
    #[test]
    fn eml_without_headers_or_body_is_corrupt() {
        assert_eq!(read(EML, b"").unwrap_err(), READER_CORRUPT);
    }
}
