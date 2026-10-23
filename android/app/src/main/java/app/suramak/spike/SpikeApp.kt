package app.suramak.spike

import android.app.Application
import android.util.Log

/**
 * Installs the Java-side crash recorder before any Activity exists.
 *
 * The spike's own failures are mostly native (Rust panics on the `android_main`
 * thread, recorded by `diag.rs`), but a `java.lang.*` exception — a JNI
 * `UnsatisfiedLinkError`, an `Activity` lifecycle throw, an OOM — closes the
 * app just as abruptly and would leave nothing on screen. The default handler
 * writes the same `spike-last-crash.txt` the Rust hook writes, so the next
 * launch explains either kind of death in one place, then hands the exception
 * back to the platform's own handler so logcat and the system crash dialog keep
 * working.
 */
class SpikeApp : Application() {
    override fun onCreate() {
        super.onCreate()
        // v2 → v2.1: get the crash files out of /data/data/…/files (invisible to
        // every file manager) and into Android/data/…/files, before any Activity
        // or native init can read or write them.
        CrashFiles.migrate(this)?.let { Log.i(TAG, it) }
        val previous = Thread.getDefaultUncaughtExceptionHandler()
        Thread.setDefaultUncaughtExceptionHandler { thread, error ->
            try {
                CrashFiles.writeFatal(this, thread, error)
            } catch (t: Throwable) {
                Log.e(TAG, "could not record the fatal exception", t)
            }
            if (previous != null) {
                previous.uncaughtException(thread, error)
            } else {
                Log.e(TAG, "uncaught on ${thread.name}", error)
                android.os.Process.killProcess(android.os.Process.myPid())
            }
        }
    }

    private companion object {
        const val TAG = "SuramakSpike"
    }
}
