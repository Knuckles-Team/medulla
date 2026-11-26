//! Raw-access sources (MEDULLA-FEDERATED-QUERY-R729).
//!
//! A source that declares `access_kind: raw` holds raw text the connector scans for the
//! engine. Medulla never calls the connector: [`RawRemote`] hands each call to the
//! host-registered [`ConnectorRowSource`] port, which runs it through the fleet path under
//! the caller's identity. A scan is three tools in order: `raw_search` finds the candidate
//! ids, `raw_check_context` keeps the ids the caller may read, `raw_read_span` reads the
//! kept spans. Every call carries `max_scan_bytes`, the bytes of the query budget not yet
//! spent; the connector answers `OVER_CEILING` rather than scan past it, and that answer is
//! the typed over-budget refusal `FEDERATION_BUDGET_EXCEEDED:scan_bytes`. The bytes each call
//! reports are charged to the budget and priced in cost units by
//! `federated.raw_scan_cost_per_kib`.

use std::collections::{BTreeMap, BTreeSet};

use medulla_exec::rowset::{RowSet, TextChannels};
use medulla_query_types::wire::ForeignSourceSpec;
use medulla_query_types::wire::Op;
use medulla_types::contract::ServerErrorCode;
use serde_json::{json, Value};

use super::budget::refusal;
use super::mcp::{registered_port, RawToolCall, SharedPort};
use super::remote::Identity;
use super::search::REMOTE_CHANNEL;
use super::session::FederationSession;
use super::trace::{FetchStrategy, FragmentTrace, RawScanReceipt};
use crate::federation::ForeignSourceRegistry;
use medulla_catalog_types::metadata_acceleration::FragmentExecutionMode;

/// The tool that finds candidate ids for a text query.
pub const RAW_SEARCH: &str = "raw_search";
/// The tool that reads the span of each id.
pub const RAW_READ_SPAN: &str = "raw_read_span";
/// The tool that answers which ids the caller may read.
pub const RAW_CHECK_CONTEXT: &str = "raw_check_context";
/// The text column a read span lands in.
pub const SPAN_COLUMN: &str = "span";

/// The result rows kept when the caller states no limit.
const DEFAULT_RAW_K: usize = 10;
/// Time budget of one connector call in milliseconds (contract bounds: 100 to 60000).
const CALL_TIMEOUT_MS: u64 = 30_000;

/// An `access_kind: raw` source resolved to the connector port.
pub(crate) struct RawRemote {
    server: String,
    port: Option<SharedPort>,
    identity: Identity,
}

/// What the calls of one scan have cost so far.
#[derive(Default)]
struct ScanCost {
    bytes: u64,
    calls: u32,
}

/// Resolve `spec` (or the spec a registered name holds) to a raw-access source when its
/// registration declares `raw`; `None` for every other spec.
pub(crate) fn resolve_raw(
    spec: &ForeignSourceSpec,
    registry: Option<&ForeignSourceRegistry>,
) -> Option<RawRemote> {
    let (spec, name) = match spec {
        ForeignSourceSpec::Named { name } => (registry?.spec(name)?, Some(name.as_str())),
        other => (other, None),
    };
    match spec {
        ForeignSourceSpec::Mcp {
            server,
            capabilities,
            ..
        } if capabilities.raw => Some(RawRemote {
            server: server.clone(),
            port: registered_port(),
            identity: Identity::of_spec(spec, name),
        }),
        _ => None,
    }
}

impl RawRemote {
    fn unreachable(&self, why: &str) -> String {
        format!(
            "{}: {}: {why}",
            ServerErrorCode::SourceUnreachable.as_str(),
            self.identity.label
        )
    }

    /// One raw tool call under the budget the query has left. The call is told the unspent
    /// scan bytes; an `OVER_CEILING` answer refuses, and the bytes it reports are charged.
    fn call(
        &self,
        session: &FederationSession,
        tool: &str,
        arguments: BTreeMap<String, Value>,
        cost: &mut ScanCost,
    ) -> Result<RowSet, String> {
        let port = self
            .port
            .as_deref()
            .ok_or_else(|| self.unreachable("no connector port is registered"))?;
        let max_scan_bytes = session.with_meter(|meter| {
            meter.charge_request()?;
            Ok::<u64, String>(meter.remaining_scan_bytes())
        })?;
        let request = RawToolCall {
            source_label: self.identity.label.clone(),
            server: self.server.clone(),
            tool: tool.to_string(),
            arguments,
            max_scan_bytes,
            timeout_ms: CALL_TIMEOUT_MS,
        };
        let answer = port
            .raw_call(&request)
            .map_err(|why| self.unreachable(&why))?;
        if answer.over_ceiling {
            return Err(refusal("scan_bytes", session.budget().max_scan_bytes));
        }
        session.with_meter(|meter| {
            meter.charge_scan_bytes(answer.scan_bytes)?;
            meter.charge_rows(answer.rows.len())
        })?;
        cost.bytes = cost.bytes.saturating_add(answer.scan_bytes);
        cost.calls += 1;
        Ok(answer.rows)
    }
}

