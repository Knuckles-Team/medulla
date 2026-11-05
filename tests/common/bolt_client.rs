//! In-tree Bolt v4.4 test client (MEDULLA-DURABLE-KERNEL-R860.2).
//!
//! Speaks the standard Bolt handshake and chunked PackStream framing over a real TCP
//! socket, authenticating with the engine's `epistemic` scheme and a signed session
//! credential. No third-party Bolt driver is linked: such a driver hardcodes the
//! `basic` scheme, which the listener refuses.
#![allow(dead_code)]

use medulla_protocol_types::protocol::Request;
use medulla_wire::bolt_wire::packstream::{self, PackValue};
use medulla_wire::bolt_wire::{encode_session_credential, BOLT_AUTH_SCHEME};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const BOLT_MAGIC: [u8; 4] = [0x60, 0x60, 0xB0, 0x17];
const BOLT_VERSION_4_4: [u8; 4] = [0x00, 0x00, 0x04, 0x04];

pub const MSG_HELLO: u8 = 0x01;
pub const MSG_RUN: u8 = 0x10;
pub const MSG_PULL: u8 = 0x3F;
pub const MSG_SUCCESS: u8 = 0x70;
pub const MSG_RECORD: u8 = 0x71;
pub const MSG_IGNORED: u8 = 0x7E;
pub const MSG_FAILURE: u8 = 0x7F;

/// One Bolt reply message.
#[derive(Debug)]
pub enum BoltReply {
    Success(PackValue),
    Record(Vec<PackValue>),
    Failure(PackValue),
    Ignored,
}

pub struct BoltClient {
    stream: TcpStream,
}

impl BoltClient {
    /// Connect and negotiate Bolt 4.4.
    pub async fn connect(addr: &str) -> std::io::Result<Self> {
        let mut stream = TcpStream::connect(addr).await?;
        stream.write_all(&BOLT_MAGIC).await?;
        let mut proposals = [0u8; 16];
        proposals[0..4].copy_from_slice(&BOLT_VERSION_4_4);
        stream.write_all(&proposals).await?;
        stream.flush().await?;
        let mut chosen = [0u8; 4];
        stream.read_exact(&mut chosen).await?;
        if chosen != BOLT_VERSION_4_4 {
            return Err(std::io::Error::other(format!(
                "server did not negotiate Bolt 4.4: {chosen:?}"
            )));
        }
        Ok(Self { stream })
    }

    async fn send(&mut self, msg: &PackValue) -> std::io::Result<()> {
        self.stream
            .write_all(&packstream::encode_chunked(msg))
            .await?;
        self.stream.flush().await
    }

    async fn recv(&mut self) -> std::io::Result<BoltReply> {
        let mut body = Vec::new();
        loop {
            let mut header = [0u8; 2];
            self.stream.read_exact(&mut header).await?;
            let len = u16::from_be_bytes(header) as usize;
            if len == 0 {
                if body.is_empty() {
                    continue; // NOOP keep-alive chunk
                }
                break;
            }
            let start = body.len();
            body.resize(start + len, 0);
            self.stream.read_exact(&mut body[start..]).await?;
        }
        let value = packstream::decode(&body)
            .map_err(|e| std::io::Error::other(format!("packstream decode: {e:?}")))?;
        match value {
            PackValue::Structure { tag, mut fields } => {
                let first = if fields.is_empty() {
                    PackValue::Null
                } else {
                    fields.remove(0)
                };
                Ok(match tag {
                    MSG_SUCCESS => BoltReply::Success(first),
                    MSG_FAILURE => BoltReply::Failure(first),
                    MSG_IGNORED => BoltReply::Ignored,
                    MSG_RECORD => match first {
                        PackValue::List(values) => BoltReply::Record(values),
                        other => BoltReply::Record(vec![other]),
                    },
                    other => {
                        return Err(std::io::Error::other(format!(
                            "unexpected Bolt message tag {other:#04x}"
                        )))
                    }
                })
            }
            other => Err(std::io::Error::other(format!(
                "Bolt reply is not a structure: {other:?}"
            ))),
        }
    }

    /// `HELLO` with the `epistemic` scheme and `session` (a signed `Health` request)
    /// as the credential.
    pub async fn hello(&mut self, session: &Request) -> std::io::Result<BoltReply> {
        let credential = encode_session_credential(session).map_err(std::io::Error::other)?;
        self.send(&PackValue::Structure {
            tag: MSG_HELLO,
            fields: vec![PackValue::map(vec![
                (
                    "user_agent",
                    PackValue::String("medulla-test-bolt/1.0".into()),
                ),
                ("scheme", PackValue::String(BOLT_AUTH_SCHEME.into())),
                ("principal", PackValue::String("principal:opaque".into())),
                ("credentials", PackValue::String(credential)),
            ])],
        })
        .await?;
        self.recv().await
    }

    /// `RUN query` then `PULL {n: -1}`; returns the records, or the FAILURE metadata.
    pub async fn run_pull(
        &mut self,
        query: &str,
    ) -> std::io::Result<Result<Vec<Vec<PackValue>>, PackValue>> {
        self.send(&PackValue::Structure {
            tag: MSG_RUN,
            fields: vec![
                PackValue::String(query.into()),
                PackValue::map(vec![]),
                PackValue::map(vec![]),
            ],
        })
        .await?;
        match self.recv().await? {
            BoltReply::Success(_) => {}
            BoltReply::Failure(meta) => return Ok(Err(meta)),
            other => return Err(std::io::Error::other(format!("RUN answered {other:?}"))),
        }
        self.send(&PackValue::Structure {
            tag: MSG_PULL,
            fields: vec![PackValue::map(vec![("n", PackValue::Int(-1))])],
        })
        .await?;
        let mut records = Vec::new();
        loop {
            match self.recv().await? {
                BoltReply::Record(values) => records.push(values),
                BoltReply::Success(_) => return Ok(Ok(records)),
                BoltReply::Failure(meta) => return Ok(Err(meta)),
                BoltReply::Ignored => {
                    return Err(std::io::Error::other("PULL was ignored"));
                }
            }
        }
    }
}
