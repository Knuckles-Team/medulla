//! One governed tool call that changes a connector's system, made through the engine's
//! connector gateway (MEDULLA-SDLC-R017.8).
//!
//! The engine never calls a connector itself. [`call_connector_tool`] hands one
//! [`ConnectorToolRequest`] to the host-registered connector port (or the HTTP gateway named by
//! `MEDULLA_CONNECTOR_GATEWAY_URL`), which runs it through the fleet under the caller's
//! identity and the shared outbound guard. A build with no connector port, or a port that serves
//! no tool calls, answers the typed failure [`PORT_UNAVAILABLE`]. The call blocks on the network;
//! an async caller runs it on a blocking thread.

use std::collections::BTreeMap;

use serde_json::Value;

/// The code of a build or port that serves no tool calls.
pub const PORT_UNAVAILABLE: &str = "CONNECTOR_PORT_UNAVAILABLE";

/// The time one tool call may take, in milliseconds (contract bounds: 100 to 60000).
pub const TOOL_CALL_TIMEOUT_MS: u64 = 30_000;

/// One tool call: the connector server and tool, its arguments, the principal it runs under and
/// the key that makes a repeat safe.
#[derive(Clone, Debug, PartialEq)]
pub struct ConnectorToolRequest {
    /// The registered connector server.
    pub server: String,
    /// The tool on that server.
    pub tool: String,
    /// The tool arguments.
    pub arguments: BTreeMap<String, Value>,
    /// The verified principal the call runs under.
    pub principal: String,
    /// A repeat with this key must not repeat the effect.
    pub idempotency_key: String,
}

/// Why a tool call did not answer: the stable code and a short message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectorToolFailure {
    /// The stable code.
    pub code: String,
    /// A short message; empty when the connector gave none.
    pub message: String,
}

/// Run one tool call through the connector gateway.
#[cfg(feature = "federation")]
pub fn call_connector_tool(request: &ConnectorToolRequest) -> Result<Value, ConnectorToolFailure> {
    use medulla_federation::federation_opt::mcp::{registered_port, ConnectorToolCall};
    let Some(port) = registered_port() else {
        return Err(ConnectorToolFailure {
            code: PORT_UNAVAILABLE.to_string(),
            message: "no connector port is registered".to_string(),
        });
    };
    port.tool_call(&ConnectorToolCall {
        server: request.server.clone(),
        tool: request.tool.clone(),
        arguments: request.arguments.clone(),
        principal: request.principal.clone(),
        idempotency_key: request.idempotency_key.clone(),
        timeout_ms: TOOL_CALL_TIMEOUT_MS,
    })
    .map_err(|error| ConnectorToolFailure {
        code: error.code,
        message: error.message,
    })
}

/// Run one tool call through the connector gateway: this build has none.
#[cfg(not(feature = "federation"))]
pub fn call_connector_tool(request: &ConnectorToolRequest) -> Result<Value, ConnectorToolFailure> {
    let _ = request;
    Err(ConnectorToolFailure {
        code: PORT_UNAVAILABLE.to_string(),
        message: "this build has no connector gateway".to_string(),
    })
}

#[cfg(all(test, feature = "federation"))]
mod tests {
    use super::*;
    use medulla_federation::federation_opt::mcp::{
        register_connector_row_source, ConnectorRowRequest, ConnectorRowSource, ConnectorRows,
        ConnectorToolCall, ConnectorToolError,
    };
    use std::sync::Arc;

    struct Scm;

    impl ConnectorRowSource for Scm {
        fn declared_arguments(&self, _server: &str, _tool: &str) -> Option<Vec<String>> {
            None
        }
        fn rows(&self, _request: &ConnectorRowRequest) -> Result<ConnectorRows, String> {
            Err("no rows".to_string())
        }
        fn tool_call(&self, request: &ConnectorToolCall) -> Result<Value, ConnectorToolError> {
            if request.tool == "open_pull_request" {
                Ok(serde_json::json!({
                    "url": format!("https://scm.example/{}", request.idempotency_key),
                    "principal": request.principal,
                }))
            } else {
                Err(ConnectorToolError {
                    code: "SCM_TOOL_UNKNOWN".to_string(),
                    message: request.tool.clone(),
                })
            }
        }
    }

    // spec: MEDULLA-SDLC-R017.8
    #[test]
    fn a_registered_port_answers_the_call_and_its_failure_is_typed() {
        register_connector_row_source(Arc::new(Scm));
        let mut request = ConnectorToolRequest {
            server: "scm".to_string(),
            tool: "open_pull_request".to_string(),
            arguments: BTreeMap::new(),
            principal: "reviewer-1".to_string(),
            idempotency_key: "proposal:1".to_string(),
        };
        let answer = call_connector_tool(&request).unwrap();
        assert_eq!(answer["url"], "https://scm.example/proposal:1");
        assert_eq!(answer["principal"], "reviewer-1");
        request.tool = "merge".to_string();
        let failure = call_connector_tool(&request).unwrap_err();
        assert_eq!(failure.code, "SCM_TOOL_UNKNOWN");
    }
}
