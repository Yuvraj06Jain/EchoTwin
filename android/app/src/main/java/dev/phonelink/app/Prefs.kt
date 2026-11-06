package dev.phonelink.app

import android.content.Context

/** The phone app's own switches (stored privately; deleted with the app). */
object Prefs {
    private const val FILE = "settings"
    private const val CLIPBOARD = "clipboard_sharing"

    private fun prefs(context: Context) = context.getSharedPreferences(FILE, Context.MODE_PRIVATE)

    /** Share the clipboard with the computer (on by default). */
    fun clipboardSharing(context: Context): Boolean = prefs(context).getBoolean(CLIPBOARD, true)

    fun setClipboardSharing(context: Context, on: Boolean) =
        prefs(context).edit().putBoolean(CLIPBOARD, on).apply()
}
