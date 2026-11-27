//! The served source connection methods (MEDULLA-UNIFIED-DATA-PLANE-R791):
//! `SourceConnectionGet`, `SourceConnectionList`, `SourceConnectionControl`, `SourceMappingGet`
//! and `SourceMappingControl`.
//!
//! Each arm reads only the nodes the caller can see in the request graph, decides in the pure
//! functions of `medulla_source_types::source_connection` and, for a write, commits the node in
//! one gateway-routed batch. A refusal and a replay return before the batch is built, so they
//! write nothing. A connection or mapping of another tenant refuses like an absent one.

use super::deploy::{verified_tenant, visible_nodes_of};
use super::*;

use medulla_protocol_types::result_contract::ingestion::{
    SourceConnectionControl as SourceConnectionControlResult,
    SourceConnectionGet as SourceConnectionGetResult,
    SourceConnectionList as SourceConnectionListResult,
    SourceMappingControl as SourceMappingControlResult, SourceMappingGet as SourceMappingGetResult,
};
use medulla_source_types::source_connection::{
    connection_node_id, handle_connection_control, handle_connection_get, handle_connection_list,
    handle_mapping_control, handle_mapping_get, mapping_node_id, validate_connection_id,
    SourceConnection, SourceConnectionOp, SourceMapping, SourceMappingOp, SOURCE_CONNECTION_TYPE,
    SOURCE_MAPPING_TYPE,
};
use medulla_types::transition_refused::MachineState;

/// Most connection nodes one list reads.
const MAX_CONNECTIONS_READ: usize = 100_000;

/// The stored connection of `connection_id` in the verified tenant that the caller can see. A
/// malformed id holds no connection.
async fn stored_connection(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
    tenant: &str,
    connection_id: &str,
) -> Result<Option<SourceConnection>, String> {
    if validate_connection_id(connection_id).is_err() {
        return Ok(None);
    }
    Ok(
        visible_node(state, target, &connection_node_id(tenant, connection_id))
            .await?
            .and_then(|node| SourceConnection::from_node(&node, tenant)),
    )
}

/// The stored mapping of `connection_id` in the verified tenant that the caller can see.
async fn stored_mapping(
    state: &Arc<RwLock<ServerState>>,
    target: &SdlcTarget<'_>,
    tenant: &str,
    connection_id: &str,
) -> Result<Option<SourceMapping>, String> {
    if validate_connection_id(connection_id).is_err() {
        return Ok(None);
    }
    Ok(
        visible_node(state, target, &mapping_node_id(tenant, connection_id))
            .await?
            .and_then(|node| SourceMapping::from_node(&node, tenant)),
    )
}

fn connection_properties(
    connection: &SourceConnection,
    target: &SdlcTarget<'_>,
) -> Result<Map<String, Value>, String> {
    let settings = serde_json::to_string(&connection.settings)
        .map_err(|_| "connection encoding failed".to_string())?;
    let mut properties = Map::new();
    properties.insert("type".into(), SOURCE_CONNECTION_TYPE.into());
    properties.insert(
        "connection_id".into(),
        connection.connection_id.clone().into(),
    );
    properties.insert("provider".into(), connection.provider.clone().into());
    properties.insert("label".into(), connection.label.clone().into());
    properties.insert("settings".into(), settings.into());
    properties.insert(
        "credential_ref".into(),
        connection.credential_ref.clone().into(),
    );
    properties.insert("state".into(), connection.state.as_str().into());
    stamp(&mut properties, target);
    Ok(properties)
}

fn mapping_properties(
    mapping: &SourceMapping,
    target: &SdlcTarget<'_>,
) -> Result<Map<String, Value>, String> {
    let body = serde_json::to_string(&mapping.mapping)
        .map_err(|_| "mapping encoding failed".to_string())?;
    let mut properties = Map::new();
    properties.insert("type".into(), SOURCE_MAPPING_TYPE.into());
    properties.insert("connection_id".into(), mapping.connection_id.clone().into());
    properties.insert("mapping".into(), body.into());
    properties.insert("digest".into(), mapping.digest.clone().into());
    properties.insert("state".into(), mapping.state.stored().into());
    stamp(&mut properties, target);
    Ok(properties)
}

