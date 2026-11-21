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

pub mod machine;
