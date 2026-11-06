//! Operation-bound MCP sources (MEDULLA-FEDERATED-QUERY-R073.3).
//!
//! Medulla never calls MCP. [`McpRemote`] keeps the federation planning (capabilities,
//! pushed key filter, exact local residual) and hands one [`ConnectorRowRequest`] to the
//! host-registered [`ConnectorRowSource`] port; the governed call, the caller identity and
//! the shared outbound guard all run on the provider side (`contracts/mcp-source-fleet-call.md`).
//! `McpRemote` holds no MCP client and opens no connection.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use medulla_exec::rowset::RowSet;
use medulla_types::contract::ServerErrorCode;
use serde_json::{json, Value};

use super::budget::RESULT_INCOMPLETE;
use super::capability::{KeyLookup, LimitPushdown, RemoteRequest, SourceCapabilities, SourceRate};
use super::remote::{Identity, RemoteFetch};

/// Keys per request when the registration declares key-filter pushdown.
const MAX_MCP_KEYS: usize = 100;
/// Row budget of one connector call (contract bounds: 1 to 10000).
const CALL_MAX_ROWS: usize = 10_000;
/// Time budget of one connector call in milliseconds (contract bounds: 100 to 60000).
const CALL_TIMEOUT_MS: u64 = 30_000;

/// The budget one connector call runs under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectorBudget {
    pub max_rows: usize,
    pub timeout_ms: u64,
}

/// One tool call handed to the provider.
#[derive(Clone, Debug, PartialEq)]
pub struct ConnectorRowRequest {
    pub source_label: String,
    pub server: String,
    pub tool: String,
    pub arguments: BTreeMap<String, Value>,
    pub pushed_filter: Vec<String>,
    pub budget: ConnectorBudget,
}

/// The provider's reply: rows in the connector row contract, or a partial answer that
/// names the source it is missing.
pub struct ConnectorRows {
    pub source_label: String,
    pub rows: RowSet,
    pub truncated: bool,
    pub missing_source: Option<String>,
}

/// The host-registered port that serves operation-bound MCP sources.
pub trait ConnectorRowSource: Send + Sync {
    /// The argument names the tool declares, or `None` when the tool is not registered.
    fn declared_arguments(&self, server: &str, tool: &str) -> Option<Vec<String>>;
    /// Run the call; `Err` is any failure, timeout or non-row-list result.
    fn rows(&self, request: &ConnectorRowRequest) -> Result<ConnectorRows, String>;
}

type SharedPort = Arc<dyn ConnectorRowSource>;

static PORT: RwLock<Option<SharedPort>> = RwLock::new(None);

/// Register the host's connector port, replacing any earlier one.
pub fn register_connector_row_source(port: SharedPort) {
    if let Ok(mut slot) = PORT.write() {
        *slot = Some(port);
    }
}

/// The registered port, if the host bound one.
pub(crate) fn registered_port() -> Option<SharedPort> {
    PORT.read().ok().and_then(|slot| slot.clone())
}

fn refuse(code: ServerErrorCode, detail: impl std::fmt::Display) -> String {
    format!("{}: {detail}", code.as_str())
}

/// An operation-bound MCP source resolved to the connector port.
pub(crate) struct McpRemote {
    server: String,
    tool: String,
    key_pushdown: bool,
    port: Option<SharedPort>,
    identity: Identity,
}

impl McpRemote {
    pub(crate) fn new(
        (server, tool): (&str, &str),
        key_pushdown: bool,
        port: Option<SharedPort>,
        identity: Identity,
    ) -> Self {
        Self {
            server: server.to_string(),
            tool: tool.to_string(),
            key_pushdown,
            port,
            identity,
        }
    }

    fn unreachable(&self, why: &str) -> String {
        refuse(
            ServerErrorCode::SourceUnreachable,
            format!("{}: {why}", self.identity.label),
        )
    }

    /// The tool arguments `request` implies; only the keys and the limit are ever sent.
    fn arguments(request: &RemoteRequest) -> BTreeMap<String, Value> {
        let mut arguments = BTreeMap::new();
        if !request.keys.is_empty() {
            arguments.insert("keys".to_string(), json!(request.keys));
        }
        if let Some(limit) = request.limit {
            arguments.insert("limit".to_string(), json!(limit));
        }
        arguments
    }

    /// Arguments outside the declared tool schema refuse before anything is sent.
    fn check_schema(
        &self,
        port: &dyn ConnectorRowSource,
        arguments: &BTreeMap<String, Value>,
    ) -> Result<(), String> {
        let declared = port
            .declared_arguments(&self.server, &self.tool)
            .ok_or_else(|| self.unreachable("the tool is not registered"))?;
        match arguments.keys().find(|name| !declared.contains(name)) {
            Some(name) => Err(refuse(
                ServerErrorCode::UnsupportedSearchArgument,
                format!(
                    "{}: `{name}` is not declared by the tool",
                    self.identity.label
                ),
            )),
            None => Ok(()),
        }
    }

    fn check_reply(&self, reply: ConnectorRows) -> Result<RowSet, String> {
        if let Some(missing) = reply.missing_source {
            return Err(self.unreachable(&format!("partial reply, missing {missing}")));
        }
        if reply.truncated {
            return Err(format!(
                "{RESULT_INCOMPLETE}: {} returned a truncated row list",
                reply.source_label
            ));
        }
        Ok(reply.rows)
    }
}

impl RemoteFetch for McpRemote {
    fn capabilities(&self) -> SourceCapabilities {
        let key_lookup = if self.key_pushdown {
            KeyLookup::Batched {
                max_keys: MAX_MCP_KEYS,
            }
        } else {
            KeyLookup::Unsupported
        };
        SourceCapabilities {
            rate: SourceRate::new(2, 4),
            ..SourceCapabilities::single_full_fetch(key_lookup, LimitPushdown::Unsupported)
        }
    }

    fn identity(&self) -> &Identity {
        &self.identity
    }

    fn fetch(&self, request: &RemoteRequest) -> Result<RowSet, String> {
        let port = self
            .port
            .as_deref()
            .ok_or_else(|| self.unreachable("no connector port is registered"))?;
        let arguments = Self::arguments(request);
        self.check_schema(port, &arguments)?;
        let call = ConnectorRowRequest {
            source_label: self.identity.label.clone(),
            server: self.server.clone(),
            tool: self.tool.clone(),
            arguments,
            pushed_filter: request.keys.clone(),
            budget: ConnectorBudget {
                max_rows: CALL_MAX_ROWS,
                timeout_ms: CALL_TIMEOUT_MS,
            },
        };
        let reply = port.rows(&call).map_err(|why| self.unreachable(&why))?;
        self.check_reply(reply)
    }

    fn fit_keys(&self, keys: &[String]) -> usize {
        keys.len().min(MAX_MCP_KEYS)
    }
}
