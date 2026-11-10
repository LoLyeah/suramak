# M13f — microphone recording on Android (locked screen included)

Workorder: `docs/WORKORDER-M13F.md` (plan source `docs/PLAN-M13-ANDROID.md` §3 M13f).
Base tree: `5248c9e` (post-M13E: share-target import + background transcription,
961 tests). Scope delivered: the `RECORD_AUDIO` runtime flow with honest denial
states, a `microphone`-typed foreground service started only from the
foreground, AAudio capture through the M6/M14B2 `suramak-asr` path with a
streaming 16 kHz mono PCM16 tail, the interruption state machine and its
journal, record → transcribe hand-off into the M13E inbox, locked-screen
survival with Doze/kill surfacing, and Quit's capture-first ordering. **No
emulator and no device run** (the workorder excludes them and this workspace has
neither): every runtime sentence below is either host-tested logic or a
compile/artifact claim, and §11 is the owner-device checklist that is the honest
boundary.

---

## 0. The two platform facts that shaped the slice

### 0.1 cpal 0.15.3 needs no feature flip for AAudio

`docs/WORKORDER-M13F.md` says to verify cpal's oboe/audio-aaudio feature
selection before reaching for a crate. Checked against the vendored crate
manifest: **there is no `aaudio` or `oboe` feature in cpal 0.15.3.** For
`cfg(target_os = "android")` it depends on `oboe 0.6` (with `java-interface`)
and `ndk 0.8` unconditionally. `Cargo.lock` already carried `oboe`, `ndk`,
`ndk-context` and cpal's `jni` edge (they were in the graph through cpal even
with `capture` off). So the change is a **feature enablement**, not a new
dependency and not a feature flip:

```toml
[target.'cfg(target_os = "android")'.dependencies]
suramak-asr = { path = "../suramak-asr", features = ["capture"] }
```

The target-scoped edge keeps desktop builds exactly as they were (no ALSA
requirement). `cargo ndk -t arm64-v8a --platform 29 check -p suramak-asr
--features capture --locked` compiled `oboe-sys`, `cpal` and the Android
capture module in 1 m 30 s before any of this slice's code existed; the final
`cargo ndk ... check -p suramak-android --locked` is the gate in §9.

### 0.2 Buffering is not an option on a phone — the tail is streamed

The desktop `Recorder` accumulates the whole take as f32 and resamples once at
stop. Thirty minutes at 48 kHz is ≈345 MB of f32, and a process kill would lose
all of it. The Android path therefore performs the **same arithmetic
incrementally** in `crates/suramak-asr/src/capture_stream.rs`:

* the mono device-rate stream is cut into the **same 1024-sample chunks** core's
  `to_16khz_mono` feeds rubato;
* `rubato::FftFixedIn` is constructed with the same parameters
  (`FftFixedIn::new(rate, 16000, 1024, 2, 1)`);
* the 16 kHz output is appended to a canonical 44-byte-header PCM16 WAV as it
  arrives, and `finish` replicates core's exact **truncate-only** tail rule;
* the file is therefore byte-for-byte the desktop writer's output for the same
  input (unit-tested at 48 kHz, stereo-callback mixdown, 44.1 kHz and 16 kHz
  native).

A crash before `finish` leaves a valid header plus every captured frame;
`capture_stream::repair` patches the two size fields after a kill. This is what
makes "locked-screen, process killed" recoverable rather than a lost take.

---

## 1. Permissions — request once, then say the truth

`MainActivity` adds the M13F record bar (a small Kotlin overlay at the bottom of
the existing spike-era screen; M13G replaces it with the Deck's compact
recording surface). Its state is read from the Rust host, so a stop performed by
the notification, an interruption or a recovery is reflected within half a
second.

* **First Record tap** requests `RECORD_AUDIO` once and records that it did
  (`SharedPreferences "mic.permission_requested"`).
* **Granted** → the service starts (only while the Activity has window focus).
* **Denied** → the Record button is disabled, the bar says *"Microphone
  permission is off — recording is disabled."*, and a **Settings** button opens
  `ACTION_APPLICATION_DETAILS_SETTINGS` for the app. A later tap never re-prompts
  silently; it opens Settings.
