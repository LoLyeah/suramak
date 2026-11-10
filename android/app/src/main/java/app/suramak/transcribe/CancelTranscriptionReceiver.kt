package app.suramak.transcribe

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log

/**
 * M13e — the notification's Cancel action.
 *
 * A broadcast (not an activity) because the notification must work while the
 * app is in the background: the tap flips the running job's cooperative token
 * in the Rust host, the decode/engine stops at its next boundary, the copied
 * media is deleted and no library row is written. The service's poll loop sees
 * the terminal state and removes the progress notification.
 *
 * No key in the intent means the notification was rebuilt while nothing was
 * running: pause everything live rather than leaving an uncancellable job.
 */
class CancelTranscriptionReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Notify.ACTION_CANCEL) return
        try {
            TranscribeBridge.init(context)
            val key = intent.getStringExtra(Notify.EXTRA_KEY)
                ?: TranscribeBridge.status().optString("current_key").ifEmpty { null }
            if (key != null) {
                TranscribeBridge.cancel(key)
            }
        } catch (t: Throwable) {
            // A receiver that throws is an ANR/crash in the background where
            // the user cannot see it; the job's own timeout covers the rest.
            Log.w("SuramakTranscribe", "cancel did not reach the job host", t)
        }
    }
}
