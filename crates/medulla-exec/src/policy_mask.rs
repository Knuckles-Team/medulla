//! Column masking and row-filter policies (MEDULLA-FEDERATED-QUERY-R706).
//!
//! ONE policy set, ONE rewrite, every query language: a policy binds to a
//! classification tag (MEDULLA-TYPED-PACKS-R652 markings / R803 detections).
//! [`apply_policies`] rewrites the shared [`Plan`] IR before pushdown — the
//! same rewrite serves UQL, UnifiedQuery, SQL and SPARQL (the served SQL and
//! SPARQL paths reach it through the projection adapters [`sql_read_rewrite`]
//! and [`sparql_read_rewrite`]; `execute` runs it at its top,
//! MEDULLA-FEDERATED-QUERY-R706.3). The rewrite is pure and writes nothing:
//! it mutates only the owned pre-pushdown plan copy and returns the applied
//! policies as evidence records (no masked value ever rides on the record).

use sha2::{Digest, Sha256};

use medulla_query_types::wire::{CmpOp, Op, Plan, Pred, PredLiteral};

use serde::{Deserialize, Serialize};

/// How a masked column value is rewritten. Closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskMethod {
    Null,
    Hash,
    Partial,
    Redact,
}

/// A masking policy on every column carrying `tag` (MEDULLA-FEDERATED-QUERY-R706.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaskPolicy {
    pub tag: String,
    #[serde(default)]
    pub principals_exempt: Vec<String>,
    pub method: MaskMethod,
}

/// A row filter on every table carrying `tag` (MEDULLA-FEDERATED-QUERY-R706.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RowFilterPolicy {
    pub tag: String,
    /// The predicate rows must satisfy, in the shared predicate syntax.
    pub predicate: String,
}

/// What a shape policy does with a result node that does not conform to its shape
/// (MEDULLA-FEDERATED-QUERY-R706.8). Closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShapeEffect {
    /// The non-conforming node leaves the result.
    Filter,
    /// The non-conforming node refuses the whole request.
    Refuse,
}

/// A query policy that is a SHACL shape with a closed effect (MEDULLA-FEDERATED-QUERY-R706.8).
/// The record is a tenant `QueryPolicy` node of kind `shape`; `policy_id` is its node id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShapePolicy {
    pub policy_id: String,
    /// The shape document, Turtle, at most [`SHAPE_TURTLE_MAX_BYTES`] bytes.
    pub shape_turtle: String,
    pub effect: ShapeEffect,
}

/// The size ceiling of one [`ShapePolicy::shape_turtle`]: 64 KiB.
pub const SHAPE_TURTLE_MAX_BYTES: usize = 64 * 1024;

/// The verdict on one result node under the shape policies (MEDULLA-FEDERATED-QUERY-R706.8).
/// Closed: a node conforming to every shape is `allowed`; a non-conforming node under a
/// `filter` effect is `filtered`; a non-conforming node under a `refuse` effect is a
/// `violation`, and one violation refuses the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyOutcome {
    Allowed,
    Filtered,
    Violation,
}

/// The tag-based policies bound to ONE query run (MEDULLA-FEDERATED-QUERY-R706.2).
/// Loaded per tenant (MEDULLA-FEDERATED-QUERY-R706.7); `Default` is the no-policy
/// set, so a run with no loaded policies rewrites nothing — byte-for-byte the
/// unmasked fold.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PolicySet {
    #[serde(default)]
    pub masks: Vec<MaskPolicy>,
    #[serde(default)]
    pub row_filters: Vec<RowFilterPolicy>,
    /// Column name -> the classification tags its promoted claims give it
    /// (MEDULLA-FEDERATED-QUERY-R706.7). A column with no entry carries no tag, so
    /// only a projection that names a tag itself is bound to its policy.
    #[serde(default)]
    pub column_tags: std::collections::BTreeMap<String, Vec<String>>,
    /// The shape policies (MEDULLA-FEDERATED-QUERY-R706.8), in policy node-id order. They
    /// rewrite no plan and no query text: they act on the result nodes
    /// ([`apply_shape_policies`]). An empty list is the set as it was before shapes, and is
    /// left out of the encoding so the digest of such a set is unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shapes: Vec<ShapePolicy>,
}

impl PolicySet {
    /// True when no mask or row-filter policy is bound — the plan and query-text rewrite is a
    /// no-op. Shape policies rewrite no plan; see [`PolicySet::has_shapes`].
    pub fn is_empty(&self) -> bool {
        self.masks.is_empty() && self.row_filters.is_empty()
    }

    /// True when at least one shape policy is bound (MEDULLA-FEDERATED-QUERY-R706.8).
    pub fn has_shapes(&self) -> bool {
        !self.shapes.is_empty()
    }

    /// The mask policy bound to `tag`, if any (one policy per tag). A projected
    /// column is tagged when the plan/projection names it by the classification
    /// tag the loader (R706.7) resolves off the R652 column claims.
    pub fn mask_for(&self, tag: &str) -> Option<&MaskPolicy> {
        self.masks.iter().find(|m| m.tag == tag)
    }

    /// The mask policy that covers `column`: the policy of the tag the column's
    /// promoted claims give it, or of the tag the projection names directly.
    pub fn mask_for_column(&self, column: &str) -> Option<&MaskPolicy> {
        self.mask_for(column).or_else(|| {
            self.column_tags
                .get(column)?
                .iter()
                .find_map(|tag| self.mask_for(tag))
        })
    }

    /// The row-filter policy bound to `tag`, if any.
    pub fn row_filter_for(&self, tag: &str) -> Option<&RowFilterPolicy> {
        self.row_filters.iter().find(|f| f.tag == tag)
    }
}

/// The verified principal a query runs as, matched against
/// [`MaskPolicy::principals_exempt`] (MEDULLA-FEDERATED-QUERY-R706.2). The loader
/// (R706.7) binds the caller's verified principal id; exemption is an EXACT string
/// match — an unbound principal (the empty id) matches no exemption entry, so a
/// bound policy set without a bound principal masks everything (fail closed,
/// never a wildcard).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrincipalRef<'a>(pub &'a str);

impl<'a> From<&'a str> for PrincipalRef<'a> {
    fn from(id: &'a str) -> Self {
        Self(id)
    }
}

