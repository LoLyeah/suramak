//! M24A — the watched inbox: a folder in, transcripts out.
//!
//! One polling scanner, no OS notification crate (the roadmap's named cost
//! was `notify`; the owner's constraint is polling with **no new crates**).
//! Every [`WATCH_POLL_INTERVAL_MS`] it walks the chosen folder, bounded in
//! depth and entries, and hands the caller only files that are safe to import:
//!
//! * an **audio extension** ([`is_audio_name`]),
//! * no in-progress marker in the name and no `.part` / `.crdownload` / `.tmp`
//!   sibling ([`is_in_progress_name`], [`has_partial_sibling`]),
//! * **settled**: the size has not changed for [`WATCH_SETTLE_MS`] (the
//!   `is_settled` predicate is a parameter, defaulting to
//!   [`default_is_settled`]),
//! * content-hashed (sha256), so [`Inbox::poll`] can dedupe through the
//!   `watched_files` ledger: the same bytes copied under a new name import
//!   once.
//!
//! ## The seam
//!
//! [`WatchSource`] is the platform's "list a folder + open a file" pair. The
//! desktop uses [`FsWatchSource`]; Android's SAF traversal implements the same
//! trait in `suramak-android` (compiled by the NDK gate only); host tests use
//! [`FakeWatchSource`]. The scanner never touches a path itself, so every rule
//! above is unit-tested with a fake tree and a fake clock.
//!
//! ## Cadence
//!
//! [`WatchCadence`] is the Android battery rule: a 5 s poll must not keep a
//! phone awake. Scanning continues for [`BACKGROUND_SCAN_WINDOW_MS`] after the
//! app leaves the foreground and then pauses until it returns. Desktop is
//! always foreground.
//!
//! ## One import path
//!
//! The scanner imports nothing. [`Inbox::poll`] writes the ledger row and
//! returns the files that are new; the host hands those to its **existing**
//! import path (desktop `AppState::begin_imports`, Android `JobHost`), so the
//! queue's model choice, language default and dedupe are exactly the ones a
//! picked file gets.

use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::io::Read;
use std::path::Path;
use std::time::Instant;

use sha2::{Digest, Sha256};

use crate::store::{Store, WatchedFile};
use crate::SuramakError;

/// How often the watcher polls its folder (the workorder's 5 s).
pub const WATCH_POLL_INTERVAL_MS: u64 = 5_000;

/// A file's size must hold still this long before it is eligible.
pub const WATCH_SETTLE_MS: u64 = 2_000;

/// How deep the walk may go below the chosen folder (the folder is depth 0).
pub const WATCH_MAX_DEPTH: usize = 3;

/// How many filesystem entries one scan examines (the logged bound).
pub const WATCH_MAX_ENTRIES: usize = 200;

/// Scanning stops this long after the app enters the background on Android.
///
/// A 5 s poll must not keep a phone awake. 30 s is long enough for a quick
/// app switch (the user comes straight back) and short enough that a pocketed
/// phone stops touching the card within a minute.
pub const BACKGROUND_SCAN_WINDOW_MS: u64 = 30_000;

/// The one named failure the Library row shows when the folder is gone.
pub const FOLDER_UNREACHABLE: &str = "the folder is no longer reachable";

/// Extensions the inbox treats as audio (lower-case, without the dot).
///
/// Deliberately audio-only: a watched folder is a recorder / voice-note
/// dropzone, not a video library. Video containers stay on the explicit
/// import path.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "wav", "mp3", "m4a", "m4b", "aac", "flac", "ogg", "oga", "opus", "aiff", "aif", "wma", "amr",
    "spx", "caf", "au",
];

/// Suffixes that mean "this file (or its writer) is still working".
pub const PARTIAL_SUFFIXES: &[&str] = &["part", "crdownload", "tmp"];

/// Why a [`WatchSource`] could not list or open something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchError {
    pub kind: WatchErrorKind,
    pub detail: String,
}

/// The two failures the scanner distinguishes: the folder itself is gone
/// (the one the UI names), or one entry could not be read (skipped).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchErrorKind {
    /// The folder does not exist, is not a directory, or the grant is gone.
    Unreachable,
    /// A transient I/O failure on one entry.
    Io,
}

impl WatchError {
    pub fn unreachable(detail: impl Into<String>) -> Self {
        Self {
            kind: WatchErrorKind::Unreachable,
            detail: detail.into(),
        }
    }

    pub fn io(detail: impl Into<String>) -> Self {
        Self {
            kind: WatchErrorKind::Io,
            detail: detail.into(),
        }
    }

    fn from_io(err: std::io::Error, what: &str) -> Self {
        use std::io::ErrorKind;
        let detail = format!("{what}: {err}");
        match err.kind() {
            ErrorKind::NotFound | ErrorKind::PermissionDenied | ErrorKind::NotADirectory => {
                Self::unreachable(detail)
            }
            _ => Self::io(detail),
        }
    }
}

impl std::fmt::Display for WatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for WatchError {}

/// One directory entry as the platform reports it.
///
/// `locator` is opaque to the scanner: an absolute path on the desktop, a SAF
/// document locator on Android. `name` is the leaf's display name (what the
/// extension/sibling rules read). Sizes are bytes, mtimes milliseconds since
/// the Unix epoch (0 when the platform cannot say).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchEntry {
    pub locator: String,
    pub name: String,
    pub size: u64,
    pub mtime: i64,
    pub is_dir: bool,
}

impl WatchEntry {
    /// A file entry.
    pub fn file(
        locator: impl Into<String>,
        name: impl Into<String>,
        size: u64,
        mtime: i64,
    ) -> Self {
        Self {
            locator: locator.into(),
            name: name.into(),
            size,
            mtime,
            is_dir: false,
        }
    }

    /// A directory entry.
    pub fn dir(locator: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            locator: locator.into(),
            name: name.into(),
            size: 0,
            mtime: 0,
            is_dir: true,
        }
    }
}

