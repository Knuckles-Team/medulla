//! The logical-replication connection (MEDULLA-DURABLE-KERNEL-R035.7.2).
//!
//! A connection whose startup parameter is `replication=database` is handed here by
//! `message_loop::serve_connection` once login completes. `serve_walsender` alone speaks the
//! copy-both framing: an ordinary connection keeps refusing that tag through
//! `UNSUPPORTED_MESSAGES`. It serves three commands, `IDENTIFY_SYSTEM`,
//! `CREATE_REPLICATION_SLOT <name> LOGICAL pgoutput` (which writes the R035.7.1 slot row) and
//! `START_REPLICATION SLOT <name> LOGICAL <lsn>`, which pages `Method::ReadGraphChanges`
//! (MEDULLA-DURABLE-KERNEL-R043.100) from the slot's `confirmed_cursor`, maps every change to
//! `pgoutput` messages and advances `confirmed_cursor` on each standby status update.
//!
//! Every engine call goes through the router's one dispatch chokepoint: the change feed as the
//! login's own principal (`WireSession::dispatch_method`), the slot row as the fixed engine
//! service of `pg_replication::REPLICATION_SLOT_AGENT` (`WireSession::dispatch_service_method`),
//! the only identity the reserved slot namespace admits. The wire crate never links the graph
//! handler.
//!
//! ## Mapping
//!
//! The feed carries identities, not row images, so every relation has one text column holding
//! them. A user-table row change becomes one `Insert`, `Update` or `Delete` on `public.<table>`
//! with the single column `key` (the primary key rendered as text). A node or edge written by a
//! commit is an `Insert` on `public.graph_nodes` or `public.graph_edges` with the single column
//! `id`; a tombstoned id is a `Delete` on `public.graph_nodes`. Each feed event is one
//! transaction: `Begin`, a `Relation` the first time a relation is used, its row messages, then
//! `Commit`. The stream numbers its own LSNs, one per event, from the LSN the client asked to
//! start at.

use std::collections::{HashSet, VecDeque};
use std::io;
use std::sync::Arc;

use medulla_mutation_types::graph_change_feed::{
    GraphChange, GraphChangeEvent, GraphChangePage, RowOperation,
};
use medulla_mutation_types::pg_replication::{
    slot_key, ReplicationSlotRow, REPLICATION_SLOT_NAMESPACE, REPLICATION_SLOT_SERVICE,
};
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_protocol_types::storage_wire::{KvCasOutcome, KvEntry};
use medulla_wire_core::wire::WireSession;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::{Duration, Instant};

/// The outbound bytes the stream may hold unconfirmed before it stops reading the feed and waits
/// for the standby to confirm (64 MiB).
pub(super) const MAX_UNCONFIRMED_BYTES: usize = 64 * 1024 * 1024;

/// The most scanned events one `ReadGraphChanges` page asks for.
const FEED_PAGE: u32 = 256;

/// How long the stream waits before it polls an idle feed again.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// How often an idle stream sends a keepalive.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);

/// The largest frame the walsender accepts from the client.
const MAX_CLIENT_FRAME: usize = 1024 * 1024;

/// The scope a replication login must hold.
const REPLICATION_SCOPE: &str = "replication:read";

/// The scopes the slot-row service identity holds: it reads and writes shared KV rows only.
const SLOT_SERVICE_SCOPES: &[&str] = &["kv:read", "kv:write", "kv:shared:write"];

/// Microseconds between the Unix epoch and the PostgreSQL epoch (2000-01-01).
const POSTGRES_EPOCH_OFFSET_US: i64 = 946_684_800_000_000;

/// The OID of `text` and its type length.
const TEXT_OID: u32 = 25;
/// The OID of `int4` and its type length.
const INT4_OID: u32 = 23;

// -- framing --------------------------------------------------------------------------------

/// One backend frame: the tag, a length that counts itself, the body.
fn frame(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + body.len());
    out.push(tag);
    out.extend_from_slice(&((body.len() + 4) as u32).to_be_bytes());
    out.extend_from_slice(body);
    out
}

fn push_cstring(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(text.as_bytes());
    out.push(0);
}

/// An `ErrorResponse` carrying `sqlstate` and `message`.
pub(super) fn error_response(sqlstate: &str, message: &str) -> Vec<u8> {
    let mut body = Vec::new();
    for (field, text) in [
        (b'S', "ERROR"),
        (b'V', "ERROR"),
        (b'C', sqlstate),
        (b'M', message),
    ] {
        body.push(field);
        push_cstring(&mut body, text);
    }
    body.push(0);
    frame(b'E', &body)
}

fn ready_for_query() -> Vec<u8> {
    frame(b'Z', b"I")
}