/// The cost units `bytes` scanned are worth at `cost_per_kib`: whole units, rounded up.
pub(crate) fn cost_units(bytes: u64, cost_per_kib: f64) -> u64 {
    ((bytes as f64 / 1024.0) * cost_per_kib).ceil() as u64
}

/// `Op::ForeignScan` over a raw-access source: `raw_search` the `RankText` query of the
/// enclosing `FuseRrf`, keep the ids `raw_check_context` says the caller may read (and, as a
/// join, only those already in `input`), `raw_read_span` the kept ids into the text column
/// `span`, and return the rows ranked by their `remote` score. Records one fragment trace
/// carrying the scan's cost report.
pub(crate) fn raw_scan(
    op: &Op,
    input: &RowSet,
    join: bool,
    remote: &RawRemote,
    session: &FederationSession,
) -> Result<RowSet, String> {
    let Some(text) = session.search_text(op) else {
        return Err(format!(
            "{}: {}: a raw source needs a text query; fuse it with a RANK TEXT branch",
            ServerErrorCode::UnsupportedSearchArgument.as_str(),
            remote.identity.label,
        ));
    };
    let k = session.limit_hint(op).unwrap_or(DEFAULT_RAW_K);
    let mut cost = ScanCost::default();
    let found = remote.call(
        session,
        RAW_SEARCH,
        BTreeMap::from([("query".to_string(), json!(text)), ("k".to_string(), json!(k))]),
        &mut cost,
    )?;
    let joined = join && !input.is_empty();
    let keep = input.id_set();
    let candidates: Vec<(String, Option<f32>)> = found
        .rows()
        .iter()
        .filter(|row| !joined || keep.contains(row.id.as_str()))
        .map(|row| (row.id.clone(), row.score))
        .collect();
    let readable = if candidates.is_empty() {
        BTreeSet::new()
    } else {
        let ids: Vec<&str> = candidates.iter().map(|(id, _)| id.as_str()).collect();
        let checked = remote.call(
            session,
            RAW_CHECK_CONTEXT,
            BTreeMap::from([("ids".to_string(), json!(ids))]),
            &mut cost,
        )?;
        checked.ids().into_iter().collect::<BTreeSet<String>>()
    };
    // The caller's own check decides what is read: an id it did not clear is never read.
    let mut kept: Vec<(String, Option<f32>)> = candidates
        .into_iter()
        .filter(|(id, _)| readable.contains(id))
        .collect();
    kept.sort_by(|a, b| match (a.1, b.1) {
        (Some(x), Some(y)) => y.total_cmp(&x).then_with(|| a.0.cmp(&b.0)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.0.cmp(&b.0),
    });
    kept.truncate(k);
    let mut texts = TextChannels::new();
    if !kept.is_empty() {
        let ids: Vec<&str> = kept.iter().map(|(id, _)| id.as_str()).collect();
        let spans = remote.call(
            session,
            RAW_READ_SPAN,
            BTreeMap::from([("ids".to_string(), json!(ids))]),
            &mut cost,
        )?;
        for row in spans.rows() {
            if let Some(span) = spans.text(&row.id, SPAN_COLUMN) {
                texts.insert(
                    row.id.clone(),
                    BTreeMap::from([(SPAN_COLUMN.to_string(), span.to_string())]),
                );
            }
        }
    }
    let channels = kept
        .iter()
        .filter_map(|(id, score)| {
            Some((
                id.clone(),
                BTreeMap::from([(REMOTE_CHANNEL.to_string(), f64::from((*score)?))]),
            ))
        })
        .collect();
    let rows = RowSet::from_rows(kept.iter().cloned())
        .with_values(channels)
        .with_texts(texts);
    let mut trace = FragmentTrace::new(
        remote.identity.label.clone(),
        FetchStrategy::Paged,
        FragmentExecutionMode::Live,
    );
    trace.requests = cost.calls;
    trace.rows_fetched = found.len();
    trace.rows_kept = rows.len();
    trace.raw_scan = Some(RawScanReceipt {
        scan_bytes: cost.bytes,
        cost_units: cost_units(cost.bytes, session.raw_scan_cost_per_kib()),
        calls: cost.calls,
    });
    session.record(trace);
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::federation_opt::mcp::{ConnectorRowSource, RawToolAnswer};
    use medulla_exec::budget::FederationBudget;
    use std::sync::{Arc, Mutex};

    /// A loopback connector: three documents, each search scans 4 KiB, each check 1 KiB and
    /// each span read 2 KiB; it answers `OVER_CEILING` when the bytes left cannot cover one.
    struct Loopback {
        seen: Mutex<Vec<(String, u64)>>,
    }

    impl ConnectorRowSource for Loopback {
        fn declared_arguments(&self, _server: &str, _tool: &str) -> Option<Vec<String>> {
            Some(Vec::new())
        }
        fn rows(
            &self,
            _request: &crate::federation_opt::mcp::ConnectorRowRequest,
        ) -> Result<crate::federation_opt::mcp::ConnectorRows, String> {
            Err("a raw source is read through raw tools".to_string())
        }
        fn raw_call(&self, request: &RawToolCall) -> Result<RawToolAnswer, String> {
            self.seen
                .lock()
                .unwrap()
                .push((request.tool.clone(), request.max_scan_bytes));
            let bytes = match request.tool.as_str() {
                RAW_SEARCH => 4 * 1024,
                RAW_CHECK_CONTEXT => 1024,
                _ => 2 * 1024,
            };
            if bytes > request.max_scan_bytes {
                return Ok(RawToolAnswer {
                    rows: RowSet::new(),
                    scan_bytes: 0,
                    over_ceiling: true,
                });
            }
            let rows = match request.tool.as_str() {
                RAW_SEARCH => RowSet::from_rows([
                    ("d1".to_string(), Some(0.9)),
                    ("d2".to_string(), Some(0.8)),
                    ("d3".to_string(), Some(0.7)),
                ]),
                // The caller may not read d2.
                RAW_CHECK_CONTEXT => RowSet::from_ids(["d1".to_string(), "d3".to_string()]),
                _ => RowSet::from_rows([("d1".to_string(), None), ("d3".to_string(), None)])
                    .with_texts(TextChannels::from([
                        (
                            "d1".to_string(),
                            BTreeMap::from([(SPAN_COLUMN.to_string(), "one".to_string())]),
                        ),
                        (
                            "d3".to_string(),
                            BTreeMap::from([(SPAN_COLUMN.to_string(), "three".to_string())]),
                        ),
                    ])),
            };
            Ok(RawToolAnswer {
                rows,
                scan_bytes: bytes,
                over_ceiling: false,
            })
        }
    }

    fn remote(port: Arc<Loopback>) -> RawRemote {
        let spec = ForeignSourceSpec::Mcp {
            server: "docs-mcp".into(),
            tool: "raw".into(),
            capabilities: medulla_query_types::wire::ForeignOperationCapabilities {
                raw: true,
                ..Default::default()
            },
        };
        RawRemote {
            server: "docs-mcp".into(),
            port: Some(port as SharedPort),
            identity: Identity::of_spec(&spec, None),
        }
    }

    fn scan(session: &FederationSession, remote: &RawRemote) -> Result<RowSet, String> {
        let op = Op::ForeignScan {
            source: ForeignSourceSpec::Named { name: "docs".into() },
            join: false,
        };
        session.bind_search_text(&op, "alpha");
        raw_scan(&op, &RowSet::new(), false, remote, session)
    }

    // spec: MEDULLA-FEDERATED-QUERY-R729
    #[test]
    fn a_raw_scan_reports_its_cost_and_keeps_only_cleared_ids() {
        let port = Arc::new(Loopback {
            seen: Mutex::new(Vec::new()),
        });
        let session = FederationSession::new(FederationBudget::default());
        session.set_raw_scan_cost_per_kib(2.0);
        let rows = scan(&session, &remote(port.clone())).expect("the scan runs");
        assert_eq!(rows.ids(), ["d1", "d3"], "d2 was not cleared by the check");
        assert_eq!(rows.text("d1", SPAN_COLUMN), Some("one"));
        let receipt = session.trace()[0].raw_scan.clone().expect("a cost report");
        // 4 KiB + 1 KiB + 2 KiB over three calls, at two cost units per KiB.
        assert_eq!(
            receipt,
            RawScanReceipt {
                scan_bytes: 7 * 1024,
                cost_units: 14,
                calls: 3
            }
        );
        // Each call is told the bytes the budget has left.
        let seen = port.seen.lock().unwrap().clone();
        let max = FederationBudget::default().max_scan_bytes;
        assert_eq!(seen[0], (RAW_SEARCH.to_string(), max));
        assert_eq!(seen[1], (RAW_CHECK_CONTEXT.to_string(), max - 4 * 1024));
    }

    // spec: MEDULLA-FEDERATED-QUERY-R729
    #[test]
    fn an_over_ceiling_answer_is_the_typed_budget_refusal() {
        let port = Arc::new(Loopback {
            seen: Mutex::new(Vec::new()),
        });
        let session = FederationSession::new(FederationBudget {
            max_scan_bytes: 3 * 1024,
            ..FederationBudget::default()
        });
        let error = scan(&session, &remote(port)).expect_err("4 KiB cannot fit 3 KiB");
        assert!(
            error.starts_with("FEDERATION_BUDGET_EXCEEDED:scan_bytes"),
            "{error}"
        );
        assert!(session.trace().is_empty(), "a refused scan records no cost");
    }

    #[test]
    fn cost_units_round_up_per_kib() {
        assert_eq!(cost_units(0, 1.0), 0);
        assert_eq!(cost_units(1, 1.0), 1);
        assert_eq!(cost_units(1024, 1.0), 1);
        assert_eq!(cost_units(1025, 1.0), 2);
        assert_eq!(cost_units(2048, 100.0), 200);
    }
}
