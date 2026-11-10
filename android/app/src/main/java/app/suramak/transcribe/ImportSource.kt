package app.suramak.transcribe

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import androidx.core.content.IntentCompat

/**
 * M13e — pulling the shared/picked file out of an intent.
 *
 * Both routes end in the same place: a `content://` URI the app may read once,
 * which the service copies into `files/media/<key>.<ext>` before anything else
 * touches it. That copy is what makes the job survive the granting task going
 * away, and it is why the decode never sees a SAF descriptor.
 *
 * Actions handled:
 *
 * * `ACTION_SEND` / `ACTION_SEND_MULTIPLE` — the share sheet (the manifest's
 *   mime matrix decides what reaches us: every `audio/` type, `application/ogg`,
 *   `application/x-ogg`, `audio/opus`, `audio/webm`, `video/webm`,
 *   `video/mp4`, `video/x-matroska`);
 * * `ACTION_OPEN_DOCUMENT` — the SAF picker, launched from the app's
 *   "Import audio" shortcut with every audio and video type.
 */
object ImportSource {

    /** Every readable URI an intent carries, de-duplicated, in order. */
    fun urisFrom(intent: Intent?): List<Uri> {
        if (intent == null) return emptyList()
        val found = LinkedHashSet<Uri>()
        when (intent.action) {
            Intent.ACTION_SEND -> {
                IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java)
                    ?.let(found::add)
            }
            Intent.ACTION_SEND_MULTIPLE, Intent.ACTION_OPEN_DOCUMENT -> {
                IntentCompat.getParcelableArrayListExtra(
                    intent,
                    Intent.EXTRA_STREAM,
                    Uri::class.java,
                )?.let(found::addAll)
            }
        }
        // Some share targets (and the picker contract) only set ClipData.
        intent.clipData?.let { clip ->
            for (index in 0 until clip.itemCount) {
                clip.getItemAt(index).uri?.let(found::add)
            }
        }
        return found.toList()
    }

    /** The name the sending app shows (`OpenableColumns.DISPLAY_NAME`). */
    fun displayName(context: Context, uri: Uri): String {
        val fallback = uri.lastPathSegment?.substringAfterLast('/') ?: "voice note"
        val cursor = context.contentResolver.query(uri, null, null, null, null) ?: return fallback
        cursor.use {
            val column = it.getColumnIndex(OpenableColumns.DISPLAY_NAME)
            if (column >= 0 && it.moveToFirst()) {
                val name = it.getString(column)
                if (!name.isNullOrBlank()) return name
            }
        }
        return fallback
    }

    /** The declared byte count, or 0 when the provider does not publish one. */
    fun sizeOf(context: Context, uri: Uri): Long {
        val cursor = context.contentResolver.query(uri, null, null, null, null) ?: return 0
        cursor.use {
            val column = it.getColumnIndex(OpenableColumns.SIZE)
            if (column >= 0 && it.moveToFirst() && !it.isNull(column)) {
                return it.getLong(column)
            }
        }
        return 0
    }

    /** The mime type, with the manifest's family as a fallback. */
    fun mimeOf(context: Context, uri: Uri): String =
        context.contentResolver.getType(uri) ?: "audio/*"
}
