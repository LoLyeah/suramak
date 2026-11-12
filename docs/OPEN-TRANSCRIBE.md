# Open Transcribe

An explicit contract: another app hands Suramak one audio or video file and
receives the transcript back through its **own** `PendingIntent`. No
notification scraping, no media-folder access, no second transcription path —
the request enters the same queue, the same model and the same library as a
share.

```
caller ──ACTION_TRANSCRIBE + content:// uri + PendingIntent──▶ Suramak
                                                                 │ the one job queue
                                                                 │ (kind = open_transcribe)
caller ◀──ACTION_TRANSCRIBE_RESULT + TRANSCRIPT_TEXT / ERROR─────┘  broadcast
```

The contract's rules live in `crates/suramak-android/src/open_transcribe.rs`
and are host-tested; the activity that receives the intent is
`MainActivity.acceptOpenTranscribe` and the reply is sent by
`TranscriptionJobService`. This page is the caller's side of the same thing.

---

## 1. The request

| | |
| --- | --- |
| action | `app.suramak.action.TRANSCRIBE` |
| type | `audio/*` or `video/*` (the share target's matrix: `audio/ogg`, `audio/mpeg`, `audio/mp4`, `audio/webm`, `audio/opus`, `application/ogg`, `application/x-ogg`, `video/mp4`, `video/webm`, `video/x-matroska`) |
| `android.intent.extra.STREAM` | a `content://` uri — **required**, exactly one |
| `app.suramak.extra.LANGUAGE` | `auto` or a two-letter code — optional, default `auto` |
| `app.suramak.extra.SENDER` | a display name — optional, shown in the notification, never looked up |
| `app.suramak.extra.RESULT_INTENT` | a broadcast `PendingIntent` — **required** |

Rules the parser enforces before a job exists (each refusal is a named
sentence, not a silent drop):

| Input | Result |
| --- | --- |
| no `STREAM`, or a blank one | refused: `missing_stream` |
| `file://` or a bare path | refused: `not_a_content_uri` — the import reads a file descriptor, so a path Suramak cannot open under its sandbox would only fail later |
| a mime the import would refuse (`text/plain`, `application/octet-stream`, `*/*` with no provider type) | refused: `unsupported_mime` |
| no `RESULT_INTENT` | refused: `missing_result_intent` — a transcript nobody can receive is work the caller did not ask for |
| `LANGUAGE` = `EN`, `id-ID`, `en_US` | honoured as `en` / `id` / `en` (the region suffix is dropped — whisper's selector is the two-letter code) |
| `LANGUAGE` = `English`, `fil`, `banana` | `auto`; the raw tag goes to the app log, never into a guess |
| anything else | accepted |

A refusal that still carries a `RESULT_INTENT` is answered immediately with the
reason in `ERROR`; a refusal without one can only be logged (and shown as a
toast if the app is in the foreground).

`ACTION_SEND_MULTIPLE` is **not** part of this contract: one request is one
file. Send N requests (they queue) rather than inventing a batch shape.

## 2. The reply

| | |
| --- | --- |
| action | `app.suramak.action.TRANSCRIBE_RESULT` |
| `app.suramak.extra.TRANSCRIPT_ID` | the library row's uuid — success only |
| `app.suramak.extra.TRANSCRIPT_TEXT` | the plain text — success only |
| `app.suramak.extra.ERROR` | Suramak's own sentence — failure only |

* **Success** carries `TRANSCRIPT_ID` *and* `TRANSCRIPT_TEXT`.
* **Failure** carries `ERROR` and no text. The reason is the queue's own
  sentence (`the audio track is empty`, `the copied file is 0 bytes but the
  source declared 4096`, `the tiny model is not installed on this phone yet`),
  never a generic "something went wrong".
* **The text** is the cleaned transcript: one line per segment, whitespace
  collapsed, no timestamps. When the stored row has a diarization run, each
  line is prefixed `Speaker: ` (or the owner's name) — see the limits below,
  because a fresh import has no run.
* **An empty transcript is a failure**, not an empty success: the reply says
  `the transcript is empty — there is no text to reply with`.
* **The reply is a broadcast, not an activity result.** There is no
  `startActivityForResult`; the caller may have no visible activity when the
  job finishes.

## 3. A caller, in Kotlin

```kotlin
// The receiver. `exported="false"` is enough: a PendingIntent executes as its
// creator, so the platform delivers this broadcast as *your* app.
class TranscribeResultReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != "app.suramak.action.TRANSCRIBE_RESULT") return
        val tag = intent.getStringExtra("my.request.tag")   // your own extra survives
        val error = intent.getStringExtra("app.suramak.extra.ERROR")
        if (error != null) {
            Log.w("Transcribe", "request $tag failed: $error")
            return
        }
        val id = intent.getStringExtra("app.suramak.extra.TRANSCRIPT_ID")
        val text = intent.getStringExtra("app.suramak.extra.TRANSCRIPT_TEXT") ?: return
        Log.i("Transcribe", "request $tag → transcript $id:\n$text")
    }
}

fun askSuramak(context: Context, audio: Uri, tag: String) {
    // Your own extras ride along: Suramak adds only its three names, and the
    // base intent's extras win on a collision.
    val replyIntent = Intent(context, TranscribeResultReceiver::class.java)
        .setAction("app.suramak.action.TRANSCRIBE_RESULT")
        .putExtra("my.request.tag", tag)

    val reply = PendingIntent.getBroadcast(
        context,
        tag.hashCode(),                                   // one requestCode per in-flight request
        replyIntent,
        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE,
    )

    val request = Intent("app.suramak.action.TRANSCRIBE")
        .setType(context.contentResolver.getType(audio) ?: "audio/*")
        .putExtra(Intent.EXTRA_STREAM, audio)
        .putExtra("app.suramak.extra.LANGUAGE", "auto")     // optional
        .putExtra("app.suramak.extra.SENDER", "Forkgram")   // optional, display only
        .putExtra("app.suramak.extra.RESULT_INTENT", reply)
        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)

    // The platform only migrates EXTRA_STREAM into ClipData for ACTION_SEND, so
    // a custom action must put the uri there itself or the read grant does not
    // attach to it.
    request.clipData = ClipData.newRawUri("audio", audio)

    // From a non-Activity context add Intent.FLAG_ACTIVITY_NEW_TASK.
    context.startActivity(request)
}
```

Manifest, in the caller:

```xml
<queries>
    <!-- Android 11+ package visibility: without this the implicit intent does
         not resolve and startActivity throws ActivityNotFoundException. -->
    <intent>
        <action android:name="app.suramak.action.TRANSCRIBE" />
    </intent>
</queries>

<receiver
    android:name=".TranscribeResultReceiver"
    android:exported="false" />
```

Three things that quietly break a caller:

1. **`FLAG_IMMUTABLE` drops the extras.** A mutable `PendingIntent` is what
   lets Suramak fill in the action and the three extras; with an immutable one
   the receiver gets a broadcast with nothing in it. This is the platform's
   rule, not Suramak's.
2. **Android 14+ forbids mutable implicit `PendingIntent`s.** The snippet's
   `Intent(context, TranscribeResultReceiver::class.java)` is explicit (a
   component), which is what the platform requires.
3. **The read grant needs `ClipData`.** `FLAG_GRANT_READ_URI_PERMISSION` alone
   covers the intent's data uri and its `ClipData`; `EXTRA_STREAM` is only
   auto-migrated for `ACTION_SEND`. Without the `clipData` line the copy fails
   with a named `SecurityException` reason.

## 4. What the user sees

No sheet. A share shows the 1.2 s voice-note sheet because the *app* has to
pick a language; an Open Transcribe request has already picked one (or accepted
`auto`), so a sheet would be a question the caller cannot answer. Instead:

* the existing **foreground notification** appears with the file name and the
  real decode-ahead percent (the same one a share gets), and its Cancel action
  works;
* the transcript lands in **Suramak's library** like any other import — the
  caller receives a copy, not an exclusive result;
* the completion notification stays, with its tap-to-open row.

## 5. Permissions — the honest version

* **The caller needs no permission.** It grants Suramak read access to one uri
  it already owns (`FLAG_GRANT_READ_URI_PERMISSION`) and receives a broadcast
  it sent to itself. Receiving it needs no permission, and the receiver may be
  `exported="false"`.
* **Suramak needs no new permission.** This milestone added two intent filters
  to the manifest (one for the typed request, one for a caller that leaves the
  type unset) and nothing else. There is no `NotificationListenerService`,
  no media-folder scan, no `MANAGE_EXTERNAL_STORAGE`, no `READ_MEDIA_AUDIO`,
  and no new runtime prompt. The permissions the app already had
  (`FOREGROUND_SERVICE`, `WAKE_LOCK`, `POST_NOTIFICATIONS`) are the ones a
  share already uses.
* **The reply's permission check is the platform's.** `PendingIntent.send`
  executes the broadcast under the caller's identity, so Suramak cannot send it
  anywhere the caller could not send it itself, and a caller that cancelled its
  `PendingIntent` gets nothing (logged, no crash).

## 6. Limits — the ones a caller must design around

1. **One job at a time.** Suramak has a single inference slot. A request that
   arrives while a job runs waits behind it (and behind any earlier requests).
   There is no priority, no queue position and no caller-side cancel; the only
   progress is the notification.
2. **The model must be installed.** With no speech model the reply is an
   `ERROR` (`Download a speech model first — transcribing needs one on this
   phone.`) and no job is scheduled. There is no download-on-demand in this
   contract.
3. **The reply lives only as long as Suramak's process.** The `PendingIntent`
   is held in memory (it cannot be put in a `JobInfo`). If Android kills
   Suramak mid-job, the job resumes on the next launch and the transcript is
   still written — but the caller's route is gone, so **no broadcast arrives**.
   Treat a reply as best-effort with a timeout.
4. **The notification is the fallback.** Whenever the reply cannot be
   delivered — dead `PendingIntent`, restarted process, a build with no reply
   route — the completion notification and the library row are still there.
   That is the documented degradation, not a failure to hide.
5. **The request opens Suramak's window.** Android delivers an activity action
   to an activity; there is no headless receiver in this contract, because a
   background broadcast receiver cannot start a foreground service on Android
   12+ without an exemption, and adding one would be a second start path.
   Expect Suramak to come to the front for the handoff.
6. **Plain text only.** No timestamps, no segments, no JSON, no per-word
   timings. `TRANSCRIPT_ID` is Suramak's own row id — useful for correlation,
   not an API to read the row.
7. **Speaker labels are conditional.** The reply prefixes `Speaker: ` only
   when the stored row already has a diarization run. A fresh import does not:
   diarization is a separate, user-started step (and its Android runtime gate
   is still owner-device, `docs/reports/M23b-android-diarization-gate.md`). In
   practice an Open Transcribe reply is plain text; the prefix path is built
   and tested for the rows that do have a run.
8. **No request id in the reply.** The contract carries none. Correlate with
   your own extra on the `PendingIntent`'s base intent (it survives the
   fill-in), or with one distinct `PendingIntent` per in-flight request.
9. **One file per request, and the copy happens immediately.** The uri grant is
   tied to the receiving activity; the import starts as soon as the service
   starts, which is what makes a long job safe. A caller that revokes the grant
   in that window gets a named failure.

## 7. The desktop equivalent

The desktop already is this contract, with no socket and no daemon:

```sh
suramak-cli transcribe clip.ogg --db
```

One file (or several, queued) in, the transcript out, `--db` to also write the
library row. It is the same engine and the same library as the app; it is a
different *process*, so its queue is its own — a GUI run and a CLI run do not
share the one inference slot. Nothing new was built for the desktop in this
milestone.

## 8. Where this lives

| Piece | File |
| --- | --- |
| action/extra names, request parse, language rule, result payload | `crates/suramak-android/src/open_transcribe.rs` |
| the stored row → reply text (speaker prefixes) | `TranscribeBridge.openTranscribeResult` → `JobHost::open_transcribe_result` |
| receiving the intent | `MainActivity.acceptOpenTranscribe` (`handleLaunchIntent`, so `onNewIntent` lands there too) |
| the reply | `TranscriptionJobService.replyOpenTranscribe` |
| the request-id → `PendingIntent` map | `app/suramak/transcribe/OpenTranscribe.kt` |
