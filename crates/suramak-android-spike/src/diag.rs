//! Crash and event diagnostics — the part that makes the spike survivable.
//!
//! The M13b spike shipped blind: a panic on the `android_main` thread is caught
//! by `android-activity`, which then calls `GameActivity_finish()` — the
//! Activity closes instantly and the panic text goes to logcat, which the owner
//! has no way to read (`android-activity-0.6.1/src/game_activity/mod.rs`,
//! `_rust_glue_entry`: `catch_unwind(|| android_main(app)).unwrap_or_else(log_panic)`
//! followed by `GameActivity_finish`). That is exactly the "opens then
//! immediately closes" symptom.
//!
//! This module gives the spike a memory that outlives the process:
//!
//! * a **panic hook** that records message, location, thread and timestamp, the
//!   last [`RING_CAPACITY`] platform events (the run-up to the failure) and
//!   writes them to two files in the directory Kotlin hands over — the app's
//!   external files dir since v2.1 (`Android/data/app.suramak.spike/files`), so
//!   the phone's own file manager and the banner's **Share log** button can
//!   reach them;
//! * a **live crash buffer** so a panic contained by `catch_unwind` is visible
//!   on screen in the same session, not only after a relaunch;
//! * an **event ring** of the last [`RING_CAPACITY`] timestamped lines, fed by
//!   the log bridge here and by the JNI exports in [`crate::platform`];
//! * `clear`, which the on-screen **Clear crash** button calls — nothing else
//!   deletes the record.
//!
//! **v2.4: the native signal.** Everything above is in-process, so it dies with
//! the process — and the owner's v2.3 run produced exactly the failure that
//! leaves nothing: a rotation within the first second of launch killed the
//! process natively (SIGSEGV/SIGABRT class), no Rust panic, no Java exception,
//! never reached `onStop`, and the ring (which only lived in memory) went with
//! it. The next launch could only *infer* it from the session marker.
//!
//! So a POSIX handler for `SIGSEGV`/`SIGABRT`/`SIGBUS` is installed as early as
//! the library allows ([`init_signal_handlers`], called from `android_main` and
//! from `nativeInit`), and it writes **the same ring, plus the phase the app
//! was in**, to `spike-last-crash.txt` before re-raising the signal so Android
//! still writes its tombstone. The constraints are the usual ones and they
//! shape the code below: nothing in the handler allocates, formats, locks or
//! calls the logger — it does `open`/`write`/`fsync`/`close` on a pre-rendered
//! byte buffer, a static per-signal line, then `signal(SIG_DFL)` + `raise`.
//!
//! The pre-rendered buffer ([`SignalSnapshot`]) is refreshed by the *normal*
//! thread on every event and on every [`set_phase`], which is what makes the
//! write a single `memcpy`-shaped operation the handler can trust. A crash
//! inside the refresh itself can leave a mixed snapshot; that is still better
//! than an empty file, and it is named in `docs/reports/M13b6-spike-v2.4.md`.
//!
//! Nothing in here allocates in a panic path beyond the message itself, takes a
//! lock twice, or calls the logger from the hook (a panic inside the logger's
//! lock would abort). File I/O errors are ignored on purpose: a diagnostic that
//! panics while reporting a panic is worse than a missing file.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// How many platform events the on-screen panel and the crash file keep.
pub const RING_CAPACITY: usize = 40;

/// Longest single ring line, in characters. One line on a phone screen is
/// ~55 monospace glyphs; the panel scrolls horizontally, the file keeps more.
const MAX_LINE_CHARS: usize = 400;

/// Crash files, *relative to the app's private files dir*. `MainActivity.kt`
/// holds the same two names — keep them in sync.
pub const LAST_CRASH_FILE: &str = "spike-last-crash.txt";
pub const CRASH_LOG_FILE: &str = "spike-crash-log.txt";

/// Never read back more than this from the persisted crash file.
const MAX_CRASH_READ: u64 = 32 * 1024;
/// Roll the append-only history over at this size, keeping the newest part.
const MAX_HISTORY_BYTES: u64 = 256 * 1024;
const KEEP_HISTORY_BYTES: u64 = 48 * 1024;

/// Longest snapshot the signal handler can write: the 40-line ring at its
/// per-line ceiling plus the header, with room to spare. A `static` byte array,
/// because the handler may not allocate and a stack buffer is exactly what a
/// stack-related fault cannot afford.
const SIGNAL_SNAPSHOT_CAP: usize = 16 * 1024;
/// The snapshot line that opens the file and identifies what wrote it.
const SIGNAL_HEADER: &[u8] = b"=== Suramak spike NATIVE SIGNAL ===\n";
/// Signals the handler covers. The field's failure class is
/// `SIGSEGV`/`SIGABRT`; `SIGBUS` is the third native-kill shape ARM produces
/// (misaligned/absent device memory). `SIGILL`/`SIGFPE` are left to the
/// tombstone on purpose — see `docs/reports/M13b6-spike-v2.4.md` §9.
const SIGABRT: i32 = 6;
const SIGBUS: i32 = 7;
const SIGSEGV: i32 = 11;
/// `signal(2)`'s "restore the default action" value.
const SIG_DFL: usize = 0;
/// `open(2)` flags, from the Linux generic `fcntl.h` (same values on Android
/// and on the host this is tested on). `O_CLOEXEC` keeps the fd out of any
/// child; `O_APPEND` is used for the history file only.
const O_WRONLY: i32 = 1;
const O_CREAT: i32 = 0o100;
const O_TRUNC: i32 = 0o1000;
const O_APPEND: i32 = 0o2000;

