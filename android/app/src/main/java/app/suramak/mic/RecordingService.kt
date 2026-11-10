package app.suramak.mic

import android.Manifest
import android.app.ActivityManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.media.AudioAttributes
import android.media.AudioDeviceCallback
import android.media.AudioDeviceInfo
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import android.util.Log
import app.suramak.spike.R
import app.suramak.transcribe.ModelChoice
import app.suramak.transcribe.Notify
import app.suramak.transcribe.TranscribeBridge
import app.suramak.transcribe.TranscriptionJobService
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import org.json.JSONObject

/**
 * M13F — the `microphone`-typed foreground service that owns a recording.
 *
 * **Started only from the foreground.** Android 14+ forbids starting a
 * microphone foreground service while the app is not visible (the
 * while-in-use rule). The Activity passes the foreground claim, this class
 * re-checks its own process importance, and Rust refuses a start that does not
 * claim it — three layers, so no path can open a microphone from the
 * background. The service also calls [startForeground] **before** it asks Rust
 * to open the stream: the platform must see the microphone type first.
 *
 * **The persistent notification** carries a system chronometer (elapsed time,
 * truthful even while the app is frozen) and a Stop action that works from the
 * lock screen. Its body is the last-known state: "screen locked keeps it going"
 * normally, "Doze is active" when the device enters idle mode.
 *
 * **Interruptions stop cleanly.** Audio-focus loss, a call (audio mode
 * `MODE_IN_CALL`/`MODE_IN_COMMUNICATION`), a Bluetooth input removal, a change
 * of input device, `onTaskRemoved`, the foreground-service timeout and
 * `onDestroy` all call [stopCapture] with a named reason. The Rust host closes
 * the stream, finalizes the WAV, journals the kind and hands the take to the
 * M13E inbox — the recording never continues silently into a call.
 *
 * **Process death** leaves the Rust journal and the streamed WAV; the next
 * launch's `MicHost::recover` repairs the header and adopts the take. This
 * service can only surface the last-known state in the notification while it
 * lives; the journal is the durable record (report §"locked-screen boundary").
 */
class RecordingService : Service() {

    /** Set only while the Rust host reports a live capture. */
    @Volatile
    private var active = false

    private var wakeLock: PowerManager.WakeLock? = null
    private var focusRequest: AudioFocusRequest? = null
    private var deviceCallback: AudioDeviceCallback? = null
    private var ticker: Handler? = null
    private var tickerTask: Runnable? = null
    private var startedWallMs = 0L
    private var currentTitle = ""
    private var startedDevice = ""
    private var lastIdle = false

