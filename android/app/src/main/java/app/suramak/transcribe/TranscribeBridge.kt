package app.suramak.transcribe

import android.content.Context
import org.json.JSONObject

/**
 * M13e — the one JNI call between the Kotlin transcription surfaces and the Rust
 * job host (`crates/suramak-android/src/jobs.rs`).
 *
 * Same shape as [app.suramak.models.ModelsBridge]: one `external fun` carrying a
 * JSON request and returning a JSON reply, so a new operation is one `match` arm
 * in Rust rather than a new JNI symbol. Every reply is an object; a missing
 * native library becomes `{ok:false, error:...}` instead of a crash, because the
 * service must never die on a bridge problem.
 *
 * **`init` must run first** (Activity and Service `onCreate`): the host needs the
 * app's `filesDir` before it can own `media/`, `jobs/journal.json` and the
 * library. Any other call before `init` fails closed.
 */
object TranscribeBridge {

    init {
        System.loadLibrary("suramak_android")
    }

    private external fun nativeCommand(request: String): String

    fun init(context: Context): JSONObject =
        dispatch(JSONObject().put("op", "init").put("root", context.filesDir.absolutePath))

    /** Creates the journal entry and returns `{key, media_path, title}`. */
    fun beginImport(
        name: String,
        mime: String,
        bytesTotal: Long,
        model: String,
        language: String,
    ): JSONObject = dispatch(
        JSONObject()
            .put("op", "begin_import")
            .put("name", name)
            .put("mime", mime)
            .put("bytes_total", bytesTotal)
            .put("model", model)
            .put("language", language),
    )

    /** One bounded progress report (the service calls this every 256 KiB). */
    fun progress(key: String, bytes: Long): JSONObject =
        dispatch(JSONObject().put("op", "import_progress").put("key", key).put("bytes", bytes))

    fun importDone(key: String): JSONObject =
        dispatch(JSONObject().put("op", "import_done").put("key", key))

    fun importFailed(key: String, reason: String): JSONObject =
        dispatch(JSONObject().put("op", "import_failed").put("key", key).put("reason", reason))

    /** `{running, percent, waiting, done, failed, cancelled, last:{...}}`. */
    fun status(): JSONObject = dispatch(JSONObject().put("op", "status"))

    /** The user's cancel: the copied file goes away and no row is written. */
    fun cancel(key: String): JSONObject =
        dispatch(JSONObject().put("op", "cancel").put("key", key))

    /** The system's stop (`onTimeout`, `onStopJob`): the copy stays for resume. */
    fun pause(key: String): JSONObject = dispatch(JSONObject().put("op", "pause").put("key", key))

    fun pauseAll(): JSONObject = dispatch(JSONObject().put("op", "pause_all"))

    /** The launch-time prompt: `{jobs:[{key,title,resumable,reason,percent}]}`. */
    fun recover(): JSONObject = dispatch(JSONObject().put("op", "recover"))

    fun resume(key: String): JSONObject = dispatch(JSONObject().put("op", "resume").put("key", key))

    fun discard(key: String): JSONObject = dispatch(JSONObject().put("op", "discard").put("key", key))

    /** The completion notification's landing: the library row, when it exists. */
    fun openTranscript(id: String): JSONObject =
        dispatch(JSONObject().put("op", "open").put("transcript_id", id))

    /**
     * The foreground-service type for an API level, selected in Rust (the rule
     * is unit-tested there) and mapped to the platform constant here.
     */
    fun foregroundType(api: Int): JSONObject =
        dispatch(JSONObject().put("op", "foreground_type").put("api", api))

    fun dispatch(request: JSONObject): JSONObject = try {
        JSONObject(nativeCommand(request.toString()))
    } catch (t: Throwable) {
        JSONObject()
            .put("ok", false)
            .put("error", t.message ?: t.javaClass.simpleName)
    }
}
