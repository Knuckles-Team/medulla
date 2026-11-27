use super::*;

/// Wire mirror of `medulla_epistemic::RankedResult` (EPI-P3-3) — one ranked candidate:
/// the final blended score plus its components, kept separate for explainability.
#[cfg(feature = "epistemic")]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RankedResultWire {
    pub id: String,
    pub score: f64,
    pub similarity: f64,
    pub evidence_quality: f64,
    /// The governed read of this candidate's source (MEDULLA-DECISION-ENGINE-R118.1.2.3);
    /// `InsufficientHistory` means the reliability component abstained (counted as 0.0).
    pub source_reliability: medulla_decision_types::decision::reputation::SourceReliability,
}

/// Materialized result of a `Method::RankByProvenance` run, highest score first.
/// Returned via `ResultPayload::raw`.
#[cfg(feature = "epistemic")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RankByProvenanceResult {
    pub ranked: Vec<RankedResultWire>,
}

/// Graph type for multi-tenant registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum GraphType {
    Agent,
    Team,
    Global,
    Commons,
}

/// Channel type for dynamic communication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ChannelType {
    /// 1:1 direct messaging between two agents.
    PeerToPeer,
    /// Many-to-many group channel.
    Group,
}

// ── Response ────────────────────────────────────────────────────────────

/// Untagged result payload for efficient serialization without JSON overhead.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ResultPayload {
    Bool(bool),
    Count(u64),
    Float(f64),
    String(String),
    Ids(Vec<String>),
    NodeList(Vec<(String, serde_json::Value)>),
    EdgeList(Vec<(String, String, Vec<u8>)>),
    /// A typed result serialized STRAIGHT to MessagePack (Phase C-D — compact
    /// result encoding). Skips building a `serde_json::Value` tree on the server —
    /// the dominant allocator for large algorithm results (PageRank/centrality/
    /// communities over the whole graph). On the wire it is a MessagePack `bin`.
    /// The Python client decodes any
    /// top-level `bytes` result with a second `unpackb`, recovering the exact same
    /// structure the `Json` path produced. Opaque-byte methods are explicitly
    /// identified by method at the client boundary and never decoded a second time.
    Raw(
        #[cfg_attr(feature = "contract-schema", schemars(with = "Vec<u8>"))]
        #[serde(with = "serde_bytes")]
        Vec<u8>,
    ),
    Json(serde_json::Value),
}

impl ResultPayload {
    /// Encode a typed value straight to MessagePack as a [`ResultPayload::Raw`],
    /// bypassing the `serde_json::Value` tree (the dominant allocator for large
    /// algorithm results). The compact encoding is the ONE wire contract — clients
    /// decode a top-level `bytes` result with a second `unpackb`; there is no
    /// alternate encoding flag. (Phase C-D)
    pub(crate) fn raw<T: Serialize + ?Sized>(value: &T) -> Result<Self, String> {
        rmp_serde::to_vec_named(value)
            .map(ResultPayload::Raw)
            .map_err(|error| format!("INTERNAL: result serialization failed: {error}"))
    }
}

/// Input accepted by [`Response::ok`].
///
/// Compact encoding is deliberately fallible: a serializer failure becomes an
/// error response and can never masquerade as a successful empty byte string.
#[doc(hidden)]
pub trait IntoResponsePayload {
    fn into_response_payload(self) -> Result<ResultPayload, String>;
}

impl IntoResponsePayload for ResultPayload {
    fn into_response_payload(self) -> Result<ResultPayload, String> {
        Ok(self)
    }
}

impl IntoResponsePayload for Result<ResultPayload, String> {
    fn into_response_payload(self) -> Result<ResultPayload, String> {
        self
    }
}

/// Response envelope sent back to the Python client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// Correlation ID matching the request.
    pub id: u64,
    /// Result payload on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<ResultPayload>,
    /// Stable declared error code on failure. Unclassified legacy refusals
    /// become INTERNAL without exposing their original text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Human-readable detail for a declared error code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_detail: Option<String>,
    /// The four fields of a refused state-machine transition
    /// (MEDULLA-ORCH-VOCAB-R811.9). Present only when `error` is `TRANSITION_REFUSED`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_refused: Option<medulla_types::transition_refused::TransitionRefusedWire>,
    /// The structured detail of a refusal, tagged by its code (MEDULLA-FEDERATED-QUERY-R040.2.4).
    /// Present only for a refusal that declares a variant of `ErrorDetail`. The
    /// human-readable text stays in `error_detail`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_data: Option<medulla_types::retrieval::ErrorDetail>,
}