* A permanent denial (`shouldShowRequestPermissionRationale == false`) and a
  soft denial get different sentences (`mic_permission_settings` /
  `mic_permission_denied`).
* The Rust host refuses `start` unless the request claims **both**
  `foreground: true` and `permission: true`, with named refusals
  (`"the microphone service must start from the foreground (Android 14+
  while-in-use rule)"`, `"RECORD_AUDIO is not granted — enable the microphone
  for Suramak in Settings"`). Unit tests pin both, plus the missing-model case;
  a refused start writes no WAV and no journal entry.
* The bar also has an explicit unavailable state when the native host does not
  answer at all (a library/init failure), instead of a button that fails
  silently.

There is no Settings screen until M13G; "Settings" here is the system app page,
which is the only honest destination that exists today.

## 2. The `microphone` foreground service

`app.suramak.mic.RecordingService`, a plain started `Service` (not a
`JobService`: a recording is user-initiated and must start immediately, and no
job scheduler quota should gate it).

**Started only from the foreground, three ways:**

1. `MainActivity.startRecordingFromForeground` refuses without
   `hasWindowFocus()`;
2. `RecordingService.begin` re-checks its own process importance
   (`runningAppProcesses` ≤ `IMPORTANCE_VISIBLE`) and refuses otherwise — no
   `startForeground` is attempted from the background, so there is no
   background microphone promotion to catch;
3. `MicHost::start` independently refuses `foreground: false`.

`begin` then calls `startForeground(id, notification,
FOREGROUND_SERVICE_TYPE_MICROPHONE)` **before** asking Rust to open the stream.
A platform refusal (API 34+ while-in-use `SecurityException`, a denied
`FOREGROUND_SERVICE_MICROPHONE`, an OEM policy) is caught, surfaced as a status
notification, and the service ends — it never pretends to record.

**The persistent notification** (`Notify.recording`, channel `recording`, low
importance, silent, no badge):

* title "Recording", sub-text = the take's local-time title;
* `setUsesChronometer(true)` + `setWhen(startedWallMs)` — the **elapsed time** is
  the system's chronometer, so it stays truthful while the app is frozen behind
  the lock screen;
* body = the last-known state: "Screen locked? Recording continues." normally,
  "Doze is active — the foreground service keeps recording." when
  `PowerManager.isDeviceIdleMode` flips (the service polls it once a second and
  updates the notification only when it changes);
* **Stop** action = `PendingIntent.getService(ACTION_STOP)`, so it works from the
  lock screen;
* `FOREGROUND_SERVICE_IMMEDIATE` from API 31, so the notification is not
  deferred into a Doze window;
* the service holds a `PARTIAL_WAKE_LOCK` for up to 2 h and releases it on every
  exit path.

The service **stops itself when the recording stops**: `stopCapture` calls the
Rust host (which finalizes and journals), removes the foreground notification,
releases the wake lock, abandons audio focus, unregisters the device callback and
calls `stopSelf()`. There is no lingering `microphone` service.

## 3. Capture through the existing path

`crates/suramak-asr/src/capture_android.rs` is the Android arm of the M6/M14B2
lineage:

```
AAudio callback (RT)                        consumer thread (non-RT)
────────────────────                        ───────────────────────
RtProducer::write_block                     rtrb::Consumer → mono block
  MonoFrames mixdown (existing)      ──►      StreamingWav::push
  try-push into the SPSC ring                 (1024-sample → rubato → PCM16)
  full ring ⇒ drop + count                   RollingRms (100 ms meter)
```

* The callback is the **same allocation-free mixdown** and `RtProducer` the
  desktop uses (`capture_rt.rs`); `cpal::Stream` stays on the dedicated capture
  thread because it is `!Send`.
* Device-native rate is whatever AAudio reports (48 kHz is the common phone
  case; 44.1 and 16 kHz are handled); the stream is resampled to 16 kHz and
  written as PCM16 mono, the one recording format.
* `take_error()` is polled by the capture thread; a stream error breaks the loop,
  finalizes the WAV, and is surfaced as `StopReason::StreamError` on the next
  status poll — never a silent stop of growth.
