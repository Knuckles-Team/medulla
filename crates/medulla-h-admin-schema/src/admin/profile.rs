//! The served on-demand profile (MEDULLA-OBSERVE-R010.1 and R010.3).
//!
//! `Method::Profile` either samples the stack of every thread of the process (`stacks`) or reads
//! one atomic snapshot of the runtime registries (`task_dump`). Both read the engine itself: no
//! durable substrate and no graph content, so the call serves in every build. The scope is
//! `observe:profile` and the call is audited by its capability row.

use medulla_observe::profile::{self, ProfileError};
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_types::introspect::ProfileKind;

/// Answer one `Method::Profile` request.
///
/// Refusals: `PROFILE_DURATION_INVALID` for `seconds` outside 1 to 60, `PROFILE_BUSY` while
/// another profile runs, and `PROFILE_UNAVAILABLE` on a build without a sampler.
pub async fn handle(req_id: u64, method: Method) -> Response {
    let Method::Profile { kind, seconds } = method else {
        return Response::err(
            req_id,
            "INTERNAL: profile handler received a foreign method",
        );
    };
    let report = match kind {
        ProfileKind::TaskDump => Ok(profile::task_dump()),
        ProfileKind::Stacks => {
            let hz = medulla_observe::settings::profile_hz().min(u64::from(u16::MAX)) as u16;
            // The sampler blocks for the whole duration, so it runs off the async workers.
            tokio::task::spawn_blocking(move || profile::stacks(seconds, hz))
                .await
                .unwrap_or_else(|error| {
                    tracing::error!(%error, "the profile sampler task failed");
                    Err(ProfileError::Unavailable)
                })
        }
    };
    match report {
        Ok(report) => Response::ok(
            req_id,
            ResultPayload::of::<medulla_protocol_types::result_contract::security::Profile>(report),
        ),
        Err(error) => Response::err(req_id, error.to_string()),
    }
}
