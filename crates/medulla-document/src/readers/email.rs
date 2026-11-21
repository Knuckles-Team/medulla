//! `message/rfc822` (MEDULLA-INGEST-WORKER-R700.10): the subject, then every text
//! body part in message order. Attachments are not read; a document inside one
//! is ingested as its own record.

use mail_parser::{MessageParser, MimeHeaders};
use medulla_modality::artifact::ModalityKind;
use medulla_types::contract::Digest256;

use super::extracted::{extracted_fragments, ExtractedUnit};
use super::{ReaderLimits, READER_CORRUPT, READER_ENCRYPTED};
use crate::fragment::{Fragment, Reader, ReaderError};

/// Reads the subject and the text bodies of one message.
pub(super) struct EmailReader {
    limits: ReaderLimits,
}

impl EmailReader {
    pub(super) fn new(limits: ReaderLimits) -> Self {
        Self { limits }
    }
}

/// The root `Content-Type` marks the body as encrypted: `multipart/encrypted`, or
/// an enveloped-data `pkcs7-mime`. A `multipart/signed` message is readable.
fn body_is_encrypted(content_type: &mail_parser::ContentType<'_>) -> bool {
    let kind = content_type.ctype();
    let subtype = content_type.subtype().unwrap_or("");
    if kind.eq_ignore_ascii_case("multipart") && subtype.eq_ignore_ascii_case("encrypted") {
        return true;
    }
    kind.eq_ignore_ascii_case("application")
        && (subtype.eq_ignore_ascii_case("pkcs7-mime")
            || subtype.eq_ignore_ascii_case("x-pkcs7-mime"))
        && content_type
            .attribute("smime-type")
            .is_some_and(|value| value.eq_ignore_ascii_case("enveloped-data"))
}

impl Reader for EmailReader {
    fn media_types(&self) -> &'static [&'static str] {
        &["message/rfc822"]
    }

    fn read(&self, source: Digest256, bytes: &[u8]) -> Result<Vec<Fragment>, ReaderError> {
        self.limits.check_size(bytes)?;
        let message = MessageParser::default()
            .parse(bytes)
            .ok_or_else(|| ReaderError {
                code: READER_CORRUPT,
                detail: "the bytes are not an RFC 5322 message".into(),
            })?;
        if message.content_type().is_some_and(body_is_encrypted) {
            return Err(ReaderError {
                code: READER_ENCRYPTED,
                detail: "message body is encrypted".into(),
            });
        }
        let mut units = Vec::new();
        if let Some(subject) = message.subject() {
            units.push(ExtractedUnit {
                text: format!("Subject: {subject}"),
                heading_path: Vec::new(),
                page: None,
            });
        }
        for index in 0..message.text_body_count() {
            if let Some(body) = message.body_text(index) {
                units.push(ExtractedUnit {
                    text: body.replace("\r\n", "\n").trim().to_string(),
                    heading_path: Vec::new(),
                    page: None,
                });
            }
        }
        extracted_fragments(source, units, ModalityKind::Document)
    }
}
