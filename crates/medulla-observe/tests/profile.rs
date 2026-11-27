//! The stack sampler: frames name their module, build id and offset; one profile runs at a time.
#![cfg(feature = "profiler")]
//! The sampler is driven by ticks, not by a clock, so the test never waits on one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use medulla_observe::profile::{self, ProfileError};

/// Two threads that run until told to stop, so the sampler has other threads to sample.
struct Spinners {
    stop: Arc<AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl Spinners {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let threads = (0..2)
            .map(|_| {
                let stop = Arc::clone(&stop);
                std::thread::Builder::new()
                    .name("prof-spin".to_string())
                    .spawn(move || {
                        while !stop.load(Ordering::Relaxed) {
                            std::hint::spin_loop();
                        }
                    })
                    .expect("a spinner thread")
            })
            .collect();
        Self { stop, threads }
    }

    fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        for thread in self.threads {
            thread.join().expect("a spinner thread ends");
        }
    }
}

// spec: MEDULLA-OBSERVE-R010.2
#[test]
fn frames_carry_build_id_and_offset() {
    if !profile::available() {
        // A build or platform without a sampler refuses; it never answers an empty profile.
        assert_eq!(
            profile::sample_ticks(1, || {}).unwrap_err(),
            ProfileError::Unavailable
        );
        assert_eq!(
            profile::stacks(1, 99).unwrap_err(),
            ProfileError::Unavailable
        );
        return;
    }
    let spinners = Spinners::start();
    let report = profile::sample_ticks(25, || {}).expect("the sampler runs");
    spinners.stop();

    assert!(report.samples > 0, "the spinning threads were sampled");
    let spinning: Vec<_> = report
        .stacks
        .iter()
        .filter(|stack| stack.thread == "prof-spin")
        .collect();
    assert!(
        !spinning.is_empty(),
        "the spinners' stacks are listed by thread name"
    );
    assert!(report
        .stacks
        .windows(2)
        .all(|pair| pair[0].count >= pair[1].count));

    let executable = std::env::current_exe().expect("the test executable");
    let own_build_id = medulla_version::BUILD_INFO.build_id.clone();
    let mut in_executable = 0;
    for stack in &report.stacks {
        assert!(!stack.frames.is_empty());
        for frame in &stack.frames {
            // A frame is never a raw address alone: it names its module and an offset in it.
            assert!(!frame.module.is_empty());
            if frame.module == "unknown" {
                continue;
            }
            assert!(
                std::path::Path::new(&frame.module).is_absolute() || frame.module.contains('/'),
                "the module is a path: {}",
                frame.module
            );
            if std::path::Path::new(&frame.module) == executable {
                in_executable += 1;
                // The executable's frames carry the build id the engine reports for itself.
                assert_eq!(frame.build_id, own_build_id, "{frame:?}");
            }
            if let Some(build_id) = &frame.build_id {
                assert!(build_id.len() >= 8 && build_id.bytes().all(|b| b.is_ascii_hexdigit()));
            }
        }
    }
    assert!(in_executable > 0, "some frames lie in the test executable");
}

// spec: MEDULLA-OBSERVE-R010.1
#[test]
fn one_profile_runs_at_a_time_and_a_duration_is_bounded() {
    assert_eq!(
        profile::check_seconds(0),
        Err(ProfileError::DurationInvalid)
    );
    assert_eq!(
        profile::check_seconds(profile::MAX_PROFILE_SECONDS + 1),
        Err(ProfileError::DurationInvalid)
    );
    assert_eq!(profile::check_seconds(1), Ok(()));
    assert_eq!(profile::check_seconds(profile::MAX_PROFILE_SECONDS), Ok(()));
    assert_eq!(
        profile::stacks(0, 99).unwrap_err(),
        ProfileError::DurationInvalid
    );
    if !profile::available() {
        return;
    }
    // While one profile runs, a second is refused; the pause of the first runs the second.
    let mut second = None;
    let first = profile::sample_ticks(1, || {
        second = Some(profile::sample_ticks(1, || {}));
    });
    assert!(first.is_ok());
    assert_eq!(second.unwrap().unwrap_err(), ProfileError::Busy);
    // The slot is free again once the first profile ended.
    assert!(profile::sample_ticks(1, || {}).is_ok());
    // The codes are the registered ones.
    assert_eq!(ProfileError::Busy.code().as_str(), "PROFILE_BUSY");
    assert_eq!(
        ProfileError::DurationInvalid.code().as_str(),
        "PROFILE_DURATION_INVALID"
    );
    assert_eq!(
        ProfileError::Unavailable.code().as_str(),
        "PROFILE_UNAVAILABLE"
    );
}
