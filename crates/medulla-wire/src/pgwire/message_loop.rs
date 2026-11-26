//! The pgwire connection loop (MEDULLA-ENGINE-PARITY-R006.2).
//!
//! The loop is the one `pgwire::tokio::process_socket` runs (negotiate TLS, read a
//! frame, hand it to the handlers, answer an error through `process_error`), with one
//! addition: a frontend message the engine does not implement (`FunctionCall`,
//! `CopyBoth`) is refused with `FEATURE_NOT_SUPPORTED` (SQLSTATE `0A000`) and the
//! connection keeps serving. The stock loop cannot do that: the frame decoder returns
//! `InvalidMessageType` for a tag it does not know, and the stock loop ends the
//! connection on any decode error without an `ErrorResponse`.
//!
//! A connection whose startup parameter is `replication=database` is handed to
//! [`super::replication::serve_walsender`] the moment login completes
//! (MEDULLA-DURABLE-KERNEL-R035.7.2): the walsender alone speaks copy-both, on the raw
//! transport, so the pgwire codec and its refusal of the `CopyBoth` tag stay exactly as they are
//! for every ordinary connection.
//!
//! The decoder leaves the unknown frame in the read buffer. The loop drops exactly
//! that frame (its declared length), rebuilds the framed stream so the decoder starts
//! clean on whatever follows, and answers per connection state.

use std::io;

use futures::StreamExt;
use pgwire::api::{ClientInfo, ErrorHandler, PgWireConnectionState, PgWireServerHandlers};
use pgwire::error::PgWireError;
use pgwire::messages::PgWireFrontendMessage;
use pgwire::tokio::server::{
    negotiate_tls, process_error, process_message, MaybeTls, PgWireMessageServerCodec,
};
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio::time::Duration;
use tokio_util::codec::Framed;

/// Frontend message tags the engine does not implement, with the protocol name the
/// refusal reports. A tag outside this table is not a message the protocol defines,
/// and ends the connection like any other undecodable frame.
const UNSUPPORTED_MESSAGES: [(u8, &str); 2] = [(b'F', "FunctionCall"), (b'W', "CopyBoth")];

/// A frame starts with its tag byte and an `int32` length that counts itself.
const FRAME_HEADER_LEN: usize = 5;

/// The protocol name of an unsupported frontend message tag.
fn unsupported_message(tag: u8) -> Option<&'static str> {
    UNSUPPORTED_MESSAGES
        .iter()
        .find(|(known, _)| *known == tag)
        .map(|(_, name)| *name)
}

type Connection<S> = Framed<MaybeTls, PgWireMessageServerCodec<S>>;

/// What a connection factory gives the loop when a connection turns out to be a replication
/// connection: the backend whose session the walsender dispatches through.
pub(super) trait ReplicationHost {
    fn replication_backend(&self) -> std::sync::Arc<super::EngineBackend>;
}

/// The startup parameter that opens a replication connection.
const REPLICATION_PARAMETER: &str = "replication";

/// What a connection asked for through its `replication` startup parameter.
#[derive(Debug, PartialEq, Eq)]
enum ReplicationRequest {
    /// An ordinary connection.
    None,
    /// `replication=database`: a logical replication connection.
    Logical,
    /// Any other value, such as physical replication, which the engine does not serve.
    Unsupported(String),
}

fn replication_request<C: ClientInfo>(client: &C) -> ReplicationRequest {
    match client.metadata().get(REPLICATION_PARAMETER) {
        None => ReplicationRequest::None,
        Some(value) if value.eq_ignore_ascii_case("database") => ReplicationRequest::Logical,
        Some(value) => ReplicationRequest::Unsupported(value.clone()),
    }
}

/// Refuse a replication request the engine does not serve and end the connection.
async fn refuse_replication<S>(socket: Connection<S>, value: &str) -> io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let mut parts = socket.into_parts();
    let refusal = super::replication::error_response(
        "0A000",
        &format!("FEATURE_NOT_SUPPORTED: replication={value} is not supported; use replication=database"),
    );
    parts.io.write_all(&refusal).await?;
    parts.io.flush().await
}

