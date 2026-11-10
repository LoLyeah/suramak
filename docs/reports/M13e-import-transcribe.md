# M13e — import and background transcription

Workorder: `docs/WORKORDER-M13E.md` (plan source `docs/PLAN-M13-ANDROID.md` §3 M13e).
Base tree: `4d33383` (post-M13D; M16.1 queue and M16B landed).
Scope delivered: share-target intent filters, SAF import with a bounded progress
channel, a foreground transcription job (per-API type, wake lock, progress,
cancel, `onTimeout`), launch-time recovery with a resume/discard prompt, the
completion notification's landing, and the decode through the M15.2 shared path
— all host-tested and build-gated. **No emulator run and no device run** (the
workorder excludes them and this workspace has neither); every runtime claim
below says what was and was not executed, and §11 is the owner-device checklist
that is the honest boundary.

---

## 0. What the tree actually had, and the two decisions that follow

### 0.1 The M13c bridge named in the plan does not exist — again

`docs/PLAN-M13-ANDROID.md` §3 M13c calls for one `PlatformBridge`. The tree has
the M13b spike Activity with its own JNI methods and the M13d `ModelsBridge`
(one JSON function). M13e therefore follows the **M13d pattern** rather than a
plan-level abstraction that was never built: a second one-function bridge,
`app.suramak.transcribe.TranscribeBridge.nativeCommand(String): String`, whose
protocol is `crates/suramak-android/src/jobs.rs`. Two bridges exist because two
features landed in two slices; when M13G builds the compact UI it can collapse
them into one — the Rust side is already one `match` per operation.

### 0.2 The host is a `JobService` that implements `Service.onTimeout`

The plan says "`JobService` hosting transcription jobs … `onTimeout`". M13d
already established the platform fact (checked against `android-35/android.jar`
with `javap`): **`JobService` has no `onTimeout`; `Service` does** — Android 15's
six-hour limit is a foreground-service rule. M13e keeps the `JobService` (the
platform schedules and survives it, the same host M13d proved) and overrides
`Service.onTimeout(startId)`, exactly as `ModelDownloadJobService` does. The
"type selection" in the verification bullet is therefore
`foreground_type_for_api` — the *foreground-service* type, which is the only
type that exists.

### 0.3 The manifest still declares both foreground types

The service declares `foregroundServiceType="mediaProcessing|dataSync"` and picks
one at `startForeground` time: `mediaProcessing` from API 35, `dataSync` on 34,
the untyped two-argument call below 34. The selection lives in Rust
(`jobs::foreground_type_for_api`, unit-tested for 29/33/34/35/36) so the Kotlin
side cannot drift; Kotlin only maps the returned name to the `ServiceInfo`
constant (compile-checked).

---

## 1. Share targets (`ACTION_SEND` / `ACTION_SEND_MULTIPLE`)

`android/app/src/main/AndroidManifest.xml`, on the existing `MainActivity`
(`singleTask`, so a second share arrives at `onNewIntent`):

| mime type | why it is declared |
|---|---|
| `audio/*` | every audio type the share sheet offers; covers WhatsApp's `audio/ogg` voice notes, mp3, m4a, wav, flac |
| `audio/ogg` | explicit: WhatsApp `Ptt …opus` voice notes (also arrives as `audio/ogg; codecs=opus`) |
| `audio/opus` | explicit: Telegram `.opus` documents |
| `audio/webm` | explicit: Telegram WebM voice notes |
| `application/ogg` | the non-audio spelling providers use for `.opus` |
| `application/x-ogg` | the older spelling some file managers still send |
| `video/webm` | the decode path accepts WebM video's first audio track |
| `video/mp4` | screen recordings / video notes (first audio track) |
| `video/x-matroska` | `.mkv` / `.mka` |

Deliberately **not** declared: `application/octet-stream`. It would claim every
binary file on the share sheet for a decode that can only fail.

The same matrix is asserted twice: a host test parses the manifest
(`crates/suramak-android/tests/jobs_host.rs::the_manifest_declares_the_share_matrix_and_the_service`)
and `scripts/android/build-apk.sh` asserts each `mimeType` in the **packaged**
manifest with `aapt dump xmltree`, so a resource-merge surprise cannot ship.