/// What the app was doing when a native signal arrives.
///
/// This is the "in-flight event" half of the dump: the ring says what already
/// happened, the phase says what was happening *now*. The variants are the
/// stages of the exact path the v2.3 kill went through — launch → event loop →
/// gpu init (surface, adapter, device, configure) → frame — plus the two
/// lifecycle states, so a rotation kill names the stage it died in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    /// Library loaded, `android_main` not entered yet.
    Boot = 0,
    /// Building/entering the winit event loop.
    EventLoop = 1,
    /// `create_surface` — the window handle's first use.
    GpuSurface = 2,
    /// `request_adapter`.
    GpuAdapter = 3,
    /// `request_device`.
    GpuDevice = 4,
    /// `surface.configure` + renderer/egui construction.
    GpuConfigure = 5,
    /// Drawing/presenting a frame.
    Frame = 6,
    /// Suspended: no window, nothing may be built or drawn.
    Suspended = 7,
    /// The Activity is being destroyed (this generation is over).
    Destroying = 8,
    /// Between events.
    Idle = 9,
}

impl Phase {
    pub const ALL: [Phase; 10] = [
        Phase::Boot,
        Phase::EventLoop,
        Phase::GpuSurface,
        Phase::GpuAdapter,
        Phase::GpuDevice,
        Phase::GpuConfigure,
        Phase::Frame,
        Phase::Suspended,
        Phase::Destroying,
        Phase::Idle,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Boot => "boot",
            Phase::EventLoop => "event loop",
            Phase::GpuSurface => "gpu init: surface",
            Phase::GpuAdapter => "gpu init: adapter",
            Phase::GpuDevice => "gpu init: device",
            Phase::GpuConfigure => "gpu init: configure",
            Phase::Frame => "frame",
            Phase::Suspended => "suspended",
            Phase::Destroying => "destroying (activity)",
            Phase::Idle => "idle",
        }
    }

    fn from_u8(value: u8) -> Phase {
        Phase::ALL
            .get(value as usize)
            .copied()
            .unwrap_or(Phase::Idle)
    }
}

/// Severity of one event line. Ordered so `>=` reads naturally.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn tag(self) -> &'static str {
        match self {
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "ERR ",
        }
    }
}

/// One timestamped event.
#[derive(Clone, Debug)]
pub struct LogLine {
    /// Seconds since this process loaded the spike library.
    pub at: f64,
    pub level: Level,
    pub text: String,
}

/// A crash the panic hook recorded in *this* process.
#[derive(Clone, Debug)]
pub struct CrashRecord {
    /// 1-based, per process.
    pub seq: u64,
    /// The whole block, byte-for-byte what is in `spike-last-crash.txt`.
    pub text: String,
    /// Where a `catch_unwind` caught it (empty while still unwinding).
    pub contained_at: Option<String>,
}

/// What the UI shows: either this session's crash or the previous session's.
#[derive(Clone, Debug)]
pub struct CrashView {
    pub text: String,
    /// `true` = recorded by this process; `false` = read from the file at start.
    pub live: bool,
}

struct Diag {
    started: Instant,
    ring: VecDeque<LogLine>,
    dropped: u64,
    /// Bumped whenever the ring changes, so the UI can cache its copy instead
    /// of cloning 40 strings every frame.
    ring_rev: u64,
    crashes: u64,
    live_crash: Option<CrashRecord>,
    loaded_crash: Option<String>,
    /// Bumped whenever the crash record changes (recorded, contained, loaded,
    /// cleared).
    crash_rev: u64,
}

static DIAG: OnceLock<Mutex<Diag>> = OnceLock::new();
/// Kept outside [`Diag`] so the panic hook can still reach it when the diag lock
/// is the thing being poisoned (see `record_panic`).
static FILES_DIR: OnceLock<PathBuf> = OnceLock::new();
static HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);

fn diag() -> &'static Mutex<Diag> {
    DIAG.get_or_init(|| {
        Mutex::new(Diag {
            started: Instant::now(),
            ring: VecDeque::with_capacity(RING_CAPACITY),
            dropped: 0,
            ring_rev: 0,
            crashes: 0,
            live_crash: None,
            loaded_crash: None,
            crash_rev: 0,
        })
    })
}

/// Poison-tolerant: every value behind this lock is a counter, a string or a
/// ring that stays consistent when a push is abandoned halfway, so recovering
/// the guard is always correct — and never panicking here is what keeps a
/// diagnostics call from turning into a second panic.
fn lock() -> MutexGuard<'static, Diag> {
    diag()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Seconds since the library was first used (monotonic, for the log prefix).
pub fn uptime_secs() -> f64 {
    diag()
        .lock()
        .map(|d| d.started.elapsed().as_secs_f64())
        .unwrap_or_else(|p| p.into_inner().started.elapsed().as_secs_f64())
}

/// Append one event to the ring. Never panics, never blocks the caller for long.
pub fn push(level: Level, text: impl Into<String>) {
    let mut text = text.into();
    if text.chars().count() > MAX_LINE_CHARS {
        text = text.chars().take(MAX_LINE_CHARS).collect::<String>() + "…";
    }
    // One ring line must stay one line: a panic payload with a newline would
    // otherwise break the panel's row accounting.
    text = text.replace(['\n', '\r'], " ⏎ ");

    let mut diag = lock();
    if diag.ring.len() == RING_CAPACITY {
        diag.ring.pop_front();
        diag.dropped = diag.dropped.saturating_add(1);
    }
    diag.ring_rev = diag.ring_rev.wrapping_add(1);
    let at = diag.started.elapsed().as_secs_f64();
    diag.ring.push_back(LogLine { at, level, text });
    // Keep the signal dump current while the ring is locked: a native kill at
    // the next stage must be able to name this line. The render is a few KiB of
    // formatting per event, and the spike logs tens per second — cheap next to
    // the value of the file existing at all.
    let snapshot = render_signal_snapshot(&diag);
    drop(diag);
    SIGNAL_SNAPSHOT.store(&snapshot);
}

