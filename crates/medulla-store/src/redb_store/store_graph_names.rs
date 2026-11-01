use super::store_prelude::*;
use super::*;

pub fn reject_reserved_graph(graph: &str) -> Result<(), String> {
    if graph == SHARD_CONTROL_GRAPH || sanitize(graph) == SHARD_CONTROL_GRAPH {
        return Err(format!(
            "'{SHARD_CONTROL_GRAPH}' is the shard's reserved control scope and cannot be used as a graph"
        ));
    }
    Ok(())
}

/// Map a logical graph name to the bounded durable key used by the served and
/// embedded paths. Escaping avoids collisions caused by lossy path replacement.
pub fn sanitize(name: &str) -> String {
    use sha2::{Digest, Sha256};

    let mut key = String::with_capacity(name.len());
    for &byte in name.as_bytes() {
        use std::fmt::Write as _;
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            key.push(char::from(byte));
        } else {
            write!(&mut key, "~{byte:02x}").expect("writing to String cannot fail");
        }
    }
    if key.len() <= 200 {
        return key;
    }
    let digest = Sha256::digest(name.as_bytes());
    let mut bounded = String::with_capacity(66);
    bounded.push_str("~h");
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut bounded, "{byte:02x}").expect("writing to String cannot fail");
    }
    bounded
}

/// Drop one graph's catalog entry on the control scope.
pub fn remove_graph_catalog_row(shard: &Shard, graph: &str) -> Result<(), String> {
    let op_id = format!("graph_purge/{graph}");
    let (group, batches) = shard.admit_maintenance(&[], &op_id)?;
    let write = ShardWrite::open(shard, &group, &[], &batches)?;
    let removed = write
        .control()
        .open_table(GRAPH_META)?
        .remove(graph)
        .map(|_| ())
        .map_err(|error| error.to_string());
    let finished = write.finish();
    match (removed, finished) {
        (Ok(()), Ok(())) => shard.commit_drain(group, &batches, 0),
        (Err(error), _) | (Ok(()), Err(error)) => {
            shard.mutations().abort_group(group)?;
            Err(error)
        }
    }
}

/// The operation envelope a graph-shard test fixture batch carries.
///
/// Minted through the one public constructor, so a fixture cannot become a
/// second minting path: everything it does not name is this deployment's
/// documented constant, exactly as a producer with no verified request carrier
/// gets. The attempt nonce is server-minted, so rebuilding a fixture for the
/// same key is a FRESH attempt over the same stable operation -- which is the
/// case the kernel must replay rather than refuse.
#[cfg(any(test, feature = "test-support"))]
pub fn fixture_operation_envelope(
    identity: &medulla_mutation_types::mutation_scope::MutationScopeIdentity,
    actor: &str,
    request_id: u64,
    idempotency_key: &str,
) -> medulla_protocol_types::mutation_batch::MutationEnvelope {
    let method = medulla_types::contract::MethodId::new(
        medulla_protocol_types::mutation_batch::BATCH_COMPILED_METHODS,
    )
    .expect("the reserved batch method id is canonical");
    medulla_protocol_types::mutation_batch::MutationEnvelope::for_scope(
        medulla_protocol_types::mutation_batch::CompiledScope {
            identity,
            actor,
            serving_principal: actor,
            request_id,
            idempotency_key,
            nonce: medulla_types::contract::Nonce::minted(),
            now_ms: 0,
        },
        medulla_protocol_types::mutation_batch::CompiledOperation {
            method_schema_id: medulla_protocol_types::mutation_batch::method_schema_id(&method)
                .expect("a reserved batch method has a derived schema id"),
            method,
            method_schema_digest: medulla_types::contract::Digest256::from_bytes([0_u8; 32]),
            canonical_payload_digest: medulla_types::contract::Digest256::from_bytes([1_u8; 32]),
        },
    )
    .expect("a fixture scope mints a valid operation envelope")
}

pub fn durable_msgpack_limits() -> medulla_types::msgpack::MsgpackLimits {
    medulla_types::msgpack::MsgpackLimits::new(
        MAX_DURABLE_MSGPACK_BYTES,
        MAX_DURABLE_MSGPACK_ITEMS,
        medulla_types::msgpack::DEFAULT_MAX_DEPTH,
    )
}

pub fn property_string<'a>(
    props: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> &'a str {
    props
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
}

pub fn property_u64(props: &serde_json::Map<String, serde_json::Value>, key: &str) -> u64 {
    props
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
}

pub fn property_f64(props: &serde_json::Map<String, serde_json::Value>, key: &str) -> f64 {
    props
        .get(key)
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0)
}

