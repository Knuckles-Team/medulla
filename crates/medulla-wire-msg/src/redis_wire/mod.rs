//! Redis RESP wire-protocol listener (CONCEPT:MEDULLA-KG.ontology.resp2-resp3-codec-round) — a native, HAND-ROLLED
//! Redis server so a Redis client, an ORM cache layer, or `redis-cli` connects
//! DIRECTLY to the engine and runs the core Redis command set.
//!
//! ## What this is (and is NOT)
//!
//! Like the SQL wire shims (pgwire / mysql-wire, CONCEPT:MEDULLA-KG.compute.subsystems-reference) and the Bolt
//! adapter (CONCEPT:MEDULLA-KG.query.bolt-wire-protocol), this is an ADAPTER — NOT a re-implemented Redis. The
//! bytes are stored by the engine's own served key-value methods
//! (`KvGet`/`KvPut`/`KvDelete`/`KvCas`, MEDULLA-DURABLE-KERNEL-R102): the listener
//! owns no store. `AUTH` verifies the credential and every command enters the
//! dispatch with the verified request context of that principal, so a `SET`
//! that returned `OK` is a `KvPut` that survives a `kill -9`, and each principal
//! reads and writes only its own private KV scope. Only the string commands
//! `GET`, `SET`, `DEL`, `EXISTS`, `MGET` and `INCR` are served; the hash, list,
//! set, sorted-set, TTL and transaction commands answer `RESP_COMMAND_UNSUPPORTED`
//! and change nothing (MEDULLA-DURABLE-KERNEL-R860.3).
//!
//! It links NO server-side redis protocol crate — the RESP2 + RESP3 codec is
//! hand-rolled against the documented RESP spec (the Pi-contract idiom pgwire /
//! mysql-wire / bolt-wire follow). PURE-RUST, so it stays outside the Pi forbidden
//! set; it is deliberately OUT of the `pi` tier (a network listener).
//!
//! ## Protocol subset (CONCEPT:MEDULLA-KG.ontology.resp2-resp3-codec-round)
//!
//! LANDED: the RESP2 + RESP3 codec (simple strings, errors, integers, bulk
//! strings, arrays, and the RESP3 map/set/double/boolean/null types) + inline
//! commands, the `HELLO`/`COMMAND` handshake so `redis-cli` connects, and the core
//! command set: `PING`, `ECHO`, the served string commands `GET`, `SET`, `DEL`,
//! `EXISTS`, `MGET` and `INCR`, plus `AUTH`/`SELECT`/`QUIT`/`CONFIG`/`CLIENT`
//! niceties.
//!
//! ## Pub/sub (CONCEPT:MEDULLA-KG.txn.pubsub-transactions)
//!
//! LANDED (MEDULLA-307): the publish/subscribe surface — `SUBSCRIBE`/`UNSUBSCRIBE`,
//! `PSUBSCRIBE`/`PUNSUBSCRIBE` (glob-pattern channels), and `PUBLISH`, backed by a
//! per-listener [`PubSub`] registry (an mpsc channel per connection; the connection
//! driver `select!`s between the socket and its subscriber mailbox so published
//! messages are pushed out as they arrive). `MULTI`/`EXEC`/`DISCARD` are refused:
//! the served KV methods commit one key at a time, so no atomic multi-command
//! transaction exists to offer. Lua, streams, optimistic locking, and Redis
//! cluster commands are outside this adapter's declared protocol and return an
//! explicit unknown-command error; unsupported operations are never accepted as
//! successful no-ops.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::sync::RwLock;

use medulla_protocol_types::protocol::{Method, Request, ResultPayload};
use medulla_protocol_types::storage_wire::{KvCasOutcome, KvEntry};
use medulla_server_api::state::ServerState;
use medulla_wire_core::broker_wire;
use medulla_wire_core::broker_wire::BrokerProtocol;

/// Env var: when set (and built `--features redis-wire`) the Redis RESP listener
/// binds this address (documented loopback default `127.0.0.1:6379`, the Redis
/// default port). Unset ⇒ no listener.
pub const REDIS_ADDR_ENV: &str = "MEDULLA_REDIS_ADDR";
/// Derive the Redis credential for a principal from the deployment auth secret.
/// Clients authenticate with `AUTH <principal> <credential>`; the principal is
/// pseudonymized before it is used as a durable keyspace or pub/sub scope.
pub fn derive_redis_password(secret: &str, principal: &str) -> String {
    broker_wire::derive_password(BrokerProtocol::Redis, secret, principal)
}

fn verify_redis_password(secret: &str, principal: &str, password: &[u8]) -> bool {
    broker_wire::verify_password(
        BrokerProtocol::Redis,
        secret,
        principal,
        password,
        MAX_REDIS_COMMAND_BYTES,
    )
}

/// Which subscription table a pub/sub registration touches. Both are
/// accounted against the SAME link and key-byte budgets, which is why one
/// registrar owns them.
#[derive(Clone, Copy)]
enum SubscriptionKind {
    /// `SUBSCRIBE`: an exact channel name.
    Channel,
    /// `PSUBSCRIBE`: a glob pattern matched against published channels.
    Pattern,
}

/// The result of parsing one command out of the read buffer: `Ok(None)` ⇒
/// incomplete (read more), `Ok(Some((args, consumed)))` ⇒ a complete command.
type CommandParse = Result<Option<(Vec<Vec<u8>>, usize)>, String>;

const MAX_REDIS_REQUEST_BYTES: usize = 64 * 1024 * 1024;
const MAX_REDIS_COMMAND_BYTES: usize = 64;
const MAX_REDIS_KEY_BYTES: usize = 4 * 1024;
const MAX_RESP_ARGUMENTS: usize = 1_024;
const MAX_RESP_BULK_BYTES: usize = MAX_REDIS_REQUEST_BYTES;
const MAX_RESP_LINE_BYTES: usize = 64 * 1024;
const MAX_REDIS_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const MAX_REDIS_SUBSCRIPTIONS: usize = 1_024;
const MAX_REDIS_SUBSCRIPTION_BYTES: usize = 1024 * 1024;
const MAX_REDIS_PUBSUB_CONNECTIONS: usize = 4_096;
const MAX_REDIS_PUBSUB_LINKS: usize = 65_536;
const MAX_REDIS_PUBSUB_KEY_BYTES: usize = 64 * 1024 * 1024;
const MAX_REDIS_CHANNEL_BYTES: usize = 4 * 1024;
const MAX_REDIS_RESPONSE_ITEMS: usize = 1_000_000;
const PUBSUB_MAILBOX_CAPACITY: usize = 1_024;

pub fn invalid_data(message: &'static str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message)
}

fn utf8_argument(bytes: &[u8], max_bytes: usize) -> Result<String, String> {
    if bytes.len() > max_bytes {
        return Err("ERR argument exceeds resource limits".to_string());
    }
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| "ERR argument must be valid UTF-8".to_string())
}

