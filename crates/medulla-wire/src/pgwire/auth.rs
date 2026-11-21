//! pg-wire authentication bridged to the engine identity (CONCEPT:MEDULLA-KG.query.concept-13).
//!
//! Authentication is mandatory SCRAM-SHA-256, bridged to the engine's existing
//! shared secret (`GRAPH_SERVICE_AUTH_SECRET`) and ACL identity model. There is no
//! anonymous fallback.
//!
//! ## How a pg user maps to an engine identity
//! A pg connection's `user` IS an engine `agent_id`. The connection's password is a
//! deterministic per-user token DERIVED from the engine's shared
//! `GRAPH_SERVICE_AUTH_SECRET`: `derived_password(user) = hex(HMAC-SHA256(secret,
//! "pgwire:" || user))`. An operator who holds the engine secret can compute each
//! agent's pg password offline; the engine validates the SCRAM proof against the
//! same derivation WITHOUT storing per-user passwords. The SCRAM salt is likewise
//! derived deterministically from the secret + user, so the same login always
//! re-validates (no stored salt table). On a successful login the connection's
//! actor is set to `user`, so every subsequent query runs under that
//! `AgentIdentity` against the engine ACL (`IsolationLayer::check_access`).
//!
//! ## Mode (`MEDULLA_PGWIRE_AUTH`)
//!   * `scram` — SCRAM-SHA-256 (what modern drivers negotiate). This is the only
//!     accepted mode and requires a non-empty `GRAPH_SERVICE_AUTH_SECRET`.
//!
//! Native TLS paths are configured independently through
//! `MEDULLA_PGWIRE_TLS_CERT`, `MEDULLA_PGWIRE_TLS_KEY`, and the
//! optional `MEDULLA_PGWIRE_TLS_CLIENT_CA` (mTLS) variable.
//!
//! Native TLS is optional for loopback deployments and required before a listener
//! can bind a non-loopback address. When a certificate is configured, SCRAM also
//! advertises `SCRAM-SHA-256-PLUS`, binding clients that select channel binding to
//! the configured server certificate. The SCRAM proof still binds the pg user to
//! the ACL actor independently of the transport identity.
//!
//! The mode is resolved ONCE at `serve()` startup and logged.

use std::sync::Arc;

use async_trait::async_trait;
use futures::SinkExt;

use pgwire::api::auth::finish_authentication;
use pgwire::api::auth::sasl::scram::{gen_salted_password, ScramAuth, SCRAM_ITERATIONS};
use pgwire::api::auth::sasl::SASLAuthStartupHandler;
use pgwire::api::auth::{
    AuthSource, DefaultServerParameterProvider, LoginInfo, Password, StartupHandler,
};
use pgwire::api::{
    ClientInfo, ConnectionGuard, ConnectionHandle, ConnectionManager, PgWireConnectionState,
    PidSecretKeyGenerator, RandomPidSecretKeyGenerator,
};
use pgwire::error::{ErrorInfo, PgWireError, PgWireResult};
use pgwire::messages::startup::Authentication;
use pgwire::messages::{PgWireBackendMessage, PgWireFrontendMessage};

use medulla_identity_types::identity::WireLoginKind;
use medulla_wire_core::sql_wire_auth::{
    resolve_wire_login, InvalidLogin, WireLoginSource, WireProof, INVALID_LOGIN_SQLSTATE,
};

/// Env var selecting the pgwire auth mode. Only `scram` is accepted; unset also
/// selects SCRAM.
pub const PGWIRE_AUTH_ENV: &str = "MEDULLA_PGWIRE_AUTH";

/// The resolved pgwire auth mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PgWireAuthMode {
    /// SCRAM-SHA-256 bridged to the engine secret.
    Scram,
}

impl PgWireAuthMode {
    /// Resolve the sole secure mode. Empty key material and every legacy or
    /// unknown value are startup errors rather than compatibility fallbacks.
    pub fn resolve(auth_secret: &str) -> std::io::Result<Self> {
        medulla_wire_core::sql_wire_auth::resolve_mode(
            auth_secret,
            PGWIRE_AUTH_ENV,
            "scram",
            "pgwire requires non-empty authentication key material",
            "pgwire authentication mode must be scram",
        )
        .map(|()| Self::Scram)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            PgWireAuthMode::Scram => "scram",
        }
    }

    /// Whether this mode and secret cryptographically bind a successful login's
    /// user to the ACL actor. SCRAM with an empty root secret is not a verified
    /// deployment, even though the protocol exchange itself can run.
    pub fn verified_identity_binding(self, auth_secret: &str) -> bool {
        matches!(self, PgWireAuthMode::Scram) && !auth_secret.is_empty()
    }
}

