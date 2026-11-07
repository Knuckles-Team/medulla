//! The server's explicit process state (MEDULLA-SDLC-R023).
//!
//! State that a process-wide static used to hold is a field here. `main` builds
//! one context, [`ServerContext::build`], with the environment passed in as a
//! lookup, so no library code reads an environment variable for it. The
//! [`crate::state::ServerState`] holds the context, so every listener, handler
//! and worker reaches it through the state it already has. A test builds its
//! own context with [`ServerContext::fixture`]; two contexts in one process
//! share nothing.

use crate::authority_context::RequestContextPolicy;
use crate::request_replay::ReplayLedger;

/// The variable that overrides the replay ledger's directory.
#[cfg(all(
    feature = "security",
    not(test),
    not(feature = "request-policy-fixture")
))]
const SECURITY_STATE_DIR: &str = "MEDULLA_SECURITY_STATE_DIR";

/// Where the replay ledger of one context lives.
enum ReplaySource {
    /// The durable ledger under `dir`, opened on its first use, so a server
    /// that never verifies a signed request creates no ledger file.
    #[cfg(feature = "security")]
    Durable {
        dir: std::path::PathBuf,
        ledger: Box<std::sync::OnceLock<Result<crate::request_replay::RedbReplayLedger, String>>>,
    },
    /// The in-memory window of a test context.
    #[cfg(any(test, feature = "test-support"))]
    Memory(crate::request_replay::ReplayCache),
}

/// The explicit process state of one server.
pub struct ServerContext {
    request_policy: Result<RequestContextPolicy, String>,
    replay: Result<ReplaySource, String>,
}

impl ServerContext {
    /// Build the context of a serving process. `state_dir` is the persist
    /// directory; `env` looks up a configuration variable. A missing policy
    /// variable or ledger directory is kept as the refusal that startup
    /// validation and each signed request report.
    #[cfg(all(not(test), not(feature = "request-policy-fixture")))]
    pub fn build(state_dir: Option<&str>, env: &dyn Fn(&str) -> Option<String>) -> Self {
        let request_policy = RequestContextPolicy::from_lookup(env);
        #[cfg(feature = "security")]
        {
            let dir = configured(env, SECURITY_STATE_DIR)
                .or_else(|| state_dir.map(str::to_string))
                .ok_or_else(|| {
                    "secure request context requires MEDULLA_SECURITY_STATE_DIR or a persist directory"
                        .to_string()
                });
            match dir {
                Ok(dir) => Self::with_durable_replay(request_policy, std::path::Path::new(&dir)),
                Err(refusal) => Self {
                    request_policy,
                    replay: Err(refusal),
                },
            }
        }
        #[cfg(not(feature = "security"))]
        {
            let _ = state_dir;
            Self {
                request_policy,
                replay: Err("secure request context requires the security feature".to_string()),
            }
        }
    }

    /// Test builds of the crates that moved out of this one run under the
    /// fixed policy unless the environment provisions one, with the in-memory
    /// window: a parallel test process cannot share one durable ledger.
    #[cfg(all(not(test), feature = "request-policy-fixture"))]
    pub fn build(state_dir: Option<&str>, env: &dyn Fn(&str) -> Option<String>) -> Self {
        let _ = state_dir;
        let mut context = Self::fixture();
        if let Ok(policy) = RequestContextPolicy::from_lookup(env) {
            context.request_policy = Ok(policy);
        }
        context
    }

    /// The in-crate test build runs under the fixed policy.
    #[cfg(test)]
    pub fn build(state_dir: Option<&str>, env: &dyn Fn(&str) -> Option<String>) -> Self {
        let _ = (state_dir, env);
        Self::fixture()
    }

    /// A context whose replay ledger is durable under `dir`.
    #[cfg(feature = "security")]
    pub fn with_durable_replay(
        request_policy: Result<RequestContextPolicy, String>,
        dir: &std::path::Path,
    ) -> Self {
        Self {
            request_policy,
            replay: Ok(ReplaySource::Durable {
                dir: dir.to_path_buf(),
                ledger: Box::default(),
            }),
        }
    }

    /// A test context: the fixed policy and its own in-memory replay window.
    #[cfg(any(test, feature = "test-support"))]
    pub fn fixture() -> Self {
        Self {
            request_policy: Ok(RequestContextPolicy::fixture()),
            replay: Ok(ReplaySource::Memory(
                crate::request_replay::ReplayCache::new(),
            )),
        }
    }

    /// The one context the state constructors of the test harnesses share, built
    /// from the process environment the way `main` builds it. Integration tests
    /// provision their policy and ledger directory through the environment and
    /// build many states that verify against one ledger; until each builds its
    /// own context, they share this one.
    #[cfg(any(test, feature = "test-support"))]
    pub fn shared_for_tests() -> std::sync::Arc<Self> {
        static SHARED: std::sync::OnceLock<std::sync::Arc<ServerContext>> =
            std::sync::OnceLock::new();
        SHARED
            .get_or_init(|| {
                std::sync::Arc::new(Self::build(None, &|name| std::env::var(name).ok()))
            })
            .clone()
    }

    /// The deployment policy that turns signed claims into a trusted context.
    pub fn request_policy(&self) -> Result<&RequestContextPolicy, String> {
        self.request_policy.as_ref().map_err(Clone::clone)
    }

    /// The replay ledger of this context.
    pub fn replay_ledger(&self) -> Result<&dyn ReplayLedger, String> {
        let source = self.replay.as_ref().map_err(Clone::clone)?;
        match *source {
            #[cfg(feature = "security")]
            ReplaySource::Durable {
                ref dir,
                ref ledger,
            } => match ledger.get_or_init(|| crate::request_replay::RedbReplayLedger::open(dir)) {
                Ok(ledger) => Ok(ledger),
                Err(ref message) => Err(message.clone()),
            },
            #[cfg(any(test, feature = "test-support"))]
            ReplaySource::Memory(ref cache) => Ok(cache),
        }
    }

    /// Graceful shutdown: hand this context's replay window to the next
    /// incarnation (see [`crate::request_replay::RedbReplayLedger::seal`]), so a
    /// clean restart does not refuse fresh envelopes signed in the last second
    /// this one accepted. A no-op when the durable ledger was never opened.
    /// Returns the nonces handed off.
    pub fn seal_replay_ledger(&self) -> Result<usize, String> {
        #[cfg(feature = "security")]
        if let Ok(ReplaySource::Durable { ledger, .. }) = &self.replay {
            return match ledger.get() {
                Some(Ok(ledger)) => ledger.seal(),
                _ => Ok(0),
            };
        }
        Ok(0)
    }
}

/// A trimmed, non-empty configuration value.
#[cfg(all(
    feature = "security",
    not(test),
    not(feature = "request-policy-fixture")
))]
fn configured(env: &dyn Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    env(name)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
