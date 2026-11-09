package dev.phonelink.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import androidx.core.content.ContextCompat

/** Reconnect automatically after a reboot or an app update, if a computer is paired. */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val relevant = intent.action == Intent.ACTION_BOOT_COMPLETED ||
            intent.action == Intent.ACTION_MY_PACKAGE_REPLACED
        if (relevant && LinkManager.hasPairedDevice(context) && Prefs.connection(context)) {
            ContextCompat.startForegroundService(context, Intent(context, LinkService::class.java))
        }
    }
}
