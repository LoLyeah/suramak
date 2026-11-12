//! M24A — the desktop side of the watched inbox.
//!
//! One thread per configured folder. Every 5 s it polls the folder through
//! [`suramak_core::watch`] (`FsWatchSource`), hashes newly settled audio, and
//! writes the `watched_files` ledger through **its own** SQLite connection
//! (WAL: the UI's connection sees the committed rows). It hands the main
//! thread the files that are genuinely new; those go through the ordinary
//! import path (`AppState::begin_imports`), so the queue, model choice and
//! language default are exactly a picked file's.
//!
//! The desktop is always foreground: the Android background rule lives in
//! `WatchCadence` and only the phone's host feeds it.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use suramak_core::paths;
use suramak_core::store::Store;
use suramak_core::watch::{
    folder_display_name, FsWatchSource, Inbox, InboxReport, RealClock, WATCH_POLL_INTERVAL_MS,
};

/// What the watcher tells the UI after one or more polls.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatchUpdate {
    /// The folder's display name (never a raw SAF URI).
    pub folder: String,
    /// Ledger rows after the polls (the status row's `N`).
    pub imports: u64,
    /// Files that are new and must be queued through the ordinary import path.
    pub imported: Vec<PathBuf>,
    /// The named failure, when the folder is unreachable or the ledger refused
    /// a write.
    pub failure: Option<String>,
}

enum Command {
    Stop,
}

enum Event {
    Poll(InboxReport),
    Failed(String),
}

/// A running watch thread. Dropping it stops the thread.
pub struct WatchJob {
    commands: Sender<Command>,
    events: Arc<Mutex<VecDeque<Event>>>,
    thread: Option<JoinHandle<()>>,
    folder: String,
}

impl WatchJob {
    /// Starts watching `folder`, recording into the library at `db_path`.
    ///
    /// A folder that does not exist yet is not an error at start: the first
    /// poll reports the named failure and keeps polling, so a folder on a
    /// card that mounts a moment later recovers by itself.
    pub fn spawn(folder: &str, db_path: PathBuf) -> Self {
        Self::spawn_with_interval(
            folder,
            db_path,
            Duration::from_millis(WATCH_POLL_INTERVAL_MS),
        )
    }

    /// The production spawn with a test-controlled interval (the settle gate
    /// is still the real 2 s, so a fast test waits for it rather than faking
    /// it away).
    fn spawn_with_interval(folder: &str, db_path: PathBuf, interval: Duration) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let events: Arc<Mutex<VecDeque<Event>>> = Arc::new(Mutex::new(VecDeque::new()));
        let thread_events = Arc::clone(&events);
        let thread_folder = folder.to_string();
        let thread = std::thread::Builder::new()
            .name("suramak-watch".to_string())
            .spawn(move || {
                run(
                    &thread_folder,
                    &db_path,
                    command_rx,
                    &thread_events,
                    interval,
                );
            })
            .ok();
        Self {
            commands: command_tx,
            events,
            thread,
            folder: folder.to_string(),
        }
    }

    pub fn folder(&self) -> &str {
        &self.folder
    }

    /// Drains the thread's events into one update. `None` when nothing new
    /// happened since the last call (the UI keeps its last status row).
    pub fn take_update(&mut self) -> Option<WatchUpdate> {
        let drained: Vec<Event> = {
            let Ok(mut events) = self.events.lock() else {
                return None;
            };
            events.drain(..).collect()
        };
        if drained.is_empty() {
            return None;
        }
        let mut update = WatchUpdate {
            folder: folder_display_name(&self.folder),
            ..WatchUpdate::default()
        };
        for event in drained {
            match event {
                Event::Poll(report) => {
                    update.imports = report.imports_total;
                    if let Some(failure) = report.scan.failure {
                        update.failure = Some(failure);
                    }
                    for candidate in report.imported {
                        update.imported.push(PathBuf::from(candidate.locator));
                    }
                }
                Event::Failed(reason) => {
                    update.failure = Some(reason);
                }
            }
        }
        Some(update)
    }

    /// Sends the stop command and joins the thread (it wakes immediately).
    pub fn stop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for WatchJob {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(
    folder: &str,
    db_path: &std::path::Path,
    commands: mpsc::Receiver<Command>,
    events: &Arc<Mutex<VecDeque<Event>>>,
    interval: Duration,
) {
    let push = |event: Event| {
        if let Ok(mut events) = events.lock() {
            events.push_back(event);
        }
    };
    let store = match Store::open(db_path) {
        Ok(store) => store,
        Err(err) => {
            // The watcher cannot record anything without the ledger. Say so
            // once and stop; the ordinary app still reports its own store error.
            push(Event::Failed(format!(
                "the watched inbox cannot read the library: {err}"
            )));
            return;
        }
    };
    let mut inbox = Inbox::new(Box::new(FsWatchSource), folder.to_string());
    let clock = RealClock::new();
    // One immediate poll: a folder that already holds files settles on the
    // second poll (5 s later) instead of waiting 10 s from launch.
    let poll_now = |inbox: &mut Inbox, store: &Store| match inbox.poll_with_clock(store, &clock) {
        Ok(report) => push(Event::Poll(report)),
        Err(err) => push(Event::Failed(format!(
            "the watched inbox could not record an import: {err}"
        ))),
    };
    poll_now(&mut inbox, &store);
    loop {
        match commands.recv_timeout(interval) {
            Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => poll_now(&mut inbox, &store),
        }
    }
}

/// Whether a settings value names a folder the desktop watcher should run.
pub fn configured(settings: &suramak_core::settings::Settings) -> Option<&str> {
    let watch = &settings.watch;
    if watch.is_configured() && !watch.folder.starts_with("content://") {
        Some(watch.folder.as_str())
    } else {
        None
    }
}

/// The library database the watcher records into.
pub fn db_path() -> Option<PathBuf> {
    paths::db_path().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// The thread end to end: a real folder, the real 2 s settle gate, the
    /// real ledger in a real database, and one import — then no second one.
    #[test]
    fn the_thread_settles_records_and_imports_once() {
        let dir = std::env::temp_dir().join(format!("suramak-watch-job-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let inbox = dir.join("Inbox");
        std::fs::create_dir_all(&inbox).unwrap();
        let file = inbox.join("take.m4a");
        std::fs::write(&file, vec![9_u8; 2_048]).unwrap();
        let db = dir.join("suramak.db");
        let mut job = WatchJob::spawn_with_interval(
            &inbox.to_string_lossy(),
            db.clone(),
            Duration::from_millis(200),
        );

        let locator = file.to_string_lossy().into_owned();
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut update = None;
        while Instant::now() < deadline {
            if let Some(candidate) = job.take_update() {
                if candidate.imports == 1 {
                    update = Some(candidate);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let update = update.expect("the watcher must import a settled file");
        assert_eq!(update.folder, "Inbox");
        assert!(update.failure.is_none(), "{:?}", update.failure);
        assert_eq!(update.imported, vec![file.clone()]);

        let store = Store::open(&db).expect("ledger");
        let row = store
            .watched_file(&locator)
            .expect("query")
            .expect("the import is recorded");
        assert_eq!(row.size, 2_048);
        assert_eq!(row.sha256.len(), 64);

        // A few more polls: the same file is never a second import.
        std::thread::sleep(Duration::from_millis(800));
        while let Some(later) = job.take_update() {
            assert!(later.imported.is_empty(), "a re-scan must import nothing");
            assert_eq!(later.imports, 1);
        }
        job.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