/// Convenience wrappers so call sites read like `log::info!`.
pub fn info(text: impl Into<String>) {
    push(Level::Info, text);
}
pub fn warn(text: impl Into<String>) {
    push(Level::Warn, text);
}
pub fn error(text: impl Into<String>) {
    push(Level::Error, text);
}

/// Copy of the ring, oldest first — the UI reads it once per frame.
pub fn ring() -> Vec<LogLine> {
    lock().ring.iter().cloned().collect()
}

/// How many lines fell off the front since process start.
pub fn dropped_lines() -> u64 {
    lock().dropped
}

/// Revision of the ring: changes on every [`push`]. Cheaper than cloning the
/// lines each frame.
pub fn ring_revision() -> u64 {
    lock().ring_rev
}

/// Revision of the crash record.
pub fn crash_revision() -> u64 {
    lock().crash_rev
}

/// How many crashes this process recorded.
pub fn crash_count() -> u64 {
    lock().crashes
}

/// The crash to show, if any: this session's takes precedence over the one read
/// from disk at startup.
pub fn crash_for_display() -> Option<CrashView> {
    let diag = lock();
    if let Some(crash) = &diag.live_crash {
        return Some(CrashView {
            text: crash.text.clone(),
            live: true,
        });
    }
    diag.loaded_crash.as_ref().map(|text| CrashView {
        text: text.clone(),
        live: false,
    })
}

/// Point the diagnostics at the directory Kotlin chose for the crash files —
/// the app's *external* files dir since v2.1 (`CrashFiles.dir`, browseable and
/// shareable), with the private dir only as its fallback. Called once from
/// `MainActivity.onCreate` (before `super.onCreate`, i.e. before `android_main`
/// exists), which is also when the persisted crash record is loaded.
pub fn set_files_dir(dir: &str) {
    let path = PathBuf::from(dir);
    if FILES_DIR.set(path).is_err() {
        return;
    }
    let loaded = read_crash_file(&PathBuf::from(dir));
    {
        let mut diag = lock();
        diag.loaded_crash = loaded;
        diag.crash_rev = diag.crash_rev.wrapping_add(1);
    }
    // The signal dump needs the path *before* it can be armed, and Kotlin calls
    // this before `super.onCreate` — the earliest point in the process the
    // spike gets. `nativeInit` calls `init_signal_handlers` again; idempotent.
    init_signal_handlers();
}

/// The files dir, when Kotlin has told us about it.
pub fn files_dir() -> Option<&'static Path> {
    FILES_DIR.get().map(PathBuf::as_path)
}

// --- the native-signal dump ------------------------------------------------

/// The libc calls the handler is allowed to make, declared here instead of
/// adding `libc` as a dependency (the workorder forbids new ones). All of them
/// are exported by bionic on every Android version this spike supports and by the
/// host libc the subprocess test runs against, and none of them allocates.
mod sys {
    use core::ffi::{c_char, c_int, c_long, c_void};

    extern "C" {
        pub fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
        pub fn fsync(fd: c_int) -> c_int;
        pub fn close(fd: c_int) -> c_int;
        /// Installs `handler` (a function pointer) or `SIG_DFL`; returns the
        /// previous handler. Simpler than `sigaction`, whose struct layout is
        /// per-arch — see the report for why the loss of `SA_SIGINFO` is
        /// acceptable here.
        pub fn signal(signum: c_int, handler: usize) -> usize;
        pub fn raise(sig: c_int) -> c_int;
        /// The raw syscall entry point. `open` is deliberately **not** declared
        /// directly: it is variadic in glibc and Rust reserves the name as a
        /// runtime symbol (a plain `extern "C" { fn open(..) }` is rejected at
        /// compile time on the host), so the file is opened through the
        /// `openat` syscall instead — same operation, one extra `AT_FDCWD`.
        pub fn syscall(num: c_long, ...) -> c_long;
    }

    /// `openat(dirfd, path, flags, mode)`.
    ///
    /// The numbers are the Linux syscall table for the two targets this code
    /// builds for (aarch64 Android, x86_64 for the host test).
    #[cfg(target_arch = "aarch64")]
    const SYS_OPENAT: c_long = 56;
    #[cfg(not(target_arch = "aarch64"))]
    const SYS_OPENAT: c_long = 257;
    /// The "paths are relative to the process' cwd" dirfd.
    const AT_FDCWD: c_int = -100;

    /// `open(2)`, through `openat`. Returns the fd, or a negative errno.
    ///
    /// # Safety
    /// `path` must be a NUL-terminated string that stays valid for the call.
    pub unsafe fn open_path(path: *const c_char, flags: c_int) -> c_int {
        syscall(
            SYS_OPENAT,
            AT_FDCWD,
            path,
            flags | 0o2000000, // O_CLOEXEC
            0o644,             // mode, only used with O_CREAT
        ) as c_int
    }
}