/// The first declared code inside `text` that a caller's context wraps:
/// `"<context>: CODE: detail"` or `"<context>: CODE"`. Returns the code and the
/// detail with the context kept ahead of the refusal's own text.
fn embedded_declared_code(text: &str) -> Option<(String, String)> {
    let mut start = 0;
    while let Some(offset) = text[start..].find(": ") {
        let segment_start = start + offset + 2;
        let rest = &text[segment_start..];
        let (code, tail) = match rest.split_once(": ") {
            Some((code, tail)) => (code, Some(tail)),
            None => (rest, None),
        };
        // `INTERNAL` is never lifted out of free text: its detail is what the
        // unclassified path below keeps off the wire.
        if code != medulla_types::contract::ServerErrorCode::Internal.as_str()
            && crate::error_registry::declared_error_code(code)
        {
            let context = &text[..start + offset];
            let detail = match tail {
                Some(tail) => format!("{context}: {tail}"),
                None => context.to_string(),
            };
            return Some((code.to_string(), detail));
        }
        start = segment_start;
    }
    None
}

/// The fixed head of the detail that an uncoded refusal carries on the wire.
pub const UNCLASSIFIED_DETAIL: &str = "unclassified engine refusal";

/// Count of uncoded refusals this process reported as `INTERNAL`. The served-test
/// harness reads it to fail a test that collapsed a typed refusal
/// (MEDULLA-OBSERVE-R001.3).
pub static UNCLASSIFIED_REFUSALS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// The same count by the crate that reported the refusal, behind the series
/// `medulla_refusal_unclassified_total{module}` (MEDULLA-OBSERVE-R001.3). The label is a
/// crate name, so it is bounded by the workspace and never names a request value.
static UNCLASSIFIED_BY_MODULE: std::sync::Mutex<std::collections::BTreeMap<String, u64>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// The crate a source file belongs to: `crates/<name>/...` is `<name>`, any other path is the
/// root package.
fn module_of(file: &str) -> &str {
    file.strip_prefix("crates/")
        .and_then(|rest| rest.split('/').next())
        .unwrap_or("medulla-engine")
}

/// Uncoded refusals reported since the process started, by reporting crate, ascending by name.
pub fn unclassified_by_module() -> Vec<(String, u64)> {
    UNCLASSIFIED_BY_MODULE
        .lock()
        .map(|counts| counts.iter().map(|(module, n)| (module.clone(), *n)).collect())
        .unwrap_or_default()
}

/// One uncoded refusal a test harness can read back.
#[cfg(feature = "test-support")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnclassifiedRefusal {
    /// The id of the request the refusal answered.
    pub request_id: u64,
    /// The cause reference the wire answer carries.
    pub cause: CauseRef,
    /// The crate that reported it.
    pub module: String,
}

/// The newest uncoded refusals, bounded; kept only in a test-support build.
#[cfg(feature = "test-support")]
static UNCLASSIFIED_LOG: std::sync::Mutex<std::collections::VecDeque<UnclassifiedRefusal>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());

/// The most uncoded refusals the test-support log keeps.
#[cfg(feature = "test-support")]
const UNCLASSIFIED_LOG_CAPACITY: usize = 4096;

