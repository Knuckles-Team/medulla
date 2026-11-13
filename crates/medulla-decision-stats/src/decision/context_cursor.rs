//! MEDULLA-DECISION-ENGINE-R100.100.4: the opaque cursor of a context search.
//!
//! A cursor encodes the tenant, the digest of the query, the snapshot
//! revision it was issued against and the next span offset, with a SHA-256
//! check value over that payload. A caller cannot forge or edit one, replay
//! it for another tenant or query, or resume it after the snapshot moved.

use medulla_decision_types::decision::statistical::context_search::sha256_hex;
use medulla_decision_types::decision::statistical::errors::StatisticalErrorCode;

/// The `context_cursor` family refusal table.
pub const REFUSALS: &[StatisticalErrorCode] = &[
    StatisticalErrorCode::ContextCursorInvalid,
    StatisticalErrorCode::ContextCursorStale,
];

const PREFIX: &str = "c1.";
const CHECK_DOMAIN: &[u8] = b"eg/context-cursor/v1\0";

/// What a cursor pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorState {
    pub tenant_id: String,
    /// The digest of the query the cursor pages (64 lowercase hex characters).
    pub query_digest: String,
    pub snapshot_revision: u64,
    /// Index of the first span the next page returns.
    pub next_offset: u64,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).ok())
        .collect()
}

fn payload(state: &CursorState) -> String {
    format!(
        "{}\n{}\n{}\n{}",
        state.snapshot_revision, state.next_offset, state.query_digest, state.tenant_id
    )
}

fn check(payload: &[u8]) -> String {
    sha256_hex(&[CHECK_DOMAIN, payload])
}

/// The opaque cursor for `state`.
pub fn encode(state: &CursorState) -> String {
    let payload = payload(state);
    format!(
        "{PREFIX}{}.{}",
        hex(payload.as_bytes()),
        check(payload.as_bytes())
    )
}

fn invalid(detail: &str) -> String {
    StatisticalErrorCode::ContextCursorInvalid.refusal(detail)
}

/// The next offset of `cursor`, when it was issued to `tenant_id` for the
/// query with `query_digest` against `current_revision`. A cursor of another
/// tenant or query, or one that fails its check value, is invalid; one from an
/// older or newer snapshot revision is stale.
pub fn decode(
    cursor: &str,
    tenant_id: &str,
    query_digest: &str,
    current_revision: u64,
) -> Result<u64, String> {
    let body = cursor
        .strip_prefix(PREFIX)
        .ok_or_else(|| invalid("not a context cursor"))?;
    let (encoded, stated_check) = body
        .split_once('.')
        .ok_or_else(|| invalid("a cursor has two parts"))?;
    let bytes = unhex(encoded).ok_or_else(|| invalid("the payload is not hex"))?;
    if check(&bytes) != stated_check {
        return Err(invalid("the check value does not match"));
    }
    let text = String::from_utf8(bytes).map_err(|_| invalid("the payload is not text"))?;
    let mut parts = text.splitn(4, '\n');
    let (Some(revision), Some(offset), Some(digest), Some(tenant)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(invalid("the payload is incomplete"));
    };
    let revision: u64 = revision.parse().map_err(|_| invalid("bad revision"))?;
    let offset: u64 = offset.parse().map_err(|_| invalid("bad offset"))?;
    if tenant != tenant_id || digest != query_digest {
        return Err(invalid("the cursor was issued for another tenant or query"));
    }
    if revision != current_revision {
        return Err(StatisticalErrorCode::ContextCursorStale.refusal(format!(
            "issued at revision {revision}, snapshot is at {current_revision}"
        )));
    }
    Ok(offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> CursorState {
        CursorState {
            tenant_id: "tenant-a".into(),
            query_digest: "ab".repeat(32),
            snapshot_revision: 12,
            next_offset: 40,
        }
    }

    fn code(error: String) -> String {
        error.split(':').next().unwrap().to_string()
    }

    #[test]
    fn a_cursor_round_trips_and_refuses_forgery_replay_and_staleness() {
        let state = state();
        let cursor = encode(&state);
        assert_eq!(decode(&cursor, "tenant-a", &state.query_digest, 12), Ok(40));

        // Another tenant, another query: invalid, not stale.
        assert_eq!(
            code(decode(&cursor, "tenant-b", &state.query_digest, 12).unwrap_err()),
            "CONTEXT_CURSOR_INVALID"
        );
        assert_eq!(
            code(decode(&cursor, "tenant-a", &"cd".repeat(32), 12).unwrap_err()),
            "CONTEXT_CURSOR_INVALID"
        );
        // A moved snapshot: stale, in either direction.
        for revision in [11, 13] {
            assert_eq!(
                code(decode(&cursor, "tenant-a", &state.query_digest, revision).unwrap_err()),
                "CONTEXT_CURSOR_STALE"
            );
        }
        // An edited payload (a larger offset under the old check value), a
        // wrong check value, a missing prefix and noise are all invalid.
        let edited = encode(&CursorState {
            next_offset: 41,
            ..state.clone()
        });
        let (_, old_check) = cursor.rsplit_once('.').unwrap();
        let (edited_body, _) = edited.rsplit_once('.').unwrap();
        for bad in [
            format!("{edited_body}.{old_check}"),
            format!("{}0", cursor),
            cursor.replacen("c1.", "c2.", 1),
            "c1.zz.00".to_string(),
            "c1.".to_string(),
            String::new(),
        ] {
            assert_eq!(
                code(decode(&bad, "tenant-a", &state.query_digest, 12).unwrap_err()),
                "CONTEXT_CURSOR_INVALID",
                "{bad}"
            );
        }
        assert_eq!(REFUSALS.len(), 2);
    }
}
