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
use super::search::{SearchHit, SearchRemote};
use medulla_types::search_source::SearchIndexCapability;
use medulla_types::web_source::WebSearchCapability;

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

/// One read-time access check handed to the provider
/// (MEDULLA-FEDERATED-QUERY-R703.3): which of `record_refs` may `principal` read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectorAccessCheck {
    pub server: String,
    pub tool: String,
    pub principal: String,
    pub record_refs: Vec<String>,
    pub timeout_ms: u64,
}

/// The provider's answer to one access check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectorAccessAnswer {
    pub allowed: Vec<String>,
    pub denied: Vec<String>,
    /// Unix milliseconds.
    pub checked_at: u64,
}

/// The host-registered port that serves operation-bound MCP sources.
pub trait ConnectorRowSource: Send + Sync {
    /// Ask the connector which records `principal` may read. A provider that cannot
    /// answer access checks reports `Err`, and the caller fails closed.
    fn access_check(
        &self,
        _request: &ConnectorAccessCheck,
    ) -> Result<ConnectorAccessAnswer, String> {
        Err("this connector port does not answer access checks".to_string())
    }
    /// The argument names the tool declares, or `None` when the tool is not registered.
    fn declared_arguments(&self, server: &str, tool: &str) -> Option<Vec<String>>;
    /// Run the call; `Err` is any failure, timeout or non-row-list result.
    fn rows(&self, request: &ConnectorRowRequest) -> Result<ConnectorRows, String>;
    /// The web metasearch capability the tool's declared schema carries, or `None` when the tool
    /// is not a web metasearch tool (MEDULLA-UNIFIED-DATA-PLANE-R038.7.1). A web tool answers
    /// with one row per result: the id is the URL, the text columns `title`, `snippet` and
    /// `engine`, and the value columns `rank` and `retrieved_at` (Unix seconds).
    fn web_capability(&self, _server: &str, _tool: &str) -> Option<WebSearchCapability> {
        None
    }
}

pub(crate) type SharedPort = Arc<dyn ConnectorRowSource>;

static PORT: RwLock<Option<SharedPort>> = RwLock::new(None);

/// Register the host's connector port, replacing any earlier one.
pub fn register_connector_row_source(port: SharedPort) {
    if let Ok(mut slot) = PORT.write() {
        *slot = Some(port);
    }
}

/// The registered port: the one the host bound, else the HTTP gateway named by
/// `MEDULLA_CONNECTOR_GATEWAY_URL` (see [`super::gateway`]), else none.
pub fn registered_port() -> Option<SharedPort> {
    PORT.read()
        .ok()
        .and_then(|slot| slot.clone())
        .or_else(super::gateway::from_env)
}

fn refuse(code: ServerErrorCode, detail: impl std::fmt::Display) -> String {
    format!("{}: {detail}", code.as_str())
}

/// An operation-bound MCP source resolved to the connector port.
pub(crate) struct McpRemote {
    server: String,
    tool: String,
    key_pushdown: bool,
    search: Option<SearchIndexCapability>,
    port: Option<SharedPort>,
    identity: Identity,
}

impl McpRemote {
    pub(crate) fn new(
        (server, tool): (&str, &str),
        (key_pushdown, search): (bool, Option<SearchIndexCapability>),
        port: Option<SharedPort>,
        identity: Identity,
    ) -> Self {
        Self {
            server: server.to_string(),
            tool: tool.to_string(),
            key_pushdown,
            search,
            port,
            identity,
        }
    }

    pub(super) fn label(&self) -> &str {
        &self.identity.label
    }

    pub(super) fn fingerprint(&self) -> super::stats::Fingerprint {
        self.identity.fingerprint
    }

    pub(super) fn cache_name(&self) -> Option<&str> {
        self.identity.cache_name.as_deref()
    }

    /// The web capability of the bound tool, when it is a web metasearch tool.
    pub(super) fn web_capability(&self) -> Option<WebSearchCapability> {
        self.port
            .as_deref()?
            .web_capability(&self.server, &self.tool)
    }

    pub(super) fn unreachable(&self, why: &str) -> String {
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
        self.call(Self::arguments(request), request.keys.clone())
    }

    fn fit_keys(&self, keys: &[String]) -> usize {
        keys.len().min(MAX_MCP_KEYS)
    }
}

impl McpRemote {
    /// One governed tool call with `arguments`, checked against the declared schema first.
    pub(super) fn call(
        &self,
        arguments: BTreeMap<String, Value>,
        pushed_filter: Vec<String>,
    ) -> Result<RowSet, String> {
        let port = self
            .port
            .as_deref()
            .ok_or_else(|| self.unreachable("no connector port is registered"))?;
        self.check_schema(port, &arguments)?;
        let call = ConnectorRowRequest {
            source_label: self.identity.label.clone(),
            server: self.server.clone(),
            tool: self.tool.clone(),
            arguments,
            pushed_filter,
            budget: ConnectorBudget {
                max_rows: CALL_MAX_ROWS,
                timeout_ms: CALL_TIMEOUT_MS,
            },
        };
        let reply = port.rows(&call).map_err(|why| self.unreachable(&why))?;
        self.check_reply(reply)
    }

    /// The tool arguments a pushed search implies: `query`, `k`, `offset` and one argument
    /// per structured filter. An argument the tool does not declare refuses before the call.
    fn search_arguments(request: &RemoteRequest) -> Result<BTreeMap<String, Value>, String> {
        let push = request
            .search
            .as_ref()
            .ok_or_else(|| "a search request carries a pushed search".to_string())?;
        let mut arguments = BTreeMap::new();
        if let Some(text) = &push.text {
            arguments.insert("query".to_string(), json!(text));
        }
        arguments.insert("k".to_string(), json!(push.k));
        arguments.insert(
            "offset".to_string(),
            json!(request.page.map_or(0, |page| page.offset)),
        );
        for (name, value) in &push.filters {
            arguments.insert(name.clone(), json!(value));
        }
        Ok(arguments)
    }
}

/// A search-index tool answers a pushed search: the rows it returns are the hits, scored by
/// the index (MEDULLA-UNIFIED-DATA-PLANE-R038.5.3).
impl SearchRemote for McpRemote {
    fn identity(&self) -> &Identity {
        &self.identity
    }

    fn capability(&self) -> SearchIndexCapability {
        self.search
            .expect("a SearchRemote is built only for a declared search capability")
    }

    fn fetch(&self, request: &RemoteRequest) -> Result<Vec<SearchHit>, String> {
        let rows = self.call(Self::search_arguments(request)?, Vec::new())?;
        Ok(rows
            .rows()
            .iter()
            .map(|row| SearchHit {
                id: row.id.clone(),
                score: row.score,
                attributes: Vec::new(),
            })
            .collect())
    }
}