fn command_complete(tag: &str) -> Vec<u8> {
    let mut body = Vec::new();
    push_cstring(&mut body, tag);
    frame(b'C', &body)
}

/// A `RowDescription` of text-format columns, each `(name, type oid, type length)`.
fn row_description(columns: &[(&str, u32, i16)]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&(columns.len() as i16).to_be_bytes());
    for (name, oid, length) in columns {
        push_cstring(&mut body, name);
        body.extend_from_slice(&0_i32.to_be_bytes());
        body.extend_from_slice(&0_i16.to_be_bytes());
        body.extend_from_slice(&oid.to_be_bytes());
        body.extend_from_slice(&length.to_be_bytes());
        body.extend_from_slice(&(-1_i32).to_be_bytes());
        body.extend_from_slice(&0_i16.to_be_bytes());
    }
    frame(b'T', &body)
}

fn data_row(values: &[String]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&(values.len() as i16).to_be_bytes());
    for value in values {
        body.extend_from_slice(&(value.len() as i32).to_be_bytes());
        body.extend_from_slice(value.as_bytes());
    }
    frame(b'D', &body)
}

/// The `CopyBothResponse` that opens the stream: text format, no columns.
fn copy_both_response() -> Vec<u8> {
    frame(b'W', &[0, 0, 0])
}

fn copy_data(payload: &[u8]) -> Vec<u8> {
    frame(b'd', payload)
}

fn copy_done() -> Vec<u8> {
    frame(b'c', &[])
}

/// One WAL data message: `w`, the start and end LSN, the send time and the `pgoutput` payload.
fn wal_data(start_lsn: u64, end_lsn: u64, sent_us: i64, payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(25 + payload.len());
    body.push(b'w');
    body.extend_from_slice(&start_lsn.to_be_bytes());
    body.extend_from_slice(&end_lsn.to_be_bytes());
    body.extend_from_slice(&sent_us.to_be_bytes());
    body.extend_from_slice(payload);
    copy_data(&body)
}

/// A primary keepalive: `k`, the WAL end, the send time and whether a reply is requested.
fn keepalive(wal_end: u64, sent_us: i64, reply_requested: bool) -> Vec<u8> {
    let mut body = Vec::with_capacity(18);
    body.push(b'k');
    body.extend_from_slice(&wal_end.to_be_bytes());
    body.extend_from_slice(&sent_us.to_be_bytes());
    body.push(u8::from(reply_requested));
    copy_data(&body)
}

/// The current time in microseconds since the PostgreSQL epoch.
fn now_us() -> i64 {
    let unix_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_micros() as i64);
    unix_us - POSTGRES_EPOCH_OFFSET_US
}

/// Take one complete frontend frame off the front of `buffer`: its tag and body. `Ok(None)` while
/// the frame has not fully arrived.
fn take_frame(buffer: &mut Vec<u8>) -> io::Result<Option<(u8, Vec<u8>)>> {
    if buffer.len() < 5 {
        return Ok(None);
    }
    let declared = i32::from_be_bytes([buffer[1], buffer[2], buffer[3], buffer[4]]);
    let length = usize::try_from(declared)
        .ok()
        .filter(|length| (4..=MAX_CLIENT_FRAME).contains(length))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "replication frame length is invalid",
            )
        })?;
    if buffer.len() < 1 + length {
        return Ok(None);
    }
    let tag = buffer[0];
    let body = buffer[5..1 + length].to_vec();
    buffer.drain(..1 + length);
    Ok(Some((tag, body)))
}

// -- pgoutput -------------------------------------------------------------------------------

/// The relation id of `name`: a stable non-zero 32-bit hash.
fn relation_id(name: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in name.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash.max(1)
}

/// A `Begin` message.
fn pgoutput_begin(final_lsn: u64, commit_us: i64, xid: u32) -> Vec<u8> {
    let mut out = vec![b'B'];
    out.extend_from_slice(&final_lsn.to_be_bytes());
    out.extend_from_slice(&commit_us.to_be_bytes());
    out.extend_from_slice(&xid.to_be_bytes());
    out
}

/// A `Commit` message.
fn pgoutput_commit(commit_lsn: u64, end_lsn: u64, commit_us: i64) -> Vec<u8> {
    let mut out = vec![b'C', 0];
    out.extend_from_slice(&commit_lsn.to_be_bytes());
    out.extend_from_slice(&end_lsn.to_be_bytes());
    out.extend_from_slice(&commit_us.to_be_bytes());
    out
}

