package app.suramak.models

import android.app.ActivityManager
import android.content.Context
import android.os.Build

/**
 * M13d — the device floor probe.
 *
 * The owner's floor is a 64-bit ARM phone with 4 GB of RAM. Android reports
 * **usable** memory, which is below the nominal figure on the box (a 4 GB phone
 * reports roughly 3.6 GB), so the tier thresholds live in Rust
 * (`suramak_core::device`) and this class only supplies the raw report.
 *
 * `ActivityManager.getMemoryInfo` is the platform's authoritative answer for
 * both values: `totalMem` (installed, minus kernel reservations) and
 * `lowMemory` (the OS's own "I am under pressure" flag, sampled at probe time).
 * `isLowRamDevice` is the OS's static classification and wins over the byte
 * count in the Rust tier rule.
 */
object DeviceMemory {

    /** One memory report, in the shape the Rust host expects. */
    data class Report(
        val totalBytes: Long,
        val availableBytes: Long,
        val lowMemory: Boolean,
        val arm64: Boolean,
    )

    /** Reads the current report. Safe to call from the main thread. */
    fun read(context: Context): Report {
        val manager = context.getSystemService(Context.ACTIVITY_SERVICE) as ActivityManager
        val info = ActivityManager.MemoryInfo()
        manager.getMemoryInfo(info)
        return Report(
            totalBytes = info.totalMem,
            availableBytes = info.availMem,
            lowMemory = manager.isLowRamDevice || info.lowMemory,
            // The APK ships arm64-v8a only, but the bridge must not assume it:
            // a 32-bit install would be refused by the Rust floor rule instead
            // of silently trying to run a model.
            arm64 = Build.SUPPORTED_64_BIT_ABIS.any { it == "arm64-v8a" },
        )
    }

    /**
     * M13g3 — free bytes on the volume the models live on (`filesDir`). The
     * first-run card shows this before a download and the M13D preflight is the
     * authority that refuses; this is only the number the card prints.
     */
    fun freeBytes(context: Context): Long {
        val stat = android.os.StatFs(context.filesDir.absolutePath)
        return stat.availableBytes
    }
}
