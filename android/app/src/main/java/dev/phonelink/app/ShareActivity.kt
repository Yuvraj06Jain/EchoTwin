package dev.phonelink.app

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle

/**
 * "EchoTwin" in Android's Share menu: sends the shared files to the computer, then closes.
 * Text shared without a file goes to the computer's clipboard instead.
 */
class ShareActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val uris = streams(intent)
        if (uris.isNotEmpty()) {
            // Opened now, while Android still lets us read them (see FileShare).
            LinkManager.sendFiles(uris, fromShare = true)
        } else {
            val text = intent?.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()
            if (!text.isNullOrBlank()) LinkManager.sendClipboard(text) else LinkManager.toast("Nothing to send")
        }
        finish()
    }

    @Suppress("DEPRECATION")
    private fun streams(i: Intent?): List<Uri> {
        if (i == null) return emptyList()
        return when (i.action) {
            Intent.ACTION_SEND -> listOfNotNull(
                if (Build.VERSION.SDK_INT >= 33) i.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
                else i.getParcelableExtra<Uri>(Intent.EXTRA_STREAM)
            )
            Intent.ACTION_SEND_MULTIPLE ->
                (if (Build.VERSION.SDK_INT >= 33) i.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
                else i.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM)).orEmpty().filterNotNull()
            else -> emptyList()
        }
    }
}
