package app.suramak.transcribe

import android.app.job.JobInfo
import android.app.job.JobParameters
import android.app.job.JobScheduler
import android.app.job.JobService
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.ParcelFileDescriptor
import android.os.PersistableBundle
import android.os.PowerManager
import android.util.Log
import androidx.core.os.BundleCompat
import app.suramak.spike.R
import java.io.File
import java.io.FileOutputStream
import java.io.IOException

/**
 * M13e — the foreground service that hosts transcription jobs.
 *
 * **Why a `JobService` and not a bare started service.** The job survives the
 * Activity, is scheduled by the platform and is the same host M13d proved for
 * downloads. The service promotes itself to the foreground for the duration,
 * so the CPU keeps running with the screen off.
 *
 * **The foreground type is chosen per API level** — `mediaProcessing` from 35
 * (Android 15's type for on-device media work), `dataSync` on 34, no type below
 * 34. The rule lives in Rust (`foreground_type_for_api`, unit-tested) and this
 * class only maps the name to the platform constant; the manifest declares both
 * types, and the permissions are `FOREGROUND_SERVICE_MEDIA_PROCESSING` /
 * `FOREGROUND_SERVICE_DATA_SYNC`.
 *
 * **One inference slot.** The work itself is the M16.1 `QueueDriver` in Rust:
 * the service never runs an engine, it only copies bytes, polls `status()` and
 * paints the notification. Two shared files therefore cannot run two
 * inferences, and the decode goes through the shared M15.2 decoder.
 *
 * **Cancel is a user action; timeout is a system pause.** The notification's
 * Cancel action sets the cooperative token and deletes the copy. `onTimeout`
 * (Android 15's six-hour `dataSync` limit, `Service.onTimeout`) and
 * `onStopJob` instead *pause*: the cooperative token is set, the copied file and
 * the journal entry stay, and the next launch offers to resume — which is the
 * contract the workorder asks for.
 */
class TranscriptionJobService : JobService() {

    /** Set by every system stop path; the worker thread stops at its next poll. */
    @Volatile
    private var stopped = false

    @Volatile
    private var activeParams: JobParameters? = null

    private var worker: Thread? = null
    private var wakeLock: PowerManager.WakeLock? = null

    /**
     * One notification per service instance: a second share while a batch runs
     * gets its own job (and its own progress notification) instead of replacing
     * the running one, which JobScheduler would do for a shared job id.
     */
    private var notificationId = Notify.PROGRESS_ID

    /** `done` at the start of this run, so a second batch does not re-post the
     * completion notification of the first. */
    private var doneBaseline = 0

    /** M22B: this run came from a share intent's voice note. */
    private var voiceNote = false

    /** M22B: the sender the intent provided (null when it did not). */
    private var voiceNoteSender: String? = null

    /** `jobFinished` exactly once per `onStartJob` that returned true. */
    private val finishLock = Any()
    private var finished = false

    override fun onCreate() {
        super.onCreate()
        TranscribeBridge.init(this)
        Notify.ensureChannels(this)
        notificationId = NEXT_NOTIFICATION_ID.getAndIncrement()
    }

    override fun onStartJob(params: JobParameters): Boolean {
        Notify.ensureChannels(this)
        activeParams = params
        finished = false
        stopped = false

        val language = params.extras.getString(KEY_LANGUAGE) ?: "auto"
        val uris = sharedUris(params)
        val mode = params.extras.getString(KEY_MODE) ?: MODE_IMPORT
        val resume = mode == MODE_RESUME
        voiceNote = params.extras.getString(KEY_KIND) == KIND_VOICE_NOTE
        voiceNoteSender = params.extras.getString(KEY_SENDER)?.takeIf { it.isNotEmpty() }

        if (!resume && uris.isEmpty()) {
            // Nothing to import and nothing to resume: a replaced job's echo.
            return false
        }

        val model = ModelChoice.forJob(this, language)
        if (model == null && !resume) {
            Notify.show(
                this,
                Notify.STATUS_ID,
                Notify.status(
                    this,
                    getString(R.string.transcribe_no_model_title),
                    getString(R.string.transcribe_no_model_body),
                ),
            )
            return false
        }

        doneBaseline = TranscribeBridge.status().optInt("done", 0)
        promoteToForeground(getString(R.string.transcribe_starting), 0, 0, null)
        acquireWakeLock()

        worker = Thread({ runJob(uris, model, language) }, "suramak-transcribe").also { it.start() }
        return true
    }