/// "List a folder's entries + open one file" — the platform seam.
///
/// Implementations must be `Send` (the scanner lives on its own thread). The
/// real SAF binding attaches to the JVM inside `list`/`open`, so the value
/// itself carries only a `JavaVM` handle.
pub trait WatchSource: Send {
    /// Entries directly under `dir` (a locator previously returned by this
    /// source, or the root the caller configured).
    fn list(&self, dir: &str) -> Result<Vec<WatchEntry>, WatchError>;

    /// Opens one file entry's bytes. The scanner streams this through sha256;
    /// it never buffers a whole recording.
    fn open(&self, entry: &WatchEntry) -> Result<Box<dyn Read + '_>, WatchError>;
}

/// The desktop's source: ordinary `std::fs`.
#[derive(Debug, Default, Clone, Copy)]
pub struct FsWatchSource;

impl WatchSource for FsWatchSource {
    fn list(&self, dir: &str) -> Result<Vec<WatchEntry>, WatchError> {
        let read = std::fs::read_dir(dir).map_err(|err| WatchError::from_io(err, dir))?;
        let mut entries = Vec::new();
        for entry in read {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => return Err(WatchError::from_io(err, dir)),
            };
            // Non-UTF-8 names cannot be reopened through the string locator;
            // skip them honestly instead of importing a guessed path.
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let path = entry.path();
            let meta = match std::fs::metadata(&path) {
                Ok(meta) => meta,
                Err(err) => return Err(WatchError::from_io(err, dir)),
            };
            let locator = path.to_string_lossy().into_owned();
            let mtime = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|age| age.as_millis().min(i64::MAX as u128) as i64)
                .unwrap_or(0);
            entries.push(WatchEntry {
                locator,
                name,
                size: if meta.is_dir() { 0 } else { meta.len() },
                mtime,
                is_dir: meta.is_dir(),
            });
        }
        Ok(entries)
    }

    fn open(&self, entry: &WatchEntry) -> Result<Box<dyn Read + '_>, WatchError> {
        let file = std::fs::File::open(&entry.locator)
            .map_err(|err| WatchError::from_io(err, &entry.locator))?;
        Ok(Box::new(file))
    }
}

/// A fake tree for host tests: directories and file bytes in memory.
///
/// The scanner's rules do not care whether the bytes come from a card, a SAF
/// provider or this map, which is the point of the seam.
#[derive(Debug, Default, Clone)]
pub struct FakeWatchSource {
    dirs: BTreeMap<String, Vec<WatchEntry>>,
    files: BTreeMap<String, Vec<u8>>,
}

impl FakeWatchSource {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces one directory's listing (entries are sorted by the scanner,
    /// not here).
    pub fn set_dir(&mut self, locator: impl Into<String>, entries: Vec<WatchEntry>) {
        self.dirs.insert(locator.into(), entries);
    }

    /// Adds one file's bytes, keyed by its locator.
    pub fn set_bytes(&mut self, locator: impl Into<String>, bytes: impl Into<Vec<u8>>) {
        self.files.insert(locator.into(), bytes.into());
    }
}

impl WatchSource for FakeWatchSource {
    fn list(&self, dir: &str) -> Result<Vec<WatchEntry>, WatchError> {
        self.dirs
            .get(dir)
            .cloned()
            .ok_or_else(|| WatchError::unreachable(format!("no such folder: {dir}")))
    }

    fn open(&self, entry: &WatchEntry) -> Result<Box<dyn Read + '_>, WatchError> {
        self.files
            .get(&entry.locator)
            .map(|bytes| Box::new(std::io::Cursor::new(bytes.clone())) as Box<dyn Read>)
            .ok_or_else(|| WatchError::io(format!("no bytes for {}", entry.locator)))
    }
}

/// The scanner's clock seam. Milliseconds are monotonic and arbitrary-origin;
/// only differences matter.
pub trait WatchClock {
    fn now_ms(&self) -> u64;
}

/// The production clock (`Instant`-based, so a wall-clock change cannot make
/// a half-written file look settled).
#[derive(Debug, Clone, Copy)]
pub struct RealClock {
    base: Instant,
}

impl RealClock {
    pub fn new() -> Self {
        Self {
            base: Instant::now(),
        }
    }
}

impl Default for RealClock {
    fn default() -> Self {
        Self::new()
    }
}

impl WatchClock for RealClock {
    fn now_ms(&self) -> u64 {
        self.base.elapsed().as_millis().min(u64::MAX as u128) as u64
    }
}

/// A clock tests advance by hand.
#[derive(Debug, Default)]
pub struct FakeClock {
    now: std::sync::atomic::AtomicU64,
}

impl FakeClock {
    pub fn new(start_ms: u64) -> Self {
        Self {
            now: std::sync::atomic::AtomicU64::new(start_ms),
        }
    }

