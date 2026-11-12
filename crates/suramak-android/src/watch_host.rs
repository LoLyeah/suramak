//! M24A — the Android watched inbox host.
//!
//! The phone's folder is a **SAF tree**, never a raw path: `MainActivity`
//! launches `ACTION_OPEN_DOCUMENT_TREE`, takes the persistable read grant and
//! hands the tree URI here through the compact bridge. That URI is what
//! `settings.watch.folder` stores, so the grant survives reboots.
//!
//! [`SafWatchSource`] is the Android implementation of
//! [`suramak_core::watch::WatchSource`] (list a document's children, open one
//! document's bytes). The traversal itself lives in Kotlin (`WatchTree.kt`)
//! because only the content resolver can walk a tree; the Rust side is the
//! small JNI binding, compiled by the NDK gate only. The scanner's rules and
//! the ledger live in `suramak-core` and are host-tested with the fake source.
//!
//! One thread polls every 5 s while the app is in the foreground, and for at
//! most [`BACKGROUND_SCAN_WINDOW_MS`] after it leaves the foreground (a 5 s
//! poll must not keep a phone awake). Imports are copied into `media/` and
//! handed to the existing [`crate::jobs::JobHost`] — the same import path a
//! SAF pick or a share takes.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use suramak_core::settings::{LangMode, Settings};
use suramak_core::store::Store;
use suramak_core::watch::{
    folder_display_name, Inbox, RealClock, WatchCadence, WatchClock, WATCH_POLL_INTERVAL_MS,
};
use suramak_ui::deck::compact::WatchView;

use crate::jobs::HostDirs;

/// The process-wide host (created by `init` from the compact bridge).
static HOST: std::sync::OnceLock<WatchHost> = std::sync::OnceLock::new();

/// Creates the one host; repeated `init` calls are no-ops.
pub fn init(root: &Path) {
    let _ = HOST.set(WatchHost::new(root));
    if let Some(host) = HOST.get() {
        host.configure();
    }
}

pub fn host() -> Option<&'static WatchHost> {
    HOST.get()
}

/// The Library/Settings view of the watcher: whether watching is on, the
/// folder's display name, and the status row (None until the first poll).
pub fn view() -> (bool, String, Option<WatchView>) {
    match host() {
        Some(host) => host.view(),
        None => (false, String::new(), None),
    }
}

/// `settings.watch.enabled` (the Settings switch).
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    match host() {
        Some(host) => host.set_enabled(enabled),
        None => Err("the watched inbox host has not been initialised".to_string()),
    }
}

/// The tree URI the picker returned (persisted + started).
pub fn set_folder(uri: &str) -> Result<(), String> {
    match host() {
        Some(host) => host.set_folder(uri),
        None => Err("the watched inbox host has not been initialised".to_string()),
    }
}

/// The Activity's foreground state (the battery rule's input).
pub fn set_foreground(foreground: bool) {
    if let Some(host) = host() {
        host.set_foreground(foreground);
    }
}

/// Re-reads the settings and reconciles the poll thread (after a Settings
/// save; a no-op when the folder did not change).
pub fn configure() {
    if let Some(host) = host() {
        host.configure();
    }
}

/// A JSON status for the bridge (`{"ok":true,"enabled":..,"folder":..,...}`).
pub fn status_json() -> String {
    let (enabled, folder, view) = view();
    let imports = view.as_ref().map(|view| view.imports).unwrap_or(0);
    let failure = view.as_ref().and_then(|view| view.failure.clone());
    serde_json::json!({
        "ok": true,
        "enabled": enabled,
        "folder": folder,
        "imports": imports,
        "failure": failure,
        "poll_ms": WATCH_POLL_INTERVAL_MS,
    })
    .to_string()
}

/// The Android watched inbox.
pub struct WatchHost {
    inner: Mutex<Inner>,
}

struct Inner {
    settings_path: PathBuf,
    dirs: HostDirs,
    /// The shared status row, read by the frame without taking this lock.
    shared: Arc<Mutex<ViewState>>,
    commands: Option<Sender<Command>>,
    thread: Option<JoinHandle<()>>,
}

/// What the poll thread publishes.
struct ViewState {
    enabled: bool,
    folder: String,
    folder_name: String,
    imports: u64,
    failure: Option<String>,
}

