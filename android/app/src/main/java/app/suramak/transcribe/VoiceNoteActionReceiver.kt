package app.suramak.transcribe

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.BroadcastReceiver
import android.util.Log
import org.json.JSONObject

/**
 * M22B — the voice-note notification's three non-UI actions.
 *
 * * **Copy** asks the Rust host for the whole cleaned transcript (never the
 *   page) and writes it to the clipboard; the notification then says
 *   `Copied · Page n of m`, so the tap is visible without a toast.
 * * **Next page** re-reads the next page from the same model and re-posts the
 *   notification in place.
 * * **Retry** re-queues the failed job on the copy it still owns and schedules
 *   the foreground pump again; a refusal is posted verbatim.
 *
 * A broadcast receiver owns no UI: every outcome is another notification, and
 * every JNI call goes through the one bridge the rest of the app uses.
 */
class VoiceNoteActionReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context, intent: Intent) {
        val transcriptId = intent.getStringExtra(Notify.EXTRA_TRANSCRIPT_ID)
        val sender = intent.getStringExtra(Notify.EXTRA_SENDER)
        val page = intent.getIntExtra(Notify.EXTRA_PAGE, 0)
        val notificationId = intent.getIntExtra(Notify.EXTRA_NOTIFICATION_ID, Notify.STATUS_ID)
        Notify.ensureChannels(context)
        TranscribeBridge.init(context)
        when (intent.action) {
            Notify.ACTION_COPY -> {
                val id = transcriptId ?: return
                val content = TranscribeBridge.voiceNote(id, page, sender)
                if (!content.optBoolean("ok", false)) {
                    Log.w(TAG, "copy refused: ${content.optString("error")}")
                    return
                }
                val text = content.optString("copy_text")
                val clipboard = context.getSystemService(ClipboardManager::class.java)
                if (clipboard == null) {
                    Log.w(TAG, "the clipboard service is unavailable")
                    return
                }
                clipboard.setPrimaryClip(ClipData.newPlainText("Suramak transcript", text))
                Notify.show(
                    context,
                    notificationId,
                    Notify.voiceNoteReady(context, notificationId, id, content, copied = true),
                )
                Log.i(TAG, "copied ${text.length} chars of the cleaned transcript")
            }
            Notify.ACTION_NEXT_PAGE -> {
                val id = transcriptId ?: return
                val content = TranscribeBridge.voiceNote(id, page + 1, sender)
                if (!content.optBoolean("ok", false)) {
                    Log.w(TAG, "next page refused: ${content.optString("error")}")
                    return
                }
                Notify.show(
                    context,
                    notificationId,
                    Notify.voiceNoteReady(context, notificationId, id, content, copied = false),
                )
            }
            Notify.ACTION_RETRY -> {
                val key = intent.getStringExtra(Notify.EXTRA_KEY) ?: return
                val reply = TranscribeBridge.retry(key)
                if (reply.optBoolean("ok", false)) {
                    val scheduled = TranscriptionJobService.resumePending(context)
                    Log.i(TAG, "retry queued for $key (service scheduled=$scheduled)")
                } else {
                    val reason = reply.optString("error").ifEmpty { "the job could not be retried" }
                    Notify.show(
                        context,
                        Notify.STATUS_ID,
                        Notify.status(context, "Retry not started", reason),
                    )
                    Log.w(TAG, "retry refused: $reason")
                }
            }
        }
    }

    companion object {
        private const val TAG = "SuramakVoiceNote"
    }
}