/// `derived_password(user) = hex(HMAC-SHA256(secret, "pgwire:" || user))` — the
/// per-user pg password an authorized operator computes from the engine secret.
pub fn derive_pg_password(secret: &str, user: &str) -> String {
    medulla_wire_core::sql_wire_auth::derive_hmac_hex(secret, b"pgwire:", user)
}

/// A deterministic 16-byte SCRAM salt derived from the secret + user, so the same
/// login re-validates without a stored salt table.
fn derive_salt(secret: &str, user: &str) -> Vec<u8> {
    medulla_wire_core::sql_wire_auth::derive_hmac_digest(secret, b"pgwire-salt:", user)[..16]
        .to_vec()
}

/// `AuthSource` that yields the SCRAM-salted form of the derived per-user password
/// (CONCEPT:MEDULLA-KG.query.concept-13). For SCRAM, the server stores/returns the SALTED password
/// (`Hi(password, salt, iters)`) plus the salt; pgwire's SCRAM handler verifies the
/// client's proof against it. We compute both deterministically from the engine
/// secret + the connecting user, so no password is ever persisted.
struct EngineAuthSource {
    secret: String,
    logins: Arc<dyn WireLoginSource>,
}

impl std::fmt::Debug for EngineAuthSource {
    // The secret and the login source are never printed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineAuthSource").finish_non_exhaustive()
    }
}

/// The `verifier` of a `Password` wire login: `<salt hex>:<SCRAM salted password
/// hex>`, the form the SCRAM handler checks a proof against. The plaintext is
/// never stored.
pub fn password_verifier(password: &str, salt: &[u8]) -> String {
    let salted = gen_salted_password(password, salt, SCRAM_ITERATIONS);
    format!("{}:{}", hex::encode(salt), hex::encode(salted))
}

fn decode_password_verifier(verifier: &str) -> Option<(Vec<u8>, Vec<u8>)> {
    let (salt, salted) = verifier.split_once(':')?;
    Some((hex::decode(salt).ok()?, hex::decode(salted).ok()?))
}

/// The refusal a failed wire login raises: SQLSTATE `28000`.
fn invalid_login_error(refused: &InvalidLogin) -> PgWireError {
    PgWireError::UserError(Box::new(ErrorInfo::new(
        "FATAL".to_string(),
        INVALID_LOGIN_SQLSTATE.to_string(),
        refused.to_string(),
    )))
}

#[async_trait]
impl AuthSource for EngineAuthSource {
    async fn get_password(&self, login: &LoginInfo) -> PgWireResult<Password> {
        // A missing user can never match a derived password — return a salted form
        // of an empty user so the proof check fails cleanly (no panic, no bypass).
        let user = login.user().unwrap_or("");
        // A registered wire login answers from its stored verifier. A record that
        // is revoked or of another kind never matches a proof.
        if let Some(record) = self.logins.find(user) {
            let stored = (record.is_active() && record.kind == WireLoginKind::Password)
                .then(|| decode_password_verifier(&record.verifier))
                .flatten();
            return Ok(match stored {
                Some((salt, salted)) => Password::new(Some(salt), salted),
                None => Password::new(Some(derive_salt(&self.secret, user)), Vec::new()),
            });
        }
        let salt = derive_salt(&self.secret, user);
        let derived = derive_pg_password(&self.secret, user);
        let salted = gen_salted_password(&derived, &salt, SCRAM_ITERATIONS);
        Ok(Password::new(Some(salt), salted))
    }
}

/// Which exchange a connection's login uses, decided at its startup message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoginRoute {
    Scram,
    Token,
    Certificate,
}

