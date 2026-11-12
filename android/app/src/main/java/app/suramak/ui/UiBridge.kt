package app.suramak.ui

import android.content.Context
import org.json.JSONObject

/**
 * M13g1 — the compact UI's one JNI call (`crates/suramak-android/src/compact_host.rs`).
 *
 * Same one-function JSON pattern as the model, transcription and microphone
 * bridges: a new operation is one `match` arm in Rust, never a new JNI symbol.
 * The host owns the real library store, `settings.toml` and the `CompactApp`
 * navigation state, so **the notification does not invent a route in Kotlin**:
 * it calls [openTranscript] and the host lands on Library → Editor with the row
 * loaded (or on Library with the honest "no longer in the library" line).
 *
 * `init` must run once before any other call; it is safe to call repeatedly
 * (the Rust side keeps one process-wide host).
 */
object UiBridge {

    init {
        System.loadLibrary("suramak_android")
    }

    private external fun nativeCommand(request: String): String

    fun init(context: Context): JSONObject =
        dispatch(JSONObject().put("op", "init").put("root", context.filesDir.absolutePath))

    /** The completion-notification landing: Library, then the transcript's editor. */
    fun openTranscript(id: String): JSONObject =
        dispatch(JSONObject().put("op", "open_transcript").put("transcript_id", id))

    /** The current route payload (`screen`, `depth`, `sheet`, `finish`, …). */
    fun route(): JSONObject = dispatch(JSONObject().put("op", "route"))

    /**
     * The platform back gesture's one question. `finish:true` means the compact
     * stack is at its root and the Activity should leave; anything else was
     * consumed by the UI (a sheet dismissed or a screen popped).
     */
    fun back(): JSONObject = dispatch(JSONObject().put("op", "back"))

    /** Re-read the library (after an import/transcription completes). */
    fun library(): JSONObject = dispatch(JSONObject().put("op", "library"))

    fun saveSegment(idx: Int, text: String): JSONObject =
        dispatch(JSONObject().put("op", "save_segment").put("idx", idx).put("text", text))

    fun setting(name: String, value: Any): JSONObject =
        dispatch(
            JSONObject()
                .put("op", "setting")
                .put("setting", name)
                .put("value", value),
        )

    fun device(
        report: app.suramak.models.DeviceMemory.Report,
        freeBytes: Long? = null,
        reduceMotion: Boolean? = null,
    ): JSONObject {
        val request = JSONObject()
            .put("op", "device")
            .put("total_ram_bytes", report.totalBytes)
            .put("available_ram_bytes", report.availableBytes)
            .put("low_memory", report.lowMemory)
            .put("arm64", report.arm64)
        freeBytes?.let { request.put("free_bytes", it) }
        reduceMotion?.let { request.put("reduce_motion", it) }
        return dispatch(request)
    }

    /**
     * Stages one export through the M15 serializer. The reply carries `job`,
     * `path`, `suggested_name`, `bytes`, `checksum` and `mime`; the platform
     * writes that one file and then calls [exportDone].
     *
     * `bilingual` adds the stacked translation (SRT/VTT only); `speakers`
     * prefixes each line with its speaker (SRT/VTT/Markdown only).
     */
    fun export(
        format: String,
        bilingual: Boolean = false,
        speakers: Boolean = false,
    ): JSONObject =
        dispatch(
            JSONObject()
                .put("op", "export")
                .put("format", format)
                .put("bilingual", bilingual)
                .put("speakers", speakers),
        )

    /** The `ACTION_SEND` alternative: the same staged bytes, one file. */
    fun exportShare(
        format: String,
        bilingual: Boolean = false,
        speakers: Boolean = false,
    ): JSONObject =
        dispatch(
            JSONObject()
                .put("op", "export_share")
                .put("format", format)
                .put("bilingual", bilingual)
                .put("speakers", speakers),
        )

    /** The picker's stream is closed and verified at [bytes]. */
    fun exportDone(job: String, bytes: Long): JSONObject =
        dispatch(JSONObject().put("op", "export_done").put("job", job).put("bytes", bytes))

    /** The chooser has the one-file URI (the staged file outlives the call). */
    fun exportShared(job: String): JSONObject =
        dispatch(JSONObject().put("op", "export_shared").put("job", job))

    /** The destination write failed; `error` is shown verbatim by the sheet. */
    fun exportFailed(job: String, error: String): JSONObject =
        dispatch(
            JSONObject()
                .put("op", "export_failed")
                .put("job", job)
                .put("error", error),
        )

    /** The picker/chooser was dismissed: drop the staged bytes. */
    fun exportCancel(job: String): JSONObject =
        dispatch(JSONObject().put("op", "export_cancel").put("job", job))

    /**
     * M22B — stages the page the notification is showing as a PNG quote card.
     * The reply carries `path`, `name`, `bytes`, `checksum`, `mime`, `page`
     * and `page_count`; the Activity hands that one file to `ACTION_SEND`.
     */
    fun quoteCard(transcriptId: String, page: Int, sender: String?): JSONObject {
        val request = JSONObject()
            .put("op", "quote_card")
            .put("transcript_id", transcriptId)
            .put("page", page)
        if (sender != null) request.put("sender", sender)
        return dispatch(request)
    }

    /**
     * M22B — arms the voice-note micro-sheet. `model` is empty when no speech
     * model is verified; `language` is the intent's explicit tag (or empty).
     * The reply: `{ok, auto, language, title, sender, reason}`.
     */
    fun voiceNoteOffer(
        title: String,
        mime: String,
        sender: String?,
        model: String,
        language: String,
    ): JSONObject {
        val request = JSONObject()
            .put("op", "voice_note_offer")
            .put("title", title)
            .put("mime", mime)
            .put("model", model)
            .put("language", language)
        if (sender != null) request.put("sender", sender)
        return dispatch(request)
    }

    /** The ordered plan (capture → jobs → services → exit); Kotlin executes it. */
    fun quit(): JSONObject = dispatch(JSONObject().put("op", "quit"))

    fun dispatch(request: JSONObject): JSONObject = try {
        JSONObject(nativeCommand(request.toString()))
    } catch (t: Throwable) {
        JSONObject()
            .put("ok", false)
            .put("error", t.message ?: t.javaClass.simpleName)
    }
}
