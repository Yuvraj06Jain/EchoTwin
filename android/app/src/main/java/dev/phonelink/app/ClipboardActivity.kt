package dev.phonelink.app

import android.app.Activity
import android.content.ClipboardManager
import android.content.Intent
import android.os.Bundle

/**
 * Sends text to the computer's clipboard, then closes at once.
 *
 * * Share → "Copy to computer" on any text: sends the shared text.
 * * "Send clipboard" (notification button / Quick Settings tile): sends what is on this
 *   phone's clipboard. Android lets only the app in front read the clipboard, so this
 *   invisible screen comes to the front for a moment and reads it once it has focus.
 */
class ClipboardActivity : Activity() {
    private var done = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val shared = intent?.takeIf { it.action == Intent.ACTION_SEND }
            ?.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()
        if (shared != null) send(shared)
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (!hasFocus || done) return
        val clip = getSystemService(ClipboardManager::class.java)?.primaryClip
        val text = clip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(this)?.toString()
        if (text.isNullOrBlank()) {
            LinkManager.toast("The clipboard is empty")
            finishNow()
        } else {
            send(text)
        }
    }

    private fun send(text: String) {
        done = true
        LinkManager.sendClipboard(text)
        finishNow()
    }

    private fun finishNow() {
        done = true
        finish()
        overridePendingTransition(0, 0)
    }
}