    /** Copies every shared URI, then pumps the queue until it is idle. */
    private fun runJob(uris: List<Uri>, model: String?, language: String) {
        var lastError = ""
        for (uri in uris) {
            if (stopped) return
            if (model == null) {
                lastError = getString(R.string.transcribe_no_model_body)
                break
            }
            if (!importUri(uri, model, language)) {
                lastError = getString(R.string.transcribe_failed_body)
            }
        }
        if (stopped) return
        if (!pumpQueue()) return
        complete(lastError)
    }

    /**
     * Copies one URI into app storage. The read is 64 KiB at a time; the bridge
     * hears about it every [PROGRESS_STEP] bytes (the host coalesces again), so
     * a 500 MB import is a bounded number of JNI calls, not one per read.
     */
    private fun importUri(uri: Uri, model: String, language: String): Boolean {
        val name = ImportSource.displayName(this, uri)
        val mime = ImportSource.mimeOf(this, uri)
        val declared = ImportSource.sizeOf(this, uri)
        val begun = TranscribeBridge.beginImport(
            name,
            mime,
            declared,
            model,
            language,
            kind = if (voiceNote) KIND_VOICE_NOTE else "import",
            sender = voiceNoteSender,
        )
        if (!begun.optBoolean("ok", false)) {
            Log.w(TAG, "import refused: ${begun.optString("refused")}")
            Notify.show(
                this,
                Notify.STATUS_ID,
                Notify.status(
                    this,
                    getString(R.string.transcribe_refused_title),
                    begun.optString("refused").ifEmpty { getString(R.string.transcribe_failed_body) },
                ),
            )
            return false
        }
        val key = begun.optString("key")
        val destination = File(begun.optString("media_path"))
        return try {
            val descriptor = contentResolver.openFileDescriptor(uri, "r")
                ?: throw IOException("the picker returned no file descriptor")
            ParcelFileDescriptor.AutoCloseInputStream(descriptor).use { input ->
                FileOutputStream(destination).use { output ->
                    val buffer = ByteArray(COPY_CHUNK)
                    var written = 0L
                    var reported = 0L
                    while (true) {
                        val read = input.read(buffer)
                        if (read <= 0) break
                        output.write(buffer, 0, read)
                        written += read
                        if (written - reported >= PROGRESS_STEP) {
                            reported = written
                            TranscribeBridge.progress(key, written)
                        }
                    }
                    // The bytes must be on disk before `import_done` verifies
                    // the size: the journal is the recovery contract.
                    output.fd.sync()
                }
            }
            TranscribeBridge.progress(key, destination.length())
            val done = TranscribeBridge.importDone(key)
            if (!done.optBoolean("ok", false)) {
                TranscribeBridge.importFailed(key, done.optString("error"))
                Log.w(TAG, "import failed: ${done.optString("error")}")
                return false
            }
            true
        } catch (t: Throwable) {
            TranscribeBridge.importFailed(key, t.message ?: t.javaClass.simpleName)
            Log.e(TAG, "import of $uri failed", t)
            false
        }
    }

