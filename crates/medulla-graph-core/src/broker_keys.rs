//! Broker node-id and node-type vocabulary (CONCEPT:MEDULLA-275): the stable ids and
//! labels the graph core and the broker both name, so the graph's broker claim,
//! delivery and stream paths do not depend on the broker module itself.

/// Node id for a queue's durable policy node (CONCEPT:MEDULLA-KG.compute.dead-letter-queues/277/278).
pub fn queue_policy_node_id(queue: &str) -> String {
    format!("broker:qpolicy:{queue}")
}

/// Type carried by a delivery-tag reverse-lookup node.
pub const DTAG_LOOKUP_TYPE: &str = "BrokerDeliveryTag";

/// Type carried by the two broker-wide monotonic counter nodes (confirm + dtag).
pub const BROKER_COUNTER_TYPE: &str = "BrokerCounter";

/// Type carried by a stream's durable monotonic offset counter node.
pub const STREAM_OFFSET_TYPE: &str = "BrokerStreamOffset";

/// Node id for a stream's durable monotonic offset counter (CONCEPT:MEDULLA-KG.compute.replayable-append-log).
pub fn stream_offset_node_id(stream: &str) -> String {
    format!("broker:soff:{stream}")
}

/// The label a stream's RETAINED message nodes carry (CONCEPT:MEDULLA-KG.compute.replayable-append-log). Distinct from
/// the queue label `qmsg:<queue>` so a stream is never scanned by the queue claim.
pub fn stream_msg_label(stream: &str) -> String {
    format!("smsg:{stream}")
}

/// Node id for the message appended to `stream` at `offset` (CONCEPT:MEDULLA-KG.compute.replayable-append-log).
pub fn stream_msg_node_id(stream: &str, offset: i64) -> String {
    format!("broker:smsg:{stream}:{offset}")
}

/// Node id of the broker-wide monotonic consumer delivery-tag counter (CONCEPT:MEDULLA-KG.compute.publisher-confirms-consumer-qos).
pub fn dtag_seq_node_id() -> String {
    "broker:dtag_seq".to_string()
}

/// Node id of the reverse-lookup node mapping a consumer `delivery_tag` → the claimed
/// message node id + queue, so ack/nack-by-tag resolves in O(1) (CONCEPT:MEDULLA-KG.compute.publisher-confirms-consumer-qos).
pub fn dtag_lookup_node_id(tag: i64) -> String {
    format!("broker:dtag:{tag}")
}

/// Node id for a queue's durable monotonic sequence counter.
pub fn queue_seq_node_id(queue: &str) -> String {
    format!("broker:seq:{queue}")
}

/// The `type`/label a queue's pending message nodes carry — the label
/// `claim_next_fields` scans to deliver the queue FIFO (CONCEPT:MEDULLA-KG.compute.atomically-claim-oldest-pending).
pub fn queue_msg_label(queue: &str) -> String {
    format!("qmsg:{queue}")
}

/// Node id for the `seq`-th message appended to `queue`.
pub fn message_node_id(queue: &str, seq: i64) -> String {
    format!("broker:msg:{queue}:{seq}")
}

/// Type carried by a producer's durable dedup high-water-mark node (CONCEPT:MEDULLA-KG.ingest.broker-reject-publish).
pub const PRODUCER_SEQ_TYPE: &str = "BrokerProducerSeq";