/// A minimal glob matcher for `SCAN MATCH` — supports `*` (any run) and `?` (one
/// char). Sufficient for the common `prefix:*` patterns; anything fancier degrades
/// to a literal comparison.
fn glob_match(pat: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pat.chars().collect(), text.chars().collect());
    // Classic two-pointer glob with backtracking on `*`.
    let (mut pi, mut ti, mut star, mut mark) = (0usize, 0usize, None, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(sp) = star {
            pi = sp + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

// ── pub/sub registry (CONCEPT:MEDULLA-KG.txn.pubsub-transactions) ────────────────────────────────────────────

/// One message delivered to a subscriber's mailbox (CONCEPT:MEDULLA-KG.txn.pubsub-transactions). A `Channel`
/// message is rendered as a RESP `message` push; a `Pattern` message (from a glob
/// `PSUBSCRIBE`) as a `pmessage` push carrying the originating pattern.
#[derive(Clone, Debug)]
enum PubMessage {
    Channel {
        channel: Arc<str>,
        payload: Arc<[u8]>,
    },
    Pattern {
        pattern: Arc<str>,
        channel: Arc<str>,
        payload: Arc<[u8]>,
    },
}

impl PubMessage {
    /// Render this delivery as the RESP push frame Redis clients expect.
    fn to_resp(&self) -> Resp {
        match self {
            PubMessage::Channel { channel, payload } => Resp::Push(vec![
                Resp::bulk_str("message"),
                Resp::bulk_str(channel.as_bytes()),
                Resp::Bulk(Some(payload.as_ref().to_vec())),
            ]),
            PubMessage::Pattern {
                pattern,
                channel,
                payload,
            } => Resp::Push(vec![
                Resp::bulk_str("pmessage"),
                Resp::bulk_str(pattern.as_bytes()),
                Resp::bulk_str(channel.as_bytes()),
                Resp::Bulk(Some(payload.as_ref().to_vec())),
            ]),
        }
    }
}

#[derive(Default)]
struct PubSubInner {
    next_id: u64,
    subscription_links: usize,
    subscription_key_bytes: usize,
    /// conn-id → the mailbox sender for that connection.
    conns: HashMap<u64, mpsc::Sender<PubMessage>>,
    /// exact channel → the set of conn-ids subscribed to it.
    channels: HashMap<String, HashSet<u64>>,
    /// glob pattern → the set of conn-ids subscribed to it.
    patterns: HashMap<String, HashSet<u64>>,
}

impl PubSubInner {
    /// The subscription table `kind` names. One accessor, so the link and
    /// key-byte accounting around it never has to branch on the kind twice.
    fn table(&mut self, kind: SubscriptionKind) -> &mut HashMap<String, HashSet<u64>> {
        match kind {
            SubscriptionKind::Channel => &mut self.channels,
            SubscriptionKind::Pattern => &mut self.patterns,
        }
    }
}

/// The per-listener publish/subscribe registry (CONCEPT:MEDULLA-KG.txn.pubsub-transactions). Shared (via `Arc`)
/// across every connection the listener accepts; each connection registers an
/// bounded mpsc mailbox on connect and drops it on disconnect. `PUBLISH` fans a
/// payload out to every exact-channel subscriber plus every glob-pattern subscriber
/// whose pattern matches, returning the delivery count. All state lives under one
/// `parking_lot::Mutex` — `try_send` is non-blocking, so the lock is
/// never held across an `.await`.
#[derive(Default)]
struct PubSub {
    inner: Mutex<PubSubInner>,
}

impl PubSub {
    /// Register `id` under one scoped subscription key. `false` means the
    /// registration was refused: an unknown connection, or the shared link /
    /// key-byte budget is exhausted. `SUBSCRIBE` and `PSUBSCRIBE` differ only
    /// in which table the key lands in, so both are this one rule.
    fn add_subscription(&self, kind: SubscriptionKind, id: u64, scope: &str, key: &str) -> bool {
        let mut g = self.inner.lock();
        if !g.conns.contains_key(&id) {
            return false;
        }
        let key = scoped_pubsub_key(scope, key);
        let new_key = !g.table(kind).contains_key(&key);
        let next_key_bytes = g.subscription_key_bytes.checked_add(key.len());
        if g.subscription_links >= MAX_REDIS_PUBSUB_LINKS
            || (new_key && next_key_bytes.is_none_or(|bytes| bytes > MAX_REDIS_PUBSUB_KEY_BYTES))
        {
            return false;
        }
        if g.table(kind).entry(key).or_default().insert(id) {
            g.subscription_links += 1;
            if new_key {
                g.subscription_key_bytes = next_key_bytes.unwrap_or(g.subscription_key_bytes);
            }
        }
        true
    }

    /// Drop `id` from one scoped subscription key, releasing its link and — once
    /// the key has no subscribers left — the key's own byte budget.
    fn remove_subscription(&self, kind: SubscriptionKind, id: u64, scope: &str, key: &str) {
        let mut g = self.inner.lock();
        let key = scoped_pubsub_key(scope, key);
        let (removed, empty) = match g.table(kind).get_mut(&key) {
            Some(ids) => {
                let removed = ids.remove(&id);
                (removed, ids.is_empty())
            }
            None => (false, false),
        };
        if removed {
            g.subscription_links = g.subscription_links.saturating_sub(1);
        }
        if empty {
            g.table(kind).remove(&key);
            g.subscription_key_bytes = g.subscription_key_bytes.saturating_sub(key.len());
        }
    }

    /// Register a fresh connection mailbox, returning its unique connection id.
    fn register(&self, tx: mpsc::Sender<PubMessage>) -> Option<u64> {
        let mut g = self.inner.lock();
        if g.conns.len() >= MAX_REDIS_PUBSUB_CONNECTIONS {
            return None;
        }
        g.next_id = g.next_id.checked_add(1)?;
        let id = g.next_id;
        g.conns.insert(id, tx);
        Some(id)
    }

    /// Drop a connection: remove its mailbox and prune it from every channel /
    /// pattern subscription (garbage-collecting now-empty entries).
    fn unregister(&self, id: u64) {
        let mut g = self.inner.lock();
        g.conns.remove(&id);
        let mut removed_links = 0usize;
        let mut removed_key_bytes = 0usize;
        g.channels.retain(|channel, ids| {
            if ids.remove(&id) {
                removed_links += 1;
            }
            let keep = !ids.is_empty();
            if !keep {
                removed_key_bytes = removed_key_bytes.saturating_add(channel.len());
            }
            keep
        });
        g.patterns.retain(|pattern, ids| {
            if ids.remove(&id) {
                removed_links += 1;
            }
            let keep = !ids.is_empty();
            if !keep {
                removed_key_bytes = removed_key_bytes.saturating_add(pattern.len());
            }
            keep
        });
        g.subscription_links = g.subscription_links.saturating_sub(removed_links);
        g.subscription_key_bytes = g.subscription_key_bytes.saturating_sub(removed_key_bytes);
    }

    fn publish(&self, scope: &str, channel: &str, payload: &[u8]) -> i64 {
        let g = self.inner.lock();
        // Share one immutable payload allocation across every bounded subscriber
        // mailbox.  Cloning a full payload per subscriber lets one large PUBLISH
        // amplify memory linearly with fan-out even when each mailbox is bounded.
        let payload: Arc<[u8]> = Arc::from(payload);
        let wire_channel: Arc<str> = Arc::from(channel);
        let scoped_channel = scoped_pubsub_key(scope, channel);
        publish_to_channel_subscribers(&g, &scoped_channel, &wire_channel, &payload)
            + publish_to_pattern_subscribers(&g, scope, &scoped_channel, &wire_channel, &payload)
    }
}

/// Fan `payload` out to every EXACT subscriber of `scoped_channel`.
fn publish_to_channel_subscribers(
    g: &PubSubInner,
    scoped_channel: &str,
    wire_channel: &Arc<str>,
    payload: &Arc<[u8]>,
) -> i64 {
    let mut count = 0i64;
    if let Some(ids) = g.channels.get(scoped_channel) {
        for id in ids {
            if let Some(tx) = g.conns.get(id) {
                let msg = PubMessage::Channel {
                    channel: Arc::clone(wire_channel),
                    payload: Arc::clone(payload),
                };
                if tx.try_send(msg).is_ok() {
                    count += 1;
                }
            }
        }
    }
    count
}

/// Fan `payload` out to every glob-PATTERN subscriber whose pattern matches
/// `scoped_channel`.
fn publish_to_pattern_subscribers(
    g: &PubSubInner,
    scope: &str,
    scoped_channel: &str,
    wire_channel: &Arc<str>,
    payload: &Arc<[u8]>,
) -> i64 {
    let mut count = 0i64;
    let scope_prefix = format!("{scope}\0");
    for (pat, ids) in g.patterns.iter() {
        if !glob_match(pat, scoped_channel) {
            continue;
        }
        let Some(wire_pattern) = pat.strip_prefix(&scope_prefix) else {
            continue;
        };
        let pattern: Arc<str> = Arc::from(wire_pattern);
        for id in ids {
            if let Some(tx) = g.conns.get(id) {
                let msg = PubMessage::Pattern {
                    pattern: Arc::clone(&pattern),
                    channel: Arc::clone(wire_channel),
                    payload: Arc::clone(payload),
                };
                if tx.try_send(msg).is_ok() {
                    count += 1;
                }
            }
        }
    }
    count
}

fn scoped_pubsub_key(scope: &str, name: &str) -> String {
    format!("{scope}\0{name}")
}

// ── RESP2 / RESP3 codec (CONCEPT:MEDULLA-KG.ontology.resp2-resp3-codec-round) ─────────────────────────────────────────

/// A RESP reply value. Version-aware: [`encode`](Resp::encode) renders the RESP3
/// extended types (map/set/double/boolean/null) natively when the connection has
/// upgraded via `HELLO 3`, and downgrades them to their RESP2 equivalents
/// (array/bulk/integer/null-bulk) otherwise.
#[derive(Clone, Debug, PartialEq)]
enum Resp {
    Simple(String),
    Error(String),
    Int(i64),
    Bulk(Option<Vec<u8>>),
    Array(Option<Vec<Resp>>),
    Map(Vec<(Resp, Resp)>),
    Set(Vec<Resp>),
    /// A RESP3 push message (`>`, used for pub/sub delivery + subscribe confirms,
    /// CONCEPT:MEDULLA-KG.txn.pubsub-transactions). Downgrades to a plain array (`*`) on RESP2 — the RESP2
    /// wire has no distinct push type, exactly how real Redis behaves.
    Push(Vec<Resp>),
    Double(f64),
    #[allow(dead_code)] // RESP3 boolean; the encoder handles it, no server reply builds one yet
    Bool(bool),
    Null,
}

impl Resp {
    fn bulk_str(s: impl Into<Vec<u8>>) -> Resp {
        Resp::Bulk(Some(s.into()))
    }

    /// Serialize into RESP bytes for protocol version `proto` (2 or 3).
    fn encode(&self, proto: u8, out: &mut Vec<u8>) {
        match self {
            Resp::Simple(s) => {
                out.push(b'+');
                out.extend_from_slice(s.as_bytes());
                out.extend_from_slice(b"\r\n");
            }
            Resp::Error(s) => {
                out.push(b'-');
                out.extend_from_slice(s.as_bytes());
                out.extend_from_slice(b"\r\n");
            }
            Resp::Int(i) => {
                out.push(b':');
                out.extend_from_slice(i.to_string().as_bytes());
                out.extend_from_slice(b"\r\n");
            }
            Resp::Bulk(Some(b)) => {
                out.push(b'$');
                out.extend_from_slice(b.len().to_string().as_bytes());
                out.extend_from_slice(b"\r\n");
                out.extend_from_slice(b);
                out.extend_from_slice(b"\r\n");
            }
            Resp::Bulk(None) | Resp::Null => encode_null_bulk(proto, out),
            Resp::Array(Some(items)) => encode_aggregate(b'*', items, proto, out),
            Resp::Array(None) => encode_null_array(proto, out),
            Resp::Map(pairs) => encode_map(pairs, proto, out),
            Resp::Set(items) => encode_aggregate(b'~', items, proto, out),
            Resp::Push(items) => encode_aggregate(b'>', items, proto, out),
            Resp::Double(d) => encode_double(*d, proto, out),
            Resp::Bool(b) => encode_bool(*b, proto, out),
        }
    }
}

fn encode_null_bulk(proto: u8, out: &mut Vec<u8>) {
    if proto >= 3 {
        out.extend_from_slice(b"_\r\n");
    } else {
        out.extend_from_slice(b"$-1\r\n");
    }
}

fn encode_null_array(proto: u8, out: &mut Vec<u8>) {
    if proto >= 3 {
        out.extend_from_slice(b"_\r\n");
    } else {
        out.extend_from_slice(b"*-1\r\n");
    }
}

fn encode_map(pairs: &[(Resp, Resp)], proto: u8, out: &mut Vec<u8>) {
    if proto >= 3 {
        out.push(b'%');
        out.extend_from_slice(pairs.len().to_string().as_bytes());
        out.extend_from_slice(b"\r\n");
    } else {
        // RESP2: a flat array of 2N elements.
        out.push(b'*');
        out.extend_from_slice((pairs.len() * 2).to_string().as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    for (k, v) in pairs {
        k.encode(proto, out);
        v.encode(proto, out);
    }
}

/// `Set`/`Push` share the same shape on the wire: a RESP3-typed aggregate
/// (`~`/`>`) that downgrades to a plain array (`*`) on RESP2 — the RESP2 wire
/// has no distinct set/push type, exactly how real Redis behaves.
fn encode_aggregate(resp3_prefix: u8, items: &[Resp], proto: u8, out: &mut Vec<u8>) {
    out.push(if proto >= 3 { resp3_prefix } else { b'*' });
    out.extend_from_slice(items.len().to_string().as_bytes());
    out.extend_from_slice(b"\r\n");
    for it in items {
        it.encode(proto, out);
    }
}

fn encode_double(d: f64, proto: u8, out: &mut Vec<u8>) {
    if proto >= 3 {
        out.push(b',');
        out.extend_from_slice(fmt_double(d).as_bytes());
        out.extend_from_slice(b"\r\n");
    } else {
        Resp::bulk_str(fmt_double(d)).encode(proto, out);
    }
}

fn encode_bool(b: bool, proto: u8, out: &mut Vec<u8>) {
    if proto >= 3 {
        out.extend_from_slice(if b { b"#t\r\n" } else { b"#f\r\n" });
    } else {
        Resp::Int(if b { 1 } else { 0 }).encode(proto, out);
    }
}

/// Conservative pre-encoding budget. It counts retained payload bytes plus fixed
/// framing/headroom per nested value, preventing a small request from
/// materializing or encoding an arbitrarily large aggregate response.
fn response_cost(resp: &Resp, total: &mut usize, items: &mut usize) -> bool {
    *items = match (*items).checked_add(1) {
        Some(value) if value <= MAX_REDIS_RESPONSE_ITEMS => value,
        _ => return false,
    };
    *total = match (*total).checked_add(32) {
        Some(value) if value <= MAX_REDIS_RESPONSE_BYTES => value,
        _ => return false,
    };
    let payload_bytes = match resp {
        Resp::Simple(value) | Resp::Error(value) => value.len(),
        Resp::Bulk(Some(value)) => value.len(),
        Resp::Array(Some(values)) | Resp::Set(values) | Resp::Push(values) => {
            return values
                .iter()
                .all(|value| response_cost(value, total, items));
        }
        Resp::Map(values) => {
            return values.iter().all(|(key, value)| {
                response_cost(key, total, items) && response_cost(value, total, items)
            });
        }
        Resp::Int(_)
        | Resp::Bulk(None)
        | Resp::Array(None)
        | Resp::Double(_)
        | Resp::Bool(_)
        | Resp::Null => 0,
    };
    *total = match (*total).checked_add(payload_bytes) {
        Some(value) if value <= MAX_REDIS_RESPONSE_BYTES => value,
        _ => return false,
    };
    true
}

fn responses_within_budget(responses: &[Resp]) -> bool {
    let mut total = 0usize;
    let mut items = 0usize;
    responses
        .iter()
        .all(|response| response_cost(response, &mut total, &mut items))
}

/// Format an `f64` the Redis way (bare integer when whole, else the shortest
/// round-trippable decimal).
fn fmt_double(d: f64) -> String {
    if d.fract() == 0.0 && d.abs() < 1e15 {
        format!("{}", d as i64)
    } else {
        format!("{d}")
    }
}

/// Try to parse ONE complete command from `buf`. Supports the RESP array-of-bulk
/// form (`redis-cli`) and the inline (whitespace) form (telnet). Returns
/// `Ok(None)` when the buffer holds only a partial command (read more),
/// `Ok(Some((args, consumed)))` on a complete one, and `Err` on a protocol error.
fn try_parse_command(buf: &[u8]) -> CommandParse {
    if buf.is_empty() {
        return Ok(None);
    }
    if buf[0] == b'*' {
        parse_array_command(buf)
    } else {
        parse_inline_command(buf)
    }
}

/// Find the byte index just past the next CRLF, returning `(line_bytes, next)`.
fn read_crlf_line(buf: &[u8], from: usize) -> Option<(&[u8], usize)> {
    let mut i = from;
    while i + 1 < buf.len() {
        if buf[i] == b'\r' && buf[i + 1] == b'\n' {
            return Some((&buf[from..i], i + 2));
        }
        i += 1;
    }
    None
}

fn parse_inline_command(buf: &[u8]) -> CommandParse {
    match read_crlf_line(buf, 0) {
        Some((line, next)) => {
            if line.len() > MAX_RESP_LINE_BYTES {
                return Err("ERR Protocol error: too big inline request".into());
            }
            let mut args = Vec::new();
            for argument in line
                .split(|b| *b == b' ' || *b == b'\t')
                .filter(|argument| !argument.is_empty())
            {
                if args.len() >= MAX_RESP_ARGUMENTS {
                    return Err("ERR Protocol error: too many arguments".into());
                }
                args.push(argument.to_vec());
            }
            Ok(Some((args, next)))
        }
        None => {
            // Guard against an unbounded inline line with no terminator.
            if buf.len() > MAX_RESP_LINE_BYTES {
                Err("ERR Protocol error: too big inline request".into())
            } else {
                Ok(None)
            }
        }
    }
}

fn parse_array_command(buf: &[u8]) -> CommandParse {
    let (header, mut pos) = match read_crlf_line(buf, 0) {
        Some(v) => v,
        None if buf.len() > MAX_RESP_LINE_BYTES => {
            return Err("ERR Protocol error: multibulk header too large".into());
        }
        None => return Ok(None),
    };
    if header.len() > MAX_RESP_LINE_BYTES {
        return Err("ERR Protocol error: multibulk header too large".into());
    }
    let count: i64 = std::str::from_utf8(&header[1..])
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| "ERR Protocol error: invalid multibulk length".to_string())?;
    if count == -1 {
        return Ok(Some((Vec::new(), pos)));
    }
    if count < -1 {
        return Err("ERR Protocol error: invalid multibulk length".into());
    }
    let count = usize::try_from(count)
        .map_err(|_| "ERR Protocol error: invalid multibulk length".to_string())?;
    if count > MAX_RESP_ARGUMENTS {
        return Err("ERR Protocol error: too many arguments".into());
    }
    let mut args = Vec::with_capacity(count);
    let mut aggregate_bytes = 0usize;
    for _ in 0..count {
        match parse_one_bulk_argument(buf, pos, &mut aggregate_bytes)? {
            Some((bytes, new_pos)) => {
                args.push(bytes);
                pos = new_pos;
            }
            None => return Ok(None), // bytes + trailing CRLF not all here yet
        }
    }
    Ok(Some((args, pos)))
}

/// Parse one `$<len>\r\n<bytes>\r\n` bulk argument starting at `pos`, tracking
/// the running `aggregate_bytes` across the whole multibulk command against
/// `MAX_REDIS_REQUEST_BYTES`. `Ok(None)` means the buffer doesn't yet hold the
/// complete argument (the connection driver should read more and retry).
fn parse_one_bulk_argument(
    buf: &[u8],
    pos: usize,
    aggregate_bytes: &mut usize,
) -> Result<Option<(Vec<u8>, usize)>, String> {
    let Some((blen, after_len)) = parse_bulk_length(buf, pos)? else {
        return Ok(None);
    };
    if blen == -1 {
        return Ok(Some((Vec::new(), after_len)));
    }
    if blen < -1 {
        return Err("ERR Protocol error: invalid bulk length".into());
    }
    let blen =
        usize::try_from(blen).map_err(|_| "ERR Protocol error: invalid bulk length".to_string())?;
    read_bulk_payload(buf, after_len, blen, aggregate_bytes)
}

/// Parse a bulk argument's `$<len>\r\n` length header. `Ok(None)` means the
/// header itself hasn't fully arrived yet.
fn parse_bulk_length(buf: &[u8], pos: usize) -> Result<Option<(i64, usize)>, String> {
    let (blen_line, after_len) = match read_crlf_line(buf, pos) {
        Some(v) => v,
        None if buf.len().saturating_sub(pos) > MAX_RESP_LINE_BYTES => {
            return Err("ERR Protocol error: bulk header too large".into());
        }
        None => return Ok(None),
    };
    if blen_line.len() > MAX_RESP_LINE_BYTES {
        return Err("ERR Protocol error: bulk header too large".into());
    }
    if blen_line.first() != Some(&b'$') {
        return Err("ERR Protocol error: expected '$'".into());
    }
    let blen: i64 = std::str::from_utf8(&blen_line[1..])
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| "ERR Protocol error: invalid bulk length".to_string())?;
    Ok(Some((blen, after_len)))
}

/// Read a bulk argument's `blen`-byte payload plus its trailing CRLF, starting
/// at `start`, and fold `blen` into the running `aggregate_bytes` bound.
/// `Ok(None)` means the payload + terminator hasn't fully arrived yet.
fn read_bulk_payload(
    buf: &[u8],
    start: usize,
    blen: usize,
    aggregate_bytes: &mut usize,
) -> Result<Option<(Vec<u8>, usize)>, String> {
    if blen > MAX_RESP_BULK_BYTES {
        return Err("ERR Protocol error: bulk request too large".into());
    }
    *aggregate_bytes = aggregate_bytes
        .checked_add(blen)
        .filter(|total| *total <= MAX_REDIS_REQUEST_BYTES)
        .ok_or_else(|| "ERR Protocol error: request too large".to_string())?;
    let end = start
        .checked_add(blen)
        .ok_or_else(|| "ERR Protocol error: bulk length overflow".to_string())?;
    let framed_end = end
        .checked_add(2)
        .ok_or_else(|| "ERR Protocol error: bulk length overflow".to_string())?;
    if buf.len() < framed_end {
        return Ok(None);
    }
    if &buf[end..framed_end] != b"\r\n" {
        return Err("ERR Protocol error: invalid bulk terminator".into());
    }
    Ok(Some((buf[start..end].to_vec(), end + 2)))
}

// ── command dispatch (CONCEPT:MEDULLA-KG.ontology.resp2-resp3-codec-round) ─────────────────────────────────────────────

/// Per-connection mutable state threaded through command execution. Carries the
/// RESP version + auth flag, the pub/sub subscription sets, and the `MULTI`
/// transaction queue (CONCEPT:MEDULLA-KG.ontology.resp2-resp3-codec-round core; CONCEPT:MEDULLA-KG.txn.pubsub-transactions pub/sub + transactions).
struct ConnState {
    proto: u8,
    /// Present only after HMAC credential verification. The value is a
    /// secret-keyed pseudonym, never the supplied principal.
    actor_scope: Option<String>,
    quit: bool,
    /// This connection's unique id in the [`PubSub`] registry (0 until registered;
    /// the pure-`execute` unit tests never register, which is fine).
    id: u64,
    /// Channels this connection is `SUBSCRIBE`d to (CONCEPT:MEDULLA-KG.txn.pubsub-transactions).
    sub_channels: HashSet<String>,
    /// Glob patterns this connection is `PSUBSCRIBE`d to (CONCEPT:MEDULLA-KG.txn.pubsub-transactions).
    sub_patterns: HashSet<String>,
    /// Total retained channel/pattern bytes across both subscription sets.
    sub_bytes: usize,
}

impl ConnState {
    pub fn new(proto: u8) -> Self {
        ConnState {
            proto,
            actor_scope: None,
            quit: false,
            id: 0,
            sub_channels: HashSet::new(),
            sub_patterns: HashSet::new(),
            sub_bytes: 0,
        }
    }

    /// Total live subscriptions (channels + patterns) — the count Redis echoes in
    /// every subscribe/unsubscribe confirmation.
    fn sub_count(&self) -> i64 {
        (self.sub_channels.len() + self.sub_patterns.len()) as i64
    }

    fn authenticated_scope(&self) -> Result<&str, Resp> {
        self.actor_scope
            .as_deref()
            .ok_or_else(|| Resp::Error("NOAUTH Authentication required.".into()))
    }
}

/// Uppercase an argument for case-insensitive command / option matching.
fn upper(b: &[u8]) -> String {
    if b.len() > MAX_REDIS_COMMAND_BYTES {
        return String::new();
    }
    std::str::from_utf8(b)
        .map(str::to_ascii_uppercase)
        .unwrap_or_default()
}

/// Execute ONE parsed command, returning the reply. Handshake / session commands
/// (`HELLO`/`AUTH`/`QUIT`/`SELECT`) mutate `conn`; every data command enters the
/// served key-value methods through the dispatch with the connection's verified
/// actor ([`KvSeam`]).
async fn execute(kv: &KvSeam, args: &[Vec<u8>], conn: &mut ConnState, auth_secret: &str) -> Resp {
    if args.is_empty() {
        return Resp::Error("ERR empty command".into());
    }
    let cmd = upper(&args[0]);

    // Only authentication and disconnect are available before identity binding.
    if let Some(resp) = execute_preauth(&cmd, args, conn, auth_secret) {
        return resp;
    }

    let actor = match conn.authenticated_scope() {
        Ok(scope) => scope,
        Err(error) => return error,
    };

    if let Some(resp) = execute_quick_verb(&cmd, args) {
        return resp;
    }

    match execute_data(kv, actor, &cmd, args).await {
        Ok(r) => r,
        Err(e) => Resp::Error(e),
    }
}

/// The verbs available before identity binding: `QUIT`, `HELLO`, and `AUTH`.
/// `None` means the caller should fall through to the authenticated path.
fn execute_preauth(
    cmd: &str,
    args: &[Vec<u8>],
    conn: &mut ConnState,
    auth_secret: &str,
) -> Option<Resp> {
    match cmd {
        "QUIT" => {
            conn.quit = true;
            Some(Resp::Simple("OK".into()))
        }
        "HELLO" => Some(hello(args, conn, auth_secret)),
        "AUTH" => {
            if args.len() != 3 {
                return Some(Resp::Error(
                    "ERR AUTH requires principal and credential".into(),
                ));
            }
            Some(
                match authenticate_redis_principal(auth_secret, &args[1], &args[2]) {
                    Some(scope) => {
                        conn.actor_scope = Some(scope);
                        Resp::Simple("OK".into())
                    }
                    None => Resp::Error("WRONGPASS invalid username-password pair".into()),
                },
            )
        }
        _ => None,
    }
}

/// Verbs answered directly without touching the data store. `None` means the
/// caller should fall through to [`execute_data`].
fn execute_quick_verb(cmd: &str, args: &[Vec<u8>]) -> Option<Resp> {
    match cmd {
        "PING" => Some(match args.get(1) {
            Some(msg) => Resp::Bulk(Some(msg.clone())),
            None => Resp::Simple("PONG".into()),
        }),
        "ECHO" => Some(match args.get(1) {
            Some(msg) => Resp::Bulk(Some(msg.clone())),
            None => Resp::Error("ERR wrong number of arguments for 'echo'".into()),
        }),
        "SELECT" => Some(Resp::Simple("OK".into())),
        "COMMAND" | "CONFIG" => Some(Resp::Array(Some(Vec::new()))),
        "CLIENT" => Some(Resp::Simple("OK".into())),
        _ => None,
    }
}

fn authenticate_redis_principal(
    auth_secret: &str,
    principal: &[u8],
    credential: &[u8],
) -> Option<String> {
    let principal = std::str::from_utf8(principal).ok()?;
    if !verify_redis_password(auth_secret, principal, credential) {
        return None;
    }
    medulla_server_api::api::dispatch_support::pseudonymous_broker_actor(auth_secret, principal)
        .ok()
}

/// If `args[1]` parses as a protocol version number, validate and apply it
/// (advancing `*i` past it). A non-numeric or absent `args[1]` is not a
/// protover at all — leaves `*i` untouched so it's parsed as a `HELLO` option
/// instead, matching real Redis's argument-shape sniffing.
fn hello_apply_protover(args: &[Vec<u8>], conn: &mut ConnState, i: &mut usize) -> Option<Resp> {
    let value = args.get(1)?;
    let p: u8 = std::str::from_utf8(value)
        .unwrap_or_default()
        .parse()
        .ok()?;
    if p != 2 && p != 3 {
        return Some(Resp::Error("NOPROTO unsupported protocol version".into()));
    }
    conn.proto = p;
    *i = 2;
    None
}

/// Apply every remaining `HELLO` option starting at `i` (today, only
/// `AUTH <principal> <credential>`). `None` means all options were valid.
fn hello_apply_options(
    args: &[Vec<u8>],
    mut i: usize,
    conn: &mut ConnState,
    auth_secret: &str,
) -> Option<Resp> {
    while i < args.len() {
        if upper(&args[i]) == "AUTH" && i + 2 < args.len() {
            match authenticate_redis_principal(auth_secret, &args[i + 1], &args[i + 2]) {
                Some(scope) => conn.actor_scope = Some(scope),
                None => {
                    return Some(Resp::Error(
                        "WRONGPASS invalid username-password pair".into(),
                    ))
                }
            }
            i += 3;
        } else {
            return Some(Resp::Error("ERR invalid HELLO option".into()));
        }
    }
    None
}

/// The `HELLO` handshake upgrades RESP and performs mandatory identity binding
/// before returning server metadata.
fn hello(args: &[Vec<u8>], conn: &mut ConnState, auth_secret: &str) -> Resp {
    let mut i = 1;
    if let Some(resp) = hello_apply_protover(args, conn, &mut i) {
        return resp;
    }
    // `HELLO ... AUTH <principal> <credential>` binds a fresh connection. An
    // already-authenticated connection may renegotiate only the RESP version.
    if let Some(resp) = hello_apply_options(args, i, conn, auth_secret) {
        return resp;
    }
    if conn.actor_scope.is_none() {
        return Resp::Error("NOAUTH Authentication required.".into());
    }
    Resp::Map(vec![
        (Resp::bulk_str("server"), Resp::bulk_str("medulla")),
        (Resp::bulk_str("version"), Resp::bulk_str("2.1.0")),
        (Resp::bulk_str("proto"), Resp::Int(conn.proto as i64)),
        (Resp::bulk_str("id"), Resp::Int(1)),
        (Resp::bulk_str("mode"), Resp::bulk_str("standalone")),
        (Resp::bulk_str("role"), Resp::bulk_str("master")),
        (Resp::bulk_str("modules"), Resp::Array(Some(Vec::new()))),
    ])
}

/// The reply text of a command outside the served string set.
const RESP_COMMAND_UNSUPPORTED: &str = "ERR RESP_COMMAND_UNSUPPORTED";

/// Commands the listener names and refuses: the hash, list, set, sorted-set and
/// TTL families, the transaction verbs and the string variants that need more
/// than one served KV method. They change nothing.
const REFUSED_COMMANDS: &[&str] = &[
    "EXPIRE",
    "PEXPIRE",
    "EXPIREAT",
    "PEXPIREAT",
    "TTL",
    "PTTL",
    "PERSIST",
    "DECR",
    "INCRBY",
    "DECRBY",
    "MSET",
    "SETNX",
    "SETEX",
    "PSETEX",
    "GETSET",
    "APPEND",
    "STRLEN",
    "HSET",
    "HSETNX",
    "HMSET",
    "HGET",
    "HMGET",
    "HGETALL",
    "HDEL",
    "HEXISTS",
    "HLEN",
    "HKEYS",
    "HVALS",
    "HINCRBY",
    "LPUSH",
    "RPUSH",
    "LPOP",
    "RPOP",
    "LRANGE",
    "LLEN",
    "LINDEX",
    "LSET",
    "LREM",
    "SADD",
    "SMEMBERS",
    "SREM",
    "SCARD",
    "SISMEMBER",
    "SPOP",
    "ZADD",
    "ZRANGE",
    "ZSCORE",
    "ZREM",
    "ZCARD",
    "ZRANK",
    "ZINCRBY",
    "SCAN",
    "TYPE",
    "KEYS",
    "MULTI",
    "EXEC",
    "DISCARD",
    "WATCH",
    "UNWATCH",
];

/// The namespace every RESP key lives in, inside the principal's private KV scope.
const RESP_KV_NAMESPACE: &str = "resp";

/// The graph the served KV methods are addressed to.
const RESP_KV_GRAPH: &str = "__commons__";

/// One RESP listener's handle on the served dispatch. It holds the shared server
/// state; it owns no store.
struct KvSeam {
    state: Arc<RwLock<ServerState>>,
}

/// Request ids are unique across restarts: the durable mutation ledger keys on
/// them, so a counter restarting at zero would replay an earlier operation.
fn next_request_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: std::sync::OnceLock<AtomicU64> = std::sync::OnceLock::new();
    NEXT.get_or_init(|| {
        let micros = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros() as u64)
            .unwrap_or(1);
        AtomicU64::new(micros)
    })
    .fetch_add(1, Ordering::Relaxed)
}

