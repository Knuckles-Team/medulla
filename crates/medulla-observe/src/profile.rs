//! On-demand profiling behind admin auth (MEDULLA-OBSERVE-R010).
//!
//! [`stacks`] samples the stack of every thread of the process for a bounded time and returns
//! folded stacks, each frame naming its module, the module's build id and the offset inside it, so
//! a stack can be symbolized offline (`scripts/symbolize.sh`). [`task_dump`] returns the tasks,
//! requests, locks and the last events in one snapshot under one sequence number.
//!
//! The sampler is signal based and needs a Linux target with glibc: a sampler thread sends
//! `SIGPROF` to every other thread of the process with `tgkill`, and the handler of each thread
//! captures its own stack into a preallocated slot with `backtrace` and touches nothing else. At
//! most one profile runs at a time. On any other target, and without the `profiler` feature,
//! [`stacks`] refuses `PROFILE_UNAVAILABLE`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use medulla_types::contract::ServerErrorCode;
use medulla_types::introspect::{ProfileKind, ProfileReport};
use serde_json::{json, Value};

/// The most seconds one profile call may sample (`observe.profile_max_seconds`, a fixed knob).
pub const MAX_PROFILE_SECONDS: u16 = 60;
/// The events a `task_dump` carries.
pub const TASK_DUMP_EVENTS: usize = 256;

/// Why a profile call was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileError {
    /// A profile is already running.
    Busy,
    /// `seconds` is 0 or over the maximum.
    DurationInvalid,
    /// The platform or the build has no sampler.
    Unavailable,
}

impl ProfileError {
    /// The registered refusal code of this error.
    pub fn code(self) -> ServerErrorCode {
        match self {
            Self::Busy => ServerErrorCode::ProfileBusy,
            Self::DurationInvalid => ServerErrorCode::ProfileDurationInvalid,
            Self::Unavailable => ServerErrorCode::ProfileUnavailable,
        }
    }
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let detail = match self {
            Self::Busy => "a profile is already running".to_string(),
            Self::DurationInvalid => {
                format!("seconds must be 1 to {MAX_PROFILE_SECONDS}")
            }
            Self::Unavailable => "this build has no stack sampler for this platform".to_string(),
        };
        write!(f, "{}: {detail}", self.code().as_str())
    }
}

impl std::error::Error for ProfileError {}

/// Whether this build can sample stacks.
pub const fn available() -> bool {
    cfg!(all(
        feature = "profiler",
        target_os = "linux",
        target_env = "gnu"
    ))
}

static RUNNING: AtomicBool = AtomicBool::new(false);
static DUMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Holds the one profile slot; releases it when dropped.
struct Running;

impl Running {
    fn acquire() -> Result<Self, ProfileError> {
        RUNNING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| ProfileError::Busy)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::Release);
    }
}

/// Check `seconds` against the bounds of a `stacks` profile.
pub fn check_seconds(seconds: u16) -> Result<(), ProfileError> {
    if (1..=MAX_PROFILE_SECONDS).contains(&seconds) {
        Ok(())
    } else {
        Err(ProfileError::DurationInvalid)
    }
}

/// Sample every thread at `hz` for `seconds` and fold the stacks. Blocks the calling thread for
/// the duration, so a caller on an async runtime runs it on a blocking thread. Refuses
/// `PROFILE_DURATION_INVALID`, `PROFILE_BUSY` when a profile is running, and
/// `PROFILE_UNAVAILABLE` when this build has no sampler.
pub fn stacks(seconds: u16, hz: u16) -> Result<ProfileReport, ProfileError> {
    check_seconds(seconds)?;
    if !available() {
        return Err(ProfileError::Unavailable);
    }
    let _running = Running::acquire()?;
    let hz = hz.max(1);
    let ticks = u32::from(seconds) * u32::from(hz);
    let pause = std::time::Duration::from_micros(1_000_000 / u64::from(hz));
    sample(seconds, hz, ticks, || std::thread::sleep(pause))
}

/// Sample for `ticks` ticks and call `pause` between ticks. [`stacks`] pauses one tick of
/// wall-clock time; a test passes a pause that does nothing, so it never waits on a clock.
#[doc(hidden)]
pub fn sample_ticks(ticks: u32, pause: impl FnMut()) -> Result<ProfileReport, ProfileError> {
    if !available() {
        return Err(ProfileError::Unavailable);
    }
    let _running = Running::acquire()?;
    sample(0, 0, ticks, pause)
}