/// A `Relation` message of one text column that is the replica identity key.
fn pgoutput_relation(relation: u32, namespace: &str, name: &str, column: &str) -> Vec<u8> {
    let mut out = vec![b'R'];
    out.extend_from_slice(&relation.to_be_bytes());
    push_cstring(&mut out, namespace);
    push_cstring(&mut out, name);
    out.push(b'd');
    out.extend_from_slice(&1_i16.to_be_bytes());
    out.push(1);
    push_cstring(&mut out, column);
    out.extend_from_slice(&TEXT_OID.to_be_bytes());
    out.extend_from_slice(&(-1_i32).to_be_bytes());
    out
}

/// One text tuple of a single column.
fn pgoutput_tuple(value: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(7 + value.len());
    out.extend_from_slice(&1_i16.to_be_bytes());
    out.push(b't');
    out.extend_from_slice(&(value.len() as i32).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
    out
}

/// An `Insert`, `Update` or `Delete` message of one single-column row. `Delete` names the old key
/// tuple with `K`; the other two name the new tuple with `N`.
fn pgoutput_row(kind: u8, relation: u32, value: &str) -> Vec<u8> {
    let mut out = vec![kind];
    out.extend_from_slice(&relation.to_be_bytes());
    out.push(if kind == b'D' { b'K' } else { b'N' });
    out.extend(pgoutput_tuple(value));
    out
}

/// What one row message of an event does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Insert,
    Update,
    Delete,
}

impl RowKind {
    fn tag(self) -> u8 {
        match self {
            Self::Insert => b'I',
            Self::Update => b'U',
            Self::Delete => b'D',
        }
    }
}

/// The relation and the rows one feed change touches: `(table, column, kind, value)`.
fn change_rows(change: &GraphChange) -> Vec<(String, &'static str, RowKind, String)> {
    match change {
        GraphChange::Upsert { node_ids, edge_ids } => node_ids
            .iter()
            .map(|id| ("graph_nodes".to_string(), "id", RowKind::Insert, id.clone()))
            .chain(
                edge_ids
                    .iter()
                    .map(|id| ("graph_edges".to_string(), "id", RowKind::Insert, id.clone())),
            )
            .collect(),
        GraphChange::Tombstone { deleted_ids, .. } => deleted_ids
            .iter()
            .map(|id| ("graph_nodes".to_string(), "id", RowKind::Delete, id.clone()))
            .collect(),
        GraphChange::Row {
            table,
            operation,
            key,
        } => vec![(
            table.clone(),
            "key",
            match operation {
                RowOperation::Insert => RowKind::Insert,
                RowOperation::Update => RowKind::Update,
                RowOperation::Delete => RowKind::Delete,
            },
            key.clone(),
        )],
    }
}

/// Maps feed events to `pgoutput` messages, numbering the stream's LSNs.
pub(super) struct PgOutputMapper {
    announced: HashSet<u32>,
    next_lsn: u64,
}

impl PgOutputMapper {
    /// A mapper whose first event is numbered `start_lsn` (at least 1).
    pub(super) fn new(start_lsn: u64) -> Self {
        Self {
            announced: HashSet::new(),
            next_lsn: start_lsn.max(1),
        }
    }

    /// The LSN the next event will carry.
    pub(super) fn next_lsn(&self) -> u64 {
        self.next_lsn
    }

    /// The `pgoutput` messages of one event, in order: `Begin`, a `Relation` the first time a
    /// relation appears, the row messages, `Commit`. Returns the event's LSN with them.
    pub(super) fn map_event(&mut self, event: &GraphChangeEvent) -> (u64, Vec<Vec<u8>>) {
        let lsn = self.next_lsn;
        self.next_lsn += 1;
        let commit_us = (event.committed_at_ms as i64)
            .saturating_mul(1000)
            .saturating_sub(POSTGRES_EPOCH_OFFSET_US);
        let xid = relation_id(&event.commit_id);
        let mut messages = vec![pgoutput_begin(lsn, commit_us, xid)];
        for (table, column, kind, value) in change_rows(&event.change) {
            let relation = relation_id(&format!("public.{table}"));
            if self.announced.insert(relation) {
                messages.push(pgoutput_relation(relation, "public", &table, column));
            }
            messages.push(pgoutput_row(kind.tag(), relation, &value));
        }
        messages.push(pgoutput_commit(lsn, lsn + 1, commit_us));
        (lsn, messages)
    }
}

// -- commands -------------------------------------------------------------------------------

/// One command of the replication grammar.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Command {
    IdentifySystem,
    CreateSlot { name: String },
    StartReplication { name: String, lsn: u64 },
    Unsupported(String),
}

/// Parse an `X/Y` hexadecimal LSN.
fn parse_lsn(text: &str) -> Option<u64> {
    let (high, low) = text.split_once('/')?;
    let high = u64::from_str_radix(high, 16).ok()?;
    let low = u64::from_str_radix(low, 16).ok()?;
    (high <= u64::from(u32::MAX) && low <= u64::from(u32::MAX)).then_some((high << 32) | low)
}

