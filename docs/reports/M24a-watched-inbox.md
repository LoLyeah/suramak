# M24A — the watched inbox (a folder in, transcripts out)

Status: complete on the host and cross-compiled for `arm64-v8a`.
Non-goals kept: no new crates (polling, not `notify`), no cloud folder APIs,
no second transcription path, no queue journal/resume changes, no Library or
editor restyle beyond the one status row, no CI edits, no commits.

The user job: point Suramak at a folder once; every audio file that finishes
landing there is imported and queued automatically, and the Library says what
it picked up. No dialogs, no re-imports, no half-written files.

## 1. Watch settings (additive)

`Settings.watch` is a new `[watch]` table:

```toml
[watch]
enabled = false
folder = ""
```

* Missing in an older file → **off with no folder**, which is exactly the
  pre-M24A behaviour: no thread, no status row, no scan.
* A half-configured section (enabled but no folder) is inert by construction
  (`WatchSettings::is_configured`).
* **Desktop** picks the folder with the existing rfd picker on macOS/Windows
  (`Choose folder…` in Settings → Watched inbox); Linux keeps the path field,
  the same split the Home view's import affordance uses.
* **Android** stores a persisted SAF tree URI (`content://…/tree/…`) — never a
  raw path. `MainActivity` launches `ACTION_OPEN_DOCUMENT_TREE`, calls
  `takePersistableUriPermission(uri, FLAG_GRANT_READ_URI_PERMISSION)`, and hands
  the URI to the host. `settings.toml` keeps the URI, so the grant survives
  reboots; the content resolver is the only reader.
* `WatchSettings::folder_name()` renders the display name for the status row
  (`/Users/me/Inbox` → `Inbox`; `…/tree/primary%3ARecordings` → `Recordings`)
  and never shows the raw URI.

## 2. The polling scanner (`suramak-core::watch`)

One thread, one folder, no filesystem-notification crate.

| Rule | Value |
|---|---|
| Poll interval | **5 000 ms** |
| Settle gate | size unchanged for **≥ 2 000 ms** across two scans |
| Depth | **≤ 3** below the chosen folder (the folder is depth 0) |
| Entries per scan | **≤ 200**, then `truncated = true` (logged, never silent) |
| Eligible names | audio extensions only (`wav mp3 m4a m4b aac flac ogg oga opus aiff aif wma amr spx caf au`) |
| Writer markers | `.part`, `.crdownload`, `.tmp` — in the name or as a sibling |
| Identity | sha256 of the bytes (streamed, 64 KiB buffer — never a whole-file buffer) |

A file is eligible only when all of these hold:

1. its name has an audio extension;
2. it does not end with / contain a writer marker, and no
   `name.part` / `name.crdownload` / `name.tmp` sibling exists;
3. its size has not changed for at least 2 s (the first scan only observes;
   the second compares).

`Scanner` keeps one `Observation` per locator between scans (size, first-seen
time, last-seen, mtime). A size change restarts the settle clock. An emitted
candidate is suppressed until the host acknowledges it, so one scan can never
queue the same file twice. Files that leave the folder are forgotten (bounded
memory).

### The test seam

* `WatchSource` — “list one directory + open one file”, implemented by
  `FsWatchSource` (desktop), `FakeWatchSource` (host tests) and
  `SafWatchSource` (Android, NDK-gated). The scanner never touches a path.
* `WatchClock` / `RealClock` / `FakeClock` — the scanner's time is a parameter
  (`scan_with_clock`, `poll_with_clock`), never `Instant::now()` inside.
* `SettlePredicate` — the settle rule is a parameter
  (`Scanner::with_settle`); the default is `default_is_settled`.

## 3. `watched_files` — schema v11, additive

```sql
CREATE TABLE watched_files (
    path          TEXT PRIMARY KEY,
    size          INTEGER NOT NULL,
    mtime         INTEGER NOT NULL,
    sha256        TEXT NOT NULL,
    imported_at   TEXT NOT NULL,
    transcript_id TEXT
);
CREATE INDEX watched_files_sha ON watched_files(sha256);
```

* One row per **actual import**. `path + size + mtime` is the quick path
  identity (a re-scan never re-hashes an imported file).
* `sha256` is the content identity: the same bytes copied under a new name are
  a duplicate and never queue again. A duplicate name is acknowledged for the
  session but writes no row, so `N` stays “files imported”, not “files seen”.
* `transcript_id` is attached in the **same transaction** that saves the
  transcript (`insert_transcript_with_words` → `UPDATE watched_files … WHERE
  path = audio_path`), so a crash cannot leave a half-linked row.
* The migration is `v10 → v11` in one transaction, `CREATE TABLE IF NOT
  EXISTS`; no existing table is rebuilt. A fresh database creates the table in
  the main DDL and the migration is a no-op.

## 4. One import path

`Inbox::poll` records the ledger and returns the files that are genuinely new.
The host hands those to its **existing** import path:

* Desktop: `AppState::begin_imports` — the same function the picker and the
  path field call. The first watch import starts a job exactly like a picked
  file (`SubmitJob`); later ones join the queue.
* Android: `JobHost::begin_import` → stream the SAF document into
  `media/<key>.<ext>` → `import_done` → the same `QueueDriver`.

There is no second transcription route. The model, language default, glossary,
queue order and dedupe are the ordinary import's.

If a recording owns the audio session when a watch import lands, `begin_imports`
would refuse it (by design). `AppState` keeps those paths in `watch_deferred`
and queues them through the same import path when the recording stops
(`on_recording_stopped`) — nothing is silently dropped.

## 5. Honesty in the UI