/// The `EXPLAIN` evidence record of one applied policy and its kind
/// (MEDULLA-FEDERATED-QUERY-R706.5); the wire types live with `UqlResult`.
pub use medulla_query_types::wire::{AppliedPolicy, PolicyKind};

/// The value-level mask (MEDULLA-FEDERATED-QUERY-R706.2): `null` gives `NULL`;
/// `hash` gives lowercase SHA-256 hex; `partial` keeps the last 4 characters and
/// replaces the rest with `*` (a value of 4 characters or fewer is FULLY masked —
/// a masked value is never returned raw); `redact` gives `[REDACTED]`.
pub fn mask_expr(method: MaskMethod, value: &str) -> String {
    match method {
        MaskMethod::Null => "NULL".to_string(),
        MaskMethod::Hash => sha256_hex(value.as_bytes()),
        MaskMethod::Partial => {
            let n = value.len();
            if n <= 4 {
                "*".repeat(n)
            } else {
                format!("{}{}", "*".repeat(n - 4), &value[n - 4..])
            }
        }
        MaskMethod::Redact => "[REDACTED]".to_string(),
    }
}

/// Lowercase SHA-256 hex — the same digest the medulla-kernel audit chain hashes
/// with (the kernel's `sha2` `Sha256::digest` helper).
fn sha256_hex(input: &[u8]) -> String {
    let digest = Sha256::digest(input);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// The mask expression the PUSHED source query selects instead of the raw column
/// (MEDULLA-FEDERATED-QUERY-R706.3 — the pushed query carries the masked
/// projection, in source SQL form). `NULL` and `'[REDACTED]'` are source literals;
/// `SHA256(col)` / `MASK_PARTIAL(col)` are the pushed-projection function contract
/// the source side of the served row implements.
pub fn mask_column_expr(method: MaskMethod, column: &str) -> String {
    match method {
        MaskMethod::Null => "NULL".to_string(),
        MaskMethod::Hash => format!("SHA256({column})"),
        MaskMethod::Partial => format!("MASK_PARTIAL({column})"),
        MaskMethod::Redact => "'[REDACTED]'".to_string(),
    }
}

/// The projection adapter (MEDULLA-FEDERATED-QUERY-R706.3): one query language's
/// projected column list (a SQL SELECT list, a SPARQL SELECT variable list, the
/// UQL `RETURN` channels) adapted to the shared [`Plan`] IR, so the ONE rewrite
/// [`apply_policies`] serves every language.
pub fn projection_plan(columns: Vec<String>) -> Plan {
    Plan::new(vec![Op::Project { channels: columns }])
}

/// The shared predicate syntax → plan algebra (the R078 tenant predicates join the
/// row filters through this SAME builder, MEDULLA-FEDERATED-QUERY-R706.2). The
/// operator-writable subset: `prop = 'value'`, `prop = "value"`, `prop = number`
/// and `prop <op> number` (`>`, `>=`, `<`, `<=`).
pub fn predicate_from_syntax(text: &str) -> Option<Pred> {
    let text = text.trim();
    for (op_tok, op) in [
        (">=", CmpOp::Ge),
        (">", CmpOp::Gt),
        ("<=", CmpOp::Le),
        ("<", CmpOp::Lt),
    ] {
        if let Some((prop, rest)) = text.split_once(op_tok) {
            if let Ok(n) = rest.trim().parse::<f64>() {
                return Some(Pred::Cmp {
                    prop: prop.trim().to_string(),
                    op,
                    value: PredLiteral::Num(n),
                });
            }
        }
    }
    if let Some((prop, rest)) = text.split_once('=') {
        let (prop, rest) = (prop.trim(), rest.trim());
        let unquoted = rest
            .strip_prefix('\'')
            .and_then(|r| r.strip_suffix('\''))
            .or_else(|| rest.strip_prefix('"').and_then(|r| r.strip_suffix('"')));
        if let Some(value) = unquoted {
            return Some(Pred::Eq {
                prop: prop.to_string(),
                value: value.to_string(),
            });
        }
        if let Ok(n) = rest.parse::<f64>() {
            return Some(Pred::Cmp {
                prop: prop.to_string(),
                op: CmpOp::Eq,
                value: PredLiteral::Num(n),
            });
        }
    }
    None
}

/// The plan rewrite before pushdown (MEDULLA-FEDERATED-QUERY-R706.2).
///
/// Pure and write-free: mutates only `plan` (the owned pre-pushdown copy) and
/// returns the applied policies as evidence records.
/// - Each projected column whose name carries a [`MaskPolicy`] tag takes the mask
///   expression (`Project` channel rewrite), unless `principals_exempt` lists the
///   principal.
/// - Each [`RowFilterPolicy::predicate`] joins by AND — appended to EVERY
///   `Filter` op's predicate list (a row must satisfy all of them), or inserted as
///   its own `Filter` directly after the plan's first (source) op when the plan
///   filters nothing yet. A row filter guards every row the plan touches; the
///   table-scoped narrowing arrives with the R706.7 loader.
///
/// A predicate the rewrite cannot express fails closed: it is recorded as an
/// empty membership list, which matches no row.
pub fn apply_policies(
    plan: &mut Plan,
    set: &PolicySet,
    principal: &PrincipalRef,
) -> Vec<AppliedPolicy> {
    let mut applied = Vec::new();

    for op in plan.ops.iter_mut() {
        if let Op::Project { channels } = op {
            for channel in channels.iter_mut() {
                let Some(mask) = set.mask_for_column(channel) else {
                    continue;
                };
                if mask
                    .principals_exempt
                    .iter()
                    .any(|exempt| *exempt == principal.0)
                {
                    // Exempt: the raw projection stands.
                    continue;
                }
                applied.push(AppliedPolicy {
                    tag: mask.tag.clone(),
                    kind: PolicyKind::Mask,
                    column: channel.clone(),
                });
                *channel = mask_column_expr(mask.method, channel);
            }
        }
    }

    for filter in &set.row_filters {
        let (pred, column) = match predicate_from_syntax(&filter.predicate) {
            Some(pred) => {
                let column = match &pred {
                    Pred::Eq { prop, .. } | Pred::Cmp { prop, .. } | Pred::In { prop, .. } => {
                        prop.clone()
                    }
                    _ => filter.predicate.clone(),
                };
                (pred, column)
            }
            // Fail closed: a predicate the rewrite cannot express admits no rows —
            // an empty membership list matches nothing.
            None => (
                Pred::In {
                    prop: filter.tag.clone(),
                    values: Vec::new(),
                },
                filter.tag.clone(),
            ),
        };
        join_row_filter(plan, pred);
        applied.push(AppliedPolicy {
            tag: filter.tag.clone(),
            kind: PolicyKind::RowFilter,
            column,
        });
    }

    applied
}

/// AND a row-filter predicate into the plan: every existing `Filter` gains it; a
/// plan with no `Filter` gains one directly after its first op (the source op of
/// every canonical plan — a non-source head filters an empty row set, fail
/// closed).
fn join_row_filter(plan: &mut Plan, pred: Pred) {
    if plan.ops.iter().any(|op| matches!(op, Op::Filter { .. })) {
        for op in plan.ops.iter_mut() {
            if let Op::Filter { preds } = op {
                preds.push(pred.clone());
            }
        }
    } else if !plan.ops.is_empty() {
        plan.ops.insert(1, Op::Filter { preds: vec![pred] });
    }
}

/// The node label of a tenant policy record (MEDULLA-FEDERATED-QUERY-R706.7).
pub const QUERY_POLICY_LABEL: &str = "QueryPolicy";

/// The node label of a classification claim record (MEDULLA-TYPED-PACKS-R652).
pub const CLASSIFICATION_CLAIM_LABEL: &str = "ClassificationClaim";

/// The claim state whose markings bind a column to a tag.
const PROMOTED_STATE: &str = "promoted";

/// A policy or claim record that cannot be read as written: the query is
/// refused, never run unmasked (MEDULLA-FEDERATED-QUERY-R706.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyLoadError {
    /// `POLICY_RECORD_INVALID`, or `POLICY_TAG_UNKNOWN` for a policy whose tag has no
    /// registered marking (MEDULLA-FEDERATED-QUERY-R706.4).
    pub code: &'static str,
    /// The node id of the malformed record.
    pub record: String,
    /// The field that failed.
    pub field: &'static str,
}