/// The text of an LSN as `X/Y`.
fn format_lsn(lsn: u64) -> String {
    format!("{:X}/{:X}", lsn >> 32, lsn & 0xFFFF_FFFF)
}

/// Parse one replication command. Keywords are case-insensitive; a slot name is kept as written.
pub(super) fn parse_command(text: &str) -> Command {
    let text = text.trim().trim_end_matches(';').trim();
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let keyword = |index: usize| tokens.get(index).map(|token| token.to_ascii_uppercase());
    match keyword(0).as_deref() {
        Some("IDENTIFY_SYSTEM") if tokens.len() == 1 => Command::IdentifySystem,
        Some("CREATE_REPLICATION_SLOT")
            if tokens.len() == 4
                && keyword(2).as_deref() == Some("LOGICAL")
                && keyword(3).as_deref() == Some("PGOUTPUT") =>
        {
            Command::CreateSlot {
                name: tokens[1].to_string(),
            }
        }
        Some("START_REPLICATION")
            if tokens.len() >= 5
                && keyword(1).as_deref() == Some("SLOT")
                && keyword(3).as_deref() == Some("LOGICAL") =>
        {
            match parse_lsn(tokens[4]) {
                Some(lsn) => Command::StartReplication {
                    name: tokens[2].to_string(),
                    lsn,
                },
                None => Command::Unsupported(text.to_string()),
            }
        }
        _ => Command::Unsupported(text.to_string()),
    }
}

// -- the slot row ---------------------------------------------------------------------------

/// Read the typed body of a response, or the refusal text it carries.
fn response_body<T: serde::de::DeserializeOwned>(response: Response) -> Result<T, String> {
    if let Some(refusal) = response.refusal_text() {
        return Err(refusal);
    }
    match response.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes)
            .map_err(|error| format!("INTERNAL: replication reply is unreadable: {error}")),
        Some(ResultPayload::Json(value)) => serde_json::from_value(value)
            .map_err(|error| format!("INTERNAL: replication reply is unreadable: {error}")),
        _ => Err("INTERNAL: replication reply carries no result".to_string()),
    }
}

/// The slot rows of one verified tenant, read and written through the dispatch seam.
struct SlotStore<'a> {
    session: &'a WireSession,
    tenant: String,
}

impl SlotStore<'_> {
    async fn call(&self, method: Method) -> Result<Response, String> {
        self.session
            .dispatch_service_method(REPLICATION_SLOT_SERVICE, SLOT_SERVICE_SCOPES, method)
            .await
            .map_err(|error| format!("{}: {}", error.code, error.message))
    }

    /// The row of slot `name` and the version it was stored at, `None` when no such slot exists.
    async fn read(&self, name: &str) -> Result<Option<(ReplicationSlotRow, u64)>, String> {
        let key = slot_key(&self.tenant, name)?;
        let response = self
            .call(Method::KvGet {
                namespace: REPLICATION_SLOT_NAMESPACE.to_string(),
                key,
                shared: true,
            })
            .await?;
        let entry: Option<KvEntry> = response_body(response)?;
        entry
            .map(|entry| {
                let row: ReplicationSlotRow = rmp_serde::from_slice(&entry.value)
                    .map_err(|error| format!("INTERNAL: slot row is unreadable: {error}"))?;
                Ok((row, entry.version))
            })
            .transpose()
    }

    /// Store `row` when the slot is at `expected_version` (`0` for a new slot); the version after
    /// the write, or the refusal text.
    async fn write(&self, row: &ReplicationSlotRow, expected_version: u64) -> Result<u64, String> {
        let key = slot_key(&self.tenant, &row.name)?;
        let bytes = rmp_serde::to_vec_named(row).map_err(|error| error.to_string())?;
        let response = self
            .call(Method::KvCas {
                namespace: REPLICATION_SLOT_NAMESPACE.to_string(),
                key,
                expected_version,
                new: Some(bytes),
                shared: true,
            })
            .await?;
        let outcome: KvCasOutcome = response_body(response)?;
        if outcome.swapped {
            Ok(outcome.version)
        } else {
            Err("CONFLICT: the replication slot moved under this connection".to_string())
        }
    }
}

// -- the walsender --------------------------------------------------------------------------

/// What one standby status update confirms: the flushed LSN.
fn status_update_flushed(body: &[u8]) -> Option<u64> {
    // `d` frames carry one inner message: `r`, written, flushed, applied, time, reply.
    if body.first() != Some(&b'r') || body.len() < 34 {
        return None;
    }
    Some(u64::from_be_bytes(body[9..17].try_into().ok()?))
}

