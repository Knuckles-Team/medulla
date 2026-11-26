//! Source connections and their field mappings (MEDULLA-UNIFIED-DATA-PLANE-R791): the durable,
//! tenant-keyed record of each connection a tenant makes to an external source, and the one
//! mapping proposed and approved for it.
//!
//! A connection names a provider, holds its settings and points at its credential by locator
//! (`env://` or `openbao://`); a credential value is never accepted. A mapping is proposed with a
//! digest and approved against exactly that digest: [`SOURCE_MAPPING_MACHINE`] is the one edge
//! table, and approving a digest that is no longer the stored one refuses `MAPPING_CHANGED`. A
//! connection or mapping of another tenant answers like an absent one, so no read is an existence
//! oracle.
//!
//! Pure data and decisions; the served handler reads the stored nodes the caller can see, calls
//! the `handle_*` functions below and writes the nodes.

use medulla_types::refusal_code::RefusalCode;
use medulla_types::tenant_cursor::CursorFamily;
use medulla_types::transition_refused::{
    transition, Edge, MachineDef, MachineEvent, MachineState, TransitionRefused,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::connector_run::CONNECTION_NOT_FOUND;
use crate::lifecycle::sha256_hex;

/// The graph `type` of a source connection node.
pub const SOURCE_CONNECTION_TYPE: &str = "SourceConnection";
/// The graph `type` of a source mapping node.
pub const SOURCE_MAPPING_TYPE: &str = "SourceMapping";

/// A connection or mapping field is out of bounds, or a cursor is unusable.
pub const SOURCE_CONNECTION_INVALID: &str = "SOURCE_CONNECTION_INVALID";
/// The connection has no mapping yet.
pub const MAPPING_NOT_FOUND: &str = "MAPPING_NOT_FOUND";
/// The digest an approval names is no longer the digest of the stored mapping.
pub const MAPPING_CHANGED: &str = "MAPPING_CHANGED";

/// The stable refusal codes of the source connection methods (`CONNECTION_NOT_FOUND` is the code
/// of [`crate::connector_run`]; a refused transition is `TRANSITION_REFUSED`).
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new(SOURCE_CONNECTION_INVALID),
    RefusalCode::new(MAPPING_NOT_FOUND),
    RefusalCode::new(MAPPING_CHANGED),
];

/// Longest connection id, provider, label or credential locator, in bytes.
pub const MAX_CONNECTION_TEXT_BYTES: usize = 256;
/// Largest settings or mapping document, as canonical JSON, in bytes.
pub const MAX_CONNECTION_DOCUMENT_BYTES: usize = 64 * 1024;
/// Most connections one `SourceConnectionList` page carries, and the page size when none is asked.
pub const MAX_CONNECTION_LIST_LIMIT: u32 = 100;
/// Longest opaque list cursor a caller may hand back.
pub const MAX_CONNECTION_CURSOR_BYTES: usize = 1024;
/// Domain-separation prefix of the `SourceConnectionList` cursor family.
pub const CONNECTION_CURSOR_DOMAIN: &[u8] = b"medulla/source-connection-list-cursor/v1";

const LIST_CURSOR: CursorFamily = CursorFamily {
    domain: CONNECTION_CURSOR_DOMAIN,
    max_bytes: MAX_CONNECTION_CURSOR_BYTES,
    noun: "source connection list",
};

/// A typed refusal of a source connection method.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConnectionRefusal {
    /// A field is out of bounds, or a cursor is not usable.
    #[error("SOURCE_CONNECTION_INVALID: {0}")]
    Invalid(String),
    /// No connection of the verified tenant has the id.
    #[error("CONNECTION_NOT_FOUND: {0}")]
    NotFound(String),
    /// The connection has no mapping.
    #[error("MAPPING_NOT_FOUND: {0}")]
    MappingNotFound(String),
    /// The approved digest is not the stored mapping's digest.
    #[error("MAPPING_CHANGED: {0}")]
    MappingChanged(String),
    /// The mapping machine refuses the event in the mapping's current state.
    #[error("{0}")]
    Transition(TransitionRefused),
}