    pub fn set(&self, now_ms: u64) {
        self.now.store(now_ms, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn advance(&self, by_ms: u64) {
        self.now
            .fetch_add(by_ms, std::sync::atomic::Ordering::SeqCst);
    }
}

impl WatchClock for FakeClock {
    fn now_ms(&self) -> u64 {
        self.now.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// What the scanner remembers about one file between scans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    /// The size seen on the scan that started the current stable run.
    pub size: u64,
    /// When that size was first seen (the settle clock's origin).
    pub first_seen_ms: u64,
    /// The last time the file was seen at all.
    pub last_seen_ms: u64,
    /// The platform's mtime (informational; the ledger stores it).
    pub mtime: i64,
}

/// The default settle gate: same size across two scans at least
/// [`WATCH_SETTLE_MS`] apart. `previous` is the observation from the last
/// scan (the caller has not yet stored this scan's).
pub fn default_is_settled(entry: &WatchEntry, previous: Option<&Observation>, now_ms: u64) -> bool {
    previous.is_some_and(|previous| {
        previous.size == entry.size
            && now_ms.saturating_sub(previous.first_seen_ms) >= WATCH_SETTLE_MS
    })
}

/// The injected settle rule.
pub type SettlePredicate = Box<dyn Fn(&WatchEntry, Option<&Observation>, u64) -> bool + Send>;

/// One eligible file: settled, audio, not being written, hashed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchCandidate {
    /// The platform handle the host imports (a path on desktop, a SAF
    /// locator on Android).
    pub locator: String,
    pub name: String,
    pub size: u64,
    pub mtime: i64,
    /// Lower-case hex sha256 of the bytes at the time of the scan.
    pub sha256: String,
}

/// One scan's numbers — the report's poll/IO table is built from these.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanReport {
    /// Filesystem entries examined (bounded by [`WATCH_MAX_ENTRIES`]).
    pub examined: usize,
    /// Directories listed.
    pub dirs_listed: usize,
    /// File entries among `examined`.
    pub files_seen: usize,
    /// Audio-named files that passed the name rules.
    pub audio_seen: usize,
    /// Audio files skipped because a writer marker names them.
    pub blocked_in_progress: usize,
    /// Audio files skipped because a `.part`/`.crdownload`/`.tmp` sibling exists.
    pub blocked_partial: usize,
    /// Audio files seen but not yet settled.
    pub unsettled: usize,
    /// Newly settled files hashed this scan.
    pub hashed: usize,
    /// Bytes read while hashing (the scan's I/O cost).
    pub bytes_hashed: u64,
    /// Per-entry I/O failures (the scan continues).
    pub io_errors: usize,
    /// The scan hit [`WATCH_MAX_ENTRIES`]; the rest waits for the next poll.
    pub truncated: bool,
    /// The named failure when the folder itself is unreachable.
    pub failure: Option<String>,
    /// The platform's detail for `failure` (logs only; never the UI).
    pub failure_detail: Option<String>,
    /// Files that became eligible on this scan.
    pub candidates: Vec<WatchCandidate>,
}

/// The bounded polling scanner.
pub struct Scanner {
    source: Box<dyn WatchSource>,
    root: String,
    observations: BTreeMap<String, Observation>,
    /// Settled candidates handed out but not yet acknowledged. They are not
    /// emitted again, so a scan cannot import the same file twice.
    emitted: BTreeSet<String>,
    settle: SettlePredicate,
    /// The last scan's bound, kept for the status line's truthful count.
    last_examined: usize,
}

impl Scanner {
    /// A scanner over `source` rooted at `root`, with the default 2 s gate.
    pub fn new(source: Box<dyn WatchSource>, root: impl Into<String>) -> Self {
        Self::with_settle(source, root, Box::new(default_is_settled))
    }

    /// The test seam: a scanner with an injected settle rule.
    pub fn with_settle(
        source: Box<dyn WatchSource>,
        root: impl Into<String>,
        settle: SettlePredicate,
    ) -> Self {
        Self {
            source,
            root: root.into(),
            observations: BTreeMap::new(),
            emitted: BTreeSet::new(),
            settle,
            last_examined: 0,
        }
    }

    pub fn root(&self) -> &str {
        &self.root
    }

    /// The number of entries the last scan examined.
    pub fn last_examined(&self) -> usize {
        self.last_examined
    }

    /// Forgets every observation and emitted marker (the folder changed).
    pub fn reset(&mut self) {
        self.observations.clear();
        self.emitted.clear();
        self.last_examined = 0;
    }

    /// Confirms a candidate was handled. It stays suppressed for the life of
    /// this scanner (a re-scan or a rename of the same path must not import
    /// the file twice); only a new process — or a folder change — starts over.
    pub fn acknowledge(&mut self, locator: &str) {
        self.emitted.insert(locator.to_string());
    }

    /// Opens a candidate's bytes (Android copies them into its media inbox;
    /// desktop re-reads the path through its ordinary import).
    pub fn open(&self, candidate: &WatchCandidate) -> Result<Box<dyn Read + '_>, WatchError> {
        self.source.open(&WatchEntry {
            locator: candidate.locator.clone(),
            name: candidate.name.clone(),
            size: candidate.size,
            mtime: candidate.mtime,
            is_dir: false,
        })
    }

    /// Walks the tree once, with the clock **injected**: the scanner never
    /// reads a clock itself, so a fake clock makes every settle decision
    /// deterministic.
    pub fn scan_with_clock(&mut self, clock: &dyn WatchClock) -> ScanReport {
        self.scan(clock.now_ms())
    }