/// The cursor a page confirms once the standby has flushed through its last LSN.
struct PendingConfirm {
    end_lsn: u64,
    cursor: String,
    bytes: usize,
}

/// Why the stream ended.
enum StreamEnd {
    /// The client ended the stream or the connection.
    Client,
    /// The stream ended with this refusal text.
    Refused(String),
}

/// The SQLSTATE of a refusal text: `55000` for an expired cursor, `XX000` otherwise.
fn refusal_sqlstate(refusal: &str) -> &'static str {
    if refusal.starts_with("CURSOR_EXPIRED") {
        "55000"
    } else {
        "XX000"
    }
}

/// Serve one replication connection after login. `pending` holds the bytes the startup framing
/// already read past the login. A login without scope `replication:read` is refused with
/// SQLSTATE `42501` before any command runs.
pub(super) async fn serve_walsender<S>(
    io: S,
    pending: Vec<u8>,
    session: Arc<WireSession>,
) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut reader, mut writer) = tokio::io::split(io);
    let allowed = matches!(session.holds_scope(REPLICATION_SCOPE).await, Ok(true));
    if !allowed {
        writer
            .write_all(&error_response(
                "42501",
                "ACCESS_DENIED: the login lacks scope replication:read",
            ))
            .await?;
        return writer.flush().await;
    }
    let tenant = match session.carrier_authority() {
        Ok(authority) => authority.log_owner().0.to_string(),
        Err(error) => {
            writer
                .write_all(&error_response(&error.code, &error.message))
                .await?;
            return writer.flush().await;
        }
    };
    let slots = SlotStore {
        session: &session,
        tenant,
    };
    let mut inbound = pending;
    let mut chunk = vec![0_u8; 8192];
    loop {
        let (tag, body) = loop {
            if let Some(next) = take_frame(&mut inbound)? {
                break next;
            }
            let read = reader.read(&mut chunk).await?;
            if read == 0 {
                return Ok(());
            }
            inbound.extend_from_slice(&chunk[..read]);
        };
        match tag {
            b'X' => return Ok(()),
            b'Q' => {
                let text = String::from_utf8_lossy(&body)
                    .trim_end_matches('\0')
                    .to_string();
                let command = parse_command(&text);
                let mut reply = Vec::new();
                match command {
                    Command::IdentifySystem => {
                        identify_system(&session, &mut reply);
                    }
                    Command::CreateSlot { name } => {
                        match create_slot(&session, &slots, &name).await {
                            Ok(rows) => reply.extend(rows),
                            Err(refusal) => {
                                reply.extend(error_response(refusal_state(&refusal), &refusal))
                            }
                        }
                    }
                    Command::StartReplication { name, lsn } => {
                        let slot = slots.read(&name).await;
                        match slot {
                            Ok(Some((row, version))) => {
                                writer.write_all(&copy_both_response()).await?;
                                writer.flush().await?;
                                let end = stream_changes(
                                    &mut reader,
                                    &mut writer,
                                    &mut inbound,
                                    StreamSlot {
                                        session: &session,
                                        slots: &slots,
                                        row,
                                        version,
                                    },
                                    lsn,
                                )
                                .await?;
                                match end {
                                    StreamEnd::Client => {
                                        writer.write_all(&copy_done()).await?;
                                        writer
                                            .write_all(&command_complete("START_STREAMING"))
                                            .await?;
                                        writer.write_all(&ready_for_query()).await?;
                                        writer.flush().await?;
                                        continue;
                                    }
                                    StreamEnd::Refused(refusal) => {
                                        writer
                                            .write_all(&error_response(
                                                refusal_sqlstate(&refusal),
                                                &refusal,
                                            ))
                                            .await?;
                                        writer.flush().await?;
                                        return Ok(());
                                    }
                                }
                            }
                            Ok(None) => reply.extend(error_response(
                                "42704",
                                &format!("replication slot \"{name}\" does not exist"),
                            )),
                            Err(refusal) => {
                                reply.extend(error_response(refusal_state(&refusal), &refusal))
                            }
                        }
                    }
                    Command::Unsupported(text) => reply.extend(error_response(
                        "0A000",
                        &format!(
                            "FEATURE_NOT_SUPPORTED: replication command `{text}` is not supported"
                        ),
                    )),
                }
                reply.extend(ready_for_query());
                writer.write_all(&reply).await?;
                writer.flush().await?;
            }
            _ => {
                writer
                    .write_all(&error_response(
                        "08P01",
                        "protocol violation: a replication connection takes simple queries",
                    ))
                    .await?;
                return writer.flush().await;
            }
        }
    }
}

/// The SQLSTATE of a slot refusal: an invalid name is `42602`, anything else `XX000`.
fn refusal_state(refusal: &str) -> &'static str {
    if refusal.starts_with("REPLICATION_SLOT_NAME_INVALID") {
        "42602"
    } else if refusal.starts_with("CONFLICT") {
        "42710"
    } else {
        "XX000"
    }
}