impl ConnectionRefusal {
    /// The stable refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => SOURCE_CONNECTION_INVALID,
            Self::NotFound(_) => CONNECTION_NOT_FOUND,
            Self::MappingNotFound(_) => MAPPING_NOT_FOUND,
            Self::MappingChanged(_) => MAPPING_CHANGED,
            Self::Transition(_) => medulla_types::transition_refused::TRANSITION_REFUSED,
        }
    }

    /// The one refusal of a read or a control that cannot find its connection.
    pub fn not_found() -> Self {
        Self::NotFound("no connection with this id".to_owned())
    }
}

/// Whether a connection is in use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    /// The connection syncs.
    Active,
    /// The connection is kept but does not sync.
    Paused,
}

impl ConnectionState {
    /// The stored wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
        }
    }

    /// Parse the stored wire name.
    pub fn parse(name: &str) -> Option<Self> {
        serde_json::from_value(Value::String(name.to_owned())).ok()
    }
}

/// What a caller writes for a connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceConnectionInput {
    /// The id the caller gives the connection.
    pub connection_id: String,
    /// The provider the connection reaches.
    pub provider: String,
    /// A name a person reads.
    pub label: String,
    /// The provider settings: a JSON object with no credential value in it.
    #[serde(default)]
    pub settings: Map<String, Value>,
    /// Where the credential lives: `env://NAME` or `openbao://path`. Never a value.
    pub credential_ref: String,
    /// Whether the connection syncs; `active` when absent.
    #[serde(default)]
    pub state: Option<ConnectionState>,
}

/// One stored connection of a tenant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceConnection {
    /// The id of the connection.
    pub connection_id: String,
    /// The verified tenant that owns the connection.
    pub tenant_id: String,
    /// The provider the connection reaches.
    pub provider: String,
    /// A name a person reads.
    pub label: String,
    /// The provider settings.
    pub settings: Map<String, Value>,
    /// Where the credential lives.
    pub credential_ref: String,
    /// Whether the connection syncs.
    pub state: ConnectionState,
}

/// One cursor-paged page of connections, by connection id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceConnectionPage {
    /// The connections of this page.
    pub connections: Vec<SourceConnection>,
    /// The opaque cursor of the next page, absent on the last page.
    #[serde(default)]
    pub next_cursor: Option<String>,
}

/// What `SourceConnectionControl` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceConnectionReceipt {
    /// The connection the control applied to.
    pub connection_id: String,
    /// True for a delete.
    pub deleted: bool,
    /// True when the connection already held exactly this body and nothing was written.
    pub replayed: bool,
}

/// One control operation on a connection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceConnectionOp {
    /// Create a connection or replace the one with this id.
    Put {
        /// The connection body.
        connection: SourceConnectionInput,
    },
    /// Remove a connection and its mapping. Runs it made stay.
    Delete {
        /// The connection to remove.
        connection_id: String,
    },
}

impl SourceConnectionOp {
    /// The wire name of the operation.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Put { .. } => "put",
            Self::Delete { .. } => "delete",
        }
    }

    /// The connection id the operation names.
    pub fn connection_id(&self) -> &str {
        match self {
            Self::Put { connection } => &connection.connection_id,
            Self::Delete { connection_id } => connection_id,
        }
    }
}

/// Where a mapping stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MappingState {
    /// Proposed and waiting for approval.
    Proposed,
    /// Approved at its digest.
    Approved,
}

/// The closed event set of `SourceMappingMachine`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MappingEvent {
    /// A mapping with a new digest is proposed.
    Propose,
    /// The stored mapping is approved.
    Approve,
}

impl MachineState for MappingState {
    fn stored(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Approved => "approved",
        }
    }
}

