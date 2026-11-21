//! Only `HarnessEdit` sets `editStatus` (MEDULLA-DECISION-ENGINE-R173.8).
//!
//! `compile_methods_with_outbox` calls [`refuse_direct_edit_status`] on every method of a
//! caller's write set before anything is compiled. A node write whose properties hold the key
//! `editStatus` (any value, including `proposed`) and a statement whose text names it are
//! refused `EDIT_STATUS_ENGINE_OWNED`; no graph read is needed. The guard runs only when the
//! writer is [`StatusWriter::Caller`](medulla_types::edit_machine::StatusWriter): the edit
//! machine itself and a replica re-compiling an admitted batch pass.

use medulla_protocol_types::protocol::Method;
use medulla_types::edit_machine::{StatusWriter, EDIT_STATUS_ENGINE_OWNED_MESSAGE};
use serde_json::Value;

use super::identity_metadata::{properties_blob, statement_text};

/// The graph property the edit machine owns.
const EDIT_STATUS_KEY: &str = "editStatus";

fn refusal() -> String {
    EDIT_STATUS_ENGINE_OWNED_MESSAGE.to_string()
}

/// Whether a decoded property map holds `editStatus` as a key.
fn blob_sets_status(blob: &[u8]) -> bool {
    medulla_types::msgpack::decode_property_object(blob)
        .is_ok_and(|properties| properties.contains_key(EDIT_STATUS_KEY))
}

/// Whether any object in `value`, at any depth, holds `editStatus` as a key.
fn value_sets_status(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            map.contains_key(EDIT_STATUS_KEY) || map.values().any(value_sets_status)
        }
        Value::Array(items) => items.iter().any(value_sets_status),
        _ => false,
    }
}

/// Whether `method` writes `editStatus`.
fn writes_status(method: &Method) -> bool {
    if let Some(blob) = properties_blob(method) {
        return blob_sets_status(blob);
    }
    if let Some(texts) = statement_text(method) {
        return texts.iter().any(|text| text.contains(EDIT_STATUS_KEY));
    }
    match method {
        Method::CompareAndSetNodeFields {
            updates_msgpack, ..
        } => blob_sets_status(updates_msgpack),
        Method::BatchUpdate { operations_msgpack } => {
            rmp_serde::from_slice::<Value>(operations_msgpack)
                .is_ok_and(|operations| value_sets_status(&operations))
        }
        Method::Batch { operations } => {
            use medulla_query_types::wire::BatchOperation;
            operations.iter().any(|operation| match operation {
                BatchOperation::CreateNodeIfAbsent {
                    properties_msgpack, ..
                } => blob_sets_status(properties_msgpack),
                BatchOperation::CompareAndSetNodeFields {
                    updates_msgpack, ..
                }
                | BatchOperation::ClaimNext {
                    updates_msgpack, ..
                } => blob_sets_status(updates_msgpack),
                _ => false,
            })
        }
        _ => false,
    }
}

/// Refuse a caller write of `editStatus`: `Ok` for every other method.
pub fn refuse_direct_edit_status(method: &Method) -> Result<(), String> {
    if writes_status(method) {
        Err(refusal())
    } else {
        Ok(())
    }
}

/// Run [`refuse_direct_edit_status`] over a whole write set when `writer` is a caller.
pub fn refuse_direct_edit_status_all(
    methods: &[Method],
    writer: StatusWriter,
) -> Result<(), String> {
    if writer != StatusWriter::Caller {
        return Ok(());
    }
    methods.iter().try_for_each(refuse_direct_edit_status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_types::edit_machine::StatusWriter;

    fn add_node(properties: Value) -> Method {
        Method::AddNode {
            node_id: "edit:1".into(),
            properties_msgpack: rmp_serde::to_vec_named(&properties).unwrap(),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R173.8
    #[test]
    fn caller_writes_of_edit_status_are_refused() {
        let message = EDIT_STATUS_ENGINE_OWNED_MESSAGE;
        for status in ["proposed", "shipped"] {
            let write = add_node(serde_json::json!({"_harness_edit": true, "editStatus": status}));
            assert_eq!(
                refuse_direct_edit_status_all(&[write], StatusWriter::Caller).unwrap_err(),
                message
            );
        }
        let statement = Method::Sql {
            query: "UPDATE edits SET editStatus = 'shipped'".into(),
            params_msgpack: Vec::new(),
        };
        assert_eq!(
            refuse_direct_edit_status_all(&[statement], StatusWriter::Caller).unwrap_err(),
            message
        );
        // The same node write passes under the edit machine and a replica.
        for writer in [StatusWriter::EditMachine, StatusWriter::Replicated] {
            let write = add_node(serde_json::json!({"_harness_edit": true, "editStatus": "proposed"}));
            assert!(refuse_direct_edit_status_all(&[write], writer).is_ok());
        }
        // A write that never names the key passes for a caller.
        let plain = add_node(serde_json::json!({"type": "Doc", "note": "editStatus is a word"}));
        assert!(refuse_direct_edit_status_all(&[plain], StatusWriter::Caller).is_ok());
    }
}
