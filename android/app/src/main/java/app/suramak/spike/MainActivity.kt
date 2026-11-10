package app.suramak.spike

import android.Manifest
import android.animation.ValueAnimator
import android.content.ClipData
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.graphics.Typeface
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.provider.Settings
import android.util.Log
import android.util.TypedValue
import android.view.Gravity
import android.view.KeyEvent
import android.view.SurfaceHolder
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast
import android.window.OnBackInvokedDispatcher
import androidx.activity.OnBackPressedCallback
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AlertDialog
import androidx.core.content.FileProvider
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import app.suramak.mic.MicBridge
import app.suramak.mic.RecordingService
import app.suramak.transcribe.ImportSource
import app.suramak.transcribe.ModelChoice
import app.suramak.transcribe.Notify
import app.suramak.transcribe.TranscribeBridge
import app.suramak.models.DeviceMemory
import app.suramak.models.ModelDownloadJobService
import app.suramak.transcribe.TranscriptionJobService
import app.suramak.ui.ExportWriter
import app.suramak.ui.UiBridge
import com.google.androidgamesdk.GameActivity
import com.google.androidgamesdk.gametextinput.State
import org.json.JSONArray
import org.json.JSONObject

/**
 * M13b shell spike v2.4 — the entire Kotlin surface.
 *
 * Extends the androidx `GameActivity` (AGDK), which is what gives the Rust side
 * `GameTextInput` (the soft-keyboard path egui's IME support needs). This class
 * owns almost nothing visual: the UI is egui, drawn by
 * `libsuramak_android_spike.so`. The one exception is the crash overlay — a
 * plain `TextView` stack added over the surface, because it has to survive the
 * Rust side dying. Since v2.1 that overlay also carries a **Share log** button
 * (`FileProvider` + `ACTION_SEND`), so the crash record can leave the phone
 * without a PC even when the Rust surface is dead.
 *
 * The JNI contract:
 *   Kotlin -> Rust   nativeInit(filesDir)                         [once, pre-super]
 *   Kotlin -> Rust   nativeSetInsets(top, bottom, left, right, imeBottom)  [px]
 *   Kotlin -> Rust   nativeBackPressed()
 *   Kotlin -> Rust   nativeImeText(text, selectionStart, selectionEnd)     [UTF-16 indices]
 *   Kotlin -> Rust   nativeLogLine(line)        [lifecycle lines for the on-screen ring]
 *   Kotlin -> Rust   nativeActivityDestroyed()  [stop the event loop]
 *   Kotlin -> Rust   nativeSaveProbe(): String? [state for onSaveInstanceState]
 *   Kotlin -> Rust   nativeRestoreProbe(payload) [the same payload back on onCreate]
 *   Rust -> Kotlin   postPushInsets()           [re-read + re-push the window insets]
 *   Rust -> Kotlin   postShowCrashOverlay()     [re-read the crash file, show it now]
 *
 * **v2.4: rotation.** The manifest deliberately does not swallow orientation, so
 * a rotate destroys and recreates this Activity. Two mechanisms make that
 * survivable:
 *
 *  * the Rust side latches `nativeActivityDestroyed` (called below, *before*
 *    `super.onDestroy`) and refuses every later gpu build/frame for that
 *    generation, so the destroy cannot land inside an init against a dead
 *    window (see `docs/reports/M13b6-spike-v2.4.md`);
 *  * [onSaveInstanceState] stores the probe payload in the instance state and
 *    [onCreate] hands it back, so the text survives a rotation *and* a process
 *    kill — the platform keeps the Bundle for this Activity record even across
 *    a fresh process.
 *
 * **There is deliberately no recreate entry point.** v2, v2.1 and v2.2 all
 * reached this class through `ndk_context`'s handle to call `postRecreate()`;
 * `android-activity` 0.6.1 initialises that handle with
 * `activity.getApplication()` (`src/init.rs:281`), i.e. with the Application —
 * so the `postRecreate` method looked up on it could only fail
 * (`NoSuchMethodError`, which `jni` reports as `Java exception was thrown`) and
 * `Activity.recreate()` was never called by any build. v2.3 removes the method
 * instead of repairing the handle: the spike must not be able to ask Android to
 * relaunch the Activity, and with no method here a future Rust call has nothing
 * to reach. The button is a named limitation in the egui UI
 * (`docs/reports/M13b5-spike-v2.3.md`).
 *
 * `filesDir` above is `CrashFiles.dir(this)` — the app's *external* files dir
 * since v2.1 (`Android/data/app.suramak.spike/files`), not the private one.
 */
class MainActivity : GameActivity() {

    /** Cached so Rust's recreate request never touches a View off the UI thread. */
    private var root: View? = null

    /** The crash overlay, once built for this Activity. */
    private var overlay: View? = null
    private var overlayBody: TextView? = null

    /**
     * Set when Rust reported a fatal panic on the `android_main` thread. The
     * surface is gone for good at that point, so back must be able to leave the
     * app again instead of feeding a dead bridge. Written from the JNI thread,
     * read from the UI thread.
     */
    @Volatile
    private var nativePanicked = false

    /**
     * Last IME height pushed, so `dispatchKeyEvent` can let back close the
     * keyboard instead of swallowing the gesture that belongs to the IME.
     */
    private var lastImeBottom = 0

    /** Uptime of the last counted back press, for cross-path dedupe. */
    private var lastBackAtMs = 0L