* The ring capacity is one second of mono samples (the M14B2 rule); a full ring
  drops samples and counts them, which the host reports as `dropped`.
* The consumer writes the WAV as it goes, so the phone never holds the take in
  memory and a kill leaves the captured part on disk.

## 4. Interruptions — clean stop, always a reason

`crates/suramak-android/src/mic.rs` owns a pure `MicMachine` plus the host.

**Detectors in Kotlin** (all stop through `MicBridge.stop(reason)`):

| signal | detector | reason |
|---|---|---|
| incoming/active call | `AudioManager.getMode()` is `MODE_IN_CALL` / `MODE_IN_COMMUNICATION`, polled by the service ticker | `call` |
| audio-focus loss | `AudioFocusRequest` listener: any change away from `AUDIOFOCUS_GAIN` (a refusal at start also prevents the take) | `audio_focus_loss` |
| Bluetooth headset disconnect/switch | `AudioDeviceCallback.onAudioDevicesRemoved` for `TYPE_BLUETOOTH_SCO` / `TYPE_BLE_HEADSET`, **plus** the ticker's input-device-name change against the name captured at start | `bluetooth_disconnect` |
| user | notification Stop / record-bar Stop | `user` |
| task removed | `onTaskRemoved` | `service_stop` |
| service timeout | `Service.onTimeout` | `timeout` |
| service destroyed mid-take | `onDestroy` | `service_stop` |
| capture stream failure | `take_error()` / the thread ending while `Recording` | `stream_error` |
| process death | journal replay at the next init | `process_killed` |

`READ_PHONE_STATE` is deliberately **not** requested: a call necessarily sets
the audio mode and takes focus, and the permission-free mode check is enough to
stop before audio from the call is captured. The boundary (a phone that is
ringing but not yet answered may not change the mode) is stated in §10 and is on
the owner checklist.

**The state machine** (`MicMachine`, 9 unit tests):

* `Stop` in `Starting` is remembered and closes the instant the session reports
  ready (a stop that races the microphone open is not lost);
* `Stop` in `Recording` closes once; a duplicate stop from a second listener is a
  no-op and the **first reason wins** (a call that also loses focus is one
  `call` stop);
* `Finished` is reachable only through `Stopping`; a capture thread that ends
  while the machine still believes it records settles as `StreamError` — there
  is no path from `Recording` to silent continuation;
* a late stop after the take settled cannot resurrect it; a new take can start
  after `Finished`.

**The journal** (`files/mic/journal.json`, atomic `tmp → fsync → rename → dir
fsync`):

* written **before** capture starts, at most every 5 s of elapsed time while
  recording (the service polls once a second; the journal is bounded), and at
  close;
* stores the key, title, source name, WAV path, model, language, state
  (`recording`/`stopped`/`abrupt`/`failed`), stop reason, elapsed, device
  name/rate, frames, bytes, hand-off flag and a note — **no audio and no
  transcript text**;
* a torn journal is moved to `journal.json.corrupt`, not a launch failure;
* finished, handed-off takes are pruned to the newest 20; a failed start with no
  frames is pruned so repeated failed attempts cannot grow it without bound.

**Clean means saved:** on every stop the WAV is finalized (header sizes patched),
the journal records the reason, and a take with frames is handed off. "What was
recorded stays" is the same promise for a user Stop, a call, focus loss, a BT
disconnect, a timeout and a service stop.

## 5. Record → transcribe hand-off

The WAV is written **directly at the M13E inbox path**, `files/media/<key>.wav`,
not to a separate recordings directory. At stop the host calls
`JobHost::adopt_recording` (`crates/suramak-android/src/jobs.rs`), the
recording's `import_done`:

* verifies the model is installed, the path is exactly in `media/`, the
  extension is `.wav`, and the file is a whole PCM16 WAV (44-byte header + even
  data);
* creates the same `JobRecord` an import creates with `copy_complete = true`,
  `bytes_done = size`, `percent = 100`, phase `Queued`;
* sends the same `Accept` command, so the M16.1 `QueueDriver` (one inference
  slot, ≤2 decodes ahead) and the M15.2 shared decoder run **unchanged**;
* the completion notification and the library row are the M13E ones.