impl std::fmt::Display for PolicyLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {} ({})", self.code, self.record, self.field)
    }
}

impl std::error::Error for PolicyLoadError {}

fn invalid(record: &str, field: &'static str) -> PolicyLoadError {
    PolicyLoadError {
        code: medulla_types::contract::ServerErrorCode::PolicyRecordInvalid.as_str(),
        record: record.to_string(),
        field,
    }
}

fn string_list(
    object: &serde_json::Value,
    key: &'static str,
    record: &str,
) -> Result<Vec<String>, PolicyLoadError> {
    match object.get(key) {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| invalid(record, key))
            })
            .collect(),
        Some(_) => Err(invalid(record, key)),
    }
}

fn required_str<'v>(
    object: &'v serde_json::Value,
    key: &'static str,
    record: &str,
) -> Result<&'v str, PolicyLoadError> {
    object
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| invalid(record, key))
}

/// A record that names another tenant is not this tenant's; one that names none
/// belongs to the graph it is stored in (the graph is the tenant's).
fn belongs_to(object: &serde_json::Value, tenant: &str) -> bool {
    object
        .get("tenant")
        .and_then(serde_json::Value::as_str)
        .is_none_or(|owner| owner == tenant)
}

/// One `QueryPolicy` record of kind `shape` (MEDULLA-FEDERATED-QUERY-R706.8): `shape_turtle`
/// (at most [`SHAPE_TURTLE_MAX_BYTES`]) and `effect` (`filter` or `refuse`). A shape that is
/// too large or does not parse refuses `POLICY_SHAPE_INVALID`, and the load changes nothing
/// (MEDULLA-FEDERATED-QUERY-R706.10); a missing or unknown field refuses `POLICY_RECORD_INVALID`.
fn shape_policy(id: &str, object: &serde_json::Value) -> Result<ShapePolicy, PolicyLoadError> {
    let shape_turtle = required_str(object, "shape_turtle", id)?.to_string();
    let effect = match required_str(object, "effect", id)? {
        "filter" => ShapeEffect::Filter,
        "refuse" => ShapeEffect::Refuse,
        _ => return Err(invalid(id, "effect")),
    };
    if shape_turtle.len() > SHAPE_TURTLE_MAX_BYTES || shapes_graph(&shape_turtle).is_err() {
        return Err(PolicyLoadError {
            code: medulla_types::contract::ServerErrorCode::PolicyShapeInvalid.as_str(),
            record: id.to_string(),
            field: "shape_turtle",
        });
    }
    Ok(ShapePolicy {
        policy_id: id.to_string(),
        shape_turtle,
        effect,
    })
}

/// The shapes document as an RDF graph. A build without `owl` has no SHACL engine, so it
/// cannot evaluate a shape and refuses one rather than run it unchecked.
#[cfg(feature = "owl")]
fn shapes_graph(turtle: &str) -> Result<medulla_rdf::oxrdf::Graph, String> {
    medulla_shacl::graph_from_turtle(turtle)
}

#[cfg(not(feature = "owl"))]
fn shapes_graph(_turtle: &str) -> Result<(), String> {
    Err("this build has no SHACL engine".to_string())
}

