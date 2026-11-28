//! Source-query approval limits (MEDULLA-FEDERATED-QUERY-R085.6): every caller
//! value is clamped to the tenant ceiling of the `knob source_query.*` table,
//! and a zero value refuses.

use medulla_result_types::source_query::{SourceQueryError, SourceQueryLimits};
use medulla_work_types::scheduler_config::{SourceQueryKnobs, SOURCE_QUERY_KNOBS};

fn asked(value: u64) -> SourceQueryLimits {
    SourceQueryLimits {
        max_rows: value,
        max_bytes: value,
        chunk_rows: value,
        chunk_bytes: value,
        queue_depth: value,
        queue_bytes: value,
        deadline_ms: value,
        max_resident_bytes: value,
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R085.6
#[test]
fn limits_clamp_to_tenant_ceiling() {
    // The defaults and bounds of the knob table.
    let defaults = SourceQueryKnobs::default();
    let ceiling = defaults.ceiling();
    assert_eq!(
        ceiling,
        SourceQueryLimits {
            max_rows: 1_000_000,
            max_bytes: 268_435_456,
            chunk_rows: 5_000,
            chunk_bytes: 4_194_304,
            queue_depth: 4,
            queue_bytes: 16_777_216,
            deadline_ms: 60_000,
            max_resident_bytes: 134_217_728,
        }
    );
    assert_eq!(SourceQueryKnobs::from_lookup(|_| None).unwrap(), defaults);
    for knob in &SOURCE_QUERY_KNOBS {
        let (lo, hi) = knob.range;
        for (value, accepted) in [(lo, true), (hi, true), (lo - 1, false), (hi + 1, false)] {
            let read =
                SourceQueryKnobs::from_lookup(|key| (key == knob.env).then(|| value.to_string()));
            assert_eq!(read.is_ok(), accepted, "{} = {value}", knob.knob);
            if !accepted {
                assert_eq!(read.unwrap_err().knob, knob.knob);
            }
        }
    }

    // A value above its ceiling is clamped to it.
    assert_eq!(
        asked(u64::MAX >> 30).clamp_to_ceiling(&ceiling),
        Ok(ceiling.clone())
    );

    // A value under its ceiling is kept as asked.
    let small = SourceQueryLimits {
        max_rows: 100,
        max_bytes: 4_096,
        chunk_rows: 10,
        chunk_bytes: 1_024,
        queue_depth: 2,
        queue_bytes: 2_048,
        deadline_ms: 5_000,
        max_resident_bytes: 8_192,
    };
    assert_eq!(small.clamp_to_ceiling(&ceiling), Ok(small.clone()));

    // Mixed: each field clamps on its own.
    let mut mixed = small.clone();
    mixed.max_rows = 50_000_000;
    mixed.deadline_ms = 7_200_000;
    let clamped = mixed.clamp_to_ceiling(&ceiling).unwrap();
    assert_eq!(clamped.max_rows, 1_000_000);
    assert_eq!(clamped.deadline_ms, 60_000);
    assert_eq!(clamped.chunk_rows, 10);

    // A zero value refuses, whichever field carries it.
    for field in 0..8 {
        let mut zero = small.clone();
        let slot = match field {
            0 => &mut zero.max_rows,
            1 => &mut zero.max_bytes,
            2 => &mut zero.chunk_rows,
            3 => &mut zero.chunk_bytes,
            4 => &mut zero.queue_depth,
            5 => &mut zero.queue_bytes,
            6 => &mut zero.deadline_ms,
            _ => &mut zero.max_resident_bytes,
        };
        *slot = 0;
        assert_eq!(
            zero.clamp_to_ceiling(&ceiling),
            Err(SourceQueryError::LimitExceeded),
            "field {field}"
        );
    }

    // A lowered ceiling from the knob table clamps the same way.
    let lowered = SourceQueryKnobs::from_lookup(|key| {
        (key == "MEDULLA_SOURCE_QUERY_MAX_ROWS").then(|| "20".to_string())
    })
    .unwrap()
    .ceiling();
    assert_eq!(small.clamp_to_ceiling(&lowered).unwrap().max_rows, 20);
}