impl KvSeam {
    fn new(state: Arc<RwLock<ServerState>>) -> Self {
        Self { state }
    }

    /// Enter the served dispatch with the verified context of `actor`.
    async fn call(&self, actor: &str, method: Method) -> Result<ResultPayload, String> {
        let req = Request {
            id: next_request_id(),
            graph: RESP_KV_GRAPH.to_string(),
            auth_token: String::new(),
            agent_id: None,
            method,
        };
        let resp = Box::pin(
            medulla_server_api::api::reentry::dispatch_authenticated_resp_actor(
                &self.state,
                req,
                actor,
            ),
        )
        .await;
        if let Some(error) = resp.error {
            return Err(format!("ERR {error}"));
        }
        resp.result
            .ok_or_else(|| "ERR the engine returned no result".to_string())
    }

    fn decode<T: serde::de::DeserializeOwned>(payload: ResultPayload) -> Result<T, String> {
        match payload {
            ResultPayload::Raw(bytes) => rmp_serde::from_slice(&bytes)
                .map_err(|_| "ERR the engine returned an unreadable result".to_string()),
            _ => Err("ERR the engine returned an unexpected result".to_string()),
        }
    }

    /// `KvGet`: the value and the version of the write that produced it.
    async fn get(&self, actor: &str, key: &str) -> Result<Option<KvEntry>, String> {
        let payload = self
            .call(
                actor,
                Method::KvGet {
                    namespace: RESP_KV_NAMESPACE.to_string(),
                    key: key.to_string(),
                    shared: false,
                },
            )
            .await?;
        Self::decode(payload)
    }