/// The uncoded refusals reported for any request id `wanted` accepts, oldest first.
#[cfg(feature = "test-support")]
pub fn unclassified_refusals(wanted: impl Fn(u64) -> bool) -> Vec<UnclassifiedRefusal> {
    UNCLASSIFIED_LOG
        .lock()
        .map(|log| {
            log.iter()
                .filter(|entry| wanted(entry.request_id))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// A short, stable reference that ties a wire `INTERNAL` to the one WARN log
/// line holding the full refusal text (MEDULLA-OBSERVE-R001.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CauseRef(u32);

impl CauseRef {
    /// The reference of `text`: the low 32 bits of its FNV-1a hash.
    pub fn of(text: &str) -> Self {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in text.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Self((hash ^ (hash >> 32)) as u32)
    }
}

impl std::fmt::Display for CauseRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cause-{:08x}", self.0)
    }
}

impl Response {
    /// Create a successful response.
    pub fn ok(id: u64, result: impl IntoResponsePayload) -> Self {
        match result.into_response_payload() {
            Ok(result) => Response {
                id,
                result: Some(result),
                error: None,
                error_detail: None,
                transition_refused: None,
                error_data: None,
            },
            Err(error) => Response::err(id, error),
        }
    }

    /// Create an error response.
    #[track_caller]
    pub fn err(id: u64, error: impl Into<String>) -> Self {
        let text = error.into();
        let (error, error_detail) = match text.split_once(": ") {
            Some((code, detail)) if crate::error_registry::declared_error_code(code) => {
                (code.to_string(), Some(detail.to_string()))
            }
            _ if crate::error_registry::declared_error_code(&text) => (text, None),
            // A declared refusal a caller wrapped in its own context
            // ("<context>: CODE: detail") keeps its code; the context stays
            // in the detail ahead of the refusal's own text.
            _ => match embedded_declared_code(&text) {
                Some((code, detail)) => (code, Some(detail)),
                None => {
                    // The wire never carries uncoded text (it may name request values).
                    // The full text stays in the operator log, tied to the wire answer
                    // by a cause reference (MEDULLA-OBSERVE-R001.2).
                    let cause = CauseRef::of(&text);
                    UNCLASSIFIED_REFUSALS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let module = module_of(std::panic::Location::caller().file()).to_string();
                    #[cfg(feature = "test-support")]
                    if let Ok(mut log) = UNCLASSIFIED_LOG.lock() {
                        if log.len() >= UNCLASSIFIED_LOG_CAPACITY {
                            log.pop_front();
                        }
                        log.push_back(UnclassifiedRefusal {
                            request_id: id,
                            cause,
                            module: module.clone(),
                        });
                    }
                    if let Ok(mut counts) = UNCLASSIFIED_BY_MODULE.lock() {
                        *counts.entry(module).or_insert(0) += 1;
                    }
                    tracing::warn!(
                        target: "medulla::refusal",
                        request_id = id,
                        cause_ref = %cause,
                        refusal = %text,
                        "unclassified engine refusal reported as INTERNAL"
                    );
                    (
                        medulla_types::contract::ServerErrorCode::Internal
                            .as_str()
                            .to_string(),
                        Some(format!("{UNCLASSIFIED_DETAIL} ({cause})")),
                    )
                }
            },
        };
        let transition_refused = (error == medulla_types::transition_refused::TRANSITION_REFUSED)
            .then(|| error_detail.as_deref())
            .flatten()
            .and_then(medulla_types::transition_refused::TransitionRefused::from_detail)
            .map(Into::into);
        let error_data = error_detail
            .as_deref()
            .and_then(|detail| medulla_types::retrieval::ErrorDetail::from_refusal(&error, detail));
        Response {
            id,
            result: None,
            error: Some(error),
            error_detail,
            transition_refused,
            error_data,
        }
    }

    /// The refusal folded back into the `"CODE: detail"` text `Response::err`
    /// accepts, so re-wrapping a refused response keeps its detail. `None`
    /// when the response carries no error.
    pub fn refusal_text(&self) -> Option<String> {
        let code = self.error.as_deref()?;
        Some(match self.error_detail.as_deref() {
            Some(detail) => format!("{code}: {detail}"),
            None => code.to_string(),
        })
    }

    /// Schema-generated placement redirect used when this node cannot serve the
    /// graph's current `(group, epoch)`.
    pub fn stale_route(
        id: u64,
        graph: &str,
        group: u64,
        epoch: u64,
        leader: Option<u64>,
        _reason: impl Into<String>,
    ) -> Self {
        use medulla_result_types::{
            epistemic_operations::OperationRedirect, epistemic_operations::OperationRedirectKind,
            epistemic_operations::OperationResult,
            epistemic_operations::OperationResultSchemaVersion,
            epistemic_operations::OperationResultOutcome,
        };

        let detail = OperationResult {
            schema_version: OperationResultSchemaVersion::V1,
            operation_id: format!("request:{id}"),
            outcome: OperationResultOutcome::Redirected,
            result_kind: None,
            result_ref: None,
            error: None,
            redirect: Some(OperationRedirect {
                kind: OperationRedirectKind::Placement,
                target_ref: graph.to_string(),
                group,
                epoch,
                fencing_token: group,
                leader_ref: leader.map(|node| format!("node:{node}")),
            }),
        };
        match ResultPayload::raw(&detail) {
            Ok(result) => Response {
                id,
                result: Some(result),
                error: Some("OPERATION_REDIRECTED".to_string()),
                error_detail: None,
                transition_refused: None,
                error_data: None,
            },
            Err(error) => Response::err(id, error),
        }
    }
}
