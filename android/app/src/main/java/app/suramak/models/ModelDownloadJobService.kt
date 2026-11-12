package app.suramak.models

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.app.job.JobInfo
import android.app.job.JobParameters
import android.app.job.JobScheduler
import android.app.job.JobService
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.graphics.drawable.Icon
import android.os.Handler
import android.os.Looper
import android.os.PersistableBundle
import android.util.Log
import app.suramak.spike.R
import org.json.JSONObject
import java.io.File

/**
 * M13d — the model download runs in a `JobService`, not in the Activity.
 *
 * **Why a job at all.** The Activity can be destroyed (rotation, process death,
 * the user leaving) while a 488 MB download is in flight. A JobScheduler job
 * survives the Activity, is retried by the platform, and — because the transfer
 * itself is resumable at the library layer — resumes from its `.part` bookmark
 * after a hard process kill.
 *
 * **"dataSync".** JobScheduler has no per-job `dataSync` type in API 35 (checked
 * against the installed `android.jar`). The type lives on the service
 * declaration (`android:foregroundServiceType="dataSync"`), which is also where
 * Android 15's six-hour limit and `Service.onTimeout` apply. The job therefore
 * promotes itself to a `dataSync` foreground service with
 * [Service.startForeground] and implements [onTimeout] for the limit.
 *
 * **The progress notification is deterministic.** Rust reports integer bytes;
 * this class derives `percent = bytes * 100 / total` (never a clock, never an
 * estimate), updates only when the percent changes, and posts one final
 * notification. Cancel is a broadcast to [CancelModelDownloadReceiver], which
 * asks Rust to stop — the bytes already written stay on disk.
 *
 * **Failure is quiet but visible.** Any refusal (below the 4 GB floor, a model
 * this phone cannot run, not enough free space) is posted as a normal
 * notification and the job ends without touching the network.
 */
class ModelDownloadJobService : JobService() {

    /** The worker polls the Rust host; the job itself owns no transfer state. */
    private var worker: Thread? = null

    /** Kept so `onTimeout`/`onStopJob` can finish the job they were given. */
    @Volatile
    private var activeParams: JobParameters? = null

    override fun onCreate() {
        super.onCreate()
        ensureChannel(this)
    }

    override fun onStartJob(params: JobParameters): Boolean {
        val model = params.extras.getString(KEY_MODEL) ?: return false
        val language = params.extras.getString(KEY_LANGUAGE) ?: "en"
        val modelsDir = params.extras.getString(KEY_MODELS_DIR)
            ?: File(filesDir, "models").absolutePath

        ensureChannel(this)

        val started = try {
            // The probe is what registers the device tier with the Rust host;
            // without it every download is refused (fail closed, never guess).
            ModelsBridge.memory(this, language)
            ModelsBridge.start(model, language, modelsDir)
        } catch (t: Throwable) {
            JSONObject()
                .put("ok", false)
                .put("refused", t.message ?: t.javaClass.simpleName)
        }

        if (!started.optBoolean("ok", false)) {
            postStatus(
                getString(R.string.model_download_refused),
                started.optString("refused").ifEmpty { getString(R.string.model_download_failed) },
            )
            return false
        }

        activeParams = params
        try {
            startForeground(
                NOTIFICATION_ID,
                buildProgress(model, 0, 0, started.optLong("bytes_total")),
                ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC,
            )
        } catch (t: Throwable) {
            // A missing FGS permission must not lose the download: the job
            // still runs in the background, and the notification below is
            // posted normally.
            Log.w(TAG, "could not promote the job to a dataSync foreground service", t)
        }

        worker = Thread({ poll(model) }, "suramak-model-download").also { it.start() }
        return true
    }

    /** Polls the host and keeps the notification in step with it. */
    private fun poll(model: String) {
        var lastPercent = -1
        var lastState = ""
        while (true) {
            val progress = try {
                ModelsBridge.progress()
            } catch (t: Throwable) {
                finish("failed", errorMessage(t))
                return
            }
            val state = progress.optString("state", "failed")
            val percent = progress.optInt("percent", 0)
            if (state == "running" && percent != lastPercent) {
                lastPercent = percent
                notifyProgress(
                    model,
                    percent,
                    progress.optLong("bytes_done"),
                    progress.optLong("bytes_total"),
                )
            }
            if (state != lastState) {
                lastState = state
                Log.i(TAG, "model $model: $state ($percent%)")
            }
            if (state != "running") {
                finish(state, progress.optString("message"))
                return
            }
            try {
                Thread.sleep(POLL_MS)
            } catch (e: InterruptedException) {
                Thread.currentThread().interrupt()
                return
            }
        }
    }

    private fun finish(state: String, message: String) {
        stopForeground(STOP_FOREGROUND_REMOVE)
        when (state) {
            "done" -> postStatus(
                getString(R.string.model_download_done),
                getString(R.string.model_download_done_body),
            )
            "cancelled" -> postStatus(
                getString(R.string.model_download_paused),
                getString(R.string.model_download_paused_body),
            )
            else -> postStatus(getString(R.string.model_download_failed), message)
        }
        val params = activeParams
        activeParams = null
        if (params != null) {
            // jobFinished is documented as callable from any thread, but the
            // scheduler's own callbacks run on the main thread — match that.
            Handler(Looper.getMainLooper()).post { jobFinished(params, false) }
        }
    }

