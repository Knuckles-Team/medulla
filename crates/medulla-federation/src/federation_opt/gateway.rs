//! The HTTP connector gateway: a [`ConnectorRowSource`] that forwards each call to the fleet
//! gateway named by `MEDULLA_CONNECTOR_GATEWAY_URL` (MEDULLA-UNIFIED-DATA-PLANE-R038.7.3).
//!
//! Medulla never calls MCP. The gateway is the provider of `contracts/mcp-source-fleet-call.md`:
//! it runs the governed call under the caller's identity and answers rows. The engine only posts
//! JSON to it, through the shared outbound guard (`validate_http_json_target`): a destination the
//! guard refuses is never contacted. A host that registers its own port with
//! [`super::mcp::register_connector_row_source`] takes precedence over the gateway.
//!
//! Requests are `{"op": "declared_arguments" | "web_capability" | "rows", ...}`; rows answer
//! `{"source_label", "rows": [{"id", "score", "values", "texts"}], "truncated", "missing_source"}`.

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use medulla_exec::rowset::{RowSet, TextChannels, ValueChannels};
use medulla_types::web_source::WebSearchCapability;
use serde_json::{json, Value};

use super::mcp::{
    ConnectorAccessAnswer, ConnectorAccessCheck, ConnectorRowRequest, ConnectorRowSource,
    ConnectorRows,
};

/// The environment variable naming the gateway endpoint.
pub const GATEWAY_URL_ENV: &str = "MEDULLA_CONNECTOR_GATEWAY_URL";

const IO_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// The gateway the environment names, if any.
pub(crate) fn from_env() -> Option<Arc<dyn ConnectorRowSource>> {
    let url = std::env::var(GATEWAY_URL_ENV)
        .ok()
        .filter(|u| !u.is_empty())?;
    Some(Arc::new(HttpConnectorGateway { url }))
}

/// A connector port that posts each call to one gateway URL.
pub(crate) struct HttpConnectorGateway {
    url: String,
}

impl HttpConnectorGateway {
    fn post(&self, body: &Value) -> Result<Value, String> {
        let target = crate::federation_ssrf::validate_http_json_target(&self.url)?;
        let builder = ureq::Agent::config_builder().timeout_global(Some(IO_TIMEOUT));
        let agent = crate::federation_ssrf::pinned_agent(&target, builder);
        let response = agent
            .post(&self.url)
            .header("Content-Type", "application/json")
            .send(body.to_string().as_bytes())
            .map_err(|_| "the connector gateway request failed".to_string())?;
        if !response.status().is_success() {
            return Err("the connector gateway returned an error".to_string());
        }
        let mut bytes = Vec::new();
        response
            .into_body()
            .into_reader()
            .take((MAX_BODY_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| "the connector gateway response could not be read".to_string())?;
        if bytes.len() > MAX_BODY_BYTES {
            return Err("the connector gateway response exceeds its limit".to_string());
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| "the connector gateway answered something that is not JSON".to_string())
    }
}

impl ConnectorRowSource for HttpConnectorGateway {
    fn declared_arguments(&self, server: &str, tool: &str) -> Option<Vec<String>> {
        let reply = self
            .post(&json!({"op": "declared_arguments", "server": server, "tool": tool}))
            .ok()?;
        reply
            .get("arguments")?
            .as_array()?
            .iter()
            .map(|name| name.as_str().map(str::to_string))
            .collect()
    }

