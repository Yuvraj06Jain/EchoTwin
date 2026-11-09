package dev.phonelink.app

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat

object Notifications {
    private const val CHANNEL_STATUS = "status"
    private const val CHANNEL_MESSAGES = "messages"
    private const val CHANNEL_FILES = "files"
    const val STATUS_ID = 1
    private const val TRANSFERS_ID = 2
    private var nextMessageId = 100

    fun createChannels(context: Context) {
        val nm = context.getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_STATUS, "Connection status", NotificationManager.IMPORTANCE_LOW).apply {
                description = "Shows whether your computer is connected"
                setShowBadge(false)
            }
        )
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_MESSAGES, "Messages from your computer", NotificationManager.IMPORTANCE_DEFAULT)
        )
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_FILES, "Files", NotificationManager.IMPORTANCE_LOW).apply {
                description = "Files sent to and from your computer"
                setShowBadge(false)
            }
        )
    }

    private fun openApp(context: Context): PendingIntent = PendingIntent.getActivity(
        context, 0,
        Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
        PendingIntent.FLAG_IMMUTABLE,
    )

    /** What the buttons on the status notification can do right now. */
    data class Actions(val lock: Boolean = false, val clipboard: Boolean = false)

    fun status(context: Context, text: String, actions: Actions = Actions()): Notification {
        val b = NotificationCompat.Builder(context, CHANNEL_STATUS)
            .setSmallIcon(R.drawable.ic_link)
            .setContentTitle("EchoTwin")
            .setContentText(text)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(openApp(context))
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
        if (actions.clipboard) {
            val send = PendingIntent.getActivity(
                context, 1,
                Intent(context, ClipboardActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                PendingIntent.FLAG_IMMUTABLE,
            )
            b.addAction(R.drawable.ic_clipboard, "Send clipboard", send)
        }
        if (actions.lock) {
            val lock = PendingIntent.getBroadcast(
                context, 2,
                Intent(context, LockReceiver::class.java),
                PendingIntent.FLAG_IMMUTABLE,
            )
            b.addAction(R.drawable.ic_lock, "Lock computer", lock)
        }
        return b.build()
    }

    private fun allowed(context: Context) =
        ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) ==
            PackageManager.PERMISSION_GRANTED

    fun updateStatus(context: Context, text: String, actions: Actions = Actions()) {
        if (!allowed(context)) return
        NotificationManagerCompat.from(context).notify(STATUS_ID, status(context, text, actions))
    }

    /** One ongoing notification for all running transfers (or none). */
    fun transfers(context: Context, running: List<dev.phonelink.core.FileTransfer>) {
        if (!allowed(context)) return
        val nm = NotificationManagerCompat.from(context)
        if (running.isEmpty()) {
            nm.cancel(TRANSFERS_ID)
            return
        }
        val size = running.sumOf { it.size.toLong() }.coerceAtLeast(1)
        val done = running.sumOf { it.done.toLong() }
        val pct = (done * 100 / size).toInt()
        val active = running.any { it.state == "active" }
        val title = when {
            running.size == 1 -> (if (running[0].outgoing) "Sending " else "Receiving ") + running[0].name
            running.all { it.outgoing } -> "Sending ${running.size} files"
            running.none { it.outgoing } -> "Receiving ${running.size} files"
            else -> "${running.size} file transfers"
        }
        val eta = running.filter { it.state == "active" && it.rate > 0uL }.mapNotNull { it.etaSecs?.toLong() }.maxOrNull()
        val text = if (active) "$pct%" + (eta?.let { " · " + dev.phonelink.app.ui.formatEta(it) } ?: "")
        else running.first().message.ifBlank { "Waiting" }
        val n = NotificationCompat.Builder(context, CHANNEL_FILES)
            .setSmallIcon(R.drawable.ic_link)
            .setContentTitle(title)
            .setContentText(text)
            .setProgress(100, pct, !active && pct == 0)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setContentIntent(openApp(context))
            .build()
        nm.notify(TRANSFERS_ID, n)
    }

    /** A file arrived (tap opens it) or could not be sent. */
    fun fileResult(context: Context, id: String, title: String, text: String, open: Intent?) {
        if (!allowed(context)) return
        val tap = open?.let {
            PendingIntent.getActivity(context, id.hashCode(), it, PendingIntent.FLAG_IMMUTABLE)
        } ?: openApp(context)
        val n = NotificationCompat.Builder(context, CHANNEL_FILES)
            .setSmallIcon(R.drawable.ic_link)
            .setContentTitle(title)
            .setContentText(text)
            .setAutoCancel(true)
            .setContentIntent(tap)
            .build()
        NotificationManagerCompat.from(context).notify(1000 + (id.hashCode() and 0x3fffffff) % 100000, n)
    }

    fun message(context: Context, from: String, text: String) {
        if (!allowed(context)) return
        val n = NotificationCompat.Builder(context, CHANNEL_MESSAGES)
            .setSmallIcon(R.drawable.ic_link)
            .setContentTitle(from)
            .setContentText(text)
            .setStyle(NotificationCompat.BigTextStyle().bigText(text))
            .setAutoCancel(true)
            .setContentIntent(openApp(context))
            .build()
        NotificationManagerCompat.from(context).notify(nextMessageId++, n)
    }
}