    /** Polls the host, painting the notification, until the queue is idle. */
    private fun pumpQueue(): Boolean {
        var lastPercent = -1
        var lastWaiting = -1
        var lastKey: String? = null
        while (!stopped) {
            val status = TranscribeBridge.status()
            val running = status.optBoolean("running", false)
            val waiting = status.optInt("waiting", 0)
            val percent = status.optInt("percent", 0)
            val key = status.optString("current_key").ifEmpty { null }
            if (running && (percent != lastPercent || waiting != lastWaiting || key != lastKey)) {
                lastPercent = percent
                lastWaiting = waiting
                lastKey = key
                val title = status.optString("current_title")
                    .ifEmpty { getString(R.string.transcribe_unnamed) }
                val isVoiceNote = voiceNote || status.optString("current_kind") == KIND_VOICE_NOTE
                // The smoke (and a support log) needs the job key the Cancel
                // action would target; it is the host's own key, not a guess.
                if (isVoiceNote && key != null) {
                    Log.i(TAG, "voice-note job key=$key percent=$percent")
                }
                if (isVoiceNote) {
                    Notify.show(
                        this,
                        notificationId,
                        Notify.voiceNoteProgress(
                            this,
                            notificationId,
                            title,
                            TranscribeBridge.progressText(percent).optString("text"),
                            percent,
                            key,
                        ),
                    )
                } else {
                    Notify.show(
                        this,
                        notificationId,
                        Notify.progress(
                            this,
                            getString(R.string.transcribe_progress_title, title),
                            percent,
                            waiting,
                            key,
                        ),
                    )
                }
            }
            if (!running && waiting == 0) {
                return true
            }
            try {
                Thread.sleep(POLL_MS)
            } catch (e: InterruptedException) {
                Thread.currentThread().interrupt()
                return false
            }
        }
        return false
    }

    /** Posts the completion/status notification and ends the job. */
    private fun complete(lastError: String) {
        val status = TranscribeBridge.status()
        val last = status.optJSONObject("last")
        val done = status.optInt("done", 0)
        val failed = status.optInt("failed", 0)
        val lastKind = last?.optString("kind").orEmpty()
        // A finished voice note gets its own id: `notificationId` is the
        // foreground notification, and `stopForeground(REMOVE)` below would
        // take a notification posted on that same id with it.
        val readyId = notificationId + 10_000
        if (last != null && done > doneBaseline) {
            if (lastKind == KIND_VOICE_NOTE) {
                val id = last.optString("transcript_id")
                val sender = last.optString("sender").ifEmpty { voiceNoteSender }
                val content = TranscribeBridge.voiceNote(id, 0, sender)
                if (content.optBoolean("ok", false)) {
                    Notify.show(
                        this,
                        readyId,
                        Notify.voiceNoteReady(this, readyId, id, content, copied = false),
                    )
                } else {
                    // The row is saved; only its notification model failed.
                    // Say exactly that instead of pretending the job failed.
                    Notify.show(
                        this,
                        Notify.STATUS_ID,
                        Notify.status(
                            this,
                            getString(R.string.transcribe_done_title),
                            content.optString("error")
                                .ifEmpty { getString(R.string.transcribe_done_body, last.optString("title"), last.optInt("segments", 0)) },
                        ),
                    )
                }
            } else {
                Notify.show(
                    this,
                    Notify.STATUS_ID,
                    Notify.completed(
                        this,
                        last.optString("title"),
                        last.optInt("segments", 0),
                        last.optString("transcript_id"),
                    ),
                )
            }
        } else if (failed > 0 || lastError.isNotEmpty()) {
            val message = status.optString("message").ifEmpty { lastError }
            val failure = status.optJSONObject("last_failed")
            if (failure != null && failure.optString("kind") == KIND_VOICE_NOTE) {
                Notify.show(
                    this,
                    readyId,
                    Notify.voiceNoteFailed(
                        this,
                        readyId,
                        getString(R.string.transcribe_failed_title),
                        failure.optString("reason").ifEmpty { message }
                            .ifEmpty { getString(R.string.transcribe_failed_body) },
                        failure.optString("key"),
                        failure.optString("sender").ifEmpty { voiceNoteSender },
                    ),
                )
            } else {
                Notify.show(
                    this,
                    Notify.STATUS_ID,
                    Notify.status(
                        this,
                        getString(R.string.transcribe_failed_title),
                        message.ifEmpty { getString(R.string.transcribe_failed_body) },
                    ),
                )
            }
        } else if (status.optInt("cancelled", 0) > 0) {
            Notify.show(
                this,
                Notify.STATUS_ID,
                Notify.status(
                    this,
                    getString(R.string.transcribe_cancelled_title),
                    getString(R.string.transcribe_cancelled_body),
                ),
            )
        }
        stopForeground(STOP_FOREGROUND_REMOVE)
        releaseWakeLock()
        val params = activeParams
        if (params != null) {
            finishJob(params, retry = false)
        }
    }

