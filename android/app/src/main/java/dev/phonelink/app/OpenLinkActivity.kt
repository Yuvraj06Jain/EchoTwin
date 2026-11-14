package dev.phonelink.app

import android.app.Activity
import android.content.Intent
import android.os.Bundle

/** Share → "Open on computer": opens the shared web link in the computer's browser. */
class OpenLinkActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val text = intent?.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()
        if (text.isNullOrBlank()) LinkManager.toast("No web link to open") else LinkManager.openOnComputer(text)
        finish()
    }
}
