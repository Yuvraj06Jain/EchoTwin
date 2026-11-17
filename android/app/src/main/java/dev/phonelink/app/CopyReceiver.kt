package dev.phonelink.app

import android.content.BroadcastReceiver
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import androidx.core.app.NotificationManagerCompat

/** "Copy" on a note's notification: puts the note on this phone's clipboard. Not exported. */
class CopyReceiver : BroadcastReceiver() {
    companion object {
        const val EXTRA_TEXT = "text"
        const val EXTRA_NOTIFICATION = "notification"
    }

    override fun onReceive(context: Context, intent: Intent) {
        val text = intent.getStringExtra(EXTRA_TEXT) ?: return
        context.getSystemService(ClipboardManager::class.java)
            ?.setPrimaryClip(ClipData.newPlainText("note", text))
        val id = intent.getIntExtra(EXTRA_NOTIFICATION, -1)
        if (id >= 0) NotificationManagerCompat.from(context).cancel(id)
    }
}
