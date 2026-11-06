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
    const val STATUS_ID = 1
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
            .setContentTitle("PhoneLink")
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