    /// Walks the tree once at `now_ms` (the clock's value, passed in). Pure
    /// apart from the source's I/O and hashing.
    pub fn scan(&mut self, now_ms: u64) -> ScanReport {
        let mut report = ScanReport::default();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        queue.push_back((self.root.clone(), 0));

        'walk: while let Some((dir, depth)) = queue.pop_front() {
            if report.examined >= WATCH_MAX_ENTRIES {
                report.truncated = true;
                break;
            }
            let entries = match self.source.list(&dir) {
                Ok(entries) => entries,
                Err(err) => {
                    if depth == 0 {
                        // The chosen folder itself is gone: the one failure the
                        // Library row names. Polling continues, so a folder
                        // that comes back (a remounted card) recovers.
                        report.failure = Some(FOLDER_UNREACHABLE.to_string());
                        report.failure_detail = Some(err.detail);
                    } else {
                        report.io_errors += 1;
                    }
                    continue;
                }
            };
            report.dirs_listed += 1;
            let mut entries = entries;
            entries.sort_by(|a, b| a.name.cmp(&b.name));
            // Lower-cased sibling names, for the `.part` rule (a writer may
            // upper-case its marker).
            let names: HashSet<String> = entries.iter().map(|e| e.name.to_lowercase()).collect();

            for entry in &entries {
                if report.examined >= WATCH_MAX_ENTRIES {
                    report.truncated = true;
                    break 'walk;
                }
                report.examined += 1;
                seen.insert(entry.locator.clone());
                if entry.is_dir {
                    // A directory named like a writer's temp is not descended
                    // into; the depth bound is the other half of the walk rule.
                    if depth < WATCH_MAX_DEPTH && !is_in_progress_name(&entry.name) {
                        queue.push_back((entry.locator.clone(), depth + 1));
                    }
                    continue;
                }
                report.files_seen += 1;
                if !is_audio_name(&entry.name) {
                    continue;
                }
                report.audio_seen += 1;
                if is_in_progress_name(&entry.name) {
                    report.blocked_in_progress += 1;
                    continue;
                }
                if has_partial_sibling(&entry.name, &names) {
                    report.blocked_partial += 1;
                    continue;
                }

                let previous = self.observations.get(&entry.locator).cloned();
                let settled = (self.settle)(entry, previous.as_ref(), now_ms);
                let first_seen_ms = match &previous {
                    Some(previous) if previous.size == entry.size => previous.first_seen_ms,
                    _ => now_ms,
                };
                self.observations.insert(
                    entry.locator.clone(),
                    Observation {
                        size: entry.size,
                        first_seen_ms,
                        last_seen_ms: now_ms,
                        mtime: entry.mtime,
                    },
                );
                if !settled {
                    report.unsettled += 1;
                    continue;
                }
                if self.emitted.contains(&entry.locator) {
                    continue;
                }
                match hash_entry(self.source.as_ref(), entry) {
                    Ok((sha256, bytes)) => {
                        report.hashed += 1;
                        report.bytes_hashed += bytes;
                        report.candidates.push(WatchCandidate {
                            locator: entry.locator.clone(),
                            name: entry.name.clone(),
                            size: entry.size,
                            mtime: entry.mtime,
                            sha256,
                        });
                        self.emitted.insert(entry.locator.clone());
                    }
                    // A file that vanished (or was truncated) between listing
                    // and open is a skip, not a scan failure: the next poll
                    // sees it again if it is real.
                    Err(_) => report.io_errors += 1,
                }
            }
        }

        // Bound the memory: files that left the folder are forgotten.
        self.observations
            .retain(|locator, _| seen.contains(locator));
        self.emitted.retain(|locator| seen.contains(locator));
        self.last_examined = report.examined;
        report
    }
}

/// Streams one entry through sha256, counting the bytes read.
fn hash_entry(source: &dyn WatchSource, entry: &WatchEntry) -> Result<(String, u64), WatchError> {
    let mut reader = source.open(entry)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|err| WatchError::io(format!("read {}: {err}", entry.locator)))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        total += read as u64;
    }
    Ok((format!("{:x}", hasher.finalize()), total))
}

/// Whether `name` has an audio extension (case-insensitive).
pub fn is_audio_name(name: &str) -> bool {
    let Some(extension) = name.rsplit_once('.').map(|(_, extension)| extension) else {
        return false;
    };
    let extension = extension.to_ascii_lowercase();
    AUDIO_EXTENSIONS.iter().any(|known| *known == extension)
}

/// Whether `name` carries a writer's in-progress marker: ends with
/// `.part`/`.crdownload`/`.tmp`, or has one in the middle
/// (`note.part.m4a`).
pub fn is_in_progress_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    PARTIAL_SUFFIXES.iter().any(|suffix| {
        lower.ends_with(&format!(".{suffix}")) || lower.contains(&format!(".{suffix}."))
    })
}

/// Whether a same-directory sibling says `name` is still being written.
///
/// `siblings` holds the directory's lower-cased entry names. A `.part` /
/// `.crdownload` / `.tmp` sibling is the writer's own "not finished" marker;
/// importing the target before that marker is gone is the half-file bug the
/// workorder forbids.
pub fn has_partial_sibling(name: &str, siblings: &HashSet<String>) -> bool {
    let lower = name.to_lowercase();
    PARTIAL_SUFFIXES
        .iter()
        .any(|suffix| siblings.contains(&format!("{lower}.{suffix}")))
}

/// One poll of the inbox: the scan, the ledger writes, and the new imports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InboxReport {
    pub scan: ScanReport,
    /// Files that were not in the ledger and are now recorded as imports.
    pub imported: Vec<WatchCandidate>,
    /// Files whose bytes were already imported under another name (sha256).
    pub duplicates: usize,
    /// Ledger rows after this poll (the status row's `N`).
    pub imports_total: u64,
}

/// The scanner + the `watched_files` ledger. This is what a host drives.
pub struct Inbox {
    scanner: Scanner,
}

impl Inbox {
    pub fn new(source: Box<dyn WatchSource>, folder: impl Into<String>) -> Self {
        Self {
            scanner: Scanner::new(source, folder),
        }
    }

    /// The test seam, forwarded to [`Scanner::with_settle`].
    pub fn with_settle(
        source: Box<dyn WatchSource>,
        folder: impl Into<String>,
        settle: SettlePredicate,
    ) -> Self {
        Self {
            scanner: Scanner::with_settle(source, folder, settle),
        }
    }

    pub fn folder(&self) -> &str {
        self.scanner.root()
    }

    pub fn scanner(&self) -> &Scanner {
        &self.scanner
    }

    /// Forgets observations/emissions (the folder changed).
    pub fn reset(&mut self) {
        self.scanner.reset();
    }

    /// One poll: scan, then dedupe each settled file against the ledger by
    /// path (already imported) and by sha256 (same bytes, another name).
    /// Recording happens only for a genuinely new file; the caller queues the
    /// returned candidates through its ordinary import path.
    pub fn poll_with_clock(
        &mut self,
        store: &Store,
        clock: &dyn WatchClock,
    ) -> Result<InboxReport, SuramakError> {
        self.poll(store, clock.now_ms())
    }

