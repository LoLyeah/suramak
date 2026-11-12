package app.suramak.transcribe

import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.util.Log
import org.json.JSONObject
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap

/**
 * M24C — the Open Transcribe contract's Kotlin half.
 *
 * The contract itself (action, extras, request rules, reply payload) lives in
 * `crates/suramak-android/src/open_transcribe.rs`, where it is host-tested; this
 * object owns the two things Rust cannot hold:
 *
 * * **the caller's `PendingIntent`s**, one per request id, in a process-wide
 *   map. A `PendingIntent` cannot be serialized into a `JobInfo` and must not
 *   be: the reply is a process-lifetime promise (the docs say so), and a
 *   restarted process has no way to reach the caller's receiver.
 * * **the send**, which is the platform's own permission check: the broadcast
 *   runs as the caller, so Suramak needs no permission to send it and the
 *   caller needs none to receive it.
 *
 * A missing, spent or cancelled `PendingIntent` is logged and dropped — the
 * transcript is already in the library and the completion notification is the
 * documented fallback.
 */
object OpenTranscribe {

    private const val TAG = "SuramakOpenTranscribe"

    // Mirrors of the Rust constants (`open_transcribe.rs`). They are the wire
    // contract another app is written against: change them in both places or
    // not at all.
    const val ACTION = "app.suramak.action.TRANSCRIBE"
    const val ACTION_RESULT = "app.suramak.action.TRANSCRIBE_RESULT"
    const val EXTRA_STREAM = "android.intent.extra.STREAM"
    const val EXTRA_LANGUAGE = "app.suramak.extra.LANGUAGE"
    const val EXTRA_SENDER = "app.suramak.extra.SENDER"
    const val EXTRA_RESULT_INTENT = "app.suramak.extra.RESULT_INTENT"
    const val KIND = "open_transcribe"

    /**
     * request id → the caller's reply. Populated by the Activity before the job
     * is scheduled, consumed once by the service.
     */
    private val replies = ConcurrentHashMap<String, PendingIntent>()

    /** A fresh request id. Opaque to the caller: it never sees this value. */
    fun newRequestId(): String = UUID.randomUUID().toString()

    /** Keeps the caller's reply until the job finishes (or the process dies). */
    fun hold(requestId: String, reply: PendingIntent) {
        replies[requestId] = reply
    }

    /** Takes the reply exactly once; a second call returns null. */
    fun take(requestId: String): PendingIntent? = replies.remove(requestId)

    /** Drops a reply that will never be used (the job could not be scheduled). */
    fun forget(requestId: String) {
        replies.remove(requestId)
    }

    /**
     * Sends one host-built payload (`{ok, action, extras}`) through the
     * caller's `PendingIntent`.
     *
     * The fill-in `Intent` carries the action and the extras: the caller's
     * `PendingIntent` must be **mutable** (the docs' one hard requirement) or
     * the platform drops them. `CanceledException` is the caller having died or
     * cancelled — a log line, never a crash.
     */
    fun send(context: Context, reply: PendingIntent, payload: JSONObject): Boolean {
        val intent = Intent(payload.optString("action").ifEmpty { ACTION_RESULT })
        val extras = payload.optJSONObject("extras") ?: JSONObject()
        for (name in extras.keys()) {
            intent.putExtra(name, extras.optString(name))
        }
        return try {
            reply.send(context, 0, intent)
            true
        } catch (e: PendingIntent.CanceledException) {
            Log.w(TAG, "the caller's PendingIntent is cancelled; the result stays in the library", e)
            false
        } catch (t: Throwable) {
            // A reply must never take the service down: the row is saved and
            // the notification already shows it.
            Log.w(TAG, "the result broadcast could not be sent", t)
            false
        }
    }
}