    /**
     * M13e — the SAF picker. `ACTION_OPEN_DOCUMENT` with the two mime families
     * the shared decoder accepts; the picker grants a per-URI read permission
     * that the service consumes immediately by copying the bytes.
     */
    private val importLauncher =
        registerForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
            enqueueImport(uris)
        }

    /** One recovery prompt per Activity instance (a rotation would repeat it). */
    private var recoveryShown = false

    /**
     * M13g2 — the export the picker is currently placing. Set before the
     * `ACTION_CREATE_DOCUMENT` chooser opens; consumed by its result.
     */
    private var pendingExport: ExportWriter.Staged? = null

    /**
     * M13g2 — the compact editor's export sheet calls
     * [exportFromCompact]/[shareFromCompact]. The picker is a per-format
     * contract (six MIME types), so it is registered once and given the staged
     * export as input.
     */
    private val exportLauncher =
        registerForActivityResult(ExportWriter.CreateExportDocument()) { uri ->
            val staged = pendingExport
            pendingExport = null
            if (staged == null) {
                logToRust("export: the picker returned with no staged export")
                return@registerForActivityResult
            }
            if (uri == null) {
                UiBridge.exportCancel(staged.job)
                logToRust("export: cancelled at the picker (${staged.name})")
                return@registerForActivityResult
            }
            ExportWriter.write(this, uri, staged).fold(
                onSuccess = { written ->
                    UiBridge.exportDone(staged.job, written)
                    logToRust("export: wrote ${staged.name} ($written bytes, checksum verified)")
                },
                onFailure = { failure ->
                    // The staged bytes stay; the sheet reports the host's reason
                    // and offers Try again.
                    val message = failure.message ?: failure.javaClass.simpleName
                    UiBridge.exportFailed(staged.job, message)
                    logToRust("export: failed — $message")
                    Toast.makeText(this, "Export failed: $message", Toast.LENGTH_LONG).show()
                },
            )
        }

    /**
     * M13F — the microphone host. The pre-compact record bar is gone (M13g3:
     * the compact Library's Record is the entry); the host itself is unchanged.
     */
    private external fun nativeInit(filesDir: String)

    private external fun nativeSetReduceMotion(enabled: Boolean)

    private external fun nativeSetInsets(
        top: Int,
        bottom: Int,
        left: Int,
        right: Int,
        imeBottom: Int,
    )

    private external fun nativeBackPressed()

    private external fun nativeImeText(text: String, selectionStart: Int, selectionEnd: Int)

    private external fun nativeLogLine(line: String)

    private external fun nativeActivityDestroyed()

    /**
     * v2.4 — the instance-state payload (see `probe_state.rs`). Called from
     * [onSaveInstanceState]; returns null when there is nothing to store, never
     * throws into the lifecycle callback.
     */
    private external fun nativeSaveProbe(): String?

    /** v2.4 — hand a payload from a previous [onSaveInstanceState] back to Rust. */
    private external fun nativeRestoreProbe(payload: String)

    /**
     * The soft keyboard's editing state, straight from GameTextInput.
     *
     * `super.stateChanged` keeps GameActivity's own native mirror in sync (the
     * Rust side of that path is the `InputEvent::TextEvent` that winit 0.30
     * consumes and drops); the extra JNI call is what actually feeds egui. Keep
     * the super call: it drives GameActivity's keyboard visibility/editor
     * action bookkeeping.
     */
    override fun stateChanged(state: State, dismissKeyboard: Boolean) {
        super.stateChanged(state, dismissKeyboard)
        // `state.text` is a platform type: it can be null on a fresh connection.
        nativeImeText(state.text ?: "", state.selectionStart, state.selectionEnd)
    }

    companion object {
        init {
            // Must run before GameActivity.onCreate calls initializeNativeCode():
            // class initialisation happens when the Activity is instantiated, so
            // the library is loaded in time. M13g3: the window and the compact
            // surface are one library now — `libsuramak_android.so` exports
            // `android_main` (the M13b lifecycle + the compact shell) and every
            // bridge symbol (Ui/Model/Transcribe/Mic). The M13b spike library is
            // still built and packaged for its history and host harness, but no
            // longer loaded: a second loaded copy would mean a second host.
            System.loadLibrary("suramak_android")
        }

        private const val TAG = "SuramakSpike"

        /**
         * One physical back press can reach two of the three dispatch paths on
         * some OEM builds; within this window it is one press.
         */
        private const val BACK_DEDUPE_MS = 250L

        /**
         * The `Bundle` key the probe payload lives under (v2.4). The *key* is
         * Kotlin's business — Rust owns the payload format, and the two meet only
         * in `nativeSaveProbe` / `nativeRestoreProbe`.
         */
        private const val KEY_PROBE = "app.suramak.spike.probe"

        /** M13e — the launcher shortcut's action (`res/xml/shortcuts.xml`). */
        const val ACTION_IMPORT_AUDIO = "app.suramak.action.IMPORT_AUDIO"

        /**
         * M13g3 — the compact surface's platform verbs. Rust sends these as
         * explicit intents to this Activity (Application context +
         * FLAG_ACTIVITY_NEW_TASK; `singleTask` delivers them to `onNewIntent`),
         * because the Activity is the only owner of the picker, the services
         * and `finishAndRemoveTask`. The strings are mirrored in
         * `crates/suramak-android-spike/src/shell.rs`.
         */
        const val ACTION_COMPACT_IMPORT = "app.suramak.action.COMPACT_IMPORT"
        const val ACTION_COMPACT_RECORD = "app.suramak.action.COMPACT_RECORD"
        const val ACTION_COMPACT_STOP_RECORDING = "app.suramak.action.COMPACT_STOP_RECORDING"
        const val ACTION_COMPACT_EXPORT = "app.suramak.action.COMPACT_EXPORT"
        const val ACTION_COMPACT_SHARE = "app.suramak.action.COMPACT_SHARE"
        const val ACTION_COMPACT_DOWNLOAD = "app.suramak.action.COMPACT_DOWNLOAD"
        const val ACTION_COMPACT_QUIT = "app.suramak.action.COMPACT_QUIT"
        const val ACTION_COMPACT_FINISH = "app.suramak.action.COMPACT_FINISH"
        const val ACTION_COMPACT_SHARE_CRASH = "app.suramak.action.COMPACT_SHARE_CRASH"

        private const val EXTRA_FORMAT = "format"
        private const val EXTRA_BILINGUAL = "bilingual"
        private const val EXTRA_MODEL = "model"
        private const val EXTRA_LANGUAGE = "language"

        /** M13e — the POST_NOTIFICATIONS request code. */
        private const val REQUEST_NOTIFICATIONS = 4301

        /** M13F — the RECORD_AUDIO runtime request code. */
        private const val REQUEST_RECORD_AUDIO = 4302

        /** M13F — the one-shot permission-request marker (request once, then Settings). */
        private const val PREFS_MIC = "mic"
        private const val KEY_MIC_REQUESTED = "permission_requested"
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        // Before anything else: what did the *last* process leave behind?
        CrashFiles.foldAbandonedSession(this)
        // ...and start this session's marker immediately, so even a failure
        // inside super.onCreate (which starts the native thread) leaves a trace.
        CrashFiles.beginSession(this)
        logToRust("onCreate")

        // One small bridge addition: the files dir, so the Rust panic hook can
        // persist a crash before `android_main` exists. Called before
        // `super.onCreate`, which is what starts the native thread. Since v2.1
        // this is the *external* files dir (browseable, shareable) — the same
        // one CrashFiles migrates into and reads from.
        try {
            val dir = CrashFiles.dir(this)
            nativeInit(dir.absolutePath)
            Log.i(TAG, "crash files: ${dir.absolutePath} (external=${CrashFiles.isExternal(this)})")
        } catch (t: Throwable) {
            Log.e(TAG, "nativeInit failed", t)
        }

        // M13g3: the compact host must exist *before* `super.onCreate` starts
        // the native thread — `android_main`'s first frame draws the Library
        // from it. `UiBridge.init` is idempotent (one process-wide host), so
        // this is the same call the later job-host block used to make.
        try {
            UiBridge.init(this)
            UiBridge.device(
                DeviceMemory.read(this),
                freeBytes = DeviceMemory.freeBytes(this),
                reduceMotion = reduceMotionEnabled(),
            )
            nativeSetReduceMotion(reduceMotionEnabled())
        } catch (t: Throwable) {
            Log.w(TAG, "the compact UI host did not initialise", t)
        }

        // v2.4: the instance state comes back *before* `super.onCreate` starts the
        // native thread, so the restored text is already in SpikeState when
        // `android_main` for this Activity runs. Same process for a rotation, a
        // brand-new process after a kill — the Bundle is the platform's, not ours.
        val savedProbe = savedInstanceState?.getString(KEY_PROBE)
        if (savedProbe != null) {
            try {
                nativeRestoreProbe(savedProbe)
                logToRust("instance state: handed ${savedProbe.length} payload chars back to Rust")
            } catch (t: Throwable) {
                logToRust("instance state: restore failed (${t.javaClass.simpleName})")
                Log.e(TAG, "nativeRestoreProbe failed", t)
            }
        } else {
            logToRust("instance state: none (cold start, or nothing was saved)")
        }

        super.onCreate(savedInstanceState)
        root = window.decorView
        installInsetsListener()
        installBackHandler()
        showCrashOverlayIfPresent("launch", force = false)

        // M13e: the job host needs `filesDir` before any job command, and the
        // share/SAF landing has to be handled on the launch intent. M13F adds
        // the mic host **after** the job host: a stopped take hands its WAV to
        // the M13E inbox.
        val micStatus = try {
            TranscribeBridge.init(this)
            val status = MicBridge.init(this)
            Notify.ensureChannels(this)
            status
        } catch (t: Throwable) {
            Log.e(TAG, "the transcription/microphone host did not initialise", t)
            null
        }
        micStatus?.let { showRecoveredRecording(it) }
        requestNotificationPermission()
        handleLaunchIntent(intent)
        offerRecovery()
    }

    /**
     * M13g3 — whether the OS asks for reduced motion. `areAnimatorsEnabled()`
     * is Android 12's accessibility animator switch; the two system scales are
     * what a user turning animation off in Developer options changes. Either
     * one at zero means the compact surface keeps its fades and drops springs.
     */
    private fun reduceMotionEnabled(): Boolean {
        val durations = Settings.Global.getFloat(
            contentResolver,
            Settings.Global.ANIMATOR_DURATION_SCALE,
            1f,
        )
        val transition = Settings.Global.getFloat(
            contentResolver,
            Settings.Global.TRANSITION_ANIMATION_SCALE,
            1f,
        )
        return !ValueAnimator.areAnimatorsEnabled() || durations == 0f || transition == 0f
    }

    /**
     * A second share/notification tap arrives here (the Activity is
     * `singleTask`): the same handler as `onCreate`, so there is one path.
     */
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleLaunchIntent(intent)
    }

    override fun onStart() {
        super.onStart()
        logToRust("onStart")
    }

    override fun onResume() {
        super.onResume()
        logToRust("onResume")
        // v2.3: print the Activity's relaunch-relevant state at every resume.
        // Nothing in this app can arm a relaunch any more (there is no recreate
        // entry point), so this line is how the device pass *sees* that the
        // Activity comes back clean instead of half-relaunched.
        logSettle("onResume")
        // Mark the session as visible again (onStop removes the marker) so a
        // native crash in the foreground still leaves a trace.
        CrashFiles.beginSession(this)
        // The insets listener only fires when insets change; re-push on resume so
        // the Rust side always has a value after a surface/activity recreate.
        // v2.1 also *asks* for a dispatch: after a reset (or a missed first push)
        // nothing would ever change, and the panel sat at zeroes.
        root?.let { ViewCompat.requestApplyInsets(it) }
        pushCurrentInsets()
        // A recording may have been stopped by the notification, an
        // interruption or a process recovery while this Activity was away.
        // M13g3: the compact surface reads the host's status; there is no
        // second Kotlin record bar to refresh.
        logToRust("mic: status is owned by the compact host")
        // A crash recorded by Rust while the surface was dead only becomes
        // readable here if the overlay missed it.
        showCrashOverlayIfPresent("resume", force = false)
    }

    override fun onPause() {
        logToRust("onPause")
        super.onPause()
    }

    override fun onStop() {
        logToRust("onStop")
        logSettle("onStop")
        CrashFiles.endSession(this)
        super.onStop()
    }

    /**
     * v2.4 — the state that survives a rotation *and* a process kill.
     *
     * Called for a configuration change (the manifest does not swallow
     * orientation) and before the process can be killed in the background. The
     * system keeps this `Bundle` for the Activity's record: a rotation in this
     * process gets it back on `onCreate`, and a process the OS killed gets a
     * fresh Activity with the same `Bundle`. Rust owns the payload's shape
     * (`probe_state.rs`); this method only stores the one string.
     */
    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        try {
            val payload = nativeSaveProbe()
            if (payload == null) {
                logToRust("instance state: save returned nothing — no payload stored")
                return
            }
            outState.putString(KEY_PROBE, payload)
            logToRust("instance state: saved ${payload.length} payload chars")
        } catch (t: Throwable) {
            // A lifecycle callback must never throw over a diagnostics path.
            logToRust("instance state: save failed (${t.javaClass.simpleName})")
            Log.e(TAG, "nativeSaveProbe failed", t)
        }
    }

    override fun onDestroy() {
        logToRust("onDestroy (finishing=$isFinishing, changingConfigurations=$isChangingConfigurations)")
        // Tell Rust *before* super.onDestroy terminates the native code: winit
        // 0.30.13 never forwards android-activity's MainEvent::Destroy, so
        // without this the event loop outlives the Activity (see
        // docs/reports/M13b2-spike-v2.md §4).
        try {
            nativeActivityDestroyed()
        } catch (t: Throwable) {
            Log.e(TAG, "nativeActivityDestroyed failed", t)
        }
        super.onDestroy()
    }

    // --- M13e: import, recovery and the notification landing -----------------

    /**
     * The one place an incoming intent is interpreted.
     *
     * * a share (`ACTION_SEND` / `ACTION_SEND_MULTIPLE`) or a picker result
     *   becomes a transcription job;
     * * the launcher shortcut's `ACTION_IMPORT_AUDIO` opens the SAF picker;
     * * the completion notification's tap carries a transcript id and lands
     *   here: today a toast naming the row (there is no compact library yet —
     *   M13G replaces the toast, the intent stays the same).
     */
    private fun handleLaunchIntent(intent: Intent?) {
        if (intent == null) return
        when (intent.action) {
            Intent.ACTION_SEND, Intent.ACTION_SEND_MULTIPLE -> {
                val uris = ImportSource.urisFrom(intent)
                if (uris.isEmpty()) {
                    Toast.makeText(this, getString(R.string.transcribe_share_empty), Toast.LENGTH_LONG)
                        .show()
                } else {
                    enqueueImport(uris)
                }
            }
            ACTION_IMPORT_AUDIO -> importLauncher.launch(arrayOf("audio/*", "video/*"))
            Notify.ACTION_OPEN_TRANSCRIPT -> {
                val id = intent.getStringExtra(Notify.EXTRA_TRANSCRIPT_ID)
                if (id != null) showTranscript(id)
            }
            Notify.ACTION_OPEN_RECORDING -> logToRust("mic: recording notification tapped")
            ACTION_COMPACT_IMPORT -> importLauncher.launch(arrayOf("audio/*", "video/*"))
            ACTION_COMPACT_RECORD -> recordFromCompact()
            ACTION_COMPACT_STOP_RECORDING -> {
                RecordingService.stop(this)
                logToRust("mic: stop requested from the compact Library")
            }
            ACTION_COMPACT_EXPORT -> exportFromCompact(
                intent.getStringExtra(EXTRA_FORMAT) ?: "md",
                intent.getBooleanExtra(EXTRA_BILINGUAL, false),
            )
            ACTION_COMPACT_SHARE -> shareFromCompact(
                intent.getStringExtra(EXTRA_FORMAT) ?: "md",
                intent.getBooleanExtra(EXTRA_BILINGUAL, false),
            )
            ACTION_COMPACT_DOWNLOAD -> {
                val model = intent.getStringExtra(EXTRA_MODEL)
                val language = intent.getStringExtra(EXTRA_LANGUAGE) ?: "auto"
                if (model.isNullOrEmpty()) {
                    logToRust("download: the compact card named no model")
                } else if (ModelDownloadJobService.enqueue(this, model, language)) {
                    logToRust("download: scheduled $model ($language)")
                } else {
                    Toast.makeText(this, "The download could not be scheduled", Toast.LENGTH_LONG)
                        .show()
                    logToRust("download: the platform refused to schedule $model")
                }
            }
            ACTION_COMPACT_QUIT -> quitApp()
            ACTION_COMPACT_FINISH -> finishAndRemoveTask()
            ACTION_COMPACT_SHARE_CRASH -> shareCrashLog()
        }
    }

    /** Schedules the foreground job for these URIs, or says why it cannot. */
    private fun enqueueImport(uris: List<Uri>) {
        if (uris.isEmpty()) return
        val model = ModelChoice.forJob(this, "auto")
        if (model == null) {
            Toast.makeText(this, getString(R.string.transcribe_no_model_body), Toast.LENGTH_LONG)
                .show()
            logToRust("import: no model installed")
            Notify.show(
                this,
                Notify.STATUS_ID,
                Notify.status(
                    this,
                    getString(R.string.transcribe_no_model_title),
                    getString(R.string.transcribe_no_model_body),
                ),
            )
            return
        }
        val scheduled = TranscriptionJobService.enqueue(this, uris)
        if (scheduled) {
            Toast.makeText(
                this,
                resources.getQuantityString(
                    R.plurals.transcribe_queued,
                    uris.size,
                    uris.size,
                    model,
                ),
                Toast.LENGTH_LONG,
            ).show()
            logToRust("import: ${uris.size} file(s) queued with $model")
        } else {
            Toast.makeText(this, getString(R.string.transcribe_schedule_failed), Toast.LENGTH_LONG)
                .show()
            logToRust("import: the platform refused to schedule the job")
        }
    }

    /**
     * The launch-time recovery prompt: a journaled job that a process kill cut
     * before it finished is offered as **Resume** (the copy completed) or
     * **Discard**. A job whose copy was cut short has no resume: its bytes
     * cannot be read again, so the prompt says that and only offers Discard.
     */
    private fun offerRecovery() {
        if (recoveryShown) return
        recoveryShown = true
        val jobs: JSONArray = try {
            TranscribeBridge.recover().optJSONArray("jobs") ?: return
        } catch (t: Throwable) {
            Log.w(TAG, "recovery could not read the journal", t)
            return
        }
        if (jobs.length() == 0) return

        val resumable = mutableListOf<String>()
        val lines = mutableListOf<String>()
        for (index in 0 until jobs.length()) {
            val job = jobs.optJSONObject(index) ?: continue
            val title = job.optString("title").ifEmpty { getString(R.string.transcribe_unnamed) }
            if (job.optBoolean("resumable")) {
                resumable.add(job.optString("key"))
                lines.add(getString(R.string.transcribe_recovery_resume_line, title, job.optInt("percent")))
            } else {
                lines.add(
                    getString(
                        R.string.transcribe_recovery_discard_line,
                        title,
                        job.optString("reason"),
                    ),
                )
            }
        }
        val builder = AlertDialog.Builder(this)
            .setTitle(getString(R.string.transcribe_recovery_title))
            .setMessage(lines.joinToString("\n"))
            .setCancelable(false)
            .setNegativeButton(getString(R.string.transcribe_recovery_discard)) { _, _ ->
                for (index in 0 until jobs.length()) {
                    jobs.optJSONObject(index)?.optString("key")?.let(TranscribeBridge::discard)
                }
                logToRust("recovery: discarded ${jobs.length()} job(s)")
            }
        if (resumable.isNotEmpty()) {
            builder.setPositiveButton(getString(R.string.transcribe_recovery_resume)) { _, _ ->
                resumable.forEach { TranscribeBridge.resume(it) }
                TranscriptionJobService.resumePending(this)
                logToRust("recovery: resuming ${resumable.size} job(s)")
            }
        }
        builder.show()
    }

    /**
     * The completion notification's tap. One path, everywhere: the intent
     * carries the transcript id, and the compact UI host lands on **Library →
     * Editor** with the row loaded (or on Library with the "no longer in the
     * library" line). This replaced M13E's toast: the route is the host's, not
     * a message about a route that did not exist yet.
     */
    private fun showTranscript(id: String) {
        try {
            val reply = UiBridge.openTranscript(id)
            val screen = reply.optString("screen", "library")
            val title = reply.optString("title")
            logToRust(
                if (reply.optBoolean("ok")) {
                    "open transcript: $id → $screen" + if (title.isNotEmpty()) " ($title)" else ""
                } else {
                    "open transcript: $id failed — ${reply.optString("error")}"
                },
            )
        } catch (t: Throwable) {
            // Never a toast and never a crash: the tap lands, the log says why.
            Log.w(TAG, "the transcript route could not be opened", t)
            logToRust("open transcript: $id failed (${t.javaClass.simpleName})")
        }
    }

    /**
     * Android 13+ needs the runtime notification permission for the progress
     * and completion notifications. Without it the job still runs — the
     * notification is silent — so a refusal is not an error, only a quieter
     * run; the toast explains it once.
     */
    private fun requestNotificationPermission() {
        if (Build.VERSION.SDK_INT < 33) return
        if (checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED) {
            return
        }
        requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), REQUEST_NOTIFICATIONS)
    }

    // --- M13F: microphone ----------------------------------------------------

    /**
     * Surfaces a take the journal replayed after a process kill. The service
     * cannot post this itself (it died with the process); the recovery notice
     * is the "abrupt end" the workorder asks for on the next launch.
     */
    private fun showRecoveredRecording(status: JSONObject) {
        val recovered = status.optJSONArray("recovered") ?: return
        val first = recovered.optJSONObject(0) ?: return
        val title = first.optString("title").ifEmpty { getString(R.string.transcribe_unnamed) }
        val queued = first.optBoolean("handed_off")
        val frames = first.optLong("frames", 0L)
        val body = when {
            frames == 0L -> getString(R.string.mic_recovered_notification_empty, title)
            queued -> getString(R.string.mic_recovered_notification_queued, title)
            else -> getString(R.string.mic_recovered_notification, title)
        }
        Notify.show(
            this,
            Notify.STATUS_ID,
            Notify.micStatus(this, getString(R.string.mic_recovered_title), body),
        )
        logToRust("mic: replayed ${recovered.length()} recovered take(s) from the journal")
    }

    /**
     * Record was tapped on the compact Library. The permission is requested
     * **once**; after a refusal the only way forward is the system app page —
     * no repeated prompts, no silent failure.
     */
    private fun recordFromCompact() {
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) ==
            PackageManager.PERMISSION_GRANTED
        ) {
            startRecordingFromForeground()
            return
        }
        val prefs = getSharedPreferences(PREFS_MIC, MODE_PRIVATE)
        if (prefs.getBoolean(KEY_MIC_REQUESTED, false)) {
            Toast.makeText(this, getString(R.string.mic_permission_settings), Toast.LENGTH_LONG)
                .show()
            logToRust("mic: RECORD_AUDIO already refused once — opening Settings")
            openMicSettings()
            return
        }
        prefs.edit().putBoolean(KEY_MIC_REQUESTED, true).apply()
        logToRust("mic: requesting RECORD_AUDIO (once)")
        requestPermissions(arrayOf(Manifest.permission.RECORD_AUDIO), REQUEST_RECORD_AUDIO)
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode != REQUEST_RECORD_AUDIO) return
        val granted = grantResults.isNotEmpty() &&
            grantResults[0] == PackageManager.PERMISSION_GRANTED
        if (granted) {
            logToRust("mic: RECORD_AUDIO granted")
            startRecordingFromForeground()
        } else {
            val permanent = !shouldShowRequestPermissionRationale(
                Manifest.permission.RECORD_AUDIO,
            )
            val message = if (permanent) {
                R.string.mic_permission_settings
            } else {
                R.string.mic_permission_denied
            }
            Toast.makeText(this, getString(message), Toast.LENGTH_LONG).show()
            logToRust("mic: RECORD_AUDIO denied (permanent=$permanent)")
        }
    }

    /**
     * Starts the microphone service. Only from the foreground: Android 14+
     * refuses a `microphone` FGS started while the app is not visible, and the
     * Rust host independently refuses a start without the foreground claim.
     */
    private fun startRecordingFromForeground() {
        if (!hasWindowFocus()) {
            Toast.makeText(this, getString(R.string.mic_foreground_required), Toast.LENGTH_LONG)
                .show()
            logToRust("mic: start refused — the Activity does not have window focus")
            return
        }
        val model = ModelChoice.forJob(this, "auto")
        if (model == null) {
            Toast.makeText(this, getString(R.string.transcribe_no_model_body), Toast.LENGTH_LONG)
                .show()
            logToRust("mic: no model installed")
            return
        }
        if (RecordingService.isRunning()) {
            Toast.makeText(this, getString(R.string.mic_already_recording), Toast.LENGTH_LONG)
                .show()
            return
        }
        if (!RecordingService.start(this, foreground = true)) {
            Toast.makeText(this, getString(R.string.mic_unavailable_body), Toast.LENGTH_LONG)
                .show()
            logToRust("mic: the platform refused to start the service")
            return
        }
        logToRust("mic: recording service started with $model")
    }

    private fun openMicSettings() {
        val intent = Intent(android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS)
            .setData(Uri.parse("package:$packageName"))
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        try {
            startActivity(intent)
        } catch (t: Throwable) {
            Log.e(TAG, "could not open the app's Settings page", t)
            Toast.makeText(this, getString(R.string.mic_permission_settings), Toast.LENGTH_LONG)
                .show()
        }
    }

    /**
     * **The owner's Quit invariant:** capture first, service second, exit
     * third. `MicBridge.quit()` stops the stream, finalizes the WAV, journals
     * the reason and hands the take to the M13E inbox synchronously; only after
     * it returns does the service stop and the Activity leave. The compact
     * Settings sheet has already confirmed, so there is no second dialog.
     */
    private fun quitApp() {
        val reply = MicBridge.quit()
        val quit = reply.optJSONObject("quit")
        logToRust(
            "quit: capture stopped=" + (quit?.optBoolean("was_recording") ?: false) +
                ", next=" + (quit?.optJSONArray("next")?.toString() ?: "[]"),
        )
        stopService(Intent(this, RecordingService::class.java))
        finishAndRemoveTask()
    }

    // --- Android surface timeline -------------------------------------------
    // GameActivity is the SurfaceHolder.Callback; overriding these (and calling
    // super) puts the exact platform surface timeline into the event ring, which
    // is how "the surface died and winit never said so" becomes visible.

    override fun surfaceCreated(holder: SurfaceHolder) {
        super.surfaceCreated(holder)
        logToRust("surfaceCreated")
    }

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
        super.surfaceChanged(holder, format, width, height)
        logToRust("surfaceChanged ${width}x$height fmt $format")
    }

    override fun surfaceDestroyed(holder: SurfaceHolder) {
        logToRust("surfaceDestroyed")
        super.surfaceDestroyed(holder)
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        logToRust("onWindowFocusChanged $hasFocus")
        // Focus changes are a cheap, reliable moment to re-read the window
        // insets: system bars, gesture handles and cutouts can change while the
        // app is backgrounded without any inset dispatch reaching us.
        if (hasFocus) {
            root?.let { ViewCompat.requestApplyInsets(it) }
            pushCurrentInsets()
        }
    }

    private fun installInsetsListener() {
        val view = window.decorView
        ViewCompat.setOnApplyWindowInsetsListener(view) { _, insets ->
            pushInsets(insets)
            insets
        }
        ViewCompat.requestApplyInsets(view)
        // The first dispatch can arrive before the decor view is laid out, and
        // `ViewCompat.getRootWindowInsets` is null until it is. v2 asked once and
        // then depended on the platform to dispatch a *change*: if the first ask
        // was early, the panel sat at zeroes forever. Ask on the next frame and
        // once more after layout — cheap, and it makes the panel populate at
        // launch without a configuration change.
        view.post { pushCurrentInsets() }
        view.postDelayed({ pushCurrentInsets() }, 300L)
    }

    /**
     * Push the current insets from wherever they are available now. Called on
     * every dispatch, on resume, on focus gain, after layout, and on Rust's
     * request (`postPushInsets`). Never throws: a missing window-insets object
     * means "ask for a dispatch", not "leave the panel at zero".
     */
    private fun pushCurrentInsets() {
        val view = root ?: return
        val insets = ViewCompat.getRootWindowInsets(view)
        if (insets == null) {
            ViewCompat.requestApplyInsets(view)
            return
        }
        pushInsets(insets)
    }

    private fun pushInsets(insets: WindowInsetsCompat) {
        val bars = insets.getInsets(
            WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout()
        )
        val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
        lastImeBottom = ime.bottom
        nativeSetInsets(bars.top, bars.bottom, bars.left, bars.right, ime.bottom)
    }

    /**
     * Called from Rust over JNI when the UI notices its inset values are stale
     * (a reset button, or a first push that never arrived). The Rust thread must
     * not touch a View, so this hops to the UI thread first.
     */
    @Suppress("unused") // referenced from platform.rs via JNI
    fun postPushInsets() {
        Handler(Looper.getMainLooper()).post { pushCurrentInsets() }
    }

    private fun installBackHandler() {
        // Three dispatch paths, one funnel — because "back" is not one code path
        // on Android any more:
        //
        //  1. `OnBackPressedDispatcher` (androidx): the legacy path, used below
        //     API 33 and by the 3-button bar when predictive back is off.
        //  2. `OnBackInvokedCallback` (API 33+): the predictive-back path. When
        //     `android:enableOnBackInvokedCallback="true"` is in the manifest the
        //     platform asks the *window's* dispatcher and never synthesises a
        //     KEYCODE_BACK — without this registration the system's default
        //     callback ("finish the Activity") wins, which is exactly what the
        //     owner saw: first press swallowed, second press landed on the
        //     launcher.
        //  3. `dispatchKeyEvent`: a platform/OEM that still delivers a legacy
        //     KEYCODE_BACK (or a hardware keyboard's back key) must not fall
        //     through to `finish()` either.
        //
        // `handleBack` dedupes, so a press that reaches two of these paths is
        // still counted once. Back never leaves the app while the checks are on
        // screen; the crash overlay's "Close app" is the way out after a native
        // death.
        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() {
                handleBack("OnBackPressed")
            }
        })
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            try {
                window.onBackInvokedDispatcher.registerOnBackInvokedCallback(
                    OnBackInvokedDispatcher.PRIORITY_DEFAULT,
                ) { handleBack("OnBackInvoked") }
            } catch (t: Throwable) {
                logToRust("back: could not register OnBackInvokedCallback (${t.javaClass.simpleName})")
                Log.e(TAG, "OnBackInvokedCallback registration failed", t)
            }
        }
    }

    /**
     * A legacy back key event. Consumed on both edges so the platform's default
     * `finish()` never sees it — except while the soft keyboard is up, where
     * back is supposed to close the keyboard and the IME owns the gesture.
     */
    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        if (event.keyCode == KeyEvent.KEYCODE_BACK) {
            if (lastImeBottom > 0) {
                return super.dispatchKeyEvent(event)
            }
            if (event.action == KeyEvent.ACTION_DOWN) {
                handleBack("key")
            }
            return true
        }
        return super.dispatchKeyEvent(event)
    }

    /**
     * Count one back press. Called from whichever dispatch path fired; a press
     * that reaches two paths inside [BACK_DEDUPE_MS] is counted once. Nothing
     * here may throw: a failing JNI call must leave the counter (and the app)
     * alone rather than closing it.
     */
    private fun handleBack(source: String) {
        val now = SystemClock.uptimeMillis()
        if (now - lastBackAtMs < BACK_DEDUPE_MS) {
            logToRust("back ($source): same press already counted — ignored")
            return
        }
        lastBackAtMs = now
        // M13g1: the compact stack gets the first answer. A dismissed sheet or a
        // popped screen consumes the gesture; `finish:true` falls through to the
        // v2.4 spike path below (and, in G2, to `finishAndRemoveTask`).
        try {
            val reply = UiBridge.back()
            if (reply.optBoolean("ok")) {
                if (reply.optBoolean("finish")) {
                    logToRust("back ($source): compact root — finishing the Activity")
                    finishAndRemoveTask()
                    return
                }
                logToRust(
                    "back ($source): compact ${reply.optString("back")} → ${reply.optString("screen")}",
                )
                return
            }
        } catch (t: Throwable) {
            Log.w(TAG, "the compact back route failed", t)
        }
        try {
            nativeBackPressed()
            logToRust(
                "back ($source): intercepted, counted" +
                    if (nativePanicked) " (native side is dead — counted locally)" else "",
            )
        } catch (t: Throwable) {
            logToRust("back ($source): nativeBackPressed failed (${t.javaClass.simpleName})")
            Log.e(TAG, "nativeBackPressed failed", t)
        }
    }

    /**
     * M13g2 — the compact editor's **Export** action.
     *
     * Kotlin formats nothing: the Rust host renders through the same M15
     * serializers the desktop uses, stages the bytes atomically, and returns
     * the one file. This opens `ACTION_CREATE_DOCUMENT` with that format's
     * MIME type and suggested name; [exportLauncher] writes and verifies.
     *
     * Called by the compact surface's action dispatcher. The surface is drawn
     * by the Deck on the host and by the M13b shell's window on the phone, so
     * this entry is the seam between them — no screen invents an export.
     */
    @Suppress("unused")
    fun exportFromCompact(format: String, bilingual: Boolean) {
        val reply = UiBridge.export(format, bilingual)
        if (!reply.optBoolean("ok")) {
            val reason = reply.optString("error", "the export could not be staged")
            logToRust("export: refused — $reason")
            Toast.makeText(this, reason, Toast.LENGTH_LONG).show()
            return
        }
        val staged = ExportWriter.Staged.from(reply)
        if (staged == null) {
            logToRust("export: the host returned an incomplete job")
            return
        }
        pendingExport = staged
        try {
            exportLauncher.launch(staged)
            logToRust("export: picker opened for ${staged.name} (${staged.bytes} bytes)")
        } catch (t: Throwable) {
            pendingExport = null
            UiBridge.exportCancel(staged.job)
            logToRust("export: the picker could not open (${t.javaClass.simpleName})")
            Toast.makeText(this, "No file picker available", Toast.LENGTH_LONG).show()
        }
    }

    /**
     * M13g2 — the sheet's **Share instead** action: the same staged file
     * handed to the share chooser (`ACTION_SEND`, one file). The staged copy
     * stays until the next process start so the receiving app can read it.
     */
    @Suppress("unused")
    fun shareFromCompact(format: String, bilingual: Boolean) {
        val reply = UiBridge.exportShare(format, bilingual)
        if (!reply.optBoolean("ok")) {
            val reason = reply.optString("error", "the export could not be staged")
            logToRust("export: share refused — $reason")
            Toast.makeText(this, reason, Toast.LENGTH_LONG).show()
            return
        }
        val staged = ExportWriter.Staged.from(reply)
        if (staged == null) {
            logToRust("export: the host returned an incomplete share job")
            return
        }
        try {
            startActivity(Intent.createChooser(ExportWriter.shareIntent(this, staged), "Share transcript"))
            UiBridge.exportShared(staged.job)
            logToRust("export: shared ${staged.name} (${staged.bytes} bytes)")
        } catch (t: Throwable) {
            UiBridge.exportCancel(staged.job)
            logToRust("export: the share chooser did not open (${t.javaClass.simpleName})")
        }
    }

    // v2.3 removed postRecreate() from here. If a future milestone ever needs an
    // Activity restart it is finish() + an explicit relaunch intent — a teardown
    // the system actually performs — never Activity.recreate(), which on this
    // stack would be a framework relaunch racing android-activity's unconditional
    // GameActivity_finish() when android_main returns. See
    // docs/reports/M13b5-spike-v2.3.md §4, and §6 for why the absence of a call
    // here is the whole disarm.

    /**
     * Called from Rust after a panic escaped `android_main`: the surface will
     * never draw again, so the only way to show the reason is this overlay.
     */
    @Suppress("unused") // referenced from platform.rs via JNI
    fun postShowCrashOverlay() {
        nativePanicked = true
        // `root` is the decor view captured on the UI thread in onCreate; post
        // through it rather than touching `window` from the JNI thread.
        root?.post { showCrashOverlayIfPresent("rust-panic", force = true) }
    }

    // --- Crash overlay -------------------------------------------------------

    /**
     * The crash record as on-screen text. Installed once per launch, refreshed
     * in place when Rust reports a new panic, and hidden (not deleted) by its
     * own button. Deletion belongs to the egui **Clear crash** button, so the
     * record survives a hide and reappears at the next launch.
     */
    private fun showCrashOverlayIfPresent(tag: String, force: Boolean) {
        val text = CrashFiles.lastCrashText(this) ?: return
        val body = overlayBody
        if (body != null) {
            if (force) {
                body.text = text
                overlay?.visibility = View.VISIBLE
            }
            return
        }
        val (container, textView) = buildOverlay(text, tag == "rust-panic")
        // Sibling of GameActivity's SurfaceView: same window, added later, so it
        // draws on top of the (default z-order, behind-the-window) surface.
        addContentView(
            container,
            FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                resources.displayMetrics.heightPixels * 55 / 100,
                Gravity.TOP,
            ),
        )
        container.bringToFront()
        overlay = container
        overlayBody = textView
        logToRust("crash overlay shown ($tag, ${text.length} chars)")
    }

    private fun buildOverlay(text: String, fromRustPanic: Boolean): Pair<View, TextView> {
        val density = resources.displayMetrics.density
        fun dp(value: Int) = (value * density).toInt()

        val container = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(0xF00B0B0E.toInt())
            setPadding(dp(14), dp(12), dp(14), dp(10))
            // Swallow taps so they do not reach the surface underneath, but stay
            // non-focusable: a live surface must keep the IME and its input.
            isClickable = true
            isFocusable = false
        }

        val title = if (fromRustPanic) {
            "⚠ NATIVE CRASH — THIS SESSION"
        } else {
            "⚠ CRASH RECORDED — LAST SESSION"
        }
        container.addView(
            TextView(this).apply {
                this.text = title
                setTextColor(0xFFFF6B6B.toInt())
                setTextSize(TypedValue.COMPLEX_UNIT_SP, 17f)
                setTypeface(Typeface.DEFAULT_BOLD)
            }
        )
        container.addView(
            TextView(this).apply {
                this.text =
                    "${CrashFiles.location(this@MainActivity)}/${CrashFiles.LAST_CRASH}\n" +
                        "history: ${CrashFiles.HISTORY}. \"Share log\" sends it to any app; " +
                        "\"Close app\" leaves the app. Clear it with the app's \"Clear crash\" " +
                        "button. Screenshot this."
                setTextColor(0xFFB9B9C0.toInt())
                setTextSize(TypedValue.COMPLEX_UNIT_SP, 10f)
            }
        )

        val body = TextView(this).apply {
            this.text = text
            typeface = Typeface.MONOSPACE
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 9.5f)
            setTextColor(0xFFFFE2DE.toInt())
            setPadding(0, dp(6), 0, dp(6))
        }
        val scroll = ScrollView(this).apply {
            addView(
                body,
                ViewGroup.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT,
                    ViewGroup.LayoutParams.WRAP_CONTENT,
                ),
            )
        }
        container.addView(
            scroll,
            LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f),
        )

        val row = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL }
        row.addView(
            Button(this).apply {
                this.text = "Share log"
                // Pure Kotlin: it reads the same file the overlay was built
                // from, so it works exactly when the Rust surface does not.
                setOnClickListener { shareCrashLog() }
            },
            LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f),
        )
        row.addView(
            Button(this).apply {
                this.text = "Close app"
                setOnClickListener {
                    logToRust("crash overlay: close requested")
                    finish()
                }
            },
            LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f),
        )
        row.addView(
            Button(this).apply {
                this.text = "Hide"
                setOnClickListener { container.visibility = View.GONE }
            },
            LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f),
        )
        container.addView(row)
        return container to body
    }

    /**
     * Hand the crash file to the system share sheet.
     *
     * `FileProvider` + `ACTION_SEND`: the file is app-private storage, so a
     * `file://` URI would be refused by the platform (FileUriExposedException);
     * a content URI with a one-shot read grant is the supported route, and it
     * needs no permission and no extra dependency (the provider ships in
     * `androidx.core`, which appcompat already brings in).
     *
     * Everything here is best-effort: a missing file, a missing provider or a
     * device with no share target must leave the banner usable, so each failure
     * becomes a toast plus a line in the event ring rather than an exception.
     */
    private fun shareCrashLog() {
        val file = CrashFiles.shareableFile(this)
        if (file == null) {
            logToRust("share: no crash file to share")
            Toast.makeText(this, "No crash file to share", Toast.LENGTH_SHORT).show()
            return
        }
        try {
            val uri = FileProvider.getUriForFile(this, "$packageName.fileprovider", file)
            val send = Intent(Intent.ACTION_SEND).apply {
                type = "text/plain"
                putExtra(Intent.EXTRA_STREAM, uri)
                putExtra(Intent.EXTRA_SUBJECT, "Suramak spike crash log")
                clipData = ClipData.newRawUri("Suramak crash log", uri)
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }
            val chooser = Intent.createChooser(send, "Share crash log")
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            startActivity(chooser)
            logToRust("share: ${file.name} (${file.length()} bytes) → share sheet")
        } catch (t: Throwable) {
            logToRust("share failed: ${t.javaClass.simpleName}: ${t.message}")
            Log.e(TAG, "share failed for ${file.absolutePath}", t)
            Toast.makeText(this, "Could not share: ${t.message}", Toast.LENGTH_LONG).show()
        }
    }

    /**
     * v2.3 — the recreate disarm, made observable.
     *
     * The framework has no public `isRecreating()`: the relaunch flag lives in
     * `ActivityThread`'s client record, not on `Activity`. What *is* readable is
     * every state a pending or half-finished relaunch would show up in — a
     * configuration change in progress, a finishing Activity, a destroyed one.
     * Printing them at each resume/stop means the device log can show the
     * Activity comes back clean after a "recreate" tap, which is the only
     * on-device evidence this class can offer (the tap itself makes no Java
     * call: there is no recreate entry point at all).
     */
    private fun logSettle(tag: String) {
        logToRust(
            "settle ($tag): changingConfigurations=$isChangingConfigurations " +
                "finishing=$isFinishing destroyed=$isDestroyed"
        )
    }

    /**
     * Lifecycle lines for the on-screen event ring. A missing symbol (library
     * not loaded) or a dead Rust side must never turn a log call into a crash.
     */
    private fun logToRust(line: String) {
        try {
            nativeLogLine(line)
        } catch (t: Throwable) {
            Log.d(TAG, "rust log unavailable: $line")
        }
    }
}
