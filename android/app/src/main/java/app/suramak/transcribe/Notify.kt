package app.suramak.transcribe

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.graphics.drawable.Icon
import android.os.Build
import app.suramak.mic.RecordingService
import app.suramak.spike.MainActivity
import app.suramak.ui.QuitReceiver
import app.suramak.spike.R

/**
 * M13e — the two notification channels and the three shapes of notification.
 *
 * * **progress** (`transcription`): low importance, silent, ongoing, with a
 *   Cancel action. Shows the current file's integer percent and how many jobs
 *   wait behind it — the two numbers the Rust host reports and nothing else.
 * * **done** (`transcription_done`): default importance, one per finished
 *   batch; tapping it opens the transcript (the one landing path M13G keeps).
 * * status: reused for failures, "no model installed" and the paused notices —
 *   informational, auto-cancel.
 *
 * All icons are platform drawables: the app ships no custom notification art,
 * and a missing icon would crash the service where it cannot be debugged.
 */
object Notify {

    const val CHANNEL_PROGRESS = "transcription"
    const val CHANNEL_DONE = "transcription_done"
    const val CHANNEL_RECORDING = "recording"
    const val PROGRESS_ID = 4201
    const val STATUS_ID = 4202
    const val RECORDING_ID = 4301

    const val ACTION_CANCEL = "app.suramak.transcribe.CANCEL"
    const val ACTION_OPEN_TRANSCRIPT = "app.suramak.transcribe.OPEN_TRANSCRIPT"
    const val ACTION_STOP_RECORDING = "app.suramak.mic.STOP"
    const val ACTION_QUIT = "app.suramak.ui.QUIT"
    const val ACTION_OPEN_RECORDING = "app.suramak.mic.OPEN"
    const val EXTRA_KEY = "app.suramak.transcribe.KEY"
    const val EXTRA_TRANSCRIPT_ID = "app.suramak.transcribe.TRANSCRIPT_ID"
    /** M22B: the page the voice-note notification is showing (0-based). */
    const val ACTION_COPY = "app.suramak.transcribe.COPY"
    const val ACTION_NEXT_PAGE = "app.suramak.transcribe.NEXT_PAGE"
    const val ACTION_RETRY = "app.suramak.transcribe.RETRY"
    const val ACTION_SHARE_QUOTE = "app.suramak.transcribe.SHARE_QUOTE"
    const val EXTRA_PAGE = "app.suramak.transcribe.PAGE"
    const val EXTRA_SENDER = "app.suramak.transcribe.SENDER"
    const val EXTRA_NOTIFICATION_ID = "app.suramak.transcribe.NOTIFICATION_ID"