/// Loads the caller's tenant policy set (MEDULLA-FEDERATED-QUERY-R706.7).
///
/// `view` is the tenant graph's own snapshot (never another tenant's). Policies
/// are the `QueryPolicy` records written through `Method::ApplyMutation` under the
/// admin scope: `kind` is `mask` (with `tag`, `method`, optional
/// `principals_exempt`) or `row_filter` (with `tag`, `predicate`). Tag-to-column
/// binding reads the PROMOTED `ClassificationClaim` records: each names its column
/// node (`asset`, `col:<qualified name>`) and its markings (`categories`); a
/// column with no promoted claim carries no tag and is unmasked. Records are
/// read in node-id order, so the set and its digest are deterministic. A malformed
/// record refuses with `POLICY_RECORD_INVALID`; nothing runs unmasked.
pub fn load_policy_set(
    tenant: &str,
    view: &medulla_graph_core::graph::GraphView,
) -> Result<PolicySet, PolicyLoadError> {
    let mut ids: Vec<&String> = view.node_properties.keys().collect();
    ids.sort_unstable();
    let mut set = PolicySet::default();
    // MEDULLA-FEDERATED-QUERY-R706.4: the tenant's registered markings, and the tag each
    // policy names, checked once every record has been read.
    let mut markings: std::collections::BTreeSet<String> = Default::default();
    let mut policy_tags: Vec<(String, String)> = Vec::new();
    for id in ids {
        let Some(blob) = view.node_properties.get(id) else {
            continue;
        };
        let Ok(object) = medulla_types::msgpack::decode_property_value(blob.as_slice()) else {
            continue;
        };
        if !belongs_to(&object, tenant) {
            continue;
        }
        if let Some(registered) = id.strip_prefix(medulla_identity_types::classification::MARKING_NODE_PREFIX) {
            if medulla_graph_core::graph::node_carries_label(
                &object,
                medulla_identity_types::classification::MARKING_LABEL,
            ) {
                markings.insert(registered.to_string());
            }
            continue;
        }
        if medulla_graph_core::graph::node_carries_label(&object, QUERY_POLICY_LABEL)
            && object.get("kind").and_then(serde_json::Value::as_str) == Some("shape")
        {
            // MEDULLA-FEDERATED-QUERY-R706.8: a shape policy binds no tag, so it names no marking.
            set.shapes.push(shape_policy(id, &object)?);
        } else if medulla_graph_core::graph::node_carries_label(&object, QUERY_POLICY_LABEL) {
            let tag = required_str(&object, "tag", id)?.to_string();
            policy_tags.push((id.clone(), tag.clone()));
            match required_str(&object, "kind", id)? {
                "mask" => {
                    let method: MaskMethod = serde_json::from_value(serde_json::Value::String(
                        required_str(&object, "method", id)?.to_string(),
                    ))
                    .map_err(|_| invalid(id, "method"))?;
                    set.masks.push(MaskPolicy {
                        tag,
                        principals_exempt: string_list(&object, "principals_exempt", id)?,
                        method,
                    });
                }
                "row_filter" => set.row_filters.push(RowFilterPolicy {
                    tag,
                    predicate: required_str(&object, "predicate", id)?.to_string(),
                }),
                _ => return Err(invalid(id, "kind")),
            }
        } else if medulla_graph_core::graph::node_carries_label(&object, CLASSIFICATION_CLAIM_LABEL)
        {
            if object.get("state").and_then(serde_json::Value::as_str) != Some(PROMOTED_STATE) {
                continue;
            }
            let asset = required_str(&object, "asset", id)?;
            let Some(qualified) = asset.strip_prefix("col:") else {
                // A claim on a non-column asset binds no column.
                continue;
            };
            let tags = string_list(&object, "categories", id)?;
            let leaf = qualified.rsplit('.').next().unwrap_or(qualified);
            for column in [qualified, leaf] {
                let entry = set.column_tags.entry(column.to_string()).or_default();
                for tag in &tags {
                    if !entry.contains(tag) {
                        entry.push(tag.clone());
                    }
                }
            }
        }
    }
    if let Some((record, _)) = policy_tags.iter().find(|(_, tag)| !markings.contains(tag)) {
        return Err(PolicyLoadError {
            code: medulla_types::contract::ServerErrorCode::PolicyTagUnknown.as_str(),
            record: record.clone(),
            field: "tag",
        });
    }
    Ok(set)
}

/// The counts of the three outcomes over one result (MEDULLA-FEDERATED-QUERY-R706.9).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutcomeCounts {
    pub allowed: u64,
    pub filtered: u64,
    pub violation: u64,
}

/// One evaluation of the shape policies over a result (MEDULLA-FEDERATED-QUERY-R706.11): the
/// policy digest, the principal and the outcome counts. It holds no node value and no node
/// id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyEvaluation {
    pub policy_digest: String,
    pub principal: String,
    pub counts: OutcomeCounts,
}

/// Where one evaluation record goes (MEDULLA-FEDERATED-QUERY-R706.11). The executor calls
/// [`PolicyAuditSink::record`] exactly once per evaluation, whether the request is answered
/// or refused; the served path appends the record to the audit chain.
pub trait PolicyAuditSink: Send + Sync {
    fn record(&self, evaluation: PolicyEvaluation);
}

/// A sink that keeps the records of one request in order, for the caller to append.
#[derive(Debug, Default)]
pub struct PolicyEvaluationLog(std::sync::Mutex<Vec<PolicyEvaluation>>);

impl PolicyEvaluationLog {
    /// The records so far, emptying the log.
    pub fn take(&self) -> Vec<PolicyEvaluation> {
        self.0
            .lock()
            .map(|mut records| std::mem::take(&mut *records))
            .unwrap_or_default()
    }
}

impl PolicyAuditSink for PolicyEvaluationLog {
    fn record(&self, evaluation: PolicyEvaluation) {
        if let Ok(mut records) = self.0.lock() {
            records.push(evaluation);
        }
    }
}

/// The result of the shape policies over one result (MEDULLA-FEDERATED-QUERY-R706.9).
#[derive(Debug)]
pub struct ShapeEvaluation {
    /// The result without the `filtered` nodes, in the input order.
    pub rows: crate::rowset::RowSet,
    pub counts: OutcomeCounts,
    /// The first policy (in set order) a node violated; its presence refuses the request.
    pub violated_policy: Option<String>,
}

/// The refusal of a request a `refuse` shape policy rejected (MEDULLA-FEDERATED-QUERY-R706.10).
/// It names the policy and never a node value.
pub fn violation_refusal(policy_id: &str) -> String {
    format!(
        "{}: policy {policy_id} rejects a result node",
        medulla_types::contract::ServerErrorCode::PolicyViolation.as_str()
    )
}

/// The base IRI shape policies project the graph under: property-graph ids and keys become
/// terms of the core ontology namespace, so a shape names them as `:Claim`, `:name`.
#[cfg(feature = "owl")]
const SHAPE_PROJECTION_BASE: &str = medulla_types::ontology_base::CORE_NAMESPACE;