impl ViewState {
    fn view(&self) -> Option<WatchView> {
        self.enabled.then(|| WatchView {
            folder: self.folder_name.clone(),
            imports: self.imports,
            failure: self.failure.clone(),
        })
    }
}

enum Command {
    Stop,
    Foreground(bool),
}

impl WatchHost {
    pub fn new(root: &Path) -> Self {
        Self {
            inner: Mutex::new(Inner {
                settings_path: root.join("settings.toml"),
                dirs: HostDirs::under(root),
                shared: Arc::new(Mutex::new(ViewState {
                    enabled: false,
                    folder: String::new(),
                    folder_name: String::new(),
                    imports: 0,
                    failure: None,
                })),
                commands: None,
                thread: None,
            }),
        }
    }

    /// Reconciles the thread with the stored settings (called at init and
    /// after every Settings save).
    pub fn configure(&self) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let settings = Settings::load(&inner.settings_path).unwrap_or_default();
        let wanted = settings
            .watch
            .is_configured()
            .then(|| settings.watch.folder.clone());
        if inner.thread.is_some() {
            let current = inner
                .shared
                .lock()
                .ok()
                .map(|state| state.folder.clone())
                .unwrap_or_default();
            if Some(current.as_str()) == wanted.as_deref() {
                return;
            }
        }
        inner.stop();
        let Some(folder) = wanted else {
            if let Ok(mut shared) = inner.shared.lock() {
                shared.enabled = false;
                shared.failure = None;
            }
            return;
        };
        if let Some(commands) = inner.spawn(folder, settings) {
            inner.commands = Some(commands);
        } else if let Ok(mut shared) = inner.shared.lock() {
            shared.failure = Some("the folder watcher could not start".to_string());
        }
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        let mut settings = Settings::load(&self.settings_path()).unwrap_or_default();
        settings.watch.enabled = enabled;
        settings
            .save(&self.settings_path())
            .map_err(|err| err.to_string())?;
        self.configure();
        Ok(())
    }

    /// Stores the picker's tree URI and starts watching it. A different tree
    /// starts a fresh ledger (the old folder's rows would otherwise suppress
    /// nothing useful and would inflate `N`).
    pub fn set_folder(&self, uri: &str) -> Result<(), String> {
        if !uri.starts_with("content://") {
            return Err("the watched folder must be a SAF tree URI".to_string());
        }
        let mut settings = Settings::load(&self.settings_path()).unwrap_or_default();
        let changed = settings.watch.folder != uri;
        settings.watch.folder = uri.to_string();
        settings.watch.enabled = true;
        settings
            .save(&self.settings_path())
            .map_err(|err| err.to_string())?;
        if changed {
            // A new folder is a new inbox.
            let db_path = self
                .inner
                .lock()
                .map(|inner| inner.dirs.db_path.clone())
                .unwrap_or_default();
            if let Ok(store) = Store::open(&db_path) {
                let _ = store.clear_watched_files();
            }
        }
        self.configure();
        Ok(())
    }

    pub fn set_foreground(&self, foreground: bool) {
        if let Ok(inner) = self.inner.lock() {
            if let Some(commands) = &inner.commands {
                let _ = commands.send(Command::Foreground(foreground));
            }
        }
    }

    pub fn view(&self) -> (bool, String, Option<WatchView>) {
        let Ok(inner) = self.inner.lock() else {
            return (false, String::new(), None);
        };
        let Ok(shared) = inner.shared.lock() else {
            return (false, String::new(), None);
        };
        (shared.enabled, shared.folder_name.clone(), shared.view())
    }

    fn settings_path(&self) -> PathBuf {
        self.inner
            .lock()
            .map(|inner| inner.settings_path.clone())
            .unwrap_or_default()
    }
}

