//! The `traceparent` the caller sends: parsed, round-tripped and dropped when malformed.

use medulla_observe::trace::{trace_id_of, TraceContext};

const GOOD: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

// spec: MEDULLA-OBSERVE-R002.2
#[test]
fn traceparent_parses_and_malformed_is_dropped() {
    let context = TraceContext::parse(GOOD).expect("a W3C example parses");
    assert_eq!(context.trace_id_hex(), "4bf92f3577b34da6a3ce929d0e0e4736");
    assert_eq!(context.parent, [0x00, 0xf0, 0x67, 0xaa, 0x0b, 0xa9, 0x02, 0xb7]);
    assert_eq!(context.flags, 1);
    assert_eq!(context.to_string(), GOOD, "the text round-trips");
    assert_eq!(trace_id_of(Some(GOOD)), "4bf92f3577b34da6a3ce929d0e0e4736");
    assert_eq!(trace_id_of(None), "");

    let malformed = [
        "",
        "not-a-traceparent",
        // The wrong length, version, separators and digits.
        "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-0",
        "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        "00_4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01",
        "00-4bf92f3577b34da6a3ce929d0e0e473g-00f067aa0ba902b7-01",
        // The all-zero ids the specification marks invalid.
        "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
        "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",
    ];
    for text in malformed {
        assert_eq!(TraceContext::parse(text), None, "`{text}` must be dropped");
        // A dropped value never raises: the span shows no trace id.
        assert_eq!(trace_id_of(Some(text)), "", "`{text}`");
    }
}
