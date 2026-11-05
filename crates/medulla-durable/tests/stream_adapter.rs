use std::collections::BTreeMap;

use medulla_durable::{
    accept_page, map_stream_message, ChangeOp, StreamMapping, StreamMessage, StreamRefusal,
};

fn mapping() -> StreamMapping {
    StreamMapping {
        stream: "orders".into(),
        key_column: "id".into(),
        max_message_bytes: 1024,
    }
}

fn msg(partition: u32, offset: u64, key: Option<&str>) -> StreamMessage {
    StreamMessage {
        stream: "orders".into(),
        partition,
        offset,
        key: key.map(String::from),
        payload: br#"{"total":3}"#.to_vec(),
        headers: BTreeMap::from([("trace".to_string(), "t-1".to_string())]),
        timestamp_ms: 1_761_000_000_000,
    }
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R710
#[test]
fn mapping_is_pure_and_cursor_is_stable() {
    let page = vec![
        msg(0, 4, Some("o-1")),
        msg(1, 9, None),
        msg(0, 5, Some("o-2")),
    ];
    let first = accept_page(&page, &mapping()).unwrap();
    let second = accept_page(&page, &mapping()).unwrap();
    assert_eq!(first, second);
    let envelopes: Vec<_> = first.into_iter().map(Result::unwrap).collect();
    assert_eq!(envelopes[0].source_position, "0:4");
    assert_eq!(envelopes[0].table, "orders");
    assert_eq!(envelopes[0].op, ChangeOp::Insert);
    assert_eq!(
        envelopes[0].key,
        Some(BTreeMap::from([("id".to_string(), "o-1".to_string())]))
    );
    assert!(!envelopes[0].table_is_keyless);
    assert!(envelopes[1].table_is_keyless && envelopes[1].key.is_none());
    assert_eq!(envelopes[1].source_position, "1:9");
    assert!(envelopes.iter().all(|e| e.validate().is_ok()));

    let mut big = msg(0, 6, Some("o-3"));
    big.payload = vec![0; 1025];
    assert_eq!(
        map_stream_message(&big, &mapping()).unwrap_err().code(),
        "STREAM_MESSAGE_TOO_LARGE"
    );
    assert_eq!(
        map_stream_message(&msg(0, 6, Some("")), &mapping())
            .unwrap_err()
            .code(),
        "STREAM_KEY_INVALID"
    );
    let mut other = msg(0, 6, None);
    other.stream = "payments".into();
    assert_eq!(
        map_stream_message(&other, &mapping()).unwrap_err().code(),
        "STREAM_MAPPING_MISSING"
    );
    let regressed = vec![msg(0, 5, None), msg(0, 5, None)];
    assert_eq!(
        accept_page(&regressed, &mapping()).unwrap_err(),
        StreamRefusal::OffsetRegressed {
            partition: 0,
            previous: 5,
            offset: 5
        }
    );
}

/// Property: over generated pages, an accepted page never holds a
/// per-partition offset regression, and a page with one is refused.
// spec: MEDULLA-UNIFIED-DATA-PLANE-R710
#[test]
fn offsets_never_regress_inside_an_accepted_page() {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..2_000 {
        let len = (next() % 12) as usize;
        let page: Vec<StreamMessage> = (0..len)
            .map(|_| msg((next() % 3) as u32, next() % 20, None))
            .collect();
        let mut last: BTreeMap<u32, u64> = BTreeMap::new();
        let rising = page.iter().all(|m| {
            let ok = last.get(&m.partition).is_none_or(|&p| m.offset > p);
            last.insert(m.partition, m.offset);
            ok
        });
        match accept_page(&page, &mapping()) {
            Ok(envelopes) => {
                assert!(rising);
                assert_eq!(envelopes.len(), page.len());
            }
            Err(refusal) => {
                assert!(!rising);
                assert_eq!(refusal.code(), "STREAM_OFFSET_REGRESSED");
            }
        }
    }
}