    /// `KvPut`: an unconditional write.
    async fn put(&self, actor: &str, key: &str, value: Vec<u8>) -> Result<(), String> {
        self.call(
            actor,
            Method::KvPut {
                namespace: RESP_KV_NAMESPACE.to_string(),
                key: key.to_string(),
                value,
                shared: false,
            },
        )
        .await
        .map(|_| ())
    }

    /// `KvDelete`: whether the key existed.
    async fn delete(&self, actor: &str, key: &str) -> Result<bool, String> {
        match self
            .call(
                actor,
                Method::KvDelete {
                    namespace: RESP_KV_NAMESPACE.to_string(),
                    key: key.to_string(),
                    shared: false,
                },
            )
            .await?
        {
            ResultPayload::Bool(existed) => Ok(existed),
            _ => Err("ERR the engine returned an unexpected result".to_string()),
        }
    }

    /// `KvCas` on the read revision (`0` = the key was absent).
    async fn cas(
        &self,
        actor: &str,
        key: &str,
        expected_version: u64,
        new: Vec<u8>,
    ) -> Result<KvCasOutcome, String> {
        let payload = self
            .call(
                actor,
                Method::KvCas {
                    namespace: RESP_KV_NAMESPACE.to_string(),
                    key: key.to_string(),
                    expected_version,
                    new: Some(new),
                    shared: false,
                },
            )
            .await?;
        Self::decode(payload)
    }
}

