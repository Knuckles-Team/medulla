//! Message-stream records normalize into a [`ChangeEnvelope`]
//! (`MEDULLA-UNIFIED-DATA-PLANE-R710`).
//!
//! The engine consumes no broker: a connector reads the broker and submits
//! pages of [`StreamMessage`]s, and the engine maps each one with the pure
//! [`map_stream_message`]. The stream name is the table, the opaque cursor
//! `<partition>:<offset>` is the source position, every message is an
//! `Insert`, and a message without a key marks its envelope keyless.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::change_envelope::{ChangeEnvelope, ChangeOp};

/// `knob stream.max_message_bytes` default.
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 1_048_576;
/// `knob stream.page_messages` default.
pub const DEFAULT_PAGE_MESSAGES: usize = 500;

/// One record read from a message stream by a connector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamMessage {
    pub stream: String,
    pub partition: u32,
    pub offset: u64,
    pub key: Option<String>,
    pub payload: Vec<u8>,
    pub headers: BTreeMap<String, String>,
    pub timestamp_ms: i64,
}

impl StreamMessage {
    /// The opaque cursor `<partition>:<offset>`.
    pub fn cursor(&self) -> String {
        format!("{}:{}", self.partition, self.offset)
    }
}

/// How messages of one stream map: the key column name and the size cap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamMapping {
    pub stream: String,
    pub key_column: String,
    pub max_message_bytes: usize,
}

/// Why a message did not map. [`StreamRefusal::code`] is the stable reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamRefusal {
    TooLarge {
        bytes: usize,
        max: usize,
    },
    KeyInvalid,
    OffsetRegressed {
        partition: u32,
        previous: u64,
        offset: u64,
    },
    MappingMissing {
        stream: String,
    },
}

impl StreamRefusal {
    pub fn code(&self) -> &'static str {
        match self {
            Self::TooLarge { .. } => "STREAM_MESSAGE_TOO_LARGE",
            Self::KeyInvalid => "STREAM_KEY_INVALID",
            Self::OffsetRegressed { .. } => "STREAM_OFFSET_REGRESSED",
            Self::MappingMissing { .. } => "STREAM_MAPPING_MISSING",
        }
    }
}

/// Map one message. Pure: the same message and mapping give the same envelope.
pub fn map_stream_message(
    msg: &StreamMessage,
    mapping: &StreamMapping,
) -> Result<ChangeEnvelope, StreamRefusal> {
    if msg.stream != mapping.stream {
        return Err(StreamRefusal::MappingMissing {
            stream: msg.stream.clone(),
        });
    }
    if msg.payload.len() > mapping.max_message_bytes {
        return Err(StreamRefusal::TooLarge {
            bytes: msg.payload.len(),
            max: mapping.max_message_bytes,
        });
    }
    let key = match &msg.key {
        Some(key) if key.is_empty() => return Err(StreamRefusal::KeyInvalid),
        Some(key) => Some(BTreeMap::from([(mapping.key_column.clone(), key.clone())])),
        None => None,
    };
    let envelope = ChangeEnvelope {
        source_position: msg.cursor(),
        table: msg.stream.clone(),
        op: ChangeOp::Insert,
        table_is_keyless: key.is_none(),
        key,
    };
    envelope.validate().map_err(|_| StreamRefusal::KeyInvalid)?;
    Ok(envelope)
}

/// Accept a page only when each partition's offsets rise by 1 or more;
/// returns the envelopes in page order, or the first regression.
pub fn accept_page(
    page: &[StreamMessage],
    mapping: &StreamMapping,
) -> Result<Vec<Result<ChangeEnvelope, StreamRefusal>>, StreamRefusal> {
    let mut last: BTreeMap<u32, u64> = BTreeMap::new();
    for msg in page {
        if let Some(&previous) = last.get(&msg.partition) {
            if msg.offset <= previous {
                return Err(StreamRefusal::OffsetRegressed {
                    partition: msg.partition,
                    previous,
                    offset: msg.offset,
                });
            }
        }
        last.insert(msg.partition, msg.offset);
    }
    Ok(page
        .iter()
        .map(|m| map_stream_message(m, mapping))
        .collect())
}