    /**
     * The system is stopping the job (quota, constraints lost, app removed).
     * Keep the `.part` and ask for a retry: the next run resumes the bookmark.
     */
    override fun onStopJob(params: JobParameters): Boolean {
        cancelQuietly()
        stopForeground(STOP_FOREGROUND_REMOVE)
        worker?.interrupt()
        worker = null
        activeParams = null
        // Ask the platform to retry: the partial file is the bookmark, so the
        // next run resumes rather than restarting.
        return true
    }

    /**
     * Android 15's `dataSync` foreground-service limit (six hours) lands here.
     * It is far beyond any model download, but the contract is the same as a
     * cancel: stop cleanly, keep the bookmark, and let the next open resume.
     */
    override fun onTimeout(startId: Int) {
        cancelQuietly()
        stopForeground(STOP_FOREGROUND_REMOVE)
        postStatus(
            getString(R.string.model_download_timed_out),
            getString(R.string.model_download_paused_body),
        )
        val params = activeParams
        activeParams = null
        if (params != null) {
            jobFinished(params, true)
        }
    }

    override fun onDestroy() {
        cancelQuietly()
        stopForeground(STOP_FOREGROUND_REMOVE)
        super.onDestroy()
    }

    /** A cancel must never turn a lifecycle callback into a crash. */
    private fun cancelQuietly() {
        try {
            ModelsBridge.cancel()
        } catch (t: Throwable) {
            Log.w(TAG, "cancel could not reach the model host", t)
        }
    }

    // --- notifications -------------------------------------------------------

    private fun buildProgress(model: String, percent: Int, done: Long, total: Long): Notification {
        val cancel = PendingIntent.getBroadcast(
            this,
            0,
            Intent(this, CancelModelDownloadReceiver::class.java).setAction(ACTION_CANCEL),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val text = if (total > 0) {
            getString(
                R.string.model_download_progress,
                percent,
                megabytes(done),
                megabytes(total),
            )
        } else {
            getString(R.string.model_download_starting, model)
        }
        return Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(android.R.drawable.stat_sys_download)
            .setContentTitle(getString(R.string.model_download_title, model))
            .setContentText(text)
            .setProgress(100, percent, total <= 0)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .addAction(
                Notification.Action.Builder(
                    Icon.createWithResource(this, android.R.drawable.ic_menu_close_clear_cancel),
                    getString(R.string.model_download_cancel),
                    cancel,
                ).build(),
            )
            .build()
    }

    private fun notifyProgress(model: String, percent: Int, done: Long, total: Long) {
        val manager = getSystemService(NotificationManager::class.java)
        manager.notify(NOTIFICATION_ID, buildProgress(model, percent, done, total))
    }

    private fun postStatus(title: String, body: String) {
        val notification = Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(android.R.drawable.stat_sys_download_done)
            .setContentTitle(title)
            .setContentText(body)
            .setAutoCancel(true)
            .build()
        getSystemService(NotificationManager::class.java).notify(STATUS_NOTIFICATION_ID, notification)
    }

    private fun errorMessage(t: Throwable): String = t.message ?: t.javaClass.simpleName

    companion object {
        const val ACTION_CANCEL = "app.suramak.models.CANCEL_DOWNLOAD"

        /** Public so the receiver can clear the same notification. */
        const val NOTIFICATION_ID = 4101
        const val CHANNEL_ID = "model_downloads"

        private const val TAG = "SuramakModels"
        private const val JOB_ID = 4100
        private const val STATUS_NOTIFICATION_ID = 4102
        private const val POLL_MS = 500L
        private const val KEY_MODEL = "model"
        private const val KEY_LANGUAGE = "language"
        private const val KEY_MODELS_DIR = "models_dir"

        /**
         * Schedules the one model-download job. Returns false when the platform
         * refused to schedule it (job quota exhausted, service disabled).
         */
        fun enqueue(context: Context, model: String, language: String): Boolean {
            val extras = PersistableBundle().apply {
                putString(KEY_MODEL, model)
                putString(KEY_LANGUAGE, language)
                putString(KEY_MODELS_DIR, File(context.filesDir, "models").absolutePath)
            }
            val job = JobInfo.Builder(
                JOB_ID,
                ComponentName(context, ModelDownloadJobService::class.java),
            )
                .setRequiredNetworkType(JobInfo.NETWORK_TYPE_ANY)
                .setRequiresStorageNotLow(true)
                .setExtras(extras)
                .build()
            val scheduler = context.getSystemService(JobScheduler::class.java)
            // A refused schedule is a named failure, never a crash: the manifest
            // once lacked ACCESS_NETWORK_STATE, and the platform answers such a
            // job with a SecurityException from inside schedule() (the monkey
            // pass killed the app on it). The caller already reports false.
            return try {
                scheduler.schedule(job) == JobScheduler.RESULT_SUCCESS
            } catch (t: SecurityException) {
                Log.e(TAG, "the download job was refused by the platform", t)
                false
            }
        }

        /** Clears the progress notification (used by the cancel broadcast). */
        fun clearProgressNotification(context: Context) {
            context.getSystemService(NotificationManager::class.java).cancel(NOTIFICATION_ID)
        }

        private fun ensureChannel(context: Context) {
            val manager = context.getSystemService(NotificationManager::class.java)
            if (manager.getNotificationChannel(CHANNEL_ID) != null) {
                return
            }
            val channel = NotificationChannel(
                CHANNEL_ID,
                context.getString(R.string.model_download_channel),
                NotificationManager.IMPORTANCE_LOW,
            ).apply {
                description = context.getString(R.string.model_download_channel_desc)
                setShowBadge(false)
            }
            manager.createNotificationChannel(channel)
        }

        private fun megabytes(bytes: Long): Long = bytes / 1_000_000
    }
}
