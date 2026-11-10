package app.suramak.mic

import android.content.Context
import org.json.JSONObject

/**
 * M13F — the one JNI call between the Kotlin recording surfaces and the Rust
 * mic host (`crates/suramak-android/src/mic.rs`).
 *
 * Same shape as [app.suramak.transcribe.TranscribeBridge] and
 * [app.suramak.models.ModelsBridge]: one `external fun` carrying a JSON request
 * and returning a JSON reply. Every reply is an object; a missing native
 * library becomes `{ok:false, error:...}` instead of crashing the service.
 *
 * **`init` must run after `TranscribeBridge.init`**: stopping a recording hands
 * its WAV to the M13E job inbox, so the job host has to exist first. The
 * Activity and the service both call it (idempotent on the Rust side).
 */
object MicBridge {

    init {
        System.loadLibrary("suramak_android")
    }

    private external fun nativeCommand(request: String): String

    fun init(context: Context): JSONObject =
        dispatch(JSONObject().put("op", "init").put("root", context.filesDir.absolutePath))

    /** `{phase, recording, elapsed_ms, bytes, last_stop:{...}, recovered:[...]}`. */
    fun status(): JSONObject = dispatch(JSONObject().put("op", "status"))

    /**
     * Starts a take. `foreground` and `permission` are Kotlin's answers to the
     * two Android rules; Rust refuses a start that does not claim them.
     */
    fun start(
        foreground: Boolean,
        permission: Boolean,
        title: String,
        model: String,
        language: String,
    ): JSONObject = dispatch(
        JSONObject()
            .put("op", "start")
            .put("foreground", foreground)
            .put("permission", permission)
            .put("title", title)
            .put("model", model)
            .put("language", language),
    )

    /** A clean stop. `reason` is one of the Rust host's spellings. */
    fun stop(reason: String): JSONObject =
        dispatch(JSONObject().put("op", "stop").put("reason", reason))

    /** A diagnostic line attached to the live take's journal entry. */
    fun note(text: String): JSONObject =
        dispatch(JSONObject().put("op", "note").put("text", text))

    /**
     * Quit's capture-first half: stops (and finalizes/hands off) any live take
     * synchronously and returns `{was_recording, handoff_key, next:[...]}`.
     * The caller must then stop the service and exit, in that order.
     */
    fun quit(): JSONObject = dispatch(JSONObject().put("op", "quit"))

    /** Recovered takes (a process kill during a recording, or a pending hand-off). */
    fun recover(): JSONObject = dispatch(JSONObject().put("op", "recover"))

    fun discard(id: String): JSONObject =
        dispatch(JSONObject().put("op", "discard").put("id", id))

    fun dispatch(request: JSONObject): JSONObject = try {
        JSONObject(nativeCommand(request.toString()))
    } catch (t: Throwable) {
        JSONObject()
            .put("ok", false)
            .put("error", t.message ?: t.javaClass.simpleName)
    }
}
