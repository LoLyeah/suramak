# M24C — Open Transcribe

**Status:** done. Another app can ask Suramak to transcribe one file and receives
the transcript through its own `PendingIntent`. The request is parsed in Rust
(host-tested), queued as `kind = open_transcribe` through the **existing** import
job, and answered by the existing foreground service. No permission was added,
no notification was scraped, no folder was scanned, and there is one
transcription path.

---

## 1. The contract

```text
action:  app.suramak.action.TRANSCRIBE
type:    audio/* | video/*  (the share target's matrix)
extras:  android.intent.extra.STREAM            content:// uri   (required)
         app.suramak.extra.LANGUAGE             auto|en|id|…     (optional)
         app.suramak.extra.SENDER               display name     (optional)
         app.suramak.extra.RESULT_INTENT        PendingIntent    (required)

reply:   broadcast app.suramak.action.TRANSCRIBE_RESULT
         app.suramak.extra.TRANSCRIPT_ID        uuid (success)
         app.suramak.extra.TRANSCRIPT_TEXT      text (success)
         app.suramak.extra.ERROR                reason (failure only)
```

`docs/OPEN-TRANSCRIBE.md` is the caller's page: the table above, a complete
Kotlin caller (receiver + `PendingIntent` + request), the manifest `<queries>`
element, the three traps (`FLAG_IMMUTABLE` drops the extras, Android 14 forbids
mutable *implicit* `PendingIntent`s, the read grant needs `ClipData`), the
permission reality and the limits.

## 2. Receiving — one activity, one queue

`MainActivity.handleLaunchIntent` matches `ACTION_TRANSCRIBE` and calls
`acceptOpenTranscribe`. `onNewIntent` already delegates to `handleLaunchIntent`,
so a second request while the task is alive takes the same path; there is no
second handler to drift.

`acceptOpenTranscribe` reads the platform extras and asks the host to parse them
(`TranscribeBridge.openTranscribeRequest` → `op: open_transcribe_request`). The
rules are Rust's and host-tested:

