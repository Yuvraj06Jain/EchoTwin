package dev.phonelink.app

import android.content.Context

/** The phone app's own switches (stored privately; deleted with the app). */
object Prefs {
    private const val FILE = "settings"
    private const val CLIPBOARD = "clipboard_sharing"
    private const val QUEUE = "queue_notifications"
    private const val CONNECTION = "connection"
    private const val BATTERY_ALERTS = "battery_alerts"
    private const val PRESENT_APP = "present_app"
    private const val RECEIVE_FOLDER = "receive_folder"

    /** The folder received files go to (a document tree the user picked), or null: Download/EchoTwin. */
    fun receiveFolder(context: Context): String? = prefs(context).getString(RECEIVE_FOLDER, null)

    fun setReceiveFolder(context: Context, uri: String?) =
        prefs(context).edit().apply { if (uri == null) remove(RECEIVE_FOLDER) else putString(RECEIVE_FOLDER, uri) }.apply()
    private const val POINTER_SHAPE = "pointer_shape"
    private const val POINTER_COLOR = "pointer_color"
    private const val POINTER_SIZE = "pointer_size"

    /** The presentation pointer's look: shape (dot | ring | spotlight), 0xRRGGBB, size 0..2. */
    fun pointerStyle(context: Context): Triple<String, Int, Int> = prefs(context).let {
        Triple(it.getString(POINTER_SHAPE, "dot") ?: "dot", it.getInt(POINTER_COLOR, 0xFF3B30), it.getInt(POINTER_SIZE, 1))
    }

    fun setPointerStyle(context: Context, shape: String, color: Int, size: Int) =
        prefs(context).edit().putString(POINTER_SHAPE, shape).putInt(POINTER_COLOR, color).putInt(POINTER_SIZE, size).apply()

    /** Presentation app for the Start key: office | slides | canva | fullscreen. */
    fun presentApp(context: Context): String = prefs(context).getString(PRESENT_APP, "office") ?: "office"

    fun setPresentApp(context: Context, app: String) =
        prefs(context).edit().putString(PRESENT_APP, app).apply()

    private fun prefs(context: Context) = context.getSharedPreferences(FILE, Context.MODE_PRIVATE)

    /** Share the clipboard with the computer (on by default). */
    fun clipboardSharing(context: Context): Boolean = prefs(context).getBoolean(CLIPBOARD, true)

    fun setClipboardSharing(context: Context, on: Boolean) =
        prefs(context).edit().putBoolean(CLIPBOARD, on).apply()

    /** Keep notifications for a computer that's away and send them later (off by default). */
    fun queueNotifications(context: Context): Boolean = prefs(context).getBoolean(QUEUE, false)

    fun setQueueNotifications(context: Context, on: Boolean) =
        prefs(context).edit().putBoolean(QUEUE, on).apply()

    /** The connection switch (on by default). Off: nothing runs and nothing connects. */
    fun connection(context: Context): Boolean = prefs(context).getBoolean(CONNECTION, true)

    fun setConnection(context: Context, on: Boolean) =
        prefs(context).edit().putBoolean(CONNECTION, on).apply()

    /** "Computer battery low / fully charged" notifications (on by default). */
    fun batteryAlerts(context: Context): Boolean = prefs(context).getBoolean(BATTERY_ALERTS, true)

    fun setBatteryAlerts(context: Context, on: Boolean) =
        prefs(context).edit().putBoolean(BATTERY_ALERTS, on).apply()
}