impl MachineEvent for MappingEvent {
    fn name(self) -> &'static str {
        match self {
            Self::Propose => "Propose",
            Self::Approve => "Approve",
        }
    }
}

/// `SourceMappingMachine`: the closed edge table. Every other pair is refused.
pub static SOURCE_MAPPING_MACHINE: MachineDef<MappingState, MappingEvent, &'static str> =
    MachineDef {
        name: "SourceMappingMachine",
        states: &[MappingState::Proposed, MappingState::Approved],
        initial: MappingState::Proposed,
        terminals: &[],
        events: &[MappingEvent::Propose, MappingEvent::Approve],
        guards: &[],
        edges: &[
            Edge {
                id: "M01",
                from: MappingState::Proposed,
                event: MappingEvent::Propose,
                guard: None,
                to: MappingState::Proposed,
            },
            Edge {
                id: "M02",
                from: MappingState::Proposed,
                event: MappingEvent::Approve,
                guard: None,
                to: MappingState::Approved,
            },
            Edge {
                id: "M03",
                from: MappingState::Approved,
                event: MappingEvent::Propose,
                guard: None,
                to: MappingState::Proposed,
            },
        ],
    };

impl MappingState {
    /// The one transition function of `SourceMappingMachine`.
    pub fn apply(self, event: MappingEvent) -> Result<Self, TransitionRefused> {
        transition(&SOURCE_MAPPING_MACHINE, self, event, |_| false).map(|edge| edge.to)
    }

    /// Parse the stored wire name.
    pub fn parse(name: &str) -> Option<Self> {
        SOURCE_MAPPING_MACHINE.state_from_stored(name)
    }
}

/// A stored mapping with its digest and state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceMapping {
    /// The connection the mapping is for.
    pub connection_id: String,
    /// The mapping document.
    pub mapping: Map<String, Value>,
    /// `sha256:` over the canonical JSON of the mapping.
    pub digest: String,
    /// Where the mapping stands.
    pub state: MappingState,
}

/// One control operation on a mapping.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceMappingOp {
    /// Propose a mapping for the connection.
    Propose {
        /// The connection the mapping is for.
        connection_id: String,
        /// The mapping document.
        mapping: Map<String, Value>,
    },
    /// Approve the stored mapping at exactly this digest.
    Approve {
        /// The connection whose mapping is approved.
        connection_id: String,
        /// The digest the approver reviewed.
        mapping_digest: String,
    },
}

impl SourceMappingOp {
    /// The wire name of the operation.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Propose { .. } => "propose",
            Self::Approve { .. } => "approve",
        }
    }

    /// The connection id the operation names.
    pub fn connection_id(&self) -> &str {
        match self {
            Self::Propose { connection_id, .. } | Self::Approve { connection_id, .. } => {
                connection_id
            }
        }
    }
}

/// What `SourceMappingControl` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceMappingOutcome {
    /// The mapping as it stands after the control.
    pub mapping: Map<String, Value>,
    /// Its digest.
    pub digest: String,
    /// Where it stands.
    pub state: MappingState,
    /// True when the same mapping was already stored in this state and nothing was written.
    pub replayed: bool,
}

fn tenant_key(tenant: &str) -> String {
    sha256_hex(tenant.as_bytes())[..16].to_owned()
}

/// The graph node id of the connection `connection_id` in `tenant`.
pub fn connection_node_id(tenant: &str, connection_id: &str) -> String {
    format!("source_connection:{}:{connection_id}", tenant_key(tenant))
}

/// The graph node id of the mapping of connection `connection_id` in `tenant`.
pub fn mapping_node_id(tenant: &str, connection_id: &str) -> String {
    format!("source_mapping:{}:{connection_id}", tenant_key(tenant))
}

fn invalid<T>(what: &str) -> Result<T, ConnectionRefusal> {
    Err(ConnectionRefusal::Invalid(format!("{what} is out of bounds")))
}