fn wrong_args(cmd: &str) -> String {
    format!(
        "ERR wrong number of arguments for '{}'",
        cmd.to_ascii_lowercase()
    )
}

/// Every key argument after the command name (at least one).
fn redis_keys(args: &[Vec<u8>], cmd: &str) -> Result<Vec<String>, String> {
    if args.len() < 2 {
        return Err(wrong_args(cmd));
    }
    args[1..]
        .iter()
        .map(|bytes| utf8_argument(bytes, MAX_REDIS_KEY_BYTES))
        .collect()
}

/// `INCR`: a `KvGet`, then a `KvCas` on the read revision. A lost race rereads and
/// retries up to the `resp.incr_retry_limit` knob, then answers `INCR_CONFLICT`.
async fn cmd_incr(kv: &KvSeam, actor: &str, key: &str) -> Result<Resp, String> {
    let attempts = medulla_server_api::knob_config::knobs().resp_incr_retry_limit;
    for _ in 0..attempts {
        let (current, version) = match kv.get(actor, key).await? {
            Some(entry) => {
                let n = std::str::from_utf8(&entry.value)
                    .ok()
                    .and_then(|text| text.trim().parse::<i64>().ok())
                    .ok_or_else(|| "ERR value is not an integer".to_string())?;
                (n, entry.version)
            }
            None => (0, 0),
        };
        let next = current
            .checked_add(1)
            .ok_or_else(|| "ERR increment would overflow".to_string())?;
        if kv
            .cas(actor, key, version, next.to_string().into_bytes())
            .await?
            .swapped
        {
            return Ok(Resp::Int(next));
        }
    }
    Err("ERR INCR_CONFLICT".to_string())
}

/// Execute a data command (auth already checked). `Err(msg)` becomes a RESP error.
async fn execute_data(
    kv: &KvSeam,
    actor: &str,
    cmd: &str,
    args: &[Vec<u8>],
) -> Result<Resp, String> {
    match cmd {
        "GET" => {
            let [_, key] = args else {
                return Err(wrong_args(cmd));
            };
            let key = utf8_argument(key, MAX_REDIS_KEY_BYTES)?;
            Ok(Resp::Bulk(kv.get(actor, &key).await?.map(|e| e.value)))
        }
        "SET" => {
            let [_, key, value] = args else {
                // `EX`/`PX`/`NX`/`XX` need expiry and condition semantics the
                // served KV methods do not have; they are refused, never ignored.
                return Err(if args.len() > 3 {
                    RESP_COMMAND_UNSUPPORTED.to_string()
                } else {
                    wrong_args(cmd)
                });
            };
            let key = utf8_argument(key, MAX_REDIS_KEY_BYTES)?;
            kv.put(actor, &key, value.clone()).await?;
            Ok(Resp::Simple("OK".into()))
        }
        "DEL" => {
            let mut removed = 0i64;
            for key in redis_keys(args, cmd)? {
                removed += i64::from(kv.delete(actor, &key).await?);
            }
            Ok(Resp::Int(removed))
        }
        "EXISTS" => {
            let mut present = 0i64;
            for key in redis_keys(args, cmd)? {
                present += i64::from(kv.get(actor, &key).await?.is_some());
            }
            Ok(Resp::Int(present))
        }
        "MGET" => {
            let mut values = Vec::new();
            for key in redis_keys(args, cmd)? {
                values.push(Resp::Bulk(kv.get(actor, &key).await?.map(|e| e.value)));
            }
            Ok(Resp::Array(Some(values)))
        }
        "INCR" => {
            let [_, key] = args else {
                return Err(wrong_args(cmd));
            };
            let key = utf8_argument(key, MAX_REDIS_KEY_BYTES)?;
            cmd_incr(kv, actor, &key).await
        }
        refused if REFUSED_COMMANDS.contains(&refused) => Err(RESP_COMMAND_UNSUPPORTED.to_string()),
        _ => Err(format!(
            "ERR unknown command '{}'",
            cmd.to_ascii_lowercase()
        )),
    }
}

// ── pub/sub + transaction dispatch (CONCEPT:MEDULLA-KG.txn.pubsub-transactions) ───────────────────────────────

/// Commands permitted while a RESP2 connection is in subscriber mode. Everything
/// else is refused with the exact Redis error until the client unsubscribes.
fn allowed_in_subscribe(cmd: &str) -> bool {
    matches!(
        cmd,
        "SUBSCRIBE"
            | "UNSUBSCRIBE"
            | "PSUBSCRIBE"
            | "PUNSUBSCRIBE"
            | "PING"
            | "QUIT"
            | "RESET"
            | "HELLO"
    )
}

/// Top-level command dispatch used by the connection driver (CONCEPT:MEDULLA-KG.txn.pubsub-transactions).
/// Unlike [`execute`] (one reply) this returns a VECTOR of replies — subscribe /
/// unsubscribe emit one confirmation per channel — and threads the [`PubSub`]
/// registry plus the connection's transaction/subscription state. Non-pub/sub,
/// non-transaction commands delegate to [`execute`] for their single reply.
async fn dispatch(
    kv: &KvSeam,
    pubsub: &PubSub,
    args: &[Vec<u8>],
    conn: &mut ConnState,
    auth_secret: &str,
) -> Vec<Resp> {
    if args.is_empty() {
        return vec![Resp::Error("ERR empty command".into())];
    }
    let cmd = upper(&args[0]);
    if conn.actor_scope.is_none() && !matches!(cmd.as_str(), "AUTH" | "HELLO" | "QUIT") {
        return vec![Resp::Error("NOAUTH Authentication required.".into())];
    }

    // Enforce the RESP2 subscriber-mode command gate.
    if conn.proto < 3
        && (!conn.sub_channels.is_empty() || !conn.sub_patterns.is_empty())
        && !allowed_in_subscribe(&cmd)
    {
        return vec![Resp::Error(format!(
            "ERR Can't execute '{}': only (P|S)SUBSCRIBE / (P|S)UNSUBSCRIBE / PING / QUIT / RESET are allowed in this context",
            cmd.to_ascii_lowercase()
        ))];
    }

    dispatch_command(kv, pubsub, &cmd, args, conn, auth_secret).await
}

/// The per-command dispatch table [`dispatch`] falls into once the
/// auth/MULTI-queue/subscriber-mode gates have all passed. A plain exhaustive
/// match (not a lookup table) so the compiler keeps proving every handled verb
/// routes somewhere and an unrecognized one falls through to [`execute`].
async fn dispatch_command(
    kv: &KvSeam,
    pubsub: &PubSub,
    cmd: &str,
    args: &[Vec<u8>],
    conn: &mut ConnState,
    auth_secret: &str,
) -> Vec<Resp> {
    match cmd {
        "SUBSCRIBE" | "PSUBSCRIBE" => subscribe(pubsub, conn, &args[1..], cmd == "PSUBSCRIBE"),
        "UNSUBSCRIBE" | "PUNSUBSCRIBE" => {
            unsubscribe(pubsub, conn, &args[1..], cmd == "PUNSUBSCRIBE")
        }
        "PUBLISH" => handle_publish_cmd(pubsub, conn, args),
        "RESET" => reset_connection(pubsub, conn),
        _ => vec![execute(kv, args, conn, auth_secret).await],
    }
}