/// The per-connection startup handler. A login registered as `Token` or
/// `Certificate` authenticates through [`resolve_wire_login`]; every other login
/// (engine agents and `Password` records) runs SCRAM-SHA-256. pgwire builds a
/// fresh handler per connection, so the route is per-connection state.
pub struct EngineStartupHandler {
    scram: Box<SASLAuthStartupHandler<DefaultServerParameterProvider>>,
    logins: Arc<dyn WireLoginSource>,
    route: tokio::sync::Mutex<LoginRoute>,
    /// The listener's registry of live connections (MEDULLA-DURABLE-KERNEL-R035.5).
    /// Every completed login registers here, so a cancel request can find it.
    connections: Arc<ConnectionManager>,
}

/// The one backend-key generator every connection of the process draws from, so
/// two live connections never share a pid (MEDULLA-DURABLE-KERNEL-R035.5).
fn pid_secret_key_generator() -> Arc<RandomPidSecretKeyGenerator> {
    static GENERATOR: std::sync::OnceLock<Arc<RandomPidSecretKeyGenerator>> =
        std::sync::OnceLock::new();
    GENERATOR
        .get_or_init(|| Arc::new(RandomPidSecretKeyGenerator::default()))
        .clone()
}

impl EngineStartupHandler {
    /// Build the startup handler over the engine key material.
    ///
    /// `certificate_pem` is optional for loopback/plain deployments. When it is
    /// supplied, the handler advertises SCRAM channel binding and verifies the
    /// `tls-server-end-point` proof for clients that select `SCRAM-SHA-256-PLUS`.
    /// Startup validates the PEM before a listener is opened; the `expect` below
    /// therefore protects an already-validated invariant rather than accepting
    /// malformed operator input at connection time.
    pub fn new(
        mode: PgWireAuthMode,
        auth_secret: &str,
        certificate_pem: Option<&[u8]>,
        logins: Arc<dyn WireLoginSource>,
        connections: Arc<ConnectionManager>,
    ) -> Self {
        match mode {
            PgWireAuthMode::Scram => {
                let source = Arc::new(EngineAuthSource {
                    secret: auth_secret.to_string(),
                    logins: logins.clone(),
                });
                let mut scram = ScramAuth::new(source);
                if let Some(certificate_pem) = certificate_pem {
                    scram
                        .configure_certificate(certificate_pem)
                        .expect("pgwire TLS certificate was validated before listener startup");
                }
                let generator: Arc<dyn PidSecretKeyGenerator> = pid_secret_key_generator();
                let handler = SASLAuthStartupHandler::new(Arc::new(
                    DefaultServerParameterProvider::default(),
                ))
                .with_scram(scram)
                .with_pid_secret_key_generator(generator)
                .with_connection_manager(connections.clone());
                Self {
                    scram: Box::new(handler),
                    logins,
                    route: tokio::sync::Mutex::new(LoginRoute::Scram),
                    connections,
                }
            }
        }
    }

    fn route_for(&self, user: &str) -> LoginRoute {
        match self.logins.find(user) {
            Some(record) if record.is_active() => match record.kind {
                WireLoginKind::Token => LoginRoute::Token,
                WireLoginKind::Certificate => LoginRoute::Certificate,
                WireLoginKind::Password => LoginRoute::Scram,
            },
            _ => LoginRoute::Scram,
        }
    }
}

async fn complete_login<C>(client: &mut C, connections: &Arc<ConnectionManager>) -> PgWireResult<()>
where
    C: ClientInfo + futures::Sink<PgWireBackendMessage> + Unpin + Send + Sync,
    C::Error: std::fmt::Debug,
    PgWireError: From<<C as futures::Sink<PgWireBackendMessage>>::Error>,
{
    let (pid, secret_key) = pid_secret_key_generator().generate(client);
    client.set_pid_and_secret_key(pid, secret_key.clone());
    // MEDULLA-DURABLE-KERNEL-R035.5: register the connection under its backend key.
    // The handle lets pgwire race each statement against a cancel; the guard
    // unregisters the connection when the session ends.
    let (handle, guard) = connections.register(pid, secret_key);
    client
        .session_extensions()
        .insert::<Arc<ConnectionHandle>>(handle);
    client.session_extensions().insert::<ConnectionGuard>(guard);
    finish_authentication(client, &DefaultServerParameterProvider::default()).await
}

