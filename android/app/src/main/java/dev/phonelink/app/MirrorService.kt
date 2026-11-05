package dev.phonelink.app

import android.app.Notification
import android.app.RemoteInput
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.Canvas
import android.os.Bundle
import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification
import androidx.core.app.NotificationCompat
import dev.phonelink.core.MirroredNotification
import java.io.ByteArrayOutputStream
import java.util.concurrent.ConcurrentHashMap

/**
 * Reads the phone's notifications (after you grant "Notification access") and mirrors them
 * to the paired computer. One-way: nothing from the computer is ever shown here, except
 * the requests below about notifications this phone sent (reply, button, dismiss).
 *
 * Android runs this service while access is granted; it needs no camera, microphone or
 * any other sensor.
 */
class MirrorService : NotificationListenerService() {

    companion object {
        @Volatile
        var instance: MirrorService? = null
            private set

        private const val MAX_BUTTONS = 3
        private const val ICON_PX = 48
    }

    /** Apps whose icon was already sent this session. */
    private val iconsSent = ConcurrentHashMap.newKeySet<String>()

    override fun onListenerConnected() {
        instance = this
        LinkManager.onMirrorReady()
    }

    override fun onListenerDisconnected() {
        if (instance === this) instance = null
    }

    override fun onDestroy() {
        if (instance === this) instance = null
        super.onDestroy()
    }

    override fun onNotificationPosted(sbn: StatusBarNotification) {
        val m = toMirrored(sbn) ?: return
        LinkManager.mirrorPosted(m)
        sendIconOnce(sbn.packageName)
    }

    override fun onNotificationRemoved(sbn: StatusBarNotification) {
        if (isMirrorable(sbn)) LinkManager.mirrorRemoved(sbn.key)
    }

    /** Everything currently shown, for a computer that just (re)connected. */
    fun snapshot(): List<MirroredNotification> {
        val active = try {
            activeNotifications ?: emptyArray()
        } catch (e: SecurityException) {
            emptyArray() // access was revoked meanwhile
        }
        active.forEach { sendIconOnce(it.packageName) }
        return active.mapNotNull(::toMirrored).sortedByDescending { it.postedMs }.take(100)
    }

    // ------------------------------------------------------------------ requests from the computer

    fun dismiss(key: String): Result<String> = runCatching {
        cancelNotification(key)
        "Cleared on your phone"
    }

    fun reply(key: String, text: String): Result<String> = runCatching {
        val sbn = find(key) ?: error("That notification is no longer on your phone")
        val action = replyAction(sbn.notification) ?: error("This app doesn't allow replies from notifications")
        val inputs = action.remoteInputs
        val results = Bundle().apply { inputs.forEach { putCharSequence(it.resultKey, text) } }
        val intent = Intent()
        RemoteInput.addResultsToIntent(inputs, intent, results)
        action.actionIntent.send(this, 0, intent)
        "Reply sent"
    }

    fun press(key: String, index: Int): Result<String> = runCatching {
        val sbn = find(key) ?: error("That notification is no longer on your phone")
        val action = buttons(sbn.notification).getOrNull(index) ?: error("That button is no longer available")
        action.actionIntent.send()
        "Done"
    }

    // ------------------------------------------------------------------ reading notifications

    private fun find(key: String): StatusBarNotification? =
        try {
            activeNotifications?.firstOrNull { it.key == key }
        } catch (e: SecurityException) {
            null
        }

    /** Noise is skipped: our own, ongoing (music, navigation, downloads), group summaries. */
    private fun isMirrorable(sbn: StatusBarNotification): Boolean {
        if (sbn.packageName == packageName) return false
        if (!sbn.isClearable) return false
        val flags = sbn.notification.flags
        if (flags and Notification.FLAG_ONGOING_EVENT != 0) return false
        if (flags and Notification.FLAG_FOREGROUND_SERVICE != 0) return false
        if (flags and Notification.FLAG_GROUP_SUMMARY != 0) return false
        return true
    }

    private fun replyAction(n: Notification): Notification.Action? =
        n.actions?.firstOrNull { a -> a.remoteInputs?.any { it.allowFreeFormInput } == true }

    /** Ordinary buttons (not the reply box), in the order the computer shows them. */
    private fun buttons(n: Notification): List<Notification.Action> =
        n.actions?.filter { it.remoteInputs.isNullOrEmpty() }?.take(MAX_BUTTONS) ?: emptyList()

    /** Latest lines of a chat (WhatsApp, Telegram, Messages…), "Sender: text". */
    private fun messagingText(n: Notification): String? {
        val style = NotificationCompat.MessagingStyle.extractMessagingStyleFromNotification(n) ?: return null
        val messages = style.messages
        if (messages.isEmpty()) return null
        return messages.takeLast(3).joinToString("\n") { m ->
            val who = m.person?.name
            if (who.isNullOrBlank()) m.text.toString() else "$who: ${m.text}"
        }
    }

    private fun toMirrored(sbn: StatusBarNotification): MirroredNotification? {
        if (!isMirrorable(sbn)) return null
        val n = sbn.notification
        val extras = n.extras
        val title = (extras.getCharSequence(Notification.EXTRA_TITLE)
            ?: extras.getCharSequence(Notification.EXTRA_CONVERSATION_TITLE))?.toString()?.trim().orEmpty()
        val text = (messagingText(n)
            ?: extras.getCharSequence(Notification.EXTRA_BIG_TEXT)?.toString()
            ?: extras.getCharSequence(Notification.EXTRA_TEXT)?.toString()
            ?: extras.getCharSequence(Notification.EXTRA_SUB_TEXT)?.toString())?.trim().orEmpty()
        if (title.isEmpty() && text.isEmpty()) return null
        return MirroredNotification(
            key = sbn.key,
            `package` = sbn.packageName,
            app = appName(sbn.packageName),
            title = title,
            text = text,
            postedMs = sbn.postTime.toULong(),
            canReply = replyAction(n) != null,
            actions = buttons(n).map { it.title?.toString().orEmpty() },
        )
    }

    private fun appName(pkg: String): String =
        try {
            packageManager.getApplicationLabel(packageManager.getApplicationInfo(pkg, 0)).toString()
        } catch (e: Exception) {
            pkg.substringAfterLast('.').replaceFirstChar { it.uppercase() }
        }

    private fun sendIconOnce(pkg: String) {
        if (pkg == packageName || !iconsSent.add(pkg)) return
        val png = try {
            val drawable = packageManager.getApplicationIcon(pkg)
            val bitmap = Bitmap.createBitmap(ICON_PX, ICON_PX, Bitmap.Config.ARGB_8888)
            drawable.setBounds(0, 0, ICON_PX, ICON_PX)
            drawable.draw(Canvas(bitmap))
            ByteArrayOutputStream().use { out ->
                bitmap.compress(Bitmap.CompressFormat.PNG, 100, out)
                out.toByteArray()
            }
        } catch (e: Exception) {
            iconsSent.remove(pkg)
            return
        }
        LinkManager.mirrorIcon(pkg, png)
    }
}
