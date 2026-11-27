//! The copy route of a fragment (MEDULLA-UNIFIED-DATA-PLANE-R038.2.2): a stored replica answers
//! only while the source's acceleration policy resolves to `Copy`, and only within the copy row
//! bound. Everything else runs live.

use {
    medulla_catalog_source_types::metadata_acceleration::copy_admitted,
    medulla_catalog_source_types::metadata_acceleration::AccelerationPolicy,
    medulla_catalog_source_types::metadata_acceleration::FragmentExecutionMode,
};

/// The knob values the route reads, resolved by the served read
/// (`acceleration.max_copy_rows`, `acceleration.watermark_ttl_ms`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopyKnobs {
    pub max_copy_rows: u64,
    pub watermark_ttl_ms: u64,
}

impl Default for CopyKnobs {
    fn default() -> Self {
        Self {
            max_copy_rows: 1_000_000,
            watermark_ttl_ms: 300_000,
        }
    }
}

/// Why a stored replica does not answer the fragment.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum LiveReason {
    /// The policy resolved `Live` (no watermark, an unparsable one, an expired one, no predicate
    /// or no copy capability).
    Policy,
    /// `COPY_ROW_CAP_EXCEEDED` or `COPY_CAPABILITY_MISSING`: recorded in the trace, never
    /// returned to the reader.
    Refused(String),
}

/// Whether a replica of `replica_rows` rows may answer a fragment of a source registered under
/// `policy` at `now_ms`. `valid_through_ms` is the policy's watermark plus the watermark ttl; a
/// watermark that is not RFC 3339 gives `Live`.
pub(super) fn copy_route(
    policy: &AccelerationPolicy,
    now_ms: u64,
    replica_rows: usize,
    knobs: CopyKnobs,
) -> Result<(), LiveReason> {
    let Some(watermark_ms) = policy
        .freshness_watermark
        .as_deref()
        .and_then(parse_rfc3339_ms)
    else {
        return Err(LiveReason::Policy);
    };
    let valid_through_ms = watermark_ms.saturating_add(knobs.watermark_ttl_ms);
    if policy.resolve_mode(now_ms, valid_through_ms) != FragmentExecutionMode::Copy {
        return Err(LiveReason::Policy);
    }
    copy_admitted(
        policy,
        u64::try_from(replica_rows).unwrap_or(u64::MAX),
        knobs.max_copy_rows,
    )
    .map_err(|refusal| LiveReason::Refused(refusal.to_string()))
}

/// Unix milliseconds of an RFC 3339 timestamp (`YYYY-MM-DDTHH:MM:SS[.fff](Z|+HH:MM|-HH:MM)`);
/// `None` for anything else, or a time before 1970.
pub(super) fn parse_rfc3339_ms(text: &str) -> Option<u64> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 || !text.is_ascii() {
        return None;
    }
    let num = |range: std::ops::Range<usize>| -> Option<i64> { text.get(range)?.parse().ok() };
    let sep = |at: usize, want: &[u8]| want.contains(&bytes[at]);
    if !(sep(4, b"-") && sep(7, b"-") && sep(10, b"Tt ") && sep(13, b":") && sep(16, b":")) {
        return None;
    }
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut at = 19;
    let mut millis = 0_i64;
    if bytes[at] == b'.' {
        let start = at + 1;
        at = start;
        while at < bytes.len() && bytes[at].is_ascii_digit() {
            at += 1;
        }
        if at == start {
            return None;
        }
        let digits: String = text[start..at]
            .chars()
            .chain("000".chars())
            .take(3)
            .collect();
        millis = digits.parse().ok()?;
    }
    let offset_minutes = match bytes.get(at)? {
        b'Z' | b'z' if at + 1 == bytes.len() => 0,
        sign @ (b'+' | b'-') if bytes.len() == at + 6 && bytes[at + 3] == b':' => {
            let minutes = num(at + 1..at + 3)? * 60 + num(at + 4..at + 6)?;
            if *sign == b'+' {
                minutes
            } else {
                -minutes
            }
        }
        _ => return None,
    };
    // Days from civil (proleptic Gregorian), after Howard Hinnant's algorithm.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_minutes * 60;
    u64::try_from(seconds * 1_000 + millis).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(watermark: Option<&str>) -> AccelerationPolicy {
        AccelerationPolicy {
            source_supports_copy: true,
            hot_subset_predicate: "updated_at > now() - interval '1 day'".into(),
            freshness_watermark: watermark.map(str::to_string),
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R038.2.2
    #[test]
    fn rfc3339_parses_to_unix_milliseconds() {
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:01Z"), Some(1_000));
        assert_eq!(
            parse_rfc3339_ms("2026-01-01T00:00:00.250Z"),
            Some(1_767_225_600_250)
        );
        assert_eq!(
            parse_rfc3339_ms("2026-01-01T01:00:00+01:00"),
            Some(1_767_225_600_000)
        );
        assert_eq!(parse_rfc3339_ms("lsn:42"), None);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R038.2.2
    #[test]
    fn route_is_copy_only_inside_the_ttl_and_the_row_cap() {
        let knobs = CopyKnobs {
            max_copy_rows: 10,
            watermark_ttl_ms: 5_000,
        };
        let p = policy(Some("1970-01-01T00:00:01Z"));
        assert_eq!(copy_route(&p, 6_000, 10, knobs), Ok(()));
        assert_eq!(copy_route(&p, 6_001, 10, knobs), Err(LiveReason::Policy));
        let Err(LiveReason::Refused(text)) = copy_route(&p, 6_000, 11, knobs) else {
            panic!("a replica over the cap is refused");
        };
        assert!(text.starts_with("COPY_ROW_CAP_EXCEEDED"), "{text}");
        assert_eq!(
            copy_route(&policy(Some("not a time")), 0, 1, knobs),
            Err(LiveReason::Policy)
        );
        assert_eq!(
            copy_route(&policy(None), 0, 1, knobs),
            Err(LiveReason::Policy)
        );
    }
}
