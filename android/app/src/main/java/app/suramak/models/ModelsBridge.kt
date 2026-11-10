package app.suramak.models

import android.content.Context
import org.json.JSONObject

/**
 * M13d — the Kotlin side of the model bridge.
 *
 * One JNI function carries every operation (`nativeCommand`); the protocol is
 * documented in `crates/suramak-android/src/model_host.rs` and every reply is a
 * JSON object. Keeping it to one symbol means the JNI surface cannot drift from
 * the Kotlin declarations feature by feature, and the whole host is testable on
 * the host without an emulator.
 *
 * The native library is `libsuramak_android.so`, built by
 * `scripts/android/build-apk.sh` and packaged next to the spike library.
 */
object ModelsBridge {

    init {
        System.loadLibrary("suramak_android")
    }

    private external fun nativeCommand(request: String): String

    /** Probes the device and returns the tier + recommendation for `language`. */
    fun memory(context: Context, language: String): JSONObject {
        val report = DeviceMemory.read(context)
        return call(
            JSONObject()
                .put("op", "memory")
                .put("total_ram_bytes", report.totalBytes)
                .put("available_ram_bytes", report.availableBytes)
                .put("low_memory", report.lowMemory)
                .put("arm64", report.arm64)
                .put("language", language),
        )
    }

    /** Whether this device may download `model` (the same rule `start` enforces). */
    fun plan(model: String, language: String): JSONObject = call(
        JSONObject().put("op", "plan").put("model", model).put("language", language),
    )

    /**
     * Starts the install. The reply is `{ok:false, refused:"…"}` when the phone
     * cannot run the model or the disk cannot hold it — in both cases nothing
     * has been downloaded.
     */
    fun start(model: String, language: String, modelsDir: String): JSONObject = call(
        JSONObject()
            .put("op", "start")
            .put("model", model)
            .put("language", language)
            .put("models_dir", modelsDir),
    )

    /** The current phase, bytes and deterministic percent. */
    fun progress(): JSONObject = call(JSONObject().put("op", "progress"))

    /** Asks the worker to stop; the `.part` stays and the next run resumes. */
    fun cancel(): JSONObject = call(JSONObject().put("op", "cancel"))

    /** Free-space preflight for `needed` bytes in `modelsDir`. */
    fun space(modelsDir: String, needed: Long): JSONObject = call(
        JSONObject().put("op", "space").put("models_dir", modelsDir).put("needed", needed),
    )

    /** The landing copy for the app's UI language (`id` → Indonesian). */
    fun landing(language: String): String =
        call(JSONObject().put("op", "landing").put("language", language)).optString("text")

    /**
     * One command round-trip. Any failure — a missing library, a native panic
     * reported as JSON, an unreadable reply — becomes `{ok:false,error:…}` so
     * callers never have to catch.
     */
    private fun call(request: JSONObject): JSONObject = try {
        JSONObject(nativeCommand(request.toString()))
    } catch (t: Throwable) {
        JSONObject()
            .put("ok", false)
            .put("error", t.message ?: t.javaClass.simpleName)
    }
}