/// A pre-rendered crash block the signal handler can `write` without touching
/// the allocator or the diag lock.
///
/// Single-writer (the app thread) / single-reader (the handler, on whichever
/// thread faulted). `seq` is a seqlock: odd while a refresh is copying, even
/// when the buffer is stable. The handler does not spin on it — if it catches
/// an odd value it writes the previous content, which is the right trade for a
/// diagnostics path (never block a dying process on a writer that may be the
/// one that died).
struct SignalSnapshot {
    seq: AtomicU64,
    len: AtomicUsize,
    bytes: std::cell::UnsafeCell<[u8; SIGNAL_SNAPSHOT_CAP]>,
}

// SAFETY: the only writer is the thread that owns the diag lock (every call
// site holds it), and the reader only reads `len` then copies `bytes` through a
// raw pointer into `write(2)`. A torn read yields a mixed snapshot, never
// undefined behaviour: the buffer is plain bytes and `len` is bounded by the
// array size.
unsafe impl Sync for SignalSnapshot {}

impl SignalSnapshot {
    fn store(&self, text: &str) {
        let seq = self.seq.load(Ordering::Relaxed);
        // Odd: a refresh is in progress.
        self.seq.store(seq.wrapping_add(1), Ordering::Release);
        let mut len = text.len().min(SIGNAL_SNAPSHOT_CAP);
        // Never cut a UTF-8 sequence in half: the reader lossy-decodes, but a
        // whole character is nicer to look at on a phone.
        while len > 0 && !text.is_char_boundary(len) {
            len -= 1;
        }
        // SAFETY: `bytes` is SIGNAL_SNAPSHOT_CAP long and `len` is clamped to it.
        unsafe {
            std::ptr::copy_nonoverlapping(text.as_ptr(), self.bytes.get().cast::<u8>(), len);
        }
        self.len.store(len, Ordering::Release);
        // Even: stable again.
        self.seq.store(seq.wrapping_add(2), Ordering::Release);
    }
}

static SIGNAL_SNAPSHOT: SignalSnapshot = SignalSnapshot {
    seq: AtomicU64::new(0),
    len: AtomicUsize::new(0),
    bytes: std::cell::UnsafeCell::new([0; SIGNAL_SNAPSHOT_CAP]),
};
/// The phase, readable without the diag lock — that is its whole purpose.
static PHASE: AtomicU8 = AtomicU8::new(Phase::Boot as u8);
/// `spike-last-crash.txt`, NUL-terminated, resolved once at install time: the
/// handler may not build a `Path`.
static SIGNAL_LAST_PATH: OnceLock<Vec<u8>> = OnceLock::new();
/// `spike-crash-log.txt`, same shape, appended so consecutive native kills all
/// survive instead of overwriting each other.
static SIGNAL_LOG_PATH: OnceLock<Vec<u8>> = OnceLock::new();
static SIGNAL_HANDLERS: AtomicBool = AtomicBool::new(false);

/// What the app is doing right now, as the signal dump will report it.
pub fn phase() -> Phase {
    Phase::from_u8(PHASE.load(Ordering::Relaxed))
}

/// Record the phase and refresh the snapshot.
///
/// Called by `app.rs` at every boundary the v2.3 kill could have landed in
/// (before/after each gpu-init stage, per frame, on suspend, on destroy), so a
/// native signal names the stage it interrupted rather than "something in
/// android_main".
pub fn set_phase(phase: Phase) {
    PHASE.store(phase as u8, Ordering::Relaxed);
    let snapshot = {
        let diag = lock();
        render_signal_snapshot(&diag)
    };
    SIGNAL_SNAPSHOT.store(&snapshot);
}

/// The body of `spike-last-crash.txt` as the handler writes it.
///
/// Deliberately the same shape as [`crash_block`] — `when`/`where`/`panic`
/// lines plus the ring — so the on-screen banner, the Kotlin overlay and this
/// file all read alike. There is no `panic:` line here: no Rust panic happened,
/// which is exactly why the file has to exist.
fn render_signal_snapshot(diag: &Diag) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // SAFETY-free: `process::id()` is a cached read of the pid.
    let pid = std::process::id();
    let mut out = String::with_capacity(2048);
    let _ = writeln!(
        out,
        "when:   {} (uptime {:.2}s)",
        utc_stamp(now),
        diag.started.elapsed().as_secs_f64()
    );
    let _ = writeln!(out, "pid:    {pid}");
    let _ = writeln!(out, "phase:  {}", phase().as_str());
    let _ = writeln!(
        out,
        "--- last {} platform events (oldest first) ---",
        diag.ring.len()
    );
    if diag.ring.is_empty() {
        let _ = writeln!(out, "(no events recorded)");
    }
    for line in &diag.ring {
        let _ = writeln!(out, "[{:8.2}s] {} {}", line.at, line.level.tag(), line.text);
    }
    out
}

/// The per-signal line that closes the dump. Pre-formatted so the handler does
/// no formatting at all.
fn signal_line(sig: i32) -> &'static [u8] {
    match sig {
        SIGSEGV => {
            b"SIGNAL: SIGSEGV (11) - the process faulted and is dying now.\n\
                     This file is the last in-process state; no Rust panic was raised.\n"
        }
        SIGABRT => {
            b"SIGNAL: SIGABRT (6) - the process aborted and is dying now.\n\
                     This file is the last in-process state; no Rust panic was raised.\n"
        }
        SIGBUS => {
            b"SIGNAL: SIGBUS (7) - the process hit a bus error and is dying now.\n\
                     This file is the last in-process state; no Rust panic was raised.\n"
        }
        _ => b"SIGNAL: unexpected signal - the process is dying now.\n",
    }
}