/// Validate the exact certificate bytes used by native TLS before a listener is
/// opened. This keeps SCRAM channel-binding failures in the fail-closed startup
/// path instead of discovering an invalid X.509 chain only after a client logs in.
pub fn validate_certificate(certificate_pem: &[u8]) -> PgWireResult<()> {
    let source = Arc::new(EngineAuthSource {
        secret: ["pgwire", "tls", "validation"].join("-"),
        logins: medulla_wire_core::sql_wire_auth::wire_login_registry(),
    });
    let mut scram = ScramAuth::new(source);
    scram.configure_certificate(certificate_pem)
}

#[async_trait]
impl StartupHandler for EngineStartupHandler {
    async fn on_startup<C>(
        &self,
        client: &mut C,
        message: PgWireFrontendMessage,
    ) -> PgWireResult<()>
    where
        C: ClientInfo + futures::Sink<PgWireBackendMessage> + Unpin + Send + Sync,
        C::Error: std::fmt::Debug,
        pgwire::error::PgWireError: From<<C as futures::Sink<PgWireBackendMessage>>::Error>,
    {
        if let PgWireFrontendMessage::Startup(ref startup) = message {
            let user = startup.parameters.get("user").cloned().unwrap_or_default();
            let route = self.route_for(&user);
            *self.route.lock().await = route;
            if route != LoginRoute::Scram {
                pgwire::api::auth::protocol_negotiation(client, startup).await?;
                pgwire::api::auth::save_startup_parameters_to_metadata(client, startup);
                client.set_state(PgWireConnectionState::AuthenticationInProgress);
                if route == LoginRoute::Token {
                    client
                        .send(PgWireBackendMessage::Authentication(
                            Authentication::CleartextPassword,
                        ))
                        .await?;
                    return Ok(());
                }
                // Certificate: the mTLS handshake already accepted the chain; the
                // leaf's fingerprint must match the record.
                let leaf = client
                    .client_certificates()
                    .and_then(|chain| chain.first().map(|c| c.as_ref().to_vec()))
                    .unwrap_or_default();
                resolve_wire_login(
                    self.logins.as_ref(),
                    &user,
                    WireProof::Certificate {
                        certificate_der: &leaf,
                    },
                )
                .map_err(|refused| invalid_login_error(&refused))?;
                return complete_login(client, &self.connections).await;
            }
        }
        let route = *self.route.lock().await;
        if route == LoginRoute::Token {
            if let PgWireFrontendMessage::PasswordMessageFamily(pwd) = message {
                let token = pwd.into_password()?.password;
                let user = client
                    .metadata()
                    .get(pgwire::api::METADATA_USER)
                    .cloned()
                    .unwrap_or_default();
                resolve_wire_login(self.logins.as_ref(), &user, WireProof::Token(&token))
                    .map_err(|refused| invalid_login_error(&refused))?;
                return complete_login(client, &self.connections).await;
            }
            return Ok(());
        }
        self.scram.on_startup(client, message).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_identity_types::identity::{sha256_verifier, WireLoginRecord};
    use medulla_wire_core::sql_wire_auth::{CachedWireLogins, WireLoginRegistry};

    fn record(kind: WireLoginKind, verifier: &str) -> WireLoginRecord {
        WireLoginRecord::new("bi", kind, verifier, "agent:analyst", "tenant-a", 1)
    }

    fn one(rec: WireLoginRecord) -> WireLoginRegistry {
        let registry = WireLoginRegistry::default();
        registry.put(rec).expect("put");
        registry
    }

    // spec: MEDULLA-ENGINE-PARITY-R007.1
    #[test]
    fn three_login_kinds_resolve_one_principal() {
        let salt = [7u8; 16];
        let (cert, token) = (b"client-cert-der".to_vec(), "tok-123");
        let password = one(record(
            WireLoginKind::Password,
            &password_verifier("pw", &salt),
        ));
        let stored = password.find("bi").expect("record");
        assert_eq!(
            decode_password_verifier(&stored.verifier).map(|(salt, _)| salt),
            Some(salt.to_vec()),
            "a password login answers SCRAM from its verifier"
        );
        let ok = resolve_wire_login(&password, "bi", WireProof::ScramVerified).expect("password");
        let by_cert = one(record(WireLoginKind::Certificate, &sha256_verifier(&cert)));
        let c = resolve_wire_login(
            &by_cert,
            "bi",
            WireProof::Certificate {
                certificate_der: &cert,
            },
        )
        .expect("certificate");
        let by_token = one(record(
            WireLoginKind::Token,
            &sha256_verifier(token.as_bytes()),
        ));
        let t = resolve_wire_login(&by_token, "bi", WireProof::Token(token)).expect("token");
        for resolved in [&ok, &c, &t] {
            assert_eq!(resolved.principal, "agent:analyst");
            assert_eq!(resolved.tenant, "tenant-a");
        }
        assert_eq!(
            [ok.kind, c.kind, t.kind],
            [
                WireLoginKind::Password,
                WireLoginKind::Certificate,
                WireLoginKind::Token
            ]
        );
    }

    // spec: MEDULLA-ENGINE-PARITY-R007.1
    #[test]
    fn unknown_revoked_mismatched_and_wrong_kind_logins_refuse() {
        let token = "tok-123"; // sanitizer:ignore - test placeholder token
        let mut rec = record(WireLoginKind::Token, &sha256_verifier(token.as_bytes()));
        let source = one(rec.clone());
        for refused in [
            resolve_wire_login(&source, "nobody", WireProof::Token(token)),
            resolve_wire_login(&source, "bi", WireProof::Token("wrong")),
            resolve_wire_login(&source, "bi", WireProof::ScramVerified),
        ] {
            let err = refused.expect_err("must refuse");
            assert!(err.to_string().starts_with("INVALID_LOGIN"));
        }
        rec.apply(medulla_identity_types::identity::WireLoginEvent::Revoke { now_ms: 5 })
            .expect("revoke");
        let revoked = one(rec);
        assert!(resolve_wire_login(&revoked, "bi", WireProof::Token(token)).is_err());
    }

    // spec: MEDULLA-ENGINE-PARITY-R007.1
    #[test]
    fn cache_serves_a_login_until_its_ttl_and_zero_never_caches() {
        let registry = Arc::new(WireLoginRegistry::default());
        let cached = CachedWireLogins::new(registry.clone(), 60);
        assert!(cached.find("bi").is_none());
        registry
            .put(record(WireLoginKind::Token, "ab"))
            .expect("put");
        assert!(
            cached.find("bi").is_none(),
            "the miss is cached within the ttl"
        );
        let uncached = CachedWireLogins::new(registry, 0);
        assert!(uncached.find("bi").is_some(), "ttl 0 reads through");
    }

    #[test]
    fn derived_password_is_stable_and_secret_dependent() {
        let p1 = derive_pg_password("s3cret", "agent:planner");
        let p2 = derive_pg_password("s3cret", "agent:planner");
        assert_eq!(p1, p2, "derivation is deterministic");
        assert_ne!(
            p1,
            derive_pg_password("other", "agent:planner"),
            "a different secret yields a different password"
        );
        assert_ne!(
            p1,
            derive_pg_password("s3cret", "agent:worker"),
            "a different user yields a different password"
        );
        assert_eq!(p1.len(), 64, "hex of a 32-byte HMAC");
    }

    #[test]
    fn mode_resolution_is_scram_only_and_fail_closed() {
        std::env::remove_var(PGWIRE_AUTH_ENV);
        assert_eq!(
            PgWireAuthMode::resolve("s3cret").unwrap(),
            PgWireAuthMode::Scram
        );
        assert!(PgWireAuthMode::resolve("").is_err());
        std::env::set_var(PGWIRE_AUTH_ENV, "trust");
        assert!(PgWireAuthMode::resolve("s3cret").is_err());
        std::env::set_var(PGWIRE_AUTH_ENV, "scram");
        assert_eq!(
            PgWireAuthMode::resolve("s3cret").unwrap(),
            PgWireAuthMode::Scram
        );
        std::env::remove_var(PGWIRE_AUTH_ENV);
    }

    #[test]
    fn only_scram_with_nonempty_secret_is_verified_identity_binding() {
        assert!(PgWireAuthMode::Scram.verified_identity_binding("secret"));
        assert!(!PgWireAuthMode::Scram.verified_identity_binding(""));
    }
}
