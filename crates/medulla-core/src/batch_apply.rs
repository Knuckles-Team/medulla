//! The one application of a served `Batch` (MEDULLA-DURABLE-KERNEL-R871).
//!
//! The served handler, durable replay, the Raft state machine and the embedded engine all
//! run [`apply_batch_operations`], so a committed batch produces the same graph state on
//! every path. [`validate_batch`] is the check that runs before the first operation does.

use medulla_graph_core::graph::GraphCore;
use medulla_types::wire::{BatchClaimedNode, BatchOpResult, BatchOperation};

/// Why a batch is refused before any operation runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BatchRefusal {
    /// No operations.
    Empty,
    /// More than the registry knob `batch.max_operations` operations.
    TooLarge { count: usize, max: usize },
    /// An operation carries a blob that is not a canonical property object.
    Malformed { index: usize, reason: &'static str },
}

impl BatchRefusal {
    /// The registered wire code, then the detail.
    pub fn message(&self) -> String {
        match self {
            Self::Empty => "INVALID_ARGUMENT: a batch needs at least one operation".to_string(),
            Self::TooLarge { count, max } => {
                format!("BATCH_TOO_LARGE: a batch carries at most {max} operations, got {count}")
            }
            Self::Malformed { index, reason } => {
                format!("INVALID_ARGUMENT: batch operation {index}: {reason}")
            }
        }
    }
}

fn is_property_object(blob: &[u8]) -> bool {
    medulla_types::msgpack::decode_property_object(blob).is_ok()
}

/// Check the batch as a whole: its size against `max_operations` (the resolved registry knob
/// `batch.max_operations`, passed by the caller that owns the knobs), then every operation's
/// blobs. Nothing is written.
pub fn validate_batch(
    operations: &[BatchOperation],
    max_operations: usize,
) -> Result<(), BatchRefusal> {
    if operations.is_empty() {
        return Err(BatchRefusal::Empty);
    }
    if operations.len() > max_operations {
        return Err(BatchRefusal::TooLarge {
            count: operations.len(),
            max: max_operations,
        });
    }
    for (index, operation) in operations.iter().enumerate() {
        let bad = match operation {
            BatchOperation::CompareAndSetNodeFields {
                conditions_msgpack,
                updates_msgpack,
                ..
            } => {
                if !is_property_object(conditions_msgpack) {
                    Some("conditions_msgpack is not a property object")
                } else if !is_property_object(updates_msgpack) {
                    Some("updates_msgpack is not a property object")
                } else {
                    None
                }
            }
            BatchOperation::ClaimNext {
                updates_msgpack, ..
            } if !is_property_object(updates_msgpack) => {
                Some("updates_msgpack is not a property object")
            }
            _ => None,
        };
        if let Some(reason) = bad {
            return Err(BatchRefusal::Malformed { index, reason });
        }
    }
    Ok(())
}

/// A read inside the batch hit a durable-tier fault. Nothing the batch wrote is committed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchApplyError {
    pub index: usize,
    pub reason: String,
}

impl BatchApplyError {
    pub fn message(&self) -> String {
        format!("batch operation {}: {}", self.index, self.reason)
    }
}

fn property_object(blob: &[u8]) -> serde_json::Map<String, serde_json::Value> {
    // `validate_batch` ran first on every live path; a replayed record was validated when
    // it was first served, so a blob that fails here applies as an empty object.
    medulla_types::msgpack::decode_property_object(blob).unwrap_or_default()
}