fn handle_publish_cmd(pubsub: &PubSub, conn: &ConnState, args: &[Vec<u8>]) -> Vec<Resp> {
    let Some(scope) = conn.actor_scope.as_deref() else {
        return vec![Resp::Error("NOAUTH Authentication required.".into())];
    };
    match (args.get(1), args.get(2)) {
        (Some(chan), Some(payload)) => match utf8_argument(chan, MAX_REDIS_CHANNEL_BYTES) {
            Ok(channel) => vec![Resp::Int(pubsub.publish(scope, &channel, payload))],
            Err(error) => vec![Resp::Error(error)],
        },
        _ => vec![Resp::Error(
            "ERR wrong number of arguments for 'publish'".into(),
        )],
    }
}

fn reset_connection(pubsub: &PubSub, conn: &mut ConnState) -> Vec<Resp> {
    let Some(scope) = conn.actor_scope.clone() else {
        return vec![Resp::Error("NOAUTH Authentication required.".into())];
    };
    for c in conn.sub_channels.drain().collect::<Vec<_>>() {
        pubsub.remove_subscription(SubscriptionKind::Channel, conn.id, &scope, &c);
    }
    for p in conn.sub_patterns.drain().collect::<Vec<_>>() {
        pubsub.remove_subscription(SubscriptionKind::Pattern, conn.id, &scope, &p);
    }
    conn.sub_bytes = 0;
    conn.proto = 2;
    conn.actor_scope = None;
    vec![Resp::Simple("RESET".into())]
}

/// `SUBSCRIBE` / `PSUBSCRIBE` (CONCEPT:MEDULLA-KG.txn.pubsub-transactions): add each channel/pattern to the
/// registry + the connection's set, emitting one confirmation push per channel with
/// the running total subscription count.
fn subscribe(pubsub: &PubSub, conn: &mut ConnState, chans: &[Vec<u8>], pattern: bool) -> Vec<Resp> {
    let kind = if pattern { "psubscribe" } else { "subscribe" };
    let Some(scope) = conn.actor_scope.clone() else {
        return vec![Resp::Error("NOAUTH Authentication required.".into())];
    };
    if chans.is_empty() {
        return vec![Resp::Error(format!(
            "ERR wrong number of arguments for '{kind}'"
        ))];
    }
    let mut out = Vec::with_capacity(chans.len());
    for c in chans {
        let name = match utf8_argument(c, MAX_REDIS_CHANNEL_BYTES) {
            Ok(name) => name,
            Err(error) => {
                out.push(Resp::Error(error));
                continue;
            }
        };
        out.push(subscribe_one(pubsub, conn, &scope, kind, pattern, name));
    }
    out
}

/// Subscribe `conn` to one already-decoded channel/pattern `name`, enforcing
/// the per-connection and global subscription resource limits, and return
/// its confirmation push (or the resource-limit error in its place).
fn subscribe_one(
    pubsub: &PubSub,
    conn: &mut ConnState,
    scope: &str,
    kind: &str,
    pattern: bool,
    name: String,
) -> Resp {
    let already_subscribed = if pattern {
        conn.sub_patterns.contains(&name)
    } else {
        conn.sub_channels.contains(&name)
    };
    let next_sub_bytes = conn.sub_bytes.checked_add(name.len());
    if !already_subscribed
        && (conn.sub_count() as usize >= MAX_REDIS_SUBSCRIPTIONS
            || next_sub_bytes.is_none_or(|bytes| bytes > MAX_REDIS_SUBSCRIPTION_BYTES))
    {
        return Resp::Error("ERR subscription resource limit exceeded".into());
    }
    if pattern {
        if !already_subscribed {
            if !pubsub.add_subscription(SubscriptionKind::Pattern, conn.id, scope, &name) {
                return Resp::Error("ERR global subscription resource limit exceeded".into());
            }
            conn.sub_patterns.insert(name.clone());
            conn.sub_bytes = next_sub_bytes.unwrap_or(conn.sub_bytes);
        }
    } else if !already_subscribed {
        if !pubsub.add_subscription(SubscriptionKind::Channel, conn.id, scope, &name) {
            return Resp::Error("ERR global subscription resource limit exceeded".into());
        }
        conn.sub_channels.insert(name.clone());
        conn.sub_bytes = next_sub_bytes.unwrap_or(conn.sub_bytes);
    }
    Resp::Push(vec![
        Resp::bulk_str(kind),
        Resp::bulk_str(name.into_bytes()),
        Resp::Int(conn.sub_count()),
    ])
}

/// `UNSUBSCRIBE` / `PUNSUBSCRIBE` (CONCEPT:MEDULLA-KG.txn.pubsub-transactions): drop the named channels (or ALL
/// of this kind when none are named), one confirmation push each. Unsubscribing from
/// nothing still emits a single null-channel confirmation, matching Redis.
fn unsubscribe(
    pubsub: &PubSub,
    conn: &mut ConnState,
    chans: &[Vec<u8>],
    pattern: bool,
) -> Vec<Resp> {
    let kind = if pattern {
        "punsubscribe"
    } else {
        "unsubscribe"
    };
    let Some(scope) = conn.actor_scope.clone() else {
        return vec![Resp::Error("NOAUTH Authentication required.".into())];
    };
    let targets = match unsubscribe_targets(conn, chans, pattern) {
        Ok(targets) => targets,
        Err(resp) => return vec![resp],
    };
    if targets.is_empty() {
        return vec![Resp::Push(vec![
            Resp::bulk_str(kind),
            Resp::Null,
            Resp::Int(conn.sub_count()),
        ])];
    }
    let mut out = Vec::with_capacity(targets.len());
    for name in targets {
        out.push(unsubscribe_one(pubsub, conn, &scope, kind, pattern, name));
    }
    out
}

/// Resolve which channels/patterns an `UNSUBSCRIBE`/`PUNSUBSCRIBE` targets:
/// every currently-subscribed one of that kind when `chans` is empty,
/// otherwise the explicitly named (and UTF-8-decoded) ones.
fn unsubscribe_targets(
    conn: &ConnState,
    chans: &[Vec<u8>],
    pattern: bool,
) -> Result<Vec<String>, Resp> {
    if chans.is_empty() {
        return Ok(if pattern {
            conn.sub_patterns.iter().cloned().collect()
        } else {
            conn.sub_channels.iter().cloned().collect()
        });
    }
    let mut targets = Vec::with_capacity(chans.len());
    for channel in chans {
        match utf8_argument(channel, MAX_REDIS_CHANNEL_BYTES) {
            Ok(channel) => targets.push(channel),
            Err(error) => return Err(Resp::Error(error)),
        }
    }
    Ok(targets)
}

/// Unsubscribe `conn` from one already-resolved channel/pattern `name` and
/// return its confirmation push.
fn unsubscribe_one(
    pubsub: &PubSub,
    conn: &mut ConnState,
    scope: &str,
    kind: &str,
    pattern: bool,
    name: String,
) -> Resp {
    let removed = if pattern {
        let removed = conn.sub_patterns.remove(&name);
        pubsub.remove_subscription(SubscriptionKind::Pattern, conn.id, scope, &name);
        removed
    } else {
        let removed = conn.sub_channels.remove(&name);
        pubsub.remove_subscription(SubscriptionKind::Channel, conn.id, scope, &name);
        removed
    };
    if removed {
        conn.sub_bytes = conn.sub_bytes.saturating_sub(name.len());
    }
    Resp::Push(vec![
        Resp::bulk_str(kind),
        Resp::bulk_str(name.into_bytes()),
        Resp::Int(conn.sub_count()),
    ])
}

// ── the per-connection driver + listener ──────────────────────────────────────────

/// Drive ONE Redis connection: parse commands from the socket, execute, reply,
/// until the client quits or the socket closes. Generic over the byte stream so an
/// in-process test can drive it over any duplex transport (CONCEPT:MEDULLA-KG.ontology.resp2-resp3-codec-round).
async fn handle_connection<S>(
    s: &mut S,
    kv: Arc<KvSeam>,
    pubsub: Arc<PubSub>,
    auth_secret: String,
) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut conn = ConnState::new(2);
    // Register this connection's pub/sub mailbox so PUBLISH can reach it (MEDULLA-307).
    let (tx, mut rx) = mpsc::channel::<PubMessage>(PUBSUB_MAILBOX_CAPACITY);
    conn.id = pubsub
        .register(tx)
        .ok_or_else(|| invalid_data("Redis connection limit exceeded"))?;
    let result = drive_connection(s, &kv, &pubsub, &mut conn, &mut rx, &auth_secret).await;
    // Always release the registry slot + all subscriptions on the way out.
    pubsub.unregister(conn.id);
    result
}

/// Parse and execute every complete command already in `buf`, writing each
/// reply out. Parses by offset and compacts `buf` once so a pipeline of tiny
/// commands cannot force a full-tail memmove after every command (quadratic
/// CPU work). Returns `Ok(true)` if the connection should close now (a
/// protocol error was already written, or the client sent `QUIT`).
async fn drain_and_execute<S>(
    s: &mut S,
    kv: &Arc<KvSeam>,
    pubsub: &Arc<PubSub>,
    conn: &mut ConnState,
    buf: &mut Vec<u8>,
    auth_secret: &str,
) -> std::io::Result<bool>
where
    S: AsyncWrite + Unpin,
{
    let mut consumed_total = 0usize;
    loop {
        let (args, consumed) = match try_parse_command(&buf[consumed_total..]) {
            Ok(Some(v)) => v,
            Ok(None) => break,
            Err(e) => {
                let mut out = Vec::new();
                Resp::Error(e).encode(conn.proto, &mut out);
                s.write_all(&out).await?;
                return Ok(true);
            }
        };
        consumed_total = consumed_total
            .checked_add(consumed)
            .filter(|consumed| *consumed <= buf.len())
            .ok_or_else(|| invalid_data("invalid RESP command length"))?;
        if args.is_empty() {
            continue;
        }
        let replies = dispatch(kv, pubsub, &args, conn, auth_secret).await;
        let mut out = Vec::new();
        if responses_within_budget(&replies) {
            for reply in &replies {
                reply.encode(conn.proto, &mut out);
            }
        } else {
            Resp::Error("ERR response exceeds resource limits".into()).encode(conn.proto, &mut out);
        }
        s.write_all(&out).await?;
        if conn.quit {
            let _ = s.shutdown().await;
            return Ok(true);
        }
    }
    if consumed_total > 0 {
        buf.drain(..consumed_total);
    }
    Ok(false)
}

