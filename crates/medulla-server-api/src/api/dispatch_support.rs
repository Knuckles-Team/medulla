//! Helpers and types moved down from the server dispatch layer.

/// Run blocking store work off the async runtime; a task that panics or is
/// cancelled answers `"<task> task failed: …"`, the work's own error otherwise.
pub async fn blocking_task<T: Send + 'static>(
    task: &str,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| format!("{task} task failed: {error}"))?
}

fn loopback_hostname_addr(addr: &str) -> bool {
    addr.rsplit_once(':')
        .map(|(host, port)| {
            host.trim_matches(|character| character == '[' || character == ']')
                .eq_ignore_ascii_case("localhost")
                && !port.is_empty()
                && port.chars().all(|character| character.is_ascii_digit())
        })
        .unwrap_or(false)
}

fn direct_wire_addr_is_loopback(addr: &str) -> bool {
    addr.parse::<std::net::SocketAddr>()
        .map(|socket| socket.ip().is_loopback())
        .unwrap_or_else(|_| loopback_hostname_addr(addr))
}

/// Validate a plaintext database-compatibility listener before it binds.
///
/// PGWire, MySQL, MSSQL TDS, Bolt, AMQP, MQTT and STOMP do not terminate TLS themselves. Their direct
/// listeners therefore remain loopback-only even when the broader auxiliary
/// two-key ingress exception is enabled. A routable deployment terminates TLS
/// (preferably mTLS) in an identity-aware sidecar/gateway and connects that
/// gateway to this loopback backend. The protocol login must be
/// cryptographically verified before its principal becomes an engine ACL actor;
/// there is no anonymous or trust compatibility mode.
pub fn validate_direct_wire_security(
    addr: &str,
    surface: &str,
    verified_identity_binding: bool,
) -> std::io::Result<()> {
    if !direct_wire_addr_is_loopback(addr) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "{surface} direct listener is plaintext and must bind loopback; expose it only through an authenticated TLS identity-binding gateway"
            ),
        ));
    }
    if !verified_identity_binding {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "{surface} listener requires cryptographically verified login-to-actor binding"
            ),
        ));
    }
    Ok(())
}

/// Convert a successfully authenticated broker principal into a stable, bounded,
/// non-reversible actor reference before it reaches engine authorization or durable
/// audit state. The same principal maps to the same actor across AMQP/MQTT/STOMP,
/// while the deployment secret prevents offline recovery of low-entropy usernames.
pub fn pseudonymous_broker_actor(secret: &str, principal: &str) -> std::io::Result<String> {
    if secret.is_empty() || principal.is_empty() || principal.len() > 4 * 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "broker identity cannot be pseudonymized",
        ));
    }
    use hmac::Mac as _;
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(b"broker-actor:");
    mac.update(principal.as_bytes());
    Ok(format!(
        "broker:actor:hmac-sha256:{}",
        hex::encode(mac.finalize().into_bytes())
    ))
}

/// Refuse an auxiliary listener that is reachable off-host. Auxiliary HTTP and
/// database-wire protocols terminate only on loopback; remote deployments place
/// an authenticated TLS identity-binding gateway in front of that local socket.
pub fn require_loopback_listener(listener: &tokio::net::TcpListener) -> std::io::Result<()> {
    let address = listener.local_addr()?;
    if address.ip().is_loopback() {
        return Ok(());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "auxiliary listeners must bind loopback",
    ))
}

/// A temp dir guaranteed unique even across concurrent tests in one process.
///
/// `pid + nanos` is NOT sufficient: two threads can observe the same nanosecond,
/// and redb takes an EXCLUSIVE per-file lock, so the loser fails with
/// `Database already open. Cannot acquire lock.` That is exactly how
/// `medulla_server::lake::rest::tests` began failing once the suite ran on a 64-core
/// host -- more tests in flight at once, so the collision window is actually
/// hit. The monotonic counter removes the race rather than narrowing it.
///
/// Gated on `redb` (not `blob`): every current caller (this module's
/// `test_state`, `medulla_h_ingest::blob::store`, `medulla_server::lake::rest`) needs a durable
/// redb-backed temp dir, and `blob`/`lake` both imply `redb` transitively, so
/// this is the widest feature that still keeps the fn used (never dead code)
/// in every build that calls it.
#[cfg(feature = "redb")]
pub fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "{prefix}-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
        SEQ.fetch_add(1, Ordering::Relaxed),
    ))
}

/// Preserve the existing outbound HTTP defaults during the ureq migration.
#[cfg(any(
    feature = "federation-search",
    feature = "otel-export",
    feature = "lake",
    feature = "oidc"
))]
pub fn outbound_http_config() -> ureq::config::ConfigBuilder<ureq::typestate::AgentScope> {
    ureq::Agent::config_builder()
        .proxy(None)
        .max_redirects(5)
        .timeout_connect(Some(std::time::Duration::from_secs(30)))
}

#[cfg(test)]
mod tests {
    use super::{pseudonymous_broker_actor, validate_direct_wire_security};

    #[test]
    fn nonloopback_direct_wire_is_always_rejected() {
        assert!(validate_direct_wire_security("0.0.0.0:5433", "pgwire", true).is_err());
        assert!(validate_direct_wire_security("192.0.2.10:3306", "mysql-wire", true).is_err());
    }

    #[test]
    fn anonymous_loopback_wire_is_rejected_in_every_profile() {
        assert!(validate_direct_wire_security("127.0.0.1:5433", "pgwire", false).is_err());
        assert!(validate_direct_wire_security("localhost:7687", "bolt-wire", false).is_err());
    }

    #[test]
    fn verified_loopback_wire_is_accepted() {
        assert!(validate_direct_wire_security("[::1]:5433", "pgwire", true).is_ok());
        assert!(validate_direct_wire_security("127.0.0.1:7687", "bolt-wire", true).is_ok());
    }

    #[test]
    fn broker_actor_reference_is_stable_opaque_and_secret_bound() {
        let actor = pseudonymous_broker_actor("secret", "human-readable-name").unwrap();
        assert_eq!(
            actor,
            pseudonymous_broker_actor("secret", "human-readable-name").unwrap()
        );
        assert!(!actor.contains("human-readable-name"));
        assert_ne!(
            actor,
            pseudonymous_broker_actor("other", "human-readable-name").unwrap()
        );
        assert!(pseudonymous_broker_actor("", "principal").is_err());
    }
}
