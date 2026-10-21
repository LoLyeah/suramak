package app.suramak.spike

import android.graphics.Typeface
import android.os.Bundle
import android.util.Log
import android.util.TypedValue
import android.view.Gravity
import android.view.SurfaceHolder
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import androidx.activity.OnBackPressedCallback
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import com.google.androidgamesdk.GameActivity
import com.google.androidgamesdk.gametextinput.State

/**
 * M13b shell spike v2 — the entire Kotlin surface.
 *
 * Extends the androidx `GameActivity` (AGDK), which is what gives the Rust side
 * `GameTextInput` (the soft-keyboard path egui's IME support needs). This class
 * owns almost nothing visual: the UI is egui, drawn by
 * `libsuramak_android_spike.so`. The one exception is the crash overlay — a
 * plain `TextView` stack added over the surface, because it has to survive the
 * Rust side dying.
 *
 * The JNI contract:
 *   Kotlin -> Rust   nativeInit(filesDir)                         [once, pre-super]
 *   Kotlin -> Rust   nativeSetInsets(top, bottom, left, right, imeBottom)  [px]
 *   Kotlin -> Rust   nativeBackPressed()
 *   Kotlin -> Rust   nativeImeText(text, selectionStart, selectionEnd)     [UTF-16 indices]
 *   Kotlin -> Rust   nativeLogLine(line)        [lifecycle lines for the on-screen ring]
 *   Kotlin -> Rust   nativeActivityDestroyed()  [stop the event loop]
 *   Rust -> Kotlin   postRecreate()             [posts Activity.recreate()]
 *   Rust -> Kotlin   postShowCrashOverlay()     [re-read the crash file, show it now]
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

    private external fun nativeInit(filesDir: String)

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
            // the library is loaded in time.
            System.loadLibrary("suramak_android_spike")
        }

        private const val TAG = "SuramakSpike"
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
        // `super.onCreate`, which is what starts the native thread.
        try {
            nativeInit(filesDir.absolutePath)
        } catch (t: Throwable) {
            Log.e(TAG, "nativeInit failed", t)
        }

        super.onCreate(savedInstanceState)
        root = window.decorView
        installInsetsListener()
        installBackHandler()
        showCrashOverlayIfPresent("launch", force = false)
    }

    override fun onStart() {
        super.onStart()
        logToRust("onStart")
    }

    override fun onResume() {
        super.onResume()
        logToRust("onResume")
        // Mark the session as visible again (onStop removes the marker) so a
        // native crash in the foreground still leaves a trace.
        CrashFiles.beginSession(this)
        // The insets listener only fires when insets change; re-push on resume so
        // the Rust side always has a value after a surface/activity recreate.
        pushCurrentInsets()
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
        CrashFiles.endSession(this)
        super.onStop()
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
    }

    private fun installInsetsListener() {
        val view = window.decorView
        ViewCompat.setOnApplyWindowInsetsListener(view) { _, insets ->
            pushInsets(insets)
            insets
        }
        ViewCompat.requestApplyInsets(view)
    }

    private fun pushCurrentInsets() {
        val view = root ?: return
        val insets = ViewCompat.getRootWindowInsets(view) ?: return
        pushInsets(insets)
    }

    private fun pushInsets(insets: WindowInsetsCompat) {
        val bars = insets.getInsets(
            WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout()
        )
        val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
        nativeSetInsets(bars.top, bars.bottom, bars.left, bars.right, ime.bottom)
    }

    private fun installBackHandler() {
        // Enabled for the whole lifetime of this Activity: back must never exit
        // the spike while the checks are on screen — unless the native side is
        // dead, in which case leaving is the only useful thing back can do.
        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() {
                if (nativePanicked) {
                    logToRust("back: native side is dead → finish()")
                    finish()
                } else {
                    nativeBackPressed()
                }
            }
        })
    }

    /**
     * Called from Rust over JNI (on the android_main thread). `View.post` is
     * thread-safe, so the Activity itself is only touched on the UI thread.
     */
    @Suppress("unused") // referenced from platform.rs via JNI
    fun postRecreate() {
        root?.post { recreate() }
    }

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
                    "files/${CrashFiles.LAST_CRASH} (history: files/${CrashFiles.HISTORY}).\n" +
                        "Clear it with the app's \"Clear crash\" button. Screenshot this."
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
