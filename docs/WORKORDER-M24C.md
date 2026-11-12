# WORKORDER — M24C: the `Open Transcribe` contract (Android), and v0.6's last slice

Roadmap: `docs/reports/M21-roadmap.md` #8. The idea is Anti-Vocale's, made
honest: another app (Forkgram, a recorder, a share-menu replacement) hands
Suramak audio and receives the transcript back — **without** notification
scraping, media-folder access or a second transcription path.

## The contract

```text
action:  app.suramak.action.TRANSCRIBE
type:    audio/* | video/* (the same mime set the share target accepts)
extras:  android.intent.extra.STREAM   (content:// uri — required)
         app.suramak.extra.LANGUAGE   ("auto" | "en" | "id" | any code — optional)
         app.suramak.extra.SENDER     (display name, optional, never looked up)
         app.suramak.extra.RESULT_INTENT (PendingIntent — required for a reply)
reply:   broadcast, action app.suramak.action.TRANSCRIBE_RESULT
         extras: app.suramak.extra.TRANSCRIPT_ID (uuid string)
                 app.suramak.extra.TRANSCRIPT_TEXT (plain text, speaker labels
                   when diarization ran)
                 app.suramak.extra.ERROR (string, only on failure)
```

## Deliverables

1. **Receiving**: MainActivity recognises the action in `handleLaunchIntent`
   and `onNewIntent`, validates the URI and mime, and enqueues through the
   EXISTING job path with `kind = open_transcribe`. No new service, no new
   queue.
2. **Replying**: when the job finishes (success or named failure), the app
   sends the result broadcast to the caller's `PendingIntent`. The caller's
   permission is re-checked by the platform (PendingIntent send); a missing or
   dead PendingIntent is logged, never a crash. The reply carries the cleaned
   transcript with speaker prefixes when a run exists.
3. **No invisible behaviour**: a 1.2 s sheet is NOT shown — the request is
   explicit and the caller waits for the result; instead the existing
   foreground notification appears (it already shows progress), and the
   transcript lands in the library like any other import. State this in the
   docs.
4. **A docs page** `docs/OPEN-TRANSCRIBE.md`: the contract above, a Kotlin
   snippet for callers (PendingIntent + broadcast receiver), the permission
   reality (the caller needs nothing; Suramak needs no new permission), and
   the honest limits (one job at a time, the model must be downloaded, the
   result arrives only while the process lives — the notification path is the
   fallback).
5. **The desktop equivalent, one line**: the CLI already is the contract
   (`suramak-cli transcribe --db`); link it in the docs, build nothing new.

## Verification

- Host tests for the pure pieces: intent extras → request struct (missing URI,
  unsupported mime, unknown language → `auto`, missing PendingIntent →
  rejected with a named reason); the result payload builder (success with
  speakers, success without, failure with a reason, empty text refused).
- The Android host path gets the NDK check + clippy; a Kotlin/Gradle compile is
  CI's job.
- A `dumpsys`-free emulator step is NOT required; the existing smoke keeps its
  scope.
- fmt/clippy -D warnings; NDK check + clippy on both Android crates; one full
  workspace run (baseline 1417).
- Report: `docs/reports/M24c-open-transcribe.md`.

## Do NOT

- No NotificationListener, no media-folder scan, no MANAGE_EXTERNAL_STORAGE,
  no new permission at all; no second transcription path; no cloud; no new
  crates; no CI edits beyond none; no git commit/push; no sudo.
