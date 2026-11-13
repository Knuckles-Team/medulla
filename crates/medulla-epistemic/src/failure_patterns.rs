//! Failure-pattern kernels (MEDULLA-DECISION-ENGINE-R701.2): a normalised
//! signature per failed run, one-pass clustering by signature, and the
//! minimal step reduction a pattern keeps. Pure and deterministic: no I/O, no
//! clock; every limit arrives as a parameter read from the knob registry
//! (`failure.context_steps`, `failure.min_count`).

use std::collections::HashMap;

use medulla_types::contract::BoundedVec;
use medulla_types::failure_patterns::{
    FailurePattern, FailureRecord, Signature, MAX_FAILURE_STEPS, MAX_PATTERN_MEMBERS,
};

/// What clustering is bounded by; both figures are registry knobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClusterParams {
    /// Steps kept before the failing step (`failure.context_steps`).
    pub context_steps: usize,
    /// Records a pattern needs to count as recurring (`failure.min_count`).
    pub min_count: u64,
}

const UUID_SHAPE: [usize; 5] = [8, 4, 4, 4, 12];
const HEX_RUN: usize = 8;

fn is_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

/// Length of a UUID (8-4-4-4-12 lowercase hex) starting at `at`, else `None`.
fn uuid_len(bytes: &[u8], at: usize) -> Option<usize> {
    let mut pos = at;
    for (index, width) in UUID_SHAPE.iter().enumerate() {
        if index > 0 {
            if bytes.get(pos) != Some(&b'-') {
                return None;
            }
            pos += 1;
        }
        let group = bytes.get(pos..pos + width)?;
        if !group.iter().all(|b| is_hex(*b)) {
            return None;
        }
        pos += width;
    }
    Some(pos - at)
}

/// One part of a signature: lowercased; UUIDs and hex runs of 8 or more
/// characters become `<id>`; remaining digit runs become `#`; whitespace
/// collapses to one space.
fn normalise(part: &str) -> String {
    let lowered = part.to_lowercase();
    let collapsed = lowered.split_whitespace().collect::<Vec<_>>().join(" ");
    let bytes = collapsed.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if let Some(length) = uuid_len(bytes, at) {
            out.push_str("<id>");
            at += length;
            continue;
        }
        if is_hex(bytes[at]) {
            let run = bytes[at..].iter().take_while(|b| is_hex(**b)).count();
            if run >= HEX_RUN {
                out.push_str("<id>");
            } else {
                push_plain(&mut out, &bytes[at..at + run]);
            }
            at += run;
            continue;
        }
        push_plain(&mut out, &bytes[at..=at]);
        at += 1;
    }
    out
}

/// Append bytes of a normalised part, folding each digit run to one `#`.
fn push_plain(out: &mut String, bytes: &[u8]) {
    let mut in_digits = out.ends_with('#');
    for byte in bytes {
        if byte.is_ascii_digit() {
            if !in_digits {
                out.push('#');
            }
            in_digits = true;
        } else {
            out.push(char::from(*byte));
            in_digits = false;
        }
    }
}

/// The signature of a failure: `error_class|tool|step_kind`, each part
/// normalised. The same failure always yields the same signature.
pub fn signature_of(error_class: &str, tool: &str, step_kind: &str) -> Signature {
    Signature(format!(
        "{}|{}|{}",
        normalise(error_class),
        normalise(tool),
        normalise(step_kind)
    ))
}

/// The failing step and the `context_steps` steps before it, in order.
pub fn reduce_steps(record: &FailureRecord, context_steps: usize) -> Vec<String> {
    let steps = record.steps.as_slice();
    let Some(last) = steps.len().checked_sub(1) else {
        return Vec::new();
    };
    let failing = (record.failing_index as usize).min(last);
    steps[failing.saturating_sub(context_steps)..=failing].to_vec()
}

struct Group<'a> {
    records: Vec<&'a FailureRecord>,
}

/// Cluster records by signature in one pass over a map from signature to
/// records: time is O(n), memory O(distinct signatures + n ids). Patterns come
/// back ordered by signature. `members` keeps the first
/// [`MAX_PATTERN_MEMBERS`] ids in input order while `count` is the true
/// number; `minimal_steps` is the reduction of the earliest failure.
pub fn cluster(records: &[FailureRecord], params: &ClusterParams) -> Vec<FailurePattern> {
    let mut groups: HashMap<Signature, Group<'_>> = HashMap::new();
    for record in records {
        groups
            .entry(signature_of(
                &record.error_class,
                &record.tool,
                &record.step_kind,
            ))
            .or_insert_with(|| Group {
                records: Vec::new(),
            })
            .records
            .push(record);
    }
    let mut patterns: Vec<FailurePattern> = groups
        .into_iter()
        .filter_map(|(signature, group)| pattern_of(signature, &group, params))
        .collect();
    patterns.sort_by(|a, b| a.signature.cmp(&b.signature));
    patterns
}