/// `SourceConnectionGet`: the stored connection of one id.
pub(super) async fn get(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    connection_id: String,
) -> Response {
    let result = async {
        let tenant = verified_tenant(&target)?;
        let stored = stored_connection(state, &target, tenant, &connection_id).await?;
        handle_connection_get(stored).map_err(|e| e.to_string())
    }
    .await;
    typed_response::<SourceConnectionGetResult>(target.req_id, result)
}

/// `SourceConnectionList`: one page of the tenant's connections the caller can see.
pub(super) async fn list(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    limit: Option<u32>,
    cursor: Option<String>,
) -> Response {
    let result = async {
        let tenant = verified_tenant(&target)?;
        let connections: Vec<SourceConnection> =
            visible_nodes_of(state, &target, SOURCE_CONNECTION_TYPE, MAX_CONNECTIONS_READ)
                .await?
                .iter()
                .filter_map(|(_, node)| SourceConnection::from_node(node, tenant))
                .collect();
        handle_connection_list(connections, tenant, limit, cursor.as_deref())
            .map_err(|e| e.to_string())
    }
    .await;
    typed_response::<SourceConnectionListResult>(target.req_id, result)
}

/// `SourceConnectionControl`: put or delete one connection. A delete removes the connection's
/// mapping node too, when there is one.
pub(super) async fn control(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    op: SourceConnectionOp,
) -> Response {
    let result = async {
        let tenant = verified_tenant(&target)?;
        validate_connection_id(op.connection_id()).map_err(|e| e.to_string())?;
        let connection_id = op.connection_id().to_owned();
        let stored = stored_connection(state, &target, tenant, &connection_id).await?;
        let write =
            handle_connection_control(tenant, stored.as_ref(), op).map_err(|e| e.to_string())?;
        if !write.replayed {
            let operations = match &write.connection {
                Some(connection) => vec![json!({
                    "op": "upsert_node",
                    "id": connection_node_id(tenant, &connection.connection_id),
                    "properties": connection_properties(connection, &target)?,
                })],
                None => {
                    let mut operations = vec![json!({
                        "op": "remove_node",
                        "id": connection_node_id(tenant, &connection_id),
                    })];
                    if stored_mapping(state, &target, tenant, &connection_id)
                        .await?
                        .is_some()
                    {
                        operations.push(json!({
                            "op": "remove_node",
                            "id": mapping_node_id(tenant, &connection_id),
                        }));
                    }
                    operations
                }
            };
            commit(state, &target, bounded(operations)?).await?;
        }
        Ok(write.receipt)
    }
    .await;
    typed_response::<SourceConnectionControlResult>(target.req_id, result)
}

/// `SourceMappingGet`: the stored mapping of one connection.
pub(super) async fn mapping_get(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    connection_id: String,
) -> Response {
    let result = async {
        let tenant = verified_tenant(&target)?;
        let connection = stored_connection(state, &target, tenant, &connection_id).await?;
        let stored = stored_mapping(state, &target, tenant, &connection_id).await?;
        handle_mapping_get(connection.as_ref(), stored).map_err(|e| e.to_string())
    }
    .await;
    typed_response::<SourceMappingGetResult>(target.req_id, result)
}

/// `SourceMappingControl`: propose or approve the mapping of one connection.
pub(super) async fn mapping_control(
    state: &Arc<RwLock<ServerState>>,
    target: SdlcTarget<'_>,
    op: SourceMappingOp,
) -> Response {
    let result = async {
        let tenant = verified_tenant(&target)?;
        validate_connection_id(op.connection_id()).map_err(|e| e.to_string())?;
        let connection_id = op.connection_id().to_owned();
        let connection = stored_connection(state, &target, tenant, &connection_id).await?;
        let stored = stored_mapping(state, &target, tenant, &connection_id).await?;
        let write =
            handle_mapping_control(connection.as_ref(), stored, op).map_err(|e| e.to_string())?;
        if !write.replayed {
            let operations = bounded(vec![json!({
                "op": "upsert_node",
                "id": mapping_node_id(tenant, &connection_id),
                "properties": mapping_properties(&write.mapping, &target)?,
            })])?;
            commit(state, &target, operations).await?;
        }
        Ok(write.receipt())
    }
    .await;
    typed_response::<SourceMappingControlResult>(target.req_id, result)
}