/// The handler. Async-signal-safe by construction: no allocation, no locks, no
/// formatting, no logging — `open`/`write`/`fsync`/`close` on static bytes,
/// then the default action restored and the signal re-raised so Android's
/// tombstone still records the fault at the real address.
extern "C" fn on_fatal_signal(sig: i32) {
    let len = SIGNAL_SNAPSHOT.len.load(Ordering::Acquire);
    let bytes = SIGNAL_SNAPSHOT.bytes.get().cast::<u8>();
    let line = signal_line(sig);

    // SAFETY: every call is a plain libc syscall wrapper; all arguments point
    // at `'static`/stack-free memory valid for the duration of the call.
    unsafe {
        if let Some(path) = SIGNAL_LAST_PATH.get() {
            let fd = sys::open_path(path.as_ptr().cast(), O_WRONLY | O_CREAT | O_TRUNC);
            if fd >= 0 {
                let _ = sys::write(fd, SIGNAL_HEADER.as_ptr().cast(), SIGNAL_HEADER.len());
                let _ = sys::write(fd, bytes.cast(), len);
                let _ = sys::write(fd, line.as_ptr().cast(), line.len());
                let _ = sys::fsync(fd);
                let _ = sys::close(fd);
            }
        }
        // The history keeps every native kill, not just the newest.
        if let Some(path) = SIGNAL_LOG_PATH.get() {
            let fd = sys::open_path(path.as_ptr().cast(), O_WRONLY | O_CREAT | O_APPEND);
            if fd >= 0 {
                let _ = sys::write(fd, SIGNAL_HEADER.as_ptr().cast(), SIGNAL_HEADER.len());
                let _ = sys::write(fd, bytes.cast(), len);
                let _ = sys::write(fd, line.as_ptr().cast(), line.len());
                let _ = sys::write(fd, b"\n".as_ptr().cast(), 1);
                let _ = sys::fsync(fd);
                let _ = sys::close(fd);
            }
        }
        // Re-raise with the default action so the process still dies of the
        // original signal, at the original fault, for the tombstone to be right.
        //
        // The signal is blocked for the duration of its own handler, so this
        // `raise` becomes *pending* and is delivered the moment the handler
        // returns — with `SIG_DFL` now installed, which is what kills the
        // process here. Calling `_exit` before returning would swallow that
        // delivery (the first version did exactly that: exit code 139 instead of
        // a real SIGSEGV), so there is deliberately nothing after this line.
        sys::signal(sig, SIG_DFL);
        sys::raise(sig);
    }
}

/// Arm the native-signal dump. Idempotent; call from `nativeInit` (earliest)
/// and again from `android_main` so a library loaded without `onCreate` is
/// still covered.
///
/// Requires [`set_files_dir`] to have run, otherwise there is nowhere to write
/// and the handler is installed anyway (it simply finds no path and re-raises).
pub fn init_signal_handlers() {
    if SIGNAL_HANDLERS.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(dir) = files_dir() {
        for name in [LAST_CRASH_FILE, CRASH_LOG_FILE] {
            let mut bytes = dir.join(name).to_string_lossy().into_owned().into_bytes();
            bytes.push(0);
            let slot = if name == LAST_CRASH_FILE {
                &SIGNAL_LAST_PATH
            } else {
                &SIGNAL_LOG_PATH
            };
            let _ = slot.set(bytes);
        }
    } else {
        info("native signal handler: no files dir yet — the dump will re-raise without a file");
    }
    // One snapshot before any event: a crash during the first gpu call still
    // names its phase.
    SIGNAL_SNAPSHOT.store(&{
        let diag = lock();
        render_signal_snapshot(&diag)
    });
    for sig in [SIGSEGV, SIGABRT, SIGBUS] {
        // SAFETY: `on_fatal_signal` has the `extern "C" fn(c_int)` shape
        // `signal(2)` expects; it never unwinds.
        unsafe {
            sys::signal(sig, on_fatal_signal as *const () as usize);
        }
    }
    info("native signal handler installed (SIGSEGV/SIGABRT/SIGBUS -> spike-last-crash.txt)");
}

/// Delete the crash record (both files) and forget it in memory. Only the
/// on-screen **Clear crash** button calls this.
pub fn clear_crash() -> Result<(), String> {
    let mut diag = lock();
    diag.live_crash = None;
    diag.loaded_crash = None;
    diag.crash_rev = diag.crash_rev.wrapping_add(1);
    drop(diag);

    let Some(dir) = files_dir() else {
        return Ok(());
    };
    let mut first_error = None;
    for name in [LAST_CRASH_FILE, CRASH_LOG_FILE] {
        let path = dir.join(name);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                first_error.get_or_insert_with(|| format!("{}: {err}", path.display()));
            }
        }
    }
    match first_error {
        Some(err) => Err(err),
        None => {
            info("crash record cleared");
            Ok(())
        }
    }
}

/// Install the panic hook. Idempotent; called from `nativeInit` and again from
/// `android_main` so a panic during event-loop setup is still recorded.
pub fn init_panic_hook() {
    if HOOK_INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        record_panic(info);
        // Keep the platform's own reporting (logcat / stderr) intact.
        previous(info);
    }));
}

/// Record a panic that a `catch_unwind` caught. The hook already wrote the
/// message/location; this appends where it was contained so the file says
/// whether the process survived.
pub fn note_contained(site: &str) {
    let mut diag = lock();
    let Some(crash) = diag.live_crash.as_mut() else {
        return;
    };
    if crash.contained_at.is_some() {
        return;
    }
    crash.contained_at = Some(site.to_owned());
    let _ = writeln!(crash.text, "contained: {site} — the process kept running");
    let text = crash.text.clone();
    diag.crash_rev = diag.crash_rev.wrapping_add(1);
    drop(diag);
    if let Some(dir) = files_dir() {
        write_last_crash(dir, &text);
    }
}