/// Give each result node one of the three outcomes (MEDULLA-FEDERATED-QUERY-R706.9) by
/// validating the graph against each shape policy with `medulla_shacl::validate`. A node a
/// shape reports is `filtered` under the `filter` effect and a `violation` under `refuse`;
/// any other node is `allowed`. A node with several verdicts takes the worst. The rewrite
/// reads the snapshot and changes no stored data. `Err` only when a shape cannot be
/// evaluated (`POLICY_SHAPE_INVALID`, naming the policy).
#[cfg(feature = "owl")]
pub fn apply_shape_policies(
    rows: crate::rowset::RowSet,
    set: &PolicySet,
    view: &medulla_graph_core::graph::GraphView,
) -> Result<ShapeEvaluation, String> {
    if set.shapes.is_empty() || rows.is_empty() {
        let allowed = rows.len() as u64;
        return Ok(ShapeEvaluation {
            rows,
            counts: OutcomeCounts {
                allowed,
                ..OutcomeCounts::default()
            },
            violated_policy: None,
        });
    }
    let projection = medulla_rdf::sparql::Projection::from_wire(SHAPE_PROJECTION_BASE, "");
    let triples = medulla_rdf::mapping::export_view_projected(view, &projection)
        .map_err(|error| format!("{}: project the graph to RDF: {error}", shape_invalid_code()))?;
    let mut data = medulla_rdf::oxrdf::Graph::new();
    for triple in &triples {
        data.insert(triple);
    }
    // The nodes each shape reports, by projected IRI.
    let mut reported: Vec<std::collections::HashSet<String>> = Vec::new();
    for shape in &set.shapes {
        let invalid = |detail: &str| {
            format!(
                "{}: policy {} {detail}",
                shape_invalid_code(),
                shape.policy_id
            )
        };
        let shapes = shapes_graph(&shape.shape_turtle).map_err(|_| invalid("does not parse"))?;
        let report = medulla_shacl::validate(&shapes, &data)
            .map_err(|_| invalid("cannot be evaluated"))?;
        reported.push(
            report
                .results
                .into_iter()
                .map(|result| result.focus_node)
                .collect(),
        );
    }
    let mut counts = OutcomeCounts::default();
    let mut violated_policy: Option<String> = None;
    let mut kept: std::collections::HashSet<String> = Default::default();
    for row in rows.rows() {
        let iri = projection.node_iri(&row.id);
        let mut outcome = PolicyOutcome::Allowed;
        for (shape, nodes) in set.shapes.iter().zip(&reported) {
            if !nodes.contains(&iri) {
                continue;
            }
            match shape.effect {
                ShapeEffect::Refuse => {
                    outcome = PolicyOutcome::Violation;
                    violated_policy.get_or_insert_with(|| shape.policy_id.clone());
                }
                ShapeEffect::Filter if outcome == PolicyOutcome::Allowed => {
                    outcome = PolicyOutcome::Filtered;
                }
                ShapeEffect::Filter => {}
            }
        }
        match outcome {
            PolicyOutcome::Allowed => {
                counts.allowed += 1;
                kept.insert(row.id.clone());
            }
            PolicyOutcome::Filtered => counts.filtered += 1,
            PolicyOutcome::Violation => counts.violation += 1,
        }
    }
    let keep: std::collections::HashSet<&str> = kept.iter().map(String::as_str).collect();
    Ok(ShapeEvaluation {
        rows: rows.intersect_keep_order(&keep),
        counts,
        violated_policy,
    })
}

#[cfg(feature = "owl")]
fn shape_invalid_code() -> &'static str {
    medulla_types::contract::ServerErrorCode::PolicyShapeInvalid.as_str()
}

/// The digest of a loaded policy set: lowercase SHA-256 hex over its canonical
/// JSON. It joins the cache key and the plan identity of a query, so a changed
/// policy never serves an answer computed under another (MEDULLA-FEDERATED-QUERY-R706.7).
pub fn policy_digest(set: &PolicySet) -> String {
    let canonical = serde_json::to_vec(set).unwrap_or_default();
    sha256_hex(&canonical)
}

/// One SQL SELECT-list entry → (column, optional alias): `col AS alias`, `col
/// alias`, a quoted `"col"` (the tag-named form) or a bare `col`.
fn parse_select_entry(entry: &str) -> (String, Option<String>) {
    let entry = entry.trim();
    let upper = entry.to_ascii_uppercase();
    if let Some(at) = upper.rfind(" AS ") {
        let column = entry[..at].trim();
        let column = column
            .strip_prefix('"')
            .and_then(|c| c.strip_suffix('"'))
            .unwrap_or(column);
        return (column.to_string(), Some(entry[at + 4..].trim().to_string()));
    }
    if entry.len() >= 2 && entry.starts_with('"') && entry.ends_with('"') {
        return (entry[1..entry.len() - 1].to_string(), None);
    }
    let parts: Vec<&str> = entry.split_whitespace().collect();
    if parts.len() == 2 {
        return (parts[0].to_string(), Some(parts[1].to_string()));
    }
    (entry.to_string(), None)
}