/// The `IDENTIFY_SYSTEM` reply: system id, timeline, current LSN and database.
fn identify_system(session: &WireSession, reply: &mut Vec<u8>) {
    let database = medulla_wire_core::wire::WireProtocol::current_graph(session);
    reply.extend(row_description(&[
        ("systemid", TEXT_OID, -1),
        ("timeline", INT4_OID, 4),
        ("xlogpos", TEXT_OID, -1),
        ("dbname", TEXT_OID, -1),
    ]));
    reply.extend(data_row(&[
        "0".to_string(),
        "1".to_string(),
        format_lsn(1),
        database,
    ]));
    reply.extend(command_complete("IDENTIFY_SYSTEM"));
}

/// `CREATE_REPLICATION_SLOT`: record the slot at the feed's present high-water cursor, so no
/// change after the creation is missed, and answer the one-row result.
async fn create_slot(
    session: &WireSession,
    slots: &SlotStore<'_>,
    name: &str,
) -> Result<Vec<u8>, String> {
    slot_key(&slots.tenant, name)?;
    if slots.read(name).await?.is_some() {
        return Err(format!(
            "CONFLICT: replication slot \"{name}\" already exists"
        ));
    }
    let snapshot: medulla_mutation_types::graph_change_feed::GraphSnapshot = response_body(
        session
            .dispatch_method(Method::OpenGraphSnapshot)
            .await
            .map_err(|error| format!("{}: {}", error.code, error.message))?,
    )?;
    let row = ReplicationSlotRow {
        name: name.to_string(),
        tenant: slots.tenant.clone(),
        plugin: "pgoutput".to_string(),
        confirmed_cursor: Some(snapshot.high_water_cursor),
    };
    slots.write(&row, 0).await?;
    let mut reply = Vec::new();
    reply.extend(row_description(&[
        ("slot_name", TEXT_OID, -1),
        ("consistent_point", TEXT_OID, -1),
        ("snapshot_name", TEXT_OID, -1),
        ("output_plugin", TEXT_OID, -1),
    ]));
    reply.extend(data_row(&[
        name.to_string(),
        format_lsn(1),
        String::new(),
        "pgoutput".to_string(),
    ]));
    reply.extend(command_complete("CREATE_REPLICATION_SLOT"));
    Ok(reply)
}

/// The slot a stream reads and advances.
struct StreamSlot<'a> {
    session: &'a WireSession,
    slots: &'a SlotStore<'a>,
    row: ReplicationSlotRow,
    version: u64,
}

/// Stream the change feed after the slot's confirmed cursor until the client ends the stream or
/// the feed refuses. Inbound frames are read while the stream waits, so a status update is
/// answered promptly; unconfirmed bytes are bounded by [`MAX_UNCONFIRMED_BYTES`].
async fn stream_changes<R, W>(
    reader: &mut R,
    writer: &mut W,
    inbound: &mut Vec<u8>,
    mut slot: StreamSlot<'_>,
    start_lsn: u64,
) -> io::Result<StreamEnd>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let Some(mut cursor) = slot.row.confirmed_cursor.clone() else {
        return Ok(StreamEnd::Refused(
            "INTERNAL: the replication slot has no start cursor".to_string(),
        ));
    };
    let mut mapper = PgOutputMapper::new(start_lsn);
    let mut pending: VecDeque<PendingConfirm> = VecDeque::new();
    let mut unconfirmed = 0_usize;
    let mut last_keepalive = Instant::now();
    let mut chunk = vec![0_u8; 8192];
    loop {
        // Everything the client sent while the last page was written.
        while let Some((tag, body)) = take_frame(inbound)? {
            match tag {
                b'c' | b'X' => return Ok(StreamEnd::Client),
                b'd' => {
                    if let Some(flushed) = status_update_flushed(&body) {
                        if let Err(refusal) =
                            confirm(&mut slot, &mut pending, &mut unconfirmed, flushed).await
                        {
                            return Ok(StreamEnd::Refused(refusal));
                        }
                    }
                }
                _ => {
                    return Ok(StreamEnd::Refused(
                        "protocol violation: unexpected message during replication".to_string(),
                    ))
                }
            }
        }
        if unconfirmed >= MAX_UNCONFIRMED_BYTES {
            // The standby is behind: read only, until it confirms.
            let read = reader.read(&mut chunk).await?;
            if read == 0 {
                return Ok(StreamEnd::Client);
            }
            inbound.extend_from_slice(&chunk[..read]);
            continue;
        }
        let page = match fetch_page(slot.session, &cursor).await {
            Ok(page) => page,
            Err(refusal) => return Ok(StreamEnd::Refused(refusal)),
        };
        let had_events = !page.events.is_empty();
        let mut sent_bytes = 0_usize;
        for event in &page.events {
            let (lsn, messages) = mapper.map_event(event);
            for message in &messages {
                let out = wal_data(lsn, lsn + 1, now_us(), message);
                sent_bytes += out.len();
                writer.write_all(&out).await?;
            }
        }
        if had_events {
            unconfirmed += sent_bytes;
            pending.push_back(PendingConfirm {
                end_lsn: mapper.next_lsn() - 1,
                cursor: page.next_cursor.clone(),
                bytes: sent_bytes,
            });
            writer.flush().await?;
        }
        cursor = page.next_cursor;
        if had_events {
            continue;
        }
        // An idle feed: wait for the client or the next poll.
        if last_keepalive.elapsed() >= KEEPALIVE_INTERVAL {
            writer
                .write_all(&keepalive(mapper.next_lsn() - 1, now_us(), false))
                .await?;
            writer.flush().await?;
            last_keepalive = Instant::now();
        }
        tokio::select! {
            read = reader.read(&mut chunk) => {
                let read = read?;
                if read == 0 {
                    return Ok(StreamEnd::Client);
                }
                inbound.extend_from_slice(&chunk[..read]);
            }
            () = tokio::time::sleep(POLL_INTERVAL) => {}
        }
    }
}