    /// One poll at `now_ms` (the clock's value, passed in by the host).
    pub fn poll(&mut self, store: &Store, now_ms: u64) -> Result<InboxReport, SuramakError> {
        let scan = self.scanner.scan(now_ms);
        let mut imported = Vec::new();
        let mut duplicates = 0;
        for candidate in &scan.candidates {
            // Path identity first: a file already in the ledger at the same
            // size and mtime is the same file, and must not be re-hashed on
            // every poll.
            if store
                .watched_file(&candidate.locator)?
                .is_some_and(|row| row.size == candidate.size && row.mtime == candidate.mtime)
            {
                self.scanner.acknowledge(&candidate.locator);
                continue;
            }
            if store.watched_file_by_sha(&candidate.sha256)?.is_some() {
                // The same bytes under a new name: acknowledge it so this
                // session never re-hashes it, but write no ledger row — the
                // import already exists and `N` stays truthful.
                duplicates += 1;
                self.scanner.acknowledge(&candidate.locator);
                continue;
            }
            store.record_watched_file(&WatchedFile {
                path: candidate.locator.clone(),
                size: candidate.size,
                mtime: candidate.mtime,
                sha256: candidate.sha256.clone(),
                imported_at: chrono::Utc::now().to_rfc3339(),
                transcript_id: None,
            })?;
            self.scanner.acknowledge(&candidate.locator);
            imported.push(candidate.clone());
        }
        let imports_total = u64::try_from(store.watched_import_count()?).unwrap_or(0);
        Ok(InboxReport {
            scan,
            imported,
            duplicates,
            imports_total,
        })
    }

    /// Opens a candidate's bytes (Android copies them into its media inbox;
    /// desktop imports the locator as a path).
    pub fn open(&self, candidate: &WatchCandidate) -> Result<Box<dyn Read + '_>, WatchError> {
        self.scanner.open(candidate)
    }
}

/// The Android background rule: scan while foreground; after leaving the
/// foreground keep scanning for [`BACKGROUND_SCAN_WINDOW_MS`], then pause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchCadence {
    foreground: bool,
    background_since_ms: Option<u64>,
}

impl Default for WatchCadence {
    fn default() -> Self {
        Self {
            foreground: true,
            background_since_ms: None,
        }
    }
}

impl WatchCadence {
    pub fn set_foreground(&mut self, foreground: bool, now_ms: u64) {
        if foreground {
            self.foreground = true;
            self.background_since_ms = None;
        } else if self.foreground {
            self.foreground = false;
            self.background_since_ms = Some(now_ms);
        }
    }

    /// Whether a poll should run now. Desktop never calls this: it is always
    /// foreground, and the desktop host passes `true` as the cadence.
    pub fn should_scan(&self, now_ms: u64) -> bool {
        if self.foreground {
            return true;
        }
        match self.background_since_ms {
            Some(since) => now_ms.saturating_sub(since) < BACKGROUND_SCAN_WINDOW_MS,
            None => false,
        }
    }

    pub fn is_foreground(&self) -> bool {
        self.foreground
    }
}

/// Whether a poll is due under `cadence` (a convenience for hosts that do not
/// keep their own timer).
pub fn scan_due(cadence: &WatchCadence, now_ms: u64) -> bool {
    cadence.should_scan(now_ms)
}

/// The folder's display name from a path or SAF tree URI — re-exported here so
/// a host holding only the locator does not need the settings type.
pub fn folder_display_name(folder: &str) -> String {
    crate::settings::WatchSettings {
        enabled: true,
        folder: folder.to_string(),
    }
    .folder_name()
}

/// The audio extension for a file name, when it is importable (`None` for a
/// non-audio name). The Android host uses it to pick the media copy's suffix.
pub fn audio_extension(name: &str) -> Option<&str> {
    let (_, extension) = name.rsplit_once('.')?;
    if AUDIO_EXTENSIONS
        .iter()
        .any(|known| known.eq_ignore_ascii_case(extension))
    {
        Some(extension)
    } else {
        None
    }
}