/// The SQL projection adapter (MEDULLA-FEDERATED-QUERY-R706.3): a read SQL
/// statement's top-level SELECT list, run through the ONE rewrite [`apply_policies`].
/// Returns the pushed query — the masked projection in the SELECT list, the
/// row-filter predicates AND-joined into the WHERE (inserted before the `FROM`
/// when none is present) — plus the applied policies as evidence. Refuses (the
/// error is a refusal string) when a bound row filter cannot be spliced into the
/// shape: fail closed — a read with an inexpressible row filter never runs raw.
pub fn sql_read_rewrite(
    query: &str,
    set: &PolicySet,
    principal: &PrincipalRef,
) -> Result<(String, Vec<AppliedPolicy>), String> {
    if set.is_empty() {
        return Ok((query.to_string(), Vec::new()));
    }
    let upper = query.to_ascii_uppercase();
    let Some(select_at) = upper.find("SELECT ") else {
        return Err(
            "POLICY_REWRITE_UNSUPPORTED_QUERY: no top-level SELECT projection to mask".to_string(),
        );
    };
    let list_start = select_at + "SELECT ".len();
    let Some(from_at) = upper[list_start..].find(" FROM ").map(|i| list_start + i) else {
        return Err(
            "POLICY_REWRITE_UNSUPPORTED_QUERY: no top-level SELECT … FROM projection to mask"
                .to_string(),
        );
    };
    let select_list = &query[list_start..from_at];
    if select_list.contains('(') || select_list.contains('*') {
        return Err(
            "POLICY_REWRITE_UNSUPPORTED_QUERY: the SELECT list is not a plain column list"
                .to_string(),
        );
    }
    let entries: Vec<String> = select_list
        .split(',')
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect();
    let columns: Vec<String> = entries.iter().map(|e| parse_select_entry(e).0).collect();
    let mut plan = projection_plan(columns);
    let applied = apply_policies(&mut plan, set, principal);
    let channels = match &plan.ops[0] {
        Op::Project { channels } => channels.clone(),
        _ => unreachable!("projection_plan emits exactly one Project"),
    };
    // Splice the (possibly masked) entries back, keeping each entry's alias.
    let new_list: Vec<String> = entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let rewritten = &channels[i];
            if rewritten == entry {
                entry.clone()
            } else {
                let alias = parse_select_entry(entry).1;
                match alias {
                    Some(alias) => format!("{rewritten} AS {alias}"),
                    None => rewritten.clone(),
                }
            }
        })
        .collect();
    // The row filters AND-join the existing WHERE clause (inserted right after
    // the `FROM …` table reference when the statement has none).
    let where_splice: Option<(usize, String)> = if set.row_filters.is_empty() {
        None
    } else {
        let clause = set
            .row_filters
            .iter()
            .map(|f| format!("({})", f.predicate.trim()))
            .collect::<Vec<_>>()
            .join(" AND ");
        let tail = &upper[from_at..];
        let where_pos = tail.find(" WHERE ").map(|i| from_at + i);
        let clause_end = [" GROUP BY ", " HAVING ", " ORDER BY ", " LIMIT "]
            .iter()
            .filter_map(|k| tail.find(k))
            .min()
            .map(|i| from_at + i)
            .unwrap_or(query.len());
        let mut at = clause_end;
        while at > from_at {
            match query.as_bytes().get(at - 1) {
                Some(b' ' | b'\t' | b'\n' | b'\r' | b';') => at -= 1,
                _ => break,
            }
        }
        let text = match where_pos {
            Some(_) => format!(" AND {clause}"),
            None => format!(" WHERE {clause}"),
        };
        Some((at, text))
    };
    let mut out = String::with_capacity(query.len());
    out.push_str(&query[..list_start]);
    out.push_str(&new_list.join(", "));
    match where_splice {
        Some((at, text)) => {
            out.push_str(&query[from_at..at]);
            out.push_str(&text);
            out.push_str(&query[at..]);
        }
        None => out.push_str(&query[from_at..]),
    }
    Ok((out, applied))
}