    override fun onCreate() {
        super.onCreate()
        // The job host first: the take's hand-off writes into its inbox.
        TranscribeBridge.init(this)
        MicBridge.init(this)
        Notify.ensureChannels(this)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_START -> return begin(intent)
            ACTION_STOP -> {
                stopCapture(REASON_USER)
                return START_NOT_STICKY
            }
            else -> return START_NOT_STICKY
        }
    }

    /** Promotes to the foreground, opens the microphone, starts the ticker. */
    private fun begin(intent: Intent): Int {
        if (active) return START_NOT_STICKY
        val claimed = intent.getBooleanExtra(EXTRA_FOREGROUND, false)
        if (!claimed || !appIsForeground()) {
            // Never promote a microphone service from the background. No
            // notification: there is nothing recording to tell the user about.
            Log.w(TAG, "refusing to start the microphone outside the foreground")
            stopSelf()
            return START_NOT_STICKY
        }
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            Notify.show(
                this,
                Notify.STATUS_ID,
                Notify.micStatus(
                    this,
                    getString(R.string.mic_permission_title),
                    getString(R.string.mic_permission_body),
                ),
            )
            stopSelf()
            return START_NOT_STICKY
        }
        val model = ModelChoice.forJob(this, "auto")
        if (model == null) {
            Notify.show(
                this,
                Notify.STATUS_ID,
                Notify.micStatus(
                    this,
                    getString(R.string.transcribe_no_model_title),
                    getString(R.string.transcribe_no_model_body),
                ),
            )
            stopSelf()
            return START_NOT_STICKY
        }

        val stamp = SimpleDateFormat("yyyy-MM-dd HH:mm", Locale.getDefault()).format(Date())
        currentTitle = getString(R.string.mic_title_prefix, stamp)
        startedWallMs = System.currentTimeMillis()
        lastIdle = false
        val notification = Notify.recording(
            this,
            currentTitle,
            startedWallMs,
            getString(R.string.mic_locked_screen_body),
        )
        try {
            startForeground(
                Notify.RECORDING_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE,
            )
        } catch (t: Throwable) {
            // On API 34+ this is the while-in-use refusal as a SecurityException
            // (or ForegroundServiceStartNotAllowedException); there is no
            // recording to keep, only a sentence to show.
            Log.e(TAG, "the microphone foreground service was refused", t)
            Notify.show(
                this,
                Notify.STATUS_ID,
                Notify.micStatus(
                    this,
                    getString(R.string.mic_unavailable_title),
                    getString(R.string.mic_unavailable_body),
                ),
            )
            stopSelf()
            return START_NOT_STICKY
        }

        acquireWakeLock()
        if (!requestFocus()) {
            cleanup()
            stopForeground(STOP_FOREGROUND_REMOVE)
            Notify.show(
                this,
                Notify.STATUS_ID,
                Notify.micStatus(
                    this,
                    getString(R.string.mic_stopped_title),
                    getString(R.string.mic_stopped_focus_unavailable),
                ),
            )
            stopSelf()
            return START_NOT_STICKY
        }
        registerDeviceCallback()

        val reply = MicBridge.start(
            foreground = true,
            permission = true,
            title = currentTitle,
            model = model,
            language = "auto",
        )
        if (!reply.optBoolean("ok", false)) {
            val reason = reply.optString("refused").ifEmpty {
                reply.optString("error").ifEmpty { getString(R.string.mic_unavailable_body) }
            }
            Log.w(TAG, "the recording did not start: $reason")
            cleanup()
            stopForeground(STOP_FOREGROUND_REMOVE)
            Notify.show(
                this,
                Notify.STATUS_ID,
                Notify.micStatus(this, getString(R.string.mic_unavailable_title), reason),
            )
            stopSelf()
            return START_NOT_STICKY
        }
        active = true
        running = true
        startedDevice = reply.optString("input_device")
        startTicker()
        return START_NOT_STICKY
    }

    /**
     * Stops the take through the Rust host (which finalizes the WAV, journals
     * the reason and hands it to the M13E inbox), then ends the foreground
     * notification and the service.
     */
    private fun stopCapture(reason: String) {
        if (!active) {
            stopSelf()
            return
        }
        active = false
        val reply = MicBridge.stop(reason)
        running = false
        cleanup()
        stopForeground(STOP_FOREGROUND_REMOVE)
        postOutcome(reply, reason)
        stopSelf()
    }

    /** The host already settled the take (stream error); no second stop call. */
    private fun finishAfterHostStop(status: JSONObject) {
        if (!active) return
        active = false
        running = false
        cleanup()
        stopForeground(STOP_FOREGROUND_REMOVE)
        val reason = status.optJSONObject("last_stop")?.optString("reason")
            ?.takeIf { it.isNotEmpty() } ?: REASON_STREAM_ERROR
        postOutcome(status, reason)
        stopSelf()
    }

    private fun postOutcome(reply: JSONObject, reason: String) {
        val stopped = reply.optJSONObject("last_stop")
        val title = stopped?.optString("title")?.takeIf { it.isNotEmpty() } ?: currentTitle
        val handed = stopped?.optBoolean("handed_off") == true
        val frames = stopped?.optLong("frames", 0) ?: 0
        val reasonText = reasonText(reason)
        val body = when {
            frames == 0L -> getString(R.string.mic_saved_empty, reasonText)
            handed -> getString(R.string.mic_saved_queued, reasonText)
            else -> getString(R.string.mic_saved, reasonText)
        }
        Notify.show(
            this,
            Notify.STATUS_ID,
            Notify.micStatus(this, getString(R.string.mic_saved_title, title), body),
        )
        if (handed) {
            // The WAV is already a queued job in the M13E host; this is the
            // exact pump the import path uses after `import_done`.
            TranscriptionJobService.resumePending(this)
        }
    }

    private fun reasonText(reason: String): String = when (reason) {
        REASON_CALL -> getString(R.string.mic_stopped_call)
        REASON_FOCUS -> getString(R.string.mic_stopped_focus)
        REASON_BLUETOOTH -> getString(R.string.mic_stopped_bluetooth)
        REASON_TIMEOUT -> getString(R.string.mic_stopped_timeout)
        REASON_SERVICE -> getString(R.string.mic_stopped_service)
        REASON_STREAM_ERROR -> getString(R.string.mic_stopped_stream_error)
        else -> getString(R.string.mic_stopped_user)
    }

    // --- the ticker: call, device swap, Doze ------------------------------

    private fun startTicker() {
        val handler = Handler(Looper.getMainLooper())
        val task = object : Runnable {
            override fun run() {
                if (!active) return
                val status = MicBridge.status()
                if (!status.optBoolean("recording", false)) {
                    finishAfterHostStop(status)
                    return
                }
                val device = status.optString("input_device")
                if (startedDevice.isNotEmpty() && device.isNotEmpty() &&
                    device != startedDevice
                ) {
                    Log.i(TAG, "input device changed: $startedDevice -> $device")
                    MicBridge.note("input device changed from $startedDevice to $device")
                    stopCapture(REASON_BLUETOOTH)
                    return
                }
                val audio = getSystemService(AudioManager::class.java)
                if (audio != null &&
                    (audio.mode == AudioManager.MODE_IN_CALL ||
                        audio.mode == AudioManager.MODE_IN_COMMUNICATION)
                ) {
                    // A call is not distinguishable from a generic focus loss
                    // without READ_PHONE_STATE, which this app deliberately does
                    // not request; the audio mode is the permission-free signal
                    // (a call necessarily sets it).
                    Log.i(TAG, "a call owns the audio path (mode ${audio.mode})")
                    stopCapture(REASON_CALL)
                    return
                }
                val idle = getSystemService(PowerManager::class.java)?.isDeviceIdleMode == true
                if (idle != lastIdle) {
                    lastIdle = idle
                    updateNotification()
                }
                handler.postDelayed(this, TICK_MS)
            }
        }
        handler.post(task)
        ticker = handler
        tickerTask = task
    }

    private fun updateNotification() {
        if (!running) return
        val body = if (lastIdle) {
            getString(R.string.mic_doze_body)
        } else {
            getString(R.string.mic_locked_screen_body)
        }
        Notify.show(
            this,
            Notify.RECORDING_ID,
            Notify.recording(this, currentTitle, startedWallMs, body),
        )
    }

    // --- platform listeners ------------------------------------------------

    /** Requests audio focus; a refusal means the take must not start. */
    private fun requestFocus(): Boolean {
        val audio = getSystemService(AudioManager::class.java) ?: return false
        val attributes = AudioAttributes.Builder()
            .setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
            .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
            .build()
        val request = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
            .setAudioAttributes(attributes)
            .setAcceptsDelayedFocusGain(false)
            .setOnAudioFocusChangeListener(
                { change ->
                    if (active && change != AudioManager.AUDIOFOCUS_GAIN) {
                        Log.i(TAG, "audio focus changed: $change")
                        MicBridge.note("audio focus changed to $change")
                        stopCapture(REASON_FOCUS)
                    }
                },
                Handler(Looper.getMainLooper()),
            )
            .build()
        focusRequest = request
        return audio.requestAudioFocus(request) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
    }

    private fun abandonFocus() {
        val request = focusRequest ?: return
        focusRequest = null
        getSystemService(AudioManager::class.java)?.abandonAudioFocusRequest(request)
    }

    private fun registerDeviceCallback() {
        val audio = getSystemService(AudioManager::class.java) ?: return
        val callback = object : AudioDeviceCallback() {
            override fun onAudioDevicesRemoved(removedDevices: Array<out AudioDeviceInfo>?) {
                if (!active || removedDevices == null) return
                val bluetooth = removedDevices.any { device ->
                    device.type == AudioDeviceInfo.TYPE_BLUETOOTH_SCO ||
                        (Build.VERSION.SDK_INT >= 31 &&
                            device.type == AudioDeviceInfo.TYPE_BLE_HEADSET)
                }
                if (bluetooth) {
                    Log.i(TAG, "a Bluetooth input device was removed")
                    MicBridge.note("a Bluetooth input device was removed")
                    stopCapture(REASON_BLUETOOTH)
                }
            }
        }
        audio.registerAudioDeviceCallback(callback, Handler(Looper.getMainLooper()))
        deviceCallback = callback
    }

    private fun unregisterDeviceCallback() {
        val callback = deviceCallback ?: return
        deviceCallback = null
        getSystemService(AudioManager::class.java)?.unregisterAudioDeviceCallback(callback)
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

    private fun cleanup() {
        tickerTask?.let { task -> ticker?.removeCallbacks(task) }
        tickerTask = null
        ticker = null
        abandonFocus()
        unregisterDeviceCallback()
        releaseWakeLock()
    }

    // --- lifecycle ---------------------------------------------------------

    /**
     * The user swiped the app away. The microphone service must not keep
     * recording unseen: stop cleanly (the take is saved and queued) and end.
     */
    override fun onTaskRemoved(rootIntent: Intent?) {
        stopCapture(REASON_SERVICE)
        super.onTaskRemoved(rootIntent)
    }

    /** Android 15's foreground-service time limit (this type is exempt, but the contract is kept). */
    override fun onTimeout(startId: Int) {
        stopCapture(REASON_TIMEOUT)
        stopSelf()
    }

    override fun onLowMemory() {
        MicBridge.note("the system reported low memory while recording")
        super.onLowMemory()
    }

    override fun onTrimMemory(level: Int) {
        // The numeric trim levels are stable platform ABI; the named
        // `ComponentCallbacks2` constants are deprecated on API 34, and the
        // `allWarningsAsErrors` gate would turn using one into a build failure.
        if (level >= TRIM_MEMORY_RUNNING_CRITICAL) {
            MicBridge.note("the system is critically low on memory while recording")
        }
        super.onTrimMemory(level)
    }

    override fun onDestroy() {
        // Destroyed mid-take (the process is going away): settle it now so the
        // journal and WAV are consistent for the next launch.
        if (active) {
            stopCapture(REASON_SERVICE)
        } else {
            cleanup()
        }
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    /** Whether this app's own process is visible (the while-in-use test). */
    private fun appIsForeground(): Boolean {
        val manager = getSystemService(ActivityManager::class.java) ?: return false
        val processes = manager.runningAppProcesses ?: return false
        return processes.any { process ->
            process.processName == packageName &&
                process.importance <= ActivityManager.RunningAppProcessInfo.IMPORTANCE_VISIBLE
        }
    }

    companion object {
        private const val TAG = "SuramakMic"
        private const val ACTION_START = "app.suramak.mic.START"
        private const val ACTION_STOP = "app.suramak.mic.STOP"
        private const val EXTRA_FOREGROUND = "app.suramak.mic.FOREGROUND"
        private const val TICK_MS = 1_000L
        private const val WAKE_TAG = "suramak:mic"
        private const val MAX_WAKE_MS = 2 * 60 * 60 * 1000L
        private const val TRIM_MEMORY_RUNNING_CRITICAL = 15

        // The Rust host's spellings (`StopReason::as_str`).
        private const val REASON_USER = "user"
        private const val REASON_CALL = "call"
        private const val REASON_FOCUS = "audio_focus_loss"
        private const val REASON_BLUETOOTH = "bluetooth_disconnect"
        private const val REASON_SERVICE = "service_stop"
        private const val REASON_TIMEOUT = "timeout"
        private const val REASON_STREAM_ERROR = "stream_error"

        @Volatile
        private var running = false

        /** The Rust host reports a live take (authoritative, unlike [running]). */
        fun isRunning(): Boolean = running

        /**
         * Starts the service from the foreground. Returns `false` when the
         * platform refused to create it; the caller reports that, never retries
         * silently from the background.
         */
        fun start(context: Context, foreground: Boolean): Boolean {
            val intent = Intent(context, RecordingService::class.java)
                .setAction(ACTION_START)
                .putExtra(EXTRA_FOREGROUND, foreground)
            return try {
                context.startForegroundService(intent)
                true
            } catch (t: Throwable) {
                Log.e(TAG, "could not start the recording service", t)
                false
            }
        }

        /** The notification/button Stop: a normal service command. */
        fun stop(context: Context) {
            try {
                context.startService(
                    Intent(context, RecordingService::class.java).setAction(ACTION_STOP),
                )
            } catch (t: Throwable) {
                Log.e(TAG, "could not tell the recording service to stop", t)
            }
        }
    }
}
