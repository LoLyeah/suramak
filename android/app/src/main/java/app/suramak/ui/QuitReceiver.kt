package app.suramak.ui

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.Process
import android.util.Log
import app.suramak.mic.MicBridge
import app.suramak.mic.RecordingService
import app.suramak.transcribe.Notify
import app.suramak.transcribe.TranscribeBridge
import app.suramak.transcribe.TranscriptionJobService

/**
 * M13g1 — the foreground-service notification's **Quit** action.
 *
 * The owner's Quit contract (M13F) is an order, not a set of calls:
 *
 * 1. **capture first** — `MicBridge.quit()` stops, finalizes and hands off a
 *    live take synchronously; nothing is lost;
 * 2. **jobs second** — the transcription host pauses every live job (the copy
 *    and the journal stay, so the next launch offers Resume) and the
 *    scheduler's jobs are cancelled;
 * 3. **services third** — the recording and transcription services stop;
 * 4. **exit last** — the process ends, which also removes the persistent
 *    notification.
 *
 * A broadcast receiver, not an Activity: Quit must work from the lock screen
 * and while no Activity exists. The steps live in one function so the order is
 * reviewable; `scripts/android/build-apk.sh` asserts their source order.
 */
class QuitReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Notify.ACTION_QUIT) return
        quit(context)
    }

    companion object {
        private const val TAG = "SuramakQuit"

        /** Executes the owner's ordering. Never throws into the platform. */
        fun quit(context: Context) {
            // The hosts must exist in a process the broadcast just woke; this
            // is setup, not part of the ordering below.
            try {
                TranscribeBridge.init(context)
                MicBridge.init(context)
            } catch (t: Throwable) {
                Log.w(TAG, "the hosts could not be initialised", t)
            }

            // 1. capture first: stop/finalize/hand off before anything else.
            try {
                val reply = MicBridge.quit()
                Log.i(TAG, "capture stopped: ${reply.optBoolean("quit")}")
            } catch (t: Throwable) {
                Log.w(TAG, "capture could not be stopped", t)
            }

            // 2. jobs second: pause the live work (recoverable) and unschedule.
            try {
                TranscribeBridge.init(context)
                val paused = TranscribeBridge.dispatch(org.json.JSONObject().put("op", "quit"))
                Log.i(TAG, "jobs paused: ${paused.optInt("paused")}")
            } catch (t: Throwable) {
                Log.w(TAG, "the job host could not be paused", t)
            }

            // 3. services third.
            try {
                RecordingService.stop(context)
                context.stopService(Intent(context, RecordingService::class.java))
                TranscriptionJobService.shutdown(context)
            } catch (t: Throwable) {
                Log.w(TAG, "a service could not be stopped", t)
            }

            // 4. exit last. Nothing below this point may allocate state that a
            //    crash reporter would read as a failure: the take is final.
            Notify.clearAll(context)
            Process.killProcess(Process.myPid())
        }
    }
}
