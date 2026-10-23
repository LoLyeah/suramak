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
use std::sync::atomic::{AtomicBool, Ordering};
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
    let mut diag = lock();
    diag.loaded_crash = loaded;
    diag.crash_rev = diag.crash_rev.wrapping_add(1);
}

/// The files dir, when Kotlin has told us about it.
pub fn files_dir() -> Option<&'static Path> {
    FILES_DIR.get().map(PathBuf::as_path)
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
}