Kotlin then calls `TranscriptionJobService.resumePending(context)` — the exact
pump the import path uses after `import_done` — so the transcription FGS,
progress notification, cancel action and completion landing are literally the
M13E flow.

Equivalence is unit-tested end to end
(`a_recording_hands_off_bytes_identical_to_an_import`): a 48 kHz take goes
through the streaming writer, the test writes the desktop fixture from the same
samples (`resample_16k` + `write_recording_wav`), then imports that fixture
through `begin_import`/`import_done`. The recorded WAV and the imported WAV are
byte-identical, both media files are byte-identical, both rows carry the same
fake-engine segment text, and the fake backend observes `max_active == 1` across
the recording and the import.

## 6. Locked screen, Doze and kill

* The `microphone` FGS plus the partial wake lock are what keep the capture
  alive with the screen off; the chronometer is the elapsed-time display the
  owner can read from the lock screen.
* The ticker updates the notification body when `isDeviceIdleMode` changes, so a
  Doze attempt is surfaced as the last-known state ("Doze is active — the
  foreground service keeps recording.").
* `onTaskRemoved` (swipe away) stops cleanly with `service_stop`; `onDestroy`
  does the same; `Service.onTimeout` uses `timeout`. `onLowMemory` /
  `onTrimMemory(TRIM_MEMORY_RUNNING_CRITICAL)` write a journal note (the note is
  part of the recovered take's story).
* A real process kill cannot run Kotlin: the notification disappears, and the
  **journal is the durable record**. At the next init the mic host replays every
  still-`recording` entry as `abrupt`/`process_killed`, calls
  `capture_stream::repair` to patch the WAV header, adopts the saved part into
  the inbox, and reports it in `MicStatus::recovered`. The next launch posts a
  "Recording recovered" status notification and the record bar shows "The last
  recording ended abruptly. <title> was recovered and queued for
  transcription." The notification's last-known state is the chronometer +
  Doze text while the service lives; §10 states plainly that a kill removes it.

Journal replay is tested with a real partial WAV written without `finish`
(`a_killed_take_replays_repairs_and_adopts_once`): the repaired data size equals
the file's remaining bytes, every frame survives, the take is adopted exactly
once, and a second launch does not re-adopt it.

## 7. Quit — capture first, service second, exit third

`MicHost::quit()` stops any live take synchronously (finalize, journal, hand
off) and returns `{ was_recording, handoff_key, next: ["stop_service", "exit"] }`.
`MainActivity.quitApp()` calls `MicBridge.quit()` **first**, then
`stopService(RecordingService)`, then `finishAndRemoveTask()`. The record bar's
Quit button confirms first ("Any recording is stopped, saved and queued for
transcription first."). The host test
`quit_stops_the_capture_before_the_service_step` asserts the capture is stopped,
the WAV is finalized, the take is handed off and the journal is closed **before**
`quit()` returns the service/exit steps.

## 8. Host tests (new)

| file | tests | what they pin |
|---|---|---|
| `crates/suramak-asr/src/capture_stream.rs` | 8 | 48 kHz stream == desktop WAV byte for byte; stereo-48k AAudio-callback mixdown == desktop; 44.1 kHz and native 16 kHz; repair of a killed take keeps every frame; torn final sample dropped; empty take valid; foreign WAV refused |
| `crates/suramak-android/src/mic.rs` | 9 | every interruption closes once with the first reason; stop before ready closes on ready; capture ending while recording settles as `stream_error`; idle/late events are no-ops; second start ignored; journal replay = `process_killed`; bounded 5 s elapsed writes; torn journal set aside; reason spellings |
| `crates/suramak-android/tests/mic_host.rs` | 9 | call stop → saved + handed off + journal kind; focus/BT kinds; first reason wins / duplicate stop no-op; killed take replay→repair→adopt once; quit capture-first; second start refused / idle stop no-op; foreground/permission/model refusals write nothing; capture death = `stream_error`; recording WAV == desktop WAV == import WAV with one inference slot |

`crates/suramak-android/tests/jobs_host.rs`'s manifest test now also asserts
`RECORD_AUDIO`, `FOREGROUND_SERVICE_MICROPHONE`,
`app.suramak.mic.RecordingService` and `foregroundServiceType="microphone"`.

Test growth: **961 → 987** (+26: 8 + 9 + 9). No ignored test was added or
removed (10 ignored before and after).

## 9. Gates (all run in this workspace)

| gate | command | result |
|---|---|---|
| Rust fmt | `cargo fmt --all --check` | clean (exit 0) |
| Rust clippy (workspace) | `cargo clippy --workspace --all-targets -- -D warnings` | clean (exit 0) |
| Rust clippy (Android target) | `cargo ndk -t arm64-v8a --platform 29 clippy -p suramak-android -- -D warnings` | clean (exit 0) |
| Workspace tests | `RUST_TEST_THREADS=1 cargo test --workspace` | **987 passed, 0 failed, 10 ignored** (see §10.11 for a pre-existing CLI queue race seen in earlier attempts) |
| Android compile | `cargo ndk -t arm64-v8a --platform 29 check -p suramak-android --locked` | `Finished dev profile …` |
| Kotlin | `:app:compileDebugKotlin` with `allWarningsAsErrors = true` | `BUILD SUCCESSFUL` (no warnings) |
| APK gate | `bash scripts/android/build-apk.sh` | green; **34 794 311 bytes (33 MiB)**, `sha256 4ed2515799cdc9d378f24ee698faef024ba9ffbb99e4e04a45ccd4ca5ff72e1d` |
| APK signature | `apksigner verify --verbose app-debug.apk` | `Verifies`; v2 scheme true; 1 signer (debug key) |
| Packaged manifest | `aapt dump xmltree … AndroidManifest.xml` | `RECORD_AUDIO`; `FOREGROUND_SERVICE_MICROPHONE`; `app.suramak.mic.RecordingService` with `foregroundServiceType=0x80`; `versionName 0.4.0-m13f.1` |
| APK classes/channel | `dexdump` over all `classes*.dex` | `MicBridge`, `RecordingService`, `RecordBar`; `createNotificationChannel`; the `"recording"` channel id |
| JNI symbols | `llvm-nm -D --defined-only libsuramak_android.so` | `Java_app_suramak_mic_MicBridge_nativeCommand` (plus the two M13d/e bridges) |

The APK grew from M13e's 33 494 907 bytes to 34 794 311 (+1.3 MB), which is the
static Rust capture/oboe code plus the small Kotlin surface; no new package was
added to `Cargo.lock` (only the `rubato` edge on `suramak-asr`).

Host gates were run with the Android NDK variables unset
(`env -u ANDROID_NDK_HOME -u ANDROID_NDK_ROOT -u CMAKE_TOOLCHAIN_FILE`):
`opusic-sys`'s build script treats a set `ANDROID_NDK_HOME` as a cross-build
request, so a host `cargo test` with the NDK exported tries to configure
libopus with the Android toolchain. The Android gates set those variables for
their own invocation only.

## 10. Known issues / honest boundaries

1. **No emulator, no device.** Nothing here has opened a real microphone: the
   AAudio stream, the `microphone` FGS promotion, the permission dialog, the
   notification, the lock-screen chronometer, Doze, a call, a Bluetooth removal
   and a process kill are compile/artifact-verified plus host-tested logic. §11
   is the checklist that closes this on the owner's phone.
2. **cpal/oboe on Android is linked but never exercised at runtime.** The
   Android target compiles `oboe-sys`/`cpal` and the capture module; whether
   Oboe opens the default input on a given OEM, and at which rate, is a device
   fact. The Rust start path reports a named refusal if the platform denies it.
3. **Call detection is audio-mode + focus based, not `READ_PHONE_STATE`.**
   A call that is ringing but not answered may not set `MODE_IN_CALL` yet; a
   focus loss usually stops first. The owner checklist includes a real call
   mid-record. Adding `READ_PHONE_STATE` would be a second dangerous permission
   for a narrow window; it is deliberately not requested.
4. **Bluetooth detection has two paths, neither guaranteed by every OEM**: the
   `AudioDeviceCallback` removal event and the input-device-name change. A phone
   that switches input silently without either would keep recording from the
   new source; the stream-error path is the last backstop. That is why the BT
   swap is on the owner checklist.
5. **Doze/kill is surfaced, not prevented.** The FGS + wake lock keep the capture
   alive under normal Doze; an OEM battery killer or a force-stop can still kill
   the process. The notification disappears with it (the honest limit); the
   journal is the durable record, and the next launch repairs and queues the
   saved part. A recording that is force-stopped before any sample reaches disk
   is an empty take and is not handed off.
6. **Recording requires an installed model.** The hand-off is the transcription
   flow, so with no model the service refuses to start and says "Download a
   speech model first" (the M13E copy). Recording without transcription is not a
   separate product path in this slice.
7. **The record bar is a stopgap.** It is a Kotlin overlay on the existing
   spike-era screen, not the Deck's compact recording surface; M13G replaces it
   (the workorder explicitly defers the compact UI). It stops its status poll in
   `onPause`, so it does not keep the UI thread awake behind the lock screen.
8. **The wake lock is capped at 2 h.** A longer continuous take would need a
   re-acquire path; 30–60 minute recordings (the plan's case) are covered. The
   journal still records the take if the lock expires and Doze intervenes.
9. **The mic journal is local bookkeeping.** It is not exposed in the library;
   recovered takes appear through `MicStatus::recovered` and the hand-off, not as
   a separate "recordings" list (M13G's library is the surface).
10. **Two notifications can coexist briefly** if a recording stops while an
    M13E transcription batch is already showing its own progress notification;
    they use different ids and channels, and both are truthful.
11. **A pre-existing CLI queue flake, not M13F** — FIXED by the orchestrator
    (`3f64e8c`; see §12 for the second root cause): `QueueDriver::is_idle` now
    counts in-flight prefetch decodes explicitly (`prefetch_in_flight`
    replaced counting only the queue's own rows — a cancelled row drops out of
    waiting while its pool task still reads the file, so the CLI batch loop
    could exit mid-batch). Stress-runs + the full workspace run passed
    987/0/10; the leaked CMAKE_TOOLCHAIN_FILE env (§12) was independently a
    second source of red.

## 11. Owner-device checklist (the honest runtime boundary)

Not executable in this workspace. On the owner's phone, with the debug APK from
`scripts/android/build-apk.sh` (versionName `0.4.0-m13f.1`):

1. **Install**: `adb install -r android/app/build/outputs/apk/debug/app-debug.apk`.
2. **Model**: install `ggml-tiny.bin`/`ggml-base.bin` in `files/models` (the
   M13d download path). Without one, Record must refuse with "Download a speech
   model first" — worth seeing once.
3. **Permission, once**: tap Record. The system dialog appears once. Deny it:
   the bar must show "Microphone permission is off — recording is disabled.",
   the Record button must be disabled, and Settings must open the app page.
   Grant it, return, and record a 10-second note. Expect the persistent
   notification with a running chronometer, then "Recording saved" and a
   transcription that reaches the library.
4. **30-minute locked-screen record** (the plan's verification case): start
   recording, press power, leave it for 30 minutes. The notification must show
   the elapsed time (chronometer) and, if the device enters Doze, "Doze is
   active — the foreground service keeps recording." Stop from the lock-screen
   notification. The WAV must be ≈30 × 60 × 16 000 × 2 + 44 =
   **57 600 044 bytes**; the transcription must finish with the screen still
   locked.
5. **Call interrupt**: start recording, call the phone from another line. The
   recording must stop with "Recording stopped" (call reason), the take must be
   saved and queued, and no audio from the call may be in the WAV.
6. **Bluetooth swap**: start recording on a Bluetooth headset, then turn the
   headset off (or switch it to another device). The recording must stop with
   the Bluetooth reason — it must **not** switch silently to the phone
   microphone and keep growing.
7. **Quit invariant**: start recording, tap Quit, confirm. The take must be
   stopped, saved and queued before the app leaves; the microphone notification
   and the service must be gone; the transcription must still appear in the
   library.
8. **WAV verification (desktop)**: pull the file with
   `adb exec-out run-as app.suramak.spike cat files/media/<key>.wav > take.wav`
   (use `exec-out`, not `shell`: a PTY can translate bytes), then check it is
   **16 kHz mono PCM16**:
   `file take.wav`, `ffprobe -show_streams take.wav`, or
   `python3 -c "import wave; w=wave.open('take.wav'); print(w.getframerate(),
   w.getnchannels(), w.getsampwidth())"` → `16000 1 2`. Run
   `suramak-cli doctor` on the desktop to confirm the engine stack, then
   `suramak-cli transcribe take.wav` — a successful transcript is the end-to-end
   proof that the bytes are the desktop format. (`doctor` itself checks paths and
   the engine, not an arbitrary WAV; the `file`/`ffprobe`/`wave` probe is the
   audio check.)
9. **Kill mid-record**: start recording, then `adb shell am force-stop
   app.suramak.spike` (or swipe the app away). Reopen Suramak: the record bar
   must say the last recording ended abruptly and was recovered/queued; the WAV
   header must be valid and its duration must match the captured part. Repeat
   with a normal swipe-away (not force-stop) and confirm the service stops
   cleanly with the same saved take.
10. **Runaway check**: after any stop, confirm no `microphone` foreground service
    remains (`adb shell dumpsys activity services app.suramak.spike | grep -i
    microphone` should be empty) and no recording notification remains.

Report an issue with `adb logcat -s SuramakMic SuramakTranscribe SuramakSpike`;
the mic journal is at
`/data/data/app.suramak.spike/files/mic/journal.json` (`run-as`), the take at
`files/media/<key>.wav`.

## 12. Files touched

**Rust**

* `crates/suramak-asr/src/capture_stream.rs` — new: streaming 16 kHz PCM16 WAV
  tail, byte-identical to the desktop writer, plus `repair`; 8 unit tests.
* `crates/suramak-asr/src/capture_android.rs` — new: AAudio recorder over cpal's
  Oboe backend, same `RtProducer`/mixdown/ring as M14B2, streaming consumer.
* `crates/suramak-asr/src/lib.rs` — the two modules (`capture_stream` always;
  `capture_android` only with `capture` on Android).
* `crates/suramak-asr/Cargo.toml` — `rubato` edge (already in the workspace
  graph/lock; no new package).
* `crates/suramak-android/src/mic.rs` — new: state machine, journal, host,
  protocol, 9 unit tests.
* `crates/suramak-android/src/jobs.rs` — `JobHost::adopt_recording` (the
  recording's `import_done`).
* `crates/suramak-android/src/lib.rs` — `mic` module, `MIC_HOST`,
  `dispatch_mic`, the `MicBridge` JNI symbol.
* `crates/suramak-android/Cargo.toml` — Android-target `suramak-asr/capture`
  edge (no new package).
* `crates/suramak-android/tests/mic_host.rs` — new: 9 host tests.
* `crates/suramak-android/tests/jobs_host.rs` — manifest test extended with the
  M13F declarations.

**Kotlin / resources**

* `android/app/src/main/java/app/suramak/mic/MicBridge.kt` — new.
* `android/app/src/main/java/app/suramak/mic/RecordingService.kt` — new.
* `android/app/src/main/java/app/suramak/mic/RecordBar.kt` — new (stopgap UI).
* `android/app/src/main/java/app/suramak/transcribe/Notify.kt` — recording
  channel + persistent/status notifications.
* `android/app/src/main/java/app/suramak/spike/MainActivity.kt` — permission
  flow, record bar, Settings route, Quit ordering, mic init.
* `android/app/src/main/AndroidManifest.xml` — RECORD_AUDIO,
  FOREGROUND_SERVICE_MICROPHONE, `microphone` service.
* `android/app/src/main/res/values/strings.xml`, `values-in/strings.xml` — the
  recording copy in EN and ID.
* `android/app/build.gradle.kts` — versionCode 8, versionName `0.4.0-m13f.1`.

**Build**

* `scripts/android/build-apk.sh` — version, RECORD_AUDIO,
  FOREGROUND_SERVICE_MICROPHONE, the recording service + `0x80` type, the mic
  JNI symbol, and the dex-level channel/classes assertions.

**Deliberately untouched:** `crates/suramak-ui`, `crates/suramak-taps`,
`crates/suramak-cli`, the desktop capture tests, the M15 suites, and
`crates/suramak-android-spike` (the four checks and the spike's egui screens).