fn pattern_of(
    signature: Signature,
    group: &Group<'_>,
    params: &ClusterParams,
) -> Option<FailurePattern> {
    let earliest = group
        .records
        .iter()
        .min_by(|a, b| (a.at_ms, &a.record_id).cmp(&(b.at_ms, &b.record_id)))?;
    let latest = group.records.iter().map(|r| r.at_ms).max()?;
    let members: Vec<String> = group
        .records
        .iter()
        .take(MAX_PATTERN_MEMBERS)
        .map(|r| r.record_id.clone())
        .collect();
    let mut minimal = reduce_steps(earliest, params.context_steps);
    minimal.truncate(MAX_FAILURE_STEPS);
    Some(FailurePattern {
        signature,
        members: BoundedVec::new(members).ok()?,
        first_seen: earliest.at_ms,
        last_seen: latest,
        count: group.records.len() as u64,
        minimal_steps: BoundedVec::new(minimal).ok()?,
    })
}

/// Whether a pattern has recurred enough to file a gap.
pub fn is_recurring(pattern: &FailurePattern, params: &ClusterParams) -> bool {
    pattern.count >= params.min_count
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn record(id: usize, class: &str, tool: &str, at_ms: u64) -> FailureRecord {
        FailureRecord {
            record_id: format!("r{id}"),
            run_trace_id: format!("rt{id}"),
            error_class: class.to_string(),
            tool: tool.to_string(),
            step_kind: "tool_call".to_string(),
            steps: BoundedVec::new(
                ["plan", "read", "search", "answer"]
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            )
            .unwrap(),
            failing_index: 2,
            at_ms,
        }
    }

    const PARAMS: ClusterParams = ClusterParams {
        context_steps: 2,
        min_count: 3,
    };

    fn fixture(n: usize) -> Vec<FailureRecord> {
        (0..n)
            .map(|i| match i % 3 {
                0 => record(i, "Timeout after 30 s", "search", i as u64),
                1 => record(
                    i,
                    &format!(
                        "HTTP {} at 7c9e6679-7425-40de-944b-e07fc1f90ae7",
                        500 + i % 7
                    ),
                    "fetch",
                    i as u64,
                ),
                _ => record(i, &format!("bad id deadbeef{i:08x}"), "write", i as u64),
            })
            .collect()
    }

    // spec: MEDULLA-DECISION-ENGINE-R701.2
    #[test]
    fn cluster_is_linear_and_signatures_normalize() {
        assert_eq!(
            signature_of("  Timeout   After 30 s ", "Search", "Tool_Call").0,
            "timeout after # s|search|tool_call"
        );
        assert_eq!(
            signature_of(
                "HTTP 503 at 7C9E6679-7425-40DE-944B-E07FC1F90AE7",
                "fetch",
                "step 12"
            )
            .0,
            "http # at <id>|fetch|step #"
        );
        assert_eq!(
            signature_of("bad id deadbeef00000001", "t", "k").0,
            "bad id <id>|t|k",
            "a hex run of 8 or more characters is one id"
        );
        assert_eq!(
            signature_of("cafe 1234abc", "t", "k").0,
            "cafe #abc|t|k",
            "a hex run under 8 stays; its digits fold"
        );

        // Fixtures of 10, 1000 and 100000 items cluster into three patterns
        // whose counts sum to the input, and the whole pass stays fast.
        for n in [10_usize, 1_000, 100_000] {
            let records = fixture(n);
            let started = Instant::now();
            let patterns = cluster(&records, &PARAMS);
            let elapsed = started.elapsed();
            assert_eq!(patterns.len(), 3, "n = {n}");
            assert_eq!(patterns.iter().map(|p| p.count).sum::<u64>(), n as u64);
            assert!(
                elapsed.as_secs_f64() < 5.0,
                "clustering {n} items took {elapsed:?}"
            );
            let again = cluster(&records, &PARAMS);
            assert_eq!(patterns, again, "clustering is deterministic");
        }
        let big = cluster(&fixture(100_000), &PARAMS);
        assert!(big.iter().all(|p| p.members.len() <= MAX_PATTERN_MEMBERS));
        assert!(big.iter().all(|p| is_recurring(p, &PARAMS)));
        let small = cluster(&fixture(4), &PARAMS);
        assert!(small.iter().any(|p| !is_recurring(p, &PARAMS)));
        let timeouts = small
            .iter()
            .find(|p| p.signature.0.starts_with("timeout"))
            .unwrap();
        assert_eq!((timeouts.first_seen, timeouts.last_seen), (0, 3));
    }

    // spec: MEDULLA-DECISION-ENGINE-R701.2
    #[test]
    fn reduce_steps_keeps_the_failing_step_and_its_context() {
        let r = record(1, "e", "t", 0);
        assert_eq!(reduce_steps(&r, 2), ["plan", "read", "search"]);
        assert_eq!(reduce_steps(&r, 0), ["search"]);
        assert_eq!(reduce_steps(&r, 8), ["plan", "read", "search"]);
        let mut first = record(2, "e", "t", 0);
        first.failing_index = 0;
        assert_eq!(reduce_steps(&first, 2), ["plan"]);
        first.steps = BoundedVec::new(Vec::new()).unwrap();
        assert!(reduce_steps(&first, 2).is_empty());
    }
}
