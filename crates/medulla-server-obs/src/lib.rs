//! medulla-server-obs: Observability, traces, telemetry and QoS.
//!
//! Level F10 in `scripts/crate_split/boundaries.toml` (MEDULLA-CONTRACT-R056.11); it may depend only
//! on crates of a lower level. Budget: 25,000 lines of Rust. Specs that land here: none yet.
//! Route new rows with `scripts/crate_split/route_rows.py`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

/// One HTTP/1.1 request-framing authority for every hand-rolled listener in
/// this tree (`obs`, `sparql_http`, `federation`, `viz_interactive`,
/// `policy_export`, …). Before it each surface carried its own reader and the
/// copies disagreed about `Transfer-Encoding`, duplicate headers, header
/// bounds and body framing; see the module doc.
#[cfg(any(
    feature = "obs",
    feature = "sparql-http",
    feature = "federation-search",
    feature = "viz-interactive",
    feature = "policy_export",
    feature = "graphql",
    feature = "kvcache-server",
    feature = "lake",
    feature = "s3-api",
))]
/// Observability log ingestion + Parquet segment substrate (CONCEPT:AU-KG.ingest.self-ingest/161,
/// feature `obs`): a hand-rolled HTTP listener accepting OTLP/HTTP, Elasticsearch
/// `_bulk`/`_doc` and JSON-lines log records, landing them in medulla-tsdb series +
/// medulla-text full-text indices and rolling Parquet-on-blob-CAS segments — the first
/// slice of Phase T (surpass OpenObserve). Self-contained (its own `ObsState`), not
/// tied to the graph `ServerState`.
#[cfg(feature = "obs")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod obs;
/// PromQL + the Prometheus-compatible HTTP query API (CONCEPT:MEDULLA-KG.query.prometheus-http-query-api, feature
/// `promql`): `/api/v1/query[_range]` + `/labels` served on the obs listener over the
/// durable medulla-tsdb series, backed by the pure-Rust `medulla_tsdb::promql` engine.
#[cfg(feature = "promql")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod promql;
// Real-time QoS / SLO-aware admission scheduler (CONCEPT:MEDULLA-KG.coordination.backpressure-busy-signal). An additive, opt-in
// gate (enabled by `MEDULLA_QOS`) the transport runs BEFORE the baseline
// admission: priority-class preemption + per-tenant fair-share + hard quotas +
// deadline-aware ordering + typed backpressure. With QoS unconfigured the transport never
// touches it and the baseline path is byte-for-byte unchanged.
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod qos;
/// Distributed traces: OTLP-JSON span ingest + Jaeger/OpenObserve trace-search,
/// assembly and service-dependency-graph API (CONCEPT:MEDULLA-OS.observability.trace-assembly, feature `traces`),
/// served on the obs listener over the pure-Rust `medulla_tsdb::traces` span store —
/// completing the observability trilogy (logs + metrics + traces).
#[cfg(feature = "traces")]
#[allow(missing_docs)] // docs pending: moved facade module (doc-fill lane removes this)
pub mod traces;
