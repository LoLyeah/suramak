package app.suramak.transcribe

import android.content.Context
import app.suramak.models.ModelsBridge
import java.io.File

/**
 * M13e — which installed model a shared file transcribes with.
 *
 * The rule is the M13d one: ask the model host for the device recommendation
 * (it probes RAM, ARM64 and the phone tiers) and take the first **installed**
 * model from that order. The host refuses an uninstalled model at
 * `begin_import`, so this is the only place the app chooses one.
 *
 * The table is the whisper.cpp pinned file name per model id. Only the tiers a
 * phone may run are listed (tiny/base/small — M13d's rules); a turbo id can
 * never be the default on a phone, so it is not a candidate here.
 */
object ModelChoice {

    private val FILE_NAMES = mapOf(
        "tiny" to "ggml-tiny.bin",
        "base" to "ggml-base.bin",
        "small" to "ggml-small.bin",
    )

    fun installed(context: Context, id: String): Boolean {
        val name = FILE_NAMES[id] ?: return false
        return File(File(context.filesDir, "models"), name).isFile
    }

    /**
     * The model for the next job, or `null` when none is installed (the caller
     * posts "download a model first" instead of starting a doomed job).
     */
    fun forJob(context: Context, language: String): String? {
        val reply = ModelsBridge.memory(context, language)
        val order = LinkedHashSet<String>()
        reply.optString("default_model").takeIf { it.isNotEmpty() }?.let(order::add)
        reply.optJSONArray("allowed")?.let { allowed ->
            for (index in 0 until allowed.length()) {
                order.add(allowed.optString(index))
            }
        }
        order.addAll(listOf("tiny", "base", "small"))
        return order.firstOrNull { installed(context, it) }
    }
}