fn id_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn text_ok(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAX_CONNECTION_TEXT_BYTES
        && !value.chars().any(char::is_control)
}

/// Check the bounds of a connection id.
pub fn validate_connection_id(connection_id: &str) -> Result<(), ConnectionRefusal> {
    if id_ok(connection_id) {
        Ok(())
    } else {
        invalid("connection_id")
    }
}

/// Whether `reference` is a credential locator: `env://` or `openbao://` and a non-empty
/// remainder with no whitespace. A value that is not a locator is never kept.
pub fn credential_ref_ok(reference: &str) -> bool {
    ["env://", "openbao://"].iter().any(|scheme| {
        reference.strip_prefix(scheme).is_some_and(|rest| {
            !rest.is_empty() && !rest.chars().any(|c| c.is_whitespace() || c.is_control())
        })
    }) && reference.len() <= MAX_CONNECTION_TEXT_BYTES
}

fn document_ok(document: &Map<String, Value>) -> bool {
    serde_json::to_string(document).is_ok_and(|text| text.len() <= MAX_CONNECTION_DOCUMENT_BYTES)
}

impl SourceConnectionInput {
    /// Check every bound.
    pub fn validate(&self) -> Result<(), ConnectionRefusal> {
        validate_connection_id(&self.connection_id)?;
        if !text_ok(&self.provider) {
            return invalid("provider");
        }
        if !text_ok(&self.label) {
            return invalid("label");
        }
        if !credential_ref_ok(&self.credential_ref) {
            return invalid("credential_ref (an env:// or openbao:// locator, never a value)");
        }
        if !document_ok(&self.settings) {
            return invalid("settings");
        }
        Ok(())
    }
}

impl SourceConnection {
    /// Read the connection a graph node holds. `None` when the node is not a connection of
    /// `tenant` or holds a value the model does not allow.
    pub fn from_node(node: &Value, tenant: &str) -> Option<Self> {
        if node.get("type").and_then(Value::as_str) != Some(SOURCE_CONNECTION_TYPE)
            || node.get("tenant_id").and_then(Value::as_str) != Some(tenant)
        {
            return None;
        }
        let text = |key: &str| node.get(key).and_then(Value::as_str).map(str::to_owned);
        Some(Self {
            connection_id: text("connection_id")?,
            tenant_id: tenant.to_owned(),
            provider: text("provider")?,
            label: text("label")?,
            settings: serde_json::from_str(node.get("settings")?.as_str()?).ok()?,
            credential_ref: text("credential_ref")?,
            state: ConnectionState::parse(&text("state")?)?,
        })
    }
}

impl SourceMapping {
    /// Read the mapping a graph node holds. `None` when the node is not a mapping of `tenant` or
    /// holds a value the model does not allow, or its digest is not its mapping's.
    pub fn from_node(node: &Value, tenant: &str) -> Option<Self> {
        if node.get("type").and_then(Value::as_str) != Some(SOURCE_MAPPING_TYPE)
            || node.get("tenant_id").and_then(Value::as_str) != Some(tenant)
        {
            return None;
        }
        let text = |key: &str| node.get(key).and_then(Value::as_str).map(str::to_owned);
        let mapping: Map<String, Value> = serde_json::from_str(node.get("mapping")?.as_str()?).ok()?;
        let digest = text("digest")?;
        (digest == mapping_digest(&mapping)).then_some(())?;
        Some(Self {
            connection_id: text("connection_id")?,
            mapping,
            digest,
            state: MappingState::parse(&text("state")?)?,
        })
    }
}

/// The digest of a mapping: `sha256:` over its canonical JSON (object keys in order).
pub fn mapping_digest(mapping: &Map<String, Value>) -> String {
    format!(
        "sha256:{}",
        sha256_hex(serde_json::to_string(mapping).unwrap_or_default().as_bytes())
    )
}

