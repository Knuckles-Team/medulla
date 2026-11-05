use super::shard_writer::{send_with_deadline, DeadlineSendError};
use super::*;
use std::sync::mpsc::sync_channel;

// spec: MEDULLA-ENGINE-PARITY-R001.1
#[test]
fn enqueue_timeout_parses_and_clamps() {
    let parse = RedbGroupCommitConfig::parse_enqueue_timeout;
    assert_eq!(parse(None), Duration::from_millis(5000), "default");
    assert_eq!(parse(Some("2500")), Duration::from_millis(2500));
    assert_eq!(parse(Some(" 750 ")), Duration::from_millis(750));
    assert_eq!(parse(Some("1")), Duration::from_millis(100), "clamped up");
    assert_eq!(parse(Some("100")), Duration::from_millis(100));
    assert_eq!(parse(Some("60000")), Duration::from_millis(60_000));
    assert_eq!(
        parse(Some("9999999")),
        Duration::from_millis(60_000),
        "clamped down"
    );
    assert_eq!(
        parse(Some("soon")),
        Duration::from_millis(5000),
        "unparsable"
    );
}

// spec: MEDULLA-ENGINE-PARITY-R001.2
#[test]
fn send_with_deadline_refuses_full_and_enqueues_nothing() {
    let (tx, rx) = sync_channel::<Cmd>(1);
    let mutation = |node: &str| {
        let (done, _rx) = oneshot::channel();
        Cmd::Mutation {
            graph: "g".to_string(),
            method: Box::new(Method::AddNode {
                node_id: node.to_string(),
                properties_msgpack: Vec::new(),
            }),
            done,
        }
    };
    send_with_deadline(&tx, mutation("a"), Duration::from_millis(50)).expect("room for one");
    let refused = send_with_deadline(&tx, mutation("b"), Duration::from_millis(50));
    assert_eq!(refused, Err(DeadlineSendError::Full));
    assert!(rx.try_recv().is_ok(), "the first command stays enqueued");
    assert!(
        rx.try_recv().is_err(),
        "the refused command was not enqueued"
    );
    drop(rx);
    assert_eq!(
        send_with_deadline(&tx, mutation("c"), Duration::from_millis(50)),
        Err(DeadlineSendError::Gone)
    );
}