/// The SPARQL projection adapter (MEDULLA-FEDERATED-QUERY-R706.3): a SPARQL
/// `SELECT … WHERE …` statement's projection, run through the ONE rewrite
/// [`apply_policies`]. A row filter splices in as a `FILTER (…)` (the shared
/// predicate syntax is SPARQL-compatible once single quotes become double). A
/// COLUMN MASK cannot be expressed inline in SPARQL 1.1, so a tagged projection
/// is refused — fail closed, until the value-level boundary of the served row
/// lands. An empty policy set returns the query byte-for-byte.
pub fn sparql_read_rewrite(
    query: &str,
    set: &PolicySet,
    principal: &PrincipalRef,
) -> Result<(String, Vec<AppliedPolicy>), String> {
    if set.is_empty() {
        return Ok((query.to_string(), Vec::new()));
    }
    let upper = query.to_ascii_uppercase();
    let Some(select_at) = upper.find("SELECT ") else {
        return Err(
            "POLICY_REWRITE_UNSUPPORTED_QUERY: no SPARQL SELECT projection to mask".to_string(),
        );
    };
    let list_start = select_at + "SELECT ".len();
    let Some(where_rel) = upper[list_start..].find(" WHERE ") else {
        return Err(
            "POLICY_REWRITE_UNSUPPORTED_QUERY: no SPARQL SELECT … WHERE projection to mask"
                .to_string(),
        );
    };
    let vars: Vec<String> = query[list_start..list_start + where_rel]
        .split_whitespace()
        .map(|v| v.to_string())
        .collect();
    let mut plan = projection_plan(vars.clone());
    let applied = apply_policies(&mut plan, set, principal);
    // A SPARQL variable names a masked column when it equals a mask tag or the
    // tag's first path segment (?pii ← pii.email): the query text cannot resolve
    // the variable → column binding, so the served boundary fails closed instead
    // of projecting the raw value.
    let var_masked = set.masks.iter().any(|m| {
        m.principals_exempt
            .iter()
            .all(|exempt| principal.0 != exempt)
            && vars.iter().any(|v| {
                let name = v.strip_prefix('?').unwrap_or(v.as_str());
                m.tag == name || m.tag.starts_with(&format!("{name}."))
            })
    });
    if applied.iter().any(|a| a.kind == PolicyKind::Mask) || var_masked {
        return Err(
            "POLICY_SPARQL_MASK_UNSUPPORTED: SPARQL 1.1 cannot express the masked \
             projection inline; a tagged projection is refused until the value-level \
             boundary of MEDULLA-FEDERATED-QUERY-R706.3 lands"
                .to_string(),
        );
    }
    let filters: Vec<String> = set
        .row_filters
        .iter()
        .map(|f| format!("FILTER ({})", f.predicate.trim().replace('\'', "\"")))
        .collect();
    if filters.is_empty() {
        return Ok((query.to_string(), applied));
    }
    let insert_at = list_start + where_rel + " WHERE ".len();
    let mut out = query.to_string();
    out.insert_str(insert_at, &format!("{} ", filters.join(" ")));
    Ok((out, applied))
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-FEDERATED-QUERY-R706.1
    #[test]
    fn policies_round_trip() {
        for method in [
            MaskMethod::Null,
            MaskMethod::Hash,
            MaskMethod::Partial,
            MaskMethod::Redact,
        ] {
            let mask = MaskPolicy {
                tag: "pii.email".into(),
                principals_exempt: vec!["role:dpo".into()],
                method,
            };
            let json = serde_json::to_string(&mask).unwrap();
            assert_eq!(serde_json::from_str::<MaskPolicy>(&json).unwrap(), mask);
        }
        assert!(serde_json::from_str::<MaskMethod>("\"encrypt\"").is_err());
        let filter = RowFilterPolicy {
            tag: "region.eu".into(),
            predicate: "region = 'eu'".into(),
        };
        let json = serde_json::to_string(&filter).unwrap();
        assert_eq!(
            serde_json::from_str::<RowFilterPolicy>(&json).unwrap(),
            filter
        );
        assert!(serde_json::from_str::<RowFilterPolicy>(r#"{"tag":"t"}"#).is_err());
    }

    // spec: MEDULLA-FEDERATED-QUERY-R706.2
    #[test]
    fn mask_expr_covers_the_closed_set() {
        assert_eq!(mask_expr(MaskMethod::Null, "john@example.com"), "NULL");
        assert_eq!(
            mask_expr(MaskMethod::Redact, "john@example.com"),
            "[REDACTED]"
        );
        assert_eq!(
            mask_expr(MaskMethod::Partial, "password-123"),
            "********-123"
        );
        assert_eq!(mask_expr(MaskMethod::Partial, "ab"), "**");
        assert_eq!(mask_expr(MaskMethod::Partial, "abcd"), "****");
        // lowercase SHA-256 hex of "john@example.com"
        assert_eq!(
            mask_expr(MaskMethod::Hash, "john@example.com"),
            "855f96e983f1f8e8be944692b6f719fd54329826cb62e98015efee8e2e071dd4"
        );
    }

    // spec: MEDULLA-FEDERATED-QUERY-R706.2
    #[test]
    fn apply_policies_masks_projection_and_and_joins_row_filters() {
        let mut plan = Plan::new(vec![
            Op::Scan {
                label: "Person".into(),
            },
            Op::Filter {
                preds: vec![Pred::Eq {
                    prop: "tenant".into(),
                    value: "acme".into(),
                }],
            },
            Op::Project {
                channels: vec![
                    "pii.email".into(),
                    "pii.ssn".into(),
                    "pii.notes".into(),
                    "plain".into(),
                ],
            },
        ]);
        let set = PolicySet {
            masks: vec![
                MaskPolicy {
                    tag: "pii.email".into(),
                    principals_exempt: Vec::new(),
                    method: MaskMethod::Null,
                },
                MaskPolicy {
                    tag: "pii.ssn".into(),
                    principals_exempt: Vec::new(),
                    method: MaskMethod::Hash,
                },
                MaskPolicy {
                    tag: "pii.notes".into(),
                    principals_exempt: vec!["role:dpo".into()],
                    method: MaskMethod::Redact,
                },
            ],
            row_filters: vec![RowFilterPolicy {
                tag: "region.eu".into(),
                predicate: "region = 'eu'".into(),
            }],
            ..PolicySet::default()
        };
        let principal = PrincipalRef::from("role:dpo");
        let applied = apply_policies(&mut plan, &set, &principal);

        let masks: Vec<&AppliedPolicy> = applied
            .iter()
            .filter(|a| a.kind == PolicyKind::Mask)
            .collect();
        assert_eq!(masks.len(), 2, "the exempt principal is not masked");
        assert!(applied
            .iter()
            .any(|a| a.kind == PolicyKind::RowFilter && a.tag == "region.eu"));

        let channels = match &plan.ops[2] {
            Op::Project { channels } => channels.clone(),
            _ => panic!("projection lost"),
        };
        assert_eq!(
            channels,
            vec!["NULL", "SHA256(pii.ssn)", "pii.notes", "plain"]
        );
        let preds = match &plan.ops[1] {
            Op::Filter { preds } => preds.clone(),
            _ => panic!("filter lost"),
        };
        assert_eq!(
            preds.len(),
            2,
            "the row filter AND-joins the tenant predicate"
        );
        assert!(matches!(&preds[1], Pred::Eq { prop, value } if prop == "region" && value == "eu"));
    }

    fn view_of(nodes: &[(&str, serde_json::Value)]) -> medulla_graph_core::graph::GraphView {
        let mut view = medulla_graph_core::graph::GraphView::default();
        for (id, props) in nodes {
            let blob = rmp_serde::to_vec_named(props).expect("encode");
            view.node_properties
                .insert((*id).to_string(), std::sync::Arc::new(blob));
        }
        view
    }

    fn mask_record(tag: &str, tenant: &str) -> serde_json::Value {
        serde_json::json!({
            "node_type": "QueryPolicy", "kind": "mask", "tag": tag,
            "method": "redact", "principals_exempt": ["role:dpo"], "tenant": tenant,
        })
    }

    fn claim_record(asset: &str, state: &str, tag: &str) -> serde_json::Value {
        serde_json::json!({
            "node_type": "ClassificationClaim", "asset": asset,
            "state": state, "categories": [tag],
        })
    }

    fn marking_record(tenant: &str) -> serde_json::Value {
        serde_json::json!({"node_type": "Marking", "tenant": tenant})
    }

    // spec: MEDULLA-FEDERATED-QUERY-R706.7
    #[test]
    fn loader_binds_policies_to_promoted_claim_columns_only() {
        let view = view_of(&[
            ("marking:pii.email", marking_record("acme")),
            ("marking:region.eu", marking_record("acme")),
            ("p1", mask_record("pii.email", "acme")),
            (
                "p2",
                serde_json::json!({"node_type": "QueryPolicy", "kind": "row_filter",
                    "tag": "region.eu", "predicate": "region = 'eu'"}),
            ),
            (
                "c1",
                claim_record("col:people.email", "promoted", "pii.email"),
            ),
            (
                "c2",
                claim_record("col:people.phone", "proposed", "pii.email"),
            ),
            (
                "c3",
                claim_record("col:people.notes", "rejected", "pii.email"),
            ),
        ]);
        let set = load_policy_set("acme", &view).expect("loads");
        assert_eq!(set.masks.len(), 1);
        assert_eq!(set.row_filters.len(), 1);
        assert!(set.mask_for_column("email").is_some(), "leaf name is bound");
        assert!(set.mask_for_column("people.email").is_some());
        assert!(
            set.mask_for_column("phone").is_none(),
            "a proposed claim binds nothing"
        );
        assert!(
            set.mask_for_column("notes").is_none(),
            "a rejected claim binds nothing"
        );
        assert!(
            set.mask_for_column("unclaimed").is_none(),
            "a column with no claim is unmasked"
        );
    }

    // spec: MEDULLA-FEDERATED-QUERY-R706.7
    #[test]
    fn loader_never_crosses_tenants() {
        let view = view_of(&[
            ("marking:pii.email", marking_record("acme")),
            ("p1", mask_record("pii.email", "other")),
            (
                "c1",
                claim_record("col:people.email", "promoted", "pii.email"),
            ),
        ]);
        let set = load_policy_set("acme", &view).expect("loads");
        assert!(
            set.masks.is_empty(),
            "another tenant's policy is not loaded"
        );
    }

    // spec: MEDULLA-FEDERATED-QUERY-R706.7
    #[test]
    fn loader_refuses_malformed_records_and_digest_tracks_the_set() {
        let bad = view_of(&[(
            "p1",
            serde_json::json!({"node_type": "QueryPolicy", "kind": "mask",
                "tag": "pii.email", "method": "scramble"}),
        )]);
        let error = load_policy_set("acme", &bad).expect_err("unknown method refuses");
        assert_eq!(error.code, "POLICY_RECORD_INVALID");
        assert_eq!(error.record, "p1");

        let one = view_of(&[
            ("marking:pii.email", marking_record("acme")),
            ("p1", mask_record("pii.email", "acme")),
        ]);
        let two = view_of(&[
            ("marking:pii.email", marking_record("acme")),
            ("marking:pii.ssn", marking_record("acme")),
            ("p1", mask_record("pii.email", "acme")),
            ("p2", mask_record("pii.ssn", "acme")),
        ]);
        let a = policy_digest(&load_policy_set("acme", &one).unwrap());
        let b = policy_digest(&load_policy_set("acme", &two).unwrap());
        assert_ne!(a, b);
        assert_eq!(a, policy_digest(&load_policy_set("acme", &one).unwrap()));
    }

    // spec: MEDULLA-FEDERATED-QUERY-R706.4
    #[test]
    fn loader_refuses_a_policy_naming_an_unregistered_tag() {
        // A policy-first write: no marking node for its tag.
        let first = view_of(&[
            ("p1", mask_record("pii.email", "acme")),
            ("p2", mask_record("pii.ssn", "acme")),
            ("marking:pii.ssn", marking_record("acme")),
        ]);
        let error = load_policy_set("acme", &first).expect_err("an unregistered tag refuses");
        assert_eq!(error.code, "POLICY_TAG_UNKNOWN");
        assert_eq!((error.record.as_str(), error.field), ("p1", "tag"));

        // Registering the marking, in any node order, lets the same policy load.
        let registered = view_of(&[
            ("p1", mask_record("pii.email", "acme")),
            ("marking:pii.email", marking_record("acme")),
        ]);
        assert_eq!(load_policy_set("acme", &registered).unwrap().masks.len(), 1);

        // Another tenant's marking does not register the tag; a node that is not labelled
        // `Marking` does not either.
        let foreign = view_of(&[
            ("p1", mask_record("pii.email", "acme")),
            ("marking:pii.email", marking_record("other")),
        ]);
        assert_eq!(load_policy_set("acme", &foreign).unwrap_err().code, "POLICY_TAG_UNKNOWN");
        let unlabelled = view_of(&[
            ("p1", mask_record("pii.email", "acme")),
            ("marking:pii.email", serde_json::json!({"node_type": "Other", "tenant": "acme"})),
        ]);
        assert_eq!(load_policy_set("acme", &unlabelled).unwrap_err().code, "POLICY_TAG_UNKNOWN");
    }

    // spec: MEDULLA-FEDERATED-QUERY-R706.8
    #[test]
    fn shape_policy_round_trips_and_empty_set_is_identity() {
        let shape = ShapePolicy {
            policy_id: "policy:no-drafts".into(),
            shape_turtle: "@prefix sh: <http://www.w3.org/ns/shacl#> .".into(),
            effect: ShapeEffect::Refuse,
        };
        let json = serde_json::to_value(&shape).unwrap();
        assert_eq!(json["effect"], "refuse");
        assert_eq!(serde_json::from_value::<ShapePolicy>(json).unwrap(), shape);
        for outcome in [
            PolicyOutcome::Allowed,
            PolicyOutcome::Filtered,
            PolicyOutcome::Violation,
        ] {
            let text = serde_json::to_string(&outcome).unwrap();
            assert_eq!(serde_json::from_str::<PolicyOutcome>(&text).unwrap(), outcome);
        }

        // A set with no shape encodes without the field, so its digest is the one it had
        // before shapes existed; one with a shape differs.
        let empty = PolicySet::default();
        assert!(serde_json::to_value(&empty).unwrap().get("shapes").is_none());
        let shaped = PolicySet {
            shapes: vec![shape],
            ..PolicySet::default()
        };
        assert!(shaped.has_shapes() && !empty.has_shapes());
        assert_ne!(policy_digest(&shaped), policy_digest(&empty));
        let back: PolicySet = serde_json::from_value(serde_json::to_value(&shaped).unwrap()).unwrap();
        assert_eq!(back, shaped);

        // A shape rewrites no plan: the plan stays as lowered and nothing is applied.
        let mut plan = Plan::new(vec![Op::Project {
            channels: vec!["email".into()],
        }]);
        let before = plan.clone();
        assert!(apply_policies(&mut plan, &shaped, &PrincipalRef::from("role:analyst")).is_empty());
        assert_eq!(plan, before);

        // A shape policy loads from its record, and binds no marking.
        let loaded = load_policy_set(
            "acme",
            &view_of(&[(
                "policy:shape",
                serde_json::json!({
                    "node_type": "QueryPolicy", "kind": "shape", "effect": "filter",
                    "shape_turtle": "@prefix sh: <http://www.w3.org/ns/shacl#> .",
                }),
            )]),
        );
        if cfg!(feature = "owl") {
            let loaded = loaded.unwrap();
            assert_eq!(loaded.shapes.len(), 1);
            assert_eq!(loaded.shapes[0].policy_id, "policy:shape");
            assert_eq!(loaded.shapes[0].effect, ShapeEffect::Filter);
        } else {
            assert_eq!(loaded.unwrap_err().code, "POLICY_SHAPE_INVALID");
        }
    }
}
