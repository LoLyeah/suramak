package app.suramak.models

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * M13d — the notification's **Cancel** action.
 *
 * A broadcast rather than a service call: the job's service instance is owned
 * by the JobScheduler and is not guaranteed to exist when the user taps (the
 * system may have recreated it, or the notification may outlive a process
 * restart). The receiver's only job is to ask the Rust host to stop and to
 * clear the progress notification. The bytes already written stay in
 * `<model>.bin.part`, so the next start resumes.
 */
class CancelModelDownloadReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != ModelDownloadJobService.ACTION_CANCEL) {
            return
        }
        try {
            ModelsBridge.cancel()
        } catch (t: Throwable) {
            // The library may not be loaded in a fresh process that only got
            // the broadcast; clearing the notification is still correct.
            android.util.Log.w("SuramakModels", "cancel could not reach the model host", t)
        }
        ModelDownloadJobService.clearProgressNotification(context)
    }
}