#[cfg(all(feature = "profiler", target_os = "linux", target_env = "gnu"))]
fn sample(
    seconds: u16,
    hz: u16,
    ticks: u32,
    pause: impl FnMut(),
) -> Result<ProfileReport, ProfileError> {
    let (samples, folded) = sampler::run(ticks, pause);
    Ok(ProfileReport {
        kind: ProfileKind::Stacks,
        seq: 0,
        seconds,
        hz,
        samples,
        stacks: folded,
        tasks: Vec::new(),
        requests: Vec::new(),
        locks: Vec::new(),
        events: Vec::new(),
    })
}

#[cfg(not(all(feature = "profiler", target_os = "linux", target_env = "gnu")))]
fn sample(
    _seconds: u16,
    _hz: u16,
    _ticks: u32,
    _pause: impl FnMut(),
) -> Result<ProfileReport, ProfileError> {
    Err(ProfileError::Unavailable)
}

/// One atomic snapshot: the tasks, the in-flight requests, the lock sites and the last
/// [`TASK_DUMP_EVENTS`] events, each registry read once, under one new sequence number.
pub fn task_dump() -> ProfileReport {
    let seq = DUMP_SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1;
    let tasks: Vec<Value> = crate::global().all().iter().map(task_row).collect();
    let requests: Vec<Value> = crate::requests()
        .snapshot()
        .into_iter()
        .map(|row| {
            json!({
                "key": row.key,
                "request_id": row.request_id,
                "trace_id": row.trace_id,
                "method": row.method,
                "graph": row.graph,
                "principal_ref": row.principal_ref,
                "phase": medulla_types::transition_refused::MachineState::stored(row.phase),
                "age_ms": row.age_ms,
                "waiting_for": row.waiting_for,
            })
        })
        .collect();
    let locks: Vec<Value> = crate::locks()
        .snapshot()
        .into_iter()
        .map(|row| {
            json!({
                "name": row.name,
                "holder": row.holder,
                "hold_ms": row.hold_ms,
                "waiters": row.waiters,
                "longest_wait_ms": row.longest_wait_ms,
            })
        })
        .collect();
    let events: Vec<Value> = crate::events()
        .last(TASK_DUMP_EVENTS)
        .into_iter()
        .map(|(key, event)| {
            json!({
                "seq": key,
                "machine": event.machine,
                "subject": event.subject,
                "from": event.from,
                "event": event.event,
                "to": event.to,
                "at_unix_ms": event.at_unix_ms,
                "trace_id": event.trace_id,
            })
        })
        .collect();
    ProfileReport {
        kind: ProfileKind::TaskDump,
        seq,
        seconds: 0,
        hz: 0,
        samples: 0,
        stacks: Vec::new(),
        tasks,
        requests,
        locks,
        events,
    }
}

fn task_row(row: &crate::registry::tasks::TaskRow) -> Value {
    json!({
        "id": row.id,
        "name": row.name,
        "owner": row.owner,
        "kind": format!("{:?}", row.kind),
        "state": medulla_types::transition_refused::MachineState::stored(row.state),
        "units_done": row.units_done,
        "last_progress_unix_ms": row.last_progress_unix_ms,
        "last_progress_location": row.last_progress_location.map(crate::registry::tasks::file_line),
        "registered_location": crate::registry::tasks::file_line(row.registered_location),
        "origin_request": row.origin_request,
    })
}

#[cfg(all(feature = "profiler", target_os = "linux", target_env = "gnu"))]
#[allow(unsafe_code)]
mod sampler {
    //! The signal sampler. Every `unsafe` block states why it is sound.

    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

    use medulla_types::introspect::{FoldedStack, ProfileFrame};

    /// Frames kept per sample.
    const MAX_FRAMES: usize = 48;
    /// Slots of the capture ring. One tick takes one sample per thread, and the ring is drained
    /// after every tick, so it only has to hold the threads of one tick.
    const SLOTS: usize = 4096;
    /// The frames above the interrupted code: the handler itself and the signal trampoline.
    const HANDLER_FRAMES: usize = 2;
    /// The longest a tick waits for the handlers of the threads it signalled.
    const TICK_DEADLINE: std::time::Duration = std::time::Duration::from_millis(200);
    /// The most bytes of a module read to find its build id.
    const BUILD_ID_PREFIX_BYTES: u64 = 1024 * 1024;

    struct Slot {
        tid: AtomicU32,
        len: AtomicUsize,
        frames: [AtomicUsize; MAX_FRAMES],
    }

    impl Slot {
        const fn new() -> Self {
            Self {
                tid: AtomicU32::new(0),
                len: AtomicUsize::new(0),
                frames: [const { AtomicUsize::new(0) }; MAX_FRAMES],
            }
        }
    }