* **Phone Library** (deck/compact): one row while watching is on:
  `Watching · 3 imports` on the left, the folder's display name on the right.
  Failure: `Watching paused — the folder is no longer reachable` in the error
  ink, with the folder name still visible. Nothing is drawn when watching is
  off.
* **Desktop Library sidebar**: the same line under the search field.
* **Settings**: a `Watched inbox` section — the switch, the folder (picker or
  path field), and a caption with the poll/settle/dedupe rules. On the phone,
  the switch with no folder opens the tree picker instead of persisting a
  half-configured setting.
* New imports appear because they go through the ordinary queue: the finished
  job saves a transcript and the library list refreshes as usual.

## 6. Battery / IO discipline (Android)

`WatchCadence` keeps scanning for **30 000 ms** after the app leaves the
foreground, then stops. The poll thread does not merely skip work: it **parks
on its command channel** (`recv()`), so a backgrounded phone gets zero timer
wakes until `onStart` says the Activity is visible again. `onStart` /
`onStop` push that state through `UiBridge.watchForeground`.

Desktop is always foreground; it does not use the pause window.

## 7. Poll/IO numbers

Measured by `watch::tests::the_poll_io_budget_is_measured_and_bounded`
(debug build, 2-core Linux VPS, a real 200-entry folder: 199 × 64 B files +
one 4 MiB `.m4a`):

| Metric | Value |
|---|---|
| Entries examined, first scan | 200 (bound 200, not truncated) |
| Files hashed, first scan | 200 |
| Bytes read, first scan | 4 207 040 B (~4.0 MiB) |
| First scan wall time | **157.85 ms** (≈ 26.6 MB/s sha256, debug) |
| Steady-state scan (same 200 entries, nothing new) | **2.06 ms**, **0 bytes read**, 0 hashes |
| Steady-state CPU per day | 17 280 scans × 2.06 ms ≈ **35.6 s/day** (≈ 0.04 % of one core) |
| Steady-state bytes per day | **0** (metadata only) |
| Import latency, file present at launch | second poll ≈ **5 s** |
| Import latency, file lands just after a poll | ≤ **10 s** |
| Hash chunk buffer | 64 KiB (one per scan thread) |
| Memory between scans | one `Observation` per seen locator (≤ 200) |

The first scan reads each new file exactly once, ever: the ledger suppresses
the path afterwards and sha256 suppresses the same bytes under any other name.
On subsequent scans the only I/O is `readdir` + one `stat` per entry.

## 8. Tests

Core (`suramak-core::watch`, 17 tests) — the named acceptance cases:

* `the_settle_gate_rejects_a_growing_file_and_accepts_it_once_stable` — a
  growing file is rejected; the same size for 2 s is accepted.
* `the_settle_gate_can_be_injected_for_deterministic_tests` and
  `the_clock_is_injected_and_never_read_inside_the_scanner` — the two seam
  parameters.
* `a_part_sibling_blocks_import_until_the_writer_finishes`,
  `a_marked_name_is_never_eligible` — `.part` / `.crdownload` / `.tmp`.
* `the_same_bytes_under_a_new_name_import_once`,
  `a_known_path_is_skipped_and_a_known_sha_under_a_new_name_is_a_duplicate`,
  `a_rescan_of_the_same_folder_imports_nothing` — sha256 dedupe through the
  real `watched_files` table (`Store::open_in_memory`).
* `a_deleted_folder_surfaces_the_named_failure_and_recovers`,
  `a_real_missing_folder_is_unreachable_too` — the named failure and recovery.
* `the_scan_is_bounded_at_two_hundred_entries_and_logs_the_truncation` — 201
  entries → 200 examined + `truncated`.
* `the_walk_stops_at_depth_three` — depth 4 is never listed.
* `only_audio_names_are_eligible`, `a_partial_sibling_blocks_the_target_until_it_is_gone`,
  `scanning_pauses_in_the_background_after_the_bounded_window`,
  `the_poll_io_budget_is_measured_and_bounded`.

Store (`suramak-core::store`, 3 tests): the ledger reads back by path and sha,
a re-record keeps the transcript link, the save transaction attaches the
transcript, and a real v10 database upgrades to v11 without losing rows.

Desktop (`suramak-ui`): the Settings form round-trips `[watch]` through TOML;
the status row says `Watching · 3 imports` / singular / the named failure /
recovery; watched imports produce `SubmitJob` and join the one queue; the
watch thread settles a real file in a real folder, records one ledger row and
never imports it twice.

Android (`suramak-android::watch_host`, host tests): folder display names from
tree URIs, the 30 s window, and an unconfigured host stays off.

## 9. Platform notes and limits

* **Android transcript link.** The phone imports by copying the SAF document
  into `media/`, so the transcript's `audio_path` is the copy, not the SAF
  document id. `watched_files.transcript_id` therefore stays `NULL` on the
  phone; the desktop fills it in the save transaction. The ledger's dedupe
  (path/size/mtime/sha) is unaffected.
* **Quit with deferred imports.** If the app quits while a recording is live
  and watch imports are deferred, those files are already in the ledger but
  never queued. They are not re-imported on the next launch (the ledger still
  suppresses them). This is the one known corner of the “recording owns the
  audio session” rule; it needs a recording that spans a folder drop and then
  a quit before Stop.
* **Android process death.** The watcher runs in the app process. A killed
  process stops scanning until the Activity is opened again; the ledger makes
  the next launch's re-scan a no-op. A background `WorkManager` scan was out of
  scope (no new APIs/crates, and the workorder asks for one thread).
* The desktop watcher opens its own SQLite connection (WAL); the UI's
  connection sees committed rows on its next query. The write rate is one row
  per imported file, not per poll.
