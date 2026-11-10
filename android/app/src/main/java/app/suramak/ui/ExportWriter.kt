package app.suramak.ui

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.activity.result.contract.ActivityResultContract
import androidx.core.content.FileProvider
import org.json.JSONObject
import java.io.File
import java.io.FileInputStream
import java.io.IOException

/**
 * M13g2 — the platform half of the compact export: the `ACTION_CREATE_DOCUMENT`
 * picker and the `ACTION_SEND` alternative.
 *
 * Rust renders and stages the file (`crates/suramak-android/src/export_job.rs`,
 * byte-identical to the desktop's M15 serializers) and hands Kotlin one path.
 * Kotlin never formats anything: it opens the destination the user picked,
 * copies the staged bytes through a bounded buffer while recomputing the
 * checksum Rust sent, and reports the byte count back. A truncated copy or a
 * provider that silently rewrote the stream is a failure, not a "saved".
 *
 * The share path hands the same staged file to the chooser through the app's
 * FileProvider (declared in the manifest; `file_paths.xml` covers `files/`).
 * It is disclosed in the sheet as the faster alternative — one file, no picker.
 */
object ExportWriter {

    /** The staged export Rust returned (`export` / `export_share`). */
    data class Staged(
        val job: String,
        val path: String,
        val name: String,
        val bytes: Long,
        val checksum: String,
        val mime: String,
        val bilingual: Boolean,
    ) {
        companion object {
            fun from(reply: JSONObject): Staged? {
                if (!reply.optBoolean("ok")) return null
                val job = reply.optString("job")
                val path = reply.optString("path")
                if (job.isEmpty() || path.isEmpty()) return null
                return Staged(
                    job = job,
                    path = path,
                    name = reply.optString("suggested_name"),
                    bytes = reply.optLong("bytes"),
                    checksum = reply.optString("checksum"),
                    mime = reply.optString("mime").ifEmpty { "application/octet-stream" },
                    bilingual = reply.optBoolean("bilingual"),
                )
            }
        }
    }

    /** The copy buffer. The same 16 KiB bound the Rust writer uses. */
    const val COPY_BUFFER = 16 * 1024

    private const val FNV_OFFSET = -3750763034362895579L // 0xcbf29ce484222325
    private const val FNV_PRIME = 1099511628211L // 0x100000001b3

    /**
     * `ACTION_CREATE_DOCUMENT` with a per-export MIME type and suggested name.
     *
     * `ActivityResultContracts.CreateDocument` fixes one MIME at registration;
     * the compact sheet exports six formats, so the contract takes the staged
     * export as its input instead.
     */
    class CreateExportDocument : ActivityResultContract<Staged, Uri?>() {
        override fun createIntent(context: Context, input: Staged): Intent =
            Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
                addCategory(Intent.CATEGORY_OPENABLE)
                type = input.mime
                putExtra(Intent.EXTRA_TITLE, input.name)
            }

        override fun parseResult(resultCode: Int, intent: Intent?): Uri? =
            if (resultCode == Activity.RESULT_OK) intent?.data else null
    }

    /**
     * Copies [staged] into the URI the picker returned.
     *
     * Returns the byte count written on success. Failure is a real failure:
     * no writable stream, a short/long write, or a checksum mismatch.
     */
    fun write(context: Context, uri: Uri, staged: Staged): Result<Long> = try {
        var written = 0L
        var hash = FNV_OFFSET
        val source = File(staged.path)
        if (!source.isFile) {
            throw IOException("the staged export is gone (${staged.path})")
        }
        val stream = context.contentResolver.openOutputStream(uri, "wt")
            ?: throw IOException("the destination opened no write stream")
        stream.use { output ->
            FileInputStream(source).use { input ->
                val buffer = ByteArray(COPY_BUFFER)
                while (true) {
                    val read = input.read(buffer)
                    if (read < 0) break
                    for (index in 0 until read) {
                        hash = (hash xor (buffer[index].toLong() and 0xffL)) * FNV_PRIME
                    }
                    output.write(buffer, 0, read)
                    written += read
                    if (written > staged.bytes) {
                        throw IOException("the staged export grew while copying")
                    }
                }
            }
            output.flush()
        }
        if (written != staged.bytes) {
            throw IOException("wrote $written bytes; the export is ${staged.bytes}")
        }
        val digest = String.format("%016x", hash)
        if (digest != staged.checksum) {
            throw IOException("checksum mismatch: wrote $digest, expected ${staged.checksum}")
        }
        Result.success(written)
    } catch (t: Throwable) {
        Result.failure(t)
    }

    /** The one-file share intent (the chooser is the caller's). */
    fun shareIntent(context: Context, staged: Staged): Intent {
        val authority = "${context.packageName}.fileprovider"
        val uri = FileProvider.getUriForFile(context, authority, File(staged.path))
        return Intent(Intent.ACTION_SEND).apply {
            type = staged.mime
            putExtra(Intent.EXTRA_STREAM, uri)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
    }
}
