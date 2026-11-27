//! The caller's trace context and the request span (MEDULLA-OBSERVE-R002.2 and R002.3).
//!
//! A caller may send a W3C `traceparent` in `Request.trace_id`. [`TraceContext::parse`] reads
//! it; a malformed value is dropped (the caller is told nothing and the request is not refused:
//! the field carries no authority). [`request_span`] builds the one span that sets the request id,
//! the trace id, the operation and the graph on every log line of a request, so a line printed
//! deep in the dispatch path can be tied to the caller's trace.

use std::fmt;

/// The length of a W3C `traceparent` of version `00`.
pub const TRACEPARENT_LEN: usize = 55;

/// A parsed W3C `traceparent`: `00-<32 hex trace id>-<16 hex parent id>-<2 hex flags>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceContext {
    /// The 16-byte trace id.
    pub trace_id: [u8; 16],
    /// The 8-byte id of the caller's span.
    pub parent: [u8; 8],
    /// The trace flags; bit 0 is `sampled`.
    pub flags: u8,
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn hex_bytes<const N: usize>(text: &[u8]) -> Option<[u8; N]> {
    if text.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for (index, pair) in text.chunks_exact(2).enumerate() {
        out[index] = (hex_digit(pair[0])? << 4) | hex_digit(pair[1])?;
    }
    Some(out)
}

impl TraceContext {
    /// Parse a `traceparent` of version `00`. `None` for any other text: a wrong length or
    /// separator, an upper-case or non-hex digit, a version other than `00`, or an all-zero
    /// trace id or parent id (the W3C invalid values).
    pub fn parse(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.len() != TRACEPARENT_LEN
            || &bytes[..3] != b"00-"
            || bytes[35] != b'-'
            || bytes[52] != b'-'
        {
            return None;
        }
        let trace_id: [u8; 16] = hex_bytes(&bytes[3..35])?;
        let parent: [u8; 8] = hex_bytes(&bytes[36..52])?;
        let [flags]: [u8; 1] = hex_bytes(&bytes[53..55])?;
        if trace_id == [0; 16] || parent == [0; 8] {
            return None;
        }
        Some(Self {
            trace_id,
            parent,
            flags,
        })
    }

    /// The trace id as 32 lower-case hex digits.
    pub fn trace_id_hex(&self) -> String {
        self.trace_id
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

impl fmt::Display for TraceContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "00-{}-", self.trace_id_hex())?;
        for byte in self.parent {
            write!(f, "{byte:02x}")?;
        }
        write!(f, "-{:02x}", self.flags)
    }
}

/// The trace id a request span shows: the 32 hex digits of a parsable `traceparent`, else empty.
/// A value that does not parse is dropped with one DEBUG line and never refuses the request.
pub fn trace_id_of(trace_id: Option<&str>) -> String {
    let Some(text) = trace_id else {
        return String::new();
    };
    match TraceContext::parse(text) {
        Some(context) => context.trace_id_hex(),
        None => {
            tracing::debug!(
                target: "medulla::request",
                "a malformed traceparent was dropped"
            );
            String::new()
        }
    }
}

/// The span of one request: `request{request_id, trace_id, op, graph}` with target
/// `medulla::request`. It is the single place that sets these fields; the dispatch boundary
/// instruments the whole dispatch with it and a spawned task inherits it.
pub fn request_span(
    request_id: u64,
    trace_id: Option<&str>,
    op: &str,
    graph: &str,
) -> tracing::Span {
    tracing::info_span!(
        target: "medulla::request",
        "request",
        request_id,
        trace_id = %trace_id_of(trace_id),
        op,
        graph = %graph,
    )
}