    fn access_check(
        &self,
        request: &ConnectorAccessCheck,
    ) -> Result<ConnectorAccessAnswer, String> {
        let reply = self.post(&json!({
            "op": "access_check",
            "server": request.server,
            "tool": request.tool,
            "principal": request.principal,
            "record_refs": request.record_refs,
            "timeout_ms": request.timeout_ms,
        }))?;
        let names = |key: &str| -> Result<Vec<String>, String> {
            reply
                .get(key)
                .and_then(Value::as_array)
                .ok_or_else(|| format!("the access check reply has no `{key}` list"))?
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| format!("the access check `{key}` list holds a non-string"))
                })
                .collect()
        };
        Ok(ConnectorAccessAnswer {
            allowed: names("allowed")?,
            denied: names("denied")?,
            checked_at: reply
                .get("checked_at")
                .and_then(Value::as_u64)
                .ok_or_else(|| "the access check reply has no `checked_at`".to_string())?,
        })
    }

    fn web_capability(&self, server: &str, tool: &str) -> Option<WebSearchCapability> {
        let reply = self
            .post(&json!({"op": "web_capability", "server": server, "tool": tool}))
            .ok()?;
        serde_json::from_value(reply.get("capability")?.clone()).ok()
    }

    fn rows(&self, request: &ConnectorRowRequest) -> Result<ConnectorRows, String> {
        let reply = self.post(&json!({
            "op": "rows",
            "request": {
                "source_label": request.source_label,
                "server": request.server,
                "tool": request.tool,
                "arguments": request.arguments,
                "pushed_filter": request.pushed_filter,
                "budget": {
                    "max_rows": request.budget.max_rows,
                    "timeout_ms": request.budget.timeout_ms,
                },
            },
        }))?;
        parse_rows(&reply)
    }
}

fn parse_rows(reply: &Value) -> Result<ConnectorRows, String> {
    let list = reply
        .get("rows")
        .and_then(Value::as_array)
        .ok_or_else(|| "the connector reply is not a row list".to_string())?;
    let mut scored: Vec<(String, Option<f32>)> = Vec::with_capacity(list.len());
    let (mut values, mut texts) = (ValueChannels::new(), TextChannels::new());
    for row in list {
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "a connector row has no id".to_string())?
            .to_string();
        scored.push((
            id.clone(),
            row.get("score").and_then(Value::as_f64).map(|s| s as f32),
        ));
        let numbers: BTreeMap<String, f64> = row
            .get("values")
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .filter_map(|(name, v)| Some((name.clone(), v.as_f64()?)))
                    .collect()
            })
            .unwrap_or_default();
        let strings: BTreeMap<String, String> = row
            .get("texts")
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .filter_map(|(name, v)| Some((name.clone(), v.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        values.insert(id.clone(), numbers);
        texts.insert(id, strings);
    }
    Ok(ConnectorRows {
        source_label: reply
            .get("source_label")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        rows: RowSet::from_rows(scored)
            .with_values(values)
            .with_texts(texts),
        truncated: reply
            .get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        missing_source: reply
            .get("missing_source")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MEDULLA-UNIFIED-DATA-PLANE-R038.7.9: a gateway reply parses into typed rows, and a reply
    /// that is not a row list is refused.
    // spec: MEDULLA-UNIFIED-DATA-PLANE-R038.7.9
    #[test]
    fn gateway_rows_round_trip() {
        let reply = json!({
            "source_label": "mcp:web#abcd1234",
            "rows": [
                {"id": "https://a", "score": 0.5, "values": {"rank": 1}, "texts": {"title": "A"}},
                {"id": "https://b", "values": {"rank": 2}, "texts": {"title": "B"}}
            ],
            "truncated": false,
            "missing_source": null
        });
        let rows = parse_rows(&reply).unwrap();
        assert_eq!(rows.rows.ids(), ["https://a", "https://b"]);
        assert_eq!(rows.rows.text("https://b", "title"), Some("B"));
        assert_eq!(rows.rows.value("https://a", "rank"), Some(1.0));
        assert_eq!(rows.rows.rows()[0].score, Some(0.5));
        assert_eq!(rows.rows.rows()[1].score, None);
        assert!(!rows.truncated && rows.missing_source.is_none());
        assert!(parse_rows(&json!({"rows": "nope"})).is_err());
        assert!(parse_rows(&json!({"rows": [{"title": "no id"}]})).is_err());
    }
}