    fun ensureChannels(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java)
        if (manager.getNotificationChannel(CHANNEL_PROGRESS) == null) {
            manager.createNotificationChannel(
                NotificationChannel(
                    CHANNEL_PROGRESS,
                    context.getString(R.string.transcribe_channel),
                    NotificationManager.IMPORTANCE_LOW,
                ).apply {
                    description = context.getString(R.string.transcribe_channel_desc)
                    setShowBadge(false)
                },
            )
        }
        if (manager.getNotificationChannel(CHANNEL_DONE) == null) {
            manager.createNotificationChannel(
                NotificationChannel(
                    CHANNEL_DONE,
                    context.getString(R.string.transcribe_done_channel),
                    NotificationManager.IMPORTANCE_DEFAULT,
                ).apply {
                    description = context.getString(R.string.transcribe_done_channel_desc)
                },
            )
        }
        // M13F: the microphone foreground service's channel. Low importance and
        // silent: the persistent notification is a status surface, not an alert
        // (the recording itself is the user's action). The system still shows
        // it as an ongoing foreground-service notification.
        if (manager.getNotificationChannel(CHANNEL_RECORDING) == null) {
            manager.createNotificationChannel(
                NotificationChannel(
                    CHANNEL_RECORDING,
                    context.getString(R.string.mic_channel),
                    NotificationManager.IMPORTANCE_LOW,
                ).apply {
                    description = context.getString(R.string.mic_channel_desc)
                    setShowBadge(false)
                    setSound(null, null)
                },
            )
        }
    }

    /**
     * The progress notification. `key` is the job the Cancel action targets;
     * when it is null the action is omitted (nothing is running to cancel).
     */
    fun progress(
        context: Context,
        title: String,
        percent: Int,
        waiting: Int,
        key: String?,
    ): Notification {
        val builder = Notification.Builder(context, CHANNEL_PROGRESS)
            .setSmallIcon(android.R.drawable.stat_notify_sync)
            .setContentTitle(title)
            .setContentText(context.getString(R.string.transcribe_progress, percent, waiting))
            .setProgress(100, percent.coerceIn(0, 100), false)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setCategory(Notification.CATEGORY_PROGRESS)
        builder.addAction(quitAction(context))
        if (key != null) {
            val cancel = PendingIntent.getBroadcast(
                context,
                key.hashCode(),
                Intent(context, CancelTranscriptionReceiver::class.java)
                    .setAction(ACTION_CANCEL)
                    .putExtra(EXTRA_KEY, key),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
            builder.addAction(
                Notification.Action.Builder(
                    Icon.createWithResource(context, android.R.drawable.ic_menu_close_clear_cancel),
                    context.getString(R.string.transcribe_cancel),
                    cancel,
                ).build(),
            )
        }
        return builder.build()
    }

    /** One finished batch; the tap opens the transcript row. */
    fun completed(
        context: Context,
        title: String,
        segments: Int,
        transcriptId: String,
    ): Notification {
        val open = PendingIntent.getActivity(
            context,
            transcriptId.hashCode(),
            Intent(context, MainActivity::class.java)
                .setAction(ACTION_OPEN_TRANSCRIPT)
                .putExtra(EXTRA_TRANSCRIPT_ID, transcriptId)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        return Notification.Builder(context, CHANNEL_DONE)
            .setSmallIcon(android.R.drawable.ic_btn_speak_now)
            .setContentTitle(context.getString(R.string.transcribe_done_title))
            .setContentText(context.getString(R.string.transcribe_done_body, title, segments))
            .setContentIntent(open)
            .setAutoCancel(true)
            .build()
    }

    fun status(context: Context, title: String, body: String): Notification =
        Notification.Builder(context, CHANNEL_PROGRESS)
            .setSmallIcon(android.R.drawable.stat_notify_sync)
            .setContentTitle(title)
            .setContentText(body)
            .setAutoCancel(true)
            .build()

    /**
     * M13F — the persistent recording notification.
     *
     * A chronometer from `startedWallMs` is the elapsed-time display: the
     * system counts it, so the notification stays truthful while the app is
     * frozen behind a locked screen. `body` carries the last-known state
     * ("screen locked keeps it going", "Doze is active", …), which is what the
     * owner can read without unlocking the phone. The Stop action is a service
     * PendingIntent, so it works from the lock screen.
     */
    fun recording(
        context: Context,
        title: String,
        startedWallMs: Long,
        body: String,
    ): Notification {
        val builder = Notification.Builder(context, CHANNEL_RECORDING)
            .setSmallIcon(android.R.drawable.ic_btn_speak_now)
            .setContentTitle(context.getString(R.string.mic_recording_title))
            .setContentText(body)
            .setSubText(title)
            .setWhen(startedWallMs)
            .setShowWhen(true)
            .setUsesChronometer(true)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setCategory(Notification.CATEGORY_SERVICE)
            .setContentIntent(openRecording(context))
        if (Build.VERSION.SDK_INT >= 31) {
            builder.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
        }
        val stop = PendingIntent.getService(
            context,
            RECORDING_ID,
            Intent(context, RecordingService::class.java).setAction(ACTION_STOP_RECORDING),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        builder.addAction(
            Notification.Action.Builder(
                Icon.createWithResource(context, android.R.drawable.ic_menu_close_clear_cancel),
                context.getString(R.string.mic_stop),
                stop,
            ).build(),
        )
        builder.addAction(quitAction(context))
        return builder.build()
    }

    /** A one-shot recording outcome ("saved and queued", an interruption, …). */
    fun micStatus(context: Context, title: String, body: String): Notification =
        Notification.Builder(context, CHANNEL_DONE)
            .setSmallIcon(android.R.drawable.ic_btn_speak_now)
            .setContentTitle(title)
            .setContentText(body)
            .setStyle(Notification.BigTextStyle().bigText(body))
            .setAutoCancel(true)
            .build()

    /**
     * M13g1 — the persistent notification's Quit. A broadcast so it works with
     * no Activity alive (lock screen included); [QuitReceiver] executes the
     * owner's capture → jobs → services → exit order.
     */
    private fun quitAction(context: Context): Notification.Action {
        val quit = PendingIntent.getBroadcast(
            context,
            4999,
            Intent(context, QuitReceiver::class.java).setAction(ACTION_QUIT),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        return Notification.Action.Builder(
            Icon.createWithResource(context, android.R.drawable.ic_menu_close_clear_cancel),
            context.getString(R.string.quit_action),
            quit,
        ).build()
    }

    private fun openRecording(context: Context): PendingIntent =
        PendingIntent.getActivity(
            context,
            RECORDING_ID,
            Intent(context, MainActivity::class.java)
                .setAction(ACTION_OPEN_RECORDING)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

    /**
     * M22B — the live voice-note notification. The body is the queue's own
     * percent sentence (`Transcribing… 62%`), never partial text: whisper is
     * not a streaming engine, so the notification says only what is true.
     *
     * Cancel stays available for the whole run and targets the job key the
     * host reports.
     */
    fun voiceNoteProgress(
        context: Context,
        notificationId: Int,
        title: String,
        text: String,
        percent: Int,
        key: String?,
    ): Notification {
        val builder = Notification.Builder(context, CHANNEL_PROGRESS)
            .setSmallIcon(android.R.drawable.stat_notify_sync)
            .setContentTitle(title)
            .setContentText(text)
            .setProgress(100, percent.coerceIn(0, 100), false)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setCategory(Notification.CATEGORY_PROGRESS)
        if (key != null) {
            val cancel = PendingIntent.getBroadcast(
                context,
                notificationId,
                Intent(context, CancelTranscriptionReceiver::class.java)
                    .setAction(ACTION_CANCEL)
                    .putExtra(EXTRA_KEY, key),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
            builder.addAction(
                Notification.Action.Builder(
                    Icon.createWithResource(context, android.R.drawable.ic_menu_close_clear_cancel),
                    "Cancel",
                    cancel,
                ).build(),
            )
        }
        return builder.build()
    }

    /**
     * M22B — the finished transcript. BigText is the paging surface: the body
     * is exactly one page from the Rust model, the summary names the page, and
     * Copy always carries the whole cleaned transcript (the model's
     * `copy_text`, not the page).
     */
    fun voiceNoteReady(
        context: Context,
        notificationId: Int,
        transcriptId: String,
        content: org.json.JSONObject,
        copied: Boolean,
    ): Notification {
        val page = content.optInt("page", 0)
        val pageCount = content.optInt("page_count", 1).coerceAtLeast(1)
        val text = content.optString("text")
        val sender = content.optString("subtext").ifEmpty { null }
        val title = content.optString("title").ifEmpty { "Transcript ready" }
        val summary = if (copied) {
            "Copied · Page ${page + 1} of $pageCount"
        } else {
            "Page ${page + 1} of $pageCount"
        }
        val builder = Notification.Builder(context, CHANNEL_DONE)
            .setSmallIcon(android.R.drawable.ic_btn_speak_now)
            .setContentTitle(title)
            .setContentText(text.lineSequence().firstOrNull()?.take(160) ?: "")
            .setSubText(sender)
            .setNumber(page + 1)
            .setStyle(
                Notification.BigTextStyle()
                    .bigText(text)
                    .setSummaryText(summary),
            )
            .setContentIntent(openTranscript(context, transcriptId))
            .setAutoCancel(true)
            .setOnlyAlertOnce(true)

        builder.addAction(
            action(
                context,
                android.R.drawable.ic_menu_save,
                "Copy",
                broadcast(context, ACTION_COPY, 1, transcriptId, page, sender, notificationId),
            ),
        )
        if (pageCount > 1) {
            builder.addAction(
                action(
                    context,
                    android.R.drawable.ic_media_next,
                    "Next page",
                    broadcast(context, ACTION_NEXT_PAGE, 2, transcriptId, page, sender, notificationId),
                ),
            )
        }
        builder.addAction(
            action(
                context,
                android.R.drawable.ic_menu_edit,
                "Open editor",
                openTranscript(context, transcriptId),
            ),
        )
        builder.addAction(
            action(
                context,
                android.R.drawable.ic_menu_share,
                "Share quote card",
                PendingIntent.getActivity(
                    context,
                    notificationId * 16 + 4,
                    Intent(context, MainActivity::class.java)
                        .setAction(ACTION_SHARE_QUOTE)
                        .putExtra(EXTRA_TRANSCRIPT_ID, transcriptId)
                        .putExtra(EXTRA_PAGE, page)
                        .putExtra(EXTRA_SENDER, sender)
                        .addFlags(
                            Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP,
                        ),
                    PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
                ),
            ),
        )
        return builder.build()
    }

    /**
     * M22B — the named failure. The body is the queue's reason (the host
     * passes it verbatim); Retry re-queues the job whose copy was kept.
     */
    fun voiceNoteFailed(
        context: Context,
        notificationId: Int,
        title: String,
        reason: String,
        key: String,
        sender: String?,
    ): Notification {
        val builder = Notification.Builder(context, CHANNEL_DONE)
            .setSmallIcon(android.R.drawable.stat_notify_error)
            .setContentTitle(title)
            .setContentText(reason)
            .setStyle(Notification.BigTextStyle().bigText(reason))
            .setAutoCancel(true)
            .setOnlyAlertOnce(true)
        val retry = Intent(context, VoiceNoteActionReceiver::class.java)
            .setAction(ACTION_RETRY)
            .putExtra(EXTRA_KEY, key)
            .putExtra(EXTRA_SENDER, sender)
            .putExtra(EXTRA_NOTIFICATION_ID, notificationId)
        builder.addAction(
            action(
                context,
                android.R.drawable.ic_menu_revert,
                "Retry",
                PendingIntent.getBroadcast(
                    context,
                    notificationId * 16 + 5,
                    retry,
                    PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
                ),
            ),
        )
        return builder.build()
    }

    private fun action(
        context: Context,
        icon: Int,
        label: String,
        intent: PendingIntent,
    ): Notification.Action = Notification.Action.Builder(
        Icon.createWithResource(context, icon),
        label,
        intent,
    ).build()

    private fun broadcast(
        context: Context,
        action: String,
        offset: Int,
        transcriptId: String,
        page: Int,
        sender: String?,
        notificationId: Int,
    ): PendingIntent = PendingIntent.getBroadcast(
        context,
        notificationId * 16 + offset,
        Intent(context, VoiceNoteActionReceiver::class.java)
            .setAction(action)
            .putExtra(EXTRA_TRANSCRIPT_ID, transcriptId)
            .putExtra(EXTRA_PAGE, page)
            .putExtra(EXTRA_SENDER, sender)
            .putExtra(EXTRA_NOTIFICATION_ID, notificationId),
        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
    )

    /** The one landing path a transcript notification uses. */
    fun openTranscript(context: Context, transcriptId: String): PendingIntent =
        PendingIntent.getActivity(
            context,
            transcriptId.hashCode(),
            Intent(context, MainActivity::class.java)
                .setAction(ACTION_OPEN_TRANSCRIPT)
                .putExtra(EXTRA_TRANSCRIPT_ID, transcriptId)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

    fun show(context: Context, id: Int, notification: Notification) {
        context.getSystemService(NotificationManager::class.java).notify(id, notification)
    }

    fun clearProgress(context: Context) {
        context.getSystemService(NotificationManager::class.java).cancel(PROGRESS_ID)
    }

    /** Removes every notification this app owns (Quit's last visible act). */
    fun clearAll(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java)
        // The voice-note transcript notifications live on per-run ids, so the
        // three fixed ids are not enough; cancelAll covers every one.
        manager.cancelAll()
    }
}