| Input | Result |
| --- | --- |
| missing / blank `STREAM` | refused `missing_stream` |
| `file://` or a bare path | refused `not_a_content_uri` |
| mime outside `jobs::is_supported_mime` (the import's own table) | refused `unsupported_mime` |
| missing `RESULT_INTENT` | refused `missing_result_intent` — before any job exists |
| `EN`, `id-ID`, `en_US` | `en`, `id`, `en` (the region suffix is dropped) |
| `English`, `fil`, `banana` | `auto`, and the raw tag is returned in `ignored_language` for the log |
| absent / `auto` | `auto` |

A refusal with a live `PendingIntent` is answered with the reason in `ERROR`; a
refusal without one is logged (and toasted when the app is in front). A refusal
never creates a journal entry or a queue item — pinned by
`the_open_transcribe_protocol_parses_refuses_and_builds_the_reply`
(`host.status().total == 0`).

On success the Activity takes a request id (`UUID`), stores the caller's
`PendingIntent` in `OpenTranscribe.replies`, and calls the existing
`TranscriptionJobService.enqueue(..., KIND_OPEN_TRANSCRIBE, sender, requestId)`.
The job travels the M13e/M22B path unchanged: `begin_import` with the kind,
bounded copy progress, the one inference slot, `import_done`, the store write.

## 3. Replying — once, and never a crash

`TranscriptionJobService.replyOpenTranscribe` is the single reply helper. It is
called from:

| Where | Payload |
| --- | --- |
| the success branch of `complete()` (kind = open_transcribe) | `open_transcribe_result(id)` — the stored row's cleaned text |
| the failure branch, only when `last_failed.key` is **this run's** import key | `open_transcribe_failure(reason)` — the queue's sentence |
| the cancel branch | a named "cancelled before it finished" reason |
| `onStopJob` / `onTimeout` (the pause contract) | a named "the system stopped it; the copy is kept" reason |
| `onStartJob` with no model installed | the no-model sentence |
| a final belt at the end of `complete()` | `lastError` / the status message, so no path can end silently |

Properties this buys:

* **Exactly one reply.** `openTranscribeReplied` is set under `replyLock` before
  the send; every later call returns immediately.
* **Never a blank success.** A payload the host refuses (a stored row that reads
  back empty, an unreadable library) is converted into a failure reply carrying
  the host's sentence.
* **A dead `PendingIntent` is a log line.** `OpenTranscribe.send` catches
  `PendingIntent.CanceledException` (and anything else) and returns false.
* **A missing route is a log line too.** A journal job resumed after a restart
  has no request id (the `PendingIntent` cannot be serialised into a `JobInfo`),
  which is the documented process-lifetime limit.
* **No stale reason.** The failure branch matches `last_failed.key` against this
  run's `begin_import` key, so an older failure in the same process can never be
  reported as this caller's reason.

The text is not re-rendered: `open_transcribe_result` calls
`voice_note::build_transcript_notification(...).copy_text`, the exact string the
notification's Copy action carries — one cleaning rule (whitespace only) and one
speaker-prefix rule (`Speaker 1` or the owner's name, only when a run exists).

## 4. No invisible behaviour

No sheet. A share shows the 1.2 s voice-note sheet because the app must choose a
language; an Open Transcribe request already chose one. The user sees the
existing foreground notification with real progress and a working Cancel, and
the transcript lands in the library like any other import. Both facts are stated
in `docs/OPEN-TRANSCRIBE.md` §4, together with the notification being the
fallback whenever a reply cannot be delivered.

## 5. Verification

### Host tests — 17 new

`crates/suramak-android/src/open_transcribe.rs` (16, pure):

* `a_well_formed_request_becomes_the_contract_request`
* `a_request_without_a_stream_is_refused_by_name`
* `a_file_path_is_not_a_content_uri`
* `an_unsupported_mime_is_refused_with_the_type_the_caller_sent`
* `the_share_matrix_is_the_contract_matrix`
* `a_request_without_a_result_intent_is_refused_before_a_job_exists`
* `a_foreign_action_is_refused`
* `an_unknown_language_falls_back_to_auto_and_is_named`
* `a_blank_sender_is_absent_and_never_a_guess`
* `a_success_payload_carries_the_id_and_the_text`
* `the_payload_carries_speaker_prefixes_only_when_diarization_ran`
* `an_empty_transcript_is_refused_not_sent_as_a_blank_result`
* `a_success_payload_needs_an_id`
* `a_failure_payload_carries_the_reason_and_never_text`
* `the_extras_serialise_under_the_contract_names`
* `the_contract_names_are_the_documented_ones`

`crates/suramak-android/tests/jobs_host.rs` (1, protocol):

* `the_open_transcribe_protocol_parses_refuses_and_builds_the_reply` — the four
  op names Kotlin sends, the JSON key names, the `ID` → `id` case, the
  `English` → `auto` + `ignored_language` fallback, the refusal codes, and both
  payload shapes including the refused empty reason.

The existing `the_manifest_declares_the_share_matrix_and_the_service` gained one
needle: `app.suramak.action.TRANSCRIBE`.

### Commands

| Check | Result |
| --- | --- |
| `cargo fmt --all --check` | clean |
| `cargo clippy -p suramak-android --all-targets -- -D warnings` | clean |
| `cargo test --workspace` | **1434 passed, 0 failed** (baseline 1417 + 17) |
| `cargo ndk -t arm64-v8a --platform 29 check -p suramak-android -p suramak-android-spike` | clean |
| `cargo ndk -t arm64-v8a --platform 29 clippy -p suramak-android -- -D warnings` | clean |
| `cargo ndk … clippy -p suramak-android-spike --all-targets -- -D warnings` | clean |
| `./gradlew --offline :app:compileDebugKotlin` | **BUILD SUCCESSFUL** |

The Kotlin compile was run here (the local SDK 35 + Gradle 8.9 + the populated
Gradle cache made it cheap offline), which is stronger than the workorder's
"CI's job": `allWarningsAsErrors = true` means the new Kotlin has no warnings
either. What it does **not** prove is runtime behaviour — see §8.

## 6. Limits (also in the docs page)

1. **One job at a time** — a single inference slot; requests queue, no priority,
   no caller-side cancel.
2. **The model must be installed** — otherwise the reply is an `ERROR` and no
   job is scheduled.
3. **The reply is process-lifetime** — a killed process loses the route; the
   job resumes and the row is written, but no broadcast arrives.
4. **The notification is the fallback** — always posted, always tappable.
5. **The request opens Suramak's window** — Android delivers an activity action
   to an activity; a background receiver cannot start the FGS on 12+.
6. **Plain text only** — no timestamps, no segments, no JSON.
7. **Speaker prefixes are conditional** — a fresh import has no diarization run
   (it is a separate, user-started step), so the reply is plain text in
   practice. The prefix path is built and tested for rows that have a run.
8. **No request id in the reply** — the contract carries none; callers correlate
   with their own extra on the `PendingIntent`'s base intent (it survives the
   fill-in) or with one `PendingIntent` per request.

## 7. Files

| File | Change |
| --- | --- |
| `crates/suramak-android/src/open_transcribe.rs` | new: names, request parse, language rule, result payload, 16 tests |
| `crates/suramak-android/src/lib.rs` | module declared |
| `crates/suramak-android/src/jobs.rs` | `open_transcribe_result` + three dispatch ops; kind doc |
| `crates/suramak-android/tests/jobs_host.rs` | protocol test + manifest needle |
| `android/.../spike/MainActivity.kt` | action constants, `acceptOpenTranscribe`, dispatch case |
| `android/.../transcribe/OpenTranscribe.kt` | new: contract names mirror, `PendingIntent` map, `send` |
| `android/.../transcribe/TranscribeBridge.kt` | three ops |
| `android/.../transcribe/TranscriptionJobService.kt` | kind, reply token, `replyOpenTranscribe` + six call sites |
| `android/app/src/main/AndroidManifest.xml` | the action's intent filter (mime matrix + untyped) — no permission |
| `android/app/src/main/res/values{,-in}/strings.xml` | two failure sentences, both locales |
| `docs/OPEN-TRANSCRIBE.md` | new: the caller's contract page |
| `docs/reports/M24c-open-transcribe.md` | this report |

## 8. What this does not prove

* **A real cross-app round trip.** There is no emulator step in this milestone
  (`dumpsys`-free by the workorder), so the receive → job → broadcast path has
  not been exercised between two installed apps. The Kotlin compiles and every
  rule it calls is host-tested; the `PendingIntent` hand-off itself is the
  platform's contract, including the `FLAG_MUTABLE` requirement, which is
  documented from the platform's own rule rather than observed on a device.
* **Diarized replies on a phone.** The prefix path is tested with a synthetic
  run; the Android diarization runtime gate is still owner-device
  (`docs/reports/M23b-android-diarization-gate.md`).
* **A caller dying mid-job.** The `CanceledException` path is caught, not
  reproduced.
