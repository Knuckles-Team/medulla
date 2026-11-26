//! Runtime observability for the engine (MEDULLA-OBSERVE).
//!
//! The crate holds the process-wide task registry, the explicit task state machine, the
//! `spawn` helper that registers a background job, and the progress guard that fails a loop
//! whose cursor stops moving. It depends on `medulla-types` only: the kernel, persistence and
//! server crates depend on it and it imports no handler.
//!
//! A job takes one of two paths. `spawn(spec, future)` registers the job, runs the future and
//! finishes the handle with the future's outcome; `TaskRegistry::register` returns the handle
//! for a job the caller runs itself. Both report through `progress` and end through `finish`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod events;
pub mod machine;
pub mod progress;
pub mod registry;
pub mod runtime_stats;
pub mod settings;
pub mod spawn;
pub mod startup;
pub mod watchdog;

use std::sync::OnceLock;

use registry::readiness::ReadinessRegistry;
use registry::tasks::TaskRegistry;

/// The process-wide task registry. Tests build isolated instances with [`TaskRegistry::new`].
pub fn global() -> &'static TaskRegistry {
    static GLOBAL: OnceLock<TaskRegistry> = OnceLock::new();
    GLOBAL.get_or_init(|| TaskRegistry::new(registry::tasks::DEFAULT_TASK_REGISTRY_CAPACITY))
}

/// The process-wide transition event ring (MEDULLA-OBSERVE-R004.4). Every machine's apply
/// wrapper appends to it.
pub fn events() -> &'static events::EventRing {
    static GLOBAL: OnceLock<events::EventRing> = OnceLock::new();
    GLOBAL.get_or_init(|| events::EventRing::new(events::DEFAULT_EVENT_RING_CAPACITY))
}

/// The process-wide readiness registry (MEDULLA-OBSERVE-R007.1). Tests build isolated
/// instances with [`ReadinessRegistry::new`].
pub fn readiness() -> &'static ReadinessRegistry {
    static GLOBAL: OnceLock<ReadinessRegistry> = OnceLock::new();
    GLOBAL.get_or_init(ReadinessRegistry::new)
}

/// The process-wide in-flight request registry (MEDULLA-OBSERVE-R006.2). Tests build isolated
/// instances with [`registry::requests::RequestRegistry::new`].
pub fn requests() -> &'static registry::requests::RequestRegistry {
    static GLOBAL: OnceLock<registry::requests::RequestRegistry> = OnceLock::new();
    GLOBAL.get_or_init(registry::requests::RequestRegistry::default)
}

/// The process-wide lock-site registry (MEDULLA-OBSERVE-R006.4). Tests build isolated
/// instances with [`registry::locks::LockRegistry::new`].
pub fn locks() -> &'static registry::locks::LockRegistry {
    static GLOBAL: OnceLock<registry::locks::LockRegistry> = OnceLock::new();
    GLOBAL.get_or_init(registry::locks::LockRegistry::new)
}

/// The process-wide runtime sampler (MEDULLA-OBSERVE-R006.3).
pub fn runtime_sampler() -> &'static runtime_stats::RuntimeSampler {
    static GLOBAL: OnceLock<runtime_stats::RuntimeSampler> = OnceLock::new();
    GLOBAL.get_or_init(runtime_stats::RuntimeSampler::new)
}

/// The process-wide startup report (MEDULLA-OBSERVE-R008.1). Tests build isolated
/// instances with [`startup::StartupReport::new`].
pub fn startup() -> &'static startup::StartupReport {
    static GLOBAL: OnceLock<startup::StartupReport> = OnceLock::new();
    GLOBAL.get_or_init(startup::StartupReport::new)
}

/// Jobs the process-wide registry could not list since start: the live table was full or the
/// name was held by a live job. The job still ran. Exported as `medulla_observe_dropped_total`.
pub fn dropped_total() -> u64 {
    global().dropped_total()
}