Receiving: `ImportSource.urisFrom(intent)` reads `EXTRA_STREAM` and `ClipData`
(the two shapes share targets use), then everything goes through one path.

## 2. SAF import (`ACTION_OPEN_DOCUMENT`)

Entry point: a launcher shortcut (`res/xml/shortcuts.xml`), long-press the icon →
"Import audio" → `ActivityResultContracts.OpenMultipleDocuments` with
`audio/*` + `video/*`. A shortcut rather than an in-app button on purpose: the
spike's egui screens own the surface and the four checks must stay untouched
until M13G.

The copy (in `TranscriptionJobService`, under the foreground notification):

1. `begin_import` — the Rust host validates the mime family and the model,
   creates the destination `files/media/<key>.<ext>`, and **writes the journal
   before a byte is copied**;
2. `ParcelFileDescriptor.AutoCloseInputStream` → 64 KiB reads → the app's
   `media/` directory, with `fd.sync()` before the size check;
3. `import_progress` every 256 KiB (the only bridge traffic during the copy);
4. `import_done` — the host verifies the copied size against the declared size
   (when the provider publishes one) and only then marks `copy_complete` and
   queues the job.

The progress channel is bounded at both ends: Kotlin reports every 256 KiB, and
`import_progress` only rewrites the journal when the byte count advanced by
`PROGRESS_STEP_BYTES` (test `copy_progress_is_coalesced_to_a_bounded_step`:
8 MiB in 16 KiB reports produces ≤ 34 journal writes, ≥ 5).

Extension mapping: the display name's extension when the decode path knows it,
otherwise the mime type (`audio/ogg` → `.ogg`, `audio/opus` → `.opus`,
`video/mp4` → `.mp4`, …; unknown → `.audio`). The extension is only a probe
hint — the decoder identifies containers by content.

## 3. The foreground job

`app.suramak.transcribe.TranscriptionJobService` (a `JobService`):

* **schedule** — `JobInfo` with `setOverrideDeadline(0)`, no constraints,
  `setExpedited(true)` from API 31 (a share must start now, not at the next
  scheduler batch; the app is in the foreground when the share arrives), the
  URIs in **transient** extras (a `content://` grant is not persistable state),
  the language in normal extras; a **distinct job id per request**, because
  `JobScheduler` replaces a job with the same id (a second share would otherwise
  stop the first);
* **foreground** — `startForeground` with the type from §0.3; a refused
  promotion is logged and the job still runs with a normal notification;
* **wake lock** — `PARTIAL_WAKE_LOCK`, `acquire(6 h)`, released on every exit
  path (`complete`, `onStopJob`, `onTimeout`, `onDestroy`);
* **progress notification** — `setProgress(100, percent, false)`, text
  `"<percent>% · <n> waiting"`, `setOnlyAlertOnce`, updated only when the
  percent, the queue count or the current job changes;
* **cancel** — a notification action → `CancelTranscriptionReceiver` →
  `TranscribeBridge.cancel(key)` → the cooperative token; the copy is deleted
  and no row is written. The receiver carries the key, so it cancels the job the
  notification is about;
* **`onTimeout`** — `pauseAll()` (cooperative stop, copy kept), "Transcription
  paused (time limit)" notification, `jobFinished(..., false)`, and the journal
  still holds the job for the next launch;
* **`onStopJob`** — same pause contract, returns `false` (the journal is the
  retry, not the platform).

## 4. One inference slot

The service never runs an engine. It copies, then polls `status()`. The work is
`QueueDriver` + `Worker` + `ThreadedPrefetch(FileDecoder)` — the M16.1 pump the
desktop CLI runs, unchanged. `JobQueue::next_inference` returns `None` while an
item holds the slot, so a batch of shared files cannot run two inferences.
Test: `a_batch_keeps_exactly_one_inference_slot` submits three files to a fake
backend that counts concurrent runs and asserts `max_active == 1` and all three
land in the library.

`JobOptions` is per batch (the same shape the CLI uses), so a second job with a
different model id is refused with a named reason rather than silently
transcribed with the first job's model.

## 5. Recovery: what persists, and the prompt

One file: `files/jobs/journal.json`, written atomically
(`tmp → fsync → rename → dir fsync`) on every phase change, at most every
256 KiB of copy and every 5 % of transcription.