    static RING: [Slot; SLOTS] = [const { Slot::new() }; SLOTS];
    /// The next slot to fill. Only the handlers advance it.
    static HEAD: AtomicUsize = AtomicUsize::new(0);
    /// Signals sent and handled since the sampler started.
    static SENT: AtomicU64 = AtomicU64::new(0);
    static HANDLED: AtomicU64 = AtomicU64::new(0);

    /// The `SIGPROF` handler: it captures this thread's stack into one slot and nothing else.
    /// It allocates nothing and takes no lock; `backtrace` was run once before the handler was
    /// installed, so the unwinder is already loaded.
    extern "C" fn on_sigprof(_signal: libc::c_int) {
        // SAFETY: reading and restoring errno through its thread-local location is how a
        // signal handler keeps the interrupted code's errno intact.
        let saved_errno = unsafe { *libc::__errno_location() };
        let index = HEAD.fetch_add(1, Ordering::Relaxed) % SLOTS;
        let slot = &RING[index];
        // SAFETY: `gettid` is async-signal-safe and takes no argument.
        let tid = unsafe { libc::syscall(libc::SYS_gettid) } as u32;
        // SAFETY: `frames` holds MAX_FRAMES words, the size passed; `AtomicUsize` has the layout
        // of `usize`, which is that of a pointer on every supported target. `backtrace` writes
        // only that buffer.
        let captured = unsafe {
            libc::backtrace(
                slot.frames.as_ptr() as *mut *mut libc::c_void,
                MAX_FRAMES as libc::c_int,
            )
        };
        slot.tid.store(tid, Ordering::Relaxed);
        slot.len.store(captured.max(0) as usize, Ordering::Relaxed);
        HANDLED.fetch_add(1, Ordering::Release);
        // SAFETY: restores the value read above.
        unsafe { *libc::__errno_location() = saved_errno };
    }