fn digest_ok(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// What [`handle_connection_control`] decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionWrite {
    /// The connection to store; `None` for a delete.
    pub connection: Option<SourceConnection>,
    /// True when the connection already held this body, so nothing is written.
    pub replayed: bool,
    /// The receipt of the decision.
    pub receipt: SourceConnectionReceipt,
}

/// `SourceConnectionControl`: decide what one put or delete writes, or refuse before anything is
/// written. `stored` is the connection already held for the id of `tenant`.
pub fn handle_connection_control(
    tenant: &str,
    stored: Option<&SourceConnection>,
    op: SourceConnectionOp,
) -> Result<ConnectionWrite, ConnectionRefusal> {
    validate_connection_id(op.connection_id())?;
    match op {
        SourceConnectionOp::Delete { connection_id } => {
            stored.ok_or_else(ConnectionRefusal::not_found)?;
            Ok(ConnectionWrite {
                connection: None,
                replayed: false,
                receipt: SourceConnectionReceipt {
                    connection_id,
                    deleted: true,
                    replayed: false,
                },
            })
        }
        SourceConnectionOp::Put { connection } => {
            connection.validate()?;
            let next = SourceConnection {
                connection_id: connection.connection_id.clone(),
                tenant_id: tenant.to_owned(),
                provider: connection.provider,
                label: connection.label,
                settings: connection.settings,
                credential_ref: connection.credential_ref,
                state: connection.state.unwrap_or(ConnectionState::Active),
            };
            let replayed = stored == Some(&next);
            Ok(ConnectionWrite {
                receipt: SourceConnectionReceipt {
                    connection_id: next.connection_id.clone(),
                    deleted: false,
                    replayed,
                },
                connection: Some(next),
                replayed,
            })
        }
    }
}

/// `SourceConnectionGet`: the stored connection, or the one not-found refusal.
pub fn handle_connection_get(
    stored: Option<SourceConnection>,
) -> Result<SourceConnection, ConnectionRefusal> {
    stored.ok_or_else(ConnectionRefusal::not_found)
}

/// `SourceConnectionList`: one page of `connections` (every connection of `tenant`), by
/// connection id, resuming after `cursor`. `limit` is 1 to [`MAX_CONNECTION_LIST_LIMIT`].
pub fn handle_connection_list(
    mut connections: Vec<SourceConnection>,
    tenant: &str,
    limit: Option<u32>,
    cursor: Option<&str>,
) -> Result<SourceConnectionPage, ConnectionRefusal> {
    let limit = match limit {
        None => MAX_CONNECTION_LIST_LIMIT as usize,
        Some(0) => return invalid("limit"),
        Some(asked) if asked > MAX_CONNECTION_LIST_LIMIT => return invalid("limit"),
        Some(asked) => asked as usize,
    };
    let after = cursor
        .map(|cursor| {
            LIST_CURSOR
                .decode(tenant, cursor, |key| !key.trim().is_empty())
                .map_err(ConnectionRefusal::Invalid)
        })
        .transpose()?;
    connections.sort_by(|left, right| left.connection_id.cmp(&right.connection_id));
    let rest: Vec<SourceConnection> = connections
        .into_iter()
        .filter(|connection| after.as_ref().is_none_or(|key| connection.connection_id > *key))
        .collect();
    let more = rest.len() > limit;
    let page: Vec<SourceConnection> = rest.into_iter().take(limit).collect();
    let next_cursor = if more {
        page.last()
            .map(|last| LIST_CURSOR.encode(tenant, &last.connection_id))
    } else {
        None
    };
    Ok(SourceConnectionPage {
        connections: page,
        next_cursor,
    })
}

