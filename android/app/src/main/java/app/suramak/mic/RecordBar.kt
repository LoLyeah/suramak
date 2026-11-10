package app.suramak.mic

import android.Manifest
import android.app.Activity
import android.content.pm.PackageManager
import android.graphics.Color
import android.os.Handler
import android.os.Looper
import android.util.TypedValue
import android.view.Gravity
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import app.suramak.spike.R

/**
 * M13F — the pre-compact-UI record control.
 *
 * M13G replaces this bar with the Deck's compact recording surface; until then
 * it is the honest button the deliverable asks for: on the existing spike-era
 * screen, small, at the bottom, and never a new navigation surface. It reads
 * the Rust host's status (the authoritative phase) rather than keeping its own
 * recording flag, so a stop that happened in the service, the notification or
 * an interruption is reflected here within half a second.
 *
 * The disabled state is a real state: without `RECORD_AUDIO` the Record button
 * is disabled and the reason is on screen, with a Settings button that opens
 * the system app page. There is no silent refusal and no fake "tap again".
 */
class RecordBar(
    private val activity: Activity,
    private val listener: Listener,
) : LinearLayout(activity) {

    interface Listener {
        /** Record was tapped with the permission already granted. */
        fun onRecord()

        /** Stop was tapped while the Rust host reports a live take. */
        fun onStopRecording()

        /** The user wants to grant the microphone in the system app settings. */
        fun onOpenSettings()

        /** Quit: capture first, then the service, then exit. */
        fun onQuit()
    }

    private val status: TextView
    private val primary: Button
    private val settings: Button
    private val quit: Button
    private val handler = Handler(Looper.getMainLooper())
    private val tick = object : Runnable {
        override fun run() {
            refresh()
            handler.postDelayed(this, 500L)
        }
    }

    init {
        orientation = VERTICAL
        setPadding(dp(12), dp(8), dp(12), dp(10))
        setBackgroundColor(Color.argb(232, 21, 27, 34))
        elevation = dp(8).toFloat()

        status = TextView(activity).apply {
            setTextColor(Color.WHITE)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 13f)
            maxLines = 2
        }
        addView(
            status,
            LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.WRAP_CONTENT),
        )

        val row = LinearLayout(activity).apply {
            orientation = HORIZONTAL
            gravity = Gravity.END or Gravity.CENTER_VERTICAL
            setPadding(0, dp(6), 0, 0)
        }
        settings = Button(activity).apply {
            text = activity.getString(R.string.mic_settings)
            setOnClickListener { listener.onOpenSettings() }
            visibility = GONE
        }
        quit = Button(activity).apply {
            text = activity.getString(R.string.mic_quit)
            setOnClickListener { listener.onQuit() }
        }
        primary = Button(activity).apply {
            text = activity.getString(R.string.mic_record)
            setOnClickListener {
                val recording = runCatching {
                    MicBridge.status().optBoolean("recording", false)
                }.getOrDefault(false)
                if (recording) listener.onStopRecording() else listener.onRecord()
            }
        }
        row.addView(settings)
        row.addView(quit)
        row.addView(primary)
        addView(row, LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.WRAP_CONTENT))
        refresh()
    }

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        refresh()
    }

    override fun onDetachedFromWindow() {
        stopTicker()
        super.onDetachedFromWindow()
    }

    /**
     * Starts the half-second status poll. The Activity calls this from
     * `onResume` only: while the screen is locked the bar must not keep waking
     * the UI thread (and Doze must be left to the service's wake lock).
     */
    fun startTicker() {
        handler.removeCallbacks(tick)
        handler.post(tick)
    }

    fun stopTicker() {
        handler.removeCallbacks(tick)
    }

    /** Re-reads permission + the Rust host's status; never throws into the UI. */
    fun refresh() {
        val granted = activity.checkSelfPermission(Manifest.permission.RECORD_AUDIO) ==
            PackageManager.PERMISSION_GRANTED
        val reply = runCatching { MicBridge.status() }.getOrNull()
        val recording = reply?.optBoolean("recording", false) == true
        val recovered = reply?.optJSONArray("recovered")
        settings.visibility = if (granted) GONE else VISIBLE

        when {
            recording -> {
                val seconds = (reply?.optLong("elapsed_ms", 0L) ?: 0L) / 1_000L
                status.text = activity.getString(
                    R.string.mic_bar_recording,
                    formatElapsed(seconds),
                )
                primary.text = activity.getString(R.string.mic_stop)
                primary.isEnabled = true
                primary.alpha = 1f
            }
            !granted -> {
                status.text = activity.getString(R.string.mic_bar_denied)
                primary.text = activity.getString(R.string.mic_record)
                primary.isEnabled = false
                primary.alpha = 0.4f
            }
            reply?.optBoolean("ok", false) != true -> {
                // The native host did not answer (a library/init problem):
                // recording cannot work, so say so instead of offering a
                // button that would fail silently.
                status.text = activity.getString(R.string.mic_unavailable_body)
                primary.text = activity.getString(R.string.mic_record)
                primary.isEnabled = false
                primary.alpha = 0.4f
            }
            recovered != null && recovered.length() > 0 -> {
                val first = recovered.optJSONObject(0)
                val title = first?.optString("title")?.takeIf { it.isNotEmpty() }
                    ?: activity.getString(R.string.transcribe_unnamed)
                val queued = first?.optBoolean("handed_off") == true
                val frames = first?.optLong("frames", 0L) ?: 0L
                status.text = when {
                    frames == 0L -> activity.getString(R.string.mic_bar_recovered_empty)
                    queued -> activity.getString(R.string.mic_bar_recovered_queued, title)
                    else -> activity.getString(R.string.mic_bar_recovered, title)
                }
                primary.text = activity.getString(R.string.mic_record)
                primary.isEnabled = true
                primary.alpha = 1f
            }
            else -> {
                val message = reply?.optString("message").orEmpty()
                status.text = message.ifEmpty { activity.getString(R.string.mic_bar_ready) }
                primary.text = activity.getString(R.string.mic_record)
                primary.isEnabled = true
                primary.alpha = 1f
            }
        }
    }

    private fun formatElapsed(seconds: Long): String {
        val minutes = seconds / 60
        val rest = seconds % 60
        return String.format(java.util.Locale.US, "%d:%02d", minutes, rest)
    }

    private fun dp(value: Int): Int =
        (value * activity.resources.displayMetrics.density).toInt()
}
