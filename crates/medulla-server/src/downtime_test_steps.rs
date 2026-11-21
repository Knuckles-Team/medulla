//! The body of the `test_downtime_steps` downtime intent, installed only in a build with
//! the `test-support` feature (MEDULLA-DURABLE-KERNEL-R101.6, R101.7).
//!
//! The served scheduler proof needs a downtime job that takes real time, so that foreground
//! load can arrive while it runs. Each step takes [`STEP_MS`] and checkpoints the count of
//! steps done; the job completes after [`TOTAL_STEPS`] steps with `{ "steps": <count> }`.
//! A step that resumes from a checkpoint continues the count, so a preempted job loses no
//! progress.

use medulla_server_api::downtime::{DowntimeBody, DowntimeStep, DowntimeStepInput};

/// The intent the body serves.
pub const INTENT: &str = "test_downtime_steps";
/// The length of one step, in milliseconds.
pub const STEP_MS: u64 = 100;
/// How many steps the job takes.
pub const TOTAL_STEPS: u8 = 20;

/// The `test_downtime_steps` body.
pub struct TestStepsBody;

impl DowntimeBody for TestStepsBody {
    fn step(&self, input: DowntimeStepInput<'_>) -> Result<DowntimeStep, String> {
        std::thread::sleep(std::time::Duration::from_millis(STEP_MS));
        let before = input
            .state_blob
            .and_then(|blob| blob.first())
            .copied()
            .unwrap_or(0);
        let after = before.saturating_add(1);
        if after >= TOTAL_STEPS {
            let result = medulla_h_jobs::jobs::downtime_output_result(
                input.job,
                &serde_json::json!({ "steps": after }),
            )?;
            Ok(DowntimeStep::Done {
                result,
                working_bytes: 1_024,
            })
        } else {
            Ok(DowntimeStep::Continue {
                state_blob: vec![after],
                working_bytes: 1_024,
            })
        }
    }
}