/// The inner connection loop (split out so [`handle_connection`] can guarantee the
/// [`PubSub`] unregister runs on every exit path). `select!`s between the socket and
/// this connection's subscriber mailbox: buffered client commands are executed
/// first, then it awaits either more bytes or a published message to push out
/// (CONCEPT:MEDULLA-KG.txn.pubsub-transactions).
async fn drive_connection<S>(
    s: &mut S,
    kv: &Arc<KvSeam>,
    pubsub: &Arc<PubSub>,
    conn: &mut ConnState,
    rx: &mut mpsc::Receiver<PubMessage>,
    auth_secret: &str,
) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 8192];
    loop {
        // Drain and execute every complete command already in the buffer.
        if drain_and_execute(s, kv, pubsub, conn, &mut buf, auth_secret).await? {
            return Ok(());
        }
        // Nothing more to parse: wait for either new bytes or a published message.
        tokio::select! {
            read = s.read(&mut tmp) => {
                let n = read?;
                if n == 0 {
                    return Ok(()); // client closed
                }
                buf.extend_from_slice(&tmp[..n]);
                if buf.len() > MAX_REDIS_REQUEST_BYTES {
                    return Ok(()); // runaway request guard
                }
            }
            msg = rx.recv() => {
                match msg {
                    Some(m) => {
                        let mut out = Vec::new();
                        let response = m.to_resp();
                        if responses_within_budget(std::slice::from_ref(&response)) {
                            response.encode(conn.proto, &mut out);
                        } else {
                            Resp::Error("ERR response exceeds resource limits".into())
                                .encode(conn.proto, &mut out);
                        }
                        s.write_all(&out).await?;
                    }
                    None => return Ok(()), // mailbox closed (shouldn't happen)
                }
            }
        }
    }
}

/// Fail-closed startup policy for the direct RESP listener
/// (MEDULLA-DURABLE-KERNEL-R860.5): loopback only, and a non-empty engine key
/// for the verified principal binding, checked before bind.
pub fn validate_startup_policy(addr: &str, auth_secret: &str) -> std::io::Result<()> {
    medulla_server_api::api::dispatch_support::validate_direct_wire_security(
        addr,
        "redis-wire",
        !auth_secret.is_empty(),
    )
}

/// Bind `addr` and serve Redis RESP connections until the process exits. Spawned by
/// `main.rs` only when built `--features redis-wire` AND `MEDULLA_REDIS_ADDR`
/// is set (CONCEPT:MEDULLA-KG.ontology.resp2-resp3-codec-round). The listener owns no store: every command enters the
/// served KV methods through the dispatch with the verified context of the
/// authenticated principal, so the keyspace is the engine's own durable KV. The
/// direct listener is loopback-only and requires a non-empty engine auth secret
/// before bind.
pub async fn serve(addr: &str, state: Arc<RwLock<ServerState>>) -> std::io::Result<()> {
    let auth_secret = state.read().await.auth_secret.clone();
    validate_startup_policy(addr, &auth_secret)?;
    serve_listener(addr, Arc::new(KvSeam::new(state)), auth_secret).await
}