/// Hand a logged-in replication connection to the walsender. The session is bound from the
/// startup parameters first, so the login's verified authority is the one the walsender
/// dispatches under.
async fn hand_off_walsender<S>(
    socket: Connection<S>,
    backend: std::sync::Arc<super::EngineBackend>,
) -> io::Result<()> {
    use tokio::io::AsyncWriteExt;
    if let Err(error) = backend.bind_startup_from_client(&socket).await {
        let mut parts = socket.into_parts();
        let refusal = super::replication::error_response("28000", &error.to_string());
        parts.io.write_all(&refusal).await?;
        return parts.io.flush().await;
    }
    let session = backend.session.clone();
    let mut parts = socket.into_parts();
    // Anything the codec had queued but not flushed goes out first.
    if !parts.write_buf.is_empty() {
        parts.io.write_all(&parts.write_buf).await?;
        parts.io.flush().await?;
    }
    super::replication::serve_walsender(parts.io, parts.read_buf.to_vec(), session).await
}

/// A fresh framed stream over the same transport, codec and buffered bytes. A decode
/// error leaves the framed stream in a terminal state; this is how the loop resumes.
fn reframe<S>(socket: Connection<S>) -> Connection<S> {
    Framed::from_parts(socket.into_parts())
}

/// Drop the frame at the head of the read buffer, reading the rest of it from the
/// transport when it has not fully arrived. The declared length decides how much.
async fn discard_frame<S>(socket: &mut Connection<S>) -> io::Result<()> {
    let mut header = [0u8; FRAME_HEADER_LEN];
    let buffered = socket.read_buffer().len().min(FRAME_HEADER_LEN);
    header[..buffered].copy_from_slice(&socket.read_buffer()[..buffered]);
    if buffered < FRAME_HEADER_LEN {
        socket.get_mut().read_exact(&mut header[buffered..]).await?;
    }
    let declared = i32::from_be_bytes([header[1], header[2], header[3], header[4]]);
    let body_and_length = usize::try_from(declared)
        .ok()
        .filter(|length| *length >= FRAME_HEADER_LEN - 1)
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "pgwire frame length is invalid")
        })?;
    let total = 1 + body_and_length;
    let from_buffer = socket.read_buffer().len().min(total);
    drop(socket.read_buffer_mut().split_to(from_buffer));
    // Header bytes read straight from the transport already count against the frame.
    let from_transport = FRAME_HEADER_LEN - buffered;
    let remaining = total
        - from_buffer
        - if buffered < FRAME_HEADER_LEN {
            from_transport
        } else {
            0
        };
    let skipped = tokio::io::copy(
        &mut (&mut *socket.get_mut()).take(remaining as u64),
        &mut tokio::io::sink(),
    )
    .await?;
    if skipped < remaining as u64 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "pgwire frame ended before its declared length",
        ));
    }
    Ok(())
}

/// Refuse an unsupported frontend message and return the connection ready for the
/// next frame. A connection in `ReadyForQuery` or `QueryInProgress` gets the typed
/// `0A000` error and a `ReadyForQuery`. One discarding to a `Sync` (`AwaitingSync`)
/// drops the frame silently, as the protocol says. In any other state (startup,
/// authentication, `COPY`) the message is a protocol violation and the error is fatal,
/// so an unauthenticated connection never reaches `ReadyForQuery` through a refusal.
async fn refuse_unsupported<S>(
    mut socket: Connection<S>,
    tag: u8,
    name: &str,
) -> io::Result<Connection<S>> {
    discard_frame(&mut socket).await?;
    let mut socket = reframe(socket);
    match socket.state() {
        PgWireConnectionState::ReadyForQuery | PgWireConnectionState::QueryInProgress => {
            let error = super::user_err(format!(
                "FEATURE_NOT_SUPPORTED: protocol message {name} is not supported"
            ));
            process_error(&mut socket, error, false).await?;
        }
        PgWireConnectionState::AwaitingSync => {}
        _ => {
            process_error(&mut socket, PgWireError::InvalidMessageType(tag), false).await?;
        }
    }
    Ok(socket)
}