It carries **only**: the display name, the copied file path, the model id, the
language, the phase and the last percent/bytes — plus the copy-complete flag.
It never carries a segment or any transcript text: the first and only write of
inference text is `Store::insert_transcript_with_words`, in one transaction,
after `Finished`. Test `a_killed_job_is_offered_for_resume_and_resumes` asserts
the on-disk journal contains the title and the model but no segment text, after
a process that was killed at 50 % of transcription.

At launch `MainActivity.offerRecovery()` calls `recover()` and, when the journal
has unfinished jobs, shows a non-cancellable dialog listing them:

* **Resume** (only when the copy completed and the file is still present):
  re-queues each job in the host and schedules the service to pump it;
* **Discard**: removes the entry and the copied file. Nothing enters the library.

A job whose **copy** was cut short is not resumable — the SAF grant died with the
process, so its bytes cannot be read again. The prompt says exactly that
(`"the import had not finished copying; its bytes cannot be read again"`) and
offers only Discard. Test: `an_interrupted_copy_is_offered_as_discard_only`.

Resume restarts the decode. The percent is kept for the prompt, not as
resumable audio state — a partial decode buffer is never stored (M16.1's rule).

## 6. Completion: what a tap does today

The completion notification (`transcribe_done_channel`, default importance) has
one `PendingIntent` to `MainActivity` with
`ACTION_OPEN_TRANSCRIPT` + `EXTRA_TRANSCRIPT_ID`.

Today (no compact UI — M13G replaces the spike screens):

1. the Activity receives it in `onCreate` (cold) or `onNewIntent` (warm,
   `singleTask`);
2. `TranscribeBridge.open(transcript_id)` reads the row from the library;
3. if found: a long toast `"Transcript ready: <title> (<n> segments)"` and a line
   in the spike's existing event ring; if not found: `"That transcript is no
   longer in the library"`.

So the tap always *lands* — it never crashes, never opens a screen that does not
exist yet, and the intent is exactly the one M13G's library will consume (the
extra name is in `Notify.EXTRA_TRANSCRIPT_ID`). That is the "one path, invisible
if the target screen does not exist yet" the workorder asks for.

## 7. Decode through the M15.2 shared path

The job's decoder is `suramak_asr::FileDecoder` — `decode::decode_file_cancellable`
plus `resample::to_16khz_mono`. No `MediaCodec`, no Kotlin resampler, no second
decoder.

`crates/suramak-android/tests/decode_parity.rs` drives **that type** over the
committed fixtures and asserts:

* Ogg-Opus / WebM-Opus / Matroska-Opus decode to **bit-identical** PCM for both
  languages (M15.2's container parity; compared sample-by-sample with the same
  rule the core suite states as a hash);
* `FileDecoder`'s output equals `to_16khz_mono(decode_file(...))` exactly — the
  job adds only the documented resample;
* truncated `.opus`/`.webm`/`.mkv` prefixes fail with the named `Truncated`
  error, through `FileDecoder` too (`DecodeFailure::Failed` naming the file) —
  no fragment is ever imported;
* garbage fails with `UnsupportedFormat`; a pre-cancelled token and a token
  flipped mid-packet-loop both stop the decode.

## 8. Verification

### 8.1 Host tests (new)

| file | tests | what they pin |
|---|---|---|
| `crates/suramak-android/src/jobs.rs` | 7 | API→type matrix, mime/extension matrix, title rule, torn-journal recovery, journal round-trip (no text), integer percent |
| `crates/suramak-android/tests/jobs_host.rs` | 12 | share→copy→transcribe→library row; one slot across a batch; cancel mid-decode; cancel mid-transcribe; kill→resume; discard; interrupted copy discard-only; model refusal; bounded progress; manifest matrix; bridge/foreground types; status numbers |
| `crates/suramak-android/tests/decode_parity.rs` | 4 | M15.2 container parity through the job decoder; truncation; garbage; cancellation |

`a_shared_voice_note_becomes_a_library_row` runs the **real** `FileDecoder` over
`speech-en.opus` (the actual Ogg-Opus voice-note shape) and asserts the stored
row's title, source name, audio path and segment text — the end-to-end loop
minus whisper itself, which no host test can run without a model.

### 8.2 Gates (all run in this workspace)