    // --- lifecycle ----------------------------------------------------------

    /**
     * The system is stopping the job (quota, constraints, app removed). The
     * copy and the journal survive, so this is a pause, not a cancel.
     */
    override fun onStopJob(params: JobParameters): Boolean {
        stopped = true
        TranscribeBridge.pauseAll()
        worker?.interrupt()
        stopForeground(STOP_FOREGROUND_REMOVE)
        releaseWakeLock()
        Notify.show(
            this,
            Notify.STATUS_ID,
            Notify.status(
                this,
                getString(R.string.transcribe_paused_title),
                getString(R.string.transcribe_paused_body),
            ),
        )
        finishJob(params, retry = false)
        return false
    }

    /**
     * Android 15's foreground-service limit. `Service.onTimeout` is the only
     * hook for it (`JobService` has none — verified against `android-35`), and
     * the contract is the pause contract: stop cleanly, keep the copy, let the
     * next launch offer resume.
     */
    override fun onTimeout(startId: Int) {
        stopped = true
        TranscribeBridge.pauseAll()
        worker?.interrupt()
        stopForeground(STOP_FOREGROUND_REMOVE)
        releaseWakeLock()
        Notify.show(
            this,
            Notify.STATUS_ID,
            Notify.status(
                this,
                getString(R.string.transcribe_paused_title),
                getString(R.string.transcribe_timed_out_body),
            ),
        )
        val params = activeParams
        if (params != null) {
            finishJob(params, retry = false)
        }
    }

    override fun onDestroy() {
        stopped = true
        if (activeParams != null) {
            // Destroyed mid-job (the process is going away): pause so the next
            // launch can resume rather than losing the copy's state.
            TranscribeBridge.pauseAll()
        }
        releaseWakeLock()
        stopForeground(STOP_FOREGROUND_REMOVE)
        super.onDestroy()
    }

    private fun finishJob(params: JobParameters, retry: Boolean) {
        synchronized(finishLock) {
            if (finished) return
            finished = true
            activeParams = null
            jobFinished(params, retry)
        }
    }

    // --- foreground + wake lock ---------------------------------------------

    private fun promoteToForeground(title: String, percent: Int, waiting: Int, key: String?) {
        val notification = if (voiceNote) {
            Notify.voiceNoteProgress(
                this,
                notificationId,
                getString(R.string.transcribe_unnamed),
                TranscribeBridge.progressText(percent).optString("text"),
                percent,
                key,
            )
        } else {
            Notify.progress(this, title, percent, waiting, key)
        }
        val api = Build.VERSION.SDK_INT
        try {
            if (api >= 34) {
                startForeground(notificationId, notification, foregroundType(api))
            } else {
                // Below 34 there is no type to declare: the pre-Android-14 shape.
                startForeground(notificationId, notification)
            }
        } catch (t: Throwable) {
            // A refused promotion must not lose the job: it still runs, and the
            // notification is posted normally.
            Log.w(TAG, "could not promote the job to a foreground service", t)
            Notify.show(this, notificationId, notification)
        }
    }

    /**
     * The API → `ServiceInfo` mapping. The *selection* is Rust's
     * (`foreground_type_for_api`, unit-tested there); this is the compile-time
     * half that turns the name into the platform constant.
     */
    private fun foregroundType(api: Int): Int {
        val reply = TranscribeBridge.foregroundType(api)
        return when (reply.optString("type")) {
            "mediaProcessing" -> ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROCESSING
            "dataSync" -> ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC
            else -> 0
        }
    }