    /// The thread ids of the process.
    fn thread_ids() -> Vec<i32> {
        std::fs::read_dir("/proc/self/task")
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn thread_name(tid: u32) -> String {
        std::fs::read_to_string(format!("/proc/self/task/{tid}/comm"))
            .map(|name| name.trim().to_string())
            .unwrap_or_else(|_| format!("thread-{tid}"))
    }

    /// Restores the previous `SIGPROF` disposition when dropped.
    struct Installed {
        previous: libc::sigaction,
    }

    impl Installed {
        fn new() -> Option<Self> {
            // Load the unwinder before the first signal: its first call may allocate.
            let mut warm = [std::ptr::null_mut::<libc::c_void>(); 8];
            // SAFETY: the buffer holds 8 pointers, the size passed.
            unsafe { libc::backtrace(warm.as_mut_ptr(), warm.len() as libc::c_int) };
            // SAFETY: an all-zero `sigaction` is a valid starting value on Linux; the fields the
            // call reads are set below, and `previous` receives the old disposition.
            let (mut action, mut previous): (libc::sigaction, libc::sigaction) =
                unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
            action.sa_sigaction = on_sigprof as usize;
            action.sa_flags = libc::SA_RESTART;
            // SAFETY: `sa_mask` is a valid, owned signal set.
            unsafe { libc::sigemptyset(&mut action.sa_mask) };
            // SAFETY: both pointers are to live, owned values.
            let status = unsafe { libc::sigaction(libc::SIGPROF, &action, &mut previous) };
            (status == 0).then_some(Self { previous })
        }
    }

    impl Drop for Installed {
        fn drop(&mut self) {
            // SAFETY: restores the disposition `new` replaced.
            unsafe { libc::sigaction(libc::SIGPROF, &self.previous, std::ptr::null_mut()) };
        }
    }

    /// Run the sampler for `ticks` ticks and return the sample count and the folded stacks,
    /// hottest first.
    pub(super) fn run(ticks: u32, mut pause: impl FnMut()) -> (u64, Vec<FoldedStack>) {
        let Some(_installed) = Installed::new() else {
            return (0, Vec::new());
        };
        // SAFETY: `getpid` and `gettid` take no argument and cannot fail.
        let pid = unsafe { libc::getpid() };
        let own = unsafe { libc::syscall(libc::SYS_gettid) } as i32;
        let mut counts: HashMap<(u32, Vec<usize>), u64> = HashMap::new();
        let mut samples = 0_u64;
        for _ in 0..ticks {
            let before = HEAD.load(Ordering::Relaxed);
            let handled_before = HANDLED.load(Ordering::Acquire);
            let mut sent = 0_u64;
            for tid in thread_ids() {
                if tid == own {
                    continue;
                }
                // SAFETY: `tgkill` takes three integers; a thread that exited since it was
                // listed makes the call fail, which is not counted as sent.
                let status = unsafe { libc::syscall(libc::SYS_tgkill, pid, tid, libc::SIGPROF) };
                if status == 0 {
                    sent += 1;
                }
            }
            SENT.fetch_add(sent, Ordering::Relaxed);
            let deadline = std::time::Instant::now() + TICK_DEADLINE;
            while HANDLED.load(Ordering::Acquire) - handled_before < sent
                && std::time::Instant::now() < deadline
            {
                std::thread::yield_now();
            }
            let taken = (HEAD.load(Ordering::Relaxed) - before).min(SLOTS);
            for offset in 0..taken {
                let slot = &RING[(before + offset) % SLOTS];
                let len = slot.len.load(Ordering::Relaxed).min(MAX_FRAMES);
                if len <= HANDLER_FRAMES {
                    continue;
                }
                let frames: Vec<usize> = slot.frames[HANDLER_FRAMES..len]
                    .iter()
                    .map(|frame| frame.load(Ordering::Relaxed))
                    .collect();
                *counts
                    .entry((slot.tid.load(Ordering::Relaxed), frames))
                    .or_insert(0) += 1;
                samples += 1;
            }
            pause();
        }
        drop(_installed);
        (samples, fold(counts))
    }

    /// Fold the raw counts into resolved stacks, hottest first.
    fn fold(counts: HashMap<(u32, Vec<usize>), u64>) -> Vec<FoldedStack> {
        let mut resolver = Resolver::default();
        let mut folded: Vec<FoldedStack> = counts
            .into_iter()
            .map(|((tid, addresses), count)| FoldedStack {
                thread: thread_name(tid),
                // `backtrace` lists the innermost frame first; a folded stack runs outermost first.
                frames: addresses
                    .iter()
                    .rev()
                    .map(|address| resolver.frame(*address))
                    .collect(),
                count,
            })
            .collect();
        folded.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.thread.cmp(&b.thread)));
        folded
    }

    /// Resolves an address to its module, build id, offset and symbol, one lookup per address and
    /// one build-id read per module.
    #[derive(Default)]
    struct Resolver {
        frames: HashMap<usize, ProfileFrame>,
        build_ids: HashMap<String, Option<String>>,
    }

    impl Resolver {
        fn frame(&mut self, address: usize) -> ProfileFrame {
            if let Some(frame) = self.frames.get(&address) {
                return frame.clone();
            }
            let frame = self.resolve(address);
            self.frames.insert(address, frame.clone());
            frame
        }

        fn resolve(&mut self, address: usize) -> ProfileFrame {
            // SAFETY: an all-zero `Dl_info` is valid; `dladdr` fills it when it returns non-zero.
            let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
            // SAFETY: `info` is a live, owned value and `address` is only looked up, never read.
            let found = unsafe { libc::dladdr(address as *const libc::c_void, &mut info) } != 0;
            if !found || info.dli_fname.is_null() {
                return ProfileFrame {
                    module: "unknown".to_string(),
                    build_id: None,
                    offset: address as u64,
                    function: None,
                    file_line: None,
                };
            }
            // SAFETY: `dladdr` returned non-null NUL-terminated strings that live as long as the
            // module stays loaded; they are copied before any further call.
            let module = unsafe { std::ffi::CStr::from_ptr(info.dli_fname) }
                .to_string_lossy()
                .into_owned();
            let function = if info.dli_sname.is_null() {
                None
            } else {
                // SAFETY: as above.
                Some(
                    unsafe { std::ffi::CStr::from_ptr(info.dli_sname) }
                        .to_string_lossy()
                        .into_owned(),
                )
            };
            let build_id = self
                .build_ids
                .entry(module.clone())
                .or_insert_with(|| module_build_id(&module))
                .clone();
            ProfileFrame {
                module,
                build_id,
                offset: (address as u64).saturating_sub(info.dli_fbase as u64),
                function,
                file_line: None,
            }
        }
    }

    /// The GNU build id of the module file at `path`, read from its first
    /// [`BUILD_ID_PREFIX_BYTES`]: the note segment of a linked module sits in its first pages.
    fn module_build_id(path: &str) -> Option<String> {
        use std::io::Read;
        let mut prefix = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .take(BUILD_ID_PREFIX_BYTES)
            .read_to_end(&mut prefix)
            .ok()?;
        medulla_version::build_info::elf_build_id(&prefix)
    }
}