| gate | command | result |
|---|---|---|
| Rust fmt | `cargo fmt --all --check` | clean (exit 0) |
| Rust clippy | `cargo clippy --workspace --all-targets -- -D warnings` | clean (exit 0) |
| Workspace tests | `RUST_TEST_THREADS=1 cargo test --workspace` | **961 passed, 0 failed, 10 ignored**, 30 binaries |
| Android compile | `cargo ndk -t arm64-v8a --platform 29 check -p suramak-android --locked` | `Finished dev profile … in 1.64s` |
| Kotlin | `:app:compileDebugKotlin` with `allWarningsAsErrors = true` | `BUILD SUCCESSFUL` (no warnings) |
| APK gate | `bash scripts/android/build-apk.sh` | green; **33 494 907 bytes (31 MiB)**, `sha256 c04ac594158a43a4456f49e77ab4d7896d3b2dc3eb3dfa52f812f7b20fdde391` |
| APK signature | `apksigner verify --verbose app-debug.apk` | `Verifies`; v2 scheme true; 1 signer (the debug key) |
| Packaged manifest | `aapt dump xmltree … AndroidManifest.xml` | all nine mime types in both share filters; `OPEN_DOCUMENT` with `audio/*`+`video/*`; `TranscriptionJobService` `foregroundServiceType=0x2001`; `WAKE_LOCK`, `FOREGROUND_SERVICE_MEDIA_PROCESSING`; `CancelTranscriptionReceiver`; `android.app.shortcuts` |
| APK classes | `dexdump` over `classes*.dex` | `TranscriptionJobService`, `TranscribeBridge`, `CancelTranscriptionReceiver`, `Notify`, `ImportSource`, `ModelChoice` |
| JNI symbols | `llvm-nm -D --defined-only libsuramak_android.so` | `Java_app_suramak_models_ModelsBridge_nativeCommand`, `Java_app_suramak_transcribe_TranscribeBridge_nativeCommand` |
| Job scheduling | `dexdump` | `setExpedited` present in both enqueue paths |

Test growth: 938 → **961** (+23: 7 unit in `jobs.rs`, 12 in `tests/jobs_host.rs`,
4 in `tests/decode_parity.rs`). No ignored test was added or removed.

## 9. Known issues / honest boundaries

1. **No emulator, no device.** Nothing in this slice was exercised on Android:
   the service has never been started, the notification never shown, the SAF
   picker never opened, the wake lock never taken, and the journal never
   recovered on a real process death. Every Android-side claim above is
   compile/artifact-verified plus host-tested logic. §11 is the checklist that
   closes this on the owner's phone.
2. **Whisper is not exercised on the phone by any test here.** The host test
   stops at the library insert with a fake backend; the engine itself is M13a's
   proven path.
3. **`ACTION_OPEN_DOCUMENT` from a shortcut.** The picker is reachable only by
   long-pressing the launcher icon. M13G's library gets the in-app button; the
   shortcut is the honest pre-UI entry point (it adds no view to the spike).
4. **A second share while a batch runs** starts a second job/service instance
   (distinct job ids). Both instances show their own progress notification and
   both read the same host status, so the host still runs one inference; the
   completion notification is posted by whichever instance sees the batch finish
   first. Nothing is lost, but two notifications can coexist briefly.
5. **`POST_NOTIFICATIONS`** is requested at launch on API 33+. A refusal leaves
   the job working silently (the toast says so); the runtime prompt is not a
   gate.
6. **Language** is always `auto` today: no settings surface exists until M13G.
   The protocol carries it, and the journal records it.