/// `SourceMappingGet`: the stored mapping of a connection that exists. An absent connection is
/// `CONNECTION_NOT_FOUND`; a connection with no mapping is `MAPPING_NOT_FOUND`.
pub fn handle_mapping_get(
    connection: Option<&SourceConnection>,
    stored: Option<SourceMapping>,
) -> Result<SourceMapping, ConnectionRefusal> {
    connection.ok_or_else(ConnectionRefusal::not_found)?;
    stored.ok_or_else(|| ConnectionRefusal::MappingNotFound("no mapping for this connection".into()))
}

/// What [`handle_mapping_control`] decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MappingWrite {
    /// The mapping after the control.
    pub mapping: SourceMapping,
    /// True when nothing is written.
    pub replayed: bool,
}

impl MappingWrite {
    /// The receipt of the decision.
    pub fn receipt(&self) -> SourceMappingOutcome {
        SourceMappingOutcome {
            mapping: self.mapping.mapping.clone(),
            digest: self.mapping.digest.clone(),
            state: self.mapping.state,
            replayed: self.replayed,
        }
    }
}

/// `SourceMappingControl`: decide what one propose or approve writes, or refuse before anything
/// is written. `connection` and `stored` are what is held for the connection id of the tenant.
///
/// Proposing the stored digest again replays (an approved mapping stays approved); proposing any
/// other mapping stores it as proposed. Approving needs the stored mapping and exactly its
/// digest: a different digest refuses `MAPPING_CHANGED`.
pub fn handle_mapping_control(
    connection: Option<&SourceConnection>,
    stored: Option<SourceMapping>,
    op: SourceMappingOp,
) -> Result<MappingWrite, ConnectionRefusal> {
    validate_connection_id(op.connection_id())?;
    connection.ok_or_else(ConnectionRefusal::not_found)?;
    match op {
        SourceMappingOp::Propose {
            connection_id,
            mapping,
        } => {
            if !document_ok(&mapping) {
                return invalid("mapping");
            }
            let digest = mapping_digest(&mapping);
            if let Some(held) = stored.as_ref().filter(|held| held.digest == digest) {
                return Ok(MappingWrite {
                    mapping: held.clone(),
                    replayed: true,
                });
            }
            let from = stored.map_or(MappingState::Proposed, |held| held.state);
            let state = from
                .apply(MappingEvent::Propose)
                .map_err(ConnectionRefusal::Transition)?;
            Ok(MappingWrite {
                mapping: SourceMapping {
                    connection_id,
                    mapping,
                    digest,
                    state,
                },
                replayed: false,
            })
        }
        SourceMappingOp::Approve { mapping_digest, .. } => {
            if !digest_ok(&mapping_digest) {
                return invalid("mapping_digest");
            }
            let held = stored.ok_or_else(|| {
                ConnectionRefusal::MappingNotFound("no mapping for this connection".into())
            })?;
            if held.digest != mapping_digest {
                return Err(ConnectionRefusal::MappingChanged(
                    "the stored mapping is not the one reviewed".into(),
                ));
            }
            if held.state == MappingState::Approved {
                return Ok(MappingWrite {
                    mapping: held,
                    replayed: true,
                });
            }
            let state = held
                .state
                .apply(MappingEvent::Approve)
                .map_err(ConnectionRefusal::Transition)?;
            Ok(MappingWrite {
                mapping: SourceMapping { state, ..held },
                replayed: false,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn input(id: &str, credential: &str) -> SourceConnectionInput {
        SourceConnectionInput {
            connection_id: id.to_owned(),
            provider: "github".to_owned(),
            label: "Main".to_owned(),
            settings: json!({"org": "acme"}).as_object().cloned().expect("object"),
            credential_ref: credential.to_owned(),
            state: None,
        }
    }

    fn put(id: &str) -> SourceConnectionOp {
        SourceConnectionOp::Put {
            connection: input(id, "openbao://kv/github"),
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R791
    #[test]
    fn a_connection_keeps_a_locator_never_a_value_and_put_replays_the_same_body() {
        let made = handle_connection_control("acme", None, put("c1")).expect("put");
        let stored = made.connection.clone().expect("a connection");
        assert_eq!((stored.state, made.replayed), (ConnectionState::Active, false));
        let again = handle_connection_control("acme", Some(&stored), put("c1")).expect("replay");
        assert!(again.replayed && again.receipt.replayed);
        let mut changed = input("c1", "env://GITHUB_TOKEN");
        changed.state = Some(ConnectionState::Paused);
        let replaced = handle_connection_control(
            "acme",
            Some(&stored),
            SourceConnectionOp::Put { connection: changed },
        )
        .expect("replace");
        assert!(!replaced.replayed);
        assert_eq!(replaced.connection.expect("stored").state, ConnectionState::Paused);

        for bad in ["ghp_abcdef0123456789", "env://", "openbao://has space", "https://x"] {
            let refused = handle_connection_control(
                "acme",
                None,
                SourceConnectionOp::Put { connection: input("c1", bad) },
            )
            .expect_err("a value or a bad locator");
            assert_eq!(refused.code(), SOURCE_CONNECTION_INVALID, "{bad}");
        }
        assert!(handle_connection_control("acme", None, put("bad id")).is_err());
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R791
    #[test]
    fn delete_needs_the_connection_and_get_and_list_answer_the_one_not_found() {
        let stored = handle_connection_control("acme", None, put("c1"))
            .expect("put")
            .connection
            .expect("stored");
        let gone = handle_connection_control(
            "acme",
            Some(&stored),
            SourceConnectionOp::Delete { connection_id: "c1".into() },
        )
        .expect("delete");
        assert!(gone.connection.is_none() && gone.receipt.deleted);
        let absent = handle_connection_control(
            "acme",
            None,
            SourceConnectionOp::Delete { connection_id: "c1".into() },
        )
        .expect_err("absent");
        assert_eq!(absent, ConnectionRefusal::not_found());
        assert_eq!(handle_connection_get(None), Err(ConnectionRefusal::not_found()));
        assert_eq!(handle_connection_get(Some(stored.clone())), Ok(stored));
    }

    fn connection(id: &str) -> SourceConnection {
        handle_connection_control("acme", None, put(id))
            .expect("put")
            .connection
            .expect("stored")
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R791
    #[test]
    fn the_list_pages_by_id_with_a_tenant_bound_cursor() {
        let all = vec![connection("b"), connection("a"), connection("c")];
        let first = handle_connection_list(all.clone(), "acme", Some(2), None).expect("page");
        assert_eq!(
            first.connections.iter().map(|c| c.connection_id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        let cursor = first.next_cursor.expect("more");
        let rest = handle_connection_list(all.clone(), "acme", Some(2), Some(&cursor)).expect("page");
        assert_eq!(rest.connections.len(), 1);
        assert!(rest.next_cursor.is_none());
        let foreign = handle_connection_list(all.clone(), "globex", None, Some(&cursor)).expect_err("foreign");
        assert_eq!(foreign.code(), SOURCE_CONNECTION_INVALID);
        assert!(handle_connection_list(all, "acme", Some(0), None).is_err());
    }

    fn propose(mapping: Value) -> SourceMappingOp {
        SourceMappingOp::Propose {
            connection_id: "c1".into(),
            mapping: mapping.as_object().cloned().expect("object"),
        }
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R791
    #[test]
    fn a_mapping_is_proposed_approved_at_its_digest_and_a_stale_digest_refuses() {
        let c = connection("c1");
        let first = handle_mapping_control(Some(&c), None, propose(json!({"title": "name"}))).expect("propose");
        assert_eq!((first.mapping.state, first.replayed), (MappingState::Proposed, false));
        let digest = first.mapping.digest.clone();

        // Approving a digest that is not the stored one refuses; the stored one approves.
        let stale = SourceMappingOp::Approve {
            connection_id: "c1".into(),
            mapping_digest: format!("sha256:{}", "0".repeat(64)),
        };
        let refused = handle_mapping_control(Some(&c), Some(first.mapping.clone()), stale).expect_err("stale");
        assert_eq!(refused.code(), MAPPING_CHANGED);
        let approve = SourceMappingOp::Approve { connection_id: "c1".into(), mapping_digest: digest.clone() };
        let approved = handle_mapping_control(Some(&c), Some(first.mapping.clone()), approve.clone()).expect("approve");
        assert_eq!(approved.mapping.state, MappingState::Approved);
        let again = handle_mapping_control(Some(&c), Some(approved.mapping.clone()), approve).expect("replay");
        assert!(again.replayed);

        // The same proposal keeps an approved mapping approved; a changed one is proposed again.
        let same = handle_mapping_control(Some(&c), Some(approved.mapping.clone()), propose(json!({"title": "name"})))
            .expect("same");
        assert!(same.replayed && same.mapping.state == MappingState::Approved);
        let changed = handle_mapping_control(Some(&c), Some(approved.mapping), propose(json!({"title": "headline"})))
            .expect("changed");
        assert_eq!(changed.mapping.state, MappingState::Proposed);
        assert_ne!(changed.mapping.digest, digest);

        // No connection, no mapping.
        assert_eq!(
            handle_mapping_control(None, None, propose(json!({}))).expect_err("no connection"),
            ConnectionRefusal::not_found()
        );
        let missing = handle_mapping_control(
            Some(&c),
            None,
            SourceMappingOp::Approve { connection_id: "c1".into(), mapping_digest: digest },
        )
        .expect_err("no mapping");
        assert_eq!(missing.code(), MAPPING_NOT_FOUND);
        assert_eq!(handle_mapping_get(Some(&c), None).expect_err("none").code(), MAPPING_NOT_FOUND);
        assert_eq!(handle_mapping_get(None, None).expect_err("none"), ConnectionRefusal::not_found());
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R791
    #[test]
    fn nodes_read_back_only_for_their_tenant_and_a_mapping_must_match_its_digest() {
        let node = json!({
            "type": "SourceConnection", "tenant_id": "acme", "connection_id": "c1",
            "provider": "github", "label": "Main", "settings": "{}",
            "credential_ref": "env://X", "state": "paused",
        });
        assert_eq!(SourceConnection::from_node(&node, "acme").expect("a connection").state, ConnectionState::Paused);
        assert!(SourceConnection::from_node(&node, "globex").is_none());
        let map = json!({"a": 1}).as_object().cloned().expect("object");
        let mapping = json!({
            "type": "SourceMapping", "tenant_id": "acme", "connection_id": "c1",
            "mapping": "{\"a\":1}", "digest": mapping_digest(&map), "state": "approved",
        });
        assert!(SourceMapping::from_node(&mapping, "acme").is_some());
        let mut forged = mapping.clone();
        forged["digest"] = json!(format!("sha256:{}", "1".repeat(64)));
        assert!(SourceMapping::from_node(&forged, "acme").is_none(), "a digest must match its mapping");
        assert!(!connection_node_id("acme", "c1").contains("acme"));
        assert_ne!(mapping_node_id("acme", "c1"), mapping_node_id("globex", "c1"));
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R791
    #[test]
    fn every_legal_edge_is_taken_and_every_other_pair_is_refused() {
        for &state in SOURCE_MAPPING_MACHINE.states {
            for &event in SOURCE_MAPPING_MACHINE.events {
                let edge = SOURCE_MAPPING_MACHINE
                    .edges
                    .iter()
                    .find(|edge| edge.from == state && edge.event == event);
                match (edge, state.apply(event)) {
                    (Some(edge), result) => assert_eq!(result, Ok(edge.to)),
                    (None, Err(refused)) => assert_eq!(refused.machine, "SourceMappingMachine"),
                    (None, Ok(next)) => panic!("{state:?} {event:?} must be refused, took {next:?}"),
                }
            }
        }
    }
}