/// Run the operations in order over `core`. A read sees the writes before it.
pub fn apply_batch_operations(
    core: &GraphCore,
    operations: &[BatchOperation],
) -> Result<Vec<BatchOpResult>, BatchApplyError> {
    let mut results = Vec::with_capacity(operations.len());
    for (index, operation) in operations.iter().enumerate() {
        let result = match operation {
            BatchOperation::CreateNodeIfAbsent {
                node_id,
                properties_msgpack,
            } => BatchOpResult::CreateNodeIfAbsent {
                created: core.create_node_if_absent(node_id.clone(), properties_msgpack.clone()),
            },
            BatchOperation::CompareAndSetNodeFields {
                node_id,
                conditions_msgpack,
                updates_msgpack,
            } => BatchOpResult::CompareAndSetNodeFields {
                applied: core.compare_and_set_fields(
                    node_id,
                    &property_object(conditions_msgpack),
                    &property_object(updates_msgpack),
                ),
            },
            BatchOperation::ClaimNext {
                label,
                updates_msgpack,
            } => BatchOpResult::ClaimNext {
                claimed: core
                    .claim_next_fields(label, &property_object(updates_msgpack))
                    .map(|(node_id, properties)| BatchClaimedNode {
                        node_id,
                        properties,
                    }),
            },
            BatchOperation::Reinforce {
                node_id,
                now_ms,
                weight,
            } => BatchOpResult::Reinforce {
                existed: core.reinforce(node_id, *now_ms, *weight),
            },
            BatchOperation::GetNodeProperties { node_id } => BatchOpResult::GetNodeProperties {
                properties_msgpack: core.get_node_properties_checked(node_id).map_err(|error| {
                    BatchApplyError {
                        index,
                        reason: error.to_string(),
                    }
                })?,
            },
        };
        results.push(result);
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(value: serde_json::Value) -> Vec<u8> {
        rmp_serde::to_vec_named(&value).expect("encode")
    }

    fn create(id: &str, status: &str, seq: i64) -> BatchOperation {
        BatchOperation::CreateNodeIfAbsent {
            node_id: id.to_string(),
            properties_msgpack: pack(
                serde_json::json!({"label": "task", "status": status, "seq": seq}),
            ),
        }
    }

    fn batch() -> Vec<BatchOperation> {
        vec![
            create("t1", "pending", 1),
            create("t2", "pending", 2),
            BatchOperation::ClaimNext {
                label: "task".to_string(),
                updates_msgpack: pack(serde_json::json!({"status": "claimed"})),
            },
            BatchOperation::GetNodeProperties {
                node_id: "t1".to_string(),
            },
        ]
    }

    // spec: MEDULLA-DURABLE-KERNEL-R871
    #[test]
    fn replay_applies_the_same_state_as_the_live_run() {
        let live = GraphCore::new();
        let live_results = apply_batch_operations(&live, &batch()).expect("live apply");
        let replayed = GraphCore::new();
        crate::durable_apply::apply(
            &replayed,
            &medulla_protocol_types::protocol::Method::Batch {
                operations: batch(),
            },
        )
        .expect("replay applies the committed batch");
        assert_eq!(
            live.get_node_properties("t1"),
            replayed.get_node_properties("t1")
        );
        assert_eq!(
            live.get_node_properties("t2"),
            replayed.get_node_properties("t2")
        );
        assert!(matches!(
            &live_results[2],
            BatchOpResult::ClaimNext { claimed: Some(node) } if node.node_id == "t1"
        ));
        assert!(matches!(
            &live_results[3],
            BatchOpResult::GetNodeProperties {
                properties_msgpack: Some(_)
            }
        ));
    }

    // spec: MEDULLA-DURABLE-KERNEL-R871
    #[test]
    fn size_and_blob_checks_refuse_before_anything_runs() {
        assert_eq!(validate_batch(&[], 8), Err(BatchRefusal::Empty));
        let max = 8;
        let oversize: Vec<BatchOperation> = (0..=max)
            .map(|i| create(&format!("n{i}"), "pending", i as i64))
            .collect();
        assert_eq!(
            validate_batch(&oversize, max),
            Err(BatchRefusal::TooLarge {
                count: max + 1,
                max
            })
        );
        let at_cap = &oversize[..max];
        assert_eq!(validate_batch(at_cap, max), Ok(()));
        let malformed = vec![BatchOperation::ClaimNext {
            label: "task".to_string(),
            updates_msgpack: vec![0xc1],
        }];
        assert!(matches!(
            validate_batch(&malformed, 8),
            Err(BatchRefusal::Malformed { index: 0, .. })
        ));
    }

    #[derive(Debug)]
    struct DownTier;

    impl medulla_graph_core::read_through::ReadThrough for DownTier {
        fn read_node_blob(
            &self,
            _node_id: &str,
        ) -> medulla_graph_core::read_through::NodeReadOutcome {
            medulla_graph_core::read_through::NodeReadOutcome::Unavailable(
                "writer gone".to_string(),
            )
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R871
    #[test]
    fn a_read_fault_on_replay_is_a_replay_divergence() {
        let core = GraphCore::new();
        core.set_read_through(std::sync::Arc::new(DownTier));
        let method = medulla_protocol_types::protocol::Method::Batch {
            operations: vec![
                create("t1", "pending", 1),
                BatchOperation::GetNodeProperties {
                    node_id: "evicted".to_string(),
                },
            ],
        };
        let divergence = crate::durable_apply::apply(&core, &method)
            .expect_err("a committed batch that faults on replay must not apply silently");
        assert_eq!(divergence.method, "Batch");
        assert!(divergence.message().starts_with("REPLAY_DIVERGED:"));
        assert!(divergence.detail.contains("batch operation 1"));
        assert!(medulla_types::contract::ServerErrorCode::ALL
            .iter()
            .any(|code| code.as_str() == "REPLAY_DIVERGED"));
    }
}
