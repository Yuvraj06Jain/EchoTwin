package dev.phonelink.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** "Lock computer" button on the PhoneLink notification (not reachable by other apps). */
class LockReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        LinkManager.lockComputer()
    }
}
