package app.suramak.spike

import android.content.Context
import android.os.Build
import android.os.Process
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.TimeZone

/**
 * The crash record, shared by Kotlin and Rust.
 *
 * Rust owns the format and the write path for Rust panics
 * (`crates/suramak-android-spike/src/diag.rs`); Kotlin uses the same files for
 * Java-side exceptions and for the "session ended without a shutdown" marker.
 * The two names must stay identical in both languages — the Rust side reads
 * [LAST_CRASH] at startup and shows its content on the egui banner.
 *
 * Writes are best-effort on purpose: this code runs while the process is
 * dying, and a diagnostics path that throws would replace the real failure with
 * its own.
 */
object CrashFiles {
    /** Newest crash record; replaced on every crash, deleted only by "Clear crash". */
    const val LAST_CRASH = "spike-last-crash.txt"

    /** Append-only history, rolled over by Rust when it grows past 256 KiB. */
    const val HISTORY = "spike-crash-log.txt"

    /** Exists while a session is *visible*; a leftover marker means it vanished. */
    const val SESSION = "spike-session.txt"

    private const val MAX_READ = 32 * 1024
    private const val MAX_STACK = 8 * 1024

    /** The newest crash record, or null when there is none. */
    fun lastCrashText(context: Context): String? = try {
        val file = File(context.filesDir, LAST_CRASH)
        if (!file.isFile || file.length() == 0L) {
            null
        } else {
            val text = file.readText()
            val clipped = if (text.length > MAX_READ) text.substring(text.length - MAX_READ) else text
            if (clipped.isBlank()) null else clipped
        }
    } catch (t: Throwable) {
        null
    }

    /** Record a Java-side fatal error (called from the uncaught handler). */
    fun writeFatal(context: Context, thread: Thread, error: Throwable) {
        val stack = error.stackTraceToString().let {
            if (it.length > MAX_STACK) it.substring(0, MAX_STACK) + "…" else it
        }
        val block = buildString {
            append("=== Suramak spike crash (java) ===\n")
            append("when:   ").append(utc()).append('\n')
            append("thread: ").append(thread.name).append('\n')
            append("where:  ").append(error.stackTrace.firstOrNull()?.toString() ?: "?").append('\n')
            append("panic:  ").append(error.javaClass.name).append(": ").append(error.message ?: "").append('\n')
            append("--- stack ---\n")
            append(stack).append('\n')
        }
        write(context, block)
    }

    /**
     * If the previous session left its marker behind, it disappeared while in
     * the foreground — a native crash (SIGSEGV/SIGABRT), an ANR kill, or a
     * `kill -9`. No Rust panic and no Java exception recorded it, so say so:
     * otherwise the next launch shows nothing and the failure looks random.
     *
     * A real recorded crash wins; the marker note is only written when there is
     * no newer record to show.
     */
    fun foldAbandonedSession(context: Context) {
        try {
            val marker = File(context.filesDir, SESSION)
            if (!marker.isFile) {
                return
            }
            val started = marker.readText().trim().ifEmpty { "unknown time" }
            marker.delete()
            if (lastCrashText(context) != null) {
                return
            }
            val block = buildString {
                append("=== Suramak spike crash (session marker) ===\n")
                append("when:   ").append(utc()).append(" (detected at launch)\n")
                append("thread: -\n")
                append("where:  previous process\n")
                append("panic:  the session started at ").append(started)
                append(" and vanished without onStop.\n")
                append("        No Rust panic and no Java exception were recorded for it:\n")
                append("        the process was killed natively (SIGSEGV/SIGABRT/ANR-kill).\n")
            }
            write(context, block)
        } catch (t: Throwable) {
            // Best effort only.
        }
    }

    /** Called from onCreate/onResume: this session is on screen. */
    fun beginSession(context: Context) {
        try {
            val text = "started ${utc()} · pid ${Process.myPid()} · " +
                "android ${Build.VERSION.RELEASE} (API ${Build.VERSION.SDK_INT}) · " +
                "${Build.MANUFACTURER} ${Build.MODEL}\n"
            File(context.filesDir, SESSION).writeText(text)
        } catch (t: Throwable) {
            // Best effort only.
        }
    }

    /** Called from onStop: the session left the foreground deliberately. */
    fun endSession(context: Context) {
        try {
            File(context.filesDir, SESSION).delete()
        } catch (t: Throwable) {
            // Best effort only.
        }
    }

    fun utc(): String = SimpleDateFormat("yyyy-MM-dd HH:mm:ss'Z'", Locale.US).apply {
        timeZone = TimeZone.getTimeZone("UTC")
    }.format(Date())

    private fun write(context: Context, block: String) {
        try {
            val dir = context.filesDir
            val last = File(dir, LAST_CRASH)
            val tmp = File(dir, "$LAST_CRASH.tmp")
            // Write + fsync + rename: a reader (the next launch, the egui
            // banner) never sees half a record.
            tmp.outputStream().use { out ->
                out.write(block.toByteArray())
                out.flush()
                out.fd.sync()
            }
            if (!tmp.renameTo(last)) {
                last.writeText(block)
                tmp.delete()
            }
            File(dir, HISTORY).appendText(block + "\n")
        } catch (t: Throwable) {
            // Best effort only.
        }
    }
}