    private fun acquireWakeLock() {
        if (wakeLock != null) return
        val power = getSystemService(PowerManager::class.java) ?: return
        wakeLock = power.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, WAKE_TAG).apply {
            setReferenceCounted(false)
            acquire(MAX_WAKE_MS)
        }
    }

    private fun releaseWakeLock() {
        wakeLock?.let { lock ->
            if (lock.isHeld) lock.release()
        }
        wakeLock = null
    }

    private fun sharedUris(params: JobParameters): List<Uri> {
        val bundle: Bundle = params.transientExtras ?: return emptyList()
        return BundleCompat.getParcelableArrayList(bundle, KEY_URIS, Uri::class.java)
            ?: emptyList()
    }

    companion object {
        private const val TAG = "SuramakTranscribe"
        /** Distinct job ids: JobScheduler replaces a job with the same id. */
        private val NEXT_JOB_ID = java.util.concurrent.atomic.AtomicInteger(4200)
        private val NEXT_NOTIFICATION_ID = java.util.concurrent.atomic.AtomicInteger(4201)
        private const val KEY_URIS = "uris"
        private const val KEY_LANGUAGE = "language"
        private const val KEY_MODE = "mode"
        private const val KEY_KIND = "kind"
        private const val KEY_SENDER = "sender"
        private const val MODE_IMPORT = "import"
        private const val MODE_RESUME = "resume"
        /** M22B: this job came from a share intent's voice note. */
        const val KIND_VOICE_NOTE = "voice_note"
        private const val COPY_CHUNK = 64 * 1024
        private const val PROGRESS_STEP = 256 * 1024L
        private const val POLL_MS = 300L
        private const val WAKE_TAG = "suramak:transcribe"
        private const val MAX_WAKE_MS = 6 * 60 * 60 * 1000L

        /**
         * Schedules the one transcription job for these URIs. The URIs travel in
         * the **transient** extras: a `content://` grant is not persistable
         * state, and serializing it into a `PersistableBundle` would survive a
         * reboot without the grant. The service copies immediately, which is
         * what makes that safe.
         */
        fun enqueue(
            context: Context,
            uris: List<Uri>,
            language: String = "auto",
            kind: String = "import",
            sender: String? = null,
        ): Boolean {
            if (uris.isEmpty()) return false
            val transient = Bundle().apply {
                putParcelableArrayList(KEY_URIS, ArrayList(uris))
            }
            val extras = PersistableBundle().apply {
                putString(KEY_MODE, MODE_IMPORT)
                putString(KEY_LANGUAGE, language)
                putString(KEY_KIND, kind)
                putString(KEY_SENDER, sender)
            }
            val builder = JobInfo.Builder(
                NEXT_JOB_ID.getAndIncrement(),
                ComponentName(context, TranscriptionJobService::class.java),
            )
                .setTransientExtras(transient)
                .setExtras(extras)
                .setOverrideDeadline(0)
            // A share must start now, not at the next scheduler batch: from
            // API 31 the job is expedited (the app is in the foreground at share
            // time, so the quota is not the constraint). Below 31 a job with no
            // constraints is eligible immediately, which is the best the
            // platform offers.
            if (Build.VERSION.SDK_INT >= 31) builder.setExpedited(true)
            val job = builder.build()
            val scheduler = context.getSystemService(JobScheduler::class.java) ?: return false
            return scheduler.schedule(job) == JobScheduler.RESULT_SUCCESS
        }

        /**
         * Schedules a pump with no URIs: the launch-time prompt already resumed
         * the journaled jobs in the host, and the service is what makes them
         * foreground work with a wake lock.
         */
        /**
         * M13g1 Quit: unschedule every job this app owns and stop the service
         * instance. Called after the Rust host paused the work and before the
         * process exits, so the platform cannot restart a job on the way down.
         */
        fun shutdown(context: Context) {
            try {
                context.getSystemService(JobScheduler::class.java)?.cancelAll()
            } catch (t: Throwable) {
                Log.w(TAG, "the scheduler could not be cleared", t)
            }
            context.stopService(Intent(context, TranscriptionJobService::class.java))
        }

        fun resumePending(context: Context): Boolean {
            val extras = PersistableBundle().apply {
                putString(KEY_MODE, MODE_RESUME)
                putString(KEY_LANGUAGE, "auto")
            }
            val builder = JobInfo.Builder(
                NEXT_JOB_ID.getAndIncrement(),
                ComponentName(context, TranscriptionJobService::class.java),
            )
                .setExtras(extras)
                .setOverrideDeadline(0)
            if (Build.VERSION.SDK_INT >= 31) builder.setExpedited(true)
            val job = builder.build()
            val scheduler = context.getSystemService(JobScheduler::class.java) ?: return false
            return scheduler.schedule(job) == JobScheduler.RESULT_SUCCESS
        }
    }
}
