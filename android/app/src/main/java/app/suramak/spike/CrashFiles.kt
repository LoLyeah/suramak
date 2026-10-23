package app.suramak.spike

import android.content.Context
import android.os.Build
import android.os.Process
import android.util.Log
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
 * **Location (v2.1).** The files live in the app's *external* files dir —
 * `/sdcard/Android/data/app.suramak.spike/files/` — not the private one. That
 * is app-specific external storage (still no permission, still removed on
 * uninstall), but it is the one the phone's own Files app can browse, which is
 * the whole point: v2 wrote everything to `/data/data/…/files/`, which no file
 * manager can see. Copies left behind by v2 are moved on first launch by
 * [migrate]; [dir] falls back to the private dir if external storage is not
 * mounted, so a diagnostics path never throws over a missing volume.
 *
 * The banner's **Share log** button (`MainActivity`) hands the same file to the
 * system share sheet through a `FileProvider` — the path that works on every
 * OEM without a PC, and the one that does not depend on `Android/data` being
 * browseable at all ([AndroidManifest.xml] declares the provider,
 * `res/xml/file_paths.xml` says what may be shared).
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

    private const val TAG = "SuramakSpike"
    private const val MAX_READ = 32 * 1024
    private const val MAX_STACK = 8 * 1024

    /** The three files v2 kept in `context.filesDir`; migrated by [migrate]. */
    private val LEGACY_FILES = arrayOf(LAST_CRASH, HISTORY, SESSION)

    /**
     * Where the crash files live: the app's external files dir, or the private
     * dir when external storage is unavailable. Created if missing.
     *
     * `getExternalFilesDir` is per-app external storage — no permission, no
     * scoped-storage exemption, uninstalled with the app.
     */
    fun dir(context: Context): File {
        val external = try {
            context.getExternalFilesDir(null)
        } catch (t: Throwable) {
            null
        }
        val target = external ?: context.filesDir
        try {
            if (!target.isDirectory) {
                target.mkdirs()
            }
        } catch (t: Throwable) {
            // Fall through: the write path handles a missing dir best-effort.
        }
        return target
    }

    /**
     * True when [dir] is the browseable external one (false = private-dir
     * fallback, e.g. external storage not mounted).
     */
    fun isExternal(context: Context): Boolean = dir(context).absolutePath != context.filesDir.absolutePath

    /**
     * A short location for on-screen text: `…/Android/data/app.suramak.spike/files`
     * on a phone, the raw path in the fallback.
     */
    fun location(context: Context): String {
        val path = dir(context).absolutePath
        val marker = "/Android/data/"
        val index = path.indexOf(marker)
        return if (index >= 0) "…" + path.substring(index) else path
    }

    /**
     * v2 → v2.1: move what the private dir still holds into the external one.
     *
     * Called once per process from `SpikeApp.onCreate`, i.e. before any
     * Activity exists and before Rust is told where to write, so both the Rust
     * panic hook and the Kotlin overlay read the new location from the first
     * launch after the update. Returns a line for the event ring when something
     * actually moved; null when there was nothing to do.
     */
    fun migrate(context: Context): String? {
        return try {
            val internal = context.filesDir
            val external = context.getExternalFilesDir(null) ?: return null
            if (internal.absolutePath == external.absolutePath) {
                return null
            }
            if (!external.isDirectory && !external.mkdirs()) {
                return null
            }
            val moved = mutableListOf<String>()
            for (name in LEGACY_FILES) {
                val source = File(internal, name)
                if (!source.isFile) {
                    continue
                }
                val target = File(external, name)
                when {
                    // Nothing usable at the destination: take the whole file.
                    !target.isFile || target.length() == 0L -> {
                        if (target.exists()) {
                            target.delete()
                        }
                        if (move(source, target)) {
                            moved += name
                        }
                    }
                    // Both histories exist (a migration interrupted halfway):
                    // keep both, the external one is the newer tail.
                    name == HISTORY -> {
                        target.appendText(source.readText())
                        source.delete()
                        moved += "$name (appended)"
                    }
                    // The external record is the newer one; the private copy is stale.
                    else -> source.delete()
                }
            }
            if (moved.isEmpty()) {
                null
            } else {
                "crash files moved to ${external.absolutePath}: ${moved.joinToString()}"
            }
        } catch (t: Throwable) {
            Log.w(TAG, "crash-file migration failed", t)
            null
        }
    }

    /** Rename, falling back to copy + delete across mount points. */
    private fun move(source: File, target: File): Boolean {
        return try {
            if (source.renameTo(target)) {
                true
            } else {
                source.copyTo(target, overwrite = true)
                source.delete()
                true
            }
        } catch (t: Throwable) {
            Log.w(TAG, "could not move ${source.name} to ${target.absolutePath}", t)
            false
        }
    }

    /**
     * The file the crash banner is showing, for the share sheet — the same file
     * [lastCrashText] reads. Falls back to a leftover private-dir copy so a
     * failed migration cannot silently disable sharing.
     */
    fun shareableFile(context: Context): File? {
        val primary = File(dir(context), LAST_CRASH)
        if (primary.isFile && primary.length() > 0L) {
            return primary
        }
        val legacy = File(context.filesDir, LAST_CRASH)
        return if (legacy.isFile && legacy.length() > 0L) legacy else null
    }

    /** The newest crash record, or null when there is none. */
    fun lastCrashText(context: Context): String? {
        return read(File(dir(context), LAST_CRASH)) ?: read(File(context.filesDir, LAST_CRASH))
    }

    private fun read(file: File): String? = try {
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
            val marker = File(dir(context), SESSION)
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
            File(dir(context), SESSION).writeText(text)
        } catch (t: Throwable) {
            // Best effort only.
        }
    }

    /** Called from onStop: the session left the foreground deliberately. */
    fun endSession(context: Context) {
        try {
            File(dir(context), SESSION).delete()
        } catch (t: Throwable) {
            // Best effort only.
        }
    }

    fun utc(): String = SimpleDateFormat("yyyy-MM-dd HH:mm:ss'Z'", Locale.US).apply {
        timeZone = TimeZone.getTimeZone("UTC")
    }.format(Date())

    private fun write(context: Context, block: String) {
        try {
            val dir = dir(context)
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