/// Private authenticated listener seam used by the production boundary and its
/// socket-level tests. There is no public unowned or unauthenticated server entry.
async fn serve_listener(addr: &str, kv: Arc<KvSeam>, auth_secret: String) -> std::io::Result<()> {
    if auth_secret.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "redis-wire requires non-empty authentication key material",
        ));
    }
    let listener = TcpListener::bind(addr).await?;
    // One pub/sub registry per listener, shared across every accepted connection
    // (CONCEPT:MEDULLA-KG.txn.pubsub-transactions).
    let pubsub = Arc::new(PubSub::default());
    tracing::info!(
        "redis-wire: serving authenticated, principal-scoped Redis RESP protocol on {}",
        addr
    );
    loop {
        let (mut socket, peer) = listener.accept().await?;
        let kv = kv.clone();
        let pubsub = pubsub.clone();
        let auth_secret = auth_secret.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(&mut socket, kv, pubsub, auth_secret).await {
                tracing::debug!("redis-wire connection from {peer} ended: {e}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    //! CONCEPT:MEDULLA-KG.ontology.resp2-resp3-codec-round — RESP2/RESP3 codec round-trips, command-parse coverage, the
    //! Redis data-type command execution over the KV store, plus an in-process
    //! listener smoke test driving the private authenticated listener over a TCP socket
    //! with hand-built RESP frames (no redis client crate).
    use super::*;
    use tokio::net::TcpStream;

    const TEST_SECRET: &str = "redis-test-auth-secret";
    const TEST_PRINCIPAL: &str = "synthetic-test-principal";

    /// A handle on a fresh test engine. The unit tests reach it only for the
    /// commands that never enter a KV method; the served KV path is proved
    /// through the real listener in `tests/resp_roundtrip.rs`.
    fn kv_seam() -> Arc<KvSeam> {
        let state = medulla_server::seams_install::new_state_for_test(
            TEST_SECRET,
            ServerState::test_isolation("resp-unit"),
        );
        Arc::new(KvSeam::new(Arc::new(RwLock::new(state))))
    }

    fn authenticated_connection(proto: u8, principal: &str) -> ConnState {
        let mut connection = ConnState::new(3);
        connection.actor_scope = Some(
            medulla_server_api::api::dispatch_support::pseudonymous_broker_actor(
                TEST_SECRET,
                principal,
            )
            .unwrap(),
        );
        connection.proto = proto;
        connection
    }

    fn conn3() -> ConnState {
        authenticated_connection(3, TEST_PRINCIPAL)
    }

    fn a(parts: &[&str]) -> Vec<Vec<u8>> {
        parts.iter().map(|s| s.as_bytes().to_vec()).collect()
    }

    fn enc(r: &Resp, proto: u8) -> Vec<u8> {
        let mut out = Vec::new();
        r.encode(proto, &mut out);
        out
    }

    // ── codec round-trips ──────────────────────────────────────────────────────

    #[test]
    fn eg174_resp2_encodes_core_types() {
        assert_eq!(enc(&Resp::Simple("OK".into()), 2), b"+OK\r\n");
        assert_eq!(enc(&Resp::Error("ERR x".into()), 2), b"-ERR x\r\n");
        assert_eq!(enc(&Resp::Int(42), 2), b":42\r\n");
        assert_eq!(enc(&Resp::bulk_str("hi"), 2), b"$2\r\nhi\r\n");
        assert_eq!(enc(&Resp::Bulk(None), 2), b"$-1\r\n");
        assert_eq!(enc(&Resp::Null, 2), b"$-1\r\n");
        assert_eq!(
            enc(
                &Resp::Array(Some(vec![Resp::Int(1), Resp::bulk_str("a")])),
                2
            ),
            b"*2\r\n:1\r\n$1\r\na\r\n"
        );
    }

    #[test]
    fn eg174_resp3_extended_types_downgrade_on_resp2() {
        // Map: RESP3 uses '%', RESP2 flattens to a 2N array.
        let map = Resp::Map(vec![(Resp::bulk_str("f"), Resp::bulk_str("v"))]);
        assert_eq!(enc(&map, 3), b"%1\r\n$1\r\nf\r\n$1\r\nv\r\n");
        assert_eq!(enc(&map, 2), b"*2\r\n$1\r\nf\r\n$1\r\nv\r\n");
        // Set: '~' on RESP3, '*' on RESP2.
        let set = Resp::Set(vec![Resp::bulk_str("m")]);
        assert_eq!(enc(&set, 3), b"~1\r\n$1\r\nm\r\n");
        assert_eq!(enc(&set, 2), b"*1\r\n$1\r\nm\r\n");
        // Double + Bool + Null native on RESP3, downgraded on RESP2.
        assert_eq!(enc(&Resp::Double(1.5), 3), b",1.5\r\n");
        assert_eq!(enc(&Resp::Double(1.5), 2), b"$3\r\n1.5\r\n");
        assert_eq!(enc(&Resp::Bool(true), 3), b"#t\r\n");
        assert_eq!(enc(&Resp::Bool(true), 2), b":1\r\n");
        assert_eq!(enc(&Resp::Null, 3), b"_\r\n");
    }

    #[test]
    fn eg174_parse_array_and_inline_commands() {
        // Array form (redis-cli): *2 $3 GET $1 k
        let wire = b"*2\r\n$3\r\nGET\r\n$1\r\nk\r\n";
        let (args, consumed) = try_parse_command(wire).unwrap().unwrap();
        assert_eq!(args, a(&["GET", "k"]));
        assert_eq!(consumed, wire.len());
        // Inline form (telnet).
        let (args, consumed) = try_parse_command(b"PING hello\r\n").unwrap().unwrap();
        assert_eq!(args, a(&["PING", "hello"]));
        assert_eq!(consumed, "PING hello\r\n".len());
        // Partial array → None (need more bytes).
        assert!(try_parse_command(b"*2\r\n$3\r\nGE").unwrap().is_none());
        assert!(try_parse_command(b"*1\r\n$3\r\nGETxx").is_err());
    }

    #[test]
    fn eg174_glob_match() {
        assert!(glob_match("user:*", "user:42"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("user:*", "post:1"));
    }

    // ── session gate and refusals (no KV method is entered) ──────────────────

    #[tokio::test]
    async fn eg174_auth_gate_and_hello_upgrade() {
        let kv = kv_seam();
        let mut c = ConnState::new(2);
        // Data command before AUTH → NOAUTH.
        match execute(&kv, &a(&["GET", "k"]), &mut c, TEST_SECRET).await {
            Resp::Error(e) => assert!(e.starts_with("NOAUTH"), "{e}"),
            other => panic!("expected NOAUTH, got {other:?}"),
        }
        // Wrong password rejected, and binds nothing.
        assert!(matches!(
            execute(
                &kv,
                &a(&["AUTH", TEST_PRINCIPAL, "not-a-valid-credential"]),
                &mut c,
                TEST_SECRET
            )
            .await,
            Resp::Error(e) if e.starts_with("WRONGPASS")
        ));
        assert!(c.actor_scope.is_none());
        // Correct password accepted → subsequent command allowed.
        let credential = derive_redis_password(TEST_SECRET, TEST_PRINCIPAL);
        assert_eq!(
            execute(
                &kv,
                &a(&["AUTH", TEST_PRINCIPAL, &credential]),
                &mut c,
                TEST_SECRET
            )
            .await,
            Resp::Simple("OK".into())
        );
        assert_eq!(
            execute(&kv, &a(&["PING"]), &mut c, TEST_SECRET).await,
            Resp::Simple("PONG".into())
        );
        // HELLO 3 upgrades the protocol.
        assert!(matches!(
            execute(&kv, &a(&["HELLO", "3"]), &mut c, TEST_SECRET).await,
            Resp::Map(_)
        ));
        assert_eq!(c.proto, 3);
    }

    #[tokio::test]
    async fn unsupported_families_are_refused_before_the_engine() {
        let kv = kv_seam();
        let mut c = conn3();
        for command in [
            &["HSET", "h", "f", "v"][..],
            &["LPUSH", "l", "v"],
            &["SADD", "s", "v"],
            &["ZADD", "z", "1", "v"],
            &["EXPIRE", "k", "10"],
            &["TTL", "k"],
            &["MULTI"],
            &["SET", "k", "v", "EX", "10"],
        ] {
            assert_eq!(
                execute(&kv, &a(command), &mut c, TEST_SECRET).await,
                Resp::Error("ERR RESP_COMMAND_UNSUPPORTED".into()),
                "{command:?}"
            );
        }
        assert!(matches!(
            execute(&kv, &a(&["FROBNICATE"]), &mut c, TEST_SECRET).await,
            Resp::Error(e) if e.starts_with("ERR unknown command")
        ));
    }

    #[tokio::test]
    async fn redis_principals_have_isolated_pubsub() {
        let kv = kv_seam();
        let pubsub = ps();
        let mut first = authenticated_connection(3, "synthetic-principal-one");
        let mut second = authenticated_connection(3, "synthetic-principal-two");
        first.id = pubsub
            .register(mpsc::channel(PUBSUB_MAILBOX_CAPACITY).0)
            .unwrap();
        second.id = pubsub
            .register(mpsc::channel(PUBSUB_MAILBOX_CAPACITY).0)
            .unwrap();
        assert_eq!(
            dispatch(
                &kv,
                &pubsub,
                &a(&["SUBSCRIBE", "events"]),
                &mut first,
                TEST_SECRET
            )
            .await
            .len(),
            1
        );
        assert_eq!(
            dispatch(
                &kv,
                &pubsub,
                &a(&["PUBLISH", "events", "private"]),
                &mut second,
                TEST_SECRET
            )
            .await,
            vec![Resp::Int(0)]
        );
    }

    // ── in-process listener smoke test over a real socket ────────────────────────

    async fn read_reply(stream: &mut TcpStream) -> Vec<u8> {
        // Read once; the smoke test replies are small and fit one read.
        let mut buf = [0u8; 4096];
        let n = stream.read(&mut buf).await.unwrap();
        buf[..n].to_vec()
    }

    async fn authenticate(stream: &mut TcpStream) {
        let credential = derive_redis_password(TEST_SECRET, TEST_PRINCIPAL);
        let frame = format!("AUTH {TEST_PRINCIPAL} {credential}\r\n");
        stream.write_all(frame.as_bytes()).await.unwrap();
        assert_eq!(read_reply(stream).await, b"+OK\r\n");
    }

    #[tokio::test]
    async fn eg174_listener_roundtrip_over_tcp() {
        let addr = spawn_listener().await;
        let mut s = TcpStream::connect(&addr).await.unwrap();
        authenticate(&mut s).await;
        // PING (inline).
        s.write_all(b"PING\r\n").await.unwrap();
        assert_eq!(read_reply(&mut s).await, b"+PONG\r\n");
        // A refused family answers without entering the engine (array form).
        s.write_all(b"*4\r\n$4\r\nHSET\r\n$1\r\nh\r\n$1\r\nf\r\n$1\r\nv\r\n")
            .await
            .unwrap();
        assert_eq!(
            read_reply(&mut s).await,
            b"-ERR RESP_COMMAND_UNSUPPORTED\r\n"
        );
    }

    // ── pub/sub + transactions (CONCEPT:MEDULLA-KG.txn.pubsub-transactions) ──────────────────────────────────

    fn ps() -> PubSub {
        PubSub::default()
    }

    /// Read a reply, but fail (rather than hang forever) if none arrives.
    async fn read_reply_timeout(stream: &mut TcpStream) -> Vec<u8> {
        tokio::time::timeout(std::time::Duration::from_secs(3), read_reply(stream))
            .await
            .expect("timed out waiting for a reply")
    }

    /// Bind the authenticated listener on an ephemeral port and return the address.
    async fn spawn_listener() -> String {
        let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = probe.local_addr().unwrap().to_string();
        drop(probe);
        let serve_addr = addr.clone();
        tokio::spawn(async move {
            let _ = serve_listener(&serve_addr, kv_seam(), TEST_SECRET.to_string()).await;
        });
        // GOC-70: bounded-retry readiness probe instead of a fixed pre-connect
        // sleep -- a flat wait assumes the listener bound within an arbitrary
        // window, not guaranteed on a contended/low-core host. 1s budget (50 * 20ms).
        for _ in 0..50 {
            if TcpStream::connect(&addr).await.is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        addr
    }

    #[tokio::test]
    async fn eg307_publish_with_no_subscribers_returns_zero() {
        let kv = kv_seam();
        let pubsub = ps();
        let mut c = conn3();
        assert_eq!(
            dispatch(
                &kv,
                &pubsub,
                &a(&["PUBLISH", "ch", "hi"]),
                &mut c,
                TEST_SECRET
            )
            .await,
            vec![Resp::Int(0)]
        );
    }

    #[tokio::test]
    async fn eg307_subscribe_confirm_and_count() {
        let kv = kv_seam();
        let pubsub = ps();
        let mut c = conn3();
        c.id = pubsub
            .register(mpsc::channel(PUBSUB_MAILBOX_CAPACITY).0)
            .unwrap();
        let replies = dispatch(
            &kv,
            &pubsub,
            &a(&["SUBSCRIBE", "a", "b"]),
            &mut c,
            TEST_SECRET,
        )
        .await;
        // One confirmation per channel, with a running total count.
        assert_eq!(
            replies,
            vec![
                Resp::Push(vec![
                    Resp::bulk_str("subscribe"),
                    Resp::bulk_str("a"),
                    Resp::Int(1)
                ]),
                Resp::Push(vec![
                    Resp::bulk_str("subscribe"),
                    Resp::bulk_str("b"),
                    Resp::Int(2)
                ]),
            ]
        );
        // UNSUBSCRIBE with no args drops all, count falls back to 0.
        let un = dispatch(&kv, &pubsub, &a(&["UNSUBSCRIBE"]), &mut c, TEST_SECRET).await;
        assert_eq!(un.len(), 2);
        assert!(c.sub_channels.is_empty());
    }

    #[tokio::test]
    async fn eg307_publish_subscribe_delivery_over_tcp() {
        let addr = spawn_listener().await;
        // Subscriber connection.
        let mut sub = TcpStream::connect(&addr).await.unwrap();
        authenticate(&mut sub).await;
        sub.write_all(b"*2\r\n$9\r\nSUBSCRIBE\r\n$4\r\nnews\r\n")
            .await
            .unwrap();
        // The subscribe confirmation proves the registration landed.
        let confirm = read_reply_timeout(&mut sub).await;
        let confirm = String::from_utf8_lossy(&confirm);
        assert!(confirm.contains("subscribe"), "{confirm}");

        // Publisher connection.
        let mut pubc = TcpStream::connect(&addr).await.unwrap();
        authenticate(&mut pubc).await;
        pubc.write_all(b"*3\r\n$7\r\nPUBLISH\r\n$4\r\nnews\r\n$5\r\nhello\r\n")
            .await
            .unwrap();
        // PUBLISH reports exactly one receiver.
        assert_eq!(read_reply_timeout(&mut pubc).await, b":1\r\n");

        // The subscriber is pushed the message frame.
        let msg = read_reply_timeout(&mut sub).await;
        let msg = String::from_utf8_lossy(&msg);
        assert!(msg.contains("message"), "{msg}");
        assert!(msg.contains("news"), "{msg}");
        assert!(msg.contains("hello"), "{msg}");
    }

    #[tokio::test]
    async fn eg307_psubscribe_glob_delivery_over_tcp() {
        let addr = spawn_listener().await;
        let mut sub = TcpStream::connect(&addr).await.unwrap();
        authenticate(&mut sub).await;
        // PSUBSCRIBE news.* — a glob pattern.
        sub.write_all(b"*2\r\n$10\r\nPSUBSCRIBE\r\n$6\r\nnews.*\r\n")
            .await
            .unwrap();
        let confirm = read_reply_timeout(&mut sub).await;
        assert!(String::from_utf8_lossy(&confirm).contains("psubscribe"));

        let mut pubc = TcpStream::connect(&addr).await.unwrap();
        authenticate(&mut pubc).await;
        // Publish to news.tech — matches the glob.
        pubc.write_all(b"*3\r\n$7\r\nPUBLISH\r\n$9\r\nnews.tech\r\n$2\r\nhi\r\n")
            .await
            .unwrap();
        assert_eq!(read_reply_timeout(&mut pubc).await, b":1\r\n");

        let msg = read_reply_timeout(&mut sub).await;
        let msg = String::from_utf8_lossy(&msg);
        // A pattern delivery carries the pattern AND the concrete channel.
        assert!(msg.contains("pmessage"), "{msg}");
        assert!(msg.contains("news.*"), "{msg}");
        assert!(msg.contains("news.tech"), "{msg}");
        assert!(msg.contains("hi"), "{msg}");

        // A non-matching publish must NOT be delivered (0 receivers).
        pubc.write_all(b"*3\r\n$7\r\nPUBLISH\r\n$4\r\nchat\r\n$1\r\nx\r\n")
            .await
            .unwrap();
        assert_eq!(read_reply_timeout(&mut pubc).await, b":0\r\n");
    }
}