/// One page of the change feed after `cursor`, as the login's own principal.
async fn fetch_page(session: &WireSession, cursor: &str) -> Result<GraphChangePage, String> {
    let response = session
        .dispatch_method(Method::ReadGraphChanges {
            cursor: cursor.to_string(),
            limit: FEED_PAGE,
            table: None,
        })
        .await
        .map_err(|error| format!("{}: {}", error.code, error.message))?;
    response_body(response)
}

/// Advance the slot's `confirmed_cursor` through every page the standby has flushed through
/// `flushed`, with one compare-and-set.
async fn confirm(
    slot: &mut StreamSlot<'_>,
    pending: &mut VecDeque<PendingConfirm>,
    unconfirmed: &mut usize,
    flushed: u64,
) -> Result<(), String> {
    let mut confirmed: Option<String> = None;
    while pending.front().is_some_and(|page| page.end_lsn <= flushed) {
        if let Some(page) = pending.pop_front() {
            *unconfirmed = unconfirmed.saturating_sub(page.bytes);
            confirmed = Some(page.cursor);
        }
    }
    let Some(cursor) = confirmed else {
        return Ok(());
    };
    slot.row.confirmed_cursor = Some(cursor);
    slot.version = slot.slots.write(&slot.row, slot.version).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(commit: &str, change: GraphChange) -> GraphChangeEvent {
        GraphChangeEvent {
            schema_version: 1,
            commit_id: commit.to_string(),
            event_ordinal: 0,
            committed_at_ms: 1_700_000_000_000,
            visible_at_ms: 1_700_000_000_001,
            latency_profile_revision: 1,
            commit_to_visible_ms: 1,
            change,
        }
    }

    fn row(operation: RowOperation, key: &str) -> GraphChange {
        GraphChange::Row {
            table: "orders".to_string(),
            operation,
            key: key.to_string(),
        }
    }

    /// The tags of a message list.
    fn tags(messages: &[Vec<u8>]) -> Vec<char> {
        messages.iter().map(|message| message[0] as char).collect()
    }

    // spec: MEDULLA-DURABLE-KERNEL-R035.7.2
    #[test]
    fn change_maps_to_pgoutput_messages_in_order() {
        let mut mapper = PgOutputMapper::new(0);
        // The first change of a table announces its relation; the second does not.
        let (first_lsn, first) = mapper.map_event(&event("c1", row(RowOperation::Insert, "7")));
        assert_eq!(first_lsn, 1);
        assert_eq!(tags(&first), ['B', 'R', 'I', 'C']);
        let (second_lsn, second) = mapper.map_event(&event("c2", row(RowOperation::Update, "7")));
        assert_eq!(second_lsn, 2);
        assert_eq!(tags(&second), ['B', 'U', 'C']);
        let (_, third) = mapper.map_event(&event("c3", row(RowOperation::Delete, "7")));
        assert_eq!(tags(&third), ['B', 'D', 'C']);

        // The relation names the table and one text key column.
        let relation = &first[1];
        let id = relation_id("public.orders");
        assert_eq!(relation[1..5], id.to_be_bytes());
        assert!(relation.windows(7).any(|window| window == b"orders\0"));
        assert!(relation.windows(4).any(|window| window == b"key\0"));

        // The insert and update carry the new tuple, the delete the old key.
        let tuple = |message: &Vec<u8>| -> String {
            let length = i32::from_be_bytes(message[9..13].try_into().unwrap()) as usize;
            String::from_utf8(message[13..13 + length].to_vec()).unwrap()
        };
        assert_eq!(first[2][5], b'N');
        assert_eq!(tuple(&first[2]), "7");
        assert_eq!(second[1][5], b'N');
        assert_eq!(third[1][5], b'K');

        // Begin and Commit carry the event's LSN, and the commit time is in PostgreSQL time.
        assert_eq!(u64::from_be_bytes(first[0][1..9].try_into().unwrap()), 1);
        let commit = &first[3];
        assert_eq!(u64::from_be_bytes(commit[2..10].try_into().unwrap()), 1);
        assert_eq!(
            i64::from_be_bytes(commit[18..26].try_into().unwrap()),
            1_700_000_000_000_000 - POSTGRES_EPOCH_OFFSET_US
        );
    }

    // spec: MEDULLA-DURABLE-KERNEL-R035.7.2
    #[test]
    fn graph_changes_map_to_node_and_edge_relations() {
        let mut mapper = PgOutputMapper::new(41);
        let (lsn, upsert) = mapper.map_event(&event(
            "c1",
            GraphChange::Upsert {
                node_ids: vec!["n1".to_string()],
                edge_ids: vec!["e1".to_string()],
            },
        ));
        assert_eq!(lsn, 41);
        assert_eq!(tags(&upsert), ['B', 'R', 'I', 'R', 'I', 'C']);
        let (_, tombstone) = mapper.map_event(&event(
            "c2",
            GraphChange::Tombstone {
                deleted_ids: vec!["n1".to_string()],
                deletion_revision: 3,
            },
        ));
        assert_eq!(tags(&tombstone), ['B', 'D', 'C']);
    }

    // spec: MEDULLA-DURABLE-KERNEL-R035.7.2
    #[test]
    fn commands_parse_and_lsns_round_trip() {
        assert_eq!(parse_command("IDENTIFY_SYSTEM"), Command::IdentifySystem);
        assert_eq!(
            parse_command("create_replication_slot orders_feed logical pgoutput;"),
            Command::CreateSlot {
                name: "orders_feed".to_string()
            }
        );
        assert_eq!(
            parse_command("START_REPLICATION SLOT orders_feed LOGICAL 0/10 (proto_version '1')"),
            Command::StartReplication {
                name: "orders_feed".to_string(),
                lsn: 0x10
            }
        );
        assert!(matches!(
            parse_command("CREATE_REPLICATION_SLOT s PHYSICAL"),
            Command::Unsupported(_)
        ));
        assert!(matches!(
            parse_command("START_REPLICATION SLOT s LOGICAL nonsense"),
            Command::Unsupported(_)
        ));
        assert_eq!(parse_lsn("1/FF"), Some((1 << 32) | 0xFF));
        assert_eq!(format_lsn((1 << 32) | 0xFF), "1/FF");
    }

    // spec: MEDULLA-DURABLE-KERNEL-R035.7.2
    #[test]
    fn frames_split_across_reads_are_taken_whole() {
        let mut buffer = Vec::new();
        let mut query = b"IDENTIFY_SYSTEM\0".to_vec();
        let mut message = vec![b'Q'];
        message.extend_from_slice(&((query.len() + 4) as u32).to_be_bytes());
        message.append(&mut query);
        buffer.extend_from_slice(&message[..7]);
        assert!(take_frame(&mut buffer).unwrap().is_none());
        buffer.extend_from_slice(&message[7..]);
        let (tag, body) = take_frame(&mut buffer).unwrap().unwrap();
        assert_eq!(
            (tag, body.as_slice()),
            (b'Q', b"IDENTIFY_SYSTEM\0".as_slice())
        );
        assert!(buffer.is_empty());
        // A declared length below the length word, or above the cap, is a malformed frame.
        let mut bad = vec![b'Q', 0, 0, 0, 2];
        assert!(take_frame(&mut bad).is_err());
    }

    // spec: MEDULLA-DURABLE-KERNEL-R035.7.2
    #[test]
    fn a_status_update_names_the_flushed_lsn() {
        let mut body = vec![b'r'];
        for value in [5_u64, 9, 9, 0] {
            body.extend_from_slice(&value.to_be_bytes());
        }
        body.push(0);
        assert_eq!(status_update_flushed(&body), Some(9));
        assert_eq!(status_update_flushed(b"k"), None);
        assert_eq!(
            refusal_sqlstate("CURSOR_EXPIRED: the cursor is past retention"),
            "55000"
        );
        assert_eq!(refusal_sqlstate("INTERNAL: x"), "XX000");
    }
}