/// Best-effort text for a panic payload (which is `Box<dyn Any + Send>`).
pub fn payload_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "<panic payload was not a string>".to_owned()
    }
}

fn record_panic(info: &PanicHookInfo<'_>) {
    let message = payload_text(info.payload());
    let location = info
        .location()
        .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
        .unwrap_or_else(|| "<unknown>".to_owned());
    let thread = std::thread::current()
        .name()
        .map(str::to_owned)
        .unwrap_or_else(|| "<unnamed>".to_owned());

    // `try_lock`: if the panic happened *inside* a diagnostics call the lock is
    // already held by this thread and blocking here would deadlock the hook.
    // Everything the block needs is captured *under* that one guard — the earlier
    // version called `uptime_secs()` from `crash_block` and self-deadlocked the
    // moment a panic was actually recorded (caught by
    // `the_panic_hook_records_without_deadlocking`).
    let (seq, block, uptime) = match diag().try_lock() {
        Ok(mut diag) => {
            let uptime = Some(diag.started.elapsed().as_secs_f64());
            diag.crashes = diag.crashes.saturating_add(1);
            let seq = diag.crashes;
            let tail = diag.ring.iter().cloned().collect::<Vec<_>>();
            let text = crash_block(seq, &thread, &location, &message, uptime, &tail);
            diag.live_crash = Some(CrashRecord {
                seq,
                text: text.clone(),
                contained_at: None,
            });
            diag.crash_rev = diag.crash_rev.wrapping_add(1);
            diag.ring_rev = diag.ring_rev.wrapping_add(1);
            if diag.ring.len() == RING_CAPACITY {
                diag.ring.pop_front();
            }
            let at = diag.started.elapsed().as_secs_f64();
            diag.ring.push_back(LogLine {
                at,
                level: Level::Error,
                text: format!("PANIC {message}"),
            });
            (seq, Some(text), uptime)
        }
        Err(_) => (0, None, None),
    };

    if let Some(dir) = FILES_DIR.get() {
        let text = match block {
            Some(text) => text,
            // Lock unavailable: the file still gets message + location, just no
            // event run-up.
            None => crash_block(seq, &thread, &location, &message, uptime, &[]),
        };
        write_crash_files(dir, &text);
    }
}

fn crash_block(
    seq: u64,
    thread: &str,
    location: &str,
    message: &str,
    uptime: Option<f64>,
    tail: &[LogLine],
) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let uptime = match uptime {
        Some(secs) => format!(" (uptime {secs:.2}s)"),
        None => " (uptime unavailable)".to_owned(),
    };
    let mut out = String::with_capacity(1024);
    let _ = writeln!(out, "=== Suramak spike crash #{seq} ===");
    let _ = writeln!(out, "when:   {}{uptime}", utc_stamp(now));
    let _ = writeln!(out, "thread: {thread}");
    let _ = writeln!(out, "where:  {location}");
    let _ = writeln!(out, "panic:  {message}");
    let _ = writeln!(
        out,
        "--- last {} platform events (oldest first) ---",
        tail.len()
    );
    if tail.is_empty() {
        let _ = writeln!(out, "(no events recorded)");
    }
    for line in tail {
        let _ = writeln!(out, "[{:8.2}s] {} {}", line.at, line.level.tag(), line.text);
    }
    out
}

/// `YYYY-MM-DD HH:MM:SSZ` from a Unix timestamp, UTC. No chrono in this crate:
/// days-to-civil is Howard Hinnant's `civil_from_days`, which is exact for every
/// date this app can produce.
pub fn utc_stamp(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let secs_of_day = unix_secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}Z",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}

fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

fn read_crash_file(dir: &Path) -> Option<String> {
    let path = dir.join(LAST_CRASH_FILE);
    let meta = fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() == 0 {
        return None;
    }
    let bytes = fs::read(&path).ok()?;
    let slice = if bytes.len() as u64 > MAX_CRASH_READ {
        &bytes[bytes.len() - MAX_CRASH_READ as usize..]
    } else {
        &bytes[..]
    };
    let text = String::from_utf8_lossy(slice).into_owned();
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_owned())
    }
}

/// Replace the "last crash" file atomically (write + rename) so a reader never
/// sees half a record, and append the same block to the history log.
fn write_crash_files(dir: &Path, text: &str) {
    write_last_crash(dir, text);

    let history = dir.join(CRASH_LOG_FILE);
    if let Ok(meta) = fs::metadata(&history) {
        if meta.len() > MAX_HISTORY_BYTES {
            trim_history(&history);
        }
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&history) {
        let _ = file.write_all(text.as_bytes());
        let _ = file.write_all(b"\n");
        let _ = file.sync_data();
    }
}

fn write_last_crash(dir: &Path, text: &str) {
    let last = dir.join(LAST_CRASH_FILE);
    let tmp = dir.join(format!("{LAST_CRASH_FILE}.tmp"));
    let Ok(mut file) = File::create(&tmp) else {
        return;
    };
    if file.write_all(text.as_bytes()).is_ok() && file.sync_all().is_ok() {
        let _ = fs::rename(&tmp, &last);
    }
}