impl Inner {
    fn stop(&mut self) {
        if let Some(commands) = self.commands.take() {
            let _ = commands.send(Command::Stop);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if let Ok(mut shared) = self.shared.lock() {
            shared.enabled = false;
        }
    }

    /// Spawns the poll thread. `None` when the platform has no SAF source.
    fn spawn(&mut self, folder: String, settings: Settings) -> Option<Sender<Command>> {
        let source = build_source(&folder)?;
        let (commands, receiver) = mpsc::channel();
        let shared = Arc::clone(&self.shared);
        let db_path = self.dirs.db_path.clone();
        let model = settings.asr.model_id.clone();
        let language = match &settings.asr.language {
            LangMode::Auto => "auto".to_string(),
            LangMode::Fixed(lang) => lang.clone(),
        };
        let thread_folder = folder.clone();
        let known_imports = Store::open(&self.dirs.db_path)
            .ok()
            .and_then(|store| store.watched_import_count().ok())
            .and_then(|count| u64::try_from(count).ok())
            .unwrap_or(0);
        if let Ok(mut shared) = self.shared.lock() {
            shared.enabled = true;
            shared.folder = folder.clone();
            shared.folder_name = folder_display_name(&folder);
            shared.imports = known_imports;
            shared.failure = None;
        }
        let thread = std::thread::Builder::new()
            .name("suramak-watch".to_string())
            .spawn(move || {
                run(
                    thread_folder,
                    source,
                    db_path,
                    model,
                    language,
                    shared,
                    receiver,
                );
            })
            .ok()?;
        self.thread = Some(thread);
        Some(commands)
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(
    folder: String,
    source: Box<dyn suramak_core::watch::WatchSource>,
    db_path: PathBuf,
    model: String,
    language: String,
    shared: Arc<Mutex<ViewState>>,
    commands: mpsc::Receiver<Command>,
) {
    let set = |failure: Option<String>, imports: Option<u64>| {
        if let Ok(mut state) = shared.lock() {
            if failure.is_some() {
                state.failure = failure.clone();
            }
            if let Some(imports) = imports {
                state.imports = imports;
            }
            if failure.is_none() && imports.is_some() {
                state.failure = None;
            }
        }
    };
    let store = match Store::open(&db_path) {
        Ok(store) => store,
        Err(err) => {
            set(
                Some(format!("the watched inbox cannot read the library: {err}")),
                None,
            );
            return;
        }
    };
    let mut inbox = Inbox::new(source, folder);
    let clock = RealClock::new();
    let mut cadence = WatchCadence::default();
    loop {
        // While the background window has expired the thread parks on the
        // command channel: zero wakes until the Activity says it is visible
        // again (a 5 s timer would still wake the CPU every 5 s).
        if !cadence.should_scan(clock.now_ms()) {
            match commands.recv() {
                Ok(Command::Stop) | Err(_) => break,
                Ok(Command::Foreground(foreground)) => {
                    cadence.set_foreground(foreground, clock.now_ms());
                }
            }
            continue;
        }
        match commands.recv_timeout(Duration::from_millis(WATCH_POLL_INTERVAL_MS)) {
            Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(Command::Foreground(foreground)) => {
                cadence.set_foreground(foreground, clock.now_ms());
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        let now = clock.now_ms();
        if !cadence.should_scan(now) {
            continue;
        }
        match inbox.poll_with_clock(&store, &clock) {
            Ok(report) => {
                let mut failure = report.scan.failure.clone();
                for candidate in &report.imported {
                    if let Err(err) = import_candidate(&inbox, candidate, &model, &language) {
                        failure = Some(err);
                    }
                }
                set(failure, Some(report.imports_total));
            }
            Err(err) => set(
                Some(format!(
                    "the watched inbox could not record an import: {err}"
                )),
                None,
            ),
        }
    }
}

/// Copies one SAF document into the media inbox and queues it through the one
/// job host — the same path `MainActivity`'s SAF pick uses.
fn import_candidate(
    inbox: &Inbox,
    candidate: &suramak_core::watch::WatchCandidate,
    model: &str,
    language: &str,
) -> Result<(), String> {
    let Some(jobs) = crate::jobs_host() else {
        return Err("the transcription host is not running".to_string());
    };
    let mime = "audio/*";
    let (key, path) = jobs.begin_import(&candidate.name, mime, candidate.size, model, language)?;
    let mut reader = inbox
        .open(candidate)
        .map_err(|err| format!("could not read {}: {err}", candidate.name))?;
    let mut file = std::fs::File::create(&path)
        .map_err(|err| format!("could not stage {}: {err}", candidate.name))?;
    if let Err(err) = std::io::copy(&mut reader, &mut file) {
        let _ = std::fs::remove_file(&path);
        jobs.import_failed(&key, &format!("the folder copy failed: {err}"));
        return Err(format!("the folder copy failed: {err}"));
    }
    jobs.import_done(&key)
        .map_err(|err| format!("the queue refused {}: {err}", candidate.name))?;
    Ok(())
}

/// Builds the platform source. Android uses the Kotlin content resolver; every
/// other build refuses (the trait's fake is for tests).
#[cfg(target_os = "android")]
fn build_source(folder: &str) -> Option<Box<dyn suramak_core::watch::WatchSource>> {
    saf::SafWatchSource::new(folder.to_string())
        .ok()
        .map(|source| Box::new(source) as Box<dyn suramak_core::watch::WatchSource>)
}

#[cfg(not(target_os = "android"))]
fn build_source(_folder: &str) -> Option<Box<dyn suramak_core::watch::WatchSource>> {
    None
}

#[cfg(target_os = "android")]
mod saf {
    //! The JNI binding to `app.suramak.transcribe.WatchTree`.
    use std::io::Read;
    use std::sync::Arc;

    use jni::objects::{GlobalRef, JObject, JString, JValue};
    use jni::{JNIEnv, JavaVM};

    use suramak_core::watch::{WatchEntry, WatchError, WatchSource};

    pub struct SafWatchSource {
        tree: String,
        vm: Arc<JavaVM>,
    }

    impl SafWatchSource {
        pub fn new(tree: String) -> Result<Self, WatchError> {
            let context = ndk_context::android_context();
            // SAFETY: ndk-context holds the JavaVM pointer for the process.
            let vm = unsafe { JavaVM::from_raw(context.vm().cast()) }
                .map_err(|err| WatchError::io(format!("JavaVM: {err}")))?;
            Ok(Self {
                tree,
                vm: Arc::new(vm),
            })
        }

        fn with_env<T>(
            &self,
            run: impl FnOnce(&mut JNIEnv<'_>) -> Result<T, WatchError>,
        ) -> Result<T, WatchError> {
            let mut env = self
                .vm
                .attach_current_thread()
                .map_err(|err| WatchError::io(format!("attach: {err}")))?;
            run(&mut env)
        }
    }

    impl WatchSource for SafWatchSource {
        fn list(&self, dir: &str) -> Result<Vec<WatchEntry>, WatchError> {
            let relative = if dir == self.tree { "" } else { dir };
            self.with_env(|env| {
                let context = ndk_context::android_context();
                // SAFETY: the context is the Activity, valid while the process lives.
                let context = unsafe { JObject::from_raw(context.context().cast()) };
                let tree = env
                    .new_string(&self.tree)
                    .map_err(|err| WatchError::io(format!("tree string: {err}")))?;
                let relative = env
                    .new_string(relative)
                    .map_err(|err| WatchError::io(format!("dir string: {err}")))?;
                let reply = env
                    .call_static_method(
                        "app/suramak/transcribe/WatchTree",
                        "list",
                        "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",
                        &[
                            JValue::Object(&context),
                            JValue::Object(&tree),
                            JValue::Object(&relative),
                        ],
                    )
                    .map_err(|err| WatchError::io(format!("WatchTree.list: {err}")))?;
                let reply = reply
                    .l()
                    .map_err(|err| WatchError::io(format!("WatchTree.list: {err}")))?;
                let reply = JString::from(reply);
                let text: String = env
                    .get_string(&reply)
                    .map_err(|err| WatchError::io(format!("WatchTree.list text: {err}")))?
                    .into();
                parse_listing(&text)
            })
        }

        fn open(&self, entry: &WatchEntry) -> Result<Box<dyn Read + '_>, WatchError> {
            let context = ndk_context::android_context();
            let mut env = self
                .vm
                .attach_current_thread()
                .map_err(|err| WatchError::io(format!("attach: {err}")))?;
            // SAFETY: the context is the Activity, valid while the process lives.
            let context = unsafe { JObject::from_raw(context.context().cast()) };
            let tree = env
                .new_string(&self.tree)
                .map_err(|err| WatchError::io(format!("tree string: {err}")))?;
            let document = env
                .new_string(&entry.locator)
                .map_err(|err| WatchError::io(format!("document string: {err}")))?;
            let stream = env
                .call_static_method(
                    "app/suramak/transcribe/WatchTree",
                    "open",
                    "(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)Ljava/io/InputStream;",
                    &[
                        JValue::Object(&context),
                        JValue::Object(&tree),
                        JValue::Object(&document),
                    ],
                )
                .map_err(|err| WatchError::io(format!("WatchTree.open: {err}")))?;
            let stream = stream
                .l()
                .map_err(|err| WatchError::io(format!("WatchTree.open: {err}")))?;
            if stream.is_null() {
                return Err(WatchError::io(format!(
                    "the folder could not open {}",
                    entry.name
                )));
            }
            let stream = env
                .new_global_ref(stream)
                .map_err(|err| WatchError::io(format!("global stream: {err}")))?;
            Ok(Box::new(JavaStream {
                vm: self.vm.clone(),
                stream,
            }))
        }
    }

    fn parse_listing(text: &str) -> Result<Vec<WatchEntry>, WatchError> {
        let value: serde_json::Value = serde_json::from_str(text)
            .map_err(|err| WatchError::io(format!("WatchTree reply: {err}")))?;
        if !value
            .get("ok")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            let detail = value
                .get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("the tree could not be listed");
            return Err(WatchError::unreachable(detail.to_string()));
        }
        let mut entries = Vec::new();
        for entry in value
            .get("entries")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(name) = entry.get("name").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let Some(locator) = entry.get("id").and_then(serde_json::Value::as_str) else {
                continue;
            };
            entries.push(WatchEntry {
                locator: locator.to_string(),
                name: name.to_string(),
                size: entry
                    .get("size")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                mtime: entry
                    .get("mtime")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0),
                is_dir: entry
                    .get("dir")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            });
        }
        Ok(entries)
    }

    /// A Java `InputStream` as a Rust reader: each call attaches to the JVM,
    /// reads into a byte array and copies it out. Never blocks the UI thread —
    /// the scanner owns this thread.
    struct JavaStream {
        vm: Arc<JavaVM>,
        stream: GlobalRef,
    }

    impl Read for JavaStream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            let io = |err: String| std::io::Error::other(err);
            let mut env = self
                .vm
                .attach_current_thread()
                .map_err(|err| io(format!("attach: {err}")))?;
            let array = env
                .new_byte_array(buf.len().min(i32::MAX as usize) as i32)
                .map_err(|err| io(format!("byte array: {err}")))?;
            let read = env
                .call_method(
                    self.stream.as_obj(),
                    "read",
                    "([B)I",
                    &[JValue::Object(&array)],
                )
                .map_err(|err| io(format!("read: {err}")))?
                .i()
                .map_err(|err| io(format!("read: {err}")))?;
            if read <= 0 {
                return Ok(0);
            }
            let read = read as usize;
            let mut bytes = vec![0_i8; read];
            env.get_byte_array_region(&array, 0, &mut bytes)
                .map_err(|err| io(format!("read bytes: {err}")))?;
            for (target, byte) in buf.iter_mut().zip(bytes.iter()) {
                *target = *byte as u8;
            }
            Ok(read)
        }
    }
}

/// A small host-testable summary of the battery rule, re-exported so the shell
/// (and the report) name the same number.
pub const BACKGROUND_WINDOW_MS: u64 = suramak_core::watch::BACKGROUND_SCAN_WINDOW_MS;

/// The status row's folder name for a stored setting (never a raw URI).
pub fn folder_name(folder: &str) -> String {
    folder_display_name(folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_folder_name_of_a_tree_uri_is_readable() {
        assert_eq!(
            folder_name(
                "content://com.android.externalstorage.documents/tree/primary%3ARecordings"
            ),
            "Recordings"
        );
        assert_eq!(
            folder_name("content://com.android.externalstorage.documents/tree/home%3AInbox"),
            "Inbox"
        );
    }

    #[test]
    fn the_android_window_is_thirty_seconds() {
        // The report cites this number; it must be the cadence's own.
        assert_eq!(BACKGROUND_WINDOW_MS, 30_000);
    }

    #[test]
    fn a_host_without_settings_is_off_and_has_no_folder() {
        let dir = std::env::temp_dir().join(format!("suramak-watch-host-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let host = WatchHost::new(&dir);
        let (enabled, folder, view) = host.view();
        assert!(!enabled);
        assert!(folder.is_empty());
        assert!(view.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
