//! Offline conversion of an anonymous store to local accounts (MEDULLA-IDENTITY-R003.7).
//!
//! The served mode transition refuses a move out of `none` (`MODE_TRANSITION_FORBIDDEN`), so
//! this command is the only way out. It works against a stopped engine's data directory and
//! touches only the policy store `rbac.redb` and the anonymous marker file; every graph,
//! blob and index store is kept. Its three steps are resumable: a crash between steps leaves
//! the marker in place, so authenticated startup still refuses the store and a rerun finishes
//! the steps that remain.

use std::collections::BTreeSet;
use std::path::Path;

use medulla_identity_types::identity::{
    AuthMode, ConfigOp, IdentityActor, IdentityOp, IdentityRefusal, IdentityStamp,
    InitializeRequest, Secret, DEFAULT_PASSWORD_MIN_CHARS,
};
use medulla_isolation::isolation::IsolationLayer;
use medulla_server_identity::dispatch::identity_store::stamp::stamp_initialize;

use crate::server_startup::{ANONYMOUS_AGENT_ID, ANONYMOUS_MARKER_BYTES, ANONYMOUS_MARKER_FILE};

/// The refusal when `--confirm` is absent.
pub const IDENTITY_MODE_NOT_CONFIRMED: &str = "IDENTITY_MODE_NOT_CONFIRMED";
/// The refusal when the store does not hold the anonymous marker.
pub const IDENTITY_MODE_NOT_ANONYMOUS: &str = "IDENTITY_MODE_NOT_ANONYMOUS";

/// The principal an offline conversion acts as.
const OFFLINE_PRINCIPAL: &str = "operator:offline";

/// What one `identity-mode set` run did.
#[derive(Debug, serde::Serialize)]
pub struct IdentityModeReport {
    /// The verb that ran.
    pub verb: &'static str,
    /// The steps that completed, in order.
    pub steps: Vec<&'static str>,
    /// Why the command refused, when it did.
    pub error: Option<String>,
    /// The process exit status: 0 done, 1 refused.
    pub exit_code: i32,
}

impl IdentityModeReport {
    /// One line per completed step, then the report as a single JSON object on the last line.
    pub fn render(&self) -> String {
        let mut out = format!("identity-mode {}\n", self.verb);
        for step in &self.steps {
            out.push_str(&format!("  {step}\n"));
        }
        if let Some(error) = &self.error {
            out.push_str(&format!("  refused: {error}\n"));
        }
        out.push_str(&serde_json::to_string(self).expect("an identity-mode report is plain JSON"));
        out.push('\n');
        out
    }
}

/// Convert the anonymous store under `data_dir` to local accounts with one bootstrap
/// administrator. Refuses `IDENTITY_MODE_NOT_CONFIRMED` before opening anything, the lock
/// error when an engine runs, and `IDENTITY_MODE_NOT_ANONYMOUS` unless the marker file is a
/// regular file holding exactly the marker bytes.
pub fn set_local(
    data_dir: &Path,
    admin_username: &str,
    admin_password: Secret,
    confirm: bool,
) -> IdentityModeReport {
    let mut report = IdentityModeReport {
        verb: "set",
        steps: Vec::new(),
        error: None,
        exit_code: 0,
    };
    if let Err(error) = convert(
        data_dir,
        admin_username,
        admin_password,
        confirm,
        &mut report.steps,
    ) {
        report.error = Some(error);
        report.exit_code = 1;
    }
    report
}

fn convert(
    data_dir: &Path,
    admin_username: &str,
    admin_password: Secret,
    confirm: bool,
    steps: &mut Vec<&'static str>,
) -> Result<(), String> {
    if !confirm {
        return Err(format!(
            "{IDENTITY_MODE_NOT_CONFIRMED}: pass --confirm to convert the store to local accounts"
        ));
    }
    let dir = data_dir
        .to_str()
        .ok_or_else(|| "the data directory path is not UTF-8".to_string())?;
    let _lock = medulla_kernel::persist_lock::acquire(dir)?;
    let marker = data_dir.join(ANONYMOUS_MARKER_FILE);
    let anonymous = std::fs::symlink_metadata(&marker)
        .map(|meta| meta.file_type().is_file())
        .unwrap_or(false)
        && std::fs::read(&marker).is_ok_and(|bytes| bytes == ANONYMOUS_MARKER_BYTES);
    if !anonymous {
        return Err(format!(
            "{IDENTITY_MODE_NOT_ANONYMOUS}: {dir} does not hold the anonymous store marker"
        ));
    }
    let authority = medulla_kernel::store_authority::process_authority();
    let mut isolation = IsolationLayer::with_persist_dir(
        data_dir,
        authority.as_ref(),
        authority.principal(),
        &authority.proof(),
    )
    .map_err(|error| format!("could not open the policy store: {error}"))?;

    match isolation
        .rbac()
        .identity_store()
        .config()
        .map(|config| config.mode)
    {
        None => {
            initialize_local(&mut isolation, admin_username, admin_password)?;
            steps.push("initialize_identity_store");
        }
        Some(AuthMode::Local) => {}
        Some(_) => return Err(IdentityRefusal::AlreadyInitialized.to_string()),
    }
    isolation.try_unregister_agent(ANONYMOUS_AGENT_ID)?;
    steps.push("unregister_anonymous_agent");
    std::fs::remove_file(&marker)
        .map_err(|error| format!("could not remove the marker: {error}"))?;
    std::fs::File::open(data_dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("could not sync the data directory: {error}"))?;
    steps.push("remove_marker");
    Ok(())
}

/// Seed the identity store in `Local` mode with the bootstrap administrator, hashing the
/// password through the same stamp the served `Initialize` uses.
fn initialize_local(
    isolation: &mut IsolationLayer,
    admin_username: &str,
    admin_password: Secret,
) -> Result<(), String> {
    let mut request = InitializeRequest {
        mode: AuthMode::Local,
        admin_username: Some(admin_username.to_string()),
        admin_password,
    };
    let mut stamp = IdentityStamp::for_actor(IdentityActor {
        principal_id: OFFLINE_PRINCIPAL.to_string(),
        delegated: false,
        scopes: BTreeSet::from(["identity:admin".to_string()]),
    });
    stamp_initialize(&mut request, &mut stamp, DEFAULT_PASSWORD_MIN_CHARS)
        .map_err(|refusal| refusal.to_string())?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64);
    isolation
        .try_apply_identity(
            &IdentityOp::Config(ConfigOp::Initialize { request }),
            &stamp,
            now_ms,
            &medulla_capabilities::scopes::ScopeRegistry,
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
}