/// The default local path for a candidate locator (desktop hosts).
pub fn candidate_path(candidate: &WatchCandidate) -> &Path {
    Path::new(&candidate.locator)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(dir: &str, name: &str, size: u64) -> WatchEntry {
        WatchEntry::file(format!("{dir}/{name}"), name, size, 1_700_000_000_000)
    }

    /// One root with the given files, and their bytes.
    fn tree(files: &[(&str, &[u8])]) -> FakeWatchSource {
        let mut source = FakeWatchSource::new();
        let entries: Vec<WatchEntry> = files
            .iter()
            .map(|(name, bytes)| file("/inbox", name, bytes.len() as u64))
            .collect();
        for (name, bytes) in files {
            source.set_bytes(format!("/inbox/{name}"), *bytes);
        }
        source.set_dir("/inbox", entries);
        source
    }

    fn store() -> Store {
        Store::open_in_memory().expect("in-memory store")
    }

    // ---- name rules ---------------------------------------------------------

    #[test]
    fn only_audio_names_are_eligible() {
        for name in ["a.wav", "A.M4A", "note.opus", "x.flac", "y.aiff"] {
            assert!(is_audio_name(name), "{name} should be audio");
        }
        for name in ["a.mp4", "a.txt", "a", "a.wav.part", "a.m4a.tmp", "m4a"] {
            assert!(!is_audio_name(name), "{name} should not be audio");
        }
        assert_eq!(audio_extension("A.M4A"), Some("M4A"));
        assert_eq!(audio_extension("a.mp4"), None);
    }

    #[test]
    fn in_progress_markers_block_a_name() {
        for name in [
            "a.m4a.part",
            "a.crdownload",
            "a.tmp",
            "a.part.m4a",
            "a.CRDOWNLOAD.m4a",
        ] {
            assert!(is_in_progress_name(name), "{name} must be blocked");
        }
        assert!(!is_in_progress_name("a.m4a"));
        assert!(!is_in_progress_name("part-of-the-story.m4a"));
        assert!(!is_in_progress_name("tmp.m4a"));
    }

    #[test]
    fn a_partial_sibling_blocks_the_target_until_it_is_gone() {
        let mut siblings: HashSet<String> = ["a.m4a".to_string(), "a.m4a.part".to_string()]
            .into_iter()
            .collect();
        assert!(has_partial_sibling("a.m4a", &siblings));
        siblings.remove("a.m4a.part");
        assert!(!has_partial_sibling("a.m4a", &siblings));
        // `.crdownload` and `.tmp` are the same rule.
        for suffix in ["crdownload", "tmp"] {
            let siblings = [format!("a.m4a.{suffix}")].into_iter().collect();
            assert!(has_partial_sibling("a.m4a", &siblings));
        }
    }

    // ---- the settle gate ----------------------------------------------------

    #[test]
    fn the_settle_gate_rejects_a_growing_file_and_accepts_it_once_stable() {
        let mut source = tree(&[("take.m4a", &[0_u8; 10])]);
        source.set_dir("/inbox", vec![file("/inbox", "take.m4a", 10)]);
        let mut scanner = Scanner::new(Box::new(source), "/inbox");

        // First scan: seen, not settled (no previous observation).
        let first = scanner.scan(0);
        assert_eq!(first.examined, 1);
        assert!(
            first.candidates.is_empty(),
            "nothing is eligible on first sight"
        );
        assert_eq!(first.unsettled, 1);

        // The writer is still growing the file: at +1 s it is 20 bytes, so the
        // stable run restarts and nothing is eligible.
        let mut growing = FakeWatchSource::new();
        growing.set_dir("/inbox", vec![file("/inbox", "take.m4a", 20)]);
        growing.set_bytes("/inbox/take.m4a", [0_u8; 20]);
        let mut scanner = Scanner::new(Box::new(growing), "/inbox");
        scanner.scan(0);
        let at_one = scanner.scan(1_000);
        assert!(
            at_one.candidates.is_empty(),
            "a size change restarts the settle clock"
        );
        assert_eq!(at_one.unsettled, 1);

        // Still 20 bytes at +2.1 s: the size has now held for the whole 2 s.
        let stable = scanner.scan(2_100);
        assert_eq!(stable.candidates.len(), 1);
        assert_eq!(stable.candidates[0].name, "take.m4a");
        assert_eq!(stable.candidates[0].size, 20);

        // An acknowledged file is not re-emitted on the next poll.
        scanner.acknowledge(&stable.candidates[0].locator);
        let next = scanner.scan(7_100);
        assert!(
            next.candidates.is_empty(),
            "an acknowledged file is not re-emitted"
        );
    }

    #[test]
    fn the_clock_is_injected_and_never_read_inside_the_scanner() {
        let mut source = FakeWatchSource::new();
        source.set_dir("/inbox", vec![file("/inbox", "take.m4a", 3)]);
        source.set_bytes("/inbox/take.m4a", b"abc");
        let mut scanner = Scanner::new(Box::new(source), "/inbox");
        let clock = FakeClock::new(1_000);

        // First sight at the fake clock's 1 000 ms: not settled.
        assert!(scanner.scan_with_clock(&clock).candidates.is_empty());
        // The fake clock advances past the 2 s gate: settled without any
        // wall-clock wait.
        clock.set(3_000);
        assert_eq!(scanner.scan_with_clock(&clock).candidates.len(), 1);
    }

    #[test]
    fn the_settle_gate_can_be_injected_for_deterministic_tests() {
        let mut source = tree(&[("take.m4a", b"abc")]);
        source.set_dir("/inbox", vec![file("/inbox", "take.m4a", 3)]);
        // The seam: a predicate that says yes immediately. No 2 s wait.
        let mut scanner =
            Scanner::with_settle(Box::new(source), "/inbox", Box::new(|_, _, _| true));
        let report = scanner.scan(0);
        assert_eq!(report.candidates.len(), 1);
        assert_eq!(report.candidates[0].sha256.len(), 64);
        assert!(report.bytes_hashed > 0);
    }

    // ---- blocking -----------------------------------------------------------

    #[test]
    fn a_part_sibling_blocks_import_until_the_writer_finishes() {
        let mut source = FakeWatchSource::new();
        source.set_dir(
            "/inbox",
            vec![
                file("/inbox", "interview.m4a", 5),
                file("/inbox", "interview.m4a.part", 2),
            ],
        );
        source.set_bytes("/inbox/interview.m4a", b"hello");
        source.set_bytes("/inbox/interview.m4a.part", b"he");
        let mut scanner =
            Scanner::with_settle(Box::new(source), "/inbox", Box::new(|_, _, _| true));

        let blocked = scanner.scan(0);
        assert!(blocked.candidates.is_empty());
        assert_eq!(blocked.blocked_partial, 1);
        assert_eq!(blocked.audio_seen, 1, "the .part name is not audio");

        // The writer renames the sibling away: the target is eligible now.
        let mut source = FakeWatchSource::new();
        source.set_dir("/inbox", vec![file("/inbox", "interview.m4a", 5)]);
        source.set_bytes("/inbox/interview.m4a", b"hello");
        let mut scanner =
            Scanner::with_settle(Box::new(source), "/inbox", Box::new(|_, _, _| true));
        let imported = scanner.scan(0);
        assert_eq!(imported.candidates.len(), 1);
    }

    #[test]
    fn a_marked_name_is_never_eligible() {
        let mut source = FakeWatchSource::new();
        source.set_dir(
            "/inbox",
            vec![
                file("/inbox", "note.part.m4a", 4),
                file("/inbox", "note.m4a.crdownload", 4),
            ],
        );
        source.set_bytes("/inbox/note.part.m4a", b"data");
        source.set_bytes("/inbox/note.m4a.crdownload", b"data");
        let mut scanner =
            Scanner::with_settle(Box::new(source), "/inbox", Box::new(|_, _, _| true));
        let report = scanner.scan(0);
        assert!(report.candidates.is_empty());
        assert_eq!(report.blocked_in_progress, 1);
    }

    // ---- the 200-entry bound ------------------------------------------------

    #[test]
    fn the_scan_is_bounded_at_two_hundred_entries_and_logs_the_truncation() {
        let mut entries = Vec::new();
        let mut source = FakeWatchSource::new();
        for index in 0..201 {
            let name = format!("clip-{index:03}.wav");
            entries.push(file("/inbox", &name, 1));
            source.set_bytes(format!("/inbox/{name}"), b"x");
        }
        source.set_dir("/inbox", entries);
        let mut scanner =
            Scanner::with_settle(Box::new(source), "/inbox", Box::new(|_, _, _| true));
        let report = scanner.scan(0);
        assert_eq!(
            report.examined, WATCH_MAX_ENTRIES,
            "201 entries → 200 examined"
        );
        assert!(report.truncated, "the bound must be visible, never silent");
        assert_eq!(report.candidates.len(), WATCH_MAX_ENTRIES);
        assert_eq!(scanner.last_examined(), WATCH_MAX_ENTRIES);
    }

    #[test]
    fn the_walk_stops_at_depth_three() {
        let mut source = FakeWatchSource::new();
        source.set_dir(
            "/inbox",
            vec![
                WatchEntry::dir("/inbox/one", "one"),
                file("/inbox", "root.wav", 1),
            ],
        );
        source.set_dir(
            "/inbox/one",
            vec![
                WatchEntry::dir("/inbox/one/two", "two"),
                file("/inbox/one", "one.wav", 1),
            ],
        );
        source.set_dir(
            "/inbox/one/two",
            vec![
                WatchEntry::dir("/inbox/one/two/three", "three"),
                file("/inbox/one/two", "two.wav", 1),
            ],
        );
        source.set_dir(
            "/inbox/one/two/three",
            vec![
                WatchEntry::dir("/inbox/one/two/three/four", "four"),
                file("/inbox/one/two/three", "three.wav", 1),
            ],
        );
        // Depth 4 is never listed.
        source.set_dir(
            "/inbox/one/two/three/four",
            vec![file("/inbox/one/two/three/four", "four.wav", 1)],
        );
        for path in [
            "/inbox/root.wav",
            "/inbox/one/one.wav",
            "/inbox/one/two/two.wav",
            "/inbox/one/two/three/three.wav",
            "/inbox/one/two/three/four/four.wav",
        ] {
            source.set_bytes(path, b"x");
        }
        let mut scanner =
            Scanner::with_settle(Box::new(source), "/inbox", Box::new(|_, _, _| true));
        let report = scanner.scan(0);
        let names: Vec<&str> = report
            .candidates
            .iter()
            .map(|candidate| candidate.name.as_str())
            .collect();
        assert_eq!(names, vec!["root.wav", "one.wav", "two.wav", "three.wav"]);
        assert!(
            !names.contains(&"four.wav"),
            "depth 4 must never be reached"
        );
    }

    // ---- the named missing-folder failure -----------------------------------

    #[test]
    fn a_deleted_folder_surfaces_the_named_failure_and_recovers() {
        let mut source = tree(&[("take.m4a", b"abc")]);
        source.set_dir("/inbox", vec![file("/inbox", "take.m4a", 3)]);
        let mut scanner =
            Scanner::with_settle(Box::new(source), "/inbox", Box::new(|_, _, _| true));
        assert!(scanner.scan(0).failure.is_none());

        // The folder is gone (the fake has no listing for it).
        let gone = FakeWatchSource::new();
        let mut scanner = Scanner::with_settle(Box::new(gone), "/inbox", Box::new(|_, _, _| true));
        let report = scanner.scan(0);
        assert_eq!(report.failure.as_deref(), Some(FOLDER_UNREACHABLE));
        assert_eq!(report.examined, 0);

        // It comes back (a remounted card): the next poll works again.
        let mut back = FakeWatchSource::new();
        back.set_dir("/inbox", vec![file("/inbox", "later.m4a", 2)]);
        back.set_bytes("/inbox/later.m4a", b"ok");
        let mut scanner = Scanner::with_settle(Box::new(back), "/inbox", Box::new(|_, _, _| true));
        let report = scanner.scan(0);
        assert!(report.failure.is_none());
        assert_eq!(report.candidates.len(), 1);
    }

    #[test]
    fn a_real_missing_folder_is_unreachable_too() {
        let dir =
            std::env::temp_dir().join(format!("suramak-watch-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut scanner = Scanner::new(Box::new(FsWatchSource), dir.to_string_lossy().into_owned());
        let report = scanner.scan(0);
        assert_eq!(report.failure.as_deref(), Some(FOLDER_UNREACHABLE));
    }

    // ---- the ledger: sha256 dedupe ------------------------------------------

    #[test]
    fn the_same_bytes_under_a_new_name_import_once() {
        let bytes = b"the same recording bytes";
        let mut source = FakeWatchSource::new();
        source.set_dir(
            "/inbox",
            vec![
                file("/inbox", "first.m4a", bytes.len() as u64),
                file("/inbox", "copy.m4a", bytes.len() as u64),
            ],
        );
        source.set_bytes("/inbox/first.m4a", bytes);
        source.set_bytes("/inbox/copy.m4a", bytes);
        let store = store();
        let mut inbox = Inbox::with_settle(Box::new(source), "/inbox", Box::new(|_, _, _| true));

        let first = inbox.poll(&store, 0).expect("poll");
        assert_eq!(
            first.imported.len() + first.duplicates,
            2,
            "both files are examined"
        );
        assert_eq!(first.duplicates, 1, "one of the two names is the duplicate");
        assert_eq!(first.imports_total, 1, "one content, one import");
        // Names sort before the ledger write, so `copy.m4a` is recorded first;
        // either way exactly one row exists and it carries the shared hash.
        let recorded_path = if store.watched_file("/inbox/first.m4a").unwrap().is_some() {
            "/inbox/first.m4a"
        } else {
            "/inbox/copy.m4a"
        };
        let recorded = store.watched_file(recorded_path).unwrap().unwrap();
        assert_eq!(recorded.sha256.len(), 64);
        assert_eq!(recorded.transcript_id, None);
        assert_eq!(store.watched_import_count().unwrap(), 1);

        // A second poll imports nothing (both are acknowledged).
        let second = inbox.poll(&store, 5_000).expect("poll");
        assert!(second.imported.is_empty());
        assert_eq!(second.scan.examined, 2);
        assert_eq!(second.imports_total, 1);
    }

    #[test]
    fn a_known_path_is_skipped_and_a_known_sha_under_a_new_name_is_a_duplicate() {
        let store = store();
        let sha = {
            let mut hasher = Sha256::new();
            hasher.update(b"aaaaa");
            format!("{:x}", hasher.finalize())
        };
        store
            .record_watched_file(&WatchedFile {
                path: "/inbox/old.m4a".to_string(),
                size: 5,
                mtime: 1_700_000_000_000,
                sha256: sha,
                imported_at: "2026-11-12T10:00:00+00:00".to_string(),
                transcript_id: None,
            })
            .unwrap();

        let mut source = FakeWatchSource::new();
        source.set_dir(
            "/inbox",
            vec![
                file("/inbox", "old.m4a", 5),
                file("/inbox", "renamed.m4a", 5),
                file("/inbox", "fresh.m4a", 3),
            ],
        );
        source.set_bytes("/inbox/old.m4a", b"aaaaa");
        source.set_bytes("/inbox/renamed.m4a", b"aaaaa");
        source.set_bytes("/inbox/fresh.m4a", b"bbb");
        let mut inbox = Inbox::with_settle(Box::new(source), "/inbox", Box::new(|_, _, _| true));

        let report = inbox.poll(&store, 0).expect("poll");
        assert_eq!(report.duplicates, 1, "renamed.m4a is the same bytes");
        assert_eq!(report.imported.len(), 1, "only fresh.m4a is new");
        assert_eq!(report.imported[0].name, "fresh.m4a");
        assert_eq!(report.imports_total, 2);
    }

    #[test]
    fn a_rescan_of_the_same_folder_imports_nothing() {
        let store = store();
        let mut source = FakeWatchSource::new();
        source.set_dir("/inbox", vec![file("/inbox", "one.wav", 4)]);
        source.set_bytes("/inbox/one.wav", b"data");
        let mut inbox = Inbox::with_settle(Box::new(source), "/inbox", Box::new(|_, _, _| true));
        assert_eq!(inbox.poll(&store, 0).unwrap().imported.len(), 1);
        for tick in 1..4 {
            let report = inbox.poll(&store, tick * 5_000).unwrap();
            assert!(
                report.imported.is_empty(),
                "poll {tick} must import nothing"
            );
            assert_eq!(report.imports_total, 1);
        }
    }

    // ---- cadence ------------------------------------------------------------

    #[test]
    fn scanning_pauses_in_the_background_after_the_bounded_window() {
        let mut cadence = WatchCadence::default();
        assert!(cadence.should_scan(0), "desktop/foreground always scans");
        cadence.set_foreground(false, 1_000);
        assert!(
            cadence.should_scan(1_001),
            "a quick app switch keeps scanning"
        );
        assert!(cadence.should_scan(30_999));
        assert!(
            !cadence.should_scan(31_000),
            "30 s after backgrounding the poll must stop"
        );
        assert!(!cadence.should_scan(120_000));
        // Returning to the foreground resumes immediately.
        cadence.set_foreground(true, 120_001);
        assert!(cadence.should_scan(120_002));
        assert!(cadence.is_foreground());
        // A second background transition starts a fresh window.
        cadence.set_foreground(false, 200_000);
        assert!(cadence.should_scan(210_000));
        assert!(!cadence.should_scan(230_001));
    }

    // ---- the report's poll/IO numbers ---------------------------------------

    /// Not a timing assertion: this prints the one scan's real numbers (the
    /// report's table) and only bounds them generously. Run with `--nocapture`.
    #[test]
    fn the_poll_io_budget_is_measured_and_bounded() {
        let dir = std::env::temp_dir().join(format!("suramak-watch-io-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 199 small files + one 4 MiB file = the 200-entry bound, ~4 MiB read.
        for index in 0..199 {
            std::fs::write(dir.join(format!("clip-{index:03}.wav")), [0_u8; 64]).unwrap();
        }
        let big = vec![7_u8; 4 * 1024 * 1024];
        std::fs::write(dir.join("meeting.m4a"), &big).unwrap();

        let mut scanner = Scanner::with_settle(
            Box::new(FsWatchSource),
            dir.to_string_lossy().into_owned(),
            Box::new(|_, _, _| true),
        );
        let first = std::time::Instant::now();
        let report = scanner.scan(0);
        let first_ms = first.elapsed().as_secs_f64() * 1_000.0;
        assert_eq!(report.examined, 200);
        assert_eq!(report.hashed, 200);
        assert_eq!(report.bytes_hashed, 199 * 64 + big.len() as u64);
        assert!(!report.truncated);
        for candidate in &report.candidates {
            scanner.acknowledge(&candidate.locator);
        }

        // Steady state: the same 200 entries, nothing new to read.
        let second = std::time::Instant::now();
        let steady = scanner.scan(WATCH_POLL_INTERVAL_MS);
        let steady_ms = second.elapsed().as_secs_f64() * 1_000.0;
        assert_eq!(steady.examined, 200);
        assert_eq!(steady.hashed, 0);
        assert_eq!(steady.bytes_hashed, 0);
        println!(
            "watch scan: first examined={} files={} hashed={} bytes={} {first_ms:.2} ms; \
             steady examined={} hashed=0 bytes=0 {steady_ms:.2} ms",
            report.examined, report.files_seen, report.hashed, report.bytes_hashed, steady.examined
        );
        assert!(
            first_ms < 5_000.0 && steady_ms < 1_000.0,
            "a bounded scan must not stall the poll: {first_ms} / {steady_ms} ms"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