7. **Copy progress for providers without `OpenableColumns.SIZE`** shows 0 % until
   the copy finishes (the notification's percent is byte-derived). The job still
   works; only the number is uninformative.
8. **`files/media` grows**: a completed transcript keeps its audio (playback
   needs it), a cancelled/failed import deletes its copy. There is no storage
   manager yet (M13G/M15 territory).
9. **The share grant must outlive the copy.** A `content://` grant from
   `ACTION_SEND` lives with the sending task/UID grant, not forever. The service
   copies immediately (that is the first thing it does, before any decode), so
   the window is seconds; a job that somehow starts after the grant died fails
   as a named import error ("Permission denial") and writes nothing — it cannot
   import a zero-byte or truncated file, because `import_done` checks the size.

## 10. Files touched

**Rust**
* `crates/suramak-android/src/jobs.rs` — new: the import/transcription host,
  journal, worker, protocol, 7 unit tests.
* `crates/suramak-android/src/lib.rs` — `jobs` module, the process-wide host,
  `dispatch_jobs` (the `init` special case), the `TranscribeBridge` JNI symbol.
* `crates/suramak-android/Cargo.toml` — `serde`, `uuid` edges (both already in
  the lock and graph; no new package).
* `crates/suramak-android/tests/jobs_host.rs`, `tests/decode_parity.rs` — new.

**Kotlin / resources**
* `android/app/src/main/java/app/suramak/transcribe/` — `TranscribeBridge.kt`,
  `TranscriptionJobService.kt`, `CancelTranscriptionReceiver.kt`,
  `ImportSource.kt`, `ModelChoice.kt`, `Notify.kt`.
* `android/app/src/main/java/app/suramak/spike/MainActivity.kt` — share/SAF/
  notification intent handling, the recovery dialog, the notification-permission
  request, the SAF launcher contract. No view added, no egui change.
* `android/app/src/main/AndroidManifest.xml` — permissions, share filters,
  SAF filter, the service (both types) and the receiver, the shortcut metadata.
* `android/app/src/main/res/values/strings.xml`, `values-in/strings.xml` — the
  notification/recovery copy in EN and ID.
* `android/app/src/main/res/xml/shortcuts.xml` — new.
* `android/app/build.gradle.kts` — versionCode 7, versionName `0.4.0-m13e.1`.

**Build**
* `scripts/android/build-apk.sh` — version, share matrix (per mime), SAF action,
  service + both types + permissions, receiver, shortcut metadata, and both JNI
  symbols asserted in the packaged APK.

**Deliberately untouched:** `crates/suramak-ui`, `crates/suramak-taps`,
`crates/suramak-cli`, the M15 suites, `crates/suramak-android-spike` (the four
checks and the spike's egui screens), and `suramak-core` / `suramak-asr`
behaviour.

## 11. Owner-device checklist (the honest runtime boundary)

Not executable in this workspace. On the owner's phone, with a debug APK from
`scripts/android/build-apk.sh`:

1. Install: `adb install -r android/app/build/outputs/apk/debug/app-debug.apk`
   (versionName `0.4.0-m13e.1`).
2. **Model**: the app needs `ggml-tiny.bin`/`ggml-base.bin` in
   `files/models` — the M13d download path fills it. Without one, a share shows
   "No model installed" (that itself is worth seeing once).
3. **Share a voice note**: WhatsApp → a voice note → Share → Suramak. Expect the
   transcription notification (`N% · 0 waiting`), then "Transcript ready".
4. **Screen off**: repeat step 3 and press the power button immediately. The
   transcription must finish with the screen off (that is the wake lock +
   `mediaProcessing`/`dataSync` path).
5. **Cancel**: share a long file, tap Cancel on the notification. Expect the
   notification to disappear and **no** new library row (`adb shell run-as
   app.suramak.spike sqlite3 files/suramak.db 'select count(*) from
   transcripts'` — or the library screen once M13G exists).
6. **Kill mid-job**: share a 30–60 minute file, wait for "transcribing", swipe
   the app away from recents. Reopen Suramak: the dialog must list it with
   **Resume** / **Discard**. Resume must finish and add the row; Discard must
   remove it and leave the library unchanged.
7. **Timeout path** (optional, slow): leave a very long job running for six
   hours on API 34/35 to see "Transcription paused (time limit)" and the resume
   prompt on the next launch. Not practical on most days; the pause path is
   identical to step 6's kill path in code.
8. **Completion tap**: tap the "Transcript ready" notification. Expect the toast
   naming the transcript and its segment count (M13G replaces the toast with the
   library row). Tap it with the library row already deleted → the "no longer in
   the library" toast.
9. **SAF**: long-press the Suramak icon → "Import audio" → pick an mp3/m4a. Same
   notification flow.
10. **SEND_MULTIPLE**: share three voice notes at once from WhatsApp. Expect one
    progress notification whose percent walks 0→100 three times and "3 waiting"
    at the start, then one completion notification (the last one).

Report an issue with `adb logcat -s SuramakTranscribe SuramakSpike` and the
spike's crash banner; the journal is at
`/data/data/app.suramak.spike/files/jobs/journal.json` (`run-as`).
