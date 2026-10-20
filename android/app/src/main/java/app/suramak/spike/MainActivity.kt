package app.suramak.spike

import android.os.Bundle
import android.view.View
import androidx.activity.OnBackPressedCallback
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import com.google.androidgamesdk.GameActivity
import com.google.androidgamesdk.gametextinput.State

/**
 * M13b shell spike — the entire Kotlin surface.
 *
 * Extends the androidx `GameActivity` (AGDK), which is what gives the Rust side
 * `GameTextInput` (the soft-keyboard path egui's IME support needs). This class
 * owns nothing visual: the UI is egui, drawn by `libsuramak_android_spike.so`.
 *
 * The JNI contract (four small functions on purpose):
 *   Kotlin -> Rust   nativeSetInsets(top, bottom, left, right, imeBottom)  [px]
 *   Kotlin -> Rust   nativeBackPressed()
 *   Kotlin -> Rust   nativeImeText(text, selectionStart, selectionEnd)     [UTF-16 indices]
 *   Rust -> Kotlin   postRecreate()   (posts Activity.recreate() on the UI thread)
 */
class MainActivity : GameActivity() {

    /** Cached so Rust's recreate request never touches a View off the UI thread. */
    private var root: View? = null

    private external fun nativeSetInsets(
        top: Int,
        bottom: Int,
        left: Int,
        right: Int,
        imeBottom: Int,
    )

    private external fun nativeBackPressed()

    private external fun nativeImeText(text: String, selectionStart: Int, selectionEnd: Int)

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
        nativeImeText(state.text, state.selectionStart, state.selectionEnd)
    }

    companion object {
        init {
            // Must run before GameActivity.onCreate calls initializeNativeCode():
            // class initialisation happens when the Activity is instantiated, so
            // the library is loaded in time.
            System.loadLibrary("suramak_android_spike")
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        root = window.decorView
        installInsetsListener()
        installBackHandler()
    }

    override fun onResume() {
        super.onResume()
        // The insets listener only fires when insets change; re-push on resume so
        // the Rust side always has a value after a surface/activity recreate.
        pushCurrentInsets()
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
        // the spike while the checks are on screen.
        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() {
                nativeBackPressed()
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
}