/// Keep the newest ~48 KiB of the history, starting at a record boundary.
fn trim_history(path: &Path) {
    let Ok(bytes) = fs::read(path) else {
        return;
    };
    if bytes.len() as u64 <= KEEP_HISTORY_BYTES {
        return;
    }
    let start = bytes.len() - KEEP_HISTORY_BYTES as usize;
    let slice = &bytes[start..];
    // Resynchronise on the next crash header so the file still starts at a
    // whole record; fall back to the raw tail.
    let marker = b"=== Suramak spike crash #";
    let cut = slice
        .windows(marker.len())
        .position(|window| window == marker)
        .unwrap_or(0);
    let _ = fs::write(path, &slice[cut..]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering as AtomicOrdering};

    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "suramak-spike-diag-{}-{}-{}",
            std::process::id(),
            tag,
            COUNTER.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn utc_stamp_matches_known_dates() {
        assert_eq!(utc_stamp(0), "1970-01-01 00:00:00Z");
        // 2023-11-14T22:13:20Z
        assert_eq!(utc_stamp(1_700_000_000), "2023-11-14 22:13:20Z");
        // Leap day, and the end of a 366-day year.
        assert_eq!(utc_stamp(1_709_164_800), "2024-02-29 00:00:00Z");
        assert_eq!(utc_stamp(1_735_689_599), "2024-12-31 23:59:59Z");
    }

    #[test]
    fn ring_keeps_the_newest_lines_and_counts_drops() {
        let before = dropped_lines();
        for index in 0..(RING_CAPACITY + 5) {
            push(Level::Info, format!("line {index}"));
        }
        let lines = ring();
        assert_eq!(lines.len(), RING_CAPACITY);
        assert!(lines
            .last()
            .unwrap()
            .text
            .ends_with(&format!("line {}", RING_CAPACITY + 4)));
        assert!(dropped_lines() >= before + 5);
    }

    #[test]
    fn ring_lines_are_single_line_and_bounded() {
        push(Level::Error, "first line\nsecond line");
        let lines = ring();
        let last = lines.last().unwrap();
        assert!(!last.text.contains('\n'));
        assert!(last.text.contains("first line"));

        let long = "x".repeat(MAX_LINE_CHARS + 50);
        push(Level::Warn, long);
        let lines = ring();
        assert!(lines.last().unwrap().text.chars().count() <= MAX_LINE_CHARS + 1);
    }

    #[test]
    fn crash_block_carries_message_location_thread_and_the_run_up() {
        let tail = vec![
            LogLine {
                at: 1.5,
                level: Level::Info,
                text: "jni: nativeSetInsets(64,48,0,0,0)".to_owned(),
            },
            LogLine {
                at: 1.6,
                level: Level::Warn,
                text: "kotlin: surfaceDestroyed".to_owned(),
            },
        ];
        let block = crash_block(7, "main", "src/app.rs:10:5", "boom", Some(1.5), &tail);
        assert!(block.contains("crash #7"), "{block}");
        assert!(block.contains("thread: main"));
        assert!(block.contains("where:  src/app.rs:10:5"));
        assert!(block.contains("panic:  boom"));
        assert!(block.contains("nativeSetInsets"));
        assert!(block.contains("surfaceDestroyed"));
        assert!(block.contains("Z (uptime 1.50s)"));
    }

    /// The hook used to call `uptime_secs()` from inside `crash_block`, i.e. it
    /// re-locked the mutex it already held — the first real panic would have
    /// frozen the thread instead of recording anything. A hang here fails on the
    /// channel timeout rather than wedging the suite.
    #[test]
    fn the_panic_hook_records_without_deadlocking() {
        init_panic_hook();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let outcome = std::panic::catch_unwind(|| panic!("hook deadlock probe"));
            assert!(outcome.is_err());
            let _ = tx.send(());
        });
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("the panic hook deadlocked on its own lock");

        let view = crash_for_display().expect("the panic was not recorded");
        assert!(view.text.contains("hook deadlock probe"), "{}", view.text);
        assert!(view.text.contains("uptime"), "{}", view.text);
        assert!(view.text.contains("thread:"), "{}", view.text);
    }

    #[test]
    fn crash_files_round_trip_and_clear_removes_both() {
        let dir = temp_dir("files");
        let text = "=== Suramak spike crash #1 ===\npanic: kaput\n";
        write_crash_files(&dir, text);

        let loaded = read_crash_file(&dir).expect("crash file readable");
        assert!(loaded.contains("kaput"));
        assert!(dir.join(CRASH_LOG_FILE).is_file());

        // A second crash appends to the history but replaces the last-crash file.
        write_crash_files(&dir, "=== Suramak spike crash #2 ===\npanic: again\n");
        let history = fs::read_to_string(dir.join(CRASH_LOG_FILE)).unwrap();
        assert_eq!(history.matches("crash #").count(), 2, "{history}");
        let loaded = read_crash_file(&dir).unwrap();
        assert!(loaded.contains("again"));
        assert!(!loaded.contains("kaput"));

        fs::remove_file(dir.join(LAST_CRASH_FILE)).unwrap();
        fs::remove_file(dir.join(CRASH_LOG_FILE)).unwrap();
        assert_eq!(read_crash_file(&dir), None);

        // An empty file is "no crash", not an empty banner.
        fs::write(dir.join(LAST_CRASH_FILE), "").unwrap();
        assert_eq!(read_crash_file(&dir), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn history_is_trimmed_to_the_newest_whole_record() {
        let dir = temp_dir("trim");
        let mut big = String::new();
        for index in 0..6000 {
            let _ = writeln!(
                big,
                "=== Suramak spike crash #{index} ===\npanic: filler {index}"
            );
        }
        fs::write(dir.join(CRASH_LOG_FILE), &big).unwrap();
        assert!(fs::metadata(dir.join(CRASH_LOG_FILE)).unwrap().len() > MAX_HISTORY_BYTES);

        trim_history(&dir.join(CRASH_LOG_FILE));
        let trimmed = fs::read_to_string(dir.join(CRASH_LOG_FILE)).unwrap();
        assert!(trimmed.len() as u64 <= KEEP_HISTORY_BYTES + 128);
        assert!(
            trimmed.starts_with("=== Suramak spike crash #"),
            "{}",
            &trimmed[..40.min(trimmed.len())]
        );
        assert!(trimmed.contains("filler 5999"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn payload_text_handles_both_string_shapes() {
        let borrowed: &(dyn std::any::Any + Send) = &"borrowed";
        assert_eq!(payload_text(borrowed), "borrowed");
        let owned: &(dyn std::any::Any + Send) = &String::from("owned");
        assert_eq!(payload_text(owned), "owned");
        let other: &(dyn std::any::Any + Send) = &7u32;
        assert!(payload_text(other).contains("not a string"));
    }

    // --- v2.4: the native-signal dump -------------------------------------

    /// Read the pre-rendered buffer exactly as `on_fatal_signal` does.
    fn snapshot_text() -> String {
        let len = SIGNAL_SNAPSHOT.len.load(Ordering::Acquire);
        assert!(len <= SIGNAL_SNAPSHOT_CAP);
        // SAFETY: `len` is bounded by the array size by `SignalSnapshot::store`.
        let slice =
            unsafe { std::slice::from_raw_parts(SIGNAL_SNAPSHOT.bytes.get().cast::<u8>(), len) };
        String::from_utf8_lossy(slice).into_owned()
    }

    #[test]
    fn the_signal_snapshot_is_a_whole_crash_block_with_phase_and_pid() {
        set_phase(Phase::GpuAdapter);
        let text = snapshot_text();
        // The header is written by the handler, not the snapshot: the file
        // opens with it so a truncated snapshot can never look like a whole one.
        assert!(!text.starts_with("=== "), "{text}");
        assert!(text.contains("when:   "), "{text}");
        assert!(text.contains("Z (uptime "), "{text}");
        assert!(text.contains("pid:    "), "{text}");
        assert!(text.contains("phase:  gpu init: adapter"), "{text}");
        assert!(
            text.contains("--- last "),
            "the ring header must name the block: {text}"
        );
        assert!(text.contains("platform events (oldest first) ---"));
        assert_eq!(phase(), Phase::GpuAdapter);
        // Every phase has a distinct, non-empty name — the dump is only useful
        // if that line identifies a stage.
        let mut names: Vec<&str> = Phase::ALL.iter().map(|phase| phase.as_str()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count);
        assert!(Phase::ALL.iter().all(|phase| !phase.as_str().is_empty()));
    }

    /// The handler's persistence is proven in a **child process**, because the
    /// only honest test of a fatal-signal handler is a process that dies of the
    /// signal. The child is this same test binary, filtered to
    /// [`signal_child_raises_sigsegv`]; the parent asserts three things a mock
    /// cannot: the child really died of SIGSEGV (`ExitStatus::signal`), the
    /// file exists anyway, and it carries the ring + the phase + the signal.
    #[test]
    fn the_signal_handler_writes_the_ring_then_re_raises_sigsegv() {
        use std::os::unix::process::ExitStatusExt;

        let dir = temp_dir("signal");
        let exe = std::env::current_exe().expect("the test binary's own path");
        let output = std::process::Command::new(&exe)
            .args([
                "--exact",
                "diag::tests::signal_child_raises_sigsegv",
                "--nocapture",
            ])
            .env("SURAMAK_SIGNAL_CHILD_DIR", &dir)
            .output()
            .expect("spawn the signal child");

        assert_eq!(
            output.status.signal(),
            Some(SIGSEGV),
            "the child did not die of SIGSEGV (status {:?}); the handler must \
             re-raise, not swallow. stderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let path = dir.join(LAST_CRASH_FILE);
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("no crash file at {}: {err}", path.display()));
        assert!(
            text.starts_with("=== Suramak spike NATIVE SIGNAL ==="),
            "{text}"
        );
        assert!(text.contains("phase:  gpu init: device"), "{text}");
        assert!(
            text.contains("in-flight: request_device returned; about to configure"),
            "{text}"
        );
        assert!(text.contains("SIGSEGV (11)"), "{text}");
        assert!(text.contains("no Rust panic was raised"), "{text}");
        // The history keeps the same block, so repeated kills are all readable.
        let history = fs::read_to_string(dir.join(CRASH_LOG_FILE)).unwrap_or_default();
        assert!(history.contains("NATIVE SIGNAL"), "{history}");

        let _ = fs::remove_dir_all(&dir);
    }

    /// Not a test in its own right: it is the child body the test above spawns.
    /// Without the environment variable it returns immediately, so a normal
    /// `cargo test` run counts it as a pass and nothing is raised.
    #[test]
    fn signal_child_raises_sigsegv() {
        let Ok(dir) = std::env::var("SURAMAK_SIGNAL_CHILD_DIR") else {
            return;
        };
        set_files_dir(&dir);
        // Exactly the state the v2.3 field kill would have left: a phase, and a
        // last event describing the step in flight.
        set_phase(Phase::GpuDevice);
        info("in-flight: request_device returned; about to configure");
        init_signal_handlers();
        // SAFETY: the handler is installed immediately above and this is the
        // child process whose only job is to die here.
        unsafe {
            sys::raise(SIGSEGV);
        }
        panic!("raise(SIGSEGV) returned — the handler is not installed");
    }
}