/// Serve one accepted TCP connection until the client terminates or the stream ends.
/// `startup_timeout` bounds how long a client may take to finish startup.
pub(super) async fn serve_connection<H>(
    tcp_socket: TcpStream,
    tls_acceptor: Option<pgwire::tokio::TlsAcceptor>,
    handlers: H,
    startup_timeout: Duration,
) -> io::Result<()>
where
    H: PgWireServerHandlers + ReplicationHost,
{
    let startup = tokio::time::sleep(startup_timeout);
    tokio::pin!(startup);
    let negotiated = tokio::select! {
        _ = &mut startup => return Ok(()),
        socket = negotiate_tls(tcp_socket, tls_acceptor) => socket?,
    };
    let Some(mut socket) = negotiated else {
        return Ok(());
    };

    let startup_handler = handlers.startup_handler();
    let simple_query_handler = handlers.simple_query_handler();
    let extended_query_handler = handlers.extended_query_handler();
    let copy_handler = handlers.copy_handler();
    let cancel_handler = handlers.cancel_handler();
    let error_handler = handlers.error_handler();

    loop {
        let next = if matches!(
            socket.state(),
            PgWireConnectionState::AwaitingStartup
                | PgWireConnectionState::AuthenticationInProgress
        ) {
            tokio::select! {
                _ = &mut startup => None,
                next = socket.next() => next,
            }
        } else {
            socket.next().await
        };
        match next {
            Some(Ok(msg)) => {
                // A Terminate means the client is disconnecting; some clients block
                // until the server closes, so end the loop and drop the socket.
                if matches!(msg, PgWireFrontendMessage::Terminate(_)) {
                    break;
                }
                let is_extended_query = match socket.state() {
                    PgWireConnectionState::CopyInProgress(is_extended_query) => is_extended_query,
                    _ => msg.is_extended_query(),
                };
                if let Err(mut error) = process_message(
                    msg,
                    &mut socket,
                    startup_handler.clone(),
                    simple_query_handler.clone(),
                    extended_query_handler.clone(),
                    copy_handler.clone(),
                    cancel_handler.clone(),
                )
                .await
                {
                    error_handler.on_error(&socket, &mut error);
                    process_error(&mut socket, error, is_extended_query).await?;
                } else if matches!(socket.state(), PgWireConnectionState::ReadyForQuery) {
                    match replication_request(&socket) {
                        ReplicationRequest::None => {}
                        ReplicationRequest::Logical => {
                            return hand_off_walsender(socket, handlers.replication_backend())
                                .await;
                        }
                        ReplicationRequest::Unsupported(value) => {
                            return refuse_replication(socket, &value).await;
                        }
                    }
                }
            }
            Some(Err(PgWireError::InvalidMessageType(tag)))
                if unsupported_message(tag).is_some() =>
            {
                if let Some(name) = unsupported_message(tag) {
                    socket = refuse_unsupported(socket, tag, name).await?;
                }
            }
            _ => break,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-ENGINE-PARITY-R006.2
    #[test]
    fn unsupported_tags_are_the_named_protocol_messages() {
        assert_eq!(unsupported_message(b'F'), Some("FunctionCall"));
        assert_eq!(unsupported_message(b'W'), Some("CopyBoth"));
        // A message the engine serves, and a byte the protocol does not define.
        assert_eq!(unsupported_message(b'Q'), None);
        assert_eq!(unsupported_message(b'?'), None);
    }

    // spec: MEDULLA-ENGINE-PARITY-R006.2
    #[test]
    fn every_unsupported_tag_is_unique() {
        for (index, (tag, _)) in UNSUPPORTED_MESSAGES.iter().enumerate() {
            assert!(
                UNSUPPORTED_MESSAGES[..index]
                    .iter()
                    .all(|(other, _)| other != tag),
                "tag {tag} listed twice"
            );
        }
    }
}
